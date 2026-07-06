use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, Sqlite, SqlitePool};
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;

use crate::db as app_db;
use crate::document_parsing;
use crate::document_parsing::types::{ParserProgress, ParserProgressStage, RenderedChunk};
use crate::languages::{
    normalize_source_language, normalize_target_language, DEFAULT_SOURCE_LANGUAGE,
    DEFAULT_TARGET_LANGUAGE,
};
use crate::pdf_parsing::{self, PdfAsset, PdfParsingMode};
use crate::task_prompt::{ContentFormat, DocumentFormat};

use super::context::{
    display_name_from_path, global_background_from_texts, next_inp_path, sanitize_file_stem,
    unix_timestamp,
};
use super::types::{
    ChunkOutcome, ChunkRecord, ProgressDetail, ProgressStep, TaskGlossaryConfig,
    TranslationTaskCreationProgressPayload, TranslationTaskCreationStage,
    TranslationTaskCreationStatus,
};
use super::{
    ContextHandlingMode, CreateTranslationTaskInput, ExportTranslationTaskInput, GlossaryMode,
    ImportTranslationTaskInput, RateLimitStrategy, TokenStats, TranslationChunkStatus,
    TranslationChunkView, TranslationConfigView, TranslationProgressPayload,
    TranslationTaskActiveRetry, TranslationTaskDetail, TranslationTaskExportFormat,
    TranslationTaskFilters, TranslationTaskStatus, TranslationTaskView,
    UpdateTranslationConfigInput, UpdateTranslationTaskInfoInput, UpdateTranslationTaskNameInput,
    UpdateTranslationTaskTagsInput, CONFIG_DB_FILE, DEFAULT_CHUNK_TOKEN_LIMIT,
    DEFAULT_MAX_CONCURRENCY, DEFAULT_MAX_REQUESTS_PER_MINUTE, DEFAULT_MAX_RETRIES,
    DEFAULT_MAX_TOKENS_PER_MINUTE, INP_FILE_DAMAGED, INP_SCHEMA_VERSION, MAX_TASK_NAME_LENGTH,
    MAX_TASK_TAGS, MAX_TASK_TAG_LENGTH, SOURCE_FILE_UNAVAILABLE, TASKS_DIR,
    TRANSLATION_PROGRESS_EVENT, TRANSLATION_TASK_CREATION_PROGRESS_EVENT,
};

#[derive(Debug, Clone)]
pub(super) struct ParsedTaskSource {
    pub(super) chunks: Vec<document_parsing::types::ParsedChunk>,
    pub(super) assets: Vec<PdfAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DbActiveRetry {
    chunk_id: String,
    current: u32,
    max: u32,
    message: String,
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

pub(super) async fn connect_sqlite(
    path: &Path,
    max_connections: u32,
) -> Result<SqlitePool, String> {
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
            active_retry_json TEXT,
            progress_detail_json TEXT,
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
    add_column_if_missing(pool, "task_index", "active_retry_json", "TEXT").await?;
    add_column_if_missing(pool, "task_index", "progress_detail_json", "TEXT").await?;
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

pub(super) async fn connect_inp(path: &Path) -> Result<SqlitePool, String> {
    let pool = connect_sqlite(path, 1).await?;
    migrate_inp_db(&pool).await?;
    backfill_source_file_if_available(&pool).await?;
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

pub(super) async fn validate_inp_file(path: &Path) -> Result<TranslationTaskView, String> {
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
            "global_background",
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
    if schema_version >= 4 {
        require_columns(
            pool,
            "assets",
            &[
                "relative_path",
                "media_type",
                "bytes",
                "source",
                "created_at",
            ],
        )
        .await?;
    }
    if schema_version >= 5 {
        require_columns(
            pool,
            "source_file",
            &["id", "file_name", "bytes", "created_at"],
        )
        .await?;
        let invalid_source_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM source_file WHERE id != 1")
                .fetch_one(pool)
                .await
                .map_err(|_| INP_FILE_DAMAGED.to_string())?;
        if invalid_source_rows > 0 {
            return Err(INP_FILE_DAMAGED.into());
        }
    }
    if schema_version >= 6 {
        require_columns(
            pool,
            "metadata",
            &["use_glossary", "glossary_mode", "glossary_id"],
        )
        .await?;
    }
    if schema_version >= 7 {
        require_columns(pool, "metadata", &["global_background"]).await?;
    }
    if schema_version >= 8 {
        require_columns(pool, "metadata", &["progress_detail_json"]).await?;
        let progress_detail_json = row
            .try_get::<Option<String>, _>("progress_detail_json")
            .unwrap_or(None);
        parse_progress_detail_json(progress_detail_json)
            .map_err(|_| INP_FILE_DAMAGED.to_string())?;
    }
    if schema_version >= 9 {
        require_columns(pool, "metadata", &["active_retry_json"]).await?;
        let active_retry_json = row
            .try_get::<Option<String>, _>("active_retry_json")
            .unwrap_or(None);
        parse_active_retry_json(active_retry_json).map_err(|_| INP_FILE_DAMAGED.to_string())?;
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
    let rows = sqlx::query("SELECT id, inp_path FROM task_index WHERE status IN (?, ?)")
        .bind(TranslationTaskStatus::Running.as_str())
        .bind(TranslationTaskStatus::InterruptedPending.as_str())
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
            use_glossary INTEGER NOT NULL DEFAULT 0,
            glossary_mode TEXT NOT NULL DEFAULT 'auto',
            glossary_id TEXT,
            tags_json TEXT NOT NULL DEFAULT '[]',
            token_limit INTEGER NOT NULL,
            max_concurrency INTEGER NOT NULL,
            max_retries INTEGER NOT NULL,
            config_snapshot_json TEXT NOT NULL DEFAULT '{}',
            global_background TEXT,
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
            active_retry_json TEXT,
            progress_detail_json TEXT,
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
        r#"CREATE TABLE IF NOT EXISTS assets (
            relative_path TEXT PRIMARY KEY NOT NULL,
            media_type TEXT NOT NULL,
            bytes BLOB NOT NULL,
            source TEXT NOT NULL,
            created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS source_file (
            id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
            file_name TEXT NOT NULL,
            bytes BLOB NOT NULL,
            created_at TEXT NOT NULL
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
    add_column_if_missing(
        pool,
        "metadata",
        "use_glossary",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "metadata",
        "glossary_mode",
        "TEXT NOT NULL DEFAULT 'auto'",
    )
    .await?;
    add_column_if_missing(pool, "metadata", "glossary_id", "TEXT").await?;
    add_column_if_missing(pool, "metadata", "global_background", "TEXT").await?;
    add_column_if_missing(pool, "metadata", "active_retry_json", "TEXT").await?;
    add_column_if_missing(pool, "metadata", "progress_detail_json", "TEXT").await?;
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

pub(super) struct MaterializedSourceFile {
    root_dir: PathBuf,
    path: PathBuf,
}

impl MaterializedSourceFile {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MaterializedSourceFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root_dir);
    }
}

pub(super) enum ResolvedSourceFile {
    Embedded(MaterializedSourceFile),
    Original(PathBuf),
}

impl ResolvedSourceFile {
    pub(super) fn path(&self) -> &Path {
        match self {
            Self::Embedded(file) => file.path(),
            Self::Original(path) => path,
        }
    }
}

async fn backfill_source_file_if_available(pool: &SqlitePool) -> Result<(), String> {
    let existing_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_file")
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    if existing_count > 0 {
        return Ok(());
    }

    let Some(row) = sqlx::query("SELECT source_path, created_at FROM metadata LIMIT 1")
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let source_path: String = row.get("source_path");
    let bytes = match tokio::fs::read(&source_path).await {
        Ok(bytes) => bytes,
        Err(_) => return Ok(()),
    };
    let file_name = source_file_name_from_path(Path::new(&source_path));
    let created_at: String = row.get("created_at");
    sqlx::query(
        "INSERT OR REPLACE INTO source_file (id, file_name, bytes, created_at)
         VALUES (1, ?, ?, ?)",
    )
    .bind(file_name)
    .bind(bytes)
    .bind(created_at)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn resolve_source_file(
    inp_pool: &SqlitePool,
    fallback_source_path: &Path,
) -> Result<ResolvedSourceFile, String> {
    if let Some(row) = sqlx::query("SELECT file_name, bytes FROM source_file WHERE id = 1")
        .fetch_optional(inp_pool)
        .await
        .map_err(|error| error.to_string())?
    {
        let file_name: String = row.get("file_name");
        let bytes: Vec<u8> = row.get("bytes");
        return materialize_source_bytes(&file_name, &bytes, fallback_source_path)
            .await
            .map(ResolvedSourceFile::Embedded);
    }

    match tokio::fs::metadata(fallback_source_path).await {
        Ok(metadata) if metadata.is_file() => Ok(ResolvedSourceFile::Original(
            fallback_source_path.to_path_buf(),
        )),
        _ => Err(SOURCE_FILE_UNAVAILABLE.into()),
    }
}

async fn materialize_source_bytes(
    file_name: &str,
    bytes: &[u8],
    fallback_source_path: &Path,
) -> Result<MaterializedSourceFile, String> {
    let root_dir = std::env::temp_dir().join(format!("insitu-source-{}", app_db::new_id("src")));
    tokio::fs::create_dir_all(&root_dir)
        .await
        .map_err(|error| format!("Unable to create temporary source directory: {error}"))?;
    let path = root_dir.join(materialized_source_file_name(
        file_name,
        fallback_source_path,
    ));
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|error| format!("Unable to write temporary source file: {error}"))?;
    Ok(MaterializedSourceFile { root_dir, path })
}

async fn insert_source_file(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    file_name: &str,
    bytes: &[u8],
    created_at: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT OR REPLACE INTO source_file (id, file_name, bytes, created_at)
         VALUES (1, ?, ?, ?)",
    )
    .bind(file_name)
    .bind(bytes)
    .bind(created_at)
    .execute(&mut **transaction)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn source_file_name_from_path(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("source")
        .to_string()
}

fn materialized_source_file_name(file_name: &str, fallback_source_path: &Path) -> String {
    let base = Path::new(file_name)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(sanitize_file_stem)
        .unwrap_or_else(|| sanitize_file_stem(&source_file_name_from_path(fallback_source_path)));
    if Path::new(&base).extension().is_some() {
        return base;
    }
    match fallback_source_path
        .extension()
        .and_then(|value| value.to_str())
    {
        Some(extension) if !extension.trim().is_empty() => {
            format!("{base}.{}", sanitize_file_stem(extension))
        }
        _ => base,
    }
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
    if config.context_handling_mode == ContextHandlingMode::Off && config.use_global_background {
        config.context_handling_mode = ContextHandlingMode::GlobalBackground;
    }
    config.use_global_background = false;
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

pub(super) fn effective_translation_concurrency(config: &TranslationConfigView) -> usize {
    if config.context_handling_mode == ContextHandlingMode::SlidingWindowTarget {
        1
    } else {
        config.max_concurrency.max(1) as usize
    }
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
        context_handling_mode: if input.context_handling_mode == ContextHandlingMode::Off
            && input.use_global_background
        {
            ContextHandlingMode::GlobalBackground
        } else {
            input.context_handling_mode
        },
        use_global_background: false,
        use_glossary: input.use_glossary,
        glossary_mode: input.glossary_mode,
        glossary_id: input
            .glossary_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        thinking_effort: input.thinking_effort,
        use_web_search: input.use_web_search,
        use_custom_parameters: input.use_custom_parameters,
        confidence_mode: input.confidence_mode,
        pdf_parsing_mode: input.pdf_parsing_mode,
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

pub(super) async fn parse_source_file_for_task(
    provider_pool: &SqlitePool,
    client: &Client,
    task_id: &str,
    source_path: &Path,
    token_limit: i64,
    pdf_parsing_mode: PdfParsingMode,
) -> Result<ParsedTaskSource, String> {
    if source_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("pdf"))
        == Some(true)
    {
        let parsed_pdf = pdf_parsing::parse_pdf_for_task(
            provider_pool,
            client,
            task_id,
            source_path,
            pdf_parsing_mode,
        )
        .await?;
        let chunks = document_parsing::parse_pdf_markdown_text(&parsed_pdf.markdown, token_limit)?;
        return Ok(ParsedTaskSource {
            chunks,
            assets: parsed_pdf.assets,
        });
    }

    Ok(ParsedTaskSource {
        chunks: document_parsing::parse_source_file(task_id, source_path, token_limit)?,
        assets: Vec::new(),
    })
}

pub(super) async fn parse_source_file_for_task_with_progress<'progress>(
    provider_pool: &SqlitePool,
    client: &Client,
    task_id: &str,
    source_path: &Path,
    token_limit: i64,
    pdf_parsing_mode: PdfParsingMode,
    progress: Option<&'progress mut (dyn FnMut(ParserProgress) + Send + 'progress)>,
) -> Result<ParsedTaskSource, String> {
    if source_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("pdf"))
        == Some(true)
    {
        let parsed_pdf = pdf_parsing::parse_pdf_for_task(
            provider_pool,
            client,
            task_id,
            source_path,
            pdf_parsing_mode,
        )
        .await?;
        let chunks = document_parsing::parse_pdf_markdown_text_with_progress(
            &parsed_pdf.markdown,
            token_limit,
            progress,
        )?;
        return Ok(ParsedTaskSource {
            chunks,
            assets: parsed_pdf.assets,
        });
    }

    Ok(ParsedTaskSource {
        chunks: document_parsing::parse_source_file_with_progress(
            task_id,
            source_path,
            token_limit,
            progress,
        )?,
        assets: Vec::new(),
    })
}

pub(super) async fn insert_assets(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    assets: &[PdfAsset],
    created_at: &str,
) -> Result<(), String> {
    for asset in assets {
        validate_asset_relative_path(&asset.relative_path)?;
        sqlx::query(
            "INSERT OR REPLACE INTO assets (relative_path, media_type, bytes, source, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&asset.relative_path)
        .bind(&asset.media_type)
        .bind(&asset.bytes)
        .bind(&asset.source)
        .bind(created_at)
        .execute(&mut **transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub async fn create_translation_task(
    provider_pool: &SqlitePool,
    client: &Client,
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
    let source_bytes = tokio::fs::read(&source_path)
        .await
        .map_err(|error| format!("Unable to read source document: {error}"))?;
    let source_file_name = source_file_name_from_path(&source_path);
    let materialized_source =
        materialize_source_bytes(&source_file_name, &source_bytes, &source_path).await?;
    let model = app_db::get_model(provider_pool, &input.model_id).await?;
    if model.provider_id != input.provider_id {
        return Err("Selected model does not belong to the selected provider".into());
    }
    let config = get_translation_config(config_pool).await?;
    let (assistant_prompt, assistant_custom_parameters) = match input
        .assistant_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        Some(id) => {
            let assistant = app_db::get_assistant(provider_pool, id).await?;
            let custom_parameters = if config.use_custom_parameters {
                assistant.custom_parameters
            } else {
                json!({})
            };
            (Some(assistant.system_prompt), custom_parameters)
        }
        None => (None, json!({})),
    };
    let task_id = app_db::new_id("task");
    let display_name = display_name_from_path(&source_path);
    let inp_path = next_inp_path(workspace_root, &display_name).await?;
    let parsed_source = parse_source_file_for_task(
        provider_pool,
        client,
        &task_id,
        materialized_source.path(),
        config.chunk_token_limit,
        config.pdf_parsing_mode,
    )
    .await?;
    let global_background = if config.context_handling_mode == ContextHandlingMode::GlobalBackground
    {
        Some(global_background_from_texts(
            parsed_source
                .chunks
                .iter()
                .map(|chunk| chunk.source_text.as_str()),
        ))
    } else {
        None
    };
    let created_at = unix_timestamp();
    let inp_pool = connect_inp(&inp_path).await?;
    let config_snapshot = config_snapshot_json(&config, &input.provider_id, &model.id);
    let progress_detail = progress_detail_for_config(
        parsed_source.chunks.len() as u64,
        0,
        &TaskGlossaryConfig {
            use_glossary: config.use_glossary,
            glossary_mode: config.glossary_mode,
            glossary_id: config.glossary_id.clone(),
        },
    );
    let progress_detail_json = serialize_progress_detail(Some(&progress_detail))?;
    let mut transaction = inp_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO metadata (
            task_id, schema_version, name, source_path, source_language, target_language, status,
            progress, provider_id, model_id, model_request_name, assistant_id, assistant_system_prompt,
            assistant_custom_parameters_json, use_glossary, glossary_mode, glossary_id, tags_json,
            token_limit, max_concurrency, max_retries, config_snapshot_json, global_background,
            total_chunks, progress_detail_json,
            created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(config.use_glossary)
    .bind(config.glossary_mode.as_str())
    .bind(config.glossary_id.as_deref())
    .bind(tags_json)
    .bind(config.chunk_token_limit)
    .bind(config.max_concurrency)
    .bind(config.max_retries)
    .bind(config_snapshot)
    .bind(global_background.as_deref())
    .bind(parsed_source.chunks.len() as i64)
    .bind(progress_detail_json)
    .bind(&created_at)
    .bind(&created_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;

    insert_source_file(
        &mut transaction,
        &source_file_name,
        &source_bytes,
        &created_at,
    )
    .await?;
    insert_assets(&mut transaction, &parsed_source.assets, &created_at).await?;

    for chunk in parsed_source.chunks {
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

async fn cleanup_partial_task_creation(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    task_id: Option<&str>,
    inp_path: Option<&Path>,
) {
    if let Some(id) = task_id {
        let _ = sqlx::query("DELETE FROM task_index WHERE id = ?")
            .bind(id)
            .execute(config_pool)
            .await;
    }
    if let Some(path) = inp_path.filter(|path| path.starts_with(workspace_root)) {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

fn emit_creation_progress(
    app: &AppHandle,
    client_task_id: &str,
    file_path: &str,
    stage: TranslationTaskCreationStage,
    step: ProgressStep,
    status: TranslationTaskCreationStatus,
    task: Option<TranslationTaskView>,
    error: Option<String>,
) {
    let _ = app.emit(
        TRANSLATION_TASK_CREATION_PROGRESS_EVENT,
        TranslationTaskCreationProgressPayload {
            client_task_id: client_task_id.to_string(),
            file_path: file_path.to_string(),
            stage,
            step,
            status,
            task,
            error,
        },
    );
}

pub async fn create_translation_task_with_progress(
    app: AppHandle,
    provider_pool: SqlitePool,
    client: Client,
    config_pool: SqlitePool,
    workspace_root: PathBuf,
    input: CreateTranslationTaskInput,
    client_task_id: String,
    cancel: Arc<AtomicBool>,
) -> Result<Option<TranslationTaskView>, String> {
    let source_file_path = input.file_path.clone();
    let mut task_id_for_cleanup: Option<String> = None;
    let mut inp_path_for_cleanup: Option<PathBuf> = None;
    let mut current_stage = TranslationTaskCreationStage::Ast;

    macro_rules! fail_creation {
        ($error:expr) => {{
            let error = $error;
            cleanup_partial_task_creation(
                &config_pool,
                &workspace_root,
                task_id_for_cleanup.as_deref(),
                inp_path_for_cleanup.as_deref(),
            )
            .await;
            emit_creation_progress(
                &app,
                &client_task_id,
                &source_file_path,
                current_stage,
                ProgressStep::failed(0, 0, "创建任务失败"),
                TranslationTaskCreationStatus::Failed,
                None,
                Some(error.clone()),
            );
            return Err(error);
        }};
    }

    macro_rules! try_creation {
        ($expr:expr) => {
            match $expr {
                Ok(value) => value,
                Err(error) => fail_creation!(error),
            }
        };
    }

    macro_rules! cancel_creation {
        () => {
            if cancel.load(Ordering::SeqCst) {
                cleanup_partial_task_creation(
                    &config_pool,
                    &workspace_root,
                    task_id_for_cleanup.as_deref(),
                    inp_path_for_cleanup.as_deref(),
                )
                .await;
                emit_creation_progress(
                    &app,
                    &client_task_id,
                    &source_file_path,
                    current_stage,
                    ProgressStep::failed(0, 0, "已取消"),
                    TranslationTaskCreationStatus::Cancelled,
                    None,
                    None,
                );
                return Ok(None);
            }
        };
    }

    emit_creation_progress(
        &app,
        &client_task_id,
        &source_file_path,
        TranslationTaskCreationStage::Ast,
        ProgressStep::pending(0, 0, "等待预处理"),
        TranslationTaskCreationStatus::Queued,
        None,
        None,
    );
    cancel_creation!();

    let source_language = try_creation!(normalize_source_language(&input.source_language));
    let target_language = try_creation!(normalize_target_language(&input.target_language));
    let tags = try_creation!(normalize_tags(input.tags));
    let tags_json = try_creation!(serialize_tags(&tags));
    let source_path = PathBuf::from(input.file_path.trim());
    try_creation!(validate_supported_source_file(&source_path));
    cancel_creation!();

    let source_bytes = try_creation!(tokio::fs::read(&source_path)
        .await
        .map_err(|error| format!("Unable to read source document: {error}")));
    let source_file_name = source_file_name_from_path(&source_path);
    let materialized_source = try_creation!(
        materialize_source_bytes(&source_file_name, &source_bytes, &source_path).await
    );
    cancel_creation!();

    let model = try_creation!(app_db::get_model(&provider_pool, &input.model_id).await);
    if model.provider_id != input.provider_id {
        fail_creation!("Selected model does not belong to the selected provider".to_string());
    }
    let config = try_creation!(get_translation_config(&config_pool).await);
    let (assistant_prompt, assistant_custom_parameters) = match input
        .assistant_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        Some(id) => {
            let assistant = try_creation!(app_db::get_assistant(&provider_pool, id).await);
            let custom_parameters = if config.use_custom_parameters {
                assistant.custom_parameters
            } else {
                json!({})
            };
            (Some(assistant.system_prompt), custom_parameters)
        }
        None => (None, json!({})),
    };
    let task_id = app_db::new_id("task");
    task_id_for_cleanup = Some(task_id.clone());
    let display_name = display_name_from_path(&source_path);
    let inp_path = try_creation!(next_inp_path(&workspace_root, &display_name).await);
    inp_path_for_cleanup = Some(inp_path.clone());

    emit_creation_progress(
        &app,
        &client_task_id,
        &source_file_path,
        TranslationTaskCreationStage::Ast,
        ProgressStep::running(1, 2, "AST 解析结构 (1/2)"),
        TranslationTaskCreationStatus::Running,
        None,
        None,
    );
    let mut ast_finished = false;
    let mut emit_parse_progress = |progress: ParserProgress| {
        if progress.stage == ParserProgressStage::Chunking {
            if !ast_finished {
                ast_finished = true;
                current_stage = TranslationTaskCreationStage::Chunking;
                emit_creation_progress(
                    &app,
                    &client_task_id,
                    &source_file_path,
                    TranslationTaskCreationStage::Ast,
                    ProgressStep::success(2, 2, "AST 已完成"),
                    TranslationTaskCreationStatus::Running,
                    None,
                    None,
                );
            }
            emit_creation_progress(
                &app,
                &client_task_id,
                &source_file_path,
                TranslationTaskCreationStage::Chunking,
                ProgressStep::running(progress.current, progress.total, progress.label),
                TranslationTaskCreationStatus::Running,
                None,
                None,
            );
        }
    };
    let parsed_source = try_creation!(
        parse_source_file_for_task_with_progress(
            &provider_pool,
            &client,
            &task_id,
            materialized_source.path(),
            config.chunk_token_limit,
            config.pdf_parsing_mode,
            Some(&mut emit_parse_progress),
        )
        .await
    );
    drop(emit_parse_progress);
    cancel_creation!();
    if !ast_finished {
        emit_creation_progress(
            &app,
            &client_task_id,
            &source_file_path,
            TranslationTaskCreationStage::Ast,
            ProgressStep::success(2, 2, "AST 已完成"),
            TranslationTaskCreationStatus::Running,
            None,
            None,
        );
    }

    current_stage = TranslationTaskCreationStage::Chunking;
    let total_chunks = parsed_source.chunks.len() as u64;
    emit_creation_progress(
        &app,
        &client_task_id,
        &source_file_path,
        TranslationTaskCreationStage::Chunking,
        ProgressStep::success(
            total_chunks,
            total_chunks,
            count_label("分块", total_chunks, total_chunks),
        ),
        TranslationTaskCreationStatus::Running,
        None,
        None,
    );

    let global_background = if config.context_handling_mode == ContextHandlingMode::GlobalBackground
    {
        Some(global_background_from_texts(
            parsed_source
                .chunks
                .iter()
                .map(|chunk| chunk.source_text.as_str()),
        ))
    } else {
        None
    };
    cancel_creation!();

    let created_at = unix_timestamp();
    let inp_pool = try_creation!(connect_inp(&inp_path).await);
    let config_snapshot = config_snapshot_json(&config, &input.provider_id, &model.id);
    let progress_detail = progress_detail_for_config(
        total_chunks,
        0,
        &TaskGlossaryConfig {
            use_glossary: config.use_glossary,
            glossary_mode: config.glossary_mode,
            glossary_id: config.glossary_id.clone(),
        },
    );
    let progress_detail_json = try_creation!(serialize_progress_detail(Some(&progress_detail)));
    let mut transaction = try_creation!(inp_pool.begin().await.map_err(|error| error.to_string()));
    try_creation!(
        sqlx::query(
            "INSERT INTO metadata (
                task_id, schema_version, name, source_path, source_language, target_language, status,
                progress, provider_id, model_id, model_request_name, assistant_id, assistant_system_prompt,
                assistant_custom_parameters_json, use_glossary, glossary_mode, glossary_id, tags_json,
                token_limit, max_concurrency, max_retries, config_snapshot_json, global_background,
                total_chunks, progress_detail_json,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(config.use_glossary)
        .bind(config.glossary_mode.as_str())
        .bind(config.glossary_id.as_deref())
        .bind(tags_json)
        .bind(config.chunk_token_limit)
        .bind(config.max_concurrency)
        .bind(config.max_retries)
        .bind(config_snapshot)
        .bind(global_background.as_deref())
        .bind(total_chunks as i64)
        .bind(progress_detail_json)
        .bind(&created_at)
        .bind(&created_at)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())
    );

    try_creation!(
        insert_source_file(
            &mut transaction,
            &source_file_name,
            &source_bytes,
            &created_at,
        )
        .await
    );
    try_creation!(insert_assets(&mut transaction, &parsed_source.assets, &created_at).await);

    for chunk in parsed_source.chunks {
        try_creation!(
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
            .map_err(|error| error.to_string())
        );
    }
    try_creation!(transaction
        .commit()
        .await
        .map_err(|error| error.to_string()));
    cancel_creation!();

    let view = try_creation!(
        refresh_task_stats_without_index(&inp_pool, &config_pool, &inp_path, None).await
    );
    inp_pool.close().await;

    Ok(Some(view))
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

pub async fn publish_staged_translation_task(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    task_id: &str,
    inp_path: &Path,
) -> Result<TranslationTaskView, String> {
    if !inp_path.starts_with(workspace_root) {
        return Err("Refusing to publish a task outside the workspace".into());
    }
    let inp_pool = connect_inp(inp_path).await?;
    let metadata = metadata_task(&inp_pool, inp_path).await?;
    if metadata.id != task_id {
        inp_pool.close().await;
        return Err("Staged task identity does not match its task file".into());
    }
    let task = refresh_task_stats(&inp_pool, config_pool, inp_path, None).await?;
    inp_pool.close().await;
    Ok(task)
}

pub async fn discard_staged_translation_task(
    workspace_root: &Path,
    inp_path: &Path,
) -> Result<(), String> {
    if !inp_path.starts_with(workspace_root) {
        return Err("Refusing to discard a task outside the workspace".into());
    }
    match tokio::fs::remove_file(inp_path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
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

pub async fn update_translation_task_info(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    input: UpdateTranslationTaskInfoInput,
) -> Result<TranslationTaskView, String> {
    let name = validate_task_name(&input.name)?;
    let tags = normalize_tags(input.tags)?;
    let tags_json = serialize_tags(&tags)?;
    let indexed = get_task_from_index(config_pool, &input.id).await?;
    let inp_path = PathBuf::from(&indexed.inp_path);
    if !inp_path.starts_with(workspace_root) {
        return Err("Task file is outside the configured workspace".into());
    }

    let now = unix_timestamp();
    let inp_pool = connect_inp(&inp_path).await?;
    sqlx::query("UPDATE metadata SET name = ?, tags_json = ?, updated_at = ? WHERE task_id = ?")
        .bind(name)
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
    tokio::fs::write(&save_path, output)
        .await
        .map_err(|error| format!("Unable to export task: {error}"))?;
    if source_is_pdf(Path::new(&task.source_path)) {
        release_assets_for_export(&inp_pool, &save_path).await?;
    }
    inp_pool.close().await;
    open_folder_selecting_file(&save_path)?;
    Ok(())
}

pub(super) async fn rendered_task_document(
    inp_pool: &SqlitePool,
    source_path: &Path,
) -> Result<Vec<u8>, String> {
    let rows = sqlx::query(
        "SELECT sequence, source_text, after_translate_text, translated_text, map_json FROM chunks ORDER BY sequence",
    )
    .fetch_all(inp_pool)
    .await
    .map_err(|error| error.to_string())?;
    let chunks = rows
        .iter()
        .map(|row| {
            let translated_text: String = row.get("translated_text");
            let source_text: String = row.get("source_text");
            let after_translate_text: String = row.get("after_translate_text");
            RenderedChunk {
                sequence: row.get("sequence"),
                source_text: source_text.clone(),
                after_translate_text: if after_translate_text.is_empty() {
                    source_text.clone()
                } else {
                    after_translate_text
                },
                translated_text: if translated_text.is_empty() {
                    source_text
                } else {
                    translated_text
                },
                map_json: row.get("map_json"),
            }
        })
        .collect::<Vec<_>>();
    let resolved_source = resolve_source_file(inp_pool, source_path).await?;
    document_parsing::render_translated_document(resolved_source.path(), &chunks)
}

pub(super) async fn release_assets_for_export(
    inp_pool: &SqlitePool,
    save_path: &Path,
) -> Result<(), String> {
    let rows = sqlx::query("SELECT relative_path, bytes FROM assets ORDER BY relative_path")
        .fetch_all(inp_pool)
        .await
        .map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Ok(());
    }
    let base_dir = save_path.parent().unwrap_or_else(|| Path::new("."));
    for row in rows {
        let relative_path: String = row.get("relative_path");
        validate_asset_relative_path(&relative_path)?;
        let target_path = safe_export_asset_path(base_dir, &relative_path)?;
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("Unable to create export asset directory: {error}"))?;
        }
        let bytes: Vec<u8> = row.get("bytes");
        tokio::fs::write(&target_path, bytes)
            .await
            .map_err(|error| format!("Unable to export PDF asset: {error}"))?;
    }
    Ok(())
}

fn safe_export_asset_path(base_dir: &Path, relative_path: &str) -> Result<PathBuf, String> {
    validate_asset_relative_path(relative_path)?;
    let mut target = base_dir.to_path_buf();
    let normalized = relative_path.replace('\\', "/");
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => target.push(part),
            Component::CurDir => {}
            _ => return Err("PDF asset path must be relative".into()),
        }
    }
    Ok(target)
}

fn validate_asset_relative_path(relative_path: &str) -> Result<(), String> {
    let normalized = relative_path.replace('\\', "/");
    if normalized.trim().is_empty() || normalized.starts_with('/') {
        return Err("PDF asset path must be a non-empty relative path".into());
    }
    let mut has_component = false;
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(_) => has_component = true,
            Component::CurDir => {}
            _ => return Err("PDF asset path cannot be absolute or contain parent segments".into()),
        }
    }
    if !has_component {
        return Err("PDF asset path must contain a file name".into());
    }
    Ok(())
}

fn source_is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("pdf"))
}

#[cfg(test)]
pub(super) async fn translated_source_text(inp_pool: &SqlitePool) -> Result<String, String> {
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

pub async fn mark_task_interrupted_pending(
    app: &AppHandle,
    config_pool: &SqlitePool,
    workspace_root: &Path,
    id: &str,
) -> Result<TranslationTaskView, String> {
    let task = get_task_from_index(config_pool, id).await?;
    let inp_path = PathBuf::from(&task.inp_path);
    if !inp_path.starts_with(workspace_root) {
        return Err("Task file is outside the configured workspace".into());
    }
    let inp_pool = connect_inp(&inp_path).await?;
    sqlx::query(
        "UPDATE metadata
         SET status = ?, updated_at = ?
         WHERE task_id = ?",
    )
    .bind(TranslationTaskStatus::InterruptedPending.as_str())
    .bind(unix_timestamp())
    .bind(id)
    .execute(&inp_pool)
    .await
    .map_err(|error| error.to_string())?;
    let stats = aggregate_chunk_stats(&inp_pool).await?;
    let metadata = metadata_task(&inp_pool, &inp_path).await?;
    let glossary_config = task_glossary_config(&inp_pool).await?;
    let detail = progress_detail_for_translation_stats(
        metadata.progress_detail,
        stats.total_chunks.max(0) as u64,
        stats.completed_chunks.max(0) as u64,
        TranslationTaskStatus::InterruptedPending,
        &glossary_config,
    );
    set_progress_detail(&inp_pool, &detail).await?;
    let task = refresh_task_stats(
        &inp_pool,
        config_pool,
        &inp_path,
        Some(TranslationTaskStatus::InterruptedPending),
    )
    .await?;
    let _ = app.emit(
        TRANSLATION_PROGRESS_EVENT,
        TranslationProgressPayload { task: task.clone() },
    );
    inp_pool.close().await;
    Ok(task)
}

pub async fn delete_translation_task(
    config_pool: &SqlitePool,
    workspace_root: &Path,
    id: &str,
) -> Result<(), String> {
    let task = get_task_from_index(config_pool, id).await?;
    let inp_path = PathBuf::from(&task.inp_path);
    if matches!(
        task.status,
        TranslationTaskStatus::Running | TranslationTaskStatus::InterruptedPending
    ) {
        return Err("Pause the running task before deleting it".into());
    }
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
        if matches!(
            task.status,
            TranslationTaskStatus::Running | TranslationTaskStatus::InterruptedPending
        ) {
            return Err("请先暂停正在运行的任务".into());
        }
    }
    for id in ids {
        delete_translation_task(config_pool, workspace_root, id).await?;
    }
    Ok(())
}

pub(super) async fn apply_chunk_outcome(
    pool: &SqlitePool,
    outcome: ChunkOutcome,
) -> Result<(), String> {
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

pub(super) async fn set_active_retry_and_emit(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    chunk_id: &str,
    current: u32,
    max: u32,
    message: String,
) -> Result<(), String> {
    let retry = DbActiveRetry {
        chunk_id: chunk_id.to_string(),
        current,
        max,
        message,
    };
    let active_retry_json = serialize_active_retry(Some(&retry))?;
    sqlx::query(
        "UPDATE metadata
         SET active_retry_json = ?, updated_at = ?
         WHERE task_id = (SELECT task_id FROM metadata LIMIT 1)",
    )
    .bind(active_retry_json)
    .bind(unix_timestamp())
    .execute(inp_pool)
    .await
    .map_err(|error| error.to_string())?;
    let task = refresh_task_stats(inp_pool, config_pool, inp_path, None).await?;
    let _ = app.emit(
        TRANSLATION_PROGRESS_EVENT,
        TranslationProgressPayload { task },
    );
    Ok(())
}

pub(super) async fn clear_active_retry_for_chunk(
    inp_pool: &SqlitePool,
    chunk_id: &str,
) -> Result<(), String> {
    let row = sqlx::query("SELECT active_retry_json FROM metadata LIMIT 1")
        .fetch_one(inp_pool)
        .await
        .map_err(|error| error.to_string())?;
    let retry = parse_db_active_retry_json(
        row.try_get::<Option<String>, _>("active_retry_json")
            .unwrap_or(None),
    )?;
    if retry
        .as_ref()
        .is_some_and(|value| value.chunk_id == chunk_id)
    {
        sqlx::query(
            "UPDATE metadata
             SET active_retry_json = NULL, updated_at = ?
             WHERE task_id = (SELECT task_id FROM metadata LIMIT 1)",
        )
        .bind(unix_timestamp())
        .execute(inp_pool)
        .await
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) async fn finalize_task(
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
             rate_limit_status = COALESCE(?, rate_limit_status), active_retry_json = NULL,
             updated_at = ?
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

async fn refresh_task_stats_internal(
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    forced_status: Option<TranslationTaskStatus>,
    update_index: bool,
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
    if update_index {
        upsert_task_index(config_pool, &refreshed).await?;
    }
    Ok(refreshed)
}

pub(super) async fn refresh_task_stats(
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    forced_status: Option<TranslationTaskStatus>,
) -> Result<TranslationTaskView, String> {
    refresh_task_stats_internal(inp_pool, config_pool, inp_path, forced_status, true).await
}

async fn refresh_task_stats_without_index(
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    forced_status: Option<TranslationTaskStatus>,
) -> Result<TranslationTaskView, String> {
    refresh_task_stats_internal(inp_pool, config_pool, inp_path, forced_status, false).await
}

#[derive(Debug, Clone)]
pub(super) struct AggregateStats {
    pub(super) total_chunks: i64,
    pub(super) completed_chunks: i64,
    pub(super) failed_chunks: i64,
    pub(super) interrupted_chunks: i64,
    pub(super) progress: f64,
    pub(super) error_rate: f64,
    pub(super) token_stats: TokenStats,
}

pub(super) async fn aggregate_chunk_stats(pool: &SqlitePool) -> Result<AggregateStats, String> {
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

pub(super) async fn upsert_task_index(
    pool: &SqlitePool,
    task: &TranslationTaskView,
) -> Result<(), String> {
    let tags_json = serialize_tags(&task.tags)?;
    let active_retry_json = serialize_task_active_retry(task.active_retry.as_ref())?;
    let progress_detail_json = serialize_progress_detail(task.progress_detail.as_ref())?;
    sqlx::query(
        "INSERT INTO task_index (
            id, name, inp_path, source_path, source_language, target_language, status, progress,
            provider_id, model_id, model_request_name, assistant_id, tags_json, total_chunks, completed_chunks,
            failed_chunks, interrupted_chunks, input_tokens, output_tokens, cached_tokens,
            thinking_tokens, total_tokens, error_rate, last_error, rate_limit_status, active_retry_json,
            progress_detail_json, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            active_retry_json = excluded.active_retry_json,
            progress_detail_json = excluded.progress_detail_json,
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
    .bind(active_retry_json)
    .bind(progress_detail_json)
    .bind(&task.created_at)
    .bind(&task.updated_at)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn get_task_from_index(
    pool: &SqlitePool,
    id: &str,
) -> Result<TranslationTaskView, String> {
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
        active_retry: row_active_retry(row)?,
        progress_detail: row_progress_detail(row)?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub(super) async fn metadata_task(
    pool: &SqlitePool,
    inp_path: &Path,
) -> Result<TranslationTaskView, String> {
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
        active_retry: row_active_retry(&row)?,
        progress_detail: row_progress_detail(&row)?,
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

pub(super) async fn pending_chunks(pool: &SqlitePool) -> Result<Vec<ChunkRecord>, String> {
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
            sequence: row.get("sequence"),
            source_text: row.get("source_text"),
            map_json: row.get("map_json"),
        })
        .collect())
}

pub(super) async fn glossary_source_chunks(pool: &SqlitePool) -> Result<Vec<ChunkRecord>, String> {
    let rows =
        sqlx::query("SELECT id, sequence, source_text, map_json FROM chunks ORDER BY sequence")
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| ChunkRecord {
            id: row.get("id"),
            sequence: row.get("sequence"),
            source_text: row.get("source_text"),
            map_json: row.get("map_json"),
        })
        .collect())
}

pub(super) async fn task_assistant_prompt(pool: &SqlitePool) -> Result<Option<String>, String> {
    let prompt: Option<String> =
        sqlx::query_scalar("SELECT assistant_system_prompt FROM metadata LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
    Ok(prompt)
}

pub(super) async fn task_glossary_config(pool: &SqlitePool) -> Result<TaskGlossaryConfig, String> {
    let row = sqlx::query("SELECT use_glossary, glossary_mode, glossary_id FROM metadata LIMIT 1")
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(TaskGlossaryConfig {
        use_glossary: row.get::<i64, _>("use_glossary") != 0,
        glossary_mode: GlossaryMode::parse(row.get::<String, _>("glossary_mode").as_str())?,
        glossary_id: row.get("glossary_id"),
    })
}

pub(super) async fn set_task_glossary_id(
    pool: &SqlitePool,
    glossary_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE metadata
         SET glossary_id = ?, updated_at = ?
         WHERE task_id = (SELECT task_id FROM metadata LIMIT 1)",
    )
    .bind(glossary_id)
    .bind(unix_timestamp())
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn task_assistant_custom_parameters(pool: &SqlitePool) -> Result<Value, String> {
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

pub(super) async fn task_use_custom_parameters(pool: &SqlitePool) -> Result<bool, String> {
    let json: Option<String> =
        sqlx::query_scalar("SELECT config_snapshot_json FROM metadata LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
    let Some(value) = json.filter(|value| !value.trim().is_empty()) else {
        return Ok(false);
    };
    let parsed = serde_json::from_str::<Value>(&value)
        .map_err(|error| format!("Task config snapshot JSON is invalid: {error}"))?;
    Ok(parsed
        .get("useCustomParameters")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

pub(super) fn config_snapshot_json(
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
        "contextHandlingMode": config.context_handling_mode,
        "useGlossary": config.use_glossary,
        "glossaryMode": config.glossary_mode,
        "glossaryId": config.glossary_id,
        "thinkingEffort": config.thinking_effort,
        "useWebSearch": config.use_web_search,
        "useCustomParameters": config.use_custom_parameters,
        "confidenceMode": config.confidence_mode,
        "pdfParsingMode": config.pdf_parsing_mode,
        "providerId": provider_id,
        "modelId": model_id
    })
    .to_string()
}

pub(super) fn normalize_task_filters(
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

pub(super) fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
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

pub(super) fn serialize_tags(tags: &[String]) -> Result<String, String> {
    serde_json::to_string(tags).map_err(|error| error.to_string())
}

fn parse_tags_json(tags_json: String) -> Result<Vec<String>, String> {
    let tags = serde_json::from_str::<Vec<String>>(&tags_json)
        .map_err(|error| format!("Stored task tags are invalid: {error}"))?;
    normalize_tags(tags)
}

fn serialize_progress_detail(detail: Option<&ProgressDetail>) -> Result<Option<String>, String> {
    detail
        .map(|value| serde_json::to_string(value).map_err(|error| error.to_string()))
        .transpose()
}

fn parse_progress_detail_json(
    progress_detail_json: Option<String>,
) -> Result<Option<ProgressDetail>, String> {
    progress_detail_json
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str::<ProgressDetail>(&value)
                .map_err(|error| format!("Stored task progress detail is invalid: {error}"))
        })
        .transpose()
}

fn serialize_active_retry(retry: Option<&DbActiveRetry>) -> Result<Option<String>, String> {
    retry
        .map(|value| serde_json::to_string(value).map_err(|error| error.to_string()))
        .transpose()
}

fn serialize_task_active_retry(
    retry: Option<&TranslationTaskActiveRetry>,
) -> Result<Option<String>, String> {
    retry
        .map(|value| serde_json::to_string(value).map_err(|error| error.to_string()))
        .transpose()
}

fn parse_db_active_retry_json(
    active_retry_json: Option<String>,
) -> Result<Option<DbActiveRetry>, String> {
    active_retry_json
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str::<DbActiveRetry>(&value)
                .map_err(|error| format!("Stored task active retry is invalid: {error}"))
        })
        .transpose()
}

fn parse_active_retry_json(
    active_retry_json: Option<String>,
) -> Result<Option<TranslationTaskActiveRetry>, String> {
    active_retry_json
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            if let Ok(retry) = serde_json::from_str::<DbActiveRetry>(&value) {
                return Ok(TranslationTaskActiveRetry {
                    current: retry.current,
                    max: retry.max,
                    message: retry.message,
                });
            }
            serde_json::from_str::<TranslationTaskActiveRetry>(&value)
                .map_err(|error| format!("Stored task active retry is invalid: {error}"))
        })
        .transpose()
}

fn row_progress_detail(row: &sqlx::sqlite::SqliteRow) -> Result<Option<ProgressDetail>, String> {
    parse_progress_detail_json(
        row.try_get::<Option<String>, _>("progress_detail_json")
            .unwrap_or(None),
    )
}

fn row_active_retry(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<Option<TranslationTaskActiveRetry>, String> {
    parse_active_retry_json(
        row.try_get::<Option<String>, _>("active_retry_json")
            .unwrap_or(None),
    )
}

fn count_label(name: &str, current: u64, total: u64) -> String {
    format!("{name} ({current}/{total})")
}

fn glossary_step_for_config(config: &TaskGlossaryConfig, current: u64, total: u64) -> ProgressStep {
    if !config.use_glossary {
        return ProgressStep::success(1, 1, "术语表已忽略");
    }
    if config.glossary_mode == GlossaryMode::Existing || config.glossary_id.is_some() {
        return ProgressStep::success(1, 1, "术语表已选择");
    }
    if current == 0 {
        ProgressStep::pending(current, total, count_label("术语表建立", current, total))
    } else if current >= total && total > 0 {
        ProgressStep::success(current, total, count_label("术语表建立", current, total))
    } else {
        ProgressStep::running(current, total, count_label("术语表建立", current, total))
    }
}

pub(super) fn progress_detail_for_config(
    total_chunks: u64,
    completed_chunks: u64,
    config: &TaskGlossaryConfig,
) -> ProgressDetail {
    ProgressDetail {
        ast: ProgressStep::success(1, 1, "AST 已完成"),
        chunking: ProgressStep::success(
            total_chunks,
            total_chunks,
            count_label("分块", total_chunks, total_chunks),
        ),
        glossary: glossary_step_for_config(config, 0, total_chunks),
        translating: ProgressStep::pending(
            completed_chunks,
            total_chunks,
            count_label("翻译", completed_chunks, total_chunks),
        ),
        restore: ProgressStep::pending(
            completed_chunks,
            total_chunks,
            count_label("占位符恢复", completed_chunks, total_chunks),
        ),
    }
}

pub(super) fn progress_detail_for_translation_stats(
    existing: Option<ProgressDetail>,
    total_chunks: u64,
    completed_chunks: u64,
    task_status: TranslationTaskStatus,
    config: &TaskGlossaryConfig,
) -> ProgressDetail {
    let mut detail = existing
        .unwrap_or_else(|| progress_detail_for_config(total_chunks, completed_chunks, config));
    if matches!(detail.glossary.state.as_str(), "pending" | "running") && !config.use_glossary {
        detail.glossary = ProgressStep::success(1, 1, "术语表已忽略");
    } else if matches!(detail.glossary.state.as_str(), "pending" | "running")
        && config.glossary_mode == GlossaryMode::Existing
    {
        detail.glossary = ProgressStep::success(1, 1, "术语表已选择");
    }
    let step_state = match task_status {
        TranslationTaskStatus::Failed | TranslationTaskStatus::Interrupted => "failed",
        TranslationTaskStatus::Success if completed_chunks >= total_chunks => "success",
        TranslationTaskStatus::Success => "failed",
        TranslationTaskStatus::Pending => "pending",
        TranslationTaskStatus::Running | TranslationTaskStatus::InterruptedPending => "running",
    };
    detail.translating = ProgressStep::new(
        step_state,
        completed_chunks,
        total_chunks,
        count_label("翻译", completed_chunks, total_chunks),
    );
    detail.restore = ProgressStep::new(
        step_state,
        completed_chunks,
        total_chunks,
        count_label("占位符恢复", completed_chunks, total_chunks),
    );
    detail
}

pub(super) async fn set_progress_detail(
    pool: &SqlitePool,
    detail: &ProgressDetail,
) -> Result<(), String> {
    let progress_detail_json = serialize_progress_detail(Some(detail))?;
    sqlx::query(
        "UPDATE metadata
         SET progress_detail_json = ?, updated_at = ?
         WHERE task_id = (SELECT task_id FROM metadata LIMIT 1)",
    )
    .bind(progress_detail_json)
    .bind(unix_timestamp())
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn set_progress_detail_and_emit(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    detail: &ProgressDetail,
    forced_status: Option<TranslationTaskStatus>,
) -> Result<TranslationTaskView, String> {
    set_progress_detail(inp_pool, detail).await?;
    let task = refresh_task_stats(inp_pool, config_pool, inp_path, forced_status).await?;
    let _ = app.emit(
        TRANSLATION_PROGRESS_EVENT,
        TranslationProgressPayload { task: task.clone() },
    );
    Ok(task)
}

pub(super) fn validate_task_name(value: &str) -> Result<String, String> {
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

pub(super) fn source_extension(path: &str) -> Result<&'static str, String> {
    document_parsing::source_extension(path)
}

pub(super) fn export_file_name(output_name: &str, fallback_name: &str, extension: &str) -> String {
    let name = output_name
        .trim()
        .strip_suffix(&format!(".{extension}"))
        .unwrap_or(output_name.trim());
    let base = sanitize_file_stem(if name.is_empty() { fallback_name } else { name });
    format!("{base}.{extension}")
}

pub(super) fn document_format_from_source_path(path: &str) -> Result<DocumentFormat, String> {
    document_parsing::document_format_from_path(Path::new(path))
}

pub(super) fn content_format_from_source_path(path: &str) -> Result<ContentFormat, String> {
    document_parsing::content_format_from_path(Path::new(path))
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
