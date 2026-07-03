use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};

use crate::domain::ThinkingEffort;
use crate::languages::{DEFAULT_SOURCE_LANGUAGE, DEFAULT_TARGET_LANGUAGE};
use crate::pdf_parsing::PdfParsingMode;

use super::{
    DEFAULT_CHUNK_TOKEN_LIMIT, DEFAULT_MAX_CONCURRENCY, DEFAULT_MAX_REQUESTS_PER_MINUTE,
    DEFAULT_MAX_RETRIES, DEFAULT_MAX_TOKENS_PER_MINUTE,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TranslationTaskStatus {
    Pending,
    Running,
    InterruptedPending,
    Interrupted,
    Failed,
    Success,
}

impl TranslationTaskStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::InterruptedPending => "interrupted-pending",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::Success => "success",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "interrupted-pending" => Ok(Self::InterruptedPending),
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
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::Success => "success",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
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
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Manual => "manual",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
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
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Existing => "existing",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
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
    pub(super) fn enabled(self) -> bool {
        self == Self::ConfidenceIndex
    }
}

impl Default for ConfidenceMode {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressStep {
    pub state: String,
    pub current: u64,
    pub total: u64,
    pub percent: f64,
    pub label: String,
}

impl ProgressStep {
    pub(super) fn new(
        state: impl Into<String>,
        current: u64,
        total: u64,
        label: impl Into<String>,
    ) -> Self {
        let percent = if total == 0 {
            0.0
        } else {
            (current as f64 / total as f64).clamp(0.0, 1.0)
        };
        Self {
            state: state.into(),
            current,
            total,
            percent,
            label: label.into(),
        }
    }

    pub(super) fn pending(current: u64, total: u64, label: impl Into<String>) -> Self {
        Self::new("pending", current, total, label)
    }

    pub(super) fn running(current: u64, total: u64, label: impl Into<String>) -> Self {
        Self::new("running", current, total, label)
    }

    pub(super) fn success(current: u64, total: u64, label: impl Into<String>) -> Self {
        Self::new("success", current, total, label)
    }

    pub(super) fn failed(current: u64, total: u64, label: impl Into<String>) -> Self {
        Self::new("failed", current, total, label)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressDetail {
    pub ast: ProgressStep,
    pub chunking: ProgressStep,
    pub glossary: ProgressStep,
    pub translating: ProgressStep,
    pub restore: ProgressStep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTranslationTaskCreationResult {
    pub client_task_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TranslationTaskCreationStage {
    Ast,
    Chunking,
    Glossary,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TranslationTaskCreationStatus {
    Queued,
    Running,
    Success,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationTaskCreationProgressPayload {
    pub client_task_id: String,
    pub file_path: String,
    pub stage: TranslationTaskCreationStage,
    pub step: ProgressStep,
    pub status: TranslationTaskCreationStatus,
    pub task: Option<TranslationTaskView>,
    pub error: Option<String>,
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
    pub(super) fn add(&mut self, other: &TokenStats) {
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
    pub thinking_effort: ThinkingEffort,
    pub use_web_search: bool,
    pub use_tools: bool,
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
            thinking_effort: ThinkingEffort::None,
            use_web_search: false,
            use_tools: false,
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
    pub thinking_effort: ThinkingEffort,
    #[serde(default)]
    pub use_web_search: bool,
    #[serde(default)]
    pub use_tools: bool,
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
    pub progress_detail: Option<ProgressDetail>,
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
    pub(super) flag: Arc<AtomicBool>,
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

    pub(super) fn is_interrupted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub(super) fn reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|current| current.clone())
    }
}

#[derive(Debug, Clone)]
pub struct PreparedRun {
    pub task: TranslationTaskView,
    pub inp_path: PathBuf,
    pub(super) config: TranslationConfigView,
}

#[derive(Debug, Clone)]
pub(super) struct ChunkRecord {
    pub(super) id: String,
    pub(super) sequence: i64,
    pub(super) source_text: String,
    pub(super) map_json: String,
}

#[derive(Debug, Clone)]
pub(super) struct TaskGlossaryConfig {
    pub(super) use_glossary: bool,
    pub(super) glossary_mode: GlossaryMode,
    pub(super) glossary_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ChunkOutcome {
    pub(super) chunk_id: String,
    pub(super) status: TranslationChunkStatus,
    pub(super) interrupt_task: bool,
    pub(super) after_translate_text: String,
    pub(super) translated_text: String,
    pub(super) retry_count: i64,
    pub(super) error_message: Option<String>,
    pub(super) token_stats: TokenStats,
    pub(super) rate_limit_status: Option<String>,
    pub(super) confidence: Option<f64>,
}
