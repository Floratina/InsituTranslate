const CONFIG_DB_FILE: &str = "config.db";
const TASKS_DIR: &str = "tasks";
const DEFAULT_CHUNK_TOKEN_LIMIT: i64 = 800;
const DEFAULT_MAX_CONCURRENCY: i64 = 5;
const DEFAULT_MAX_RETRIES: i64 = 5;
const DEFAULT_MAX_FAILURE_PERCENTAGE: i64 = 20;
const DEFAULT_MAX_REQUESTS_PER_MINUTE: i64 = 60;
const DEFAULT_MAX_TOKENS_PER_MINUTE: i64 = 60_000;
const INP_SCHEMA_VERSION: i64 = 12;
const GLOBAL_BACKGROUND_TARGET_TOKENS: u64 = 1000;
const GLOBAL_BACKGROUND_BATCH_CHUNKS: i64 = 20;
const MAX_TASK_TAGS: usize = 12;
const MAX_TASK_TAG_LENGTH: usize = 48;
const MAX_TASK_NAME_LENGTH: usize = 120;
const LEGACY_TRANSLATION_FAILURE_PERCENTAGE: i64 = 30;
const LEGACY_GLOSSARY_FAILURE_PERCENTAGE: i64 = 40;
const TRANSLATION_PROGRESS_EVENT: &str = "translation-progress";
const TRANSLATION_TASK_CREATION_PROGRESS_EVENT: &str = "translation-task-creation-progress";
const INP_FILE_DAMAGED: &str = "INP_FILE_DAMAGED";
const SOURCE_FILE_UNAVAILABLE: &str = "Source file is not embedded in this .inp and the original source path is no longer readable. Recreate the task from the original document to retranslate or export it.";

fn failure_threshold_exceeded(
    failed_chunks: i64,
    total_chunks: i64,
    max_failure_percentage: i64,
) -> Result<bool, String> {
    if total_chunks <= 0 {
        return Err("Task contains no translatable chunks".into());
    }
    if failed_chunks < 0 || failed_chunks > total_chunks {
        return Err("Stored task chunk counts are invalid".into());
    }
    if !(0..=100).contains(&max_failure_percentage) {
        return Err("Maximum failure percentage must be between 0 and 100".into());
    }
    Ok((failed_chunks as u128) * 100 > (total_chunks as u128) * (max_failure_percentage as u128))
}

mod context;
mod db;
mod glossary;
mod limiter;
mod request_options;
mod scheduler;
mod types;

pub use self::glossary::prepare_auto_glossary_for_task;
pub use self::scheduler::{
    mark_task_failed_after_runtime_error, prepare_translation_run, run_translation_task,
};

pub use self::db::{
    backfill_task_index_execution_fields, connect_config_db, create_translation_task,
    create_translation_task_with_progress, default_workspace_root, delete_translation_task,
    delete_translation_tasks, discard_staged_translation_task, export_translation_task,
    get_task_runtime_action_required, get_translation_config, get_translation_task_detail,
    get_translation_task_summary, import_translation_task, list_translation_tasks,
    mark_task_index_failed, mark_task_interrupted, mark_task_interrupted_pending,
    mark_tasks_queued_atomically, migrate_legacy_workspace, open_translation_task_folder,
    publish_staged_translation_task, rebase_task_index_paths, replace_task_runtime_snapshot,
    reset_task_for_retranslation, restore_queued_tasks, update_translation_config_validated,
    update_translation_task_info, update_translation_task_name, update_translation_task_tags,
};

#[cfg(test)]
pub use self::db::update_translation_config;

#[allow(unused_imports)]
pub use self::types::{
    ConfidenceMode, ContextHandlingMode, CreateTranslationTaskInput, ExportTranslationTaskInput,
    GlossaryGenerationConfig, GlossaryMode, ImportTranslationTaskInput, PreparedRun,
    ProgressDetail, ProgressStep, RateLimitStrategy, ReplaceTaskRuntimeSnapshotInput, RunMode,
    StartTranslationTaskCreationResult, TaskRuntimeActionReason, TaskRuntimeActionRequired,
    TaskRuntimeConfigDomain, TextTokenStats, TokenStats, TranslationChunkStatus,
    TranslationChunkView, TranslationConfigView, TranslationInterrupt, TranslationProgressPayload,
    TranslationTaskActiveRetry, TranslationTaskCreationProgressPayload,
    TranslationTaskCreationStage, TranslationTaskCreationStatus, TranslationTaskDetail,
    TranslationTaskExportFormat, TranslationTaskFilters, TranslationTaskIdsInput,
    TranslationTaskPdfOptions, TranslationTaskStatus, TranslationTaskView,
    UpdateTranslationConfigInput, UpdateTranslationTaskInfoInput, UpdateTranslationTaskNameInput,
    UpdateTranslationTaskTagsInput,
};

#[cfg(test)]
mod tests;
