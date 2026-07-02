use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{stream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::adapters::{
    finish_reason_is_truncation, ProviderChatError, ProviderChatMeta, RuntimeAdapter,
};
use crate::db as app_db;
use crate::document_parsing::restore_chunk_for_map;
use crate::domain::{UnifiedChatRequest, UnifiedToolChoice};
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
    aggregate_chunk_stats, apply_chunk_outcome, config_snapshot_json, connect_inp,
    content_format_from_source_path, document_format_from_source_path,
    effective_translation_concurrency, finalize_task, get_task_from_index, get_translation_config,
    glossary_source_chunks, insert_assets, metadata_task, parse_source_file_for_task,
    pending_chunks, refresh_task_stats, resolve_source_file, task_assistant_custom_parameters,
    task_assistant_prompt,
};
use super::glossary::{prepare_task_glossary, TaskGlossaryMatcher, TaskGlossaryPreparation};
use super::limiter::{
    current_rate_limit_status, AdaptiveLimiter, HeaderQuotaPolicy, ManualRateLimiter,
};
use super::types::{ChunkOutcome, ChunkRecord};
use super::{
    ConfidenceMode, ContextHandlingMode, PreparedRun, RateLimitStrategy, RunMode, TokenStats,
    TranslationChunkStatus, TranslationInterrupt, TranslationProgressPayload,
    TranslationTaskStatus, TranslationTaskView, ERROR_RATE_FAILURE_THRESHOLD,
    TRANSLATION_PROGRESS_EVENT,
};

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

    let model = app_db::get_model(&provider_pool, &task.model_id).await?;
    let config = app_db::runtime_config(&provider_pool, &task.provider_id).await?;
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
