use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::glossary_prompt::{sanitize_and_flatten_glossary, GlossaryEntry};
use crate::languages::{
    normalize_language_code, normalize_source_language, normalize_target_language,
};

const CONFIG_DB_FILE: &str = "config.db";
const GLOSSARIES_DIR: &str = "glossaries";
const ING_SCHEMA_VERSION: i64 = 2;
const MAX_TAGS: usize = 12;
const MAX_TAG_LENGTH: usize = 48;
const MAX_NAME_LENGTH: usize = 120;
const DEFAULT_PAGE_SIZE: i64 = 100;
const MAX_PAGE_SIZE: i64 = 500;
pub const GLOSSARY_PROGRESS_EVENT: &str = "glossary-progress";

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryView {
    pub id: String,
    pub name: String,
    pub ing_path: String,
    pub source_language: String,
    pub target_language: String,
    pub tags: Vec<String>,
    pub source_type: String,
    pub entry_count: i64,
    #[serde(default = "default_glossary_status")]
    pub status: GlossaryStatus,
    #[serde(default)]
    pub has_failures: bool,
    #[serde(default)]
    pub origin_task_id: Option<String>,
    #[serde(default)]
    pub total_chunks: i64,
    #[serde(default)]
    pub success_chunks: i64,
    #[serde(default)]
    pub failed_chunks: i64,
    #[serde(default)]
    pub interrupted_chunks: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryProgressPayload {
    pub glossary: GlossaryView,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GlossaryStatus {
    Initializing,
    Building,
    Interrupted,
    Success,
    Failed,
}

impl GlossaryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initializing => "initializing",
            Self::Building => "building",
            Self::Interrupted => "interrupted",
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "initializing" => Ok(Self::Initializing),
            "building" => Ok(Self::Building),
            "interrupted" => Ok(Self::Interrupted),
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Unsupported glossary status: {value}")),
        }
    }
}

fn default_glossary_status() -> GlossaryStatus {
    GlossaryStatus::Success
}

#[derive(Debug, Clone)]
pub struct GlossaryDeletionTicket {
    pub glossary: GlossaryView,
    pub original_path: PathBuf,
    pub staged_path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryListQuery {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub source_language: Option<String>,
    #[serde(default)]
    pub target_language: Option<String>,
    #[serde(default)]
    pub sort: Option<GlossarySortInput>,
    #[serde(default)]
    pub usable_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossarySortInput {
    pub field: GlossarySortField,
    pub mode: SortMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GlossarySortField {
    Name,
    Tags,
    Language,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SortMode {
    CreatedDesc,
    CreatedAsc,
    Az,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportGlossaryInput {
    pub file_path: String,
    pub name: String,
    pub source_language: String,
    pub target_language: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGlossaryInput {
    pub glossary_id: String,
    pub name: String,
    pub source_language: String,
    pub target_language: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportGlossaryInput {
    pub id: String,
    pub format: GlossaryExportFormat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlossaryExportFormat {
    Csv,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntryView {
    pub id: String,
    pub src: String,
    pub dst: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntryPage {
    pub entries: Vec<GlossaryEntryView>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryFailedChunkView {
    pub id: String,
    pub sequence: i64,
    pub display_source_text: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryFailedChunkPage {
    pub chunks: Vec<GlossaryFailedChunkView>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryFailedChunksQuery {
    pub id: String,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub page_size: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntriesQuery {
    pub id: String,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub page_size: i64,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub sort: Option<GlossaryEntrySortInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntrySortInput {
    pub field: GlossaryEntrySortField,
    pub mode: SortMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GlossaryEntrySortField {
    Src,
    Dst,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGlossaryEntryInput {
    pub glossary_id: String,
    pub src: String,
    pub dst: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGlossaryEntryInput {
    pub glossary_id: String,
    pub entry_id: String,
    pub src: String,
    pub dst: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGlossaryEntryInput {
    pub glossary_id: String,
    pub entry_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareAutoGlossaryInput {
    pub task_id: String,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct CreateAutoGlossaryInput {
    pub name: String,
    pub source_language: String,
    pub target_language: String,
    pub entries: Vec<GlossaryEntry>,
}

#[derive(Debug, Clone)]
pub struct AutoGlossarySourceChunk {
    pub id: String,
    pub sequence: i64,
    pub glossary_source_text: String,
    pub display_source_text: String,
}

#[derive(Debug, Clone)]
pub struct EnsureAutoGlossaryDraftInput {
    pub name: String,
    pub source_language: String,
    pub target_language: String,
    pub origin_task_id: String,
    pub chunks: Vec<AutoGlossarySourceChunk>,
}

#[derive(Debug, Clone)]
pub struct AutoGlossaryGenerationChunk {
    pub id: String,
    pub sequence: i64,
    pub glossary_source_text: String,
    pub display_source_text: String,
}

#[derive(Clone)]
pub struct AutoGlossaryDraft {
    pub view: GlossaryView,
    pub pool: SqlitePool,
}

#[derive(Debug, Clone)]
struct NormalizedEntry {
    src: String,
    dst: String,
}

pub fn workspace_root(app_data: &Path) -> PathBuf {
    app_data.join("glossary-workspace")
}

pub async fn connect_config_db(workspace_root: &Path) -> Result<SqlitePool, String> {
    tokio::fs::create_dir_all(workspace_root.join(GLOSSARIES_DIR))
        .await
        .map_err(|error| error.to_string())?;
    let pool = connect_sqlite(&workspace_root.join(CONFIG_DB_FILE), 5).await?;
    migrate_config_db(&pool).await?;
    Ok(pool)
}

async fn connect_ing(path: &Path) -> Result<SqlitePool, String> {
    let pool = connect_sqlite(path, 1).await?;
    migrate_ing_db(&pool).await?;
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
        r#"CREATE TABLE IF NOT EXISTS glossary_index (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            ing_path TEXT NOT NULL UNIQUE,
            source_language TEXT NOT NULL,
            target_language TEXT NOT NULL,
            tags_json TEXT NOT NULL DEFAULT '[]',
            source_type TEXT NOT NULL DEFAULT 'uploaded',
            entry_count INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'success',
            has_failures INTEGER NOT NULL DEFAULT 0,
            origin_task_id TEXT,
            total_chunks INTEGER NOT NULL DEFAULT 0,
            success_chunks INTEGER NOT NULL DEFAULT 0,
            failed_chunks INTEGER NOT NULL DEFAULT 0,
            interrupted_chunks INTEGER NOT NULL DEFAULT 0,
            name_sort_key TEXT NOT NULL DEFAULT '',
            tags_sort_key TEXT NOT NULL DEFAULT '',
            language_sort_key TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS glossary_deletion_journal (
            glossary_id TEXT PRIMARY KEY NOT NULL,
            task_id TEXT NOT NULL,
            glossary_json TEXT NOT NULL,
            original_path TEXT NOT NULL,
            staged_path TEXT NOT NULL,
            created_at TEXT NOT NULL
        )"#,
        "CREATE INDEX IF NOT EXISTS idx_glossary_index_languages ON glossary_index(source_language, target_language)",
        "CREATE INDEX IF NOT EXISTS idx_glossary_index_updated ON glossary_index(updated_at)",
    ];
    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    add_column_if_missing(
        pool,
        "glossary_index",
        "status",
        "TEXT NOT NULL DEFAULT 'success'",
    )
    .await?;
    add_column_if_missing(
        pool,
        "glossary_index",
        "has_failures",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(pool, "glossary_index", "origin_task_id", "TEXT").await?;
    add_column_if_missing(
        pool,
        "glossary_index",
        "total_chunks",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "glossary_index",
        "success_chunks",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "glossary_index",
        "failed_chunks",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "glossary_index",
        "interrupted_chunks",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_glossary_index_origin_task
         ON glossary_index(origin_task_id) WHERE origin_task_id IS NOT NULL",
    )
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn migrate_ing_db(pool: &SqlitePool) -> Result<(), String> {
    let statements = [
        r#"CREATE TABLE IF NOT EXISTS metadata (
            glossary_id TEXT PRIMARY KEY NOT NULL,
            schema_version INTEGER NOT NULL,
            name TEXT NOT NULL,
            source_language TEXT NOT NULL,
            target_language TEXT NOT NULL,
            tags_json TEXT NOT NULL DEFAULT '[]',
            source_type TEXT NOT NULL DEFAULT 'uploaded',
            entry_count INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'success',
            has_failures INTEGER NOT NULL DEFAULT 0,
            origin_task_id TEXT,
            total_chunks INTEGER NOT NULL DEFAULT 0,
            success_chunks INTEGER NOT NULL DEFAULT 0,
            failed_chunks INTEGER NOT NULL DEFAULT 0,
            interrupted_chunks INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS entries (
            id TEXT PRIMARY KEY NOT NULL,
            src TEXT NOT NULL,
            dst TEXT NOT NULL,
            src_norm TEXT NOT NULL UNIQUE,
            src_sort_key TEXT NOT NULL,
            dst_sort_key TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS generation_chunks (
            id TEXT PRIMARY KEY NOT NULL,
            sequence INTEGER NOT NULL,
            glossary_source_text TEXT,
            display_source_text TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            retry_count INTEGER NOT NULL DEFAULT 0,
            error_message TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        "CREATE INDEX IF NOT EXISTS idx_entries_created ON entries(created_at)",
        "CREATE INDEX IF NOT EXISTS idx_entries_src_sort ON entries(src_sort_key)",
        "CREATE INDEX IF NOT EXISTS idx_entries_dst_sort ON entries(dst_sort_key)",
        "CREATE INDEX IF NOT EXISTS idx_generation_chunks_status ON generation_chunks(status, sequence)",
    ];
    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    add_column_if_missing(
        pool,
        "metadata",
        "status",
        "TEXT NOT NULL DEFAULT 'success'",
    )
    .await?;
    add_column_if_missing(
        pool,
        "metadata",
        "has_failures",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(pool, "metadata", "origin_task_id", "TEXT").await?;
    add_column_if_missing(
        pool,
        "metadata",
        "total_chunks",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "metadata",
        "success_chunks",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "metadata",
        "failed_chunks",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "metadata",
        "interrupted_chunks",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    sqlx::query("UPDATE metadata SET schema_version = ? WHERE schema_version < ?")
        .bind(ING_SCHEMA_VERSION)
        .bind(ING_SCHEMA_VERSION)
        .execute(pool)
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
    if rows
        .iter()
        .any(|row| row.get::<String, _>("name") == column)
    {
        return Ok(());
    }
    sqlx::query(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {definition}"
    ))
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn list_glossaries(
    pool: &SqlitePool,
    query: Option<GlossaryListQuery>,
) -> Result<Vec<GlossaryView>, String> {
    let query = normalize_list_query(query)?;
    let search_like = query
        .search
        .as_ref()
        .map(|value| format!("%{}%", escape_like(value)));
    let rows = sqlx::query(
        r#"SELECT * FROM glossary_index
           WHERE status <> 'initializing'
             AND (? = 0 OR status = 'success')
             AND (
                ? IS NULL OR name LIKE ? ESCAPE '\'
                OR source_language LIKE ? ESCAPE '\'
                OR target_language LIKE ? ESCAPE '\'
                OR tags_json LIKE ? ESCAPE '\'
             )"#,
    )
    .bind(query.usable_only)
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut glossaries = rows
        .iter()
        .map(glossary_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(tag) = query.tag {
        glossaries.retain(|item| {
            item.tags
                .iter()
                .any(|value| value.eq_ignore_ascii_case(&tag))
        });
    }
    if let Some(source_language) = query.source_language {
        glossaries.retain(|item| same_language(&item.source_language, &source_language));
    }
    if let Some(target_language) = query.target_language {
        glossaries.retain(|item| same_language(&item.target_language, &target_language));
    }
    sort_glossary_views(&mut glossaries, query.sort);
    Ok(glossaries)
}

pub async fn get_glossary(pool: &SqlitePool, id: &str) -> Result<GlossaryView, String> {
    get_glossary_from_index(pool, id).await
}

pub async fn detach_auto_glossary_origin(
    pool: &SqlitePool,
    glossary_id: &str,
    origin_task_id: &str,
) -> Result<(), String> {
    let glossary = get_glossary_from_index(pool, glossary_id).await?;
    if glossary.origin_task_id.as_deref() != Some(origin_task_id) {
        return Ok(());
    }
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    sqlx::query(
        "UPDATE metadata SET origin_task_id = NULL, updated_at = ?
         WHERE glossary_id = ? AND origin_task_id = ?",
    )
    .bind(unix_timestamp())
    .bind(glossary_id)
    .bind(origin_task_id)
    .execute(&ing_pool)
    .await
    .map_err(|error| error.to_string())?;
    let updated = metadata_glossary(&ing_pool, Path::new(&glossary.ing_path)).await?;
    upsert_glossary_index(pool, &updated).await?;
    ing_pool.close().await;
    Ok(())
}

pub async fn import_glossary(
    pool: &SqlitePool,
    workspace_root: &Path,
    input: ImportGlossaryInput,
) -> Result<GlossaryView, String> {
    let name = normalize_name(&input.name)?;
    let source_language = normalize_glossary_source_language(&input.source_language)?;
    let target_language = normalize_language(&input.target_language)?;
    let tags = normalize_tags(input.tags)?;
    let source_path = PathBuf::from(input.file_path.trim());
    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let content = tokio::fs::read_to_string(&source_path)
        .await
        .map_err(|error| format!("无法读取术语表文件：{error}"))?;
    let entries = match extension.as_str() {
        "csv" => parse_csv_entries(&content)?,
        "json" => parse_json_entries(&content)?,
        _ => return Err("文件格式不正确：仅支持 csv 和 json".into()),
    };
    let id = new_id("glossary");
    let ing_path = next_ing_path(workspace_root, &name).await?;
    let created_at = unix_timestamp();
    let ing_pool = connect_ing(&ing_path).await?;
    let mut transaction = ing_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        r#"INSERT INTO metadata (
            glossary_id, schema_version, name, source_language, target_language, tags_json,
            source_type, entry_count, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, 'uploaded', 0, ?, ?)"#,
    )
    .bind(&id)
    .bind(ING_SCHEMA_VERSION)
    .bind(&name)
    .bind(&source_language)
    .bind(&target_language)
    .bind(serialize_tags(&tags)?)
    .bind(&created_at)
    .bind(&created_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    for entry in entries {
        insert_entry_ignore_query(&entry.src, &entry.dst, &created_at)
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    refresh_metadata_count(&ing_pool, &created_at).await?;
    let view = metadata_glossary(&ing_pool, &ing_path).await?;
    upsert_glossary_index(pool, &view).await?;
    ing_pool.close().await;
    Ok(view)
}

pub async fn update_glossary(
    pool: &SqlitePool,
    input: UpdateGlossaryInput,
) -> Result<GlossaryView, String> {
    let glossary = get_glossary_from_index(pool, &input.glossary_id).await?;
    ensure_glossary_usable(&glossary)?;
    let name = normalize_name(&input.name)?;
    let source_language = normalize_glossary_source_language(&input.source_language)?;
    let target_language = normalize_language(&input.target_language)?;
    let tags = normalize_tags(input.tags)?;
    let now = unix_timestamp();
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    sqlx::query(
        r#"UPDATE metadata
           SET name = ?, source_language = ?, target_language = ?, tags_json = ?, updated_at = ?
           WHERE glossary_id = ?"#,
    )
    .bind(&name)
    .bind(&source_language)
    .bind(&target_language)
    .bind(serialize_tags(&tags)?)
    .bind(&now)
    .bind(&input.glossary_id)
    .execute(&ing_pool)
    .await
    .map_err(|error| error.to_string())?;
    let view = metadata_glossary(&ing_pool, Path::new(&glossary.ing_path)).await?;
    upsert_glossary_index(pool, &view).await?;
    ing_pool.close().await;
    Ok(view)
}

pub async fn delete_glossary(
    pool: &SqlitePool,
    translation_config_pool: &SqlitePool,
    workspace_root: &Path,
    id: &str,
) -> Result<(), String> {
    let ticket =
        stage_manual_glossary_deletion(pool, translation_config_pool, workspace_root, id).await?;
    commit_staged_glossary_deletion(pool, &ticket).await
}

async fn stage_manual_glossary_deletion(
    pool: &SqlitePool,
    translation_config_pool: &SqlitePool,
    workspace_root: &Path,
    glossary_id: &str,
) -> Result<GlossaryDeletionTicket, String> {
    let references: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_index WHERE glossary_id = ?")
            .bind(glossary_id)
            .fetch_one(translation_config_pool)
            .await
            .map_err(|error| error.to_string())?;
    if references > 0 {
        return Err("术语表仍被任务引用，无法删除".into());
    }
    let glossary = get_glossary_from_index(pool, glossary_id).await?;
    let original_path = PathBuf::from(&glossary.ing_path);
    if !original_path.starts_with(workspace_root) {
        return Err("Refusing to delete a glossary outside the workspace".into());
    }
    let glossary_json = serde_json::to_string(&glossary).map_err(|error| error.to_string())?;
    let staged_path = original_path.with_extension(format!("deleting-{}", new_id("cleanup")));
    tokio::fs::rename(&original_path, &staged_path)
        .await
        .map_err(|error| format!("Unable to stage glossary for deletion: {error}"))?;
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    let result = async {
        sqlx::query("DELETE FROM glossary_index WHERE id = ?")
            .bind(glossary_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO glossary_deletion_journal (
                glossary_id, task_id, glossary_json, original_path, staged_path, created_at
             ) VALUES (?, '__manual__', ?, ?, ?, ?)",
        )
        .bind(glossary_id)
        .bind(glossary_json)
        .bind(original_path.to_string_lossy().to_string())
        .bind(staged_path.to_string_lossy().to_string())
        .bind(unix_timestamp())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())
    }
    .await;
    if let Err(error) = result {
        let _ = tokio::fs::rename(&staged_path, &original_path).await;
        return Err(error);
    }
    Ok(GlossaryDeletionTicket {
        glossary,
        original_path,
        staged_path,
    })
}

pub async fn stage_unreferenced_auto_glossary_deletion(
    pool: &SqlitePool,
    translation_config_pool: &SqlitePool,
    workspace_root: &Path,
    task_id: &str,
    glossary_id: &str,
) -> Result<Option<GlossaryDeletionTicket>, String> {
    let glossary = get_glossary_from_index(pool, glossary_id).await?;
    if glossary.source_type != "auto" {
        return Ok(None);
    }
    let other_references: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_index WHERE glossary_id = ? AND id <> ?")
            .bind(glossary_id)
            .bind(task_id)
            .fetch_one(translation_config_pool)
            .await
            .map_err(|error| error.to_string())?;
    if other_references > 0 {
        return Ok(None);
    }
    let original_path = PathBuf::from(&glossary.ing_path);
    if !original_path.starts_with(workspace_root) {
        return Err("Refusing to stage a glossary outside the workspace".into());
    }
    let glossary_json = serde_json::to_string(&glossary).map_err(|error| error.to_string())?;
    let staged_path = original_path.with_extension(format!("deleting-{}", new_id("cleanup")));
    tokio::fs::rename(&original_path, &staged_path)
        .await
        .map_err(|error| format!("Unable to stage old glossary for deletion: {error}"))?;
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    let result = async {
        sqlx::query("DELETE FROM glossary_index WHERE id = ?")
            .bind(glossary_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO glossary_deletion_journal (
                glossary_id, task_id, glossary_json, original_path, staged_path, created_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(glossary_id)
        .bind(task_id)
        .bind(glossary_json)
        .bind(original_path.to_string_lossy().to_string())
        .bind(staged_path.to_string_lossy().to_string())
        .bind(unix_timestamp())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())
    }
    .await;
    if let Err(error) = result {
        let _ = tokio::fs::rename(&staged_path, &original_path).await;
        return Err(error);
    }
    Ok(Some(GlossaryDeletionTicket {
        glossary,
        original_path,
        staged_path,
    }))
}

pub async fn commit_staged_glossary_deletion(
    pool: &SqlitePool,
    ticket: &GlossaryDeletionTicket,
) -> Result<(), String> {
    match tokio::fs::remove_file(&ticket.staged_path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Unable to delete old glossary file: {error}")),
    }
    sqlx::query("DELETE FROM glossary_deletion_journal WHERE glossary_id = ?")
        .bind(&ticket.glossary.id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn rollback_staged_glossary_deletion(
    pool: &SqlitePool,
    ticket: &GlossaryDeletionTicket,
) -> Result<(), String> {
    if tokio::fs::try_exists(&ticket.staged_path)
        .await
        .map_err(|error| error.to_string())?
    {
        tokio::fs::rename(&ticket.staged_path, &ticket.original_path)
            .await
            .map_err(|error| format!("Unable to restore old glossary file: {error}"))?;
    }
    upsert_glossary_index(pool, &ticket.glossary).await?;
    sqlx::query("DELETE FROM glossary_deletion_journal WHERE glossary_id = ?")
        .bind(&ticket.glossary.id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn recover_glossary_deletion_journal(
    pool: &SqlitePool,
    translation_config_pool: &SqlitePool,
) -> Result<(), String> {
    let rows = sqlx::query("SELECT * FROM glossary_deletion_journal ORDER BY created_at")
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    for row in rows {
        let glossary = serde_json::from_str::<GlossaryView>(row.get("glossary_json"))
            .map_err(|error| format!("Stored glossary deletion journal is invalid: {error}"))?;
        let ticket = GlossaryDeletionTicket {
            original_path: PathBuf::from(row.get::<String, _>("original_path")),
            staged_path: PathBuf::from(row.get::<String, _>("staged_path")),
            glossary,
        };
        let references: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_index WHERE glossary_id = ?")
                .bind(&ticket.glossary.id)
                .fetch_one(translation_config_pool)
                .await
                .map_err(|error| error.to_string())?;
        if references > 0 {
            rollback_staged_glossary_deletion(pool, &ticket).await?;
        } else {
            commit_staged_glossary_deletion(pool, &ticket).await?;
        }
    }
    Ok(())
}

pub async fn recover_auto_glossary_drafts(pool: &SqlitePool) -> Result<(), String> {
    let rows =
        sqlx::query("SELECT * FROM glossary_index WHERE status IN ('initializing', 'building')")
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
    for row in rows {
        let glossary = glossary_from_row(&row)?;
        let ing_path = PathBuf::from(&glossary.ing_path);
        if !tokio::fs::try_exists(&ing_path)
            .await
            .map_err(|error| error.to_string())?
        {
            let temporary_path = ing_path.with_extension(format!("creating-{}", glossary.id));
            if tokio::fs::try_exists(&temporary_path)
                .await
                .map_err(|error| error.to_string())?
            {
                tokio::fs::remove_file(&temporary_path)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            sqlx::query("DELETE FROM glossary_index WHERE id = ? AND status = 'initializing'")
                .bind(&glossary.id)
                .execute(pool)
                .await
                .map_err(|error| error.to_string())?;
            continue;
        }
        let ing_pool = connect_ing(&ing_path).await?;
        let metadata_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metadata")
            .fetch_one(&ing_pool)
            .await
            .map_err(|error| error.to_string())?;
        if metadata_count == 0 {
            ing_pool.close().await;
            tokio::fs::remove_file(&ing_path)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query("DELETE FROM glossary_index WHERE id = ?")
                .bind(&glossary.id)
                .execute(pool)
                .await
                .map_err(|error| error.to_string())?;
            continue;
        }
        let now = unix_timestamp();
        let mut transaction = ing_pool.begin().await.map_err(|error| error.to_string())?;
        sqlx::query(
            "UPDATE generation_chunks SET status = 'interrupted', updated_at = ?
             WHERE status = 'running'",
        )
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query("UPDATE metadata SET status = 'interrupted', updated_at = ?")
            .bind(&now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
        refresh_generation_metadata_in_transaction(&mut transaction, &now).await?;
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        let recovered = metadata_glossary(&ing_pool, &ing_path).await?;
        upsert_glossary_index(pool, &recovered).await?;
        ing_pool.close().await;
    }
    Ok(())
}

fn export_filter(format: GlossaryExportFormat) -> (&'static str, [&'static str; 1]) {
    match format {
        GlossaryExportFormat::Csv => ("CSV", ["csv"]),
        GlossaryExportFormat::Json => ("JSON", ["json"]),
    }
}

pub async fn open_glossary_folder(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let glossary = get_glossary_from_index(pool, id).await?;
    let ing_path = PathBuf::from(&glossary.ing_path);
    open_folder_selecting_file(&ing_path)
}

pub async fn export_glossary(
    app: AppHandle,
    pool: &SqlitePool,
    input: ExportGlossaryInput,
) -> Result<(), String> {
    let glossary = get_glossary_from_index(pool, &input.id).await?;
    ensure_glossary_usable(&glossary)?;
    let extension = match input.format {
        GlossaryExportFormat::Csv => "csv",
        GlossaryExportFormat::Json => "json",
    };
    let (filter_name, filter_extensions) = export_filter(input.format);
    let default_name = format!("{}.{}", sanitize_file_stem(&glossary.name), extension);
    let save_path = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(&default_name)
            .add_filter(filter_name, &filter_extensions)
            .blocking_save_file()
    })
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "Export cancelled".to_string())?;
    let save_path: PathBuf = save_path
        .try_into()
        .map_err(|error| format!("Unable to resolve export path: {error}"))?;
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    let rows = sqlx::query("SELECT src, dst FROM entries ORDER BY created_at ASC")
        .fetch_all(&ing_pool)
        .await
        .map_err(|error| error.to_string())?;
    let output = match input.format {
        GlossaryExportFormat::Csv => export_csv(&rows),
        GlossaryExportFormat::Json => export_json(&rows)?,
    };
    ing_pool.close().await;
    tokio::fs::write(&save_path, output)
        .await
        .map_err(|error| format!("Unable to export glossary: {error}"))?;
    open_folder_selecting_file(&save_path)?;
    Ok(())
}

pub async fn get_glossary_entries(
    pool: &SqlitePool,
    query: GlossaryEntriesQuery,
) -> Result<GlossaryEntryPage, String> {
    let glossary = get_glossary_from_index(pool, &query.id).await?;
    let page = query.page.max(0);
    let page_size = if query.page_size <= 0 {
        DEFAULT_PAGE_SIZE
    } else {
        query.page_size.min(MAX_PAGE_SIZE)
    };
    let offset = page * page_size;
    let search = normalize_optional_filter(query.search);
    let search_like = search
        .as_ref()
        .map(|value| format!("%{}%", escape_like(value)));
    let order_by = entry_order_by(query.sort);
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    let total: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM entries
           WHERE ? IS NULL OR src LIKE ? ESCAPE '\' OR dst LIKE ? ESCAPE '\'"#,
    )
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .fetch_one(&ing_pool)
    .await
    .map_err(|error| error.to_string())?;
    let rows = sqlx::query(&format!(
        r#"SELECT * FROM entries
           WHERE ? IS NULL OR src LIKE ? ESCAPE '\' OR dst LIKE ? ESCAPE '\'
           ORDER BY {order_by}
           LIMIT ? OFFSET ?"#
    ))
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(page_size)
    .bind(offset)
    .fetch_all(&ing_pool)
    .await
    .map_err(|error| error.to_string())?;
    let entries = rows.iter().map(entry_from_row).collect();
    ing_pool.close().await;
    Ok(GlossaryEntryPage {
        entries,
        total,
        page,
        page_size,
    })
}

pub async fn get_glossary_failed_chunks(
    pool: &SqlitePool,
    query: GlossaryFailedChunksQuery,
) -> Result<GlossaryFailedChunkPage, String> {
    let glossary = get_glossary_from_index(pool, &query.id).await?;
    let page = query.page.max(0);
    let page_size = if query.page_size <= 0 {
        DEFAULT_PAGE_SIZE
    } else {
        query.page_size.min(MAX_PAGE_SIZE)
    };
    let offset = page * page_size;
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM generation_chunks WHERE status = 'failed'")
            .fetch_one(&ing_pool)
            .await
            .map_err(|error| error.to_string())?;
    let rows = sqlx::query(
        "SELECT id, sequence, display_source_text, error_message
         FROM generation_chunks
         WHERE status = 'failed'
         ORDER BY sequence ASC
         LIMIT ? OFFSET ?",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(&ing_pool)
    .await
    .map_err(|error| error.to_string())?;
    let chunks = rows
        .into_iter()
        .map(|row| {
            let display_source_text = row
                .get::<Option<String>, _>("display_source_text")
                .ok_or_else(|| "Display source text is missing for a failed chunk".to_string())?;
            Ok(GlossaryFailedChunkView {
                id: row.get("id"),
                sequence: row.get("sequence"),
                display_source_text,
                error_message: row.get("error_message"),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    ing_pool.close().await;
    Ok(GlossaryFailedChunkPage {
        chunks,
        total,
        page,
        page_size,
    })
}

pub async fn create_glossary_entry(
    pool: &SqlitePool,
    input: CreateGlossaryEntryInput,
) -> Result<GlossaryEntryView, String> {
    let glossary = get_glossary_from_index(pool, &input.glossary_id).await?;
    ensure_glossary_usable(&glossary)?;
    let entry = normalize_entry(&input.src, &input.dst)?;
    let now = unix_timestamp();
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    insert_entry_query(&entry.src, &entry.dst, &now)
        .execute(&ing_pool)
        .await
        .map_err(|error| entry_error(error))?;
    refresh_metadata_count(&ing_pool, &now).await?;
    let row = sqlx::query("SELECT * FROM entries WHERE src_norm = ?")
        .bind(normalize_term(&entry.src))
        .fetch_one(&ing_pool)
        .await
        .map_err(|error| error.to_string())?;
    let view = entry_from_row(&row);
    let glossary = metadata_glossary(&ing_pool, Path::new(&glossary.ing_path)).await?;
    upsert_glossary_index(pool, &glossary).await?;
    ing_pool.close().await;
    Ok(view)
}

pub async fn update_glossary_entry(
    pool: &SqlitePool,
    input: UpdateGlossaryEntryInput,
) -> Result<GlossaryEntryView, String> {
    let glossary = get_glossary_from_index(pool, &input.glossary_id).await?;
    ensure_glossary_usable(&glossary)?;
    let entry = normalize_entry(&input.src, &input.dst)?;
    let now = unix_timestamp();
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    sqlx::query(
        r#"UPDATE entries
           SET src = ?, dst = ?, src_norm = ?, src_sort_key = ?, dst_sort_key = ?, updated_at = ?
           WHERE id = ?"#,
    )
    .bind(&entry.src)
    .bind(&entry.dst)
    .bind(normalize_term(&entry.src))
    .bind(sort_key(&entry.src))
    .bind(sort_key(&entry.dst))
    .bind(&now)
    .bind(&input.entry_id)
    .execute(&ing_pool)
    .await
    .map_err(|error| entry_error(error))?;
    refresh_metadata_count(&ing_pool, &now).await?;
    let row = sqlx::query("SELECT * FROM entries WHERE id = ?")
        .bind(&input.entry_id)
        .fetch_one(&ing_pool)
        .await
        .map_err(|error| error.to_string())?;
    let view = entry_from_row(&row);
    let glossary = metadata_glossary(&ing_pool, Path::new(&glossary.ing_path)).await?;
    upsert_glossary_index(pool, &glossary).await?;
    ing_pool.close().await;
    Ok(view)
}

pub async fn delete_glossary_entry(
    pool: &SqlitePool,
    input: DeleteGlossaryEntryInput,
) -> Result<(), String> {
    let glossary = get_glossary_from_index(pool, &input.glossary_id).await?;
    ensure_glossary_usable(&glossary)?;
    let now = unix_timestamp();
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    sqlx::query("DELETE FROM entries WHERE id = ?")
        .bind(&input.entry_id)
        .execute(&ing_pool)
        .await
        .map_err(|error| error.to_string())?;
    refresh_metadata_count(&ing_pool, &now).await?;
    let glossary = metadata_glossary(&ing_pool, Path::new(&glossary.ing_path)).await?;
    upsert_glossary_index(pool, &glossary).await?;
    ing_pool.close().await;
    Ok(())
}

#[cfg(test)]
pub async fn create_auto_glossary(
    pool: &SqlitePool,
    workspace_root: &Path,
    input: CreateAutoGlossaryInput,
) -> Result<GlossaryView, String> {
    let name = normalize_name(&input.name)?;
    let source_language = normalize_auto_glossary_source_language(&input.source_language)?;
    let target_language = normalize_language(&input.target_language)?;
    let entries = dedupe_entries(
        input
            .entries
            .into_iter()
            .map(|entry| normalize_entry(&entry.src, &entry.dst))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let id = new_id("glossary");
    let ing_path = next_ing_path(workspace_root, &name).await?;
    let created_at = unix_timestamp();
    let ing_pool = connect_ing(&ing_path).await?;
    let mut transaction = ing_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        r#"INSERT INTO metadata (
            glossary_id, schema_version, name, source_language, target_language, tags_json,
            source_type, entry_count, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, '[]', 'auto', 0, ?, ?)"#,
    )
    .bind(&id)
    .bind(ING_SCHEMA_VERSION)
    .bind(&name)
    .bind(&source_language)
    .bind(&target_language)
    .bind(&created_at)
    .bind(&created_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    for entry in entries {
        insert_entry_ignore_query(&entry.src, &entry.dst, &created_at)
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    refresh_metadata_count(&ing_pool, &created_at).await?;
    let view = metadata_glossary(&ing_pool, &ing_path).await?;
    upsert_glossary_index(pool, &view).await?;
    ing_pool.close().await;
    Ok(view)
}

pub async fn ensure_auto_glossary_draft(
    pool: &SqlitePool,
    workspace_root: &Path,
    input: EnsureAutoGlossaryDraftInput,
) -> Result<AutoGlossaryDraft, String> {
    let name = normalize_name(&input.name)?;
    let source_language = normalize_auto_glossary_source_language(&input.source_language)?;
    let target_language = normalize_language(&input.target_language)?;
    let existing = sqlx::query("SELECT * FROM glossary_index WHERE origin_task_id = ? LIMIT 1")
        .bind(&input.origin_task_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .map(|row| glossary_from_row(&row))
        .transpose()?;
    let reserved = match existing {
        Some(view) => view,
        None => {
            let id = new_id("glossary");
            let ing_path = next_ing_path(workspace_root, &name).await?;
            let created_at = unix_timestamp();
            let view = GlossaryView {
                id,
                name: name.clone(),
                ing_path: ing_path.to_string_lossy().to_string(),
                source_language: source_language.clone(),
                target_language: target_language.clone(),
                tags: Vec::new(),
                source_type: "auto".into(),
                entry_count: 0,
                status: GlossaryStatus::Initializing,
                has_failures: false,
                origin_task_id: Some(input.origin_task_id.clone()),
                total_chunks: input.chunks.len() as i64,
                success_chunks: 0,
                failed_chunks: 0,
                interrupted_chunks: 0,
                created_at: created_at.clone(),
                updated_at: created_at,
            };
            upsert_glossary_index(pool, &view).await?;
            view
        }
    };
    let ing_path = PathBuf::from(&reserved.ing_path);
    if !ing_path.starts_with(workspace_root) {
        return Err("Refusing to initialize a glossary outside the workspace".into());
    }
    if !tokio::fs::try_exists(&ing_path)
        .await
        .map_err(|error| error.to_string())?
    {
        let temporary_path = ing_path.with_extension(format!("creating-{}", reserved.id));
        if tokio::fs::try_exists(&temporary_path)
            .await
            .map_err(|error| error.to_string())?
        {
            tokio::fs::remove_file(&temporary_path)
                .await
                .map_err(|error| error.to_string())?;
        }
        let temporary_pool = connect_ing(&temporary_path).await?;
        initialize_auto_glossary_contents(
            &temporary_pool,
            &reserved,
            &name,
            &source_language,
            &target_language,
            &input.origin_task_id,
            &input.chunks,
        )
        .await?;
        temporary_pool.close().await;
        tokio::fs::rename(&temporary_path, &ing_path)
            .await
            .map_err(|error| format!("Unable to publish automatic glossary draft: {error}"))?;
    }
    let ing_pool = connect_ing(&ing_path).await?;
    let metadata_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metadata")
        .fetch_one(&ing_pool)
        .await
        .map_err(|error| error.to_string())?;
    if metadata_count == 0 {
        initialize_auto_glossary_contents(
            &ing_pool,
            &reserved,
            &name,
            &source_language,
            &target_language,
            &input.origin_task_id,
            &input.chunks,
        )
        .await?;
    } else {
        let now = unix_timestamp();
        let mut transaction = ing_pool.begin().await.map_err(|error| error.to_string())?;
        sqlx::query(
            "UPDATE generation_chunks SET status = 'interrupted', updated_at = ?
             WHERE status = 'running'",
        )
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "UPDATE metadata SET status = CASE WHEN status IN ('building', 'interrupted')
                 THEN 'building' ELSE status END, updated_at = ?",
        )
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
    }
    refresh_generation_metadata(&ing_pool, &unix_timestamp()).await?;
    let view = metadata_glossary(&ing_pool, &ing_path).await?;
    upsert_glossary_index(pool, &view).await?;
    Ok(AutoGlossaryDraft {
        view,
        pool: ing_pool,
    })
}

async fn initialize_auto_glossary_contents(
    ing_pool: &SqlitePool,
    reserved: &GlossaryView,
    name: &str,
    source_language: &str,
    target_language: &str,
    origin_task_id: &str,
    chunks: &[AutoGlossarySourceChunk],
) -> Result<(), String> {
    let now = unix_timestamp();
    let mut transaction = ing_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO metadata (
            glossary_id, schema_version, name, source_language, target_language, tags_json,
            source_type, entry_count, status, has_failures, origin_task_id, total_chunks,
            success_chunks, failed_chunks, interrupted_chunks, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, '[]', 'auto', 0, 'building', 0, ?, ?, 0, 0, 0, ?, ?)",
    )
    .bind(&reserved.id)
    .bind(ING_SCHEMA_VERSION)
    .bind(name)
    .bind(source_language)
    .bind(target_language)
    .bind(origin_task_id)
    .bind(chunks.len() as i64)
    .bind(&now)
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    for chunk in chunks {
        sqlx::query(
            "INSERT INTO generation_chunks (
                id, sequence, glossary_source_text, display_source_text, status,
                retry_count, error_message, created_at, updated_at
             ) VALUES (?, ?, ?, ?, 'pending', 0, NULL, ?, ?)",
        )
        .bind(&chunk.id)
        .bind(chunk.sequence)
        .bind(&chunk.glossary_source_text)
        .bind(&chunk.display_source_text)
        .bind(&now)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

pub async fn pending_auto_glossary_chunks(
    draft: &AutoGlossaryDraft,
) -> Result<Vec<AutoGlossaryGenerationChunk>, String> {
    let rows = sqlx::query(
        "SELECT id, sequence, glossary_source_text, display_source_text
         FROM generation_chunks
         WHERE status IN ('pending', 'interrupted')
         ORDER BY sequence ASC",
    )
    .fetch_all(&draft.pool)
    .await
    .map_err(|error| error.to_string())?;
    rows.into_iter()
        .map(|row| {
            let glossary_source_text = row
                .get::<Option<String>, _>("glossary_source_text")
                .ok_or_else(|| "Glossary source text is missing for a pending chunk".to_string())?;
            let display_source_text = row
                .get::<Option<String>, _>("display_source_text")
                .ok_or_else(|| "Display source text is missing for a pending chunk".to_string())?;
            Ok(AutoGlossaryGenerationChunk {
                id: row.get("id"),
                sequence: row.get("sequence"),
                glossary_source_text,
                display_source_text,
            })
        })
        .collect()
}

pub async fn mark_auto_glossary_chunk_running(
    draft: &AutoGlossaryDraft,
    chunk_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE generation_chunks SET status = 'running', error_message = NULL, updated_at = ?
         WHERE id = ? AND status IN ('pending', 'interrupted')",
    )
    .bind(unix_timestamp())
    .bind(chunk_id)
    .execute(&draft.pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn update_auto_glossary_chunk_retry(
    draft: &AutoGlossaryDraft,
    chunk_id: &str,
    retry_count: i64,
    error_message: Option<&str>,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE generation_chunks SET retry_count = ?, error_message = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(retry_count)
    .bind(error_message)
    .bind(unix_timestamp())
    .bind(chunk_id)
    .execute(&draft.pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn persist_auto_glossary_chunk_success(
    config_pool: &SqlitePool,
    draft: &AutoGlossaryDraft,
    chunk_id: &str,
    entries: Vec<GlossaryEntry>,
) -> Result<GlossaryView, String> {
    let now = unix_timestamp();
    let entries = dedupe_entries(
        entries
            .into_iter()
            .map(|entry| normalize_entry(&entry.src, &entry.dst))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut transaction = draft
        .pool
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    for entry in entries {
        insert_entry_ignore_query(&entry.src, &entry.dst, &now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
    }
    sqlx::query(
        "UPDATE generation_chunks SET status = 'success', error_message = NULL, updated_at = ?
         WHERE id = ?",
    )
    .bind(&now)
    .bind(chunk_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    refresh_generation_metadata_in_transaction(&mut transaction, &now).await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    sync_glossary_index(config_pool, draft).await
}

pub async fn persist_auto_glossary_chunk_failed(
    config_pool: &SqlitePool,
    draft: &AutoGlossaryDraft,
    chunk_id: &str,
    error_message: &str,
) -> Result<GlossaryView, String> {
    let now = unix_timestamp();
    let mut transaction = draft
        .pool
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE generation_chunks SET status = 'failed', error_message = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(error_message)
    .bind(&now)
    .bind(chunk_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    refresh_generation_metadata_in_transaction(&mut transaction, &now).await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    sync_glossary_index(config_pool, draft).await
}

pub async fn mark_auto_glossary_interrupted(
    config_pool: &SqlitePool,
    draft: &AutoGlossaryDraft,
    error_message: &str,
) -> Result<GlossaryView, String> {
    let now = unix_timestamp();
    let mut transaction = draft
        .pool
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE generation_chunks SET status = 'interrupted', error_message = ?, updated_at = ?
         WHERE status = 'running'",
    )
    .bind(error_message)
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query("UPDATE metadata SET status = 'interrupted', updated_at = ?")
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    refresh_generation_metadata_in_transaction(&mut transaction, &now).await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    sync_glossary_index(config_pool, draft).await
}

pub async fn mark_auto_glossary_failed_after_runtime_error(
    config_pool: &SqlitePool,
    origin_task_id: &str,
    error_message: &str,
) -> Result<Option<GlossaryView>, String> {
    let row = sqlx::query(
        "SELECT * FROM glossary_index
         WHERE origin_task_id = ? AND source_type = 'auto'
         LIMIT 1",
    )
    .bind(origin_task_id)
    .fetch_optional(config_pool)
    .await
    .map_err(|error| error.to_string())?;
    let Some(row) = row else {
        return Ok(None);
    };
    let indexed = glossary_from_row(&row)?;
    if matches!(
        indexed.status,
        GlossaryStatus::Success | GlossaryStatus::Failed
    ) {
        return Ok(None);
    }
    let ing_path = PathBuf::from(&indexed.ing_path);
    if !tokio::fs::try_exists(&ing_path)
        .await
        .map_err(|error| error.to_string())?
    {
        return Ok(None);
    }
    let ing_pool = connect_ing(&ing_path).await?;
    let current = metadata_glossary(&ing_pool, &ing_path).await?;
    if current.origin_task_id.as_deref() != Some(origin_task_id)
        || matches!(
            current.status,
            GlossaryStatus::Success | GlossaryStatus::Failed
        )
    {
        upsert_glossary_index(config_pool, &current).await?;
        ing_pool.close().await;
        return Ok(None);
    }
    let now = unix_timestamp();
    let mut transaction = ing_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE generation_chunks
         SET status = 'failed', error_message = ?, updated_at = ?
         WHERE status = 'running'",
    )
    .bind(error_message)
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query("UPDATE metadata SET status = 'failed', updated_at = ?")
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    refresh_generation_metadata_in_transaction(&mut transaction, &now).await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    let failed = metadata_glossary(&ing_pool, &ing_path).await?;
    upsert_glossary_index(config_pool, &failed).await?;
    ing_pool.close().await;
    Ok(Some(failed))
}

pub async fn finalize_auto_glossary(
    config_pool: &SqlitePool,
    draft: &AutoGlossaryDraft,
    status: GlossaryStatus,
) -> Result<GlossaryView, String> {
    if !matches!(status, GlossaryStatus::Success | GlossaryStatus::Failed) {
        return Err("Automatic glossary can only be finalized as success or failed".into());
    }
    let now = unix_timestamp();
    let mut transaction = draft
        .pool
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    refresh_generation_metadata_in_transaction(&mut transaction, &now).await?;
    sqlx::query("UPDATE metadata SET status = ?, has_failures = failed_chunks > 0, updated_at = ?")
        .bind(status.as_str())
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    if status == GlossaryStatus::Success {
        sqlx::query(
            "UPDATE generation_chunks
             SET glossary_source_text = NULL, display_source_text = NULL
             WHERE status = 'success'",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    sync_glossary_index(config_pool, draft).await
}

async fn sync_glossary_index(
    config_pool: &SqlitePool,
    draft: &AutoGlossaryDraft,
) -> Result<GlossaryView, String> {
    let view = metadata_glossary(&draft.pool, Path::new(&draft.view.ing_path)).await?;
    upsert_glossary_index(config_pool, &view).await?;
    Ok(view)
}

async fn refresh_generation_metadata(pool: &SqlitePool, updated_at: &str) -> Result<(), String> {
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    refresh_generation_metadata_in_transaction(&mut transaction, updated_at).await?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

async fn refresh_generation_metadata_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    updated_at: &str,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE metadata SET
            entry_count = (SELECT COUNT(*) FROM entries),
            total_chunks = (SELECT COUNT(*) FROM generation_chunks),
            success_chunks = (SELECT COUNT(*) FROM generation_chunks WHERE status = 'success'),
            failed_chunks = (SELECT COUNT(*) FROM generation_chunks WHERE status = 'failed'),
            interrupted_chunks = (SELECT COUNT(*) FROM generation_chunks WHERE status = 'interrupted'),
            has_failures = (SELECT COUNT(*) FROM generation_chunks WHERE status = 'failed') > 0,
            updated_at = ?",
    )
    .bind(updated_at)
    .execute(&mut **transaction)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn load_glossary_entries(
    pool: &SqlitePool,
    glossary_id: &str,
) -> Result<Vec<GlossaryEntry>, String> {
    let glossary = get_glossary_from_index(pool, glossary_id).await?;
    ensure_glossary_usable(&glossary)?;
    let ing_pool = connect_ing(Path::new(&glossary.ing_path)).await?;
    let rows = sqlx::query("SELECT src, dst FROM entries ORDER BY created_at ASC, id ASC")
        .fetch_all(&ing_pool)
        .await
        .map_err(|error| error.to_string())?;
    let entries = rows
        .into_iter()
        .map(|row| GlossaryEntry {
            src: row.get("src"),
            dst: row.get("dst"),
        })
        .collect();
    ing_pool.close().await;
    Ok(entries)
}

async fn refresh_metadata_count(pool: &SqlitePool, updated_at: &str) -> Result<(), String> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entries")
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query("UPDATE metadata SET entry_count = ?, updated_at = ?")
        .bind(count)
        .bind(updated_at)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn insert_entry_query<'a>(
    src: &'a str,
    dst: &'a str,
    created_at: &'a str,
) -> sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>> {
    sqlx::query(
        r#"INSERT INTO entries (
            id, src, dst, src_norm, src_sort_key, dst_sort_key, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(new_id("entry"))
    .bind(src)
    .bind(dst)
    .bind(normalize_term(src))
    .bind(sort_key(src))
    .bind(sort_key(dst))
    .bind(created_at)
    .bind(created_at)
}

fn insert_entry_ignore_query<'a>(
    src: &'a str,
    dst: &'a str,
    created_at: &'a str,
) -> sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>> {
    sqlx::query(
        r#"INSERT OR IGNORE INTO entries (
            id, src, dst, src_norm, src_sort_key, dst_sort_key, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(new_id("entry"))
    .bind(src)
    .bind(dst)
    .bind(normalize_term(src))
    .bind(sort_key(src))
    .bind(sort_key(dst))
    .bind(created_at)
    .bind(created_at)
}

async fn upsert_glossary_index(pool: &SqlitePool, glossary: &GlossaryView) -> Result<(), String> {
    let tags_json = serialize_tags(&glossary.tags)?;
    sqlx::query(
        r#"INSERT INTO glossary_index (
            id, name, ing_path, source_language, target_language, tags_json, source_type,
            entry_count, status, has_failures, origin_task_id, total_chunks, success_chunks,
            failed_chunks, interrupted_chunks, name_sort_key, tags_sort_key, language_sort_key,
            created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            ing_path = excluded.ing_path,
            source_language = excluded.source_language,
            target_language = excluded.target_language,
            tags_json = excluded.tags_json,
            source_type = excluded.source_type,
            entry_count = excluded.entry_count,
            status = excluded.status,
            has_failures = excluded.has_failures,
            origin_task_id = excluded.origin_task_id,
            total_chunks = excluded.total_chunks,
            success_chunks = excluded.success_chunks,
            failed_chunks = excluded.failed_chunks,
            interrupted_chunks = excluded.interrupted_chunks,
            name_sort_key = excluded.name_sort_key,
            tags_sort_key = excluded.tags_sort_key,
            language_sort_key = excluded.language_sort_key,
            updated_at = excluded.updated_at"#,
    )
    .bind(&glossary.id)
    .bind(&glossary.name)
    .bind(&glossary.ing_path)
    .bind(&glossary.source_language)
    .bind(&glossary.target_language)
    .bind(tags_json)
    .bind(&glossary.source_type)
    .bind(glossary.entry_count)
    .bind(glossary.status.as_str())
    .bind(glossary.has_failures)
    .bind(glossary.origin_task_id.as_deref())
    .bind(glossary.total_chunks)
    .bind(glossary.success_chunks)
    .bind(glossary.failed_chunks)
    .bind(glossary.interrupted_chunks)
    .bind(sort_key(&glossary.name))
    .bind(sort_key(&glossary.tags.join(" ")))
    .bind(sort_key(&format!(
        "{} {}",
        glossary.source_language, glossary.target_language
    )))
    .bind(&glossary.created_at)
    .bind(&glossary.updated_at)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn get_glossary_from_index(pool: &SqlitePool, id: &str) -> Result<GlossaryView, String> {
    let row = sqlx::query("SELECT * FROM glossary_index WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Glossary not found".to_string())?;
    glossary_from_row(&row)
}

fn ensure_glossary_usable(glossary: &GlossaryView) -> Result<(), String> {
    if glossary.status != GlossaryStatus::Success {
        return Err("Glossary is still being generated and is read-only".into());
    }
    Ok(())
}

async fn metadata_glossary(pool: &SqlitePool, ing_path: &Path) -> Result<GlossaryView, String> {
    let row = sqlx::query("SELECT * FROM metadata LIMIT 1")
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(GlossaryView {
        id: row.get("glossary_id"),
        name: row.get("name"),
        ing_path: ing_path.to_string_lossy().to_string(),
        source_language: row.get("source_language"),
        target_language: row.get("target_language"),
        tags: parse_tags_json(row.get("tags_json"))?,
        source_type: row.get("source_type"),
        entry_count: row.get("entry_count"),
        status: GlossaryStatus::parse(row.get::<String, _>("status").as_str())?,
        has_failures: row.get::<i64, _>("has_failures") != 0,
        origin_task_id: row.get("origin_task_id"),
        total_chunks: row.get("total_chunks"),
        success_chunks: row.get("success_chunks"),
        failed_chunks: row.get("failed_chunks"),
        interrupted_chunks: row.get("interrupted_chunks"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn glossary_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<GlossaryView, String> {
    Ok(GlossaryView {
        id: row.get("id"),
        name: row.get("name"),
        ing_path: row.get("ing_path"),
        source_language: row.get("source_language"),
        target_language: row.get("target_language"),
        tags: parse_tags_json(row.get("tags_json"))?,
        source_type: row.get("source_type"),
        entry_count: row.get("entry_count"),
        status: GlossaryStatus::parse(row.get::<String, _>("status").as_str())?,
        has_failures: row.get::<i64, _>("has_failures") != 0,
        origin_task_id: row.get("origin_task_id"),
        total_chunks: row.get("total_chunks"),
        success_chunks: row.get("success_chunks"),
        failed_chunks: row.get("failed_chunks"),
        interrupted_chunks: row.get("interrupted_chunks"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn entry_from_row(row: &sqlx::sqlite::SqliteRow) -> GlossaryEntryView {
    GlossaryEntryView {
        id: row.get("id"),
        src: row.get("src"),
        dst: row.get("dst"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn sort_glossary_views(glossaries: &mut [GlossaryView], sort: Option<GlossarySortInput>) {
    let Some(sort) = sort else {
        glossaries.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        return;
    };
    match sort.mode {
        SortMode::CreatedDesc => {
            glossaries.sort_by(|left, right| right.created_at.cmp(&left.created_at))
        }
        SortMode::CreatedAsc => {
            glossaries.sort_by(|left, right| left.created_at.cmp(&right.created_at))
        }
        SortMode::Az => glossaries.sort_by(|left, right| {
            let left_key = match sort.field {
                GlossarySortField::Name => sort_key(&left.name),
                GlossarySortField::Tags => sort_key(&left.tags.join(" ")),
                GlossarySortField::Language => sort_key(&format!(
                    "{} {}",
                    left.source_language, left.target_language
                )),
            };
            let right_key = match sort.field {
                GlossarySortField::Name => sort_key(&right.name),
                GlossarySortField::Tags => sort_key(&right.tags.join(" ")),
                GlossarySortField::Language => sort_key(&format!(
                    "{} {}",
                    right.source_language, right.target_language
                )),
            };
            left_key.cmp(&right_key)
        }),
    }
}

fn entry_order_by(sort: Option<GlossaryEntrySortInput>) -> &'static str {
    match sort {
        Some(GlossaryEntrySortInput {
            mode: SortMode::CreatedAsc,
            ..
        }) => "created_at ASC, id ASC",
        Some(GlossaryEntrySortInput {
            mode: SortMode::Az,
            field: GlossaryEntrySortField::Src,
        }) => "src_sort_key ASC, src ASC",
        Some(GlossaryEntrySortInput {
            mode: SortMode::Az,
            field: GlossaryEntrySortField::Dst,
        }) => "dst_sort_key ASC, dst ASC",
        _ => "created_at DESC, id DESC",
    }
}

fn normalize_list_query(query: Option<GlossaryListQuery>) -> Result<GlossaryListQuery, String> {
    let mut query = query.unwrap_or_default();
    query.search = normalize_optional_filter(query.search);
    query.tag = normalize_optional_filter(query.tag);
    query.source_language = normalize_optional_filter(query.source_language);
    query.target_language = normalize_optional_filter(query.target_language);
    if let Some(tag) = query.tag.as_deref() {
        validate_tag(tag)?;
    }
    if let Some(source_language) = query.source_language.as_deref() {
        query.source_language = Some(normalize_source_language(source_language)?);
    }
    if let Some(target_language) = query.target_language.as_deref() {
        query.target_language = Some(normalize_target_language(target_language)?);
    }
    Ok(query)
}

fn same_language(left: &str, right: &str) -> bool {
    match (
        normalize_language_code(left),
        normalize_language_code(right),
    ) {
        (Some(left_code), Some(right_code)) => left_code == right_code,
        _ => left.trim().eq_ignore_ascii_case(right.trim()),
    }
}

fn normalize_optional_filter(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn normalize_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("术语表名称不能为空".into());
    }
    if name.chars().count() > MAX_NAME_LENGTH || name.chars().any(char::is_control) {
        return Err("术语表名称格式不正确".into());
    }
    Ok(name.to_string())
}

fn normalize_language(value: &str) -> Result<String, String> {
    let language = value.trim();
    if language.eq_ignore_ascii_case("auto") {
        return Err("语言格式不正确".into());
    }
    normalize_target_language(language).map_err(|_| "语言格式不正确".into())
}

fn normalize_glossary_source_language(value: &str) -> Result<String, String> {
    let language = normalize_source_language(value).map_err(|_| "语言格式不正确".to_string())?;
    if language == "auto" {
        return Err("语言格式不正确".into());
    }
    Ok(language)
}

fn normalize_auto_glossary_source_language(value: &str) -> Result<String, String> {
    normalize_source_language(value).map_err(|_| "Invalid glossary source language".to_string())
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
    if normalized.len() > MAX_TAGS {
        return Err(format!("术语表标签最多支持 {MAX_TAGS} 个"));
    }
    Ok(normalized)
}

fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.chars().count() > MAX_TAG_LENGTH || tag.chars().any(char::is_control) {
        return Err("标签格式不正确".into());
    }
    Ok(())
}

fn serialize_tags(tags: &[String]) -> Result<String, String> {
    serde_json::to_string(tags).map_err(|error| error.to_string())
}

fn parse_tags_json(tags_json: String) -> Result<Vec<String>, String> {
    let tags = serde_json::from_str::<Vec<String>>(&tags_json)
        .map_err(|error| format!("Stored glossary tags are invalid: {error}"))?;
    normalize_tags(tags)
}

fn parse_csv_entries(content: &str) -> Result<Vec<NormalizedEntry>, String> {
    let rows = parse_csv_rows(content)?;
    let Some(header) = rows.first() else {
        return Err("文件格式不正确：CSV 不能为空".into());
    };
    if header.len() != 2 {
        return Err("文件格式不正确：CSV 只能包含 src 和 dst 两列".into());
    }
    let src_index = header
        .iter()
        .position(|value| value == "src")
        .ok_or_else(|| "文件格式不正确：CSV 缺少 src 列".to_string())?;
    let dst_index = header
        .iter()
        .position(|value| value == "dst")
        .ok_or_else(|| "文件格式不正确：CSV 缺少 dst 列".to_string())?;
    if src_index == dst_index {
        return Err("文件格式不正确：CSV 列名重复".into());
    }
    let mut entries = Vec::new();
    for row in rows.into_iter().skip(1) {
        if row.iter().all(|value| value.trim().is_empty()) {
            continue;
        }
        if row.len() != 2 {
            return Err("文件格式不正确：CSV 只能包含 src 和 dst 两列".into());
        }
        entries.push(normalize_entry(&row[src_index], &row[dst_index])?);
    }
    validate_entries(entries)
}

fn parse_csv_rows(content: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = content.chars().peekable();
    let mut in_quotes = false;
    while let Some(character) = chars.next() {
        match character {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                row.push(std::mem::take(&mut field));
            }
            '\n' if !in_quotes => {
                row.push(trim_cr(std::mem::take(&mut field)));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(character),
        }
    }
    if in_quotes {
        return Err("文件格式不正确：CSV 引号未闭合".into());
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(trim_cr(field));
        rows.push(row);
    }
    Ok(rows)
}

fn trim_cr(value: String) -> String {
    value.strip_suffix('\r').unwrap_or(&value).to_string()
}

fn parse_json_entries(content: &str) -> Result<Vec<NormalizedEntry>, String> {
    let parsed = sanitize_and_flatten_glossary(content, None)
        .map_err(|error| format!("Invalid glossary JSON: {error}"))?;
    let entries = parsed
        .entries
        .into_iter()
        .map(|entry| normalize_entry(&entry.src, &entry.dst))
        .collect::<Result<Vec<_>, _>>()?;
    validate_entries(entries)
}

fn normalize_entry(src: &str, dst: &str) -> Result<NormalizedEntry, String> {
    let src = src.trim();
    let dst = dst.trim();
    if src.is_empty() || dst.is_empty() {
        return Err("文件格式不正确：src 和 dst 不能为空".into());
    }
    if src.chars().any(char::is_control) || dst.chars().any(char::is_control) {
        return Err("文件格式不正确：src 和 dst 不能包含控制字符".into());
    }
    Ok(NormalizedEntry {
        src: src.to_string(),
        dst: dst.to_string(),
    })
}

fn validate_entries(entries: Vec<NormalizedEntry>) -> Result<Vec<NormalizedEntry>, String> {
    if entries.is_empty() {
        return Err("Glossary cannot be empty".into());
    }
    let deduped = dedupe_entries(entries);
    if deduped.is_empty() {
        return Err("Glossary cannot be empty".into());
    }
    Ok(deduped)
}

fn dedupe_entries(entries: Vec<NormalizedEntry>) -> Vec<NormalizedEntry> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for entry in entries {
        if seen.insert(normalize_term(&entry.src)) {
            deduped.push(entry);
        }
    }
    deduped
}

#[allow(dead_code)]
fn validate_entries_strict_unused(
    entries: Vec<NormalizedEntry>,
) -> Result<Vec<NormalizedEntry>, String> {
    if entries.is_empty() {
        return Err("文件格式不正确：术语表不能为空".into());
    }
    let mut seen = HashSet::new();
    for entry in &entries {
        if !seen.insert(normalize_term(&entry.src)) {
            return Err("文件格式不正确：存在重复 src 术语".into());
        }
    }
    Ok(entries)
}

fn normalize_term(value: &str) -> String {
    value.trim().to_lowercase()
}

fn sort_key(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .fold(String::new(), |mut output, character| {
            if character.is_ascii_alphanumeric() || character.is_whitespace() {
                output.push(character);
            } else if let Some(romanized) = romanize_char(character) {
                output.push_str(romanized);
                output.push(' ');
            } else {
                output.push(character);
            }
            output
        })
}

fn romanize_char(character: char) -> Option<&'static str> {
    match character {
        '一' => Some("yi"),
        '丁' => Some("ding"),
        '七' => Some("qi"),
        '万' | '萬' => Some("wan"),
        '中' => Some("zhong"),
        '主' => Some("zhu"),
        '义' | '義' => Some("yi"),
        '之' => Some("zhi"),
        '书' | '書' => Some("shu"),
        '云' => Some("yun"),
        '互' => Some("hu"),
        '亚' | '亞' => Some("ya"),
        '人' => Some("ren"),
        '介' => Some("jie"),
        '以' => Some("yi"),
        '件' => Some("jian"),
        '价' | '價' => Some("jia"),
        '任' => Some("ren"),
        '传' | '傳' => Some("chuan"),
        '体' | '體' => Some("ti"),
        '作' => Some("zuo"),
        '例' => Some("li"),
        '保' => Some("bao"),
        '信' => Some("xin"),
        '值' => Some("zhi"),
        '元' => Some("yuan"),
        '入' => Some("ru"),
        '全' => Some("quan"),
        '公' => Some("gong"),
        '关' | '關' => Some("guan"),
        '具' => Some("ju"),
        '内' | '內' => Some("nei"),
        '写' | '寫' => Some("xie"),
        '出' => Some("chu"),
        '分' => Some("fen"),
        '列' => Some("lie"),
        '制' => Some("zhi"),
        '前' => Some("qian"),
        '加' => Some("jia"),
        '动' | '動' => Some("dong"),
        '包' => Some("bao"),
        '化' => Some("hua"),
        '区' | '區' => Some("qu"),
        '单' | '單' => Some("dan"),
        '原' => Some("yuan"),
        '参' | '參' => Some("can"),
        '发' | '發' => Some("fa"),
        '取' => Some("qu"),
        '变' | '變' => Some("bian"),
        '口' => Some("kou"),
        '可' => Some("ke"),
        '号' | '號' => Some("hao"),
        '名' => Some("ming"),
        '后' | '後' => Some("hou"),
        '否' => Some("fou"),
        '启' | '啟' => Some("qi"),
        '和' => Some("he"),
        '品' => Some("pin"),
        '响' | '響' => Some("xiang"),
        '器' => Some("qi"),
        '回' => Some("hui"),
        '图' | '圖' => Some("tu"),
        '在' => Some("zai"),
        '地' => Some("di"),
        '址' => Some("zhi"),
        '型' => Some("xing"),
        '增' => Some("zeng"),
        '处' | '處' => Some("chu"),
        '备' | '備' => Some("bei"),
        '外' => Some("wai"),
        '多' => Some("duo"),
        '大' => Some("da"),
        '失' => Some("shi"),
        '始' => Some("shi"),
        '存' => Some("cun"),
        '学' | '學' => Some("xue"),
        '定' => Some("ding"),
        '实' | '實' => Some("shi"),
        '客' => Some("ke"),
        '导' | '導' => Some("dao"),
        '对' | '對' => Some("dui"),
        '将' | '將' => Some("jiang"),
        '小' => Some("xiao"),
        '层' | '層' => Some("ceng"),
        '工' => Some("gong"),
        '已' => Some("yi"),
        '常' => Some("chang"),
        '并' | '並' => Some("bing"),
        '应' | '應' => Some("ying"),
        '开' | '開' => Some("kai"),
        '式' => Some("shi"),
        '引' => Some("yin"),
        '建' => Some("jian"),
        '录' | '錄' => Some("lu"),
        '态' | '態' => Some("tai"),
        '总' | '總' => Some("zong"),
        '息' => Some("xi"),
        '成' => Some("cheng"),
        '户' | '戶' => Some("hu"),
        '所' => Some("suo"),
        '手' => Some("shou"),
        '打' => Some("da"),
        '执' | '執' => Some("zhi"),
        '扩' | '擴' => Some("kuo"),
        '择' | '擇' => Some("ze"),
        '按' => Some("an"),
        '换' | '換' => Some("huan"),
        '排' => Some("pai"),
        '控' => Some("kong"),
        '提' => Some("ti"),
        '搜' => Some("sou"),
        '改' => Some("gai"),
        '数' | '數' => Some("shu"),
        '文' => Some("wen"),
        '新' => Some("xin"),
        '方' => Some("fang"),
        '时' | '時' => Some("shi"),
        '是' => Some("shi"),
        '显' | '顯' => Some("xian"),
        '更' => Some("geng"),
        '替' => Some("ti"),
        '有' => Some("you"),
        '本' => Some("ben"),
        '机' | '機' => Some("ji"),
        '条' | '條' => Some("tiao"),
        '来' | '來' => Some("lai"),
        '标' | '標' => Some("biao"),
        '格' => Some("ge"),
        '检' | '檢' => Some("jian"),
        '模' => Some("mo"),
        '次' => Some("ci"),
        '正' => Some("zheng"),
        '步' => Some("bu"),
        '每' => Some("mei"),
        '求' => Some("qiu"),
        '法' => Some("fa"),
        '注' => Some("zhu"),
        '源' => Some("yuan"),
        '点' | '點' => Some("dian"),
        '然' => Some("ran"),
        '版' => Some("ban"),
        '用' => Some("yong"),
        '由' => Some("you"),
        '界' => Some("jie"),
        '的' => Some("de"),
        '目' => Some("mu"),
        '看' => Some("kan"),
        '知' => Some("zhi"),
        '确' | '確' => Some("que"),
        '示' => Some("shi"),
        '禁' => Some("jin"),
        '种' | '種' => Some("zhong"),
        '称' | '稱' => Some("cheng"),
        '空' => Some("kong"),
        '符' => Some("fu"),
        '第' => Some("di"),
        '等' => Some("deng"),
        '签' | '簽' => Some("qian"),
        '简' | '簡' => Some("jian"),
        '索' => Some("suo"),
        '组' | '組' => Some("zu"),
        '结' | '結' => Some("jie"),
        '给' | '給' => Some("gei"),
        '维' | '維' => Some("wei"),
        '编' | '編' => Some("bian"),
        '置' => Some("zhi"),
        '翻' => Some("fan"),
        '者' => Some("zhe"),
        '联' | '聯' => Some("lian"),
        '能' => Some("neng"),
        '自' => Some("zi"),
        '英' => Some("ying"),
        '获' | '獲' => Some("huo"),
        '行' => Some("xing"),
        '表' => Some("biao"),
        '要' => Some("yao"),
        '规' | '規' => Some("gui"),
        '览' | '覽' => Some("lan"),
        '言' => Some("yan"),
        '讯' | '訊' => Some("xun"),
        '设' | '設' => Some("she"),
        '识' | '識' => Some("shi"),
        '译' | '譯' => Some("yi"),
        '语' | '語' => Some("yu"),
        '请' | '請' => Some("qing"),
        '读' | '讀' => Some("du"),
        '调' | '調' => Some("tiao"),
        '输' | '輸' => Some("shu"),
        '过' | '過' => Some("guo"),
        '返' => Some("fan"),
        '选' | '選' => Some("xuan"),
        '通' => Some("tong"),
        '速' => Some("su"),
        '递' | '遞' => Some("di"),
        '道' => Some("dao"),
        '部' => Some("bu"),
        '配' => Some("pei"),
        '重' => Some("zhong"),
        '钮' | '鈕' => Some("niu"),
        '错' | '錯' => Some("cuo"),
        '键' | '鍵' => Some("jian"),
        '闭' | '閉' => Some("bi"),
        '间' | '間' => Some("jian"),
        '问' | '問' => Some("wen"),
        '阅' | '閱' => Some("yue"),
        '队' | '隊' => Some("dui"),
        '限' => Some("xian"),
        '除' => Some("chu"),
        '需' => Some("xu"),
        '项' | '項' => Some("xiang"),
        '页' | '頁' => Some("ye"),
        '额' | '額' => Some("e"),
        '香' => Some("xiang"),
        '가'..='깋' => Some("ga"),
        '나'..='닣' => Some("na"),
        '다'..='딯' => Some("da"),
        '라'..='맇' => Some("ra"),
        '마'..='밓' => Some("ma"),
        '바'..='빟' => Some("ba"),
        '사'..='싷' => Some("sa"),
        '아'..='잏' => Some("a"),
        '자'..='짛' => Some("ja"),
        '차'..='칳' => Some("cha"),
        '카'..='킿' => Some("ka"),
        '타'..='팋' => Some("ta"),
        '파'..='핗' => Some("pa"),
        '하'..='힣' => Some("ha"),
        _ => romanize_kana(character),
    }
}

fn romanize_kana(character: char) -> Option<&'static str> {
    match character {
        'あ' | 'ア' => Some("a"),
        'い' | 'イ' => Some("i"),
        'う' | 'ウ' => Some("u"),
        'え' | 'エ' => Some("e"),
        'お' | 'オ' => Some("o"),
        'か' | 'カ' | 'が' | 'ガ' => Some("ka"),
        'き' | 'キ' | 'ぎ' | 'ギ' => Some("ki"),
        'く' | 'ク' | 'ぐ' | 'グ' => Some("ku"),
        'け' | 'ケ' | 'げ' | 'ゲ' => Some("ke"),
        'こ' | 'コ' | 'ご' | 'ゴ' => Some("ko"),
        'さ' | 'サ' | 'ざ' | 'ザ' => Some("sa"),
        'し' | 'シ' | 'じ' | 'ジ' => Some("shi"),
        'す' | 'ス' | 'ず' | 'ズ' => Some("su"),
        'せ' | 'セ' | 'ぜ' | 'ゼ' => Some("se"),
        'そ' | 'ソ' | 'ぞ' | 'ゾ' => Some("so"),
        'た' | 'タ' | 'だ' | 'ダ' => Some("ta"),
        'ち' | 'チ' | 'ぢ' | 'ヂ' => Some("chi"),
        'つ' | 'ツ' | 'づ' | 'ヅ' => Some("tsu"),
        'て' | 'テ' | 'で' | 'デ' => Some("te"),
        'と' | 'ト' | 'ど' | 'ド' => Some("to"),
        'な' | 'ナ' => Some("na"),
        'に' | 'ニ' => Some("ni"),
        'ぬ' | 'ヌ' => Some("nu"),
        'ね' | 'ネ' => Some("ne"),
        'の' | 'ノ' => Some("no"),
        'は' | 'ハ' | 'ば' | 'バ' | 'ぱ' | 'パ' => Some("ha"),
        'ひ' | 'ヒ' | 'び' | 'ビ' | 'ぴ' | 'ピ' => Some("hi"),
        'ふ' | 'フ' | 'ぶ' | 'ブ' | 'ぷ' | 'プ' => Some("fu"),
        'へ' | 'ヘ' | 'べ' | 'ベ' | 'ぺ' | 'ペ' => Some("he"),
        'ほ' | 'ホ' | 'ぼ' | 'ボ' | 'ぽ' | 'ポ' => Some("ho"),
        'ま' | 'マ' => Some("ma"),
        'み' | 'ミ' => Some("mi"),
        'む' | 'ム' => Some("mu"),
        'め' | 'メ' => Some("me"),
        'も' | 'モ' => Some("mo"),
        'や' | 'ヤ' => Some("ya"),
        'ゆ' | 'ユ' => Some("yu"),
        'よ' | 'ヨ' => Some("yo"),
        'ら' | 'ラ' => Some("ra"),
        'り' | 'リ' => Some("ri"),
        'る' | 'ル' => Some("ru"),
        'れ' | 'レ' => Some("re"),
        'ろ' | 'ロ' => Some("ro"),
        'わ' | 'ワ' => Some("wa"),
        'を' | 'ヲ' => Some("wo"),
        'ん' | 'ン' => Some("n"),
        _ => None,
    }
}

fn export_csv(rows: &[sqlx::sqlite::SqliteRow]) -> String {
    let mut output = String::from("src,dst\n");
    for row in rows {
        let src: String = row.get("src");
        let dst: String = row.get("dst");
        output.push_str(&csv_escape(&src));
        output.push(',');
        output.push_str(&csv_escape(&dst));
        output.push('\n');
    }
    output
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn export_json(rows: &[sqlx::sqlite::SqliteRow]) -> Result<String, String> {
    let values = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "src": row.get::<String, _>("src"),
                "dst": row.get::<String, _>("dst"),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&values).map_err(|error| error.to_string())
}

fn entry_error(error: sqlx::Error) -> String {
    let text = error.to_string();
    if text.contains("UNIQUE") || text.contains("unique") {
        "术语 src 已存在".into()
    } else {
        text
    }
}

async fn next_ing_path(workspace_root: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = workspace_root.join(GLOSSARIES_DIR);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|error| error.to_string())?;
    let base = sanitize_file_stem(name);
    for index in 0..10_000 {
        let filename = if index == 0 {
            format!("{base}.ing")
        } else {
            format!("{base}-{index:02}.ing")
        };
        let candidate = dir.join(filename);
        if tokio::fs::try_exists(&candidate)
            .await
            .map_err(|error| error.to_string())?
        {
            continue;
        }
        return Ok(candidate);
    }
    Err("Unable to allocate a unique ING file name".into())
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
        "glossary".into()
    } else {
        sanitized
    }
}

fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{nanos:x}{counter:x}")
}

fn unix_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("insitu-glossaries-{label}-{}", new_id("test")));
        std::fs::create_dir_all(&root).expect("create temp workspace");
        root
    }

    #[test]
    fn parses_valid_csv_and_rejects_extra_columns() {
        let entries = parse_csv_entries("dst,src\n苹果,Apple\n香蕉,Banana\n").expect("csv");
        assert_eq!(entries[0].src, "Apple");
        assert_eq!(entries[0].dst, "苹果");
        assert!(parse_csv_entries("src,dst,note\nApple,苹果,x\n").is_err());
    }

    #[test]
    fn parses_valid_json_with_legacy_fields_and_nested_arrays() {
        let entries =
            parse_json_entries("```json\n[[{\"source\":\"Apple\",\"target\":\"Pingguo\"}]]\n```")
                .expect("json entries");
        assert_eq!(entries[0].src, "Apple");
        assert_eq!(entries[0].dst, "Pingguo");
        assert!(parse_json_entries(r#"[{"src":"Apple","dst":"Pingguo"}]"#).is_ok());
        assert!(parse_json_entries(r#"[{"src":"Apple","dst":}]"#).is_err());
    }

    #[test]
    fn dedupes_duplicate_sources() {
        let entries = parse_json_entries(
            r#"[{"src":"Apple","dst":"Pingguo"},{"src":" apple ","dst":"Pingguo2"}]"#,
        )
        .expect("deduped entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].src, "Apple");
        assert_eq!(entries[0].dst, "Pingguo");
    }

    #[tokio::test]
    async fn imports_json_through_shared_sanitizer_and_refreshes_count() {
        let root = temp_workspace("json-import");
        let pool = connect_config_db(&root).await.expect("config db");
        let source = root.join("terms.json");
        tokio::fs::write(
            &source,
            r#"Here is the glossary:
```json
[
  [{"source":"Apple","target":"Pingguo"}],
  {"src":" apple ","dst":"Ignored"},
  {"src":"Banana","dst":"Xiangjiao"}
]
```"#,
        )
        .await
        .expect("write input");

        let view = import_glossary(
            &pool,
            &root,
            ImportGlossaryInput {
                file_path: source.to_string_lossy().to_string(),
                name: "JSON Terms".into(),
                source_language: "en".into(),
                target_language: "zh-CN".into(),
                tags: vec!["Book".into()],
            },
        )
        .await
        .expect("import glossary");

        assert_eq!(view.source_type, "uploaded");
        assert_eq!(view.entry_count, 2);
        let page = get_glossary_entries(
            &pool,
            GlossaryEntriesQuery {
                id: view.id.clone(),
                page: 0,
                page_size: 10,
                search: None,
                sort: Some(GlossaryEntrySortInput {
                    field: GlossaryEntrySortField::Src,
                    mode: SortMode::CreatedAsc,
                }),
            },
        )
        .await
        .expect("entries");
        assert_eq!(page.total, 2);
        assert_eq!(page.entries[0].src, "Apple");
        assert_eq!(page.entries[0].dst, "Pingguo");
        assert_eq!(page.entries[1].src, "Banana");

        pool.close().await;
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn imports_csv_with_first_duplicate_preserved() {
        let root = temp_workspace("csv-import");
        let pool = connect_config_db(&root).await.expect("config db");
        let source = root.join("terms.csv");
        tokio::fs::write(
            &source,
            "src,dst\nApple,Pingguo\n apple ,Ignored\nBanana,Xiangjiao\n",
        )
        .await
        .expect("write input");

        let view = import_glossary(
            &pool,
            &root,
            ImportGlossaryInput {
                file_path: source.to_string_lossy().to_string(),
                name: "CSV Terms".into(),
                source_language: "English".into(),
                target_language: "Simplified Chinese".into(),
                tags: Vec::new(),
            },
        )
        .await
        .expect("import glossary");

        assert_eq!(view.entry_count, 2);
        let entries = load_glossary_entries(&pool, &view.id)
            .await
            .expect("load entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].src, "Apple");
        assert_eq!(entries[0].dst, "Pingguo");

        pool.close().await;
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn creates_auto_glossary_with_empty_tags_and_actual_count() {
        let root = temp_workspace("auto-create");
        let pool = connect_config_db(&root).await.expect("config db");

        let view = create_auto_glossary(
            &pool,
            &root,
            CreateAutoGlossaryInput {
                name: "Task Auto Glossary".into(),
                source_language: "auto".into(),
                target_language: "zh-CN".into(),
                entries: vec![
                    GlossaryEntry {
                        src: "Apple".into(),
                        dst: "Pingguo".into(),
                    },
                    GlossaryEntry {
                        src: " apple ".into(),
                        dst: "Ignored".into(),
                    },
                    GlossaryEntry {
                        src: "Animation".into(),
                        dst: "Donghua".into(),
                    },
                ],
            },
        )
        .await
        .expect("create auto glossary");

        assert_eq!(view.source_type, "auto");
        assert!(view.tags.is_empty());
        assert_eq!(view.source_language, "auto");
        assert_eq!(view.entry_count, 2);

        let entries = load_glossary_entries(&pool, &view.id)
            .await
            .expect("load entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].src, "Apple");
        assert_eq!(entries[0].dst, "Pingguo");

        pool.close().await;
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn auto_glossary_draft_persists_chunks_filters_usage_and_clears_success_text() {
        let root = temp_workspace("auto-draft");
        let pool = connect_config_db(&root).await.expect("config db");
        let input = EnsureAutoGlossaryDraftInput {
            name: "Draft Glossary".into(),
            source_language: "en".into(),
            target_language: "zh-CN".into(),
            origin_task_id: "task-draft".into(),
            chunks: vec![
                AutoGlossarySourceChunk {
                    id: "chunk-1".into(),
                    sequence: 0,
                    glossary_source_text: "<t1>Apple</t1>".into(),
                    display_source_text: "**Apple**".into(),
                },
                AutoGlossarySourceChunk {
                    id: "chunk-2".into(),
                    sequence: 1,
                    glossary_source_text: "Banana".into(),
                    display_source_text: "Banana".into(),
                },
            ],
        };
        let draft = ensure_auto_glossary_draft(&pool, &root, input.clone())
            .await
            .expect("draft");
        assert_eq!(draft.view.status, GlossaryStatus::Building);
        assert_eq!(draft.view.total_chunks, 2);
        let same = ensure_auto_glossary_draft(&pool, &root, input)
            .await
            .expect("same draft");
        assert_eq!(same.view.id, draft.view.id);

        let all = list_glossaries(&pool, None).await.expect("all glossaries");
        assert_eq!(all.len(), 1);
        let usable = list_glossaries(
            &pool,
            Some(GlossaryListQuery {
                usable_only: true,
                ..GlossaryListQuery::default()
            }),
        )
        .await
        .expect("usable glossaries");
        assert!(usable.is_empty());

        persist_auto_glossary_chunk_success(
            &pool,
            &draft,
            "chunk-1",
            vec![GlossaryEntry {
                src: "Apple".into(),
                dst: "苹果".into(),
            }],
        )
        .await
        .expect("persist success");
        persist_auto_glossary_chunk_failed(&pool, &draft, "chunk-2", "request failed")
            .await
            .expect("persist failure");
        let finalized = finalize_auto_glossary(&pool, &draft, GlossaryStatus::Success)
            .await
            .expect("finalize");
        assert_eq!(finalized.status, GlossaryStatus::Success);
        assert!(finalized.has_failures);
        assert_eq!(finalized.entry_count, 1);

        let success_text: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT glossary_source_text, display_source_text FROM generation_chunks WHERE id = 'chunk-1'",
        )
        .fetch_one(&draft.pool)
        .await
        .expect("success text");
        assert_eq!(success_text, (None, None));
        let failed_text: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT glossary_source_text, display_source_text FROM generation_chunks WHERE id = 'chunk-2'",
        )
        .fetch_one(&draft.pool)
        .await
        .expect("failed text");
        assert_eq!(failed_text.0.as_deref(), Some("Banana"));
        assert_eq!(failed_text.1.as_deref(), Some("Banana"));

        let failed_page = get_glossary_failed_chunks(
            &pool,
            GlossaryFailedChunksQuery {
                id: finalized.id.clone(),
                page: 0,
                page_size: 10,
            },
        )
        .await
        .expect("failed chunks");
        assert_eq!(failed_page.total, 1);
        assert_eq!(failed_page.chunks[0].display_source_text, "Banana");
        let usable = list_glossaries(
            &pool,
            Some(GlossaryListQuery {
                usable_only: true,
                ..GlossaryListQuery::default()
            }),
        )
        .await
        .expect("usable after success");
        assert_eq!(usable.len(), 1);

        draft.pool.close().await;
        same.pool.close().await;
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn opening_legacy_ing_adds_status_columns_and_generation_table() {
        let root = temp_workspace("legacy-ing-migration");
        let ing_path = root.join("legacy.ing");
        let legacy_pool = connect_sqlite(&ing_path, 1).await.expect("legacy pool");
        sqlx::query(
            "CREATE TABLE metadata (
                glossary_id TEXT PRIMARY KEY NOT NULL,
                schema_version INTEGER NOT NULL,
                name TEXT NOT NULL,
                source_language TEXT NOT NULL,
                target_language TEXT NOT NULL,
                tags_json TEXT NOT NULL DEFAULT '[]',
                source_type TEXT NOT NULL DEFAULT 'uploaded',
                entry_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             )",
        )
        .execute(&legacy_pool)
        .await
        .expect("legacy metadata");
        sqlx::query(
            "INSERT INTO metadata VALUES ('legacy', 1, 'Legacy', 'en', 'zh-CN', '[]', 'uploaded', 0, 'now', 'now')",
        )
        .execute(&legacy_pool)
        .await
        .expect("legacy row");
        legacy_pool.close().await;

        let migrated = connect_ing(&ing_path).await.expect("migrated ing");
        let status: String = sqlx::query_scalar("SELECT status FROM metadata LIMIT 1")
            .fetch_one(&migrated)
            .await
            .expect("status");
        assert_eq!(status, "success");
        let schema_version: i64 = sqlx::query_scalar("SELECT schema_version FROM metadata LIMIT 1")
            .fetch_one(&migrated)
            .await
            .expect("schema version");
        assert_eq!(schema_version, ING_SCHEMA_VERSION);
        let generation_table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'generation_chunks'",
        )
        .fetch_one(&migrated)
        .await
        .expect("generation table");
        assert_eq!(generation_table, 1);

        migrated.close().await;
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn startup_recovery_marks_building_draft_and_running_chunks_interrupted() {
        let root = temp_workspace("draft-recovery");
        let pool = connect_config_db(&root).await.expect("config db");
        let draft = ensure_auto_glossary_draft(
            &pool,
            &root,
            EnsureAutoGlossaryDraftInput {
                name: "Recover Draft".into(),
                source_language: "en".into(),
                target_language: "zh-CN".into(),
                origin_task_id: "task-recover".into(),
                chunks: vec![AutoGlossarySourceChunk {
                    id: "chunk-running".into(),
                    sequence: 0,
                    glossary_source_text: "Apple".into(),
                    display_source_text: "Apple".into(),
                }],
            },
        )
        .await
        .expect("draft");
        mark_auto_glossary_chunk_running(&draft, "chunk-running")
            .await
            .expect("mark running");

        recover_auto_glossary_drafts(&pool)
            .await
            .expect("recover drafts");
        let recovered = get_glossary(&pool, &draft.view.id)
            .await
            .expect("recovered glossary");
        assert_eq!(recovered.status, GlossaryStatus::Interrupted);
        assert_eq!(recovered.interrupted_chunks, 1);
        let chunk_status: String =
            sqlx::query_scalar("SELECT status FROM generation_chunks WHERE id = 'chunk-running'")
                .fetch_one(&draft.pool)
                .await
                .expect("chunk status");
        assert_eq!(chunk_status, "interrupted");

        draft.pool.close().await;
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn scheduler_runtime_error_fails_only_the_active_auto_glossary_draft() {
        let root = temp_workspace("scheduler-runtime-error");
        let pool = connect_config_db(&root).await.expect("config db");
        let draft = ensure_auto_glossary_draft(
            &pool,
            &root,
            EnsureAutoGlossaryDraftInput {
                name: "Runtime Error Draft".into(),
                source_language: "en".into(),
                target_language: "zh-CN".into(),
                origin_task_id: "task-runtime-error".into(),
                chunks: vec![
                    AutoGlossarySourceChunk {
                        id: "chunk-running".into(),
                        sequence: 0,
                        glossary_source_text: "Apple".into(),
                        display_source_text: "Apple".into(),
                    },
                    AutoGlossarySourceChunk {
                        id: "chunk-pending".into(),
                        sequence: 1,
                        glossary_source_text: "Banana".into(),
                        display_source_text: "Banana".into(),
                    },
                ],
            },
        )
        .await
        .expect("draft");
        mark_auto_glossary_chunk_running(&draft, "chunk-running")
            .await
            .expect("mark running");

        let failed = mark_auto_glossary_failed_after_runtime_error(
            &pool,
            "task-runtime-error",
            "worker panicked",
        )
        .await
        .expect("mark runtime failure")
        .expect("changed glossary");
        assert_eq!(failed.status, GlossaryStatus::Failed);
        assert_eq!(failed.failed_chunks, 1);
        let statuses = sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT id, status, error_message FROM generation_chunks ORDER BY sequence",
        )
        .fetch_all(&draft.pool)
        .await
        .expect("chunk statuses");
        assert_eq!(
            statuses,
            vec![
                (
                    "chunk-running".into(),
                    "failed".into(),
                    Some("worker panicked".into()),
                ),
                ("chunk-pending".into(), "pending".into(), None),
            ]
        );

        let successful = ensure_auto_glossary_draft(
            &pool,
            &root,
            EnsureAutoGlossaryDraftInput {
                name: "Successful Glossary".into(),
                source_language: "en".into(),
                target_language: "zh-CN".into(),
                origin_task_id: "task-translation-error".into(),
                chunks: vec![AutoGlossarySourceChunk {
                    id: "chunk-success".into(),
                    sequence: 0,
                    glossary_source_text: "Orange".into(),
                    display_source_text: "Orange".into(),
                }],
            },
        )
        .await
        .expect("successful draft");
        persist_auto_glossary_chunk_success(
            &pool,
            &successful,
            "chunk-success",
            vec![GlossaryEntry {
                src: "Orange".into(),
                dst: "橙子".into(),
            }],
        )
        .await
        .expect("persist success");
        finalize_auto_glossary(&pool, &successful, GlossaryStatus::Success)
            .await
            .expect("finalize success");
        let unchanged = mark_auto_glossary_failed_after_runtime_error(
            &pool,
            "task-translation-error",
            "translation worker panicked",
        )
        .await
        .expect("preserve successful glossary");
        assert!(unchanged.is_none());
        assert_eq!(
            get_glossary(&pool, &successful.view.id)
                .await
                .expect("successful glossary")
                .status,
            GlossaryStatus::Success
        );

        draft.pool.close().await;
        successful.pool.close().await;
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[test]
    fn normalizes_glossary_languages_to_supported_codes() {
        assert_eq!(
            normalize_glossary_source_language(" English ").unwrap(),
            "en"
        );
        assert_eq!(normalize_language("Simplified Chinese").unwrap(), "zh-CN");
        assert_eq!(
            normalize_language("Chinese (Traditional)").unwrap(),
            "zh-HK"
        );
        assert!(normalize_glossary_source_language("auto").is_err());
        assert!(normalize_language("Klingon").is_err());
    }

    #[test]
    fn compares_legacy_and_code_language_values() {
        assert!(same_language("English", "en"));
        assert!(same_language("Traditional Chinese", "zh-HK"));
        assert!(same_language("zh-TW", "Chinese (Traditional)"));
        assert!(!same_language("English", "ko"));
    }
}
