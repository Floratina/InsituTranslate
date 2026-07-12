use crate::adapters::{
    ProviderChatError, ProviderChatErrorKind, RateLimitTelemetry, RuntimeAdapter,
};
use crate::db as app_db;
use crate::domain::{
    AddModelInput, CreateProviderInput, ProviderProtocol, ProviderPurpose, ProviderRuntimeConfig,
    SetProviderEnabledInput, ThinkingEffort, UpdateAssistantCustomParametersInput,
};
use crate::glossary_prompt::GlossaryEntry;
use crate::languages::{DEFAULT_SOURCE_LANGUAGE, DEFAULT_TARGET_LANGUAGE};
use crate::pdf_parsing::PdfParsingMode;
use crate::task_prompt::{ContentFormat, DocumentFormat};
use reqwest::Client;
use serde_json::{json, Value};
use sqlx::Row;

use super::context::{
    ensure_task_global_background, estimate_tokens, global_background_from_texts,
    previous_source_context, previous_translation_context, sanitize_file_stem, unix_timestamp,
};
use super::db::{
    apply_chunk_outcome, connect_inp, connect_sqlite, effective_translation_concurrency,
    export_file_name, get_task_from_index, normalize_tags, normalize_task_filters,
    release_assets_for_export, rendered_task_document, serialize_tags, source_extension,
    task_glossary_config, translated_source_text, validate_inp_file,
};
use super::glossary::TaskGlossaryMatcher;
use super::limiter::{AdaptiveLimiter, HeaderQuotaPolicy};
use super::request_options::TranslationRequestOptions;
use super::scheduler::{
    logprobs_parameter_rejected, retry_base_delay_ms, retry_delay_with_jitter_ms,
    transient_retry_base_delay_ms, translate_chunk,
};
use super::types::{ChunkOutcome, ChunkRecord};

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

#[tokio::test]
async fn config_database_uses_wal_and_five_second_busy_timeout() {
    let root = temp_root("config-pragmas");
    let pool = connect_config_db(&root).await.expect("connect config");

    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .expect("journal mode");
    let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .expect("busy timeout");

    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(busy_timeout, 5_000);
    pool.close().await;
    let _ = std::fs::remove_dir_all(root);
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

use super::db::{metadata_task, publish_task_index_snapshot};
use super::*;
use crate::document_parsing::types::{BlockRef, PlaceholderMap};
use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "insitu-test-{label}-{}",
        app_db::new_id("workspace")
    ))
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

#[test]
fn translation_task_status_roundtrips_interrupted_pending() {
    assert_eq!(
        TranslationTaskStatus::InterruptedPending.as_str(),
        "interrupted-pending"
    );
    assert_eq!(
        TranslationTaskStatus::parse("interrupted-pending").expect("parse status"),
        TranslationTaskStatus::InterruptedPending
    );
}

#[tokio::test]
async fn create_task_freezes_glossary_config_in_inp_metadata() {
    let root = temp_root("glossary-freeze");
    tokio::fs::create_dir_all(&root).await.expect("create root");
    let provider_db = root.join("providers.sqlite");
    let provider_pool = app_db::connect(&provider_db).await.expect("provider db");
    let config_pool = connect_config_db(&root).await.expect("config db");
    let provider = app_db::create_provider(
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
    let model = app_db::add_model(
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
            glossary_generation_config: GlossaryGenerationConfig::default(),
            thinking_effort: ThinkingEffort::None,
            use_web_search: false,
            use_custom_parameters: false,
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
            use_glossary: true,
            glossary_mode: GlossaryMode::Existing,
            glossary_id: Some("glossary-freeze-id".into()),
            glossary_generation_config: GlossaryGenerationConfig::default(),
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
async fn create_task_rejects_disabled_translation_provider() {
    let root = temp_root("disabled-translation-provider");
    tokio::fs::create_dir_all(&root).await.expect("create root");
    let provider_pool = app_db::connect(&root.join("providers.sqlite"))
        .await
        .expect("provider db");
    let config_pool = connect_config_db(&root).await.expect("config db");
    let provider = app_db::create_provider(
        &provider_pool,
        CreateProviderInput {
            name: "Disabled Translation Provider".into(),
            protocol: ProviderProtocol::OpenaiChat,
            purpose: ProviderPurpose::Translation,
            avatar: None,
        },
    )
    .await
    .expect("provider");
    let model = app_db::add_model(
        &provider_pool,
        AddModelInput {
            provider_id: provider.id.clone(),
            request_name: "disabled-provider-model".into(),
            alias: "Disabled Provider Model".into(),
            source: "manual".into(),
        },
    )
    .await
    .expect("model");
    app_db::set_provider_enabled(
        &provider_pool,
        SetProviderEnabledInput {
            id: provider.id.clone(),
            enabled: false,
        },
    )
    .await
    .expect("disable provider");
    let source_path = root.join("source.txt");
    tokio::fs::write(&source_path, "Apple animation.")
        .await
        .expect("write source");

    let error = create_translation_task(
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
            use_glossary: false,
            glossary_mode: GlossaryMode::Auto,
            glossary_id: None,
            glossary_generation_config: GlossaryGenerationConfig::default(),
        },
    )
    .await
    .expect_err("disabled provider must be rejected");

    assert!(error.contains("disabled"));
    provider_pool.close().await;
    config_pool.close().await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn auto_glossary_snapshot_is_frozen_and_legacy_backfill_missing_model_requires_action() {
    let root = temp_root("auto-glossary-snapshot");
    tokio::fs::create_dir_all(&root).await.expect("create root");
    let provider_pool = app_db::connect(&root.join("providers.sqlite"))
        .await
        .expect("provider db");
    let config_pool = connect_config_db(&root).await.expect("config db");

    let translation_provider = app_db::create_provider(
        &provider_pool,
        CreateProviderInput {
            name: "Translation Snapshot Provider".into(),
            protocol: ProviderProtocol::OpenaiChat,
            purpose: ProviderPurpose::Translation,
            avatar: None,
        },
    )
    .await
    .expect("translation provider");
    let translation_model = app_db::add_model(
        &provider_pool,
        AddModelInput {
            provider_id: translation_provider.id.clone(),
            request_name: "translation-snapshot-model".into(),
            alias: "Translation Snapshot Model".into(),
            source: "manual".into(),
        },
    )
    .await
    .expect("translation model");
    let glossary_provider = app_db::create_provider(
        &provider_pool,
        CreateProviderInput {
            name: "Glossary Snapshot Provider".into(),
            protocol: ProviderProtocol::OpenaiChat,
            purpose: ProviderPurpose::Glossary,
            avatar: None,
        },
    )
    .await
    .expect("glossary provider");
    let glossary_model = app_db::add_model(
        &provider_pool,
        AddModelInput {
            provider_id: glossary_provider.id.clone(),
            request_name: "glossary-snapshot-model".into(),
            alias: "Glossary Snapshot Model".into(),
            source: "manual".into(),
        },
    )
    .await
    .expect("glossary model");
    let glossary_assistant = app_db::list_assistants(&provider_pool, ProviderPurpose::Glossary)
        .await
        .expect("glossary assistants")
        .into_iter()
        .next()
        .expect("default glossary assistant");
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
            provider_id: translation_provider.id.clone(),
            model_id: translation_model.id.clone(),
            assistant_id: None,
            use_glossary: true,
            glossary_mode: GlossaryMode::Auto,
            glossary_id: None,
            glossary_generation_config: GlossaryGenerationConfig {
                provider_id: glossary_provider.id.clone(),
                model_id: glossary_model.id.clone(),
                assistant_id: Some(glossary_assistant.id.clone()),
                thinking_effort: ThinkingEffort::None,
                use_web_search: false,
                use_custom_parameters: true,
            },
        },
    )
    .await
    .expect("create task");
    let inp_pool = connect_inp(Path::new(&task.inp_path)).await.expect("inp");
    let snapshot_json: String =
        sqlx::query_scalar("SELECT glossary_generation_snapshot_json FROM metadata LIMIT 1")
            .fetch_one(&inp_pool)
            .await
            .expect("glossary snapshot");
    let snapshot: Value = serde_json::from_str(&snapshot_json).expect("snapshot json");
    assert_eq!(snapshot["modelRequestName"], "glossary-snapshot-model");
    assert_eq!(snapshot["assistantId"], glossary_assistant.id);
    assert!(snapshot.get("credential").is_none());
    assert!(snapshot.get("headers").is_none());
    sqlx::query("UPDATE metadata SET glossary_generation_snapshot_json = NULL")
        .execute(&inp_pool)
        .await
        .expect("clear glossary snapshot");
    inp_pool.close().await;

    let fallback_config = TranslationConfigView {
        provider_id: translation_provider.id,
        model_id: translation_model.id,
        use_glossary: true,
        glossary_mode: GlossaryMode::Auto,
        glossary_generation_config: GlossaryGenerationConfig {
            provider_id: glossary_provider.id.clone(),
            model_id: glossary_model.id.clone(),
            assistant_id: Some(glossary_assistant.id.clone()),
            thinking_effort: ThinkingEffort::None,
            use_web_search: false,
            use_custom_parameters: true,
        },
        ..TranslationConfigView::default()
    };
    sqlx::query("UPDATE translation_config SET config_json = ? WHERE id = 1")
        .bind(serde_json::to_string(&fallback_config).expect("fallback config json"))
        .execute(&config_pool)
        .await
        .expect("persist legacy fallback config");

    app_db::set_provider_enabled(
        &provider_pool,
        SetProviderEnabledInput {
            id: glossary_provider.id.clone(),
            enabled: false,
        },
    )
    .await
    .expect("disable glossary provider");
    let error = get_task_runtime_action_required(&provider_pool, &config_pool, &root, &task.id)
        .await
        .expect_err("disabled glossary provider must be a hard error");
    assert!(error.contains("glossary provider is disabled"));
    app_db::set_provider_enabled(
        &provider_pool,
        SetProviderEnabledInput {
            id: glossary_provider.id,
            enabled: true,
        },
    )
    .await
    .expect("enable glossary provider");

    app_db::delete_model(&provider_pool, &glossary_model.id)
        .await
        .expect("delete glossary model");
    let action = get_task_runtime_action_required(&provider_pool, &config_pool, &root, &task.id)
        .await
        .expect("runtime action")
        .expect("action required");
    assert_eq!(action.reason, TaskRuntimeActionReason::LocalConfigMissing);
    assert_eq!(action.domains, vec![TaskRuntimeConfigDomain::Glossary]);

    provider_pool.close().await;
    config_pool.close().await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn create_task_injects_custom_parameters_only_when_enabled() {
    let root = temp_root("custom-parameters-switch");
    tokio::fs::create_dir_all(&root).await.expect("create root");
    let provider_db = root.join("providers.sqlite");
    let provider_pool = app_db::connect(&provider_db).await.expect("provider db");
    let config_pool = connect_config_db(&root).await.expect("config db");
    let provider = app_db::create_provider(
        &provider_pool,
        CreateProviderInput {
            name: "Custom Parameter Provider".into(),
            protocol: ProviderProtocol::OpenaiChat,
            purpose: ProviderPurpose::Translation,
            avatar: None,
        },
    )
    .await
    .expect("provider");
    let model = app_db::add_model(
        &provider_pool,
        AddModelInput {
            provider_id: provider.id.clone(),
            request_name: "custom-parameter-model".into(),
            alias: "Custom Parameter Model".into(),
            source: "manual".into(),
        },
    )
    .await
    .expect("model");
    let assistant = app_db::list_assistants(&provider_pool, ProviderPurpose::Translation)
        .await
        .expect("assistants")
        .into_iter()
        .next()
        .expect("default assistant");
    let assistant = app_db::update_assistant_custom_parameters(
        &provider_pool,
        UpdateAssistantCustomParametersInput {
            id: assistant.id,
            custom_parameters: json!({"service_tier": "flex"}),
        },
    )
    .await
    .expect("custom parameters");
    let source_path = root.join("custom-parameters.txt");
    tokio::fs::write(&source_path, "Apple animation.")
        .await
        .expect("write source");

    for (enabled, expected) in [(false, json!({})), (true, json!({"service_tier": "flex"}))] {
        update_translation_config(
            &config_pool,
            UpdateTranslationConfigInput {
                source_language: "English".into(),
                custom_source_language: String::new(),
                target_language: "Simplified Chinese".into(),
                custom_target_language: String::new(),
                provider_id: provider.id.clone(),
                model_id: model.id.clone(),
                assistant_id: assistant.id.clone(),
                chunk_token_limit: 800,
                max_concurrency: 3,
                max_retries: 2,
                rate_limit_strategy: RateLimitStrategy::Dynamic,
                max_requests_per_minute: 120,
                max_tokens_per_minute: 60_000,
                context_handling_mode: ContextHandlingMode::Off,
                use_global_background: false,
                use_glossary: false,
                glossary_mode: GlossaryMode::Auto,
                glossary_id: None,
                glossary_generation_config: GlossaryGenerationConfig::default(),
                thinking_effort: ThinkingEffort::None,
                use_web_search: false,
                use_custom_parameters: enabled,
                confidence_mode: ConfidenceMode::Off,
                pdf_parsing_mode: PdfParsingMode::LocalFirst,
            },
        )
        .await
        .expect("update config");
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
                provider_id: provider.id.clone(),
                model_id: model.id.clone(),
                assistant_id: Some(assistant.id.clone()),
                use_glossary: false,
                glossary_mode: GlossaryMode::Auto,
                glossary_id: None,
                glossary_generation_config: GlossaryGenerationConfig::default(),
            },
        )
        .await
        .expect("create task");
        let inp_pool = connect_inp(Path::new(&task.inp_path)).await.expect("inp");
        let stored_json: String =
            sqlx::query_scalar("SELECT assistant_custom_parameters_json FROM metadata LIMIT 1")
                .fetch_one(&inp_pool)
                .await
                .expect("stored custom parameters");
        inp_pool.close().await;
        let stored: Value = serde_json::from_str(&stored_json).expect("stored json");
        assert_eq!(stored, expected);
    }

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

async fn downgrade_inp_fixture(path: &Path, schema_version: i64) {
    let pool = connect_sqlite(path, 1).await.expect("open fixture");
    let versioned_columns = [
        (2, "chunks", "map_json"),
        (2, "chunks", "preprocessed_text"),
        (2, "chunks", "after_translate_text"),
        (3, "chunks", "confidence"),
        (6, "metadata", "use_glossary"),
        (6, "metadata", "glossary_mode"),
        (6, "metadata", "glossary_id"),
        (7, "metadata", "global_background"),
        (8, "metadata", "progress_detail_json"),
        (9, "metadata", "active_retry_json"),
        (10, "metadata", "source_text_tokens"),
        (10, "metadata", "target_text_tokens"),
        (10, "metadata", "total_text_tokens"),
        (10, "metadata", "queued_from_status"),
        (10, "chunks", "source_tokens"),
        (10, "chunks", "target_tokens"),
        (11, "metadata", "assistant_temperature"),
        (11, "metadata", "assistant_top_p"),
        (11, "metadata", "glossary_generation_snapshot_json"),
        (11, "metadata", "runtime_action_required_json"),
    ];
    for (introduced, table, column) in versioned_columns {
        if schema_version < introduced {
            sqlx::query(&format!("ALTER TABLE {table} DROP COLUMN {column}"))
                .execute(&pool)
                .await
                .unwrap_or_else(|error| panic!("drop {table}.{column}: {error}"));
        }
    }
    if schema_version < 5 {
        sqlx::query("DROP TABLE source_file")
            .execute(&pool)
            .await
            .expect("drop source_file");
    }
    if schema_version < 4 {
        sqlx::query("DROP TABLE assets")
            .execute(&pool)
            .await
            .expect("drop assets");
    }
    sqlx::query("UPDATE metadata SET schema_version = ?")
        .bind(schema_version)
        .execute(&pool)
        .await
        .expect("set fixture schema version");
    pool.close().await;
}

#[tokio::test]
async fn read_only_validation_accepts_each_historical_inp_schema() {
    let root = temp_root("read-only-schema-versions");
    tokio::fs::create_dir_all(&root).await.expect("create root");
    for schema_version in 1..=INP_SCHEMA_VERSION {
        let path = root.join(format!("legacy-v{schema_version}.inp"));
        write_test_inp(
            &path,
            &format!("task-v{schema_version}"),
            &format!("Schema v{schema_version}"),
        )
        .await
        .expect("write fixture");
        downgrade_inp_fixture(&path, schema_version).await;
        validate_inp_file(&path)
            .await
            .unwrap_or_else(|error| panic!("validate schema v{schema_version}: {error}"));

        let migrated = connect_inp(&path).await.expect("migrate validated fixture");
        let migrated_version: i64 =
            sqlx::query_scalar("SELECT schema_version FROM metadata LIMIT 1")
                .fetch_one(&migrated)
                .await
                .expect("read migrated version");
        assert_eq!(migrated_version, INP_SCHEMA_VERSION);
        migrated.close().await;
    }
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn read_only_validation_rejects_missing_current_version_column() {
    let root = temp_root("read-only-schema-required-column");
    let path = root.join("invalid-v2.inp");
    write_test_inp(&path, "task-invalid-v2", "Invalid v2")
        .await
        .expect("write fixture");
    downgrade_inp_fixture(&path, 2).await;
    let pool = connect_sqlite(&path, 1).await.expect("open fixture");
    sqlx::query("ALTER TABLE chunks DROP COLUMN map_json")
        .execute(&pool)
        .await
        .expect("drop required v2 column");
    pool.close().await;
    assert!(validate_inp_file(&path).await.is_err());
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
async fn inp_migration_adds_progress_detail_column_as_null() {
    let root = temp_root("progress-detail-migration");
    let inp_path = root.join("legacy-v7.inp");
    write_test_inp(&inp_path, "task-progress-detail-v7", "Progress Detail")
        .await
        .expect("write inp");
    let pool = connect_inp(&inp_path).await.expect("open inp");
    sqlx::query("ALTER TABLE metadata DROP COLUMN progress_detail_json")
        .execute(&pool)
        .await
        .expect("drop progress detail");
    sqlx::query("UPDATE metadata SET schema_version = 7")
        .execute(&pool)
        .await
        .expect("mark v7");
    pool.close().await;

    let migrated = connect_inp(&inp_path).await.expect("migrate");
    let schema_version: i64 = sqlx::query_scalar("SELECT schema_version FROM metadata LIMIT 1")
        .fetch_one(&migrated)
        .await
        .expect("schema version");
    let progress_detail: Option<String> =
        sqlx::query_scalar("SELECT progress_detail_json FROM metadata LIMIT 1")
            .fetch_one(&migrated)
            .await
            .expect("progress detail");
    assert_eq!(schema_version, INP_SCHEMA_VERSION);
    assert_eq!(progress_detail, None);
    migrated.close().await;

    let task = validate_inp_file(&inp_path)
        .await
        .expect("validate migrated");
    assert!(task.progress_detail.is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn inp_migration_adds_active_retry_column_as_null() {
    let root = temp_root("active-retry-migration");
    let inp_path = root.join("legacy-v8.inp");
    write_test_inp(&inp_path, "task-active-retry-v8", "Active Retry")
        .await
        .expect("write inp");
    let pool = connect_inp(&inp_path).await.expect("open inp");
    sqlx::query("ALTER TABLE metadata DROP COLUMN active_retry_json")
        .execute(&pool)
        .await
        .expect("drop active retry");
    sqlx::query("UPDATE metadata SET schema_version = 8")
        .execute(&pool)
        .await
        .expect("mark v8");
    pool.close().await;

    let migrated = connect_inp(&inp_path).await.expect("migrate");
    let schema_version: i64 = sqlx::query_scalar("SELECT schema_version FROM metadata LIMIT 1")
        .fetch_one(&migrated)
        .await
        .expect("schema version");
    let active_retry: Option<String> =
        sqlx::query_scalar("SELECT active_retry_json FROM metadata LIMIT 1")
            .fetch_one(&migrated)
            .await
            .expect("active retry");
    assert_eq!(schema_version, INP_SCHEMA_VERSION);
    assert_eq!(active_retry, None);
    migrated.close().await;

    let task = validate_inp_file(&inp_path)
        .await
        .expect("validate migrated");
    assert!(task.active_retry.is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn config_migration_adds_task_index_progress_columns() {
    let root = temp_root("task-index-progress-columns");
    let pool = connect_config_db(&root).await.expect("connect config");
    let columns = sqlx::query("PRAGMA table_info(task_index)")
        .fetch_all(&pool)
        .await
        .expect("columns");
    assert!(columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "progress_detail_json"));
    assert!(columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "active_retry_json"));
    pool.close().await;
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
    let confidence: Option<f64> = sqlx::query_scalar("SELECT confidence FROM chunks WHERE id = ?")
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
        kind: ProviderChatErrorKind::HttpStatus,
    };
    assert!(logprobs_parameter_rejected(&error));
}

#[test]
fn retry_backoff_uses_compound_growth_cap_and_jitter() {
    assert_eq!(retry_base_delay_ms(0), 1500);
    assert_eq!(retry_base_delay_ms(1), 2250);
    assert_eq!(retry_base_delay_ms(2), 3375);

    let error = ProviderChatError {
        status: Some(503),
        message: "HTTP 503: overloaded".into(),
        rate_limits: RateLimitTelemetry::default(),
        kind: ProviderChatErrorKind::HttpStatus,
    };
    assert_eq!(transient_retry_base_delay_ms(&error, 8), 12_000);

    let retry_after = ProviderChatError {
        status: Some(429),
        message: "HTTP 429: rate limit".into(),
        rate_limits: RateLimitTelemetry {
            retry_after_ms: Some(2_250),
            ..RateLimitTelemetry::default()
        },
        kind: ProviderChatErrorKind::HttpStatus,
    };
    assert_eq!(transient_retry_base_delay_ms(&retry_after, 0), 2_250);

    for _ in 0..128 {
        let jittered = retry_delay_with_jitter_ms(2_250);
        assert!((1_750..=2_750).contains(&jittered));
        assert!(retry_delay_with_jitter_ms(12_000) <= 12_000);
    }
}

#[tokio::test]
async fn translate_chunk_retries_transient_429_without_interrupting_task() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock provider");
    let address = listener.local_addr().expect("mock address");
    let server = std::thread::spawn(move || {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept mock request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let body = if index == 0 {
                r#"{"error":{"message":"rate limited","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"1ms"}]}}"#.to_string()
            } else {
                r#"{"choices":[{"message":{"content":"你好"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#.to_string()
            };
            let status = if index == 0 {
                "HTTP/1.1 429 Too Many Requests"
            } else {
                "HTTP/1.1 200 OK"
            };
            write!(
                stream,
                "{status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write mock response");
        }
    });
    let adapter = Arc::new(RuntimeAdapter::new(
        Client::new(),
        ProviderRuntimeConfig {
            protocol: ProviderProtocol::OpenaiChat,
            base_url: format!("http://{address}/v1"),
            use_raw_base_url: true,
            config: json!({}),
            auth_type: "bearer".into(),
            auth_header: "Authorization".into(),
            credential: None,
            custom_headers: Vec::new(),
        },
    ));
    let outcome = translate_chunk(
        adapter,
        "test-model".into(),
        "zh-CN".into(),
        None,
        TranslationRequestOptions {
            custom_parameters: json!({}),
            web_search: false,
            thinking: None,
        },
        None,
        None,
        None,
        None,
        Arc::new(TaskGlossaryMatcher::new(Vec::new()).expect("empty glossary")),
        DocumentFormat::Txt,
        ContentFormat::PlainText,
        ChunkRecord {
            id: "chunk-429".into(),
            sequence: 0,
            source_text: "Hello".into(),
            map_json: "{}".into(),
        },
        2,
        ConfidenceMode::Off,
        Arc::new(HeaderQuotaPolicy::new(true)),
        Arc::new(AdaptiveLimiter::new(2, true)),
        None,
        None,
        TranslationInterrupt::new(),
        None,
    )
    .await;
    server.join().expect("mock server joins");

    assert_eq!(outcome.status, TranslationChunkStatus::Success);
    assert!(!outcome.interrupt_task);
    assert_eq!(outcome.retry_count, 1);
    assert_eq!(outcome.translated_text, "你好");
}

#[tokio::test]
async fn translate_chunk_interruption_returns_empty_interrupted_outcome() {
    let adapter = Arc::new(RuntimeAdapter::new(
        Client::new(),
        ProviderRuntimeConfig {
            protocol: ProviderProtocol::OpenaiChat,
            base_url: "http://127.0.0.1:9/v1".into(),
            use_raw_base_url: true,
            config: json!({}),
            auth_type: "bearer".into(),
            auth_header: "Authorization".into(),
            credential: None,
            custom_headers: Vec::new(),
        },
    ));
    let interrupt = TranslationInterrupt::new();
    interrupt.interrupt("Task paused");

    let outcome = translate_chunk(
        adapter,
        "test-model".into(),
        "zh-CN".into(),
        None,
        TranslationRequestOptions {
            custom_parameters: json!({}),
            web_search: false,
            thinking: None,
        },
        None,
        None,
        None,
        None,
        Arc::new(TaskGlossaryMatcher::new(Vec::new()).expect("empty glossary")),
        DocumentFormat::Txt,
        ContentFormat::PlainText,
        ChunkRecord {
            id: "chunk-paused".into(),
            sequence: 0,
            source_text: "Hello".into(),
            map_json: "{}".into(),
        },
        2,
        ConfidenceMode::Off,
        Arc::new(HeaderQuotaPolicy::new(true)),
        Arc::new(AdaptiveLimiter::new(2, true)),
        None,
        None,
        interrupt,
        None,
    )
    .await;

    assert_eq!(outcome.status, TranslationChunkStatus::Interrupted);
    assert_eq!(outcome.after_translate_text, "");
    assert_eq!(outcome.translated_text, "");
    assert_eq!(outcome.error_message.as_deref(), Some("Task paused"));
}

#[tokio::test]
async fn translate_chunk_interrupts_immediately_on_permanent_provider_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock provider");
    let address = listener.local_addr().expect("mock address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept mock request");
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = r#"{"error":{"message":"invalid api key"}}"#;
        write!(
            stream,
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write mock response");
    });
    let adapter = Arc::new(RuntimeAdapter::new(
        Client::new(),
        ProviderRuntimeConfig {
            protocol: ProviderProtocol::OpenaiChat,
            base_url: format!("http://{address}/v1"),
            use_raw_base_url: true,
            config: json!({}),
            auth_type: "bearer".into(),
            auth_header: "Authorization".into(),
            credential: None,
            custom_headers: Vec::new(),
        },
    ));
    let outcome = translate_chunk(
        adapter,
        "test-model".into(),
        "zh-CN".into(),
        None,
        TranslationRequestOptions {
            custom_parameters: json!({}),
            web_search: false,
            thinking: None,
        },
        None,
        None,
        None,
        None,
        Arc::new(TaskGlossaryMatcher::new(Vec::new()).expect("empty glossary")),
        DocumentFormat::Txt,
        ContentFormat::PlainText,
        ChunkRecord {
            id: "chunk-401".into(),
            sequence: 0,
            source_text: "Hello".into(),
            map_json: "{}".into(),
        },
        5,
        ConfidenceMode::Off,
        Arc::new(HeaderQuotaPolicy::new(true)),
        Arc::new(AdaptiveLimiter::new(2, true)),
        None,
        None,
        TranslationInterrupt::new(),
        None,
    )
    .await;
    server.join().expect("mock server joins");

    assert_eq!(outcome.status, TranslationChunkStatus::Failed);
    assert!(outcome.interrupt_task);
    assert_eq!(outcome.retry_count, 0);
    assert!(outcome
        .error_message
        .as_deref()
        .is_some_and(|message| message.contains("HTTP 401")));
}

#[tokio::test]
async fn translate_chunk_marks_transient_exhaustion_failed_without_interrupt() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock provider");
    let address = listener.local_addr().expect("mock address");
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept mock request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let body = r#"{"error":{"message":"overloaded","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"1ms"}]}}"#;
            write!(
                stream,
                "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write mock response");
        }
    });
    let adapter = Arc::new(RuntimeAdapter::new(
        Client::new(),
        ProviderRuntimeConfig {
            protocol: ProviderProtocol::OpenaiChat,
            base_url: format!("http://{address}/v1"),
            use_raw_base_url: true,
            config: json!({}),
            auth_type: "bearer".into(),
            auth_header: "Authorization".into(),
            credential: None,
            custom_headers: Vec::new(),
        },
    ));
    let outcome = translate_chunk(
        adapter,
        "test-model".into(),
        "zh-CN".into(),
        None,
        TranslationRequestOptions {
            custom_parameters: json!({}),
            web_search: false,
            thinking: None,
        },
        None,
        None,
        None,
        None,
        Arc::new(TaskGlossaryMatcher::new(Vec::new()).expect("empty glossary")),
        DocumentFormat::Txt,
        ContentFormat::PlainText,
        ChunkRecord {
            id: "chunk-503".into(),
            sequence: 0,
            source_text: "Hello".into(),
            map_json: "{}".into(),
        },
        2,
        ConfidenceMode::Off,
        Arc::new(HeaderQuotaPolicy::new(true)),
        Arc::new(AdaptiveLimiter::new(2, true)),
        None,
        None,
        TranslationInterrupt::new(),
        None,
    )
    .await;
    server.join().expect("mock server joins");

    assert_eq!(outcome.status, TranslationChunkStatus::Failed);
    assert!(!outcome.interrupt_task);
    assert_eq!(outcome.retry_count, 2);
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
async fn imports_rejects_duplicates_and_updates_metadata_and_index() {
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

    let updated = update_translation_task_info(
        &pool,
        &root,
        UpdateTranslationTaskInfoInput {
            id: imported.id.clone(),
            name: "Tagged Task".into(),
            tags: vec![
                " Batch ".into(),
                "urgent".into(),
                "batch".into(),
                "".into(),
                "校对".into(),
            ],
        },
    )
    .await
    .expect("update task info");
    assert_eq!(updated.name, "Tagged Task");
    assert_eq!(updated.tags, vec!["Batch", "urgent", "校对"]);

    let indexed = get_task_from_index(&pool, &imported.id)
        .await
        .expect("read updated index");
    assert_eq!(indexed.name, "Tagged Task");
    assert_eq!(indexed.tags, vec!["Batch", "urgent", "校对"]);
    let inp_pool = connect_inp(Path::new(&updated.inp_path))
        .await
        .expect("open updated inp");
    let metadata_row = sqlx::query("SELECT name, tags_json FROM metadata LIMIT 1")
        .fetch_one(&inp_pool)
        .await
        .expect("read updated metadata");
    let metadata_name: String = metadata_row.get("name");
    let metadata_tags: String = metadata_row.get("tags_json");
    assert_eq!(metadata_name, "Tagged Task");
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&metadata_tags).expect("parse metadata tags"),
        vec!["Batch", "urgent", "校对"]
    );
    inp_pool.close().await;
    pool.close().await;

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external_root);
}

#[tokio::test]
async fn staged_task_publish_controls_task_index_visibility() {
    let root = temp_root("staged-publish");
    let pool = connect_config_db(&root).await.expect("connect config");
    let staged_path = root.join(TASKS_DIR).join("staged.inp");
    write_test_inp(&staged_path, "task-staged", "Staged Task")
        .await
        .expect("write staged inp");

    let before_publish = list_translation_tasks(&pool, None)
        .await
        .expect("list before publish");
    assert!(before_publish.is_empty());

    let published = publish_staged_translation_task(&pool, &root, "task-staged", &staged_path)
        .await
        .expect("publish staged task");
    assert_eq!(published.id, "task-staged");

    let after_publish = list_translation_tasks(&pool, None)
        .await
        .expect("list after publish");
    assert_eq!(after_publish.len(), 1);
    assert_eq!(after_publish[0].id, "task-staged");

    let discard_path = root.join(TASKS_DIR).join("discarded.inp");
    write_test_inp(&discard_path, "task-discarded", "Discarded Task")
        .await
        .expect("write discarded inp");
    discard_staged_translation_task(&root, &discard_path)
        .await
        .expect("discard staged task");
    assert!(!discard_path.exists());

    let after_discard = list_translation_tasks(&pool, None)
        .await
        .expect("list after discard");
    assert_eq!(after_discard.len(), 1);

    pool.close().await;
    let _ = std::fs::remove_dir_all(root);
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
    let root = std::env::temp_dir().join(format!("insitu-test-{}", app_db::new_id("workspace")));
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
    let root = std::env::temp_dir().join(format!("insitu-test-{}", app_db::new_id("workspace")));
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
            glossary_generation_config: GlossaryGenerationConfig::default(),
            thinking_effort: ThinkingEffort::None,
            use_web_search: false,
            use_custom_parameters: false,
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
    let root = std::env::temp_dir().join(format!("insitu-test-{}", app_db::new_id("workspace")));
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
    let root = std::env::temp_dir().join(format!("insitu-test-{}", app_db::new_id("workspace")));
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
    let root = std::env::temp_dir().join(format!("insitu-test-{}", app_db::new_id("workspace")));
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

#[tokio::test]
async fn atomic_queue_updates_only_config_index_and_leaves_inp_metadata_unchanged() {
    let root = temp_root("atomic-queue");
    let external_root = temp_root("atomic-queue-external");
    let pool = connect_config_db(&root).await.expect("connect config");
    let mut imported = Vec::new();
    for index in 1..=2 {
        let path = external_root.join(format!("task-{index}.inp"));
        let id = format!("task-atomic-{index}");
        write_test_inp(&path, &id, &format!("Atomic {index}"))
            .await
            .expect("write inp");
        imported.push(
            import_translation_task(
                &pool,
                &root,
                ImportTranslationTaskInput {
                    file_path: path.to_string_lossy().to_string(),
                },
            )
            .await
            .expect("import task"),
        );
    }

    let requests = imported
        .iter()
        .map(|task| (task.id.clone(), TranslationTaskStatus::Pending))
        .collect::<Vec<_>>();
    let queued = mark_tasks_queued_atomically(&pool, &root, &requests)
        .await
        .expect("queue tasks");

    assert!(queued
        .iter()
        .all(|task| task.status == TranslationTaskStatus::Queued));
    for task in &imported {
        let inp_pool = connect_inp(Path::new(&task.inp_path))
            .await
            .expect("open inp");
        let row = sqlx::query("SELECT status, queued_from_status FROM metadata LIMIT 1")
            .fetch_one(&inp_pool)
            .await
            .expect("read metadata");
        assert_eq!(row.get::<String, _>("status"), "pending");
        assert_eq!(
            row.try_get::<Option<String>, _>("queued_from_status")
                .unwrap_or(None),
            None
        );
        inp_pool.close().await;
    }
    pool.close().await;
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external_root);
}

#[tokio::test]
async fn retranslation_reset_clears_completed_output_before_queueing() {
    let root = temp_root("retranslation-reset");
    let external_root = temp_root("retranslation-reset-external");
    let pool = connect_config_db(&root).await.expect("connect config");
    let external_path = external_root.join("completed.inp");
    write_test_inp(&external_path, "task-retranslate-reset", "Completed task")
        .await
        .expect("write inp");
    let imported = import_translation_task(
        &pool,
        &root,
        ImportTranslationTaskInput {
            file_path: external_path.to_string_lossy().to_string(),
        },
    )
    .await
    .expect("import task");
    let inp_pool = connect_inp(Path::new(&imported.inp_path))
        .await
        .expect("open inp");
    sqlx::query(
        "UPDATE chunks SET status = ?, after_translate_text = 'restored',
            translated_text = 'translated', retry_count = 2, error_message = 'old error',
            confidence = 0.8, input_tokens = 10, output_tokens = 20, cached_tokens = 3,
            thinking_tokens = 4, total_tokens = 34, target_tokens = 20",
    )
    .bind(TranslationChunkStatus::Success.as_str())
    .execute(&inp_pool)
    .await
    .expect("complete chunks");
    sqlx::query(
        "UPDATE metadata SET status = ?, progress = 1, completed_chunks = 2,
            input_tokens = 20, output_tokens = 40, total_tokens = 60,
            target_text_tokens = 40, total_text_tokens = 50, last_error = 'old error',
            rate_limit_status = 'limited', active_retry_json = '{}',
            global_background = 'old background'",
    )
    .bind(TranslationTaskStatus::Success.as_str())
    .execute(&inp_pool)
    .await
    .expect("complete metadata");
    let completed = metadata_task(&inp_pool, Path::new(&imported.inp_path))
        .await
        .expect("read completed metadata");
    inp_pool.close().await;
    publish_task_index_snapshot(&pool, &completed)
        .await
        .expect("publish completed task");

    let reset = reset_task_for_retranslation(&pool, &root, &imported.id)
        .await
        .expect("reset retranslation");

    assert_eq!(reset.status, TranslationTaskStatus::Pending);
    assert_eq!(reset.progress, 0.0);
    assert_eq!(reset.completed_chunks, 0);
    assert_eq!(reset.failed_chunks, 0);
    assert_eq!(reset.interrupted_chunks, 0);
    assert_eq!(reset.token_stats.total_tokens, 0);
    assert_eq!(reset.text_token_stats.target_tokens, 0);
    assert!(reset.last_error.is_none());
    assert!(reset.rate_limit_status.is_none());
    assert!(reset.active_retry.is_none());

    let reset_inp = connect_inp(Path::new(&imported.inp_path))
        .await
        .expect("reopen inp");
    let chunks = sqlx::query(
        "SELECT status, after_translate_text, translated_text, retry_count, error_message,
            confidence, input_tokens, output_tokens, cached_tokens, thinking_tokens,
            total_tokens, target_tokens FROM chunks",
    )
    .fetch_all(&reset_inp)
    .await
    .expect("read reset chunks");
    assert!(chunks.iter().all(|row| {
        row.get::<String, _>("status") == TranslationChunkStatus::Pending.as_str()
            && row.get::<String, _>("after_translate_text").is_empty()
            && row.get::<String, _>("translated_text").is_empty()
            && row.get::<i64, _>("retry_count") == 0
            && row
                .try_get::<Option<String>, _>("error_message")
                .unwrap_or(None)
                .is_none()
            && row
                .try_get::<Option<f64>, _>("confidence")
                .unwrap_or(None)
                .is_none()
            && row.get::<i64, _>("input_tokens") == 0
            && row.get::<i64, _>("output_tokens") == 0
            && row.get::<i64, _>("cached_tokens") == 0
            && row.get::<i64, _>("thinking_tokens") == 0
            && row.get::<i64, _>("total_tokens") == 0
            && row.get::<i64, _>("target_tokens") == 0
    }));
    let metadata = sqlx::query("SELECT status, global_background FROM metadata LIMIT 1")
        .fetch_one(&reset_inp)
        .await
        .expect("read reset metadata");
    assert_eq!(metadata.get::<String, _>("status"), "pending");
    assert_eq!(
        metadata
            .try_get::<Option<String>, _>("global_background")
            .unwrap_or(None),
        None
    );
    reset_inp.close().await;

    let queued = mark_tasks_queued_atomically(
        &pool,
        &root,
        &[(imported.id.clone(), TranslationTaskStatus::Pending)],
    )
    .await
    .expect("queue reset task");
    assert_eq!(queued[0].status, TranslationTaskStatus::Queued);
    let queue_row = sqlx::query("SELECT queued_from_status FROM task_index WHERE id = ?")
        .bind(&imported.id)
        .fetch_one(&pool)
        .await
        .expect("read queued origin");
    assert_eq!(
        queue_row.get::<String, _>("queued_from_status"),
        TranslationTaskStatus::Pending.as_str()
    );

    pool.close().await;
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external_root);
}

#[tokio::test]
async fn atomic_queue_rolls_back_every_update_when_second_update_aborts() {
    let root = temp_root("atomic-queue-rollback");
    let external_root = temp_root("atomic-queue-rollback-external");
    let pool = connect_config_db(&root).await.expect("connect config");
    let mut imported = Vec::new();
    for index in 1..=2 {
        let path = external_root.join(format!("task-{index}.inp"));
        let id = format!("task-rollback-{index}");
        write_test_inp(&path, &id, &format!("Rollback {index}"))
            .await
            .expect("write inp");
        imported.push(
            import_translation_task(
                &pool,
                &root,
                ImportTranslationTaskInput {
                    file_path: path.to_string_lossy().to_string(),
                },
            )
            .await
            .expect("import task"),
        );
    }
    sqlx::query(
        "CREATE TRIGGER fail_second_queue
         BEFORE UPDATE OF status ON task_index
         WHEN NEW.id = 'task-rollback-2' AND NEW.status = 'queued'
         BEGIN SELECT RAISE(ABORT, 'forced queue failure'); END",
    )
    .execute(&pool)
    .await
    .expect("create failure trigger");

    let requests = imported
        .iter()
        .map(|task| (task.id.clone(), TranslationTaskStatus::Pending))
        .collect::<Vec<_>>();
    assert!(mark_tasks_queued_atomically(&pool, &root, &requests)
        .await
        .is_err());
    for task in &imported {
        let indexed = get_task_from_index(&pool, &task.id)
            .await
            .expect("read task index");
        assert_eq!(indexed.status, TranslationTaskStatus::Pending);
    }
    pool.close().await;
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external_root);
}
