use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use futures_util::FutureExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::diagnostics::BackendLog;
use crate::settings::{self, TaskSchedulerPreferences};
use crate::translation_tasks::{
    self, RunMode, TranslationInterrupt, TranslationProgressPayload, TranslationTaskStatus,
    TranslationTaskView,
};

const TASK_STATUS_CHANGED_EVENT: &str = "task-status-changed";

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: String,
    pub mode: RunMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SchedulerAction {
    Enqueue { task_id: String },
    EnqueueBatch { task_ids: Vec<String> },
    Retranslate { task_id: String },
    RetranslateBatch { task_ids: Vec<String> },
    Pause { task_id: String },
    PauseAll,
    SetConcurrency { max_active_tasks: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerAck {
    pub success: bool,
    pub message: Option<String>,
}

impl SchedulerAck {
    fn accepted() -> Self {
        Self {
            success: true,
            message: None,
        }
    }

    fn rejected(message: String) -> Self {
        Self {
            success: false,
            message: Some(message),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActiveTask {
    pub cancellation_token: CancellationToken,
    interrupt: TranslationInterrupt,
}

#[derive(Debug)]
pub struct TaskCompletion {
    pub task_id: String,
    pub panic_message: Option<String>,
}

pub enum SchedulerInstruction {
    Dispatch {
        action: SchedulerAction,
        reply: oneshot::Sender<Result<SchedulerAck, String>>,
    },
    TaskFinished(TaskCompletion),
}

#[derive(Clone)]
pub struct TaskScheduler {
    sender: mpsc::Sender<SchedulerInstruction>,
}

#[derive(Clone)]
pub struct TaskSchedulerContext {
    pub app: AppHandle,
    pub provider_pool: SqlitePool,
    pub config_pool: SqlitePool,
    pub settings_pool: SqlitePool,
    pub glossary_config_pool: SqlitePool,
    pub glossary_workspace_root: PathBuf,
    pub workspace_root: PathBuf,
    pub client: Client,
}

struct TaskSchedulerWorker {
    context: TaskSchedulerContext,
    sender: mpsc::Sender<SchedulerInstruction>,
    receiver: mpsc::Receiver<SchedulerInstruction>,
    waiting_tasks: VecDeque<ScheduledTask>,
    queued_task_ids: HashSet<String>,
    active_tasks: HashMap<String, ActiveTask>,
    max_active_tasks: usize,
    backend_log: Option<BackendLog>,
}

enum DispatchError {
    Rejected(String),
    Infrastructure(String),
}

type DispatchResult<T> = Result<T, DispatchError>;

impl TaskScheduler {
    pub fn start(context: TaskSchedulerContext, max_active_tasks: usize) -> Self {
        let (sender, receiver) = mpsc::channel(128);
        let scheduler = Self {
            sender: sender.clone(),
        };
        let worker = TaskSchedulerWorker {
            backend_log: BackendLog::from_app(&context.app).ok(),
            context,
            sender,
            receiver,
            waiting_tasks: VecDeque::new(),
            queued_task_ids: HashSet::new(),
            active_tasks: HashMap::new(),
            max_active_tasks: max_active_tasks.clamp(1, 4),
        };
        tauri::async_runtime::spawn(worker.run());
        scheduler
    }

    pub async fn dispatch(&self, action: SchedulerAction) -> Result<SchedulerAck, String> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(SchedulerInstruction::Dispatch { action, reply })
            .await
            .map_err(|_| "Task scheduler is unavailable".to_string())?;
        response
            .await
            .map_err(|_| "Task scheduler stopped before replying".to_string())?
    }
}

impl TaskSchedulerWorker {
    async fn run(mut self) {
        while let Some(instruction) = self.receiver.recv().await {
            match instruction {
                SchedulerInstruction::Dispatch { action, reply } => {
                    let result = match self.dispatch_action(action).await {
                        Ok(()) => Ok(SchedulerAck::accepted()),
                        Err(DispatchError::Rejected(error)) => Ok(SchedulerAck::rejected(error)),
                        Err(DispatchError::Infrastructure(error)) => Err(error),
                    };
                    let _ = reply.send(result);
                }
                SchedulerInstruction::TaskFinished(completion) => {
                    self.handle_task_finished(completion).await;
                }
            }
            self.start_ready_tasks();
        }
    }

    async fn dispatch_action(&mut self, action: SchedulerAction) -> DispatchResult<()> {
        match action {
            SchedulerAction::Enqueue { task_id } => self.enqueue_ids(vec![task_id], false).await,
            SchedulerAction::EnqueueBatch { task_ids } => self.enqueue_ids(task_ids, false).await,
            SchedulerAction::Retranslate { task_id } => self.enqueue_ids(vec![task_id], true).await,
            SchedulerAction::RetranslateBatch { task_ids } => {
                self.enqueue_ids(task_ids, true).await
            }
            SchedulerAction::Pause { task_id } => self.pause_task(&task_id).await,
            SchedulerAction::PauseAll => self.pause_all().await,
            SchedulerAction::SetConcurrency { max_active_tasks } => {
                if !(1..=4).contains(&max_active_tasks) {
                    return Err(DispatchError::Rejected(
                        "Maximum active tasks must be between 1 and 4".into(),
                    ));
                }
                settings::update_task_scheduler_preferences(
                    &self.context.settings_pool,
                    TaskSchedulerPreferences { max_active_tasks },
                )
                .await
                .map_err(DispatchError::Infrastructure)?;
                self.max_active_tasks = max_active_tasks;
                Ok(())
            }
        }
    }

    async fn enqueue_ids(&mut self, ids: Vec<String>, retranslate: bool) -> DispatchResult<()> {
        let mut unique_ids = HashSet::new();
        let ids = ids
            .into_iter()
            .filter(|id| unique_ids.insert(id.clone()))
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Err(DispatchError::Rejected("No tasks were provided".into()));
        }
        let mut scheduled = Vec::new();
        let mut candidates = Vec::new();
        for id in ids {
            let current =
                translation_tasks::get_translation_task_summary(&self.context.config_pool, &id)
                    .await
                    .map_err(classify_task_lookup_error)?;
            if self.queued_task_ids.contains(&id) || self.active_tasks.contains_key(&id) {
                emit_task_status(&self.context.app, &current);
                continue;
            }
            let mode = if retranslate {
                if !matches!(
                    current.status,
                    TranslationTaskStatus::Success | TranslationTaskStatus::Failed
                ) {
                    return Err(DispatchError::Rejected(format!(
                        "Task {} cannot be retranslated from {:?} status",
                        id, current.status
                    )));
                }
                RunMode::Retranslate
            } else {
                match current.status {
                    TranslationTaskStatus::Pending => RunMode::Start,
                    TranslationTaskStatus::Interrupted => RunMode::Resume,
                    _ => {
                        return Err(DispatchError::Rejected(format!(
                            "Task {} cannot be enqueued from {:?} status",
                            id, current.status
                        )));
                    }
                }
            };
            scheduled.push(ScheduledTask {
                id: id.clone(),
                mode,
            });
            candidates.push((id, current.status));
        }
        if scheduled.is_empty() {
            return Ok(());
        }
        let mut queue_updates = Vec::with_capacity(candidates.len());
        if retranslate {
            for (id, _) in candidates {
                translation_tasks::reset_task_for_retranslation(
                    &self.context.config_pool,
                    &self.context.workspace_root,
                    &id,
                )
                .await
                .map_err(DispatchError::Infrastructure)?;
                queue_updates.push((id, TranslationTaskStatus::Pending));
            }
        } else {
            queue_updates = candidates;
        }
        let queued = translation_tasks::mark_tasks_queued_atomically(
            &self.context.config_pool,
            &self.context.workspace_root,
            &queue_updates,
        )
        .await
        .map_err(DispatchError::Infrastructure)?;
        insert_scheduled_tasks(
            &mut self.waiting_tasks,
            &mut self.queued_task_ids,
            scheduled,
        );
        for task in queued {
            emit_task_status(&self.context.app, &task);
        }
        Ok(())
    }

    async fn pause_task(&mut self, task_id: &str) -> DispatchResult<()> {
        if self.queued_task_ids.contains(task_id) {
            let restored = translation_tasks::restore_queued_tasks(
                &self.context.app,
                &self.context.config_pool,
                &[task_id.to_string()],
            )
            .await
            .map_err(DispatchError::Infrastructure)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                DispatchError::Infrastructure(
                    "Translation task was not restored from the queue".to_string(),
                )
            })?;
            self.queued_task_ids.remove(task_id);
            self.waiting_tasks.retain(|task| task.id != task_id);
            emit_task_status(&self.context.app, &restored);
            return Ok(());
        }
        if let Some(active) = self.active_tasks.get(task_id) {
            if active.cancellation_token.is_cancelled() {
                translation_tasks::get_translation_task_summary(&self.context.config_pool, task_id)
                    .await
                    .map_err(classify_task_lookup_error)?;
                return Ok(());
            }
            let interrupt = active.interrupt.clone();
            let task = translation_tasks::mark_task_interrupted_pending(
                &self.context.app,
                &self.context.config_pool,
                &self.context.workspace_root,
                task_id,
            )
            .await
            .map_err(DispatchError::Infrastructure)?;
            emit_task_status(&self.context.app, &task);
            interrupt.interrupt("Task paused");
            return Ok(());
        }
        translation_tasks::get_translation_task_summary(&self.context.config_pool, task_id)
            .await
            .map_err(classify_task_lookup_error)?;
        Ok(())
    }

    async fn pause_all(&mut self) -> DispatchResult<()> {
        let queued_ids = self
            .waiting_tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<Vec<_>>();
        let restored = translation_tasks::restore_queued_tasks(
            &self.context.app,
            &self.context.config_pool,
            &queued_ids,
        )
        .await
        .map_err(DispatchError::Infrastructure)?;
        self.waiting_tasks.clear();
        self.queued_task_ids.clear();
        for task in restored {
            emit_task_status(&self.context.app, &task);
        }
        let active = self
            .active_tasks
            .iter()
            .filter(|(_, task)| !task.cancellation_token.is_cancelled())
            .map(|(id, task)| (id.clone(), task.interrupt.clone()))
            .collect::<Vec<_>>();
        for (id, interrupt) in active {
            let task = translation_tasks::mark_task_interrupted_pending(
                &self.context.app,
                &self.context.config_pool,
                &self.context.workspace_root,
                &id,
            )
            .await
            .map_err(DispatchError::Infrastructure)?;
            emit_task_status(&self.context.app, &task);
            interrupt.interrupt("Task paused");
        }
        Ok(())
    }

    fn start_ready_tasks(&mut self) {
        while let Some(task) = pop_next_ready_task(
            &mut self.waiting_tasks,
            &mut self.queued_task_ids,
            self.active_tasks.len(),
            self.max_active_tasks,
        ) {
            let cancellation_token = CancellationToken::new();
            let interrupt = TranslationInterrupt::from_token(cancellation_token.clone());
            self.active_tasks.insert(
                task.id.clone(),
                ActiveTask {
                    cancellation_token,
                    interrupt: interrupt.clone(),
                },
            );
            let context = self.context.clone();
            let sender = self.sender.clone();
            let task_id = task.id.clone();
            tauri::async_runtime::spawn(async move {
                let execution = AssertUnwindSafe(execute_task(context, task, interrupt))
                    .catch_unwind()
                    .await;
                let panic_message = execution.err().map(panic_payload_message);
                let _ = sender
                    .send(SchedulerInstruction::TaskFinished(TaskCompletion {
                        task_id,
                        panic_message,
                    }))
                    .await;
            });
        }
    }

    async fn handle_task_finished(&mut self, completion: TaskCompletion) {
        if remove_active_task(&mut self.active_tasks, &completion.task_id).is_none() {
            if let Some(log) = &self.backend_log {
                log.write(
                    "WARN",
                    "task-scheduler",
                    format!(
                        "Ignoring duplicate or stale TaskFinished for id={}",
                        completion.task_id
                    ),
                );
            }
            return;
        }
        if let Some(message) = completion.panic_message {
            let error = format!("Task worker panicked: {message}");
            if let Ok(task) = translation_tasks::get_translation_task_summary(
                &self.context.config_pool,
                &completion.task_id,
            )
            .await
            {
                if let Ok(failed) = translation_tasks::mark_task_failed_after_runtime_error(
                    &self.context.config_pool,
                    PathBuf::from(task.inp_path).as_path(),
                    error.clone(),
                )
                .await
                {
                    emit_task_status(&self.context.app, &failed);
                }
            }
            if let Some(log) = &self.backend_log {
                log.write("ERROR", "task-scheduler", error);
            }
            return;
        }
        if let Ok(task) = translation_tasks::get_translation_task_summary(
            &self.context.config_pool,
            &completion.task_id,
        )
        .await
        {
            emit_task_status(&self.context.app, &task);
        }
    }
}

async fn execute_task(
    context: TaskSchedulerContext,
    task: ScheduledTask,
    interrupt: TranslationInterrupt,
) {
    if interrupt.is_interrupted() {
        finalize_cancelled_before_run(&context, &task.id, &interrupt).await;
        return;
    }
    let indexed =
        match translation_tasks::get_translation_task_summary(&context.config_pool, &task.id).await
        {
            Ok(task) => task,
            Err(error) => {
                log_execution_error(&context.app, &task.id, &error);
                return;
            }
        };
    let inp_path = PathBuf::from(&indexed.inp_path);
    let prepared = match translation_tasks::prepare_translation_run(
        &context.app,
        &context.provider_pool,
        &context.client,
        &context.config_pool,
        &context.workspace_root,
        &task.id,
        task.mode,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            mark_execution_failed(&context, &task.id, &inp_path, error).await;
            return;
        }
    };
    if interrupt.is_interrupted() {
        finalize_cancelled_before_run(&context, &task.id, &interrupt).await;
        return;
    }
    emit_task_status(&context.app, &prepared.task);
    if let Err(error) = translation_tasks::run_translation_task(
        context.app.clone(),
        context.provider_pool.clone(),
        context.config_pool.clone(),
        context.glossary_config_pool.clone(),
        context.glossary_workspace_root.clone(),
        context.client.clone(),
        prepared,
        interrupt,
    )
    .await
    {
        mark_execution_failed(&context, &task.id, &inp_path, error).await;
    }
}

async fn finalize_cancelled_before_run(
    context: &TaskSchedulerContext,
    task_id: &str,
    interrupt: &TranslationInterrupt,
) {
    let reason = interrupt
        .reason()
        .unwrap_or_else(|| "Task interrupted".to_string());
    if let Ok(task) = translation_tasks::mark_task_interrupted(
        &context.app,
        &context.config_pool,
        &context.workspace_root,
        task_id,
        reason,
    )
    .await
    {
        emit_task_status(&context.app, &task);
    }
}

async fn mark_execution_failed(
    context: &TaskSchedulerContext,
    task_id: &str,
    inp_path: &std::path::Path,
    error: String,
) {
    log_execution_error(&context.app, task_id, &error);
    match translation_tasks::mark_task_failed_after_runtime_error(
        &context.config_pool,
        inp_path,
        error.clone(),
    )
    .await
    {
        Ok(task) => emit_task_status(&context.app, &task),
        Err(sync_error) => {
            log_execution_error(&context.app, task_id, &sync_error);
            if let Ok(task) =
                translation_tasks::mark_task_index_failed(&context.config_pool, task_id, error)
                    .await
            {
                emit_task_status(&context.app, &task);
            }
        }
    }
}

fn emit_task_status(app: &AppHandle, task: &TranslationTaskView) {
    let _ = app.emit(
        TASK_STATUS_CHANGED_EVENT,
        TranslationProgressPayload { task: task.clone() },
    );
}

fn log_execution_error(app: &AppHandle, task_id: &str, error: &str) {
    if let Ok(log) = BackendLog::from_app(app) {
        log.write(
            "ERROR",
            "task-scheduler",
            format!("Task id={task_id} failed: {error}"),
        );
    }
}

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn classify_task_lookup_error(error: String) -> DispatchError {
    if error == "Translation task not found" {
        DispatchError::Rejected(error)
    } else {
        DispatchError::Infrastructure(error)
    }
}

fn remove_active_task(
    active_tasks: &mut HashMap<String, ActiveTask>,
    task_id: &str,
) -> Option<ActiveTask> {
    active_tasks.remove(task_id)
}

fn pop_next_ready_task(
    waiting_tasks: &mut VecDeque<ScheduledTask>,
    queued_task_ids: &mut HashSet<String>,
    active_count: usize,
    max_active_tasks: usize,
) -> Option<ScheduledTask> {
    if active_count >= max_active_tasks {
        return None;
    }
    let task = waiting_tasks.pop_front()?;
    queued_task_ids.remove(&task.id);
    Some(task)
}

fn insert_scheduled_tasks(
    waiting_tasks: &mut VecDeque<ScheduledTask>,
    queued_task_ids: &mut HashSet<String>,
    scheduled: Vec<ScheduledTask>,
) {
    let mut resumes = Vec::new();
    let mut regular = Vec::new();
    for task in scheduled {
        queued_task_ids.insert(task.id.clone());
        if matches!(task.mode, RunMode::Resume) {
            resumes.push(task);
        } else {
            regular.push(task);
        }
    }
    for task in resumes.into_iter().rev() {
        waiting_tasks.push_front(task);
    }
    waiting_tasks.extend(regular);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_task() -> ActiveTask {
        let cancellation_token = CancellationToken::new();
        ActiveTask {
            interrupt: TranslationInterrupt::from_token(cancellation_token.clone()),
            cancellation_token,
        }
    }

    #[test]
    fn task_finished_removes_active_task_and_drops_scheduler_token() {
        let mut active_tasks = HashMap::new();
        let task = active_task();
        let token = task.cancellation_token.clone();
        active_tasks.insert("task-1".to_string(), task);

        let removed = remove_active_task(&mut active_tasks, "task-1");

        assert!(removed.is_some());
        assert!(active_tasks.is_empty());
        assert!(!token.is_cancelled());
    }

    #[test]
    fn duplicate_task_finished_is_idempotent() {
        let mut active_tasks = HashMap::new();
        active_tasks.insert("task-1".to_string(), active_task());

        assert!(remove_active_task(&mut active_tasks, "task-1").is_some());
        assert!(remove_active_task(&mut active_tasks, "task-1").is_none());
        assert!(active_tasks.is_empty());
    }

    #[test]
    fn removing_one_finished_task_does_not_affect_other_active_tasks() {
        let mut active_tasks = HashMap::new();
        active_tasks.insert("task-1".to_string(), active_task());
        active_tasks.insert("task-2".to_string(), active_task());

        assert!(remove_active_task(&mut active_tasks, "task-1").is_some());
        assert!(!active_tasks.contains_key("task-1"));
        assert!(active_tasks.contains_key("task-2"));
    }

    #[test]
    fn ready_tasks_are_taken_in_fifo_order() {
        let mut waiting_tasks = VecDeque::from([
            ScheduledTask {
                id: "task-1".into(),
                mode: RunMode::Start,
            },
            ScheduledTask {
                id: "task-2".into(),
                mode: RunMode::Resume,
            },
        ]);
        let mut queued_task_ids = HashSet::from(["task-1".into(), "task-2".into()]);

        let first = pop_next_ready_task(&mut waiting_tasks, &mut queued_task_ids, 0, 1)
            .expect("first ready task");

        assert_eq!(first.id, "task-1");
        assert!(!queued_task_ids.contains("task-1"));
        assert_eq!(
            waiting_tasks.front().map(|task| task.id.as_str()),
            Some("task-2")
        );
    }

    #[test]
    fn concurrency_limit_prevents_new_task_from_leaving_queue() {
        let mut waiting_tasks = VecDeque::from([ScheduledTask {
            id: "task-1".into(),
            mode: RunMode::Start,
        }]);
        let mut queued_task_ids = HashSet::from(["task-1".into()]);

        let next = pop_next_ready_task(&mut waiting_tasks, &mut queued_task_ids, 2, 1);

        assert!(next.is_none());
        assert_eq!(waiting_tasks.len(), 1);
        assert!(queued_task_ids.contains("task-1"));
    }

    #[test]
    fn resumed_task_is_inserted_before_waiting_new_tasks() {
        let mut waiting_tasks = VecDeque::from([ScheduledTask {
            id: "pending-existing".into(),
            mode: RunMode::Start,
        }]);
        let mut queued_task_ids = HashSet::from(["pending-existing".into()]);

        insert_scheduled_tasks(
            &mut waiting_tasks,
            &mut queued_task_ids,
            vec![ScheduledTask {
                id: "resume-task".into(),
                mode: RunMode::Resume,
            }],
        );

        assert_eq!(
            waiting_tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec!["resume-task", "pending-existing"]
        );
    }

    #[test]
    fn batch_prioritizes_resumes_and_preserves_their_relative_order() {
        let mut waiting_tasks = VecDeque::new();
        let mut queued_task_ids = HashSet::new();

        insert_scheduled_tasks(
            &mut waiting_tasks,
            &mut queued_task_ids,
            vec![
                ScheduledTask {
                    id: "pending-1".into(),
                    mode: RunMode::Start,
                },
                ScheduledTask {
                    id: "resume-1".into(),
                    mode: RunMode::Resume,
                },
                ScheduledTask {
                    id: "pending-2".into(),
                    mode: RunMode::Start,
                },
                ScheduledTask {
                    id: "resume-2".into(),
                    mode: RunMode::Resume,
                },
            ],
        );

        assert_eq!(
            waiting_tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec!["resume-1", "resume-2", "pending-1", "pending-2"]
        );
    }

    #[test]
    fn scheduler_action_serializes_variant_fields_as_camel_case() {
        let action = SchedulerAction::EnqueueBatch {
            task_ids: vec!["task-1".into(), "task-2".into()],
        };
        let value = serde_json::to_value(&action).expect("serialize scheduler action");

        assert_eq!(
            value,
            serde_json::json!({
                "type": "enqueueBatch",
                "taskIds": ["task-1", "task-2"]
            })
        );
        let roundtrip: SchedulerAction =
            serde_json::from_value(value).expect("deserialize scheduler action");
        assert_eq!(roundtrip, action);
    }

    #[test]
    fn scheduler_action_uses_camel_case_for_concurrency_field() {
        let action = SchedulerAction::SetConcurrency {
            max_active_tasks: 3,
        };
        let value = serde_json::to_value(&action).expect("serialize concurrency action");

        assert_eq!(
            value,
            serde_json::json!({
                "type": "setConcurrency",
                "maxActiveTasks": 3
            })
        );
        assert!(
            serde_json::from_value::<SchedulerAction>(serde_json::json!({
                "type": "setConcurrency",
                "max_active_tasks": 3
            }))
            .is_err()
        );
    }
}
