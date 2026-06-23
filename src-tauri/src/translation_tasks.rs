use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::{mpsc, Mutex, Notify};

use crate::adapters::{
    finish_reason_is_truncation, ProviderChatError, ProviderChatMeta, RateLimitTelemetry,
    RuntimeAdapter,
};
use crate::db;
use crate::document_parsing::types::RenderedChunk;
use crate::document_parsing::{self, restore_chunk_for_map};
use crate::domain::{UnifiedChatRequest, UnifiedToolChoice};
use crate::languages::{
    normalize_source_language, normalize_target_language, DEFAULT_SOURCE_LANGUAGE,
    DEFAULT_TARGET_LANGUAGE,
};
use crate::task_prompt::{ContentFormat, DocumentFormat, TaskChunkInput};
use crate::translation_prompt::{
    build_translation_prompt, TranslationPromptBuildResult, TranslationPromptInput,
};

const CONFIG_DB_FILE: &str = "config.db";
const TASKS_DIR: &str = "tasks";
const DEFAULT_CHUNK_TOKEN_LIMIT: i64 = 800;
const DEFAULT_MAX_CONCURRENCY: i64 = 5;
const DEFAULT_MAX_RETRIES: i64 = 5;
const DEFAULT_MAX_REQUESTS_PER_MINUTE: i64 = 60;
const DEFAULT_MAX_TOKENS_PER_MINUTE: i64 = 60_000;
const INP_SCHEMA_VERSION: i64 = 3;
const MAX_TASK_TAGS: usize = 12;
const MAX_TASK_TAG_LENGTH: usize = 48;
const MAX_TASK_NAME_LENGTH: usize = 120;
const ERROR_RATE_FAILURE_THRESHOLD: f64 = 0.30;
const TRANSLATION_PROGRESS_EVENT: &str = "translation-progress";
const INP_FILE_DAMAGED: &str = "INP_FILE_DAMAGED";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TranslationTaskStatus {
    Pending,
    Running,
    Interrupted,
    Failed,
    Success,
}

impl TranslationTaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::Success => "success",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "interrupted" => Ok(Self::Interrupted),
            "failed" => Ok(Self::Failed),
            "success" => Ok(Self::Success),
            _ => Err(format!("Unsupported translation task status: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TranslationChunkStatus {
    Pending,
    Interrupted,
    Failed,
    Success,
}

impl TranslationChunkStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::Success => "success",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pending" => Ok(Self::Pending),
            "interrupted" => Ok(Self::Interrupted),
            "failed" => Ok(Self::Failed),
            "success" => Ok(Self::Success),
            _ => Err(format!("Unsupported translation chunk status: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RateLimitStrategy {
    Dynamic,
    Manual,
}

impl RateLimitStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Manual => "manual",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "dynamic" => Ok(Self::Dynamic),
            "manual" => Ok(Self::Manual),
            _ => Err(format!("Unsupported rate limit strategy: {value}")),
        }
    }
}

impl Default for RateLimitStrategy {
    fn default() -> Self {
        Self::Dynamic
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GlossaryMode {
    Auto,
    Existing,
}

impl Default for GlossaryMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConfidenceMode {
    Off,
    ConfidenceIndex,
}

impl ConfidenceMode {
    fn enabled(self) -> bool {
        self == Self::ConfidenceIndex
    }
}

impl Default for ConfidenceMode {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub thinking_tokens: u64,
    pub total_tokens: u64,
}

impl TokenStats {
    fn add(&mut self, other: &TokenStats) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cached_tokens += other.cached_tokens;
        self.thinking_tokens += other.thinking_tokens;
        self.total_tokens += other.total_tokens;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TranslationConfigView {
    pub source_language: String,
    pub custom_source_language: String,
    pub target_language: String,
    pub custom_target_language: String,
    pub provider_id: String,
    pub model_id: String,
    pub assistant_id: String,
    pub chunk_token_limit: i64,
    pub max_concurrency: i64,
    pub max_retries: i64,
    pub rate_limit_strategy: RateLimitStrategy,
    pub max_requests_per_minute: i64,
    pub max_tokens_per_minute: i64,
    pub use_glossary: bool,
    pub glossary_mode: GlossaryMode,
    pub glossary_id: Option<String>,
    pub confidence_mode: ConfidenceMode,
}

impl Default for TranslationConfigView {
    fn default() -> Self {
        Self {
            source_language: DEFAULT_SOURCE_LANGUAGE.into(),
            custom_source_language: String::new(),
            target_language: DEFAULT_TARGET_LANGUAGE.into(),
            custom_target_language: String::new(),
            provider_id: String::new(),
            model_id: String::new(),
            assistant_id: "__none__".into(),
            chunk_token_limit: DEFAULT_CHUNK_TOKEN_LIMIT,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            max_retries: DEFAULT_MAX_RETRIES,
            rate_limit_strategy: RateLimitStrategy::Dynamic,
            max_requests_per_minute: DEFAULT_MAX_REQUESTS_PER_MINUTE,
            max_tokens_per_minute: DEFAULT_MAX_TOKENS_PER_MINUTE,
            use_glossary: false,
            glossary_mode: GlossaryMode::Auto,
            glossary_id: None,
            confidence_mode: ConfidenceMode::Off,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTranslationConfigInput {
    pub source_language: String,
    pub custom_source_language: String,
    pub target_language: String,
    pub custom_target_language: String,
    pub provider_id: String,
    pub model_id: String,
    pub assistant_id: String,
    pub chunk_token_limit: i64,
    pub max_concurrency: i64,
    pub max_retries: i64,
    pub rate_limit_strategy: RateLimitStrategy,
    pub max_requests_per_minute: i64,
    pub max_tokens_per_minute: i64,
    pub use_glossary: bool,
    pub glossary_mode: GlossaryMode,
    pub glossary_id: Option<String>,
    #[serde(default)]
    pub confidence_mode: ConfidenceMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTranslationTaskInput {
    pub file_path: String,
    pub source_language: String,
    pub target_language: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub provider_id: String,
    pub model_id: String,
    pub assistant_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationTaskFilters {
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub source_language: Option<String>,
    #[serde(default)]
    pub target_language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTranslationTaskTagsInput {
    pub id: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportTranslationTaskInput {
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTranslationTaskNameInput {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationTaskIdsInput {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTranslationTaskInput {
    pub id: String,
    pub format: TranslationTaskExportFormat,
    pub output_name: String,
    pub pdf_options: Option<TranslationTaskPdfOptions>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TranslationTaskExportFormat {
    Source,
    Pdf,
    PdfBilingual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationTaskPdfOptions {
    pub page_size: String,
    pub margin: String,
    pub scale: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationTaskView {
    pub id: String,
    pub name: String,
    pub inp_path: String,
    pub source_path: String,
    pub source_language: String,
    pub target_language: String,
    pub status: TranslationTaskStatus,
    pub progress: f64,
    pub provider_id: String,
    pub model_id: String,
    pub model_request_name: String,
    pub assistant_id: Option<String>,
    pub tags: Vec<String>,
    pub total_chunks: i64,
    pub completed_chunks: i64,
    pub failed_chunks: i64,
    pub interrupted_chunks: i64,
    pub token_stats: TokenStats,
    pub error_rate: f64,
    pub last_error: Option<String>,
    pub rate_limit_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationChunkView {
    pub id: String,
    pub sequence: i64,
    pub map_json: String,
    pub preprocessed_text: String,
    pub source_text: String,
    pub after_translate_text: String,
    pub translated_text: String,
    pub confidence: Option<f64>,
    pub status: TranslationChunkStatus,
    pub retry_count: i64,
    pub error_message: Option<String>,
    pub token_stats: TokenStats,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationTaskDetail {
    pub task: TranslationTaskView,
    pub chunks: Vec<TranslationChunkView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationProgressPayload {
    pub task: TranslationTaskView,
}

#[derive(Debug, Clone, Copy)]
pub enum RunMode {
    Start,
    Resume,
    Retranslate,
}

#[derive(Debug, Clone)]
pub struct TranslationInterrupt {
    flag: Arc<AtomicBool>,
    reason: Arc<StdMutex<Option<String>>>,
}

impl TranslationInterrupt {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            reason: Arc::new(StdMutex::new(None)),
        }
    }

    pub fn interrupt(&self, reason: impl Into<String>) {
        if let Ok(mut current) = self.reason.lock() {
            *current = Some(reason.into());
        }
        self.flag.store(true, Ordering::SeqCst);
    }

    fn is_interrupted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    fn reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|current| current.clone())
    }
}

#[derive(Debug, Clone)]
pub struct PreparedRun {
    pub task: TranslationTaskView,
    pub inp_path: PathBuf,
    config: TranslationConfigView,
}

#[derive(Debug, Clone)]
struct ChunkRecord {
    id: String,
    source_text: String,
    map_json: String,
}

#[derive(Debug, Clone)]
struct ChunkOutcome {
    chunk_id: String,
    status: TranslationChunkStatus,
    interrupt_task: bool,
    after_translate_text: String,
    translated_text: String,
    retry_count: i64,
    error_message: Option<String>,
    token_stats: TokenStats,
    rate_limit_status: Option<String>,
    confidence: Option<f64>,
}

pub fn default_workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".insitu-workspace")
}

pub fn migrate_legacy_workspace(legacy_root: &Path, workspace_root: &Path) -> Result<(), String> {
    if !legacy_root.exists() {
        std::fs::create_dir_all(workspace_root).map_err(|error| error.to_string())?;
        return Ok(());
    }
    copy_missing_directory(legacy_root, workspace_root)
}

fn copy_missing_directory(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_missing_directory(&source_path, &destination_path)?;
        } else if !destination_path.exists() {
            std::fs::copy(&source_path, &destination_path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

pub async fn rebase_task_index_paths(
    config_pool: &SqlitePool,
    legacy_root: &Path,
    workspace_root: &Path,
) -> Result<(), String> {
    let rows = sqlx::query("SELECT id, inp_path FROM task_index")
        .fetch_all(config_pool)
        .await
        .map_err(|error| error.to_string())?;
    for row in rows {
        let id: String = row.get("id");
        let old_path = PathBuf::from(row.get::<String, _>("inp_path"));
        let Ok(relative_path) = old_path.strip_prefix(legacy_root) else {
            continue;
        };
        let new_path = workspace_root.join(relative_path);
        if !new_path.exists() {
            continue;
        }
        sqlx::query("UPDATE task_index SET inp_path = ? WHERE id = ?")
            .bind(new_path.to_string_lossy().to_string())
            .bind(id)
            .execute(config_pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub async fn connect_config_db(workspace_root: &Path) -> Result<SqlitePool, String> {
    tokio::fs::create_dir_all(workspace_root.join(TASKS_DIR))
        .await
        .map_err(|error| error.to_string())?;
    let pool = connect_sqlite(&workspace_root.join(CONFIG_DB_FILE), 5).await?;
    migrate_config_db(&pool).await?;
    recover_running_tasks(&pool).await?;
    Ok(pool)
}

async fn connect_sqlite(path: &Path, max_connections: u32) -> Result<SqlitePool, String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await
        .map_err(|error| error.to_string())
}

async fn migrate_config_db(pool: &SqlitePool) -> Result<(), String> {
    let statements = [
        r#"CREATE TABLE IF NOT EXISTS task_index (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            inp_path TEXT NOT NULL UNIQUE,
            source_path TEXT NOT NULL,
            source_language TEXT NOT NULL,
            target_language TEXT NOT NULL,
            status TEXT NOT NULL,
            progress REAL NOT NULL DEFAULT 0,
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            model_request_name TEXT NOT NULL,
            assistant_id TEXT,
            tags_json TEXT NOT NULL DEFAULT '[]',
            total_chunks INTEGER NOT NULL DEFAULT 0,
            completed_chunks INTEGER NOT NULL DEFAULT 0,
            failed_chunks INTEGER NOT NULL DEFAULT 0,
            interrupted_chunks INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0,
            thinking_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            error_rate REAL NOT NULL DEFAULT 0,
            last_error TEXT,
            rate_limit_status TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS translation_config (
            id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
            chunk_token_limit INTEGER NOT NULL,
            max_concurrency INTEGER NOT NULL,
            max_retries INTEGER NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS translation_config_migrations (
            id TEXT PRIMARY KEY NOT NULL,
            applied_at TEXT NOT NULL
        )"#,
        "CREATE INDEX IF NOT EXISTS idx_task_index_status ON task_index(status, updated_at)",
        "CREATE INDEX IF NOT EXISTS idx_task_index_source_language ON task_index(source_language)",
        "CREATE INDEX IF NOT EXISTS idx_task_index_target_language ON task_index(target_language)",
    ];
    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    add_column_if_missing(
        pool,
        "task_index",
        "tags_json",
        "TEXT NOT NULL DEFAULT '[]'",
    )
    .await?;
    add_column_if_missing(
        pool,
        "translation_config",
        "rate_limit_strategy",
        "TEXT NOT NULL DEFAULT 'dynamic'",
    )
    .await?;
    add_column_if_missing(
        pool,
        "translation_config",
        "config_json",
        "TEXT NOT NULL DEFAULT ''",
    )
    .await?;
    add_column_if_missing(
        pool,
        "translation_config",
        "max_requests_per_minute",
        "INTEGER NOT NULL DEFAULT 60",
    )
    .await?;
    add_column_if_missing(
        pool,
        "translation_config",
        "max_tokens_per_minute",
        "INTEGER NOT NULL DEFAULT 60000",
    )
    .await?;
    sqlx::query(
        "INSERT INTO translation_config (
            id, chunk_token_limit, max_concurrency, max_retries, rate_limit_strategy,
            max_requests_per_minute, max_tokens_per_minute, updated_at
         ) VALUES (1, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
    )
    .bind(DEFAULT_CHUNK_TOKEN_LIMIT)
    .bind(DEFAULT_MAX_CONCURRENCY)
    .bind(DEFAULT_MAX_RETRIES)
    .bind(RateLimitStrategy::Dynamic.as_str())
    .bind(DEFAULT_MAX_REQUESTS_PER_MINUTE)
    .bind(DEFAULT_MAX_TOKENS_PER_MINUTE)
    .bind(unix_timestamp())
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    migrate_translation_config_defaults(pool).await?;
    backfill_translation_config_json(pool).await?;
    Ok(())
}

async fn backfill_translation_config_json(pool: &SqlitePool) -> Result<(), String> {
    let row = sqlx::query("SELECT * FROM translation_config WHERE id = 1")
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    let config_json: String = row.get("config_json");
    if !config_json.trim().is_empty() {
        serde_json::from_str::<TranslationConfigView>(&config_json)
            .map_err(|error| format!("Stored translation config JSON is invalid: {error}"))?;
        return Ok(());
    }
    let config = legacy_translation_config(&row)?;
    let serialized = serde_json::to_string(&config).map_err(|error| error.to_string())?;
    sqlx::query("UPDATE translation_config SET config_json = ? WHERE id = 1")
        .bind(serialized)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn migrate_translation_config_defaults(pool: &SqlitePool) -> Result<(), String> {
    let migration_id = "translation-defaults-4000-5-5";
    let applied: Option<String> =
        sqlx::query_scalar("SELECT id FROM translation_config_migrations WHERE id = ?")
            .bind(migration_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?;
    if applied.is_some() {
        return Ok(());
    }

    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE translation_config
         SET chunk_token_limit = ?, max_concurrency = ?, max_retries = ?, updated_at = ?
         WHERE chunk_token_limit = 1200 AND max_concurrency = 4 AND max_retries = 2",
    )
    .bind(DEFAULT_CHUNK_TOKEN_LIMIT)
    .bind(DEFAULT_MAX_CONCURRENCY)
    .bind(DEFAULT_MAX_RETRIES)
    .bind(unix_timestamp())
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query("INSERT INTO translation_config_migrations (id, applied_at) VALUES (?, ?)")
        .bind(migration_id)
        .bind(unix_timestamp())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn add_column_if_missing(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    if !rows
        .iter()
        .any(|row| row.get::<String, _>("name") == column)
    {
        sqlx::query(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn connect_inp(path: &Path) -> Result<SqlitePool, String> {
    let pool = connect_sqlite(path, 1).await?;
    migrate_inp_db(&pool).await?;
    Ok(pool)
}

async fn connect_inp_read_only(path: &Path) -> Result<SqlitePool, String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| INP_FILE_DAMAGED.to_string())
}

async fn validate_inp_file(path: &Path) -> Result<TranslationTaskView, String> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("inp"))
        != Some(true)
    {
        return Err(INP_FILE_DAMAGED.into());
    }
    let pool = connect_inp_read_only(path).await?;
    let result = async {
        validate_inp_schema(&pool).await?;
        metadata_task(&pool, path).await
    }
    .await
    .map_err(|_| INP_FILE_DAMAGED.to_string());
    pool.close().await;
    result
}

async fn validate_inp_schema(pool: &SqlitePool) -> Result<(), String> {
    require_columns(
        pool,
        "metadata",
        &[
            "task_id",
            "schema_version",
            "name",
            "source_path",
            "source_language",
            "target_language",
            "status",
            "progress",
            "provider_id",
            "model_id",
            "model_request_name",
            "assistant_id",
            "assistant_system_prompt",
            "tags_json",
            "token_limit",
            "max_concurrency",
            "max_retries",
            "config_snapshot_json",
            "total_chunks",
            "completed_chunks",
            "failed_chunks",
            "interrupted_chunks",
            "input_tokens",
            "output_tokens",
            "cached_tokens",
            "thinking_tokens",
            "total_tokens",
            "error_rate",
            "last_error",
            "rate_limit_status",
            "created_at",
            "updated_at",
        ],
    )
    .await?;
    require_columns(
        pool,
        "chunks",
        &[
            "id",
            "sequence",
            "map_json",
            "preprocessed_text",
            "source_text",
            "after_translate_text",
            "translated_text",
            "confidence",
            "status",
            "retry_count",
            "error_message",
            "input_tokens",
            "output_tokens",
            "cached_tokens",
            "thinking_tokens",
            "total_tokens",
            "updated_at",
        ],
    )
    .await?;

    let metadata_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metadata")
        .fetch_one(pool)
        .await
        .map_err(|_| INP_FILE_DAMAGED.to_string())?;
    if metadata_count != 1 {
        return Err(INP_FILE_DAMAGED.into());
    }

    let row = sqlx::query("SELECT * FROM metadata LIMIT 1")
        .fetch_one(pool)
        .await
        .map_err(|_| INP_FILE_DAMAGED.to_string())?;
    let schema_version: i64 = row.get("schema_version");
    if !(1..=INP_SCHEMA_VERSION).contains(&schema_version) {
        return Err(INP_FILE_DAMAGED.into());
    }
    if schema_version >= 2 {
        require_columns(
            pool,
            "chunks",
            &["map_json", "preprocessed_text", "after_translate_text"],
        )
        .await?;
    }
    TranslationTaskStatus::parse(row.get::<String, _>("status").as_str())
        .map_err(|_| INP_FILE_DAMAGED.to_string())?;
    parse_tags_json(row.get("tags_json")).map_err(|_| INP_FILE_DAMAGED.to_string())?;

    let chunk_rows = sqlx::query("SELECT sequence, status FROM chunks")
        .fetch_all(pool)
        .await
        .map_err(|_| INP_FILE_DAMAGED.to_string())?;
    let mut seen_sequences = std::collections::HashSet::new();
    for row in chunk_rows {
        let sequence: i64 = row.get("sequence");
        if sequence < 0 || !seen_sequences.insert(sequence) {
            return Err(INP_FILE_DAMAGED.into());
        }
        TranslationChunkStatus::parse(row.get::<String, _>("status").as_str())
            .map_err(|_| INP_FILE_DAMAGED.to_string())?;
    }
    Ok(())
}

async fn require_columns(
    pool: &SqlitePool,
    table: &str,
    required_columns: &[&str],
) -> Result<(), String> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .map_err(|_| INP_FILE_DAMAGED.to_string())?;
    if rows.is_empty() {
        return Err(INP_FILE_DAMAGED.into());
    }
    let columns = rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<std::collections::HashSet<_>>();
    if required_columns
        .iter()
        .any(|column| !columns.contains(*column))
    {
        return Err(INP_FILE_DAMAGED.into());
    }
    Ok(())
}

async fn recover_running_tasks(config_pool: &SqlitePool) -> Result<(), String> {
    let rows = sqlx::query("SELECT id, inp_path FROM task_index WHERE status = ?")
        .bind(TranslationTaskStatus::Running.as_str())
        .fetch_all(config_pool)
        .await
        .map_err(|error| error.to_string())?;
    let now = unix_timestamp();
    for row in rows {
        let id: String = row.get("id");
        let inp_path: String = row.get("inp_path");
        sqlx::query(
            "UPDATE task_index SET status = ?, last_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(TranslationTaskStatus::Interrupted.as_str())
        .bind("Application closed while the task was running")
        .bind(&now)
        .bind(&id)
        .execute(config_pool)
        .await
        .map_err(|error| error.to_string())?;
        if let Ok(inp_pool) = connect_inp(Path::new(&inp_path)).await {
            let _ = sqlx::query(
                "UPDATE metadata SET status = ?, last_error = ?, updated_at = ? WHERE task_id = ?",
            )
            .bind(TranslationTaskStatus::Interrupted.as_str())
            .bind("Application closed while the task was running")
            .bind(&now)
            .bind(&id)
            .execute(&inp_pool)
            .await;
            inp_pool.close().await;
        }
    }
    Ok(())
}

async fn migrate_inp_db(pool: &SqlitePool) -> Result<(), String> {
    let statements = [
        r#"CREATE TABLE IF NOT EXISTS metadata (
            task_id TEXT PRIMARY KEY NOT NULL,
            schema_version INTEGER NOT NULL,
            name TEXT NOT NULL,
            source_path TEXT NOT NULL,
            source_language TEXT NOT NULL,
            target_language TEXT NOT NULL,
            status TEXT NOT NULL,
            progress REAL NOT NULL DEFAULT 0,
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            model_request_name TEXT NOT NULL,
            assistant_id TEXT,
            assistant_system_prompt TEXT,
            assistant_custom_parameters_json TEXT NOT NULL DEFAULT '{}',
            tags_json TEXT NOT NULL DEFAULT '[]',
            token_limit INTEGER NOT NULL,
            max_concurrency INTEGER NOT NULL,
            max_retries INTEGER NOT NULL,
            config_snapshot_json TEXT NOT NULL DEFAULT '{}',
            total_chunks INTEGER NOT NULL DEFAULT 0,
            completed_chunks INTEGER NOT NULL DEFAULT 0,
            failed_chunks INTEGER NOT NULL DEFAULT 0,
            interrupted_chunks INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0,
            thinking_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            error_rate REAL NOT NULL DEFAULT 0,
            last_error TEXT,
            rate_limit_status TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS chunks (
            id TEXT PRIMARY KEY NOT NULL,
            sequence INTEGER NOT NULL,
            map_json TEXT NOT NULL DEFAULT '{}',
            preprocessed_text TEXT NOT NULL DEFAULT '',
            source_text TEXT NOT NULL,
            after_translate_text TEXT NOT NULL DEFAULT '',
            translated_text TEXT NOT NULL DEFAULT '',
            confidence REAL DEFAULT NULL,
            status TEXT NOT NULL,
            retry_count INTEGER NOT NULL DEFAULT 0,
            error_message TEXT,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0,
            thinking_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        )"#,
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_sequence ON chunks(sequence)",
        "CREATE INDEX IF NOT EXISTS idx_chunks_status ON chunks(status, sequence)",
    ];
    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    add_column_if_missing(pool, "metadata", "tags_json", "TEXT NOT NULL DEFAULT '[]'").await?;
    add_column_if_missing(
        pool,
        "metadata",
        "assistant_custom_parameters_json",
        "TEXT NOT NULL DEFAULT '{}'",
    )
    .await?;
    add_column_if_missing(pool, "chunks", "map_json", "TEXT NOT NULL DEFAULT '{}'").await?;
    add_column_if_missing(
        pool,
        "chunks",
        "preprocessed_text",
        "TEXT NOT NULL DEFAULT ''",
    )
    .await?;
    add_column_if_missing(
        pool,
        "chunks",
        "after_translate_text",
        "TEXT NOT NULL DEFAULT ''",
    )
    .await?;
    add_column_if_missing(pool, "chunks", "confidence", "REAL DEFAULT NULL").await?;
    sqlx::query("UPDATE metadata SET schema_version = ? WHERE schema_version < ?")
        .bind(INP_SCHEMA_VERSION)
        .bind(INP_SCHEMA_VERSION)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn get_translation_config(
    config_pool: &SqlitePool,
) -> Result<TranslationConfigView, String> {
    let row = sqlx::query("SELECT * FROM translation_config WHERE id = 1")
        .fetch_one(config_pool)
        .await
        .map_err(|error| error.to_string())?;
    let config_json: String = row.get("config_json");
    let config = if config_json.trim().is_empty() {
        legacy_translation_config(&row)?
    } else {
        serde_json::from_str::<TranslationConfigView>(&config_json)
            .map_err(|error| format!("Stored translation config JSON is invalid: {error}"))?
    };
    let config = normalize_translation_config(config);
    validate_translation_config(&config)?;
    Ok(config)
}

fn legacy_translation_config(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<TranslationConfigView, String> {
    Ok(TranslationConfigView {
        chunk_token_limit: row.get("chunk_token_limit"),
        max_concurrency: row.get("max_concurrency"),
        max_retries: row.get("max_retries"),
        rate_limit_strategy: RateLimitStrategy::parse(row.get("rate_limit_strategy"))?,
        max_requests_per_minute: row.get("max_requests_per_minute"),
        max_tokens_per_minute: row.get("max_tokens_per_minute"),
        ..TranslationConfigView::default()
    })
}

fn normalize_translation_config(mut config: TranslationConfigView) -> TranslationConfigView {
    config.source_language = if config.source_language == "__other__" {
        DEFAULT_SOURCE_LANGUAGE.to_string()
    } else {
        normalize_source_language(&config.source_language)
            .unwrap_or_else(|_| DEFAULT_SOURCE_LANGUAGE.to_string())
    };
    config.custom_source_language.clear();
    config.target_language = if config.target_language == "__other__" {
        DEFAULT_TARGET_LANGUAGE.to_string()
    } else {
        normalize_target_language(&config.target_language)
            .unwrap_or_else(|_| DEFAULT_TARGET_LANGUAGE.to_string())
    };
    config.custom_target_language.clear();
    config
}

fn validate_translation_config(config: &TranslationConfigView) -> Result<(), String> {
    normalize_source_language(&config.source_language)?;
    normalize_target_language(&config.target_language)?;
    validate_saved_selection("Provider", &config.provider_id)?;
    validate_saved_selection("Model", &config.model_id)?;
    validate_saved_selection("Assistant", &config.assistant_id)?;
    if !(200..=8000).contains(&config.chunk_token_limit) {
        return Err("Chunk token limit must be between 200 and 8000".into());
    }
    if !(1..=32).contains(&config.max_concurrency) {
        return Err("Maximum concurrency must be between 1 and 32".into());
    }
    if !(0..=10).contains(&config.max_retries) {
        return Err("Maximum retries must be between 0 and 10".into());
    }
    if !(1..=1_000_000).contains(&config.max_requests_per_minute) {
        return Err("Maximum requests per minute must be between 1 and 1000000".into());
    }
    if !(1..=100_000_000).contains(&config.max_tokens_per_minute) {
        return Err("Maximum tokens per minute must be between 1 and 100000000".into());
    }
    if let Some(glossary_id) = config.glossary_id.as_deref() {
        validate_saved_selection("Glossary", glossary_id)?;
    }
    if config.use_glossary
        && matches!(config.glossary_mode, GlossaryMode::Existing)
        && config
            .glossary_id
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        return Err("Glossary selection is required when using an existing glossary".into());
    }
    Ok(())
}

fn validate_saved_selection(label: &str, value: &str) -> Result<(), String> {
    if value.len() > 255 || value.chars().any(char::is_control) {
        return Err(format!("{label} selection is invalid"));
    }
    Ok(())
}

pub async fn update_translation_config(
    config_pool: &SqlitePool,
    input: UpdateTranslationConfigInput,
) -> Result<TranslationConfigView, String> {
    let config = TranslationConfigView {
        source_language: normalize_source_language(&input.source_language)?,
        custom_source_language: String::new(),
        target_language: normalize_target_language(&input.target_language)?,
        custom_target_language: String::new(),
        provider_id: input.provider_id,
        model_id: input.model_id,
        assistant_id: input.assistant_id,
        chunk_token_limit: input.chunk_token_limit,
        max_concurrency: input.max_concurrency,
        max_retries: input.max_retries,
        rate_limit_strategy: input.rate_limit_strategy,
        max_requests_per_minute: input.max_requests_per_minute,
        max_tokens_per_minute: input.max_tokens_per_minute,
        use_glossary: input.use_glossary,
        glossary_mode: input.glossary_mode,
        glossary_id: input
            .glossary_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        confidence_mode: input.confidence_mode,
    };
    validate_translation_config(&config)?;
    let config_json = serde_json::to_string(&config).map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE translation_config
         SET chunk_token_limit = ?, max_concurrency = ?, max_retries = ?,
             rate_limit_strategy = ?, max_requests_per_minute = ?,
             max_tokens_per_minute = ?, config_json = ?, updated_at = ?
         WHERE id = 1",
    )
    .bind(config.chunk_token_limit)
    .bind(config.max_concurrency)
    .bind(config.max_retries)
    .bind(config.rate_limit_strategy.as_str())
    .bind(config.max_requests_per_minute)
    .bind(config.max_tokens_per_minute)
    .bind(config_json)
    .bind(unix_timestamp())
    .execute(config_pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(config)
}

pub async fn create_translation_task(
    provider_pool: &SqlitePool,
    config_pool: &SqlitePool,
    workspace_root: &Path,
    input: CreateTranslationTaskInput,
) -> Result<TranslationTaskView, String> {
    let source_language = normalize_source_language(&input.source_language)?;
    let target_language = normalize_target_language(&input.target_language)?;
    let tags = normalize_tags(input.tags)?;
    let tags_json = serialize_tags(&tags)?;
    let source_path = PathBuf::from(input.file_path.trim());
    validate_supported_source_file(&source_path)?;
    let model = db::get_model(provider_pool, &input.model_id).await?;
    if model.provider_id != input.provider_id {
        return Err("Selected model does not belong to the selected provider".into());
    }
    let (assistant_prompt, assistant_custom_parameters) = match input
        .assistant_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        Some(id) => {
            let assistant = db::get_assistant(provider_pool, id).await?;
            (Some(assistant.system_prompt), assistant.custom_parameters)
        }
        None => (None, json!({})),
    };
    let config = get_translation_config(config_pool).await?;
    let task_id = db::new_id("task");
    let display_name = display_name_from_path(&source_path);
    let inp_path = next_inp_path(workspace_root, &display_name).await?;
    let chunks =
        document_parsing::parse_source_file(&task_id, &source_path, config.chunk_token_limit)?;
    let created_at = unix_timestamp();
    let inp_pool = connect_inp(&inp_path).await?;
    let config_snapshot = config_snapshot_json(&config, &input.provider_id, &model.id);
    let mut transaction = inp_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO metadata (
            task_id, schema_version, name, source_path, source_language, target_language, status,
            progress, provider_id, model_id, model_request_name, assistant_id, assistant_system_prompt,
            assistant_custom_parameters_json, tags_json,
            token_limit, max_concurrency, max_retries, config_snapshot_json, total_chunks,
            created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&task_id)
    .bind(INP_SCHEMA_VERSION)
    .bind(&display_name)
    .bind(source_path.to_string_lossy().to_string())
    .bind(&source_language)
    .bind(&target_language)
    .bind(TranslationTaskStatus::Pending.as_str())
    .bind(&input.provider_id)
    .bind(&model.id)
    .bind(&model.request_name)
    .bind(input.assistant_id.as_deref().filter(|value| !value.is_empty()))
    .bind(assistant_prompt.as_deref())
    .bind(assistant_custom_parameters.to_string())
    .bind(tags_json)
    .bind(config.chunk_token_limit)
    .bind(config.max_concurrency)
    .bind(config.max_retries)
    .bind(config_snapshot)
    .bind(chunks.len() as i64)
    .bind(&created_at)
    .bind(&created_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;

    for chunk in chunks {
        sqlx::query(
            "INSERT INTO chunks (
                id, sequence, map_json, preprocessed_text, source_text,
                after_translate_text, translated_text, status, retry_count,
                input_tokens, output_tokens, cached_tokens, thinking_tokens, total_tokens, updated_at
             ) VALUES (?, ?, ?, ?, ?, '', '', ?, 0, 0, 0, 0, 0, 0, ?)",
        )
        .bind(format!("{task_id}_chunk_{:06}", chunk.sequence))
        .bind(chunk.sequence)
        .bind(chunk.map_json)
        .bind(chunk.preprocessed_text)
        .bind(chunk.source_text)
        .bind(TranslationChunkStatus::Pending.as_str())
        .bind(&created_at)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    let view = refresh_task_stats(&inp_pool, config_pool, &inp_path, None).await?;
    inp_pool.close().await;
    Ok(view)
}

pub async fn import_translation_task(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    input: ImportTranslationTaskInput,
) -> Result<TranslationTaskView, String> {
    let source_path = PathBuf::from(input.file_path.trim());
    let source_task = validate_inp_file(&source_path).await?;
    if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_index WHERE id = ?")
        .bind(&source_task.id)
        .fetch_one(config_pool)
        .await
        .map_err(|error| error.to_string())?
        > 0
    {
        return Err("任务已存在".into());
    }

    let destination = next_inp_path(workspace_root, &source_task.name).await?;
    tokio::fs::copy(&source_path, &destination)
        .await
        .map_err(|error| format!("Unable to import task file: {error}"))?;
    let inp_pool = connect_inp(&destination).await?;
    let task = metadata_task(&inp_pool, &destination).await?;
    upsert_task_index(config_pool, &task).await?;
    inp_pool.close().await;
    Ok(task)
}

pub async fn list_translation_tasks(
    config_pool: &SqlitePool,
    filters: Option<TranslationTaskFilters>,
) -> Result<Vec<TranslationTaskView>, String> {
    let filters = normalize_task_filters(filters)?;
    let rows = sqlx::query(
        "SELECT * FROM task_index
         WHERE (? IS NULL OR source_language = ?)
           AND (? IS NULL OR target_language = ?)
         ORDER BY updated_at DESC, created_at DESC",
    )
    .bind(filters.source_language.as_deref())
    .bind(filters.source_language.as_deref())
    .bind(filters.target_language.as_deref())
    .bind(filters.target_language.as_deref())
    .fetch_all(config_pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut tasks = rows
        .iter()
        .map(task_from_index_row)
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(tag) = filters.tag {
        tasks.retain(|task| task.tags.iter().any(|item| item.eq_ignore_ascii_case(&tag)));
    }
    Ok(tasks)
}

pub async fn update_translation_task_name(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    input: UpdateTranslationTaskNameInput,
) -> Result<TranslationTaskView, String> {
    let name = validate_task_name(&input.name)?;
    let indexed = get_task_from_index(config_pool, &input.id).await?;
    let inp_path = PathBuf::from(&indexed.inp_path);
    if !inp_path.starts_with(workspace_root) {
        return Err("Task file is outside the configured workspace".into());
    }
    let now = unix_timestamp();
    let inp_pool = connect_inp(&inp_path).await?;
    sqlx::query("UPDATE metadata SET name = ?, updated_at = ? WHERE task_id = ?")
        .bind(name)
        .bind(&now)
        .bind(&input.id)
        .execute(&inp_pool)
        .await
        .map_err(|error| error.to_string())?;
    let task = metadata_task(&inp_pool, &inp_path).await?;
    upsert_task_index(config_pool, &task).await?;
    inp_pool.close().await;
    Ok(task)
}

pub async fn update_translation_task_tags(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    input: UpdateTranslationTaskTagsInput,
) -> Result<TranslationTaskView, String> {
    let indexed = get_task_from_index(config_pool, &input.id).await?;
    let inp_path = PathBuf::from(&indexed.inp_path);
    if !inp_path.starts_with(workspace_root) {
        return Err("Task file is outside the configured workspace".into());
    }

    let tags = normalize_tags(input.tags)?;
    let tags_json = serialize_tags(&tags)?;
    let now = unix_timestamp();
    let inp_pool = connect_inp(&inp_path).await?;
    sqlx::query("UPDATE metadata SET tags_json = ?, updated_at = ? WHERE task_id = ?")
        .bind(tags_json)
        .bind(&now)
        .bind(&input.id)
        .execute(&inp_pool)
        .await
        .map_err(|error| error.to_string())?;
    let task = metadata_task(&inp_pool, &inp_path).await?;
    upsert_task_index(config_pool, &task).await?;
    inp_pool.close().await;
    Ok(task)
}

pub async fn open_translation_task_folder(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let task = get_task_from_index(pool, id).await?;
    open_folder_selecting_file(Path::new(&task.inp_path))
}

pub async fn export_translation_task(
    app: AppHandle,
    config_pool: &SqlitePool,
    input: ExportTranslationTaskInput,
) -> Result<(), String> {
    match input.format {
        TranslationTaskExportFormat::Pdf | TranslationTaskExportFormat::PdfBilingual => {
            return Err("PDF export is not implemented yet".into());
        }
        TranslationTaskExportFormat::Source => {}
    }

    let task = get_task_from_index(config_pool, &input.id).await?;
    let extension = source_extension(&task.source_path)?;
    let default_name = export_file_name(&input.output_name, &task.name, extension);
    let filter_name = extension.to_ascii_uppercase();
    let filter_extensions = [extension];
    let save_path = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(&default_name)
            .add_filter(&filter_name, &filter_extensions)
            .blocking_save_file()
    })
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "Export cancelled".to_string())?;
    let save_path: PathBuf = save_path
        .try_into()
        .map_err(|error| format!("Unable to resolve export path: {error}"))?;
    let inp_pool = connect_inp(Path::new(&task.inp_path)).await?;
    let output = rendered_task_document(&inp_pool, Path::new(&task.source_path)).await?;
    inp_pool.close().await;
    tokio::fs::write(&save_path, output)
        .await
        .map_err(|error| format!("Unable to export task: {error}"))?;
    open_folder_selecting_file(&save_path)?;
    Ok(())
}

async fn rendered_task_document(
    inp_pool: &SqlitePool,
    source_path: &Path,
) -> Result<Vec<u8>, String> {
    let rows = sqlx::query(
        "SELECT sequence, source_text, translated_text, map_json FROM chunks ORDER BY sequence",
    )
    .fetch_all(inp_pool)
    .await
    .map_err(|error| error.to_string())?;
    let chunks = rows
        .iter()
        .map(|row| {
            let translated_text: String = row.get("translated_text");
            let source_text: String = row.get("source_text");
            RenderedChunk {
                sequence: row.get("sequence"),
                source_text: source_text.clone(),
                translated_text: if translated_text.is_empty() {
                    source_text
                } else {
                    translated_text
                },
                map_json: row.get("map_json"),
            }
        })
        .collect::<Vec<_>>();
    document_parsing::render_translated_document(source_path, &chunks)
}

#[cfg(test)]
async fn translated_source_text(inp_pool: &SqlitePool) -> Result<String, String> {
    let rows = sqlx::query("SELECT source_text, translated_text FROM chunks ORDER BY sequence")
        .fetch_all(inp_pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(rows
        .iter()
        .map(|row| {
            let translated: String = row.get("translated_text");
            if translated.is_empty() {
                row.get::<String, _>("source_text")
            } else {
                translated
            }
        })
        .collect::<String>())
}

pub async fn get_translation_task_detail(
    config_pool: &SqlitePool,
    id: &str,
) -> Result<TranslationTaskDetail, String> {
    let task = get_task_from_index(config_pool, id).await?;
    let inp_pool = connect_inp(Path::new(&task.inp_path)).await?;
    let chunk_rows = sqlx::query("SELECT * FROM chunks ORDER BY sequence")
        .fetch_all(&inp_pool)
        .await
        .map_err(|error| error.to_string())?;
    let chunks = chunk_rows
        .iter()
        .map(chunk_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    inp_pool.close().await;
    Ok(TranslationTaskDetail { task, chunks })
}

pub async fn delete_translation_task(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    id: &str,
) -> Result<(), String> {
    let task = get_task_from_index(config_pool, id).await?;
    let inp_path = PathBuf::from(&task.inp_path);
    if !inp_path.starts_with(workspace_root) {
        return Err("Refusing to delete a task outside the workspace".into());
    }
    sqlx::query("DELETE FROM task_index WHERE id = ?")
        .bind(id)
        .execute(config_pool)
        .await
        .map_err(|error| error.to_string())?;
    match tokio::fs::remove_file(&inp_path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub async fn delete_translation_tasks(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    ids: &[String],
) -> Result<(), String> {
    for id in ids {
        let task = get_task_from_index(config_pool, id).await?;
        if task.status == TranslationTaskStatus::Running {
            return Err("请先暂停正在运行的任务".into());
        }
    }
    for id in ids {
        delete_translation_task(config_pool, workspace_root, id).await?;
    }
    Ok(())
}

pub async fn prepare_translation_run(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    id: &str,
    mode: RunMode,
) -> Result<PreparedRun, String> {
    let indexed = get_task_from_index(config_pool, id).await?;
    let inp_path = PathBuf::from(&indexed.inp_path);
    if !inp_path.starts_with(workspace_root) {
        return Err("Task file is outside the configured workspace".into());
    }
    let inp_pool = connect_inp(&inp_path).await?;
    let config = get_translation_config(config_pool).await?;
    let now = unix_timestamp();
    match mode {
        RunMode::Start => {
            if indexed.status == TranslationTaskStatus::Success {
                inp_pool.close().await;
                return Err("Successful tasks must be retranslated explicitly".into());
            }
        }
        RunMode::Resume => {
            sqlx::query(
                "UPDATE chunks
                 SET status = ?, error_message = NULL, confidence = NULL, updated_at = ?
                 WHERE status IN (?, ?)",
            )
            .bind(TranslationChunkStatus::Pending.as_str())
            .bind(&now)
            .bind(TranslationChunkStatus::Interrupted.as_str())
            .bind(TranslationChunkStatus::Failed.as_str())
            .execute(&inp_pool)
            .await
            .map_err(|error| error.to_string())?;
        }
        RunMode::Retranslate => {
            rebuild_chunks_for_retranslate(&inp_pool, &indexed, config.chunk_token_limit, &now)
                .await?;
        }
    }
    sqlx::query(
        "UPDATE metadata
         SET status = ?, token_limit = ?, max_concurrency = ?, max_retries = ?,
             config_snapshot_json = ?, last_error = NULL, rate_limit_status = NULL, updated_at = ?
         WHERE task_id = ?",
    )
    .bind(TranslationTaskStatus::Running.as_str())
    .bind(config.chunk_token_limit)
    .bind(config.max_concurrency)
    .bind(config.max_retries)
    .bind(config_snapshot_json(
        &config,
        &indexed.provider_id,
        &indexed.model_id,
    ))
    .bind(&now)
    .bind(id)
    .execute(&inp_pool)
    .await
    .map_err(|error| error.to_string())?;
    let task = refresh_task_stats(&inp_pool, config_pool, &inp_path, None).await?;
    inp_pool.close().await;
    Ok(PreparedRun {
        task,
        inp_path,
        config,
    })
}

async fn rebuild_chunks_for_retranslate(
    inp_pool: &SqlitePool,
    indexed: &TranslationTaskView,
    token_limit: i64,
    now: &str,
) -> Result<(), String> {
    let source_path = PathBuf::from(&indexed.source_path);
    let chunks = document_parsing::parse_source_file(&indexed.id, &source_path, token_limit)?;
    let mut transaction = inp_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query("DELETE FROM chunks")
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    for chunk in chunks {
        sqlx::query(
            "INSERT INTO chunks (
                id, sequence, map_json, preprocessed_text, source_text,
                after_translate_text, translated_text, status, retry_count,
                input_tokens, output_tokens, cached_tokens, thinking_tokens, total_tokens, updated_at
             ) VALUES (?, ?, ?, ?, ?, '', '', ?, 0, 0, 0, 0, 0, 0, ?)",
        )
        .bind(format!("{}_chunk_{:06}", indexed.id, chunk.sequence))
        .bind(chunk.sequence)
        .bind(chunk.map_json)
        .bind(chunk.preprocessed_text)
        .bind(chunk.source_text)
        .bind(TranslationChunkStatus::Pending.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn run_translation_task(
    app: AppHandle,
    provider_pool: SqlitePool,
    config_pool: SqlitePool,
    client: Client,
    prepared: PreparedRun,
    interrupt: TranslationInterrupt,
) -> Result<(), String> {
    let inp_pool = connect_inp(&prepared.inp_path).await?;
    let task = metadata_task(&inp_pool, &prepared.inp_path).await?;
    let pending_chunks = pending_chunks(&inp_pool).await?;
    if pending_chunks.is_empty() {
        finalize_task(
            &app,
            &inp_pool,
            &config_pool,
            &prepared.inp_path,
            TranslationTaskStatus::Success,
            None,
            None,
        )
        .await?;
        inp_pool.close().await;
        return Ok(());
    }

    let model = db::get_model(&provider_pool, &task.model_id).await?;
    let config = db::runtime_config(&provider_pool, &task.provider_id).await?;
    let adapter = Arc::new(RuntimeAdapter::new(client, config));
    let assistant_prompt = task_assistant_prompt(&inp_pool).await?;
    let assistant_custom_parameters = task_assistant_custom_parameters(&inp_pool).await?;
    let dynamic_rate_limit = prepared.config.rate_limit_strategy == RateLimitStrategy::Dynamic;
    let limiter = Arc::new(AdaptiveLimiter::new(
        prepared.config.max_concurrency as usize,
        dynamic_rate_limit,
    ));
    let quota = Arc::new(HeaderQuotaPolicy::new(dynamic_rate_limit));
    let manual_limiter = if prepared.config.rate_limit_strategy == RateLimitStrategy::Manual {
        Some(Arc::new(ManualRateLimiter::new(
            prepared.config.max_requests_per_minute as u64,
            prepared.config.max_tokens_per_minute as u64,
        )))
    } else {
        None
    };
    let (tx, rx) = mpsc::channel::<ChunkOutcome>(prepared.config.max_concurrency as usize * 2 + 1);
    let writer_pool = inp_pool.clone();
    let writer_config_pool = config_pool.clone();
    let writer_path = prepared.inp_path.clone();
    let writer_app = app.clone();
    let writer_interrupted = interrupt.clone();
    let writer = tokio::spawn(async move {
        writer_loop(
            writer_app,
            writer_pool,
            writer_config_pool,
            writer_path,
            rx,
            writer_interrupted,
        )
        .await
    });
    let max_concurrency = prepared.config.max_concurrency.max(1) as usize;
    let max_retries = prepared.config.max_retries.max(0) as u32;
    let confidence_mode = prepared.config.confidence_mode;
    let target_language = task.target_language.clone();
    let document_format = document_format_from_source_path(&task.source_path)?;
    let content_format = content_format_from_source_path(&task.source_path)?;

    stream::iter(pending_chunks)
        .for_each_concurrent(max_concurrency, |chunk| {
            let adapter = adapter.clone();
            let tx = tx.clone();
            let interrupted = interrupt.clone();
            let limiter = limiter.clone();
            let quota = quota.clone();
            let manual_limiter = manual_limiter.clone();
            let model_request_name = model.request_name.clone();
            let target_language = target_language.clone();
            let assistant_prompt = assistant_prompt.clone();
            let assistant_custom_parameters = assistant_custom_parameters.clone();
            async move {
                if interrupted.is_interrupted() {
                    return;
                }
                let Some(_permit) = limiter.acquire(&interrupted.flag).await else {
                    return;
                };
                if interrupted.is_interrupted() {
                    return;
                }
                let outcome = translate_chunk(
                    adapter,
                    model_request_name,
                    target_language,
                    assistant_prompt,
                    assistant_custom_parameters,
                    document_format,
                    content_format,
                    chunk,
                    max_retries,
                    confidence_mode,
                    quota,
                    limiter.clone(),
                    manual_limiter,
                )
                .await;
                if outcome.interrupt_task {
                    interrupted.interrupt("Rate limit reached; task interrupted");
                    limiter.notify_waiters();
                }
                let _ = tx.send(outcome).await;
            }
        })
        .await;
    drop(tx);
    writer.await.map_err(|error| error.to_string())??;
    inp_pool.close().await;
    Ok(())
}

async fn writer_loop(
    app: AppHandle,
    inp_pool: SqlitePool,
    config_pool: SqlitePool,
    inp_path: PathBuf,
    mut rx: mpsc::Receiver<ChunkOutcome>,
    interrupted: TranslationInterrupt,
) -> Result<(), String> {
    while let Some(outcome) = rx.recv().await {
        apply_chunk_outcome(&inp_pool, outcome).await?;
        let task = refresh_task_stats(&inp_pool, &config_pool, &inp_path, None).await?;
        let _ = app.emit(
            TRANSLATION_PROGRESS_EVENT,
            TranslationProgressPayload { task },
        );
    }
    let stats = aggregate_chunk_stats(&inp_pool).await?;
    let (status, last_error) = if interrupted.is_interrupted() {
        (
            TranslationTaskStatus::Interrupted,
            Some(
                interrupted
                    .reason()
                    .unwrap_or_else(|| "Task interrupted".to_string()),
            ),
        )
    } else if stats.error_rate >= ERROR_RATE_FAILURE_THRESHOLD {
        (
            TranslationTaskStatus::Failed,
            Some(format!(
                "Error rate reached {:.1}%",
                stats.error_rate * 100.0
            )),
        )
    } else {
        (TranslationTaskStatus::Success, None)
    };
    finalize_task(
        &app,
        &inp_pool,
        &config_pool,
        &inp_path,
        status,
        last_error,
        None,
    )
    .await
}

async fn translate_chunk(
    adapter: Arc<RuntimeAdapter>,
    model_request_name: String,
    target_language: String,
    assistant_prompt: Option<String>,
    assistant_custom_parameters: Value,
    document_format: DocumentFormat,
    content_format: ContentFormat,
    chunk: ChunkRecord,
    max_retries: u32,
    confidence_mode: ConfidenceMode,
    quota: Arc<HeaderQuotaPolicy>,
    limiter: Arc<AdaptiveLimiter>,
    manual_limiter: Option<Arc<ManualRateLimiter>>,
) -> ChunkOutcome {
    let mut retry_count = 0_i64;
    let mut last_error = None;
    let mut last_text = None;
    let mut last_stats = TokenStats::default();
    for attempt in 0..=max_retries {
        retry_count = attempt as i64;
        let prompt = build_translation_prompt(TranslationPromptInput {
            target_language: target_language.clone(),
            assistant_system_prompt: assistant_prompt.clone(),
            chunk: TaskChunkInput {
                text: chunk.source_text.clone(),
                document_format,
                content_format,
            },
        });
        let messages = match prompt {
            Ok(TranslationPromptBuildResult::Passthrough { text }) => {
                let translated_text = match restore_chunk_for_map(&chunk.map_json, &text) {
                    Ok(restored) => restored,
                    Err(error) => {
                        return failed_outcome(
                            chunk,
                            TranslationChunkStatus::Failed,
                            retry_count,
                            Some(error),
                            Some(text),
                            last_stats,
                            None,
                            false,
                        );
                    }
                };
                return ChunkOutcome {
                    chunk_id: chunk.id,
                    status: TranslationChunkStatus::Success,
                    interrupt_task: false,
                    after_translate_text: text,
                    translated_text,
                    retry_count,
                    error_message: None,
                    token_stats: TokenStats::default(),
                    rate_limit_status: None,
                    confidence: None,
                };
            }
            Ok(TranslationPromptBuildResult::Request { messages }) => messages,
            Err(error) => {
                return failed_outcome(
                    chunk,
                    TranslationChunkStatus::Failed,
                    retry_count,
                    Some(error),
                    last_text,
                    last_stats,
                    None,
                    false,
                );
            }
        };
        let request = UnifiedChatRequest {
            model: model_request_name.clone(),
            messages,
            tools: Vec::new(),
            tool_choice: UnifiedToolChoice::None,
            thinking: None,
            max_output_tokens: None,
            temperature: Some(0.0),
            stream: false,
            logprobs: confidence_mode.enabled(),
            custom_parameters: assistant_custom_parameters.clone(),
        };
        let estimated_tokens = estimate_tokens(&chunk.source_text) + 256;
        if let Some(manual_limiter) = manual_limiter.as_ref() {
            manual_limiter.before_request(estimated_tokens).await;
        }
        quota.before_request(estimated_tokens).await;
        match send_chat_with_logprobs_fallback(
            &adapter,
            &request,
            estimated_tokens,
            &quota,
            &limiter,
            manual_limiter.as_ref(),
        )
        .await
        {
            Ok(meta) => {
                quota.update(&meta.rate_limits).await;
                limiter
                    .on_result(meta.rate_limits.has_quota_headers(), true, false)
                    .await;
                let mut stats = token_stats_from_response(&meta.response, &chunk.source_text);
                if stats.input_tokens == 0 {
                    stats.input_tokens = estimate_tokens(&chunk.source_text);
                }
                if stats.output_tokens == 0 {
                    stats.output_tokens = estimate_tokens(&meta.response.text);
                }
                stats.thinking_tokens = estimate_tokens(&meta.response.reasoning);
                stats.total_tokens =
                    stats.input_tokens + stats.output_tokens + stats.thinking_tokens;
                last_stats = stats.clone();
                let confidence = if request.logprobs {
                    meta.response
                        .logprob_stats
                        .as_ref()
                        .map(|stats| stats.confidence)
                } else {
                    None
                };
                let text = meta.response.text;
                let rate_status =
                    current_rate_limit_status(&meta.rate_limits, &limiter, &manual_limiter).await;
                last_text = Some(if text.is_empty() {
                    chunk.source_text.clone()
                } else {
                    text.clone()
                });
                if meta.status == 429 || finish_reason_is_truncation(meta.finish_reason.as_deref())
                {
                    last_error = Some(format!(
                        "Interrupted by finish reason: {}",
                        meta.finish_reason
                            .unwrap_or_else(|| "rate-limit".to_string())
                    ));
                    if attempt == max_retries {
                        return failed_outcome(
                            chunk,
                            TranslationChunkStatus::Interrupted,
                            retry_count,
                            last_error,
                            last_text,
                            last_stats,
                            rate_status,
                            false,
                        );
                    }
                    continue;
                }
                if text.trim().is_empty() {
                    last_error = Some("Model returned empty content".to_string());
                    if attempt == max_retries {
                        return failed_outcome(
                            chunk,
                            TranslationChunkStatus::Failed,
                            retry_count,
                            last_error,
                            last_text,
                            last_stats,
                            rate_status,
                            false,
                        );
                    }
                    continue;
                }
                if text.trim() == chunk.source_text.trim() && !chunk.source_text.trim().is_empty() {
                    last_error = Some("Model returned unchanged source text".to_string());
                    if attempt == max_retries {
                        return failed_outcome(
                            chunk,
                            TranslationChunkStatus::Failed,
                            retry_count,
                            last_error,
                            last_text,
                            last_stats,
                            rate_status,
                            false,
                        );
                    }
                    continue;
                }
                let translated_text = match restore_chunk_for_map(&chunk.map_json, &text) {
                    Ok(restored) => restored,
                    Err(error) => {
                        last_error = Some(error);
                        if attempt == max_retries {
                            return failed_outcome(
                                chunk,
                                TranslationChunkStatus::Failed,
                                retry_count,
                                last_error,
                                last_text,
                                last_stats,
                                rate_status,
                                false,
                            );
                        }
                        continue;
                    }
                };
                return ChunkOutcome {
                    chunk_id: chunk.id,
                    status: TranslationChunkStatus::Success,
                    interrupt_task: false,
                    after_translate_text: text,
                    translated_text,
                    retry_count,
                    error_message: None,
                    token_stats: stats,
                    rate_limit_status: rate_status,
                    confidence,
                };
            }
            Err(error) => {
                quota.update(&error.rate_limits).await;
                limiter
                    .on_result(
                        error.rate_limits.has_quota_headers(),
                        false,
                        error.is_rate_limited(),
                    )
                    .await;
                let rate_status =
                    current_rate_limit_status(&error.rate_limits, &limiter, &manual_limiter).await;
                last_error = Some(error.to_string());
                if error.is_rate_limited() {
                    return failed_outcome(
                        chunk,
                        TranslationChunkStatus::Interrupted,
                        retry_count,
                        last_error,
                        last_text,
                        last_stats,
                        rate_status,
                        true,
                    );
                }
                if attempt == max_retries {
                    return failed_outcome(
                        chunk,
                        TranslationChunkStatus::Failed,
                        retry_count,
                        last_error,
                        last_text,
                        last_stats,
                        rate_status,
                        false,
                    );
                }
                tokio::time::sleep(Duration::from_millis(300 * (attempt as u64 + 1))).await;
            }
        }
    }
    failed_outcome(
        chunk,
        TranslationChunkStatus::Failed,
        retry_count,
        last_error,
        last_text,
        last_stats,
        None,
        false,
    )
}

async fn send_chat_with_logprobs_fallback(
    adapter: &RuntimeAdapter,
    request: &UnifiedChatRequest,
    estimated_tokens: u64,
    quota: &HeaderQuotaPolicy,
    limiter: &AdaptiveLimiter,
    manual_limiter: Option<&Arc<ManualRateLimiter>>,
) -> Result<ProviderChatMeta, ProviderChatError> {
    match adapter.send_chat_with_meta(request).await {
        Ok(meta) => Ok(meta),
        Err(error) if request.logprobs && logprobs_parameter_rejected(&error) => {
            quota.update(&error.rate_limits).await;
            limiter
                .on_result(
                    error.rate_limits.has_quota_headers(),
                    false,
                    error.is_rate_limited(),
                )
                .await;
            let mut fallback = request.clone();
            fallback.logprobs = false;
            if let Some(manual_limiter) = manual_limiter {
                manual_limiter.before_request(estimated_tokens).await;
            }
            quota.before_request(estimated_tokens).await;
            adapter.send_chat_with_meta(&fallback).await
        }
        Err(error) => Err(error),
    }
}

fn logprobs_parameter_rejected(error: &ProviderChatError) -> bool {
    let message = error.message.to_ascii_lowercase();
    matches!(error.status, Some(400) | Some(404) | Some(422))
        && (message.contains("logprob") || message.contains("log_probs"))
}

fn failed_outcome(
    chunk: ChunkRecord,
    status: TranslationChunkStatus,
    retry_count: i64,
    error_message: Option<String>,
    translated_text: Option<String>,
    mut token_stats: TokenStats,
    rate_limit_status: Option<String>,
    interrupt_task: bool,
) -> ChunkOutcome {
    if token_stats.input_tokens == 0 {
        token_stats.input_tokens = estimate_tokens(&chunk.source_text);
        token_stats.total_tokens =
            token_stats.input_tokens + token_stats.output_tokens + token_stats.thinking_tokens;
    }
    ChunkOutcome {
        chunk_id: chunk.id,
        status,
        interrupt_task,
        after_translate_text: translated_text
            .clone()
            .unwrap_or_else(|| chunk.source_text.clone()),
        translated_text: translated_text.unwrap_or(chunk.source_text),
        retry_count,
        error_message,
        token_stats,
        rate_limit_status,
        confidence: None,
    }
}

async fn apply_chunk_outcome(pool: &SqlitePool, outcome: ChunkOutcome) -> Result<(), String> {
    sqlx::query(
        "UPDATE chunks
         SET after_translate_text = ?, translated_text = ?, status = ?, retry_count = ?, error_message = ?,
             input_tokens = ?, output_tokens = ?, cached_tokens = ?, thinking_tokens = ?,
             total_tokens = ?, confidence = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(outcome.after_translate_text)
    .bind(outcome.translated_text)
    .bind(outcome.status.as_str())
    .bind(outcome.retry_count)
    .bind(outcome.error_message)
    .bind(outcome.token_stats.input_tokens as i64)
    .bind(outcome.token_stats.output_tokens as i64)
    .bind(outcome.token_stats.cached_tokens as i64)
    .bind(outcome.token_stats.thinking_tokens as i64)
    .bind(outcome.token_stats.total_tokens as i64)
    .bind(outcome.confidence)
    .bind(unix_timestamp())
    .bind(outcome.chunk_id)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    if let Some(status) = outcome.rate_limit_status {
        sqlx::query("UPDATE metadata SET rate_limit_status = ?, updated_at = ?")
            .bind(status)
            .bind(unix_timestamp())
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn finalize_task(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    status: TranslationTaskStatus,
    last_error: Option<String>,
    rate_limit_status: Option<String>,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE metadata
         SET status = ?, last_error = COALESCE(?, last_error),
             rate_limit_status = COALESCE(?, rate_limit_status), updated_at = ?
         WHERE task_id = (SELECT task_id FROM metadata LIMIT 1)",
    )
    .bind(status.as_str())
    .bind(last_error)
    .bind(rate_limit_status)
    .bind(unix_timestamp())
    .execute(inp_pool)
    .await
    .map_err(|error| error.to_string())?;
    let task = refresh_task_stats(inp_pool, config_pool, inp_path, Some(status)).await?;
    let _ = app.emit(
        TRANSLATION_PROGRESS_EVENT,
        TranslationProgressPayload { task },
    );
    Ok(())
}

async fn refresh_task_stats(
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    forced_status: Option<TranslationTaskStatus>,
) -> Result<TranslationTaskView, String> {
    let stats = aggregate_chunk_stats(inp_pool).await?;
    let metadata = metadata_task(inp_pool, inp_path).await?;
    let status = forced_status.unwrap_or(metadata.status);
    let now = unix_timestamp();
    sqlx::query(
        "UPDATE metadata
         SET status = ?, progress = ?, total_chunks = ?, completed_chunks = ?,
             failed_chunks = ?, interrupted_chunks = ?, input_tokens = ?, output_tokens = ?,
             cached_tokens = ?, thinking_tokens = ?, total_tokens = ?, error_rate = ?, updated_at = ?
         WHERE task_id = ?",
    )
    .bind(status.as_str())
    .bind(stats.progress)
    .bind(stats.total_chunks)
    .bind(stats.completed_chunks)
    .bind(stats.failed_chunks)
    .bind(stats.interrupted_chunks)
    .bind(stats.token_stats.input_tokens as i64)
    .bind(stats.token_stats.output_tokens as i64)
    .bind(stats.token_stats.cached_tokens as i64)
    .bind(stats.token_stats.thinking_tokens as i64)
    .bind(stats.token_stats.total_tokens as i64)
    .bind(stats.error_rate)
    .bind(&now)
    .bind(&metadata.id)
    .execute(inp_pool)
    .await
    .map_err(|error| error.to_string())?;
    let refreshed = metadata_task(inp_pool, inp_path).await?;
    upsert_task_index(config_pool, &refreshed).await?;
    Ok(refreshed)
}

#[derive(Debug, Clone)]
struct AggregateStats {
    total_chunks: i64,
    completed_chunks: i64,
    failed_chunks: i64,
    interrupted_chunks: i64,
    progress: f64,
    error_rate: f64,
    token_stats: TokenStats,
}

async fn aggregate_chunk_stats(pool: &SqlitePool) -> Result<AggregateStats, String> {
    let rows = sqlx::query("SELECT * FROM chunks")
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    let total_chunks = rows.len() as i64;
    let mut completed_chunks = 0_i64;
    let mut failed_chunks = 0_i64;
    let mut interrupted_chunks = 0_i64;
    let mut terminal_chunks = 0_i64;
    let mut token_stats = TokenStats::default();
    for row in rows {
        let status = TranslationChunkStatus::parse(row.get::<String, _>("status").as_str())?;
        match status {
            TranslationChunkStatus::Success => {
                completed_chunks += 1;
                terminal_chunks += 1;
            }
            TranslationChunkStatus::Failed => {
                failed_chunks += 1;
                terminal_chunks += 1;
            }
            TranslationChunkStatus::Interrupted => {
                interrupted_chunks += 1;
                terminal_chunks += 1;
            }
            TranslationChunkStatus::Pending => {}
        }
        token_stats.add(&TokenStats {
            input_tokens: row.get::<i64, _>("input_tokens").max(0) as u64,
            output_tokens: row.get::<i64, _>("output_tokens").max(0) as u64,
            cached_tokens: row.get::<i64, _>("cached_tokens").max(0) as u64,
            thinking_tokens: row.get::<i64, _>("thinking_tokens").max(0) as u64,
            total_tokens: row.get::<i64, _>("total_tokens").max(0) as u64,
        });
    }
    let progress = if total_chunks == 0 {
        1.0
    } else {
        terminal_chunks as f64 / total_chunks as f64
    };
    let error_rate = if total_chunks == 0 {
        0.0
    } else {
        (failed_chunks + interrupted_chunks) as f64 / total_chunks as f64
    };
    Ok(AggregateStats {
        total_chunks,
        completed_chunks,
        failed_chunks,
        interrupted_chunks,
        progress,
        error_rate,
        token_stats,
    })
}

async fn upsert_task_index(pool: &SqlitePool, task: &TranslationTaskView) -> Result<(), String> {
    let tags_json = serialize_tags(&task.tags)?;
    sqlx::query(
        "INSERT INTO task_index (
            id, name, inp_path, source_path, source_language, target_language, status, progress,
            provider_id, model_id, model_request_name, assistant_id, tags_json, total_chunks, completed_chunks,
            failed_chunks, interrupted_chunks, input_tokens, output_tokens, cached_tokens,
            thinking_tokens, total_tokens, error_rate, last_error, rate_limit_status, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            inp_path = excluded.inp_path,
            source_path = excluded.source_path,
            source_language = excluded.source_language,
            target_language = excluded.target_language,
            status = excluded.status,
            progress = excluded.progress,
            provider_id = excluded.provider_id,
            model_id = excluded.model_id,
            model_request_name = excluded.model_request_name,
            assistant_id = excluded.assistant_id,
            tags_json = excluded.tags_json,
            total_chunks = excluded.total_chunks,
            completed_chunks = excluded.completed_chunks,
            failed_chunks = excluded.failed_chunks,
            interrupted_chunks = excluded.interrupted_chunks,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            cached_tokens = excluded.cached_tokens,
            thinking_tokens = excluded.thinking_tokens,
            total_tokens = excluded.total_tokens,
            error_rate = excluded.error_rate,
            last_error = excluded.last_error,
            rate_limit_status = excluded.rate_limit_status,
            updated_at = excluded.updated_at",
    )
    .bind(&task.id)
    .bind(&task.name)
    .bind(&task.inp_path)
    .bind(&task.source_path)
    .bind(&task.source_language)
    .bind(&task.target_language)
    .bind(task.status.as_str())
    .bind(task.progress)
    .bind(&task.provider_id)
    .bind(&task.model_id)
    .bind(&task.model_request_name)
    .bind(task.assistant_id.as_deref())
    .bind(tags_json)
    .bind(task.total_chunks)
    .bind(task.completed_chunks)
    .bind(task.failed_chunks)
    .bind(task.interrupted_chunks)
    .bind(task.token_stats.input_tokens as i64)
    .bind(task.token_stats.output_tokens as i64)
    .bind(task.token_stats.cached_tokens as i64)
    .bind(task.token_stats.thinking_tokens as i64)
    .bind(task.token_stats.total_tokens as i64)
    .bind(task.error_rate)
    .bind(task.last_error.as_deref())
    .bind(task.rate_limit_status.as_deref())
    .bind(&task.created_at)
    .bind(&task.updated_at)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn get_task_from_index(pool: &SqlitePool, id: &str) -> Result<TranslationTaskView, String> {
    let row = sqlx::query("SELECT * FROM task_index WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Translation task not found".to_string())?;
    task_from_index_row(&row)
}

fn task_from_index_row(row: &sqlx::sqlite::SqliteRow) -> Result<TranslationTaskView, String> {
    Ok(TranslationTaskView {
        id: row.get("id"),
        name: row.get("name"),
        inp_path: row.get("inp_path"),
        source_path: row.get("source_path"),
        source_language: row.get("source_language"),
        target_language: row.get("target_language"),
        status: TranslationTaskStatus::parse(row.get::<String, _>("status").as_str())?,
        progress: row.get("progress"),
        provider_id: row.get("provider_id"),
        model_id: row.get("model_id"),
        model_request_name: row.get("model_request_name"),
        assistant_id: row.get("assistant_id"),
        tags: parse_tags_json(row.get("tags_json"))?,
        total_chunks: row.get("total_chunks"),
        completed_chunks: row.get("completed_chunks"),
        failed_chunks: row.get("failed_chunks"),
        interrupted_chunks: row.get("interrupted_chunks"),
        token_stats: TokenStats {
            input_tokens: row.get::<i64, _>("input_tokens").max(0) as u64,
            output_tokens: row.get::<i64, _>("output_tokens").max(0) as u64,
            cached_tokens: row.get::<i64, _>("cached_tokens").max(0) as u64,
            thinking_tokens: row.get::<i64, _>("thinking_tokens").max(0) as u64,
            total_tokens: row.get::<i64, _>("total_tokens").max(0) as u64,
        },
        error_rate: row.get("error_rate"),
        last_error: row.get("last_error"),
        rate_limit_status: row.get("rate_limit_status"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn metadata_task(pool: &SqlitePool, inp_path: &Path) -> Result<TranslationTaskView, String> {
    let row = sqlx::query("SELECT * FROM metadata LIMIT 1")
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(TranslationTaskView {
        id: row.get("task_id"),
        name: row.get("name"),
        inp_path: inp_path.to_string_lossy().to_string(),
        source_path: row.get("source_path"),
        source_language: row.get("source_language"),
        target_language: row.get("target_language"),
        status: TranslationTaskStatus::parse(row.get::<String, _>("status").as_str())?,
        progress: row.get("progress"),
        provider_id: row.get("provider_id"),
        model_id: row.get("model_id"),
        model_request_name: row.get("model_request_name"),
        assistant_id: row.get("assistant_id"),
        tags: parse_tags_json(row.get("tags_json"))?,
        total_chunks: row.get("total_chunks"),
        completed_chunks: row.get("completed_chunks"),
        failed_chunks: row.get("failed_chunks"),
        interrupted_chunks: row.get("interrupted_chunks"),
        token_stats: TokenStats {
            input_tokens: row.get::<i64, _>("input_tokens").max(0) as u64,
            output_tokens: row.get::<i64, _>("output_tokens").max(0) as u64,
            cached_tokens: row.get::<i64, _>("cached_tokens").max(0) as u64,
            thinking_tokens: row.get::<i64, _>("thinking_tokens").max(0) as u64,
            total_tokens: row.get::<i64, _>("total_tokens").max(0) as u64,
        },
        error_rate: row.get("error_rate"),
        last_error: row.get("last_error"),
        rate_limit_status: row.get("rate_limit_status"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn chunk_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<TranslationChunkView, String> {
    Ok(TranslationChunkView {
        id: row.get("id"),
        sequence: row.get("sequence"),
        map_json: row.get("map_json"),
        preprocessed_text: row.get("preprocessed_text"),
        source_text: row.get("source_text"),
        after_translate_text: row.get("after_translate_text"),
        translated_text: row.get("translated_text"),
        confidence: row.get("confidence"),
        status: TranslationChunkStatus::parse(row.get::<String, _>("status").as_str())?,
        retry_count: row.get("retry_count"),
        error_message: row.get("error_message"),
        token_stats: TokenStats {
            input_tokens: row.get::<i64, _>("input_tokens").max(0) as u64,
            output_tokens: row.get::<i64, _>("output_tokens").max(0) as u64,
            cached_tokens: row.get::<i64, _>("cached_tokens").max(0) as u64,
            thinking_tokens: row.get::<i64, _>("thinking_tokens").max(0) as u64,
            total_tokens: row.get::<i64, _>("total_tokens").max(0) as u64,
        },
        updated_at: row.get("updated_at"),
    })
}

async fn pending_chunks(pool: &SqlitePool) -> Result<Vec<ChunkRecord>, String> {
    let rows = sqlx::query(
        "SELECT id, sequence, source_text, map_json FROM chunks WHERE status = ? ORDER BY sequence",
    )
    .bind(TranslationChunkStatus::Pending.as_str())
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| ChunkRecord {
            id: row.get("id"),
            source_text: row.get("source_text"),
            map_json: row.get("map_json"),
        })
        .collect())
}

async fn task_assistant_prompt(pool: &SqlitePool) -> Result<Option<String>, String> {
    let prompt: Option<String> =
        sqlx::query_scalar("SELECT assistant_system_prompt FROM metadata LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
    Ok(prompt)
}

async fn task_assistant_custom_parameters(pool: &SqlitePool) -> Result<Value, String> {
    let json: Option<String> =
        sqlx::query_scalar("SELECT assistant_custom_parameters_json FROM metadata LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
    match json {
        Some(value) if !value.trim().is_empty() => {
            let parsed = serde_json::from_str::<Value>(&value)
                .map_err(|error| format!("Assistant custom parameters JSON is invalid: {error}"))?;
            if parsed.is_object() {
                Ok(parsed)
            } else {
                Ok(json!({}))
            }
        }
        _ => Ok(json!({})),
    }
}

fn token_stats_from_response(
    response: &crate::domain::UnifiedChatResponse,
    source_text: &str,
) -> TokenStats {
    match &response.usage {
        Some(usage) => TokenStats {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_tokens: usage.cached_tokens,
            thinking_tokens: estimate_tokens(&response.reasoning),
            total_tokens: usage.input_tokens
                + usage.output_tokens
                + estimate_tokens(&response.reasoning),
        },
        None => TokenStats {
            input_tokens: estimate_tokens(source_text),
            output_tokens: estimate_tokens(&response.text),
            cached_tokens: 0,
            thinking_tokens: estimate_tokens(&response.reasoning),
            total_tokens: estimate_tokens(source_text)
                + estimate_tokens(&response.text)
                + estimate_tokens(&response.reasoning),
        },
    }
}

fn config_snapshot_json(
    config: &TranslationConfigView,
    provider_id: &str,
    model_id: &str,
) -> String {
    json!({
        "chunkTokenLimit": config.chunk_token_limit,
        "maxConcurrency": config.max_concurrency,
        "maxRetries": config.max_retries,
        "rateLimitStrategy": config.rate_limit_strategy,
        "maxRequestsPerMinute": config.max_requests_per_minute,
        "maxTokensPerMinute": config.max_tokens_per_minute,
        "confidenceMode": config.confidence_mode,
        "providerId": provider_id,
        "modelId": model_id
    })
    .to_string()
}

fn normalize_task_filters(
    filters: Option<TranslationTaskFilters>,
) -> Result<TranslationTaskFilters, String> {
    let mut filters = filters.unwrap_or_default();
    filters.tag = normalize_optional_filter(filters.tag);
    filters.source_language = normalize_optional_filter(filters.source_language);
    filters.target_language = normalize_optional_filter(filters.target_language);
    if let Some(value) = filters.tag.as_deref() {
        validate_tag(value)?;
    }
    if let Some(value) = filters.source_language.as_deref() {
        let normalized = normalize_source_language(value)?;
        filters.source_language = Some(normalized);
    }
    if let Some(value) = filters.target_language.as_deref() {
        let normalized = normalize_target_language(value)?;
        filters.target_language = Some(normalized);
    }
    Ok(filters)
}

fn normalize_optional_filter(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for tag in tags {
        let value = tag.trim();
        if value.is_empty() {
            continue;
        }
        validate_tag(value)?;
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(value))
        {
            normalized.push(value.to_string());
        }
    }
    if normalized.len() > MAX_TASK_TAGS {
        return Err(format!("A task can have at most {MAX_TASK_TAGS} tags"));
    }
    Ok(normalized)
}

fn validate_tag(tag: &str) -> Result<(), String> {
    let length = tag.chars().count();
    if length > MAX_TASK_TAG_LENGTH {
        return Err(format!(
            "Task tags must be {MAX_TASK_TAG_LENGTH} characters or shorter"
        ));
    }
    if tag.chars().any(char::is_control) {
        return Err("Task tags cannot contain control characters".into());
    }
    Ok(())
}

fn serialize_tags(tags: &[String]) -> Result<String, String> {
    serde_json::to_string(tags).map_err(|error| error.to_string())
}

fn parse_tags_json(tags_json: String) -> Result<Vec<String>, String> {
    let tags = serde_json::from_str::<Vec<String>>(&tags_json)
        .map_err(|error| format!("Stored task tags are invalid: {error}"))?;
    normalize_tags(tags)
}

fn validate_task_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("任务名称不能为空".into());
    }
    if name.len() > MAX_TASK_NAME_LENGTH {
        return Err("任务名称过长".into());
    }
    if name.chars().any(char::is_control) {
        return Err("任务名称不能包含控制字符".into());
    }
    Ok(name.to_string())
}

fn validate_supported_source_file(path: &Path) -> Result<(), String> {
    if document_parsing::supported_source_file(path) {
        Ok(())
    } else {
        Err("Unsupported source document format".into())
    }
}

fn source_extension(path: &str) -> Result<&'static str, String> {
    document_parsing::source_extension(path)
}

fn export_file_name(output_name: &str, fallback_name: &str, extension: &str) -> String {
    let name = output_name
        .trim()
        .strip_suffix(&format!(".{extension}"))
        .unwrap_or(output_name.trim());
    let base = sanitize_file_stem(if name.is_empty() { fallback_name } else { name });
    format!("{base}.{extension}")
}

fn document_format_from_source_path(path: &str) -> Result<DocumentFormat, String> {
    document_parsing::document_format_from_path(Path::new(path))
}

fn content_format_from_source_path(path: &str) -> Result<ContentFormat, String> {
    document_parsing::content_format_from_path(Path::new(path))
}

#[derive(Debug, Clone)]
#[cfg(test)]
struct RawChunk {
    sequence: i64,
    source_text: String,
}

#[cfg(test)]
fn split_text_into_chunks(
    task_id: &str,
    text: &str,
    token_limit: i64,
    _document_format: DocumentFormat,
    _content_format: ContentFormat,
) -> Vec<RawChunk> {
    let token_limit = token_limit.max(1) as u64;
    let max_chars = (token_limit * 4).max(200) as usize;
    let mut chunks = Vec::new();
    let mut current = String::new();
    for segment in text.split_inclusive('\n') {
        if !current.is_empty() && estimate_tokens(&current) + estimate_tokens(segment) > token_limit
        {
            push_raw_chunk(task_id, &mut chunks, std::mem::take(&mut current));
        }
        if estimate_tokens(segment) > token_limit {
            for part in split_long_segment(segment, max_chars) {
                if current.is_empty() {
                    push_raw_chunk(task_id, &mut chunks, part);
                } else {
                    push_raw_chunk(task_id, &mut chunks, std::mem::take(&mut current));
                    push_raw_chunk(task_id, &mut chunks, part);
                }
            }
        } else {
            current.push_str(segment);
        }
    }
    if !current.is_empty() || chunks.is_empty() {
        push_raw_chunk(task_id, &mut chunks, current);
    }
    chunks
}

#[cfg(test)]
fn split_long_segment(segment: &str, max_chars: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for char_value in segment.chars() {
        current.push(char_value);
        if current.len() >= max_chars {
            parts.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

#[cfg(test)]
fn push_raw_chunk(task_id: &str, chunks: &mut Vec<RawChunk>, source_text: String) {
    let sequence = chunks.len() as i64;
    let _ = task_id;
    chunks.push(RawChunk {
        sequence,
        source_text,
    });
}

fn estimate_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let mut ascii = 0_u64;
    let mut non_ascii = 0_u64;
    for character in text.chars() {
        if character.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    ascii.div_ceil(4) + non_ascii.div_ceil(2)
}

async fn next_inp_path(workspace_root: &Path, display_name: &str) -> Result<PathBuf, String> {
    let tasks_dir = workspace_root.join(TASKS_DIR);
    tokio::fs::create_dir_all(&tasks_dir)
        .await
        .map_err(|error| error.to_string())?;
    let base = sanitize_file_stem(display_name);
    for index in 0..10_000 {
        let filename = if index == 0 {
            format!("{base}.inp")
        } else {
            format!("{base}-{index:02}.inp")
        };
        let candidate = tasks_dir.join(filename);
        if tokio::fs::try_exists(&candidate)
            .await
            .map_err(|error| error.to_string())?
        {
            continue;
        }
        return Ok(candidate);
    }
    Err("Unable to allocate a unique INP file name".into())
}

fn display_name_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("task")
        .to_string()
}

fn sanitize_file_stem(value: &str) -> String {
    let sanitized = value
        .chars()
        .filter(|character| {
            !matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'
            ) && !character.is_control()
        })
        .collect::<String>()
        .trim_matches([' ', '.'])
        .to_string();
    if sanitized.is_empty() {
        "task".into()
    } else {
        sanitized
    }
}

fn unix_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn rate_limit_status(telemetry: &RateLimitTelemetry, window: usize) -> Option<String> {
    if telemetry.has_quota_headers() {
        Some(format!(
            "{}: requests {}/{}, tokens {}/{}, window {}",
            telemetry.source.as_deref().unwrap_or("headers"),
            telemetry
                .request_remaining
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            telemetry
                .request_limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            telemetry
                .token_remaining
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            telemetry
                .token_limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            window
        ))
    } else {
        Some(format!("aimd: window {window}"))
    }
}

fn open_folder_selecting_file(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn()
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let folder = path.parent().unwrap_or(path);
        Command::new("xdg-open")
            .arg(folder)
            .spawn()
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
}

async fn current_rate_limit_status(
    telemetry: &RateLimitTelemetry,
    limiter: &AdaptiveLimiter,
    manual_limiter: &Option<Arc<ManualRateLimiter>>,
) -> Option<String> {
    match manual_limiter {
        Some(manual_limiter) => Some(manual_limiter.status().await),
        None => rate_limit_status(telemetry, limiter.window().await),
    }
}

struct HeaderQuotaPolicy {
    enabled: bool,
    state: Mutex<Option<RateLimitTelemetry>>,
}

impl HeaderQuotaPolicy {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: Mutex::new(None),
        }
    }

    async fn before_request(&self, estimated_tokens: u64) {
        if !self.enabled {
            return;
        }
        let sleep_ms = {
            let state = self.state.lock().await;
            state.as_ref().and_then(|telemetry| {
                let mut delay = telemetry.retry_after_ms;
                if telemetry
                    .request_remaining
                    .is_some_and(|remaining| remaining <= 1)
                {
                    delay = delay.max(telemetry.request_reset_ms);
                }
                if telemetry
                    .token_remaining
                    .is_some_and(|remaining| remaining <= estimated_tokens + 128)
                {
                    delay = delay.max(telemetry.token_reset_ms);
                }
                delay
            })
        };
        if let Some(delay) = sleep_ms.filter(|value| *value > 0) {
            tokio::time::sleep(Duration::from_millis(delay.min(60_000))).await;
        }
    }

    async fn update(&self, telemetry: &RateLimitTelemetry) {
        if !self.enabled {
            return;
        }
        if telemetry.has_quota_headers() || telemetry.retry_after_ms.is_some() {
            *self.state.lock().await = Some(telemetry.clone());
        }
    }
}

struct ManualRateLimiter {
    max_requests: u64,
    max_tokens: u64,
    state: Mutex<ManualRateLimiterState>,
}

struct ManualRateLimiterState {
    window_started: Instant,
    requests: u64,
    tokens: u64,
}

impl ManualRateLimiter {
    fn new(max_requests: u64, max_tokens: u64) -> Self {
        Self {
            max_requests: max_requests.max(1),
            max_tokens: max_tokens.max(1),
            state: Mutex::new(ManualRateLimiterState {
                window_started: Instant::now(),
                requests: 0,
                tokens: 0,
            }),
        }
    }

    async fn before_request(&self, estimated_tokens: u64) {
        let estimated_tokens = estimated_tokens.min(self.max_tokens);
        loop {
            let delay = {
                let mut state = self.state.lock().await;
                if state.window_started.elapsed() >= Duration::from_secs(60) {
                    state.window_started = Instant::now();
                    state.requests = 0;
                    state.tokens = 0;
                }
                if state.requests < self.max_requests
                    && state.tokens + estimated_tokens <= self.max_tokens
                {
                    state.requests += 1;
                    state.tokens += estimated_tokens;
                    None
                } else {
                    Some(Duration::from_secs(60).saturating_sub(state.window_started.elapsed()))
                }
            };
            match delay {
                Some(delay) => tokio::time::sleep(delay.max(Duration::from_millis(25))).await,
                None => return,
            }
        }
    }

    async fn status(&self) -> String {
        let state = self.state.lock().await;
        format!(
            "manual: requests {}/{}, tokens {}/{} per minute",
            state.requests, self.max_requests, state.tokens, self.max_tokens
        )
    }
}

struct AdaptiveLimiter {
    max: usize,
    adaptive: bool,
    in_flight: AtomicUsize,
    state: Mutex<AdaptiveLimiterState>,
    notify: Notify,
}

struct AdaptiveLimiterState {
    window: usize,
    success_streak: usize,
    header_mode: bool,
}

struct AdaptivePermit {
    limiter: Arc<AdaptiveLimiter>,
}

impl Drop for AdaptivePermit {
    fn drop(&mut self) {
        self.limiter.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.limiter.notify.notify_waiters();
    }
}

impl AdaptiveLimiter {
    fn new(max: usize, adaptive: bool) -> Self {
        let max = max.max(1);
        Self {
            max,
            adaptive,
            in_flight: AtomicUsize::new(0),
            state: Mutex::new(AdaptiveLimiterState {
                window: if adaptive { 1 } else { max },
                success_streak: 0,
                header_mode: false,
            }),
            notify: Notify::new(),
        }
    }

    async fn acquire(self: &Arc<Self>, interrupted: &AtomicBool) -> Option<AdaptivePermit> {
        loop {
            if interrupted.load(Ordering::SeqCst) {
                return None;
            }
            let window = self.window().await;
            if self.in_flight.load(Ordering::SeqCst) < window {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                return Some(AdaptivePermit {
                    limiter: self.clone(),
                });
            }
            self.notify.notified().await;
        }
    }

    async fn on_result(&self, has_headers: bool, success: bool, rate_limited: bool) {
        if !self.adaptive {
            return;
        }
        let mut state = self.state.lock().await;
        if has_headers {
            state.header_mode = true;
            state.window = self.max;
            state.success_streak = 0;
        } else if rate_limited {
            state.header_mode = false;
            state.window = (state.window / 2).max(1);
            state.success_streak = 0;
        } else if success && !state.header_mode {
            state.success_streak += 1;
            if state.success_streak >= state.window {
                state.window = (state.window + 1).min(self.max);
                state.success_streak = 0;
            }
        }
        self.notify.notify_waiters();
    }

    async fn window(&self) -> usize {
        self.state.lock().await.window
    }

    fn notify_waiters(&self) {
        self.notify.notify_waiters();
    }
}

pub async fn mark_task_failed_after_runtime_error(
    config_pool: &SqlitePool,
    inp_path: &Path,
    error: String,
) -> Result<TranslationTaskView, String> {
    let inp_pool = connect_inp(inp_path).await?;
    sqlx::query("UPDATE metadata SET status = ?, last_error = ?, updated_at = ?")
        .bind(TranslationTaskStatus::Failed.as_str())
        .bind(error)
        .bind(unix_timestamp())
        .execute(&inp_pool)
        .await
        .map_err(|error| error.to_string())?;
    let task = refresh_task_stats(
        &inp_pool,
        config_pool,
        inp_path,
        Some(TranslationTaskStatus::Failed),
    )
    .await?;
    inp_pool.close().await;
    Ok(task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("insitu-test-{label}-{}", db::new_id("workspace")))
    }

    async fn write_test_inp(path: &Path, task_id: &str, name: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| error.to_string())?;
        }

        let pool = connect_inp(path).await?;
        let now = unix_timestamp();
        let tags_json = serialize_tags(&["review".to_string(), "client".to_string()])?;
        let source_path = path.with_extension("txt").to_string_lossy().to_string();
        sqlx::query(
            "INSERT INTO metadata (
                task_id, schema_version, name, source_path, source_language, target_language,
                status, provider_id, model_id, model_request_name, tags_json, token_limit,
                max_concurrency, max_retries, total_chunks, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(task_id)
        .bind(INP_SCHEMA_VERSION)
        .bind(name)
        .bind(source_path)
        .bind("en")
        .bind("zh-Hans")
        .bind(TranslationTaskStatus::Pending.as_str())
        .bind("provider-test")
        .bind("model-test")
        .bind("test-model")
        .bind(tags_json)
        .bind(400)
        .bind(2)
        .bind(1)
        .bind(2)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;

        for (sequence, (source_text, translated_text)) in
            [("Hello ", "你好"), ("world", "")].into_iter().enumerate()
        {
            sqlx::query(
                "INSERT INTO chunks (
                    id, sequence, source_text, translated_text, status, updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(format!("{task_id}-chunk-{sequence}"))
            .bind(sequence as i64)
            .bind(source_text)
            .bind(translated_text)
            .bind(TranslationChunkStatus::Pending.as_str())
            .bind(&now)
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
        }

        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn inp_migration_adds_confidence_column() {
        let root = temp_root("confidence-migration");
        let inp_path = root.join("legacy.inp");
        if let Some(parent) = inp_path.parent() {
            tokio::fs::create_dir_all(parent).await.expect("mkdir");
        }
        let pool = connect_sqlite(&inp_path, 1).await.expect("connect");
        sqlx::query(
            r#"CREATE TABLE chunks (
                id TEXT PRIMARY KEY NOT NULL,
                sequence INTEGER NOT NULL,
                source_text TEXT NOT NULL,
                translated_text TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                error_message TEXT,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cached_tokens INTEGER NOT NULL DEFAULT 0,
                thinking_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .expect("legacy chunks");
        pool.close().await;

        let migrated = connect_inp(&inp_path).await.expect("migrate");
        let columns = sqlx::query("PRAGMA table_info(chunks)")
            .fetch_all(&migrated)
            .await
            .expect("columns");
        assert!(columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "confidence"));
        migrated.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn apply_chunk_outcome_writes_confidence() {
        let root = temp_root("confidence-write");
        let inp_path = root.join("task.inp");
        write_test_inp(&inp_path, "task-confidence", "Confidence")
            .await
            .expect("write inp");
        let pool = connect_inp(&inp_path).await.expect("open inp");
        apply_chunk_outcome(
            &pool,
            ChunkOutcome {
                chunk_id: "task-confidence-chunk-0".into(),
                status: TranslationChunkStatus::Success,
                interrupt_task: false,
                after_translate_text: "你好".into(),
                translated_text: "你好".into(),
                retry_count: 0,
                error_message: None,
                token_stats: TokenStats::default(),
                rate_limit_status: None,
                confidence: Some(0.875),
            },
        )
        .await
        .expect("apply outcome");
        let confidence: Option<f64> =
            sqlx::query_scalar("SELECT confidence FROM chunks WHERE id = ?")
                .bind("task-confidence-chunk-0")
                .fetch_one(&pool)
                .await
                .expect("confidence");
        assert_eq!(confidence, Some(0.875));
        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn detects_logprobs_parameter_rejection() {
        let error = ProviderChatError {
            status: Some(400),
            message: "Unrecognized request argument supplied: logprobs".into(),
            rate_limits: RateLimitTelemetry::default(),
        };
        assert!(logprobs_parameter_rejected(&error));
    }

    #[test]
    fn sanitizes_inp_file_stems() {
        assert_eq!(sanitize_file_stem("bad:name?.md"), "badname.md");
        assert_eq!(sanitize_file_stem("..."), "task");
        assert_eq!(sanitize_file_stem("  book  "), "book");
    }

    #[test]
    fn chunks_preserve_order_and_cover_text() {
        let text = "line 1\nline 2\nline 3\n";
        let chunks = split_text_into_chunks(
            "task",
            text,
            2,
            DocumentFormat::Txt,
            ContentFormat::PlainText,
        );
        let joined = chunks
            .iter()
            .map(|chunk| chunk.source_text.as_str())
            .collect::<String>();
        assert_eq!(joined, text);
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.sequence, index as i64);
        }
    }

    #[test]
    fn normalizes_task_tags() {
        let tags = normalize_tags(vec![
            " client ".into(),
            "Client".into(),
            "".into(),
            "Review".into(),
        ])
        .expect("valid tags");
        assert_eq!(tags, vec!["client".to_string(), "Review".to_string()]);
        assert!(normalize_tags(vec!["x".repeat(MAX_TASK_TAG_LENGTH + 1)]).is_err());
        assert!(normalize_tags(
            (0..=MAX_TASK_TAGS)
                .map(|index| format!("tag-{index}"))
                .collect()
        )
        .is_err());
    }

    #[test]
    fn normalizes_task_filters() {
        let filters = normalize_task_filters(Some(TranslationTaskFilters {
            tag: Some(" client ".into()),
            source_language: Some(" auto ".into()),
            target_language: Some(" Polish ".into()),
        }))
        .expect("valid filters");
        assert_eq!(filters.tag.as_deref(), Some("client"));
        assert_eq!(filters.source_language.as_deref(), Some("auto"));
        assert_eq!(filters.target_language.as_deref(), Some("pl"));
    }

    #[tokio::test]
    async fn validates_inp_files_and_rejects_damaged_shapes() {
        let root = temp_root("inp-validation");
        let valid_path = root.join("valid.inp");
        write_test_inp(&valid_path, "task-valid", "Valid Task")
            .await
            .expect("write valid inp");

        let task = validate_inp_file(&valid_path)
            .await
            .expect("valid inp is accepted");
        assert_eq!(task.id, "task-valid");
        assert_eq!(task.name, "Valid Task");
        assert_eq!(task.tags, vec!["review".to_string(), "client".to_string()]);

        let missing_chunks_path = root.join("missing-chunks.inp");
        write_test_inp(
            &missing_chunks_path,
            "task-missing-chunks",
            "Missing Chunks",
        )
        .await
        .expect("write inp before damage");
        let pool = connect_sqlite(&missing_chunks_path, 1)
            .await
            .expect("open damaged inp");
        sqlx::query("DROP TABLE chunks")
            .execute(&pool)
            .await
            .expect("drop chunks");
        pool.close().await;
        assert_eq!(
            validate_inp_file(&missing_chunks_path).await.unwrap_err(),
            INP_FILE_DAMAGED
        );

        let missing_field_path = root.join("missing-field.inp");
        let pool = connect_sqlite(&missing_field_path, 1)
            .await
            .expect("open incomplete inp");
        sqlx::query("CREATE TABLE metadata (task_id TEXT PRIMARY KEY NOT NULL)")
            .execute(&pool)
            .await
            .expect("create incomplete metadata");
        pool.close().await;
        assert_eq!(
            validate_inp_file(&missing_field_path).await.unwrap_err(),
            INP_FILE_DAMAGED
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn imports_rejects_duplicates_and_renames_metadata_and_index() {
        let root = temp_root("inp-import");
        let external_root = temp_root("inp-external");
        let external_path = external_root.join("incoming.inp");
        write_test_inp(&external_path, "task-import", "Incoming Task")
            .await
            .expect("write external inp");

        let pool = connect_config_db(&root).await.expect("connect config");
        let imported = import_translation_task(
            &pool,
            &root,
            ImportTranslationTaskInput {
                file_path: external_path.to_string_lossy().to_string(),
            },
        )
        .await
        .expect("import task");
        assert_eq!(imported.id, "task-import");
        assert_eq!(imported.name, "Incoming Task");
        assert!(PathBuf::from(&imported.inp_path).starts_with(root.join(TASKS_DIR)));
        assert_ne!(PathBuf::from(&imported.inp_path), external_path);

        let duplicate = import_translation_task(
            &pool,
            &root,
            ImportTranslationTaskInput {
                file_path: external_path.to_string_lossy().to_string(),
            },
        )
        .await
        .expect_err("duplicate task id is rejected");
        assert_eq!(duplicate, "任务已存在");

        let renamed = update_translation_task_name(
            &pool,
            &root,
            UpdateTranslationTaskNameInput {
                id: imported.id.clone(),
                name: "Renamed Task".into(),
            },
        )
        .await
        .expect("rename task");
        assert_eq!(renamed.name, "Renamed Task");

        let indexed = get_task_from_index(&pool, &imported.id)
            .await
            .expect("read index");
        assert_eq!(indexed.name, "Renamed Task");
        let inp_pool = connect_inp(Path::new(&renamed.inp_path))
            .await
            .expect("open renamed inp");
        let metadata_name: String = sqlx::query_scalar("SELECT name FROM metadata LIMIT 1")
            .fetch_one(&inp_pool)
            .await
            .expect("read metadata name");
        assert_eq!(metadata_name, "Renamed Task");
        inp_pool.close().await;
        pool.close().await;

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(external_root);
    }

    #[tokio::test]
    async fn translated_source_text_uses_translations_and_falls_back_to_source() {
        let root = temp_root("source-export");
        let inp_path = root.join("source.inp");
        write_test_inp(&inp_path, "task-export", "Source Export")
            .await
            .expect("write inp");
        let pool = connect_inp(&inp_path).await.expect("open inp");
        assert_eq!(
            translated_source_text(&pool).await.expect("render source"),
            "你好world"
        );
        assert_eq!(source_extension("chapter.md").expect("md"), "md");
        assert_eq!(
            export_file_name(" custom.txt ", "fallback", "txt"),
            "custom.txt"
        );
        assert_eq!(export_file_name("", "fallback", "txt"), "fallback.txt");
        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn default_config_is_seeded() {
        let root = std::env::temp_dir().join(format!("insitu-test-{}", db::new_id("workspace")));
        let pool = connect_config_db(&root).await.expect("connect config");
        let config = get_translation_config(&pool).await.expect("config");
        assert_eq!(config.source_language, "auto");
        assert_eq!(config.custom_source_language, "");
        assert_eq!(config.target_language, DEFAULT_TARGET_LANGUAGE);
        assert_eq!(config.custom_target_language, "");
        assert_eq!(config.chunk_token_limit, DEFAULT_CHUNK_TOKEN_LIMIT);
        assert_eq!(config.max_concurrency, DEFAULT_MAX_CONCURRENCY);
        assert_eq!(config.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(config.rate_limit_strategy, RateLimitStrategy::Dynamic);
        assert_eq!(
            config.max_requests_per_minute,
            DEFAULT_MAX_REQUESTS_PER_MINUTE
        );
        assert_eq!(config.max_tokens_per_minute, DEFAULT_MAX_TOKENS_PER_MINUTE);
        assert!(!config.use_glossary);
        assert_eq!(config.glossary_mode, GlossaryMode::Auto);
        assert_eq!(config.glossary_id, None);
        assert_eq!(config.confidence_mode, ConfidenceMode::Off);
        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn migrates_legacy_defaults_only_once() {
        let root = std::env::temp_dir().join(format!("insitu-test-{}", db::new_id("workspace")));
        let pool = connect_config_db(&root).await.expect("connect config");
        sqlx::query(
            "UPDATE translation_config
             SET chunk_token_limit = 1200, max_concurrency = 4, max_retries = 2,
                 config_json = ''",
        )
        .execute(&pool)
        .await
        .expect("set legacy defaults");
        sqlx::query(
            "DELETE FROM translation_config_migrations WHERE id = 'translation-defaults-4000-5-5'",
        )
        .execute(&pool)
        .await
        .expect("clear migration");
        pool.close().await;

        let migrated_pool = connect_config_db(&root).await.expect("reconnect config");
        let migrated = get_translation_config(&migrated_pool)
            .await
            .expect("migrated config");
        assert_eq!(migrated.chunk_token_limit, DEFAULT_CHUNK_TOKEN_LIMIT);
        assert_eq!(migrated.max_concurrency, DEFAULT_MAX_CONCURRENCY);
        assert_eq!(migrated.max_retries, DEFAULT_MAX_RETRIES);

        update_translation_config(
            &migrated_pool,
            UpdateTranslationConfigInput {
                source_language: "German".into(),
                custom_source_language: "ignored".into(),
                target_language: "Polish".into(),
                custom_target_language: "ignored".into(),
                provider_id: "provider-test".into(),
                model_id: "model-test".into(),
                assistant_id: "assistant-test".into(),
                chunk_token_limit: 1200,
                max_concurrency: 4,
                max_retries: 2,
                rate_limit_strategy: RateLimitStrategy::Manual,
                max_requests_per_minute: 90,
                max_tokens_per_minute: 90_000,
                use_glossary: true,
                glossary_mode: GlossaryMode::Auto,
                glossary_id: None,
                confidence_mode: ConfidenceMode::ConfidenceIndex,
            },
        )
        .await
        .expect("set explicit user values");
        let persisted_json: String =
            sqlx::query_scalar("SELECT config_json FROM translation_config WHERE id = 1")
                .fetch_one(&migrated_pool)
                .await
                .expect("persisted config json");
        let persisted: TranslationConfigView =
            serde_json::from_str(&persisted_json).expect("deserialize persisted config");
        assert_eq!(persisted.source_language, "de");
        assert_eq!(persisted.custom_source_language, "");
        assert_eq!(persisted.target_language, "pl");
        assert_eq!(persisted.custom_target_language, "");
        assert_eq!(persisted.provider_id, "provider-test");
        assert_eq!(persisted.model_id, "model-test");
        assert_eq!(persisted.assistant_id, "assistant-test");
        assert_eq!(persisted.chunk_token_limit, 1200);
        assert_eq!(persisted.rate_limit_strategy, RateLimitStrategy::Manual);
        assert!(persisted.use_glossary);
        assert_eq!(persisted.glossary_mode, GlossaryMode::Auto);
        assert_eq!(persisted.confidence_mode, ConfidenceMode::ConfidenceIndex);
        migrated_pool.close().await;

        let final_pool = connect_config_db(&root).await.expect("final reconnect");
        let final_config = get_translation_config(&final_pool)
            .await
            .expect("final config");
        assert_eq!(final_config.source_language, "de");
        assert_eq!(final_config.custom_source_language, "");
        assert_eq!(final_config.target_language, "pl");
        assert_eq!(final_config.custom_target_language, "");
        assert_eq!(final_config.provider_id, "provider-test");
        assert_eq!(final_config.model_id, "model-test");
        assert_eq!(final_config.assistant_id, "assistant-test");
        assert_eq!(final_config.chunk_token_limit, 1200);
        assert_eq!(final_config.max_concurrency, 4);
        assert_eq!(final_config.max_retries, 2);
        assert_eq!(final_config.rate_limit_strategy, RateLimitStrategy::Manual);
        assert_eq!(final_config.max_requests_per_minute, 90);
        assert!(final_config.use_glossary);
        assert_eq!(final_config.glossary_mode, GlossaryMode::Auto);
        assert_eq!(
            final_config.confidence_mode,
            ConfidenceMode::ConfidenceIndex
        );
        final_pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn legacy_other_language_config_falls_back_to_defaults() {
        let root = std::env::temp_dir().join(format!("insitu-test-{}", db::new_id("workspace")));
        let pool = connect_config_db(&root).await.expect("connect config");
        let legacy = TranslationConfigView {
            source_language: "__other__".into(),
            custom_source_language: "German".into(),
            target_language: "__other__".into(),
            custom_target_language: "Polish".into(),
            ..TranslationConfigView::default()
        };
        let legacy_json = serde_json::to_string(&legacy).expect("legacy json");
        sqlx::query("UPDATE translation_config SET config_json = ? WHERE id = 1")
            .bind(legacy_json)
            .execute(&pool)
            .await
            .expect("write legacy config");

        let config = get_translation_config(&pool).await.expect("read config");
        assert_eq!(config.source_language, DEFAULT_SOURCE_LANGUAGE);
        assert_eq!(config.custom_source_language, "");
        assert_eq!(config.target_language, DEFAULT_TARGET_LANGUAGE);
        assert_eq!(config.custom_target_language, "");
        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }
}
