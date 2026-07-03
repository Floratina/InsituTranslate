use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use futures_util::{stream, StreamExt};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};

use crate::adapters::RuntimeAdapter;
use crate::db as app_db;
use crate::domain::{ProviderPurpose, UnifiedChatRequest, UnifiedToolChoice};
use crate::glossaries::{self, CreateAutoGlossaryInput, GlossaryView, PrepareAutoGlossaryInput};
use crate::glossary_prompt::{
    build_glossary_prompt, sanitize_and_flatten_glossary, GlossaryEntry, GlossaryPromptBuildResult,
    GlossaryPromptInput,
};
use crate::task_prompt::{ContentFormat, DocumentFormat, TaskChunkInput};

use super::context::estimate_tokens;
use super::db::{
    connect_inp, content_format_from_source_path, document_format_from_source_path, finalize_task,
    get_task_from_index, get_translation_config, glossary_source_chunks, metadata_task,
    progress_detail_for_config, refresh_task_stats, set_progress_detail_and_emit,
    set_task_glossary_id, task_glossary_config,
};
use super::limiter::{AdaptiveLimiter, HeaderQuotaPolicy, ManualRateLimiter};
use super::types::ChunkRecord;
use super::{
    GlossaryMode, ProgressStep, RateLimitStrategy, TranslationConfigView, TranslationInterrupt,
    TranslationProgressPayload, TranslationTaskStatus, TranslationTaskView,
    AUTO_GLOSSARY_FAILURE_THRESHOLD, TRANSLATION_PROGRESS_EVENT,
};

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

pub(super) enum TaskGlossaryPreparation {
    Ready(Vec<GlossaryEntry>),
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
    Interrupted(String),
}

async fn emit_glossary_progress(
    app: &AppHandle,
    inp_pool: &SqlitePool,
    config_pool: &SqlitePool,
    inp_path: &Path,
    current: u64,
    total: u64,
    state: &str,
) -> Result<(), String> {
    let metadata = metadata_task(inp_pool, inp_path).await?;
    let glossary_config = task_glossary_config(inp_pool).await?;
    let total_chunks = metadata.total_chunks.max(0) as u64;
    let completed_chunks = metadata.completed_chunks.max(0) as u64;
    let status = metadata.status;
    let existing_detail = metadata.progress_detail;
    let mut detail = existing_detail.unwrap_or_else(|| {
        progress_detail_for_config(total_chunks, completed_chunks, &glossary_config)
    });
    detail.glossary = ProgressStep::new(
        state,
        current,
        total,
        format!("术语表建立 ({current}/{total})"),
    );
    set_progress_detail_and_emit(app, inp_pool, config_pool, inp_path, &detail, Some(status))
        .await?;
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
    let config = get_translation_config(config_pool).await?;
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
    let glossary_runtime = select_glossary_runtime(provider_pool, client).await?;
    let chunks = pending_chunks
        .iter()
        .filter(|chunk| !chunk.source_text.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if chunks.is_empty() {
        emit_glossary_progress(app, inp_pool, config_pool, inp_path, 0, 0, "success").await?;
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
    let total_chunks = chunks.len() as u64;
    emit_glossary_progress(
        app,
        inp_pool,
        config_pool,
        inp_path,
        0,
        total_chunks,
        "running",
    )
    .await?;

    let mut outcome_stream = stream::iter(chunks.clone())
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
        .buffer_unordered(max_concurrency);
    let mut outcomes = Vec::new();
    let mut completed_chunks = 0_u64;
    while let Some(outcome) = outcome_stream.next().await {
        completed_chunks += 1;
        let state = if matches!(outcome, AutoGlossaryChunkOutcome::Interrupted { .. }) {
            "failed"
        } else if completed_chunks >= total_chunks {
            "success"
        } else {
            "running"
        };
        emit_glossary_progress(
            app,
            inp_pool,
            config_pool,
            inp_path,
            completed_chunks,
            total_chunks,
            state,
        )
        .await?;
        outcomes.push(outcome);
    }
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
                    emit_glossary_progress(
                        app,
                        inp_pool,
                        config_pool,
                        inp_path,
                        completed_chunks,
                        total_chunks,
                        "failed",
                    )
                    .await?;
                    return Ok(AutoGlossaryGeneration::Interrupted(format!(
                        "Auto glossary generation failed for more than 40% of chunks: {error}"
                    )));
                }
            }
            AutoGlossaryChunkOutcome::Interrupted { error } => {
                emit_glossary_progress(
                    app,
                    inp_pool,
                    config_pool,
                    inp_path,
                    completed_chunks,
                    total_chunks,
                    "failed",
                )
                .await?;
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
    let provider = app_db::list_providers(provider_pool, Some(ProviderPurpose::Glossary))
        .await?
        .into_iter()
        .find(|provider| provider.enabled)
        .ok_or_else(|| "No enabled glossary provider is configured".to_string())?;
    let model = provider
        .models
        .first()
        .ok_or_else(|| "The selected glossary provider has no model".to_string())?;
    let assistant = app_db::list_assistants(provider_pool, ProviderPurpose::Glossary)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "No glossary assistant is configured".to_string())?;
    let config = app_db::runtime_config(provider_pool, &provider.id).await?;
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
            web_search: false,
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

pub(super) fn is_ascii_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn spans_overlap(left_start: usize, left_end: usize, right_start: usize, right_end: usize) -> bool {
    left_start < right_end && right_start < left_end
}
