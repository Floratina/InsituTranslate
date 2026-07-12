use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{stream, StreamExt};
use reqwest::Client;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::adapters::{
    finish_reason_is_truncation, ProviderChatError, ProviderChatMeta, RateLimitTelemetry,
    RuntimeAdapter,
};
use crate::db as app_db;
use crate::diagnostics::BackendLog;
use crate::document_parsing::restore_chunk_for_map;
use crate::domain::UnifiedChatRequest;
use crate::pdf_parsing::PdfParsingMode;
use crate::task_prompt::{ContentFormat, DocumentFormat, TaskChunkInput};
use crate::translation_prompt::{
    build_translation_prompt, TranslationPromptBuildResult, TranslationPromptInput,
};

use super::context::{
    ensure_task_global_background, estimate_tokens, previous_source_context,
    previous_translation_context, unix_timestamp,
};
use super::db::{
    aggregate_chunk_stats, apply_chunk_outcome, clear_active_retry_for_chunk,
    commit_prepared_run_state, config_snapshot_json, connect_inp, content_format_from_source_path,
    document_format_from_source_path, effective_translation_concurrency, finalize_task,
    get_task_from_index, get_translation_config, glossary_source_chunks, insert_assets,
    metadata_task, parse_source_file_for_task, pending_chunks, progress_detail_for_config,
    progress_detail_for_translation_stats, publish_task_index_snapshot, refresh_task_stats,
    resolve_source_file, set_active_retry_and_emit, set_progress_detail,
    task_assistant_custom_parameters, task_assistant_prompt, task_assistant_sampling,
    task_glossary_config,
};
use super::glossary::{prepare_task_glossary, TaskGlossaryMatcher, TaskGlossaryPreparation};
use super::limiter::{
    current_rate_limit_status, AdaptiveLimiter, HeaderQuotaPolicy, ManualRateLimiter,
};
use super::request_options::{resolve_translation_request_options, TranslationRequestOptions};
use super::types::{ChunkOutcome, ChunkRecord};
use super::{
    ConfidenceMode, ContextHandlingMode, PreparedRun, ProgressDetail, ProgressStep,
    RateLimitStrategy, RunMode, TokenStats, TranslationChunkStatus, TranslationInterrupt,
    TranslationProgressPayload, TranslationTaskStatus, TranslationTaskView,
    ERROR_RATE_FAILURE_THRESHOLD, TRANSLATION_PROGRESS_EVENT,
};

fn count_label(name: &str, current: u64, total: u64) -> String {
    format!("{name} ({current}/{total})")
}

fn write_translation_log(log: &Option<BackendLog>, level: &str, message: impl AsRef<str>) {
    if let Some(log) = log {
        log.write(level, "translation", message);
    }
}

#[derive(Clone)]
pub(super) struct ActiveRetryReporter {
    app: AppHandle,
    inp_pool: SqlitePool,
    config_pool: SqlitePool,
    inp_path: PathBuf,
}

impl ActiveRetryReporter {
    pub(super) fn new(
        app: AppHandle,
        inp_pool: SqlitePool,
        config_pool: SqlitePool,
        inp_path: PathBuf,
    ) -> Self {
        Self {
            app,
            inp_pool,
            config_pool,
            inp_path,
        }
    }

    pub(super) async fn report(
        &self,
        chunk_id: &str,
        current: u32,
        max: u32,
        message: String,
    ) -> Result<(), String> {
        set_active_retry_and_emit(
            &self.app,
            &self.inp_pool,
            &self.config_pool,
            &self.inp_path,
            chunk_id,
            current,
            max,
            message,
        )
        .await
    }
}

fn rate_limit_summary(rate_limits: &RateLimitTelemetry) -> String {
    let mut parts = Vec::new();
    if let Some(value) = rate_limits.request_remaining {
        parts.push(format!("request_remaining={value}"));
    }
    if let Some(value) = rate_limits.request_limit {
        parts.push(format!("request_limit={value}"));
    }
    if let Some(value) = rate_limits.request_reset_ms {
        parts.push(format!("request_reset_ms={value}"));
    }
    if let Some(value) = rate_limits.token_remaining {
        parts.push(format!("token_remaining={value}"));
    }
    if let Some(value) = rate_limits.token_limit {
        parts.push(format!("token_limit={value}"));
    }
    if let Some(value) = rate_limits.token_reset_ms {
        parts.push(format!("token_reset_ms={value}"));
    }
    if let Some(value) = rate_limits.retry_after_ms {
        parts.push(format!("retry_after_ms={value}"));
    }
    if let Some(value) = rate_limits.source.as_deref() {
        parts.push(format!("rate_limit_source={value}"));
    }
    if parts.is_empty() {
        "rate_limit_headers=none".to_string()
    } else {
        parts.join(", ")
    }
}

fn log_chunk_issue(
    log: &Option<BackendLog>,
    level: &str,
    chunk: &ChunkRecord,
    attempt: u32,
    max_retries: u32,
    message: impl AsRef<str>,
) {
    write_translation_log(
        log,
        level,
        format!(
            "chunk={} sequence={} attempt={}/{} {}",
            chunk.id,
            chunk.sequence,
            attempt + 1,
            max_retries + 1,
            message.as_ref()
        ),
    );
}

fn retry_action(attempt: u32, max_retries: u32) -> &'static str {
    if attempt == max_retries {
        "no retries left"
    } else {
        "will retry"
    }
}

async fn report_active_retry(
    reporter: Option<&ActiveRetryReporter>,
    chunk: &ChunkRecord,
    attempt: u32,
    max_retries: u32,
    message: &str,
) {
    if attempt >= max_retries {
        return;
    }
    if let Some(reporter) = reporter {
        let _ = reporter
            .report(
                &chunk.id,
                attempt + 1,
                max_retries,
                message.trim().to_string(),
            )
            .await;
    }
}

const RETRY_BACKOFF_BASE_MS: u64 = 1_500;
const RETRY_BACKOFF_CAP_MS: u64 = 12_000;
const RETRY_JITTER_MS: i64 = 500;
const RETRY_MIN_SLEEP_MS: u64 = 250;

pub(super) fn transient_retry_base_delay_ms(error: &ProviderChatError, attempt: u32) -> u64 {
    error
        .rate_limits
        .retry_after_ms
        .unwrap_or_else(|| retry_base_delay_ms(attempt))
        .min(RETRY_BACKOFF_CAP_MS)
}

pub(super) fn retry_base_delay_ms(attempt: u32) -> u64 {
    let multiplier = 1.5_f64.powi(attempt.min(32) as i32);
    ((RETRY_BACKOFF_BASE_MS as f64) * multiplier).ceil() as u64
}

pub(super) fn retry_delay_with_jitter_ms(base_ms: u64) -> u64 {
    let jitter = fastrand::i64(-RETRY_JITTER_MS..=RETRY_JITTER_MS);
    (base_ms as i64 + jitter).clamp(RETRY_MIN_SLEEP_MS as i64, RETRY_BACKOFF_CAP_MS as i64) as u64
}

pub async fn prepare_translation_run(
    app: &AppHandle,
    provider_pool: &SqlitePool,
    client: &Client,
    config_pool: &SqlitePool,
    workspace_root: &Path,
    id: &str,
    mode: RunMode,
) -> Result<PreparedRun, String> {
    let backend_log = BackendLog::from_app(app).ok();
    let indexed = get_task_from_index(config_pool, id).await?;
    write_translation_log(
        &backend_log,
        "INFO",
        format!(
            "Preparing task id={} name=\"{}\" mode={mode:?} current_status={:?}",
            indexed.id, indexed.name, indexed.status
        ),
    );
    let inp_path = PathBuf::from(&indexed.inp_path);
    if !inp_path.starts_with(workspace_root) {
        return Err("Task file is outside the configured workspace".into());
    }
    let inp_pool = connect_inp(&inp_path).await?;
    let config = get_translation_config(config_pool).await?;
    let glossary_config = task_glossary_config(&inp_pool).await?;
    let now = unix_timestamp();
    sqlx::query("UPDATE metadata SET active_retry_json = NULL WHERE task_id = ?")
        .bind(&indexed.id)
        .execute(&inp_pool)
        .await
        .map_err(|error| error.to_string())?;
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
                 SET status = ?, after_translate_text = '', translated_text = '',
                     retry_count = 0, error_message = NULL, confidence = NULL,
                     input_tokens = 0, output_tokens = 0, cached_tokens = 0,
                     thinking_tokens = 0, total_tokens = 0, target_tokens = 0, updated_at = ?
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
            let parsing_detail = ProgressDetail {
                ast: ProgressStep::running(0, 0, "AST 处理中"),
                chunking: ProgressStep::pending(0, 0, count_label("分块", 0, 0)),
                glossary: progress_detail_for_config(0, 0, &glossary_config).glossary,
                translating: ProgressStep::pending(0, 0, count_label("翻译", 0, 0)),
                restore: ProgressStep::pending(0, 0, count_label("占位符恢复", 0, 0)),
            };
            set_progress_detail(&inp_pool, &parsing_detail).await?;
            let rebuilt_chunks = match rebuild_chunks_for_retranslate(
                provider_pool,
                client,
                &inp_pool,
                &indexed,
                config.chunk_token_limit,
                config.pdf_parsing_mode,
                &now,
            )
            .await
            {
                Ok(value) => value,
                Err(error) => {
                    write_translation_log(
                        &backend_log,
                        "ERROR",
                        format!("Task id={} chunk rebuild failed: {error}", indexed.id),
                    );
                    let failed_detail = ProgressDetail {
                        ast: ProgressStep::failed(0, 0, "AST 处理失败"),
                        chunking: ProgressStep::failed(0, 0, count_label("分块", 0, 0)),
                        glossary: progress_detail_for_config(0, 0, &glossary_config).glossary,
                        translating: ProgressStep::pending(0, 0, count_label("翻译", 0, 0)),
                        restore: ProgressStep::pending(0, 0, count_label("占位符恢复", 0, 0)),
                    };
                    set_progress_detail(&inp_pool, &failed_detail).await?;
                    inp_pool.close().await;
                    return Err(error);
                }
            };
            let rebuilt_detail = progress_detail_for_config(rebuilt_chunks, 0, &glossary_config);
            set_progress_detail(&inp_pool, &rebuilt_detail).await?;
        }
    }
    let stats = aggregate_chunk_stats(&inp_pool).await?;
    let current_task = metadata_task(&inp_pool, &inp_path).await?;
    let detail = progress_detail_for_translation_stats(
        current_task.progress_detail,
        stats.total_chunks.max(0) as u64,
        stats.completed_chunks.max(0) as u64,
        TranslationTaskStatus::Running,
        &glossary_config,
    );
    let local_task = commit_prepared_run_state(
        &inp_pool,
        &inp_path,
        config.chunk_token_limit,
        config.max_concurrency,
        config.max_retries,
        config_snapshot_json(&config, &indexed.provider_id, &indexed.model_id),
        &detail,
    )
    .await?;
    inp_pool.close().await;
    let task = publish_task_index_snapshot(config_pool, &local_task).await?;
    write_translation_log(
        &backend_log,
        "INFO",
        format!(
            "Task id={} prepared: total_chunks={} completed_chunks={} concurrency={} retries={}",
            task.id,
            stats.total_chunks,
            stats.completed_chunks,
            config.max_concurrency,
            config.max_retries
        ),
    );
    Ok(PreparedRun {
        task,
        inp_path,
        config,
    })
}

async fn rebuild_chunks_for_retranslate(
    provider_pool: &SqlitePool,
    client: &Client,
    inp_pool: &SqlitePool,
    indexed: &TranslationTaskView,
    token_limit: i64,
    pdf_parsing_mode: PdfParsingMode,
    now: &str,
) -> Result<u64, String> {
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
    let total_chunks = parsed_source.chunks.len() as u64;
    for chunk in parsed_source.chunks {
        let source_tokens = estimate_tokens(&chunk.source_text) as i64;
        sqlx::query(
            "INSERT INTO chunks (
                id, sequence, map_json, preprocessed_text, source_text,
                after_translate_text, translated_text, status, retry_count,
                input_tokens, output_tokens, cached_tokens, thinking_tokens, total_tokens,
                source_tokens, target_tokens, updated_at
             ) VALUES (?, ?, ?, ?, ?, '', '', ?, 0, 0, 0, 0, 0, 0, ?, 0, ?)",
        )
        .bind(format!("{}_chunk_{:06}", indexed.id, chunk.sequence))
        .bind(chunk.sequence)
        .bind(chunk.map_json)
        .bind(chunk.preprocessed_text)
        .bind(chunk.source_text)
        .bind(TranslationChunkStatus::Pending.as_str())
        .bind(source_tokens)
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
    Ok(total_chunks)
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
    let backend_log = BackendLog::from_app(&app).ok();
    let inp_pool = connect_inp(&prepared.inp_path).await?;
    let task = metadata_task(&inp_pool, &prepared.inp_path).await?;
    let pending_chunks = pending_chunks(&inp_pool).await?;
    write_translation_log(
        &backend_log,
        "INFO",
        format!(
            "Task id={} run started: pending_chunks={} max_concurrency={} retries={}",
            task.id,
            pending_chunks.len(),
            prepared.config.max_concurrency,
            prepared.config.max_retries
        ),
    );
    if pending_chunks.is_empty() {
        write_translation_log(
            &backend_log,
            "INFO",
            format!(
                "Task id={} has no pending chunks; finalizing success",
                task.id
            ),
        );
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
            write_translation_log(
                &backend_log,
                "WARN",
                format!(
                    "Task id={} interrupted during glossary preparation",
                    task.id
                ),
            );
            inp_pool.close().await;
            return Ok(());
        }
    };

    let model = app_db::get_model(&provider_pool, &task.model_id).await?;
    let config = app_db::runtime_config(&provider_pool, &task.provider_id).await?;
    let assistant_prompt = task_assistant_prompt(&inp_pool).await?;
    let (assistant_temperature, assistant_top_p) = task_assistant_sampling(&inp_pool).await?;
    let assistant_custom_parameters = task_assistant_custom_parameters(&inp_pool).await?;
    let request_options = resolve_translation_request_options(
        &prepared.config,
        &config,
        &model,
        assistant_custom_parameters,
    )?;
    let adapter = Arc::new(RuntimeAdapter::new(client, config));
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
    write_translation_log(
        &backend_log,
        "INFO",
        format!(
            "Task id={} translation requests ready: model=\"{}\" effective_concurrency={} context_mode={:?}",
            task.id, model.request_name, max_concurrency, context_handling_mode
        ),
    );

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
            request_options,
            assistant_temperature,
            assistant_top_p,
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
            backend_log,
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
    let writer_backend_log = backend_log.clone();
    let writer = tokio::spawn(async move {
        writer_loop(
            writer_app,
            writer_pool,
            writer_config_pool,
            writer_path,
            rx,
            writer_interrupted,
            writer_backend_log,
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
            let backend_log = backend_log.clone();
            let retry_reporter = ActiveRetryReporter::new(
                app.clone(),
                inp_pool.clone(),
                config_pool.clone(),
                prepared.inp_path.clone(),
            );
            let model_request_name = model.request_name.clone();
            let target_language = target_language.clone();
            let assistant_prompt = assistant_prompt.clone();
            let request_options = request_options.clone();
            let assistant_temperature = assistant_temperature;
            let assistant_top_p = assistant_top_p;
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
                                log_chunk_issue(
                                    &backend_log,
                                    "ERROR",
                                    &chunk,
                                    0,
                                    max_retries,
                                    format!("previous source context failed: {error}"),
                                );
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
                let Some(_permit) = limiter.acquire(interrupted.token()).await else {
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
                    request_options,
                    assistant_temperature,
                    assistant_top_p,
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
                    backend_log,
                    interrupted.clone(),
                    Some(retry_reporter),
                )
                .await;
                if outcome.interrupt_task {
                    interrupted.interrupt(
                        outcome
                            .error_message
                            .clone()
                            .unwrap_or_else(|| "Task interrupted".to_string()),
                    );
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
    request_options: TranslationRequestOptions,
    assistant_temperature: Option<f64>,
    assistant_top_p: Option<f64>,
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
    backend_log: Option<BackendLog>,
    interrupt: &TranslationInterrupt,
) -> Result<(), String> {
    for chunk in pending_chunks {
        if interrupt.is_interrupted() {
            break;
        }
        let previous_context = match previous_translation_context(inp_pool, chunk.sequence).await {
            Ok(context) => context,
            Err(error) => {
                log_chunk_issue(
                    &backend_log,
                    "ERROR",
                    &chunk,
                    0,
                    max_retries,
                    format!("previous translation context failed: {error}"),
                );
                return Err(error);
            }
        };
        let Some(_permit) = limiter.acquire(interrupt.token()).await else {
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
            request_options.clone(),
            assistant_temperature,
            assistant_top_p,
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
            backend_log.clone(),
            interrupt.clone(),
            Some(ActiveRetryReporter::new(
                app.clone(),
                inp_pool.clone(),
                config_pool.clone(),
                inp_path.to_path_buf(),
            )),
        )
        .await;
        let interrupt_task = outcome.interrupt_task;
        let interrupt_reason = outcome.error_message.clone();
        apply_and_emit_chunk_outcome(app, inp_pool, config_pool, inp_path, outcome).await?;
        if interrupt_task {
            interrupt.interrupt(interrupt_reason.unwrap_or_else(|| "Task interrupted".to_string()));
            limiter.notify_waiters();
        }
    }
    finalize_translation_run(
        app,
        inp_pool,
        config_pool,
        inp_path,
        interrupt,
        &backend_log,
    )
    .await
}

async fn writer_loop(
    app: AppHandle,
    inp_pool: SqlitePool,
    config_pool: SqlitePool,
    inp_path: PathBuf,
    mut rx: mpsc::Receiver<ChunkOutcome>,
    interrupted: TranslationInterrupt,
    backend_log: Option<BackendLog>,
) -> Result<(), String> {
    while let Some(outcome) = rx.recv().await {
        apply_and_emit_chunk_outcome(&app, &inp_pool, &config_pool, &inp_path, outcome).await?;
    }
    finalize_translation_run(
        &app,
        &inp_pool,
        &config_pool,
        &inp_path,
        &interrupted,
        &backend_log,
    )
    .await
}

async fn apply_and_emit_chunk_outcome(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    outcome: ChunkOutcome,
) -> Result<(), String> {
    let chunk_id = outcome.chunk_id.clone();
    apply_chunk_outcome(inp_pool, outcome).await?;
    clear_active_retry_for_chunk(inp_pool, &chunk_id).await?;
    let stats = aggregate_chunk_stats(inp_pool).await?;
    let metadata = metadata_task(inp_pool, inp_path).await?;
    let glossary_config = task_glossary_config(inp_pool).await?;
    let task_status = metadata.status;
    let existing_detail = metadata.progress_detail;
    let detail = progress_detail_for_translation_stats(
        existing_detail,
        stats.total_chunks.max(0) as u64,
        stats.completed_chunks.max(0) as u64,
        task_status,
        &glossary_config,
    );
    set_progress_detail(inp_pool, &detail).await?;
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
    backend_log: &Option<BackendLog>,
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
    let metadata = metadata_task(inp_pool, inp_path).await?;
    let glossary_config = task_glossary_config(inp_pool).await?;
    let detail = progress_detail_for_translation_stats(
        metadata.progress_detail,
        stats.total_chunks.max(0) as u64,
        stats.completed_chunks.max(0) as u64,
        status,
        &glossary_config,
    );
    set_progress_detail(inp_pool, &detail).await?;
    let log_level = match status {
        TranslationTaskStatus::Success => "INFO",
        TranslationTaskStatus::Interrupted | TranslationTaskStatus::InterruptedPending => "WARN",
        TranslationTaskStatus::Failed => "ERROR",
        TranslationTaskStatus::Pending
        | TranslationTaskStatus::Queued
        | TranslationTaskStatus::Running => "INFO",
    };
    write_translation_log(
        backend_log,
        log_level,
        format!(
            "Task id={} finalized status={:?} completed_chunks={} failed_chunks={} interrupted_chunks={} error_rate={:.1}% last_error={}",
            metadata.id,
            status,
            stats.completed_chunks,
            stats.failed_chunks,
            stats.interrupted_chunks,
            stats.error_rate * 100.0,
            last_error.as_deref().unwrap_or("none")
        ),
    );
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

pub(super) async fn translate_chunk(
    adapter: Arc<RuntimeAdapter>,
    model_request_name: String,
    target_language: String,
    assistant_prompt: Option<String>,
    request_options: TranslationRequestOptions,
    assistant_temperature: Option<f64>,
    assistant_top_p: Option<f64>,
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
    backend_log: Option<BackendLog>,
    interrupt: TranslationInterrupt,
    retry_reporter: Option<ActiveRetryReporter>,
) -> ChunkOutcome {
    let mut retry_count = 0_i64;
    let mut last_error = None;
    let mut last_text = None;
    let mut last_stats = TokenStats::default();
    for attempt in 0..=max_retries {
        if interrupt.is_interrupted() {
            return interrupted_outcome(chunk, retry_count, interrupt.reason());
        }
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
                        log_chunk_issue(
                            &backend_log,
                            "ERROR",
                            &chunk,
                            attempt,
                            max_retries,
                            format!("placeholder restore failed in passthrough: {error}"),
                        );
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
                log_chunk_issue(
                    &backend_log,
                    "ERROR",
                    &chunk,
                    attempt,
                    max_retries,
                    format!("prompt build failed: {error}"),
                );
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
            web_search: request_options.web_search,
            thinking: request_options.thinking.clone(),
            max_output_tokens: None,
            temperature: assistant_temperature,
            top_p: assistant_top_p,
            stream: false,
            logprobs: confidence_mode.enabled(),
            custom_parameters: request_options.custom_parameters.clone(),
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
            if !manual_limiter
                .before_request(estimated_tokens, interrupt.token())
                .await
            {
                return interrupted_outcome(chunk, retry_count, interrupt.reason());
            }
        }
        if !quota
            .before_request(estimated_tokens, interrupt.token())
            .await
        {
            return interrupted_outcome(chunk, retry_count, interrupt.reason());
        }
        let request_result = send_chat_with_logprobs_fallback(
            &adapter,
            &request,
            estimated_tokens,
            &quota,
            &limiter,
            manual_limiter.as_ref(),
            interrupt.token(),
        )
        .await;
        if interrupt.is_interrupted() {
            return interrupted_outcome(chunk, retry_count, interrupt.reason());
        }
        match request_result {
            Ok(Some(meta)) => {
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
                if finish_reason_is_truncation(meta.finish_reason.as_deref()) {
                    let finish_reason = meta.finish_reason.as_deref().unwrap_or("truncation");
                    last_error = Some(format!("Interrupted by finish reason: {finish_reason}"));
                    log_chunk_issue(
                        &backend_log,
                        if attempt == max_retries {
                            "ERROR"
                        } else {
                            "WARN"
                        },
                        &chunk,
                        attempt,
                        max_retries,
                        format!(
                            "provider returned status={} finish_reason={} {}; {}",
                            meta.status,
                            finish_reason,
                            retry_action(attempt, max_retries),
                            rate_limit_summary(&meta.rate_limits)
                        ),
                    );
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
                    report_active_retry(
                        retry_reporter.as_ref(),
                        &chunk,
                        attempt,
                        max_retries,
                        last_error
                            .as_deref()
                            .unwrap_or("Interrupted by finish reason"),
                    )
                    .await;
                    continue;
                }
                if text.trim().is_empty() {
                    last_error = Some("Model returned empty content".to_string());
                    log_chunk_issue(
                        &backend_log,
                        if attempt == max_retries {
                            "ERROR"
                        } else {
                            "WARN"
                        },
                        &chunk,
                        attempt,
                        max_retries,
                        format!(
                            "model returned empty content; {}; {}",
                            retry_action(attempt, max_retries),
                            rate_limit_summary(&meta.rate_limits)
                        ),
                    );
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
                    report_active_retry(
                        retry_reporter.as_ref(),
                        &chunk,
                        attempt,
                        max_retries,
                        last_error
                            .as_deref()
                            .unwrap_or("Model returned empty content"),
                    )
                    .await;
                    continue;
                }
                if text.trim() == chunk.source_text.trim() && !chunk.source_text.trim().is_empty() {
                    last_error = Some("Model returned unchanged source text".to_string());
                    log_chunk_issue(
                        &backend_log,
                        if attempt == max_retries {
                            "ERROR"
                        } else {
                            "WARN"
                        },
                        &chunk,
                        attempt,
                        max_retries,
                        format!(
                            "model returned unchanged source text; {}; {}",
                            retry_action(attempt, max_retries),
                            rate_limit_summary(&meta.rate_limits)
                        ),
                    );
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
                    report_active_retry(
                        retry_reporter.as_ref(),
                        &chunk,
                        attempt,
                        max_retries,
                        last_error
                            .as_deref()
                            .unwrap_or("Model returned unchanged source text"),
                    )
                    .await;
                    continue;
                }
                let translated_text = match restore_chunk_for_map(&chunk.map_json, &text) {
                    Ok(restored) => restored,
                    Err(error) => {
                        last_error = Some(error);
                        log_chunk_issue(
                            &backend_log,
                            if attempt == max_retries {
                                "ERROR"
                            } else {
                                "WARN"
                            },
                            &chunk,
                            attempt,
                            max_retries,
                            format!(
                                "placeholder restore failed after model response: {}; {}; {}",
                                last_error.as_deref().unwrap_or("unknown"),
                                retry_action(attempt, max_retries),
                                rate_limit_summary(&meta.rate_limits)
                            ),
                        );
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
                        report_active_retry(
                            retry_reporter.as_ref(),
                            &chunk,
                            attempt,
                            max_retries,
                            last_error
                                .as_deref()
                                .unwrap_or("Placeholder restore failed"),
                        )
                        .await;
                        continue;
                    }
                };
                if attempt > 0 {
                    log_chunk_issue(
                        &backend_log,
                        "INFO",
                        &chunk,
                        attempt,
                        max_retries,
                        "chunk recovered after retry",
                    );
                }
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
            Ok(None) => {
                return interrupted_outcome(chunk, retry_count, interrupt.reason());
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
                if error.is_model_unavailable() {
                    return failed_outcome(
                        chunk,
                        TranslationChunkStatus::Failed,
                        retry_count,
                        Some(format!("MODEL_UNAVAILABLE:TRANSLATION:{error}")),
                        last_text,
                        last_stats,
                        rate_status,
                        true,
                    );
                }
                last_error = Some(error.to_string());
                log_chunk_issue(
                    &backend_log,
                    if !error.is_transient() || attempt == max_retries {
                        "ERROR"
                    } else {
                        "WARN"
                    },
                    &chunk,
                    attempt,
                    max_retries,
                    format!(
                        "provider request failed status={} rate_limited={} {}; {}; error={}",
                        error
                            .status
                            .map(|status| status.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        error.is_rate_limited(),
                        retry_action(attempt, max_retries),
                        rate_limit_summary(&error.rate_limits),
                        error
                    ),
                );
                if !error.is_transient() {
                    return failed_outcome(
                        chunk,
                        TranslationChunkStatus::Failed,
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
                report_active_retry(
                    retry_reporter.as_ref(),
                    &chunk,
                    attempt,
                    max_retries,
                    last_error.as_deref().unwrap_or("Provider request failed"),
                )
                .await;
                let base_delay = transient_retry_base_delay_ms(&error, attempt);
                let sleep_ms = retry_delay_with_jitter_ms(base_delay);
                log_chunk_issue(
                    &backend_log,
                    "WARN",
                    &chunk,
                    attempt,
                    max_retries,
                    format!(
                        "transient provider error backoff_ms={sleep_ms} base_backoff_ms={base_delay}"
                    ),
                );
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(sleep_ms)) => {}
                    _ = interrupt.cancelled() => {
                        return interrupted_outcome(chunk, retry_count, interrupt.reason());
                    }
                }
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
    cancellation: &CancellationToken,
) -> Result<Option<ProviderChatMeta>, ProviderChatError> {
    let initial = tokio::select! {
        result = adapter.send_chat_with_meta(request) => Some(result),
        _ = cancellation.cancelled() => None,
    };
    let Some(initial) = initial else {
        return Ok(None);
    };
    match initial {
        Ok(meta) => Ok(Some(meta)),
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
                if !manual_limiter
                    .before_request(estimated_tokens, cancellation)
                    .await
                {
                    return Ok(None);
                }
            }
            if !quota.before_request(estimated_tokens, cancellation).await {
                return Ok(None);
            }
            tokio::select! {
                result = adapter.send_chat_with_meta(&fallback) => result.map(Some),
                _ = cancellation.cancelled() => Ok(None),
            }
        }
        Err(error) => Err(error),
    }
}

pub(super) fn logprobs_parameter_rejected(error: &ProviderChatError) -> bool {
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

fn interrupted_outcome(
    chunk: ChunkRecord,
    retry_count: i64,
    reason: Option<String>,
) -> ChunkOutcome {
    ChunkOutcome {
        chunk_id: chunk.id,
        status: TranslationChunkStatus::Interrupted,
        interrupt_task: false,
        after_translate_text: String::new(),
        translated_text: String::new(),
        retry_count,
        error_message: reason.or_else(|| Some("Task paused".to_string())),
        token_stats: TokenStats::default(),
        rate_limit_status: None,
        confidence: None,
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

pub async fn mark_task_failed_after_runtime_error(
    config_pool: &SqlitePool,
    inp_path: &Path,
    error: String,
) -> Result<TranslationTaskView, String> {
    let inp_pool = connect_inp(inp_path).await?;
    sqlx::query(
        "UPDATE metadata
         SET status = ?, last_error = ?, active_retry_json = NULL, queued_from_status = NULL, updated_at = ?",
    )
    .bind(TranslationTaskStatus::Failed.as_str())
    .bind(error)
    .bind(unix_timestamp())
    .execute(&inp_pool)
    .await
    .map_err(|error| error.to_string())?;
    let stats = aggregate_chunk_stats(&inp_pool).await?;
    let metadata = metadata_task(&inp_pool, inp_path).await?;
    let glossary_config = task_glossary_config(&inp_pool).await?;
    let detail = progress_detail_for_translation_stats(
        metadata.progress_detail,
        stats.total_chunks.max(0) as u64,
        stats.completed_chunks.max(0) as u64,
        TranslationTaskStatus::Failed,
        &glossary_config,
    );
    set_progress_detail(&inp_pool, &detail).await?;
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
