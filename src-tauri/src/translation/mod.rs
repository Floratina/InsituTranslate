const CONFIG_DB_FILE: &str = "config.db";
const TASKS_DIR: &str = "tasks";
const DEFAULT_CHUNK_TOKEN_LIMIT: i64 = 800;
const DEFAULT_MAX_CONCURRENCY: i64 = 5;
const DEFAULT_MAX_RETRIES: i64 = 5;
const DEFAULT_MAX_REQUESTS_PER_MINUTE: i64 = 60;
const DEFAULT_MAX_TOKENS_PER_MINUTE: i64 = 60_000;
const INP_SCHEMA_VERSION: i64 = 7;
const GLOBAL_BACKGROUND_TARGET_TOKENS: u64 = 1000;
const GLOBAL_BACKGROUND_BATCH_CHUNKS: i64 = 20;
const MAX_TASK_TAGS: usize = 12;
const MAX_TASK_TAG_LENGTH: usize = 48;
const MAX_TASK_NAME_LENGTH: usize = 120;
const ERROR_RATE_FAILURE_THRESHOLD: f64 = 0.30;
const AUTO_GLOSSARY_FAILURE_THRESHOLD: f64 = 0.40;
const TRANSLATION_PROGRESS_EVENT: &str = "translation-progress";
const INP_FILE_DAMAGED: &str = "INP_FILE_DAMAGED";
const SOURCE_FILE_UNAVAILABLE: &str = "Source file is not embedded in this .inp and the original source path is no longer readable. Recreate the task from the original document to retranslate or export it.";

mod context;
mod db;
mod glossary;
mod limiter;
mod scheduler;
mod types;

pub use self::glossary::prepare_auto_glossary_for_task;
pub use self::scheduler::{
    mark_task_failed_after_runtime_error, prepare_translation_run, run_translation_task,
};

pub use self::db::{
    connect_config_db, create_translation_task, default_workspace_root, delete_translation_task,
    delete_translation_tasks, export_translation_task, get_translation_config,
    get_translation_task_detail, import_translation_task, list_translation_tasks,
    migrate_legacy_workspace, open_translation_task_folder, rebase_task_index_paths,
    update_translation_config, update_translation_task_name, update_translation_task_tags,
};

#[allow(unused_imports)]
pub use self::types::{
    ConfidenceMode, ContextHandlingMode, CreateTranslationTaskInput, ExportTranslationTaskInput,
    GlossaryMode, ImportTranslationTaskInput, PreparedRun, RateLimitStrategy, RunMode, TokenStats,
    TranslationChunkStatus, TranslationChunkView, TranslationConfigView, TranslationInterrupt,
    TranslationProgressPayload, TranslationTaskDetail, TranslationTaskExportFormat,
    TranslationTaskFilters, TranslationTaskIdsInput, TranslationTaskPdfOptions,
    TranslationTaskStatus, TranslationTaskView, UpdateTranslationConfigInput,
    UpdateTranslationTaskNameInput, UpdateTranslationTaskTagsInput,
};

#[cfg(test)]
mod tests;
