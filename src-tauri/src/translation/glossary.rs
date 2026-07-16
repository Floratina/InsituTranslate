use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use futures_util::{stream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::adapters::RuntimeAdapter;
use crate::db as app_db;
use crate::diagnostics::BackendLog;
use crate::domain::{ProviderPurpose, ThinkingConfig, UnifiedChatRequest};
use crate::glossaries::{self, CreateAutoGlossaryInput, GlossaryView, PrepareAutoGlossaryInput};
use crate::glossary_prompt::{
    build_glossary_prompt, sanitize_and_flatten_glossary, GlossaryEntry, GlossaryPromptBuildResult,
    GlossaryPromptInput,
};
use crate::task_prompt::{ContentFormat, DocumentFormat, TaskChunkInput};

use super::context::estimate_tokens;
use super::db::{
    apply_glossary_report_and_emit, connect_inp, content_format_from_source_path,
    document_format_from_source_path, ensure_task_glossary_generation_snapshot, finalize_task,
    get_task_from_index, glossary_source_chunks, metadata_task, refresh_task_stats,
    set_task_glossary_id, task_execution_config, task_failure_thresholds, task_glossary_config,
    GlossaryProgressSnapshot, GlossaryRetrySnapshot,
};
use super::limiter::{AdaptiveLimiter, HeaderQuotaPolicy, ManualRateLimiter};
use super::scheduler::{
    retry_base_delay_ms, retry_delay_with_jitter_ms, transient_retry_base_delay_ms,
};
use super::types::ChunkRecord;
use super::{
    failure_threshold_exceeded, GlossaryMode, RateLimitStrategy, TranslationConfigView,
    TranslationInterrupt, TranslationProgressPayload, TranslationTaskStatus, TranslationTaskView,
    TRANSLATION_PROGRESS_EVENT,
};

#[derive(Clone)]
struct GlossaryRuntime {
    adapter: Arc<RuntimeAdapter>,
    model_request_name: String,
    assistant_prompt: Option<String>,
    assistant_custom_parameters: Value,
    temperature: Option<f64>,
    top_p: Option<f64>,
    web_search: bool,
    thinking: Option<ThinkingConfig>,
}

#[derive(Clone)]
struct GlossaryRequestReporter {
    sender: mpsc::Sender<GlossaryReport>,
    total_chunks: u64,
    completed_chunks: Arc<AtomicU64>,
    backend_log: Option<BackendLog>,
}

enum GlossaryReport {
    Status {
        current: u64,
        total: u64,
        state: String,
        label: String,
    },
    Retry {
        chunk_id: String,
        current: u32,
        max: u32,
        message: Option<String>,
    },
}

#[derive(Default)]
struct GlossaryReportState {
    progress: Option<GlossaryProgressSnapshot>,
    retries: HashMap<String, (u64, GlossaryRetrySnapshot)>,
    retry_sequence: u64,
    dirty: bool,
}

impl GlossaryReportState {
    fn apply(&mut self, report: GlossaryReport) {
        match report {
            GlossaryReport::Status {
                current,
                total,
                state,
                label,
            } => {
                if self
                    .progress
                    .as_ref()
                    .is_none_or(|existing| current >= existing.current)
                {
                    self.progress = Some(GlossaryProgressSnapshot {
                        current,
                        total,
                        state,
                        label,
                    });
                    self.dirty = true;
                }
            }
            GlossaryReport::Retry {
                chunk_id,
                current,
                max,
                message,
            } => {
                self.retry_sequence = self.retry_sequence.saturating_add(1);
                match message {
                    Some(message) => {
                        self.retries.insert(
                            chunk_id.clone(),
                            (
                                self.retry_sequence,
                                GlossaryRetrySnapshot {
                                    chunk_id,
                                    current,
                                    max,
                                    message,
                                },
                            ),
                        );
                    }
                    None => {
                        self.retries.remove(&chunk_id);
                    }
                }
                self.dirty = true;
            }
        }
    }

    fn latest_retry(&self) -> Option<&GlossaryRetrySnapshot> {
        self.retries
            .values()
            .max_by_key(|(sequence, _)| *sequence)
            .map(|(_, retry)| retry)
    }
}

impl GlossaryRequestReporter {
    async fn status(&self, label: impl Into<String>) {
        let _ = self.sender.try_send(GlossaryReport::Status {
            current: self.completed_chunks.load(Ordering::SeqCst),
            total: self.total_chunks,
            state: "running".into(),
            label: label.into(),
        });
    }

    async fn retry(&self, chunk_id: &str, current: u32, max: u32, message: Option<String>) {
        let _ = self.sender.try_send(GlossaryReport::Retry {
            chunk_id: chunk_id.to_string(),
            current,
            max,
            message,
        });
    }

    fn log(&self, level: &str, message: impl AsRef<str>) {
        if let Some(log) = &self.backend_log {
            log.write(level, "glossary", message);
        }
    }
}

#[derive(Debug, Clone)]
enum AutoGlossaryChunkOutcome {
    Success {
        sequence: i64,
        entries: Vec<GlossaryEntry>,
    },
    Failed {
        sequence: i64,
        error: String,
    },
    Interrupted {
        error: String,
    },
}

pub(super) enum TaskGlossaryPreparation {
    Ready(Vec<GlossaryEntry>),
    Failed,
    Interrupted,
}

#[derive(Debug, Clone)]
pub(super) struct TaskGlossaryMatcher {
    entries: Vec<GlossaryEntry>,
    automaton: Option<AhoCorasick>,
}

#[derive(Debug, Clone, Copy)]
struct GlossaryMatchCandidate {
    entry_index: usize,
    start: usize,
    end: usize,
    term_len: usize,
}

impl TaskGlossaryMatcher {
    pub(super) fn new(entries: Vec<GlossaryEntry>) -> Result<Self, String> {
        if entries.is_empty() {
            return Ok(Self {
                entries,
                automaton: None,
            });
        }

        let patterns = entries
            .iter()
            .map(|entry| entry.src.as_str())
            .collect::<Vec<_>>();
        let automaton = AhoCorasickBuilder::new()
            .ascii_case_insensitive(true)
            // find_overlapping_iter only supports MatchKind::Standard. Do not switch this
            // to LeftmostFirst or LeftmostLongest, because that would panic at runtime.
            .match_kind(MatchKind::Standard)
            .build(patterns)
            .map_err(|error| format!("Unable to build glossary matcher: {error}"))?;

        Ok(Self {
            entries,
            automaton: Some(automaton),
        })
    }

    pub(super) fn match_entries(&self, chunk_text: &str) -> Vec<GlossaryEntry> {
        let Some(automaton) = self.automaton.as_ref() else {
            return Vec::new();
        };

        let mut candidates = automaton
            .find_overlapping_iter(chunk_text)
            .filter_map(|matched| {
                let entry_index = matched.pattern().as_usize();
                let entry = self.entries.get(entry_index)?;
                let start = matched.start();
                let end = matched.end();
                if !valid_glossary_match_boundary(chunk_text, start, end, entry) {
                    return None;
                }
                Some(GlossaryMatchCandidate {
                    entry_index,
                    start,
                    end,
                    term_len: entry.src.chars().count(),
                })
            })
            .collect::<Vec<_>>();

        candidates.sort_by(|left, right| {
            right
                .term_len
                .cmp(&left.term_len)
                .then_with(|| left.start.cmp(&right.start))
                .then_with(|| left.entry_index.cmp(&right.entry_index))
        });

        let mut matched_indexes = BTreeSet::new();
        let mut accepted_spans = Vec::new();
        for candidate in candidates {
            if accepted_spans
                .iter()
                .any(|(start, end)| spans_overlap(candidate.start, candidate.end, *start, *end))
            {
                continue;
            }
            accepted_spans.push((candidate.start, candidate.end));
            matched_indexes.insert(candidate.entry_index);
        }

        matched_indexes
            .into_iter()
            .filter_map(|index| self.entries.get(index).cloned())
            .collect()
    }
}

enum AutoGlossaryGeneration {
    Created(GlossaryView),
    Failed(String),
    Interrupted(String),
}

pub(super) fn glossary_threshold_failure_reason(
    failed_chunks: i64,
    total_chunks: i64,
    max_failure_percentage: i64,
    last_error: &str,
) -> Result<Option<String>, String> {
    if !failure_threshold_exceeded(failed_chunks, total_chunks, max_failure_percentage)? {
        return Ok(None);
    }
    Ok(Some(format!(
        "Glossary failure threshold exceeded: {failed_chunks}/{total_chunks} chunks failed (maximum {max_failure_percentage}%): {last_error}",
    )))
}

fn start_glossary_reporter(
    app: AppHandle,
    inp_pool: SqlitePool,
    config_pool: SqlitePool,
    inp_path: PathBuf,
    total_chunks: u64,
    backend_log: Option<BackendLog>,
) -> (GlossaryRequestReporter, JoinHandle<Result<(), String>>) {
    let (sender, receiver) = mpsc::channel(64);
    let reporter = GlossaryRequestReporter {
        sender,
        total_chunks,
        completed_chunks: Arc::new(AtomicU64::new(0)),
        backend_log,
    };
    let handle = tokio::spawn(run_glossary_reporter(
        app,
        inp_pool,
        config_pool,
        inp_path,
        receiver,
    ));
    (reporter, handle)
}

async fn run_glossary_reporter(
    app: AppHandle,
    inp_pool: SqlitePool,
    config_pool: SqlitePool,
    inp_path: PathBuf,
    mut receiver: mpsc::Receiver<GlossaryReport>,
) -> Result<(), String> {
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval.tick().await;
    let mut state = GlossaryReportState::default();

    loop {
        tokio::select! {
            report = receiver.recv() => {
                let Some(report) = report else {
                    if !state.retries.is_empty() {
                        state.retries.clear();
                        state.dirty = true;
                    }
                    if state.dirty {
                        flush_glossary_report(
                            &app,
                            &inp_pool,
                            &config_pool,
                            &inp_path,
                            &state,
                        ).await?;
                    }
                    return Ok(());
                };
                state.apply(report);
            }
            _ = interval.tick(), if state.dirty => {
                flush_glossary_report(
                    &app,
                    &inp_pool,
                    &config_pool,
                    &inp_path,
                    &state,
                ).await?;
                state.dirty = false;
            }
        }
    }
}

async fn flush_glossary_report(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    state: &GlossaryReportState,
) -> Result<(), String> {
    let Some(progress) = state.progress.as_ref() else {
        return Ok(());
    };
    let retry = state.latest_retry();
    apply_glossary_report_and_emit(app, inp_pool, config_pool, inp_path, progress, retry).await?;
    Ok(())
}

async fn finish_glossary_reporter(
    reporter: GlossaryRequestReporter,
    handle: JoinHandle<Result<(), String>>,
    final_report: GlossaryReport,
) -> Result<(), String> {
    let send_result = reporter.sender.send(final_report).await;
    drop(reporter);
    let reporter_result = handle
        .await
        .map_err(|error| format!("Automatic glossary progress reporter failed: {error}"))?;
    reporter_result?;
    send_result
        .map_err(|_| "Automatic glossary progress reporter stopped unexpectedly".to_string())?;
    Ok(())
}

pub async fn prepare_auto_glossary_for_task(
    app: &AppHandle,
    provider_pool: &SqlitePool,
    glossary_config_pool: &SqlitePool,
    glossary_workspace_root: &Path,
    client: &Client,
    config_pool: &SqlitePool,
    workspace_root: &Path,
    input: PrepareAutoGlossaryInput,
) -> Result<Option<GlossaryView>, String> {
    let task_id = input.task_id.trim();
    if task_id.is_empty() {
        return Err("Task id is required".into());
    }
    let indexed = get_task_from_index(config_pool, task_id).await?;
    let inp_path = PathBuf::from(&indexed.inp_path);
    if !inp_path.starts_with(workspace_root) {
        return Err("Task file is outside the configured workspace".into());
    }
    let inp_pool = connect_inp(&inp_path).await?;
    let task = metadata_task(&inp_pool, &inp_path).await?;
    let glossary_config = task_glossary_config(&inp_pool).await?;
    if !glossary_config.use_glossary
        || glossary_config.glossary_mode != GlossaryMode::Auto
        || glossary_config.glossary_id.is_some()
    {
        inp_pool.close().await;
        return Ok(None);
    }
    let chunks = glossary_source_chunks(&inp_pool).await?;
    let config = task_execution_config(&inp_pool, config_pool).await?;
    let interrupt = TranslationInterrupt::new();
    let result = generate_auto_glossary(
        app,
        provider_pool,
        glossary_config_pool,
        glossary_workspace_root,
        client,
        &inp_pool,
        config_pool,
        &inp_path,
        &task,
        &chunks,
        &config,
        &interrupt,
    )
    .await?;
    match result {
        AutoGlossaryGeneration::Created(view) => {
            let refreshed = refresh_task_stats(&inp_pool, config_pool, &inp_path, None).await?;
            let _ = app.emit(
                TRANSLATION_PROGRESS_EVENT,
                TranslationProgressPayload { task: refreshed },
            );
            inp_pool.close().await;
            Ok(Some(view))
        }
        AutoGlossaryGeneration::Failed(reason) => {
            finalize_task(
                app,
                &inp_pool,
                config_pool,
                &inp_path,
                TranslationTaskStatus::Failed,
                Some(reason),
                None,
            )
            .await?;
            inp_pool.close().await;
            Ok(None)
        }
        AutoGlossaryGeneration::Interrupted(reason) => {
            finalize_task(
                app,
                &inp_pool,
                config_pool,
                &inp_path,
                TranslationTaskStatus::Interrupted,
                Some(reason),
                None,
            )
            .await?;
            inp_pool.close().await;
            Ok(None)
        }
    }
}

pub(super) async fn prepare_task_glossary(
    app: &AppHandle,
    provider_pool: &SqlitePool,
    glossary_config_pool: &SqlitePool,
    glossary_workspace_root: &Path,
    client: &Client,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    task: &TranslationTaskView,
    pending_chunks: &[ChunkRecord],
    config: &TranslationConfigView,
    interrupt: &TranslationInterrupt,
) -> Result<TaskGlossaryPreparation, String> {
    let glossary_config = task_glossary_config(inp_pool).await?;
    if !glossary_config.use_glossary {
        return Ok(TaskGlossaryPreparation::Ready(Vec::new()));
    }

    let glossary_id = match glossary_config.glossary_mode {
        GlossaryMode::Existing => glossary_config
            .glossary_id
            .ok_or_else(|| "Glossary selection is required for this task".to_string())?,
        GlossaryMode::Auto => match glossary_config.glossary_id {
            Some(id) => id,
            None => {
                match generate_auto_glossary(
                    app,
                    provider_pool,
                    glossary_config_pool,
                    glossary_workspace_root,
                    client,
                    inp_pool,
                    config_pool,
                    inp_path,
                    task,
                    pending_chunks,
                    config,
                    interrupt,
                )
                .await?
                {
                    AutoGlossaryGeneration::Created(view) => {
                        let refreshed =
                            refresh_task_stats(inp_pool, config_pool, inp_path, None).await?;
                        let _ = app.emit(
                            TRANSLATION_PROGRESS_EVENT,
                            TranslationProgressPayload { task: refreshed },
                        );
                        view.id
                    }
                    AutoGlossaryGeneration::Failed(reason) => {
                        finalize_task(
                            app,
                            inp_pool,
                            config_pool,
                            inp_path,
                            TranslationTaskStatus::Failed,
                            Some(reason),
                            None,
                        )
                        .await?;
                        return Ok(TaskGlossaryPreparation::Failed);
                    }
                    AutoGlossaryGeneration::Interrupted(reason) => {
                        finalize_task(
                            app,
                            inp_pool,
                            config_pool,
                            inp_path,
                            TranslationTaskStatus::Interrupted,
                            Some(reason),
                            None,
                        )
                        .await?;
                        return Ok(TaskGlossaryPreparation::Interrupted);
                    }
                }
            }
        },
    };

    let entries = glossaries::load_glossary_entries(glossary_config_pool, &glossary_id).await?;
    Ok(TaskGlossaryPreparation::Ready(entries))
}

async fn generate_auto_glossary(
    app: &AppHandle,
    provider_pool: &SqlitePool,
    glossary_config_pool: &SqlitePool,
    glossary_workspace_root: &Path,
    client: &Client,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    task: &TranslationTaskView,
    pending_chunks: &[ChunkRecord],
    config: &TranslationConfigView,
    interrupt: &TranslationInterrupt,
) -> Result<AutoGlossaryGeneration, String> {
    let snapshot = ensure_task_glossary_generation_snapshot(inp_pool, provider_pool, config)
        .await?
        .ok_or_else(|| "Automatic glossary generation snapshot is missing".to_string())?;
    let live_provider = app_db::list_providers(provider_pool, Some(ProviderPurpose::Glossary))
        .await?
        .into_iter()
        .find(|provider| provider.id == snapshot.provider_id)
        .ok_or_else(|| {
            "Selected glossary provider no longer exists or is not assigned to glossary use"
                .to_string()
        })?;
    if !live_provider.enabled {
        return Err("Selected glossary provider is disabled".into());
    }
    let runtime_config = app_db::runtime_config(provider_pool, &snapshot.provider_id).await?;
    let glossary_runtime = GlossaryRuntime {
        adapter: Arc::new(RuntimeAdapter::new(client.clone(), runtime_config)),
        model_request_name: snapshot.model_request_name,
        assistant_prompt: snapshot.assistant_system_prompt,
        assistant_custom_parameters: snapshot.assistant_custom_parameters,
        temperature: snapshot.temperature,
        top_p: snapshot.top_p,
        web_search: snapshot.web_search,
        thinking: snapshot.thinking,
    };
    let chunks = pending_chunks
        .iter()
        .filter(|chunk| !chunk.source_text.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if chunks.is_empty() {
        return Err(
            "Task contains no translatable chunks for automatic glossary generation".into(),
        );
    }
    let total_chunks = chunks.len() as u64;
    let failure_thresholds = task_failure_thresholds(inp_pool).await?;
    let document_format = document_format_from_source_path(&task.source_path)?;
    let content_format = content_format_from_source_path(&task.source_path)?;
    let (request_reporter, reporter_handle) = start_glossary_reporter(
        app.clone(),
        inp_pool.clone(),
        config_pool.clone(),
        inp_path.to_path_buf(),
        total_chunks,
        BackendLog::from_app(app).ok(),
    );
    request_reporter
        .status(format!("术语表建立 (0/{total_chunks})"))
        .await;
    let dynamic_rate_limit = config.rate_limit_strategy == RateLimitStrategy::Dynamic;
    let limiter = Arc::new(AdaptiveLimiter::new(
        config.max_concurrency.max(1) as usize,
        dynamic_rate_limit,
    ));
    let quota = Arc::new(HeaderQuotaPolicy::new(dynamic_rate_limit));
    let manual_limiter = if config.rate_limit_strategy == RateLimitStrategy::Manual {
        Some(Arc::new(ManualRateLimiter::new(
            config.max_requests_per_minute as u64,
            config.max_tokens_per_minute as u64,
        )))
    } else {
        None
    };
    let max_concurrency = config.max_concurrency.max(1) as usize;
    let max_retries = config.max_retries.max(0) as u32;
    let target_language = task.target_language.clone();
    let runtime = Arc::new(glossary_runtime);
    let mut outcome_stream = stream::iter(chunks.clone())
        .map(|chunk| {
            let runtime = runtime.clone();
            let limiter = limiter.clone();
            let quota = quota.clone();
            let manual_limiter = manual_limiter.clone();
            let target_language = target_language.clone();
            let interrupted = interrupt.clone();
            let reporter = request_reporter.clone();
            async move {
                generate_glossary_for_chunk(
                    runtime,
                    target_language,
                    document_format,
                    content_format,
                    chunk,
                    max_retries,
                    quota,
                    limiter,
                    manual_limiter,
                    interrupted,
                    reporter,
                )
                .await
            }
        })
        .buffer_unordered(max_concurrency);
    let mut outcomes = Vec::new();
    let mut completed_chunks = 0_u64;
    while let Some(outcome) = outcome_stream.next().await {
        completed_chunks += 1;
        request_reporter
            .completed_chunks
            .store(completed_chunks, Ordering::SeqCst);
        let state = if matches!(outcome, AutoGlossaryChunkOutcome::Interrupted { .. }) {
            "failed"
        } else {
            "running"
        };
        let _ = request_reporter
            .sender
            .send(GlossaryReport::Status {
                current: completed_chunks,
                total: total_chunks,
                state: state.into(),
                label: format!("术语表建立 ({completed_chunks}/{total_chunks})"),
            })
            .await;
        outcomes.push(outcome);
    }
    drop(outcome_stream);
    outcomes.sort_by_key(|outcome| match outcome {
        AutoGlossaryChunkOutcome::Success { sequence, .. }
        | AutoGlossaryChunkOutcome::Failed { sequence, .. } => *sequence,
        AutoGlossaryChunkOutcome::Interrupted { .. } => i64::MAX,
    });

    let mut entries = Vec::new();
    let mut failed_chunks = 0_usize;
    for outcome in outcomes {
        match outcome {
            AutoGlossaryChunkOutcome::Success {
                entries: chunk_entries,
                ..
            } => {
                entries.extend(chunk_entries);
            }
            AutoGlossaryChunkOutcome::Failed { error, .. } => {
                failed_chunks += 1;
                if let Some(reason) = glossary_threshold_failure_reason(
                    failed_chunks as i64,
                    chunks.len() as i64,
                    failure_thresholds.glossary_max_failure_percentage,
                    &error,
                )? {
                    finish_glossary_reporter(
                        request_reporter,
                        reporter_handle,
                        GlossaryReport::Status {
                            current: completed_chunks,
                            total: total_chunks,
                            state: "failed".into(),
                            label: "自动术语表生成失败".into(),
                        },
                    )
                    .await?;
                    return Ok(AutoGlossaryGeneration::Failed(reason));
                }
            }
            AutoGlossaryChunkOutcome::Interrupted { error } => {
                finish_glossary_reporter(
                    request_reporter,
                    reporter_handle,
                    GlossaryReport::Status {
                        current: completed_chunks,
                        total: total_chunks,
                        state: "failed".into(),
                        label: "自动术语表生成已中断".into(),
                    },
                )
                .await?;
                return Ok(AutoGlossaryGeneration::Interrupted(error));
            }
        }
    }

    request_reporter
        .status(format!(
            "正在保存自动术语表... ({total_chunks}/{total_chunks})"
        ))
        .await;
    request_reporter.log(
        "INFO",
        format!(
            "task={} saving auto glossary from {} chunks with {} entries",
            task.id,
            total_chunks,
            entries.len(),
        ),
    );
    let result = glossaries::create_auto_glossary(
        glossary_config_pool,
        glossary_workspace_root,
        CreateAutoGlossaryInput {
            name: format!("{} 自动术语表", task.name),
            source_language: task.source_language.clone(),
            target_language: task.target_language.clone(),
            entries,
        },
    )
    .await;
    match result {
        Ok(view) => {
            request_reporter.log(
                "INFO",
                format!("task={} auto glossary created id={}", task.id, view.id),
            );
            if let Err(error) = set_task_glossary_id(inp_pool, &view.id).await {
                finish_glossary_reporter(
                    request_reporter,
                    reporter_handle,
                    GlossaryReport::Status {
                        current: total_chunks,
                        total: total_chunks,
                        state: "failed".into(),
                        label: "自动术语表绑定失败".into(),
                    },
                )
                .await?;
                return Err(error);
            }
            finish_glossary_reporter(
                request_reporter,
                reporter_handle,
                GlossaryReport::Status {
                    current: total_chunks,
                    total: total_chunks,
                    state: "success".into(),
                    label: format!("术语表建立 ({total_chunks}/{total_chunks})"),
                },
            )
            .await?;
            Ok(AutoGlossaryGeneration::Created(view))
        }
        Err(error) => {
            finish_glossary_reporter(
                request_reporter,
                reporter_handle,
                GlossaryReport::Status {
                    current: total_chunks,
                    total: total_chunks,
                    state: "failed".into(),
                    label: "自动术语表保存失败".into(),
                },
            )
            .await?;
            Err(error)
        }
    }
}

async fn generate_glossary_for_chunk(
    runtime: Arc<GlossaryRuntime>,
    target_language: String,
    document_format: DocumentFormat,
    content_format: ContentFormat,
    chunk: ChunkRecord,
    max_retries: u32,
    quota: Arc<HeaderQuotaPolicy>,
    limiter: Arc<AdaptiveLimiter>,
    manual_limiter: Option<Arc<ManualRateLimiter>>,
    interrupted: TranslationInterrupt,
    reporter: GlossaryRequestReporter,
) -> AutoGlossaryChunkOutcome {
    let chunk_id = chunk.id.clone();
    let completion_reporter = reporter.clone();
    let outcome = generate_glossary_for_chunk_inner(
        runtime,
        target_language,
        document_format,
        content_format,
        chunk,
        max_retries,
        quota,
        limiter,
        manual_limiter,
        interrupted,
        reporter,
    )
    .await;
    completion_reporter.retry(&chunk_id, 0, 0, None).await;
    outcome
}

async fn generate_glossary_for_chunk_inner(
    runtime: Arc<GlossaryRuntime>,
    target_language: String,
    document_format: DocumentFormat,
    content_format: ContentFormat,
    chunk: ChunkRecord,
    max_retries: u32,
    quota: Arc<HeaderQuotaPolicy>,
    limiter: Arc<AdaptiveLimiter>,
    manual_limiter: Option<Arc<ManualRateLimiter>>,
    interrupted: TranslationInterrupt,
    reporter: GlossaryRequestReporter,
) -> AutoGlossaryChunkOutcome {
    let mut last_error = None;
    for attempt in 0..=max_retries {
        if interrupted.is_interrupted() {
            return AutoGlossaryChunkOutcome::Interrupted {
                error: interrupted
                    .reason()
                    .unwrap_or_else(|| "Task interrupted".to_string()),
            };
        }
        reporter
            .status(format!(
                "等待自动术语表请求槽位... ({}/{})",
                reporter.completed_chunks.load(Ordering::SeqCst),
                reporter.total_chunks
            ))
            .await;
        let Some(_permit) = limiter.acquire(interrupted.token()).await else {
            return AutoGlossaryChunkOutcome::Interrupted {
                error: interrupted
                    .reason()
                    .unwrap_or_else(|| "Task interrupted".to_string()),
            };
        };
        let prompt = build_glossary_prompt(GlossaryPromptInput {
            target_language: target_language.clone(),
            assistant_system_prompt: runtime.assistant_prompt.clone(),
            chunk: TaskChunkInput {
                text: chunk.source_text.clone(),
                document_format,
                content_format,
            },
        });
        let messages = match prompt {
            Ok(GlossaryPromptBuildResult::Request { messages }) => messages,
            Ok(GlossaryPromptBuildResult::Skipped { .. }) => {
                return AutoGlossaryChunkOutcome::Success {
                    sequence: chunk.sequence,
                    entries: Vec::new(),
                };
            }
            Err(error) => {
                return AutoGlossaryChunkOutcome::Failed {
                    sequence: chunk.sequence,
                    error,
                };
            }
        };
        let request = UnifiedChatRequest {
            model: runtime.model_request_name.clone(),
            messages,
            web_search: runtime.web_search,
            thinking: runtime.thinking.clone(),
            max_output_tokens: None,
            temperature: runtime.temperature,
            top_p: runtime.top_p,
            stream: false,
            logprobs: false,
            custom_parameters: runtime.assistant_custom_parameters.clone(),
        };
        let estimated_tokens = estimate_tokens(&chunk.source_text) + 512;
        if let Some(manual_limiter) = manual_limiter.as_ref() {
            loop {
                match manual_limiter.reserve_or_delay(estimated_tokens).await {
                    Some(delay) => {
                        reporter.log(
                            "INFO",
                            format!(
                                "chunk={} sequence={} manual rate-limit wait_ms={}",
                                chunk.id,
                                chunk.sequence,
                                delay.as_millis(),
                            ),
                        );
                        if !wait_with_countdown(
                            delay,
                            &reporter,
                            &chunk,
                            attempt,
                            max_retries,
                            "手动限流等待",
                            &interrupted,
                        )
                        .await
                        {
                            return AutoGlossaryChunkOutcome::Interrupted {
                                error: interrupted
                                    .reason()
                                    .unwrap_or_else(|| "Task interrupted".to_string()),
                            };
                        }
                    }
                    None => break,
                }
            }
        }
        if let Some(delay) = quota.wait_duration(estimated_tokens).await {
            reporter.log(
                "INFO",
                format!(
                    "chunk={} sequence={} provider quota wait_ms={}",
                    chunk.id,
                    chunk.sequence,
                    delay.as_millis(),
                ),
            );
            if !wait_with_countdown(
                delay,
                &reporter,
                &chunk,
                attempt,
                max_retries,
                "服务商限流等待",
                &interrupted,
            )
            .await
            {
                return AutoGlossaryChunkOutcome::Interrupted {
                    error: interrupted
                        .reason()
                        .unwrap_or_else(|| "Task interrupted".to_string()),
                };
            }
        }
        reporter
            .status(format!(
                "正在请求自动术语表... ({}/{})",
                reporter.completed_chunks.load(Ordering::SeqCst),
                reporter.total_chunks
            ))
            .await;
        reporter.log(
            "INFO",
            format!(
                "chunk={} sequence={} attempt={}/{} requesting model=\"{}\" estimated_tokens={estimated_tokens}",
                chunk.id,
                chunk.sequence,
                attempt + 1,
                max_retries + 1,
                runtime.model_request_name,
            ),
        );
        reporter
            .status(format!(
                "等待术语表服务响应... ({}/{})",
                reporter.completed_chunks.load(Ordering::SeqCst),
                reporter.total_chunks
            ))
            .await;
        let request_started = Instant::now();
        let request_result = tokio::select! {
            result = runtime.adapter.send_chat_with_meta(&request) => Some(result),
            _ = interrupted.cancelled() => None,
        };
        let Some(request_result) = request_result else {
            reporter.log(
                "WARN",
                format!(
                    "chunk={} sequence={} attempt={}/{} cancelled after {}ms",
                    chunk.id,
                    chunk.sequence,
                    attempt + 1,
                    max_retries + 1,
                    request_started.elapsed().as_millis(),
                ),
            );
            return AutoGlossaryChunkOutcome::Interrupted {
                error: interrupted
                    .reason()
                    .unwrap_or_else(|| "Task interrupted".to_string()),
            };
        };
        match request_result {
            Ok(meta) => {
                reporter.log(
                    "INFO",
                    format!(
                        "chunk={} sequence={} attempt={}/{} response received in {}ms",
                        chunk.id,
                        chunk.sequence,
                        attempt + 1,
                        max_retries + 1,
                        request_started.elapsed().as_millis(),
                    ),
                );
                quota.update(&meta.rate_limits).await;
                limiter
                    .on_result(meta.rate_limits.has_quota_headers(), true, false)
                    .await;
                match sanitize_and_flatten_glossary(&meta.response.text, Some(&chunk.source_text)) {
                    Ok(parsed) => {
                        return AutoGlossaryChunkOutcome::Success {
                            sequence: chunk.sequence,
                            entries: parsed.entries,
                        }
                    }
                    Err(error) => {
                        reporter.log(
                            "WARN",
                            format!(
                                "chunk={} sequence={} attempt={}/{} response parse failed: {error}",
                                chunk.id,
                                chunk.sequence,
                                attempt + 1,
                                max_retries + 1,
                            ),
                        );
                        last_error = Some(error);
                    }
                }
            }
            Err(error) => {
                reporter.log(
                    if error.is_transient() {
                        "WARN"
                    } else {
                        "ERROR"
                    },
                    format!(
                        "chunk={} sequence={} attempt={}/{} request failed after {}ms: {error}",
                        chunk.id,
                        chunk.sequence,
                        attempt + 1,
                        max_retries + 1,
                        request_started.elapsed().as_millis(),
                    ),
                );
                quota.update(&error.rate_limits).await;
                limiter
                    .on_result(
                        error.rate_limits.has_quota_headers(),
                        false,
                        error.is_rate_limited(),
                    )
                    .await;
                if error.is_model_unavailable() {
                    return AutoGlossaryChunkOutcome::Interrupted {
                        error: format!("MODEL_UNAVAILABLE:GLOSSARY:{error}"),
                    };
                }
                let is_transient = error.is_transient();
                if !is_transient {
                    return AutoGlossaryChunkOutcome::Interrupted {
                        error: error.to_string(),
                    };
                }
                last_error = Some(error.to_string());
                if attempt < max_retries {
                    let base_delay = transient_retry_base_delay_ms(&error, attempt);
                    let sleep_ms = retry_delay_with_jitter_ms(base_delay);
                    reporter.log(
                        "WARN",
                        format!(
                            "chunk={} sequence={} attempt={}/{} retry backoff_ms={sleep_ms}",
                            chunk.id,
                            chunk.sequence,
                            attempt + 1,
                            max_retries + 1,
                        ),
                    );
                    reporter
                        .retry(
                            &chunk.id,
                            attempt + 1,
                            max_retries,
                            Some(format!("术语表请求失败，准备重试：{error}")),
                        )
                        .await;
                    if !wait_with_countdown(
                        Duration::from_millis(sleep_ms),
                        &reporter,
                        &chunk,
                        attempt,
                        max_retries,
                        "术语表请求重试",
                        &interrupted,
                    )
                    .await
                    {
                        return AutoGlossaryChunkOutcome::Interrupted {
                            error: interrupted
                                .reason()
                                .unwrap_or_else(|| "Task interrupted".to_string()),
                        };
                    }
                    continue;
                }
            }
        }
        if attempt < max_retries {
            let base_delay = retry_base_delay_ms(attempt);
            let sleep_ms = retry_delay_with_jitter_ms(base_delay);
            reporter.log(
                "WARN",
                format!(
                    "chunk={} sequence={} attempt={}/{} response retry backoff_ms={sleep_ms}",
                    chunk.id,
                    chunk.sequence,
                    attempt + 1,
                    max_retries + 1,
                ),
            );
            reporter
                .retry(
                    &chunk.id,
                    attempt + 1,
                    max_retries,
                    Some(
                        last_error
                            .as_deref()
                            .unwrap_or("术语表响应格式无效")
                            .to_string(),
                    ),
                )
                .await;
            if !wait_with_countdown(
                Duration::from_millis(sleep_ms),
                &reporter,
                &chunk,
                attempt,
                max_retries,
                "术语表响应重试",
                &interrupted,
            )
            .await
            {
                return AutoGlossaryChunkOutcome::Interrupted {
                    error: interrupted
                        .reason()
                        .unwrap_or_else(|| "Task interrupted".to_string()),
                };
            }
        }
    }
    AutoGlossaryChunkOutcome::Failed {
        sequence: chunk.sequence,
        error: last_error.unwrap_or_else(|| "Auto glossary generation failed".to_string()),
    }
}

async fn wait_with_countdown(
    delay: Duration,
    reporter: &GlossaryRequestReporter,
    chunk: &ChunkRecord,
    attempt: u32,
    max_retries: u32,
    reason: &str,
    interrupted: &TranslationInterrupt,
) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        let remaining_seconds = remaining.as_secs().max(1);
        let message = format!("{reason}，约 {remaining_seconds} 秒后继续");
        reporter.status(message.clone()).await;
        reporter
            .retry(&chunk.id, attempt + 1, max_retries.max(1), Some(message))
            .await;
        let tick = remaining.min(Duration::from_secs(1));
        tokio::select! {
            _ = tokio::time::sleep(tick) => {}
            _ = interrupted.cancelled() => return false,
        }
    }
}

fn valid_glossary_match_boundary(
    chunk_text: &str,
    start: usize,
    end: usize,
    entry: &GlossaryEntry,
) -> bool {
    if !chunk_text.is_char_boundary(start) || !chunk_text.is_char_boundary(end) {
        return false;
    }

    let needs_start_boundary = entry.src.chars().next().is_some_and(is_ascii_word_char);
    let needs_end_boundary = entry
        .src
        .chars()
        .next_back()
        .is_some_and(is_ascii_word_char);

    if needs_start_boundary {
        let char_before = chunk_text[..start].chars().next_back();
        if char_before.is_some_and(is_ascii_word_char) {
            return false;
        }
    }
    if needs_end_boundary {
        let char_after = chunk_text[end..].chars().next();
        if char_after.is_some_and(is_ascii_word_char) {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod report_state_tests {
    use super::*;

    #[test]
    fn glossary_report_state_does_not_regress_progress() {
        let mut state = GlossaryReportState::default();
        state.apply(GlossaryReport::Status {
            current: 3,
            total: 8,
            state: "running".into(),
            label: "3/8".into(),
        });
        state.apply(GlossaryReport::Status {
            current: 2,
            total: 8,
            state: "running".into(),
            label: "2/8".into(),
        });
        assert_eq!(state.progress.as_ref().map(|value| value.current), Some(3));
    }

    #[test]
    fn glossary_report_state_clears_only_matching_retry() {
        let mut state = GlossaryReportState::default();
        for chunk_id in ["chunk-a", "chunk-b"] {
            state.apply(GlossaryReport::Retry {
                chunk_id: chunk_id.into(),
                current: 1,
                max: 3,
                message: Some(format!("retry {chunk_id}")),
            });
        }
        assert_eq!(
            state.latest_retry().map(|value| value.chunk_id.as_str()),
            Some("chunk-b")
        );
        state.apply(GlossaryReport::Retry {
            chunk_id: "chunk-a".into(),
            current: 0,
            max: 0,
            message: None,
        });
        assert_eq!(
            state.latest_retry().map(|value| value.chunk_id.as_str()),
            Some("chunk-b")
        );
        state.apply(GlossaryReport::Retry {
            chunk_id: "chunk-b".into(),
            current: 0,
            max: 0,
            message: None,
        });
        assert!(state.latest_retry().is_none());
    }
}

pub(super) fn is_ascii_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn spans_overlap(left_start: usize, left_end: usize, right_start: usize, right_end: usize) -> bool {
    left_start < right_end && right_start < left_end
}
