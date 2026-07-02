use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use futures_util::{stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, Sqlite, SqlitePool};
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
use crate::domain::{ProviderPurpose, UnifiedChatRequest, UnifiedToolChoice};
use crate::glossaries::{self, CreateAutoGlossaryInput, GlossaryView, PrepareAutoGlossaryInput};
use crate::glossary_prompt::{
    build_glossary_prompt, sanitize_and_flatten_glossary, GlossaryEntry, GlossaryPromptBuildResult,
    GlossaryPromptInput,
};
use crate::languages::{
    normalize_source_language, normalize_target_language, DEFAULT_SOURCE_LANGUAGE,
    DEFAULT_TARGET_LANGUAGE,
};
use crate::pdf_parsing::{self, PdfAsset, PdfParsingMode};
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
#[serde(rename_all = "kebab-case")]
pub enum ContextHandlingMode {
    Off,
    #[serde(alias = "sliding-window")]
    SlidingWindowTarget,
    SlidingWindowSource,
    GlobalBackground,
}

impl Default for ContextHandlingMode {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GlossaryMode {
    Auto,
    Existing,
}

impl GlossaryMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Existing => "existing",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "existing" => Ok(Self::Existing),
            _ => Err(format!("Unsupported glossary mode: {value}")),
        }
    }
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
    pub context_handling_mode: ContextHandlingMode,
    #[serde(default, skip_serializing)]
    pub use_global_background: bool,
    pub use_glossary: bool,
    pub glossary_mode: GlossaryMode,
    pub glossary_id: Option<String>,
    pub confidence_mode: ConfidenceMode,
    pub pdf_parsing_mode: PdfParsingMode,
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
            context_handling_mode: ContextHandlingMode::Off,
            use_global_background: false,
            use_glossary: false,
            glossary_mode: GlossaryMode::Auto,
            glossary_id: None,
            confidence_mode: ConfidenceMode::Off,
            pdf_parsing_mode: PdfParsingMode::LocalFirst,
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
    #[serde(default)]
    pub context_handling_mode: ContextHandlingMode,
    #[serde(default, skip_serializing)]
    pub use_global_background: bool,
    pub use_glossary: bool,
    pub glossary_mode: GlossaryMode,
    pub glossary_id: Option<String>,
    #[serde(default)]
    pub confidence_mode: ConfidenceMode,
    #[serde(default)]
    pub pdf_parsing_mode: PdfParsingMode,
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
    sequence: i64,
    source_text: String,
    map_json: String,
}

#[derive(Debug, Clone)]
struct TaskGlossaryConfig {
    use_glossary: bool,
    glossary_mode: GlossaryMode,
    glossary_id: Option<String>,
}

#[derive(Clone)]
struct GlossaryRuntime {
    adapter: Arc<RuntimeAdapter>,
    model_request_name: String,
    assistant_prompt: Option<String>,
    assistant_custom_parameters: Value,
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

enum TaskGlossaryPreparation {
    Ready(Vec<GlossaryEntry>),
    Interrupted,
}

#[derive(Debug, Clone)]
struct TaskGlossaryMatcher {
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
    fn new(entries: Vec<GlossaryEntry>) -> Result<Self, String> {
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

    fn match_entries(&self, chunk_text: &str) -> Vec<GlossaryEntry> {
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
    Interrupted(String),
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

#[derive(Debug, Clone)]
struct ParsedTaskSource {
    chunks: Vec<document_parsing::types::ParsedChunk>,
    assets: Vec<PdfAsset>,
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

struct MaterializedSourceFile {
    root_dir: PathBuf,
    path: PathBuf,
}

impl MaterializedSourceFile {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MaterializedSourceFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root_dir);
    }
}

enum ResolvedSourceFile {
    Embedded(MaterializedSourceFile),
    Original(PathBuf),
}

impl ResolvedSourceFile {
    fn path(&self) -> &Path {
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

async fn resolve_source_file(
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
    let root_dir = std::env::temp_dir().join(format!("insitu-source-{}", db::new_id("src")));
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

fn effective_translation_concurrency(config: &TranslationConfigView) -> usize {
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

async fn parse_source_file_for_task(
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

async fn insert_assets(
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
    let mut transaction = inp_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO metadata (
            task_id, schema_version, name, source_path, source_language, target_language, status,
            progress, provider_id, model_id, model_request_name, assistant_id, assistant_system_prompt,
            assistant_custom_parameters_json, use_glossary, glossary_mode, glossary_id, tags_json,
            token_limit, max_concurrency, max_retries, config_snapshot_json, global_background,
            total_chunks,
            created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

async fn rendered_task_document(
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

async fn release_assets_for_export(inp_pool: &SqlitePool, save_path: &Path) -> Result<(), String> {
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
    provider_pool: &SqlitePool,
    client: &Client,
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
            rebuild_chunks_for_retranslate(
                provider_pool,
                client,
                &inp_pool,
                &indexed,
                config.chunk_token_limit,
                config.pdf_parsing_mode,
                &now,
            )
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
    let config = get_translation_config(config_pool).await?;
    let interrupt = TranslationInterrupt::new();
    let result = generate_auto_glossary(
        provider_pool,
        glossary_config_pool,
        glossary_workspace_root,
        client,
        &task,
        &chunks,
        &config,
        &interrupt,
    )
    .await?;
    match result {
        AutoGlossaryGeneration::Created(view) => {
            set_task_glossary_id(&inp_pool, &view.id).await?;
            let refreshed = refresh_task_stats(&inp_pool, config_pool, &inp_path, None).await?;
            let _ = app.emit(
                TRANSLATION_PROGRESS_EVENT,
                TranslationProgressPayload { task: refreshed },
            );
            inp_pool.close().await;
            Ok(Some(view))
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

async fn rebuild_chunks_for_retranslate(
    provider_pool: &SqlitePool,
    client: &Client,
    inp_pool: &SqlitePool,
    indexed: &TranslationTaskView,
    token_limit: i64,
    pdf_parsing_mode: PdfParsingMode,
    now: &str,
) -> Result<(), String> {
    let source_path = PathBuf::from(&indexed.source_path);
    let resolved_source = resolve_source_file(inp_pool, &source_path).await?;
    let parsed_source = parse_source_file_for_task(
        provider_pool,
        client,
        &indexed.id,
        resolved_source.path(),
        token_limit,
        pdf_parsing_mode,
    )
    .await?;
    let mut transaction = inp_pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query("DELETE FROM chunks")
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query("DELETE FROM assets")
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    insert_assets(&mut transaction, &parsed_source.assets, now).await?;
    for chunk in parsed_source.chunks {
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
    sqlx::query("UPDATE metadata SET global_background = NULL")
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
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
    glossary_config_pool: SqlitePool,
    glossary_workspace_root: PathBuf,
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

    let global_background = ensure_task_global_background(
        &inp_pool,
        prepared.config.context_handling_mode == ContextHandlingMode::GlobalBackground,
    )
    .await?;
    let glossary_chunks = glossary_source_chunks(&inp_pool).await?;
    let glossary_matcher = match prepare_task_glossary(
        &app,
        &provider_pool,
        &glossary_config_pool,
        &glossary_workspace_root,
        &client,
        &inp_pool,
        &config_pool,
        &prepared.inp_path,
        &task,
        &glossary_chunks,
        &prepared.config,
        &interrupt,
    )
    .await?
    {
        TaskGlossaryPreparation::Ready(entries) => Arc::new(TaskGlossaryMatcher::new(entries)?),
        TaskGlossaryPreparation::Interrupted => {
            inp_pool.close().await;
            return Ok(());
        }
    };

    let model = db::get_model(&provider_pool, &task.model_id).await?;
    let config = db::runtime_config(&provider_pool, &task.provider_id).await?;
    let adapter = Arc::new(RuntimeAdapter::new(client, config));
    let assistant_prompt = task_assistant_prompt(&inp_pool).await?;
    let assistant_custom_parameters = task_assistant_custom_parameters(&inp_pool).await?;
    let dynamic_rate_limit = prepared.config.rate_limit_strategy == RateLimitStrategy::Dynamic;
    let context_handling_mode = prepared.config.context_handling_mode;
    let effective_max_concurrency = effective_translation_concurrency(&prepared.config);
    let limiter = Arc::new(AdaptiveLimiter::new(
        effective_max_concurrency,
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
    let max_concurrency = effective_max_concurrency;
    let max_retries = prepared.config.max_retries.max(0) as u32;
    let confidence_mode = prepared.config.confidence_mode;
    let target_language = task.target_language.clone();
    let document_format = document_format_from_source_path(&task.source_path)?;
    let content_format = content_format_from_source_path(&task.source_path)?;

    if context_handling_mode == ContextHandlingMode::SlidingWindowTarget {
        run_sliding_window_translation(
            &app,
            &inp_pool,
            &config_pool,
            &prepared.inp_path,
            adapter,
            model.request_name.clone(),
            target_language,
            assistant_prompt,
            assistant_custom_parameters,
            global_background,
            glossary_matcher,
            document_format,
            content_format,
            pending_chunks,
            max_retries,
            confidence_mode,
            quota,
            limiter,
            manual_limiter,
            &interrupt,
        )
        .await?;
        inp_pool.close().await;
        return Ok(());
    }

    let (tx, rx) = mpsc::channel::<ChunkOutcome>(max_concurrency * 2 + 1);
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
            let glossary_matcher = glossary_matcher.clone();
            let global_background = global_background.clone();
            let inp_pool = inp_pool.clone();
            async move {
                if interrupted.is_interrupted() {
                    return;
                }
                let previous_context =
                    if context_handling_mode == ContextHandlingMode::SlidingWindowSource {
                        match previous_source_context(&inp_pool, chunk.sequence).await {
                            Ok(context) => context,
                            Err(error) => {
                                let outcome = failed_outcome(
                                    chunk,
                                    TranslationChunkStatus::Failed,
                                    0,
                                    Some(error),
                                    None,
                                    TokenStats::default(),
                                    None,
                                    false,
                                );
                                let _ = tx.send(outcome).await;
                                return;
                            }
                        }
                    } else {
                        None
                    };
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
                    global_background,
                    previous_context,
                    glossary_matcher,
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

async fn run_sliding_window_translation(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    adapter: Arc<RuntimeAdapter>,
    model_request_name: String,
    target_language: String,
    assistant_prompt: Option<String>,
    assistant_custom_parameters: Value,
    global_background: Option<String>,
    glossary_matcher: Arc<TaskGlossaryMatcher>,
    document_format: DocumentFormat,
    content_format: ContentFormat,
    pending_chunks: Vec<ChunkRecord>,
    max_retries: u32,
    confidence_mode: ConfidenceMode,
    quota: Arc<HeaderQuotaPolicy>,
    limiter: Arc<AdaptiveLimiter>,
    manual_limiter: Option<Arc<ManualRateLimiter>>,
    interrupt: &TranslationInterrupt,
) -> Result<(), String> {
    for chunk in pending_chunks {
        if interrupt.is_interrupted() {
            break;
        }
        let previous_context = previous_translation_context(inp_pool, chunk.sequence).await?;
        let Some(_permit) = limiter.acquire(&interrupt.flag).await else {
            break;
        };
        if interrupt.is_interrupted() {
            break;
        }
        let outcome = translate_chunk(
            adapter.clone(),
            model_request_name.clone(),
            target_language.clone(),
            assistant_prompt.clone(),
            assistant_custom_parameters.clone(),
            global_background.clone(),
            previous_context,
            glossary_matcher.clone(),
            document_format,
            content_format,
            chunk,
            max_retries,
            confidence_mode,
            quota.clone(),
            limiter.clone(),
            manual_limiter.clone(),
        )
        .await;
        let interrupt_task = outcome.interrupt_task;
        apply_and_emit_chunk_outcome(app, inp_pool, config_pool, inp_path, outcome).await?;
        if interrupt_task {
            interrupt.interrupt("Rate limit reached; task interrupted");
            limiter.notify_waiters();
        }
    }
    finalize_translation_run(app, inp_pool, config_pool, inp_path, interrupt).await
}

async fn prepare_task_glossary(
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
                    provider_pool,
                    glossary_config_pool,
                    glossary_workspace_root,
                    client,
                    task,
                    pending_chunks,
                    config,
                    interrupt,
                )
                .await?
                {
                    AutoGlossaryGeneration::Created(view) => {
                        set_task_glossary_id(inp_pool, &view.id).await?;
                        let refreshed =
                            refresh_task_stats(inp_pool, config_pool, inp_path, None).await?;
                        let _ = app.emit(
                            TRANSLATION_PROGRESS_EVENT,
                            TranslationProgressPayload { task: refreshed },
                        );
                        view.id
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
    provider_pool: &SqlitePool,
    glossary_config_pool: &SqlitePool,
    glossary_workspace_root: &Path,
    client: &Client,
    task: &TranslationTaskView,
    pending_chunks: &[ChunkRecord],
    config: &TranslationConfigView,
    interrupt: &TranslationInterrupt,
) -> Result<AutoGlossaryGeneration, String> {
    let glossary_runtime = select_glossary_runtime(provider_pool, client).await?;
    let chunks = pending_chunks
        .iter()
        .filter(|chunk| !chunk.source_text.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if chunks.is_empty() {
        let view = glossaries::create_auto_glossary(
            glossary_config_pool,
            glossary_workspace_root,
            CreateAutoGlossaryInput {
                name: format!("{} 自动术语表", task.name),
                source_language: task.source_language.clone(),
                target_language: task.target_language.clone(),
                entries: Vec::new(),
            },
        )
        .await?;
        return Ok(AutoGlossaryGeneration::Created(view));
    }

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
    let document_format = document_format_from_source_path(&task.source_path)?;
    let content_format = content_format_from_source_path(&task.source_path)?;
    let runtime = Arc::new(glossary_runtime);

    let mut outcomes = stream::iter(chunks.clone())
        .map(|chunk| {
            let runtime = runtime.clone();
            let limiter = limiter.clone();
            let quota = quota.clone();
            let manual_limiter = manual_limiter.clone();
            let target_language = target_language.clone();
            let interrupted = interrupt.clone();
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
                )
                .await
            }
        })
        .buffer_unordered(max_concurrency)
        .collect::<Vec<_>>()
        .await;
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
                if failed_chunks as f64 / chunks.len() as f64 > AUTO_GLOSSARY_FAILURE_THRESHOLD {
                    return Ok(AutoGlossaryGeneration::Interrupted(format!(
                        "Auto glossary generation failed for more than 40% of chunks: {error}"
                    )));
                }
            }
            AutoGlossaryChunkOutcome::Interrupted { error } => {
                return Ok(AutoGlossaryGeneration::Interrupted(error));
            }
        }
    }

    let view = glossaries::create_auto_glossary(
        glossary_config_pool,
        glossary_workspace_root,
        CreateAutoGlossaryInput {
            name: format!("{} 自动术语表", task.name),
            source_language: task.source_language.clone(),
            target_language: task.target_language.clone(),
            entries,
        },
    )
    .await?;
    Ok(AutoGlossaryGeneration::Created(view))
}

async fn select_glossary_runtime(
    provider_pool: &SqlitePool,
    client: &Client,
) -> Result<GlossaryRuntime, String> {
    let provider = db::list_providers(provider_pool, Some(ProviderPurpose::Glossary))
        .await?
        .into_iter()
        .find(|provider| provider.enabled)
        .ok_or_else(|| "No enabled glossary provider is configured".to_string())?;
    let model = provider
        .models
        .first()
        .ok_or_else(|| "The selected glossary provider has no model".to_string())?;
    let assistant = db::list_assistants(provider_pool, ProviderPurpose::Glossary)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "No glossary assistant is configured".to_string())?;
    let config = db::runtime_config(provider_pool, &provider.id).await?;
    Ok(GlossaryRuntime {
        adapter: Arc::new(RuntimeAdapter::new(client.clone(), config)),
        model_request_name: model.request_name.clone(),
        assistant_prompt: Some(assistant.system_prompt),
        assistant_custom_parameters: assistant.custom_parameters,
    })
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
        let Some(_permit) = limiter.acquire(&interrupted.flag).await else {
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
            tools: Vec::new(),
            tool_choice: UnifiedToolChoice::None,
            thinking: None,
            max_output_tokens: None,
            temperature: Some(0.0),
            stream: false,
            logprobs: false,
            custom_parameters: runtime.assistant_custom_parameters.clone(),
        };
        let estimated_tokens = estimate_tokens(&chunk.source_text) + 512;
        if let Some(manual_limiter) = manual_limiter.as_ref() {
            manual_limiter.before_request(estimated_tokens).await;
        }
        quota.before_request(estimated_tokens).await;
        match runtime.adapter.send_chat_with_meta(&request).await {
            Ok(meta) => {
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
                    Err(error) => last_error = Some(error),
                }
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
                if error.is_rate_limited() {
                    return AutoGlossaryChunkOutcome::Interrupted {
                        error: error.to_string(),
                    };
                }
                last_error = Some(error.to_string());
            }
        }
        if attempt < max_retries {
            tokio::time::sleep(Duration::from_millis(300 * (attempt as u64 + 1))).await;
        }
    }
    AutoGlossaryChunkOutcome::Failed {
        sequence: chunk.sequence,
        error: last_error.unwrap_or_else(|| "Auto glossary generation failed".to_string()),
    }
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
        apply_and_emit_chunk_outcome(&app, &inp_pool, &config_pool, &inp_path, outcome).await?;
    }
    finalize_translation_run(&app, &inp_pool, &config_pool, &inp_path, &interrupted).await
}

async fn apply_and_emit_chunk_outcome(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    outcome: ChunkOutcome,
) -> Result<(), String> {
    apply_chunk_outcome(inp_pool, outcome).await?;
    let task = refresh_task_stats(inp_pool, config_pool, inp_path, None).await?;
    let _ = app.emit(
        TRANSLATION_PROGRESS_EVENT,
        TranslationProgressPayload { task },
    );
    Ok(())
}

async fn finalize_translation_run(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    interrupted: &TranslationInterrupt,
) -> Result<(), String> {
    let stats = aggregate_chunk_stats(inp_pool).await?;
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
    global_background: Option<String>,
    previous_context: Option<String>,
    glossary_matcher: Arc<TaskGlossaryMatcher>,
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
            global_background: global_background.clone(),
            previous_context: previous_context.clone(),
            glossary: glossary_matcher.match_entries(&chunk.source_text),
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
        let estimated_tokens = estimate_tokens(&chunk.source_text)
            + global_background
                .as_deref()
                .map(estimate_tokens)
                .unwrap_or(0)
            + previous_context
                .as_deref()
                .map(estimate_tokens)
                .unwrap_or(0)
            + 256;
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
            sequence: row.get("sequence"),
            source_text: row.get("source_text"),
            map_json: row.get("map_json"),
        })
        .collect())
}

async fn glossary_source_chunks(pool: &SqlitePool) -> Result<Vec<ChunkRecord>, String> {
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

async fn previous_translation_context(
    pool: &SqlitePool,
    current_sequence: i64,
) -> Result<Option<String>, String> {
    if current_sequence <= 0 {
        return Ok(None);
    }
    let translated_text: Option<String> = sqlx::query_scalar(
        "SELECT translated_text
         FROM chunks
         WHERE sequence = ? AND status = ?
         LIMIT 1",
    )
    .bind(current_sequence - 1)
    .bind(TranslationChunkStatus::Success.as_str())
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(translated_text.and_then(|text| previous_context_section("Previous Translation", &text)))
}

async fn previous_source_context(
    pool: &SqlitePool,
    current_sequence: i64,
) -> Result<Option<String>, String> {
    if current_sequence <= 0 {
        return Ok(None);
    }
    let preprocessed_text: Option<String> = sqlx::query_scalar(
        "SELECT preprocessed_text
         FROM chunks
         WHERE sequence = ?
         LIMIT 1",
    )
    .bind(current_sequence - 1)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(preprocessed_text.and_then(|text| previous_context_section("Previous Source Text", &text)))
}

fn previous_context_section(title: &str, text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(format!("# {title}\n{text}"))
    }
}

fn append_background_text(background: &mut String, text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if !background.is_empty() {
        background.push_str("\n\n");
    }
    background.push_str(text);
    estimate_tokens(background) >= GLOBAL_BACKGROUND_TARGET_TOKENS
}

fn global_background_from_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> String {
    let mut background = String::new();
    for text in texts {
        if append_background_text(&mut background, text) {
            break;
        }
    }
    truncate_global_background(&background)
}

fn truncate_global_background(background: &str) -> String {
    let background = background.trim();
    if background.is_empty() {
        return String::new();
    }
    if estimate_tokens(background) <= GLOBAL_BACKGROUND_TARGET_TOKENS {
        return background.to_string();
    }

    let mut bounds = background
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    bounds.push(background.len());
    let mut low = 0_usize;
    let mut high = bounds.len().saturating_sub(1);
    while low < high {
        let mid = (low + high + 1) / 2;
        if estimate_tokens(&background[..bounds[mid]]) <= GLOBAL_BACKGROUND_TARGET_TOKENS {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    background[..bounds[low]].trim_end().to_string()
}

async fn generate_global_background(pool: &SqlitePool) -> Result<String, String> {
    let mut background = String::new();
    let mut cursor = -1_i64;
    loop {
        let rows = sqlx::query(
            "SELECT sequence, source_text
             FROM chunks
             WHERE sequence > ?
             ORDER BY sequence
             LIMIT ?",
        )
        .bind(cursor)
        .bind(GLOBAL_BACKGROUND_BATCH_CHUNKS)
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        if rows.is_empty() {
            break;
        }

        let row_count = rows.len();
        for row in rows {
            cursor = row.get("sequence");
            let source_text: String = row.get("source_text");
            if append_background_text(&mut background, &source_text) {
                return Ok(truncate_global_background(&background));
            }
        }
        if row_count < GLOBAL_BACKGROUND_BATCH_CHUNKS as usize {
            break;
        }
    }
    Ok(truncate_global_background(&background))
}

async fn task_global_background(pool: &SqlitePool) -> Result<Option<String>, String> {
    let background: Option<String> =
        sqlx::query_scalar("SELECT global_background FROM metadata LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
    Ok(background)
}

async fn write_task_global_background(pool: &SqlitePool, background: &str) -> Result<(), String> {
    sqlx::query(
        "UPDATE metadata
         SET global_background = ?, updated_at = ?
         WHERE task_id = (SELECT task_id FROM metadata LIMIT 1)",
    )
    .bind(background)
    .bind(unix_timestamp())
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn ensure_task_global_background(
    pool: &SqlitePool,
    enabled: bool,
) -> Result<Option<String>, String> {
    if !enabled {
        return Ok(None);
    }
    if let Some(background) = task_global_background(pool).await? {
        return if background.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(background))
        };
    }

    let background = generate_global_background(pool).await?;
    write_task_global_background(pool, &background).await?;
    if background.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(background))
    }
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

async fn task_glossary_config(pool: &SqlitePool) -> Result<TaskGlossaryConfig, String> {
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

async fn set_task_glossary_id(pool: &SqlitePool, glossary_id: &str) -> Result<(), String> {
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
        "contextHandlingMode": config.context_handling_mode,
        "useGlossary": config.use_glossary,
        "glossaryMode": config.glossary_mode,
        "glossaryId": config.glossary_id,
        "confidenceMode": config.confidence_mode,
        "pdfParsingMode": config.pdf_parsing_mode,
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

fn is_ascii_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn spans_overlap(left_start: usize, left_end: usize, right_start: usize, right_end: usize) -> bool {
    left_start < right_end && right_start < left_end
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
    document_parsing::count_tokens(text) as u64
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
    use crate::document_parsing::types::{BlockRef, PlaceholderMap};
    use crate::domain::{AddModelInput, CreateProviderInput, ProviderProtocol};
    use std::io::{Cursor, Read, Write};
    use std::path::{Path, PathBuf};
    use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("insitu-test-{label}-{}", db::new_id("workspace")))
    }

    fn test_docx_bytes(body_xml: &str) -> Result<Vec<u8>, String> {
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body_xml}</w:body></w:document>"#
        );
        let entries = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
            ),
            (
                "word/_rels/document.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
            ),
            ("word/document.xml", document.as_str()),
            ("word/styles.xml", "<w:styles />"),
        ];
        let cursor = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        for (name, text) in entries {
            writer
                .start_file(name, SimpleFileOptions::default())
                .map_err(|error| error.to_string())?;
            writer
                .write_all(text.as_bytes())
                .map_err(|error| error.to_string())?;
        }
        let cursor = writer.finish().map_err(|error| error.to_string())?;
        Ok(cursor.into_inner())
    }

    fn read_zip_entry_from_bytes(bytes: &[u8], entry: &str) -> Result<String, String> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
        let mut file = archive.by_name(entry).map_err(|error| error.to_string())?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        Ok(text)
    }

    fn docx_block_map_json(block_index: usize) -> Result<String, String> {
        PlaceholderMap::empty(
            DocumentFormat::Docx,
            ContentFormat::Xml,
            BlockRef {
                kind: "docx-text-block".into(),
                path: Some("word/document.xml".into()),
                index: Some(block_index),
                pointer: None,
                prefix: String::new(),
                suffix: String::new(),
            },
        )
        .to_json()
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

    fn test_glossary_entry(src: &str, dst: &str) -> GlossaryEntry {
        GlossaryEntry {
            src: src.into(),
            dst: dst.into(),
        }
    }

    fn test_glossary_matcher(entries: Vec<GlossaryEntry>) -> TaskGlossaryMatcher {
        TaskGlossaryMatcher::new(entries).expect("glossary matcher")
    }

    #[test]
    fn glossary_matcher_only_returns_terms_in_current_chunk() {
        let matcher = test_glossary_matcher(vec![
            test_glossary_entry("Apple", "Pingguo"),
            test_glossary_entry("animation", "Donghua"),
            test_glossary_entry("banana", "Xiangjiao"),
        ]);

        let matched = matcher.match_entries("Apple studies animation.");

        assert_eq!(
            matched,
            vec![
                test_glossary_entry("Apple", "Pingguo"),
                test_glossary_entry("animation", "Donghua"),
            ]
        );
    }

    #[test]
    fn glossary_matcher_matches_ascii_case_insensitively() {
        let matcher = test_glossary_matcher(vec![test_glossary_entry("api", "API")]);

        assert_eq!(
            matcher.match_entries("The API gateway calls an Api endpoint."),
            vec![test_glossary_entry("api", "API")]
        );
    }

    #[test]
    fn glossary_matcher_enforces_ascii_word_boundaries() {
        let matcher = test_glossary_matcher(vec![test_glossary_entry("car", "车")]);

        assert!(matcher.match_entries("cartoon").is_empty());
        assert!(matcher.match_entries("race_car").is_empty());
        assert!(matcher.match_entries("car2").is_empty());
        assert_eq!(
            matcher.match_entries("car. (car)"),
            vec![test_glossary_entry("car", "车")]
        );
    }

    #[test]
    fn glossary_matcher_prefers_longest_overlapping_term() {
        let matcher = test_glossary_matcher(vec![
            test_glossary_entry("machine", "机器"),
            test_glossary_entry("machine learning", "机器学习"),
        ]);

        assert_eq!(
            matcher.match_entries("machine learning"),
            vec![test_glossary_entry("machine learning", "机器学习")]
        );
        assert_eq!(
            matcher.match_entries("machine learning uses a machine."),
            vec![
                test_glossary_entry("machine", "机器"),
                test_glossary_entry("machine learning", "机器学习"),
            ]
        );
    }

    #[test]
    fn glossary_matcher_dedupes_repeated_terms() {
        let matcher = test_glossary_matcher(vec![test_glossary_entry("Apple", "苹果")]);

        assert_eq!(
            matcher.match_entries("Apple talks to apple about APPLE."),
            vec![test_glossary_entry("Apple", "苹果")]
        );
    }

    #[test]
    fn glossary_matcher_does_not_apply_ascii_boundaries_to_cjk_terms() {
        let matcher = test_glossary_matcher(vec![test_glossary_entry("猫", "cat")]);

        assert_eq!(
            matcher.match_entries("小猫咪"),
            vec![test_glossary_entry("猫", "cat")]
        );
    }

    #[test]
    fn glossary_matcher_outputs_original_glossary_order() {
        let matcher = test_glossary_matcher(vec![
            test_glossary_entry("banana", "香蕉"),
            test_glossary_entry("Apple", "苹果"),
            test_glossary_entry("animation", "动画"),
        ]);

        assert_eq!(
            matcher.match_entries("animation follows Apple and banana."),
            vec![
                test_glossary_entry("banana", "香蕉"),
                test_glossary_entry("Apple", "苹果"),
                test_glossary_entry("animation", "动画"),
            ]
        );
    }

    #[tokio::test]
    async fn create_task_freezes_glossary_config_in_inp_metadata() {
        let root = temp_root("glossary-freeze");
        tokio::fs::create_dir_all(&root).await.expect("create root");
        let provider_db = root.join("providers.sqlite");
        let provider_pool = db::connect(&provider_db).await.expect("provider db");
        let config_pool = connect_config_db(&root).await.expect("config db");
        let provider = db::create_provider(
            &provider_pool,
            CreateProviderInput {
                name: "Freeze Provider".into(),
                protocol: ProviderProtocol::OpenaiChat,
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("provider");
        let model = db::add_model(
            &provider_pool,
            AddModelInput {
                provider_id: provider.id.clone(),
                request_name: "freeze-model".into(),
                alias: "Freeze Model".into(),
                source: "manual".into(),
            },
        )
        .await
        .expect("model");
        update_translation_config(
            &config_pool,
            UpdateTranslationConfigInput {
                source_language: "English".into(),
                custom_source_language: String::new(),
                target_language: "Simplified Chinese".into(),
                custom_target_language: String::new(),
                provider_id: provider.id.clone(),
                model_id: model.id.clone(),
                assistant_id: String::new(),
                chunk_token_limit: 800,
                max_concurrency: 3,
                max_retries: 2,
                rate_limit_strategy: RateLimitStrategy::Manual,
                max_requests_per_minute: 120,
                max_tokens_per_minute: 60_000,
                context_handling_mode: ContextHandlingMode::Off,
                use_global_background: false,
                use_glossary: true,
                glossary_mode: GlossaryMode::Existing,
                glossary_id: Some("glossary-freeze-id".into()),
                confidence_mode: ConfidenceMode::Off,
                pdf_parsing_mode: PdfParsingMode::LocalFirst,
            },
        )
        .await
        .expect("update config");
        let source_path = root.join("source.txt");
        tokio::fs::write(&source_path, "Apple animation.")
            .await
            .expect("write source");

        let task = create_translation_task(
            &provider_pool,
            &Client::new(),
            &config_pool,
            &root,
            CreateTranslationTaskInput {
                file_path: source_path.to_string_lossy().to_string(),
                source_language: "en".into(),
                target_language: "zh-CN".into(),
                tags: Vec::new(),
                provider_id: provider.id,
                model_id: model.id,
                assistant_id: None,
            },
        )
        .await
        .expect("create task");
        let inp_pool = connect_inp(Path::new(&task.inp_path)).await.expect("inp");
        let glossary_config = task_glossary_config(&inp_pool)
            .await
            .expect("glossary config");
        let snapshot_json: String =
            sqlx::query_scalar("SELECT config_snapshot_json FROM metadata LIMIT 1")
                .fetch_one(&inp_pool)
                .await
                .expect("snapshot");
        let snapshot: Value = serde_json::from_str(&snapshot_json).expect("snapshot json");

        assert!(glossary_config.use_glossary);
        assert_eq!(glossary_config.glossary_mode, GlossaryMode::Existing);
        assert_eq!(
            glossary_config.glossary_id.as_deref(),
            Some("glossary-freeze-id")
        );
        assert_eq!(snapshot["useGlossary"], true);
        assert_eq!(snapshot["contextHandlingMode"], "off");
        assert!(snapshot.get("useGlobalBackground").is_none());
        assert_eq!(snapshot["glossaryMode"], "existing");
        assert_eq!(snapshot["glossaryId"], "glossary-freeze-id");

        inp_pool.close().await;
        provider_pool.close().await;
        config_pool.close().await;
        let _ = std::fs::remove_dir_all(root);
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
    async fn inp_migration_adds_global_background_column_as_null() {
        let root = temp_root("global-background-migration");
        let inp_path = root.join("legacy-v6.inp");
        write_test_inp(&inp_path, "task-global-background-v6", "Global Background")
            .await
            .expect("write inp");
        let pool = connect_inp(&inp_path).await.expect("open inp");
        sqlx::query("ALTER TABLE metadata DROP COLUMN global_background")
            .execute(&pool)
            .await
            .expect("drop global background");
        sqlx::query("UPDATE metadata SET schema_version = 6")
            .execute(&pool)
            .await
            .expect("mark v6");
        pool.close().await;

        let migrated = connect_inp(&inp_path).await.expect("migrate");
        let schema_version: i64 = sqlx::query_scalar("SELECT schema_version FROM metadata LIMIT 1")
            .fetch_one(&migrated)
            .await
            .expect("schema version");
        let background: Option<String> =
            sqlx::query_scalar("SELECT global_background FROM metadata LIMIT 1")
                .fetch_one(&migrated)
                .await
                .expect("global background");
        assert_eq!(schema_version, INP_SCHEMA_VERSION);
        assert_eq!(background, None);
        migrated.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn global_background_empty_marker_prevents_recalculation() {
        let root = temp_root("global-background-empty");
        let inp_path = root.join("empty.inp");
        write_test_inp(&inp_path, "task-empty-background", "Empty Background")
            .await
            .expect("write inp");
        let pool = connect_inp(&inp_path).await.expect("open inp");
        sqlx::query("UPDATE chunks SET source_text = '   '")
            .execute(&pool)
            .await
            .expect("blank chunks");
        sqlx::query("UPDATE metadata SET global_background = NULL")
            .execute(&pool)
            .await
            .expect("clear background");

        let first = ensure_task_global_background(&pool, true)
            .await
            .expect("ensure empty background");
        let stored: Option<String> =
            sqlx::query_scalar("SELECT global_background FROM metadata LIMIT 1")
                .fetch_one(&pool)
                .await
                .expect("stored background");
        assert_eq!(first, None);
        assert_eq!(stored.as_deref(), Some(""));

        sqlx::query("UPDATE chunks SET source_text = 'Now has text'")
            .execute(&pool)
            .await
            .expect("change chunks");
        let second = ensure_task_global_background(&pool, true)
            .await
            .expect("ensure skipped background");
        let stored_after: Option<String> =
            sqlx::query_scalar("SELECT global_background FROM metadata LIMIT 1")
                .fetch_one(&pool)
                .await
                .expect("stored background after");
        assert_eq!(second, None);
        assert_eq!(stored_after.as_deref(), Some(""));
        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn global_background_extraction_truncates_to_target_tokens() {
        let long_text = std::iter::repeat("background-token")
            .take(2_000)
            .collect::<Vec<_>>()
            .join(" ");
        let background = global_background_from_texts([long_text.as_str()]);

        assert!(!background.is_empty());
        assert!(estimate_tokens(&background) <= GLOBAL_BACKGROUND_TARGET_TOKENS);
    }

    #[tokio::test]
    async fn previous_translation_context_only_reads_successful_previous_chunk() {
        let root = temp_root("previous-translation-context");
        let inp_path = root.join("previous.inp");
        write_test_inp(&inp_path, "task-previous-context", "Previous Context")
            .await
            .expect("write inp");
        let pool = connect_inp(&inp_path).await.expect("open inp");
        sqlx::query(
            "UPDATE chunks
             SET status = ?, translated_text = ?
             WHERE sequence = 0",
        )
        .bind(TranslationChunkStatus::Success.as_str())
        .bind("上一段译文")
        .execute(&pool)
        .await
        .expect("mark previous success");

        let context = previous_translation_context(&pool, 1)
            .await
            .expect("previous context");
        assert_eq!(
            context.as_deref(),
            Some("# Previous Translation\n上一段译文")
        );

        sqlx::query(
            "UPDATE chunks
             SET status = ?, translated_text = ?
             WHERE sequence = 0",
        )
        .bind(TranslationChunkStatus::Failed.as_str())
        .bind("失败译文")
        .execute(&pool)
        .await
        .expect("mark previous failed");
        let failed_context = previous_translation_context(&pool, 1)
            .await
            .expect("failed previous context");
        assert_eq!(failed_context, None);

        sqlx::query(
            "UPDATE chunks
             SET status = ?, translated_text = '   '
             WHERE sequence = 0",
        )
        .bind(TranslationChunkStatus::Success.as_str())
        .execute(&pool)
        .await
        .expect("blank previous");
        let blank_context = previous_translation_context(&pool, 1)
            .await
            .expect("blank previous context");
        assert_eq!(blank_context, None);

        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn previous_source_context_reads_previous_preprocessed_text() {
        let root = temp_root("previous-source-context");
        let inp_path = root.join("previous-source.inp");
        write_test_inp(
            &inp_path,
            "task-previous-source-context",
            "Previous Source Context",
        )
        .await
        .expect("write inp");
        let pool = connect_inp(&inp_path).await.expect("open inp");
        sqlx::query(
            "UPDATE chunks
             SET preprocessed_text = ?
             WHERE sequence = 0",
        )
        .bind("Alice opened the door.")
        .execute(&pool)
        .await
        .expect("write previous source");

        let context = previous_source_context(&pool, 1)
            .await
            .expect("previous source context");
        assert_eq!(
            context.as_deref(),
            Some("# Previous Source Text\nAlice opened the door.")
        );
        assert_eq!(
            previous_source_context(&pool, 0)
                .await
                .expect("first chunk context"),
            None
        );

        sqlx::query(
            "UPDATE chunks
             SET preprocessed_text = '   '
             WHERE sequence = 0",
        )
        .execute(&pool)
        .await
        .expect("blank previous source");
        let blank_context = previous_source_context(&pool, 1)
            .await
            .expect("blank source context");
        assert_eq!(blank_context, None);

        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sliding_window_target_forces_effective_concurrency_to_one() {
        let mut config = TranslationConfigView {
            max_concurrency: 12,
            ..TranslationConfigView::default()
        };
        assert_eq!(effective_translation_concurrency(&config), 12);

        config.context_handling_mode = ContextHandlingMode::SlidingWindowTarget;
        assert_eq!(effective_translation_concurrency(&config), 1);

        config.context_handling_mode = ContextHandlingMode::SlidingWindowSource;
        assert_eq!(effective_translation_concurrency(&config), 12);

        config.context_handling_mode = ContextHandlingMode::GlobalBackground;
        assert_eq!(effective_translation_concurrency(&config), 12);
    }

    #[test]
    fn context_handling_mode_accepts_legacy_sliding_window_value() {
        let legacy: ContextHandlingMode =
            serde_json::from_str("\"sliding-window\"").expect("legacy mode");
        assert_eq!(legacy, ContextHandlingMode::SlidingWindowTarget);
        let serialized =
            serde_json::to_string(&ContextHandlingMode::SlidingWindowTarget).expect("serialize");
        assert_eq!(serialized, "\"sliding-window-target\"");
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
    async fn inp_migration_creates_assets_table_and_export_releases_assets() {
        let root = temp_root("asset-export");
        let inp_path = root.join("asset.inp");
        write_test_inp(&inp_path, "task-assets", "Asset Export")
            .await
            .expect("write inp");
        let pool = connect_inp(&inp_path).await.expect("open inp");
        let columns = sqlx::query("PRAGMA table_info(assets)")
            .fetch_all(&pool)
            .await
            .expect("asset columns");
        assert!(columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "relative_path"));
        let source_columns = sqlx::query("PRAGMA table_info(source_file)")
            .fetch_all(&pool)
            .await
            .expect("source file columns");
        assert!(source_columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "bytes"));
        sqlx::query(
            "INSERT INTO assets (relative_path, media_type, bytes, source, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind("assets/task-assets/fig.png")
        .bind("image/png")
        .bind(Vec::from(&b"png"[..]))
        .bind("mineru-standard")
        .bind(unix_timestamp())
        .execute(&pool)
        .await
        .expect("insert asset");

        let export_path = root.join("translated.md");
        release_assets_for_export(&pool, &export_path)
            .await
            .expect("release assets");
        let released = tokio::fs::read(root.join("assets/task-assets/fig.png"))
            .await
            .expect("read released asset");
        assert_eq!(released, b"png");

        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn inp_migration_backfills_readable_legacy_source_file() {
        let root = temp_root("source-backfill");
        let inp_path = root.join("legacy.inp");
        let source_path = root.join("legacy.txt");
        tokio::fs::create_dir_all(&root).await.expect("create root");
        tokio::fs::write(&source_path, b"legacy source bytes")
            .await
            .expect("write legacy source");
        write_test_inp(&inp_path, "task-backfill", "Backfill")
            .await
            .expect("write inp");

        let pool = connect_inp(&inp_path)
            .await
            .expect("open inp before legacy");
        sqlx::query("UPDATE metadata SET schema_version = 4, source_path = ?")
            .bind(source_path.to_string_lossy().to_string())
            .execute(&pool)
            .await
            .expect("mark legacy");
        sqlx::query("DELETE FROM source_file")
            .execute(&pool)
            .await
            .expect("clear source file");
        pool.close().await;

        let migrated = connect_inp(&inp_path).await.expect("migrate backfill");
        let schema_version: i64 = sqlx::query_scalar("SELECT schema_version FROM metadata LIMIT 1")
            .fetch_one(&migrated)
            .await
            .expect("schema version");
        assert_eq!(schema_version, INP_SCHEMA_VERSION);
        let row = sqlx::query("SELECT file_name, bytes FROM source_file WHERE id = 1")
            .fetch_one(&migrated)
            .await
            .expect("source file row");
        assert_eq!(row.get::<String, _>("file_name"), "legacy.txt");
        assert_eq!(
            row.get::<Vec<u8>, _>("bytes"),
            Vec::from(&b"legacy source bytes"[..])
        );
        migrated.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn missing_embedded_and_original_source_errors_clearly() {
        let root = temp_root("source-missing");
        let inp_path = root.join("missing.inp");
        write_test_inp(&inp_path, "task-missing-source", "Missing Source")
            .await
            .expect("write inp");
        let pool = connect_inp(&inp_path).await.expect("open inp");
        let source_path: String = sqlx::query_scalar("SELECT source_path FROM metadata LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("source path");

        let error = rendered_task_document(&pool, Path::new(&source_path))
            .await
            .expect_err("missing source should error");
        assert_eq!(error, SOURCE_FILE_UNAVAILABLE);
        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn rendered_docx_uses_embedded_source_file_after_original_is_deleted() {
        let root = temp_root("source-docx");
        let inp_path = root.join("docx.inp");
        let source_path = root.join("source.docx");
        let source_bytes =
            test_docx_bytes(r#"<w:p><w:r><w:t>Hello</w:t></w:r></w:p>"#).expect("docx bytes");
        tokio::fs::create_dir_all(&root).await.expect("create root");
        tokio::fs::write(&source_path, &source_bytes)
            .await
            .expect("write source");
        write_test_inp(&inp_path, "task-docx-source", "Docx Source")
            .await
            .expect("write inp");

        let pool = connect_inp(&inp_path).await.expect("open inp");
        let now = unix_timestamp();
        sqlx::query("UPDATE metadata SET source_path = ?, total_chunks = 1 WHERE task_id = ?")
            .bind(source_path.to_string_lossy().to_string())
            .bind("task-docx-source")
            .execute(&pool)
            .await
            .expect("update metadata");
        sqlx::query("DELETE FROM chunks")
            .execute(&pool)
            .await
            .expect("clear chunks");
        sqlx::query(
            "INSERT OR REPLACE INTO source_file (id, file_name, bytes, created_at)
             VALUES (1, ?, ?, ?)",
        )
        .bind("source.docx")
        .bind(source_bytes)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("insert source file");
        sqlx::query(
            "INSERT INTO chunks (
                id, sequence, map_json, preprocessed_text, source_text,
                after_translate_text, translated_text, status, retry_count, updated_at
             ) VALUES (?, 0, ?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind("task-docx-source-chunk-000000")
        .bind(docx_block_map_json(0).expect("map"))
        .bind("Hello")
        .bind("Hello")
        .bind("Hola")
        .bind("Hola")
        .bind(TranslationChunkStatus::Success.as_str())
        .bind(&now)
        .execute(&pool)
        .await
        .expect("insert docx chunk");
        tokio::fs::remove_file(&source_path)
            .await
            .expect("remove original source");

        let rendered = rendered_task_document(&pool, &source_path)
            .await
            .expect("render from embedded source");
        let document_xml =
            read_zip_entry_from_bytes(&rendered, "word/document.xml").expect("document xml");
        assert!(document_xml.contains(">Hola<"));
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
        assert_eq!(config.context_handling_mode, ContextHandlingMode::Off);
        assert!(!config.use_global_background);
        assert!(!config.use_glossary);
        assert_eq!(config.glossary_mode, GlossaryMode::Auto);
        assert_eq!(config.glossary_id, None);
        assert_eq!(config.confidence_mode, ConfidenceMode::Off);
        assert_eq!(config.pdf_parsing_mode, PdfParsingMode::LocalFirst);
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
                context_handling_mode: ContextHandlingMode::GlobalBackground,
                use_global_background: false,
                use_glossary: true,
                glossary_mode: GlossaryMode::Auto,
                glossary_id: None,
                confidence_mode: ConfidenceMode::ConfidenceIndex,
                pdf_parsing_mode: PdfParsingMode::MineruFirst,
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
        assert_eq!(
            persisted.context_handling_mode,
            ContextHandlingMode::GlobalBackground
        );
        assert!(!persisted.use_global_background);
        assert!(persisted.use_glossary);
        assert_eq!(persisted.glossary_mode, GlossaryMode::Auto);
        assert_eq!(persisted.confidence_mode, ConfidenceMode::ConfidenceIndex);
        assert_eq!(persisted.pdf_parsing_mode, PdfParsingMode::MineruFirst);
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
        assert_eq!(
            final_config.context_handling_mode,
            ContextHandlingMode::GlobalBackground
        );
        assert!(!final_config.use_global_background);
        assert!(final_config.use_glossary);
        assert_eq!(final_config.glossary_mode, GlossaryMode::Auto);
        assert_eq!(
            final_config.confidence_mode,
            ConfidenceMode::ConfidenceIndex
        );
        assert_eq!(final_config.pdf_parsing_mode, PdfParsingMode::MineruFirst);
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

    #[tokio::test]
    async fn legacy_global_background_bool_maps_to_context_mode() {
        let root = std::env::temp_dir().join(format!("insitu-test-{}", db::new_id("workspace")));
        let pool = connect_config_db(&root).await.expect("connect config");
        sqlx::query("UPDATE translation_config SET config_json = ? WHERE id = 1")
            .bind(json!({"useGlobalBackground": true}).to_string())
            .execute(&pool)
            .await
            .expect("write legacy config");

        let config = get_translation_config(&pool).await.expect("read config");
        assert_eq!(
            config.context_handling_mode,
            ContextHandlingMode::GlobalBackground
        );
        assert!(!config.use_global_background);
        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn legacy_sliding_window_mode_maps_to_target_mode() {
        let root = std::env::temp_dir().join(format!("insitu-test-{}", db::new_id("workspace")));
        let pool = connect_config_db(&root).await.expect("connect config");
        sqlx::query("UPDATE translation_config SET config_json = ? WHERE id = 1")
            .bind(json!({"contextHandlingMode": "sliding-window"}).to_string())
            .execute(&pool)
            .await
            .expect("write legacy sliding config");

        let config = get_translation_config(&pool).await.expect("read config");
        assert_eq!(
            config.context_handling_mode,
            ContextHandlingMode::SlidingWindowTarget
        );
        pool.close().await;
        let _ = std::fs::remove_dir_all(root);
    }
}
