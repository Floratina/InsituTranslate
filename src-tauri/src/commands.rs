use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;

use crate::adapters::{ProviderAdapter, RuntimeAdapter};
use crate::db;
use crate::domain::{
    AddModelInput, AssistantView, ConnectivityResult, CopyAssistantInput, CopyProviderInput,
    CreateAssistantInput, CreateProviderInput, ModelView, ProviderPurpose, ProviderRuntimeConfig,
    ProviderView, RemoteModel, ReorderAssistantsInput, ReorderProvidersInput,
    SetProviderEnabledInput, UnifiedChatRequest, UnifiedChatResponse, UnifiedContent,
    UnifiedMessage, UnifiedToolChoice, UpdateAssistantCustomParametersInput,
    UpdateAssistantPromptInput, UpdateAssistantSettingsInput, UpdateModelInput,
    UpdateProviderConfigInput, UpdateProviderMetadataInput,
};
use crate::glossaries::{
    self, CreateGlossaryEntryInput, DeleteGlossaryEntryInput, ExportGlossaryInput,
    GlossaryEntriesQuery, GlossaryEntryPage, GlossaryEntryView, GlossaryListQuery, GlossaryView,
    ImportGlossaryInput, PrepareAutoGlossaryInput, UpdateGlossaryEntryInput, UpdateGlossaryInput,
};
use crate::translation_tasks::{
    self, CreateTranslationTaskInput, RunMode, TranslationConfigView, TranslationTaskDetail,
    TranslationTaskFilters, TranslationTaskView, UpdateTranslationConfigInput,
    UpdateTranslationTaskTagsInput,
};

pub struct AppState {
    pub pool: SqlitePool,
    pub translation_config_pool: SqlitePool,
    pub translation_workspace_root: PathBuf,
    pub glossary_config_pool: SqlitePool,
    pub glossary_workspace_root: PathBuf,
    pub running_translation_task: Arc<Mutex<Option<String>>>,
    pub client: Client,
}

#[tauri::command]
pub async fn list_assistants(
    state: State<'_, AppState>,
    purpose: ProviderPurpose,
) -> Result<Vec<AssistantView>, String> {
    db::list_assistants(&state.pool, purpose).await
}

#[tauri::command]
pub async fn create_assistant(
    state: State<'_, AppState>,
    input: CreateAssistantInput,
) -> Result<AssistantView, String> {
    db::create_assistant(&state.pool, input).await
}

#[tauri::command]
pub async fn update_assistant_settings(
    state: State<'_, AppState>,
    input: UpdateAssistantSettingsInput,
) -> Result<AssistantView, String> {
    db::update_assistant_settings(&state.pool, input).await
}

#[tauri::command]
pub async fn update_assistant_prompt(
    state: State<'_, AppState>,
    input: UpdateAssistantPromptInput,
) -> Result<AssistantView, String> {
    db::update_assistant_prompt(&state.pool, input).await
}

#[tauri::command]
pub async fn update_assistant_custom_parameters(
    state: State<'_, AppState>,
    input: UpdateAssistantCustomParametersInput,
) -> Result<AssistantView, String> {
    db::update_assistant_custom_parameters(&state.pool, input).await
}

#[tauri::command]
pub async fn reorder_assistants(
    state: State<'_, AppState>,
    input: ReorderAssistantsInput,
) -> Result<Vec<AssistantView>, String> {
    db::reorder_assistants(&state.pool, input).await
}

#[tauri::command]
pub async fn copy_assistant(
    state: State<'_, AppState>,
    input: CopyAssistantInput,
) -> Result<AssistantView, String> {
    db::copy_assistant(&state.pool, input).await
}

#[tauri::command]
pub async fn delete_assistant(state: State<'_, AppState>, id: String) -> Result<(), String> {
    db::delete_assistant(&state.pool, &id).await
}

#[tauri::command]
pub async fn list_providers(
    state: State<'_, AppState>,
    purpose: Option<ProviderPurpose>,
) -> Result<Vec<ProviderView>, String> {
    db::list_providers(&state.pool, purpose).await
}

#[tauri::command]
pub async fn create_provider(
    state: State<'_, AppState>,
    input: CreateProviderInput,
) -> Result<ProviderView, String> {
    db::create_provider(&state.pool, input).await
}

#[tauri::command]
pub async fn update_provider_config(
    state: State<'_, AppState>,
    input: UpdateProviderConfigInput,
) -> Result<ProviderView, String> {
    db::update_provider_config(&state.pool, input).await
}

#[tauri::command]
pub async fn update_provider_metadata(
    state: State<'_, AppState>,
    input: UpdateProviderMetadataInput,
) -> Result<ProviderView, String> {
    db::update_provider_metadata(&state.pool, input).await
}

#[tauri::command]
pub async fn set_provider_enabled(
    state: State<'_, AppState>,
    input: SetProviderEnabledInput,
) -> Result<ProviderView, String> {
    db::set_provider_enabled(&state.pool, input).await
}

#[tauri::command]
pub async fn reorder_providers(
    state: State<'_, AppState>,
    input: ReorderProvidersInput,
) -> Result<Vec<ProviderView>, String> {
    db::reorder_providers(&state.pool, input).await
}

#[tauri::command]
pub async fn copy_provider(
    state: State<'_, AppState>,
    input: CopyProviderInput,
) -> Result<ProviderView, String> {
    db::copy_provider(&state.pool, input).await
}

#[tauri::command]
pub async fn delete_provider(state: State<'_, AppState>, id: String) -> Result<(), String> {
    db::delete_provider(&state.pool, &id).await
}

#[tauri::command]
pub async fn replace_provider_credential(
    state: State<'_, AppState>,
    provider_id: String,
    credential: Option<String>,
) -> Result<ProviderView, String> {
    db::replace_credential(&state.pool, &provider_id, credential).await
}

#[tauri::command]
pub async fn replace_provider_headers(
    state: State<'_, AppState>,
    provider_id: String,
    headers_json: Option<String>,
) -> Result<ProviderView, String> {
    db::replace_headers(&state.pool, &provider_id, headers_json).await
}

#[tauri::command]
pub async fn fetch_provider_models(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<Vec<RemoteModel>, String> {
    let provider = db::get_provider(&state.pool, &provider_id).await?;
    if db::is_mineru_provider(&provider) {
        return Ok(mineru_remote_models(&provider.models));
    }
    let config = db::runtime_config(&state.pool, &provider_id).await?;
    let adapter = RuntimeAdapter::new(state.client.clone(), config);
    let mut remote = adapter.list_models().await?;
    let local = &provider.models;
    for model in &mut remote {
        model.added = local
            .iter()
            .any(|item| item.request_name == model.request_name);
    }
    Ok(remote)
}

#[tauri::command]
pub async fn add_model(
    state: State<'_, AppState>,
    input: AddModelInput,
) -> Result<ModelView, String> {
    if input.source == "manual" {
        validate_manual_model_request_name(&input.request_name)?;
    }
    if input.request_name.trim().is_empty() {
        return Err("Model request name is required".into());
    }
    db::add_model(&state.pool, input).await
}

#[tauri::command]
pub async fn update_model(
    state: State<'_, AppState>,
    input: UpdateModelInput,
) -> Result<ModelView, String> {
    db::update_model(&state.pool, input).await
}

#[tauri::command]
pub async fn delete_model(state: State<'_, AppState>, id: String) -> Result<(), String> {
    db::delete_model(&state.pool, &id).await
}

#[tauri::command]
pub async fn test_model_connectivity(
    state: State<'_, AppState>,
    model_id: String,
) -> Result<ConnectivityResult, String> {
    let model = db::get_model(&state.pool, &model_id).await?;
    let provider = db::get_provider(&state.pool, &model.provider_id).await?;
    let config = db::runtime_config(&state.pool, &model.provider_id).await?;
    if db::is_mineru_provider(&provider) {
        return test_mineru_connectivity(
            &state.client,
            &provider,
            &config,
            &model_id,
            Instant::now(),
            &state.pool,
        )
        .await;
    }
    let adapter = RuntimeAdapter::new(state.client.clone(), config);
    let request = UnifiedChatRequest {
        model: model.request_name,
        messages: vec![UnifiedMessage {
            role: "user".into(),
            content: vec![UnifiedContent::Text {
                text: "Reply with OK.".into(),
            }],
        }],
        tools: Vec::new(),
        tool_choice: UnifiedToolChoice::None,
        thinking: None,
        max_output_tokens: Some(8),
        temperature: Some(0.0),
        stream: false,
    };
    let started = Instant::now();
    let result = adapter.send_chat(&request).await;
    let latency_ms = started.elapsed().as_millis() as i64;
    let tested_at = unix_timestamp();
    let error = result
        .err()
        .map(|value| value.chars().take(500).collect::<String>());
    let success = error.is_none();
    db::update_test_result(
        &state.pool,
        &model_id,
        success,
        latency_ms,
        &tested_at,
        error.as_deref(),
    )
    .await?;
    Ok(ConnectivityResult {
        success,
        latency_ms,
        tested_at,
        error,
    })
}

#[tauri::command]
pub async fn runtime_chat(
    state: State<'_, AppState>,
    provider_id: String,
    request: UnifiedChatRequest,
) -> Result<UnifiedChatResponse, String> {
    let config = db::runtime_config(&state.pool, &provider_id).await?;
    RuntimeAdapter::new(state.client.clone(), config)
        .send_chat(&request)
        .await
}

#[tauri::command]
pub async fn runtime_chat_stream(
    state: State<'_, AppState>,
    provider_id: String,
    request: UnifiedChatRequest,
) -> Result<Vec<UnifiedChatResponse>, String> {
    let config = db::runtime_config(&state.pool, &provider_id).await?;
    RuntimeAdapter::new(state.client.clone(), config)
        .stream_chat(&request)
        .await
}

#[tauri::command]
pub async fn create_translation_task(
    state: State<'_, AppState>,
    input: CreateTranslationTaskInput,
) -> Result<TranslationTaskView, String> {
    translation_tasks::create_translation_task(
        &state.pool,
        &state.translation_config_pool,
        &state.translation_workspace_root,
        input,
    )
    .await
}

#[tauri::command]
pub async fn list_translation_tasks(
    state: State<'_, AppState>,
    filters: Option<TranslationTaskFilters>,
) -> Result<Vec<TranslationTaskView>, String> {
    translation_tasks::list_translation_tasks(&state.translation_config_pool, filters).await
}

#[tauri::command]
pub async fn update_translation_task_tags(
    state: State<'_, AppState>,
    input: UpdateTranslationTaskTagsInput,
) -> Result<TranslationTaskView, String> {
    translation_tasks::update_translation_task_tags(
        &state.translation_config_pool,
        &state.translation_workspace_root,
        input,
    )
    .await
}

#[tauri::command]
pub async fn get_translation_task_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<TranslationTaskDetail, String> {
    translation_tasks::get_translation_task_detail(&state.translation_config_pool, &id).await
}

#[tauri::command]
pub async fn delete_translation_task(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state
        .running_translation_task
        .lock()
        .await
        .as_deref()
        .is_some_and(|running_id| running_id == id)
    {
        return Err("Cannot delete a task while it is running".into());
    }
    translation_tasks::delete_translation_task(
        &state.translation_config_pool,
        &state.translation_workspace_root,
        &id,
    )
    .await
}

#[tauri::command]
pub async fn start_translation_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<TranslationTaskView, String> {
    start_translation_task_with_mode(app, state, id, RunMode::Start).await
}

#[tauri::command]
pub async fn resume_translation_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<TranslationTaskView, String> {
    start_translation_task_with_mode(app, state, id, RunMode::Resume).await
}

#[tauri::command]
pub async fn retranslate_translation_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<TranslationTaskView, String> {
    start_translation_task_with_mode(app, state, id, RunMode::Retranslate).await
}

async fn start_translation_task_with_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    mode: RunMode,
) -> Result<TranslationTaskView, String> {
    {
        let mut running = state.running_translation_task.lock().await;
        if let Some(current) = running.as_ref() {
            return Err(format!("Translation task {current} is already running"));
        }
        *running = Some(id.clone());
    }

    let prepared = match translation_tasks::prepare_translation_run(
        &state.translation_config_pool,
        &state.translation_workspace_root,
        &id,
        mode,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            *state.running_translation_task.lock().await = None;
            return Err(error);
        }
    };
    let view = prepared.task.clone();
    let provider_pool = state.pool.clone();
    let config_pool = state.translation_config_pool.clone();
    let client = state.client.clone();
    let running = state.running_translation_task.clone();
    let task_id = view.id.clone();
    let inp_path = prepared.inp_path.clone();
    tauri::async_runtime::spawn(async move {
        let result = translation_tasks::run_translation_task(
            app,
            provider_pool,
            config_pool.clone(),
            client,
            prepared,
        )
        .await;
        if let Err(error) = result {
            let _ = translation_tasks::mark_task_failed_after_runtime_error(
                &config_pool,
                &inp_path,
                error,
            )
            .await;
        }
        let mut guard = running.lock().await;
        if guard.as_deref() == Some(task_id.as_str()) {
            *guard = None;
        }
    });
    Ok(view)
}

#[tauri::command]
pub async fn get_translation_config(
    state: State<'_, AppState>,
) -> Result<TranslationConfigView, String> {
    translation_tasks::get_translation_config(&state.translation_config_pool).await
}

#[tauri::command]
pub async fn update_translation_config(
    state: State<'_, AppState>,
    input: UpdateTranslationConfigInput,
) -> Result<TranslationConfigView, String> {
    translation_tasks::update_translation_config(&state.translation_config_pool, input).await
}

#[tauri::command]
pub async fn list_glossaries(
    state: State<'_, AppState>,
    query: Option<GlossaryListQuery>,
) -> Result<Vec<GlossaryView>, String> {
    glossaries::list_glossaries(&state.glossary_config_pool, query).await
}

#[tauri::command]
pub async fn import_glossary(
    state: State<'_, AppState>,
    input: ImportGlossaryInput,
) -> Result<GlossaryView, String> {
    glossaries::import_glossary(
        &state.glossary_config_pool,
        &state.glossary_workspace_root,
        input,
    )
    .await
}

#[tauri::command]
pub async fn update_glossary(
    state: State<'_, AppState>,
    input: UpdateGlossaryInput,
) -> Result<GlossaryView, String> {
    glossaries::update_glossary(&state.glossary_config_pool, input).await
}

#[tauri::command]
pub async fn delete_glossary(state: State<'_, AppState>, id: String) -> Result<(), String> {
    glossaries::delete_glossary(
        &state.glossary_config_pool,
        &state.glossary_workspace_root,
        &id,
    )
    .await
}

#[tauri::command]
pub async fn open_glossary_folder(state: State<'_, AppState>, id: String) -> Result<(), String> {
    glossaries::open_glossary_folder(&state.glossary_config_pool, &id).await
}

#[tauri::command]
pub async fn export_glossary(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ExportGlossaryInput,
) -> Result<(), String> {
    glossaries::export_glossary(app, &state.glossary_config_pool, input).await
}

#[tauri::command]
pub async fn get_glossary_entries(
    state: State<'_, AppState>,
    query: GlossaryEntriesQuery,
) -> Result<GlossaryEntryPage, String> {
    glossaries::get_glossary_entries(&state.glossary_config_pool, query).await
}

#[tauri::command]
pub async fn create_glossary_entry(
    state: State<'_, AppState>,
    input: CreateGlossaryEntryInput,
) -> Result<GlossaryEntryView, String> {
    glossaries::create_glossary_entry(&state.glossary_config_pool, input).await
}

#[tauri::command]
pub async fn update_glossary_entry(
    state: State<'_, AppState>,
    input: UpdateGlossaryEntryInput,
) -> Result<GlossaryEntryView, String> {
    glossaries::update_glossary_entry(&state.glossary_config_pool, input).await
}

#[tauri::command]
pub async fn delete_glossary_entry(
    state: State<'_, AppState>,
    input: DeleteGlossaryEntryInput,
) -> Result<(), String> {
    glossaries::delete_glossary_entry(&state.glossary_config_pool, input).await
}

#[tauri::command]
pub async fn prepare_auto_glossary_for_task(
    input: PrepareAutoGlossaryInput,
) -> Result<Option<GlossaryView>, String> {
    glossaries::prepare_auto_glossary_for_task(input).await
}

#[tauri::command]
pub async fn pick_glossary_file(app: AppHandle) -> Result<Option<String>, String> {
    let file_path = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("Glossary", &["csv", "json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    file_path
        .map(|path| {
            let path_buf: PathBuf = path
                .try_into()
                .map_err(|error| format!("Unable to resolve selected file path: {error}"))?;
            Ok(path_buf.to_string_lossy().to_string())
        })
        .transpose()
}

#[tauri::command]
pub async fn pick_translation_files(app: AppHandle) -> Result<Vec<String>, String> {
    let file_paths = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("Text / Markdown", &["txt", "md"])
            .blocking_pick_files()
    })
    .await
    .map_err(|error| error.to_string())?;
    file_paths
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let path_buf: PathBuf = path
                .try_into()
                .map_err(|error| format!("Unable to resolve selected file path: {error}"))?;
            Ok(path_buf.to_string_lossy().to_string())
        })
        .collect()
}

#[tauri::command]
pub async fn detect_source_language(file_paths: Vec<String>) -> Result<Option<String>, String> {
    let mut sample = String::new();
    for path in file_paths.iter().take(3) {
        let content = tokio::fs::read_to_string(path).await.map_err(|error| {
            format!("Unable to read source file for language detection: {error}")
        })?;
        sample.push_str(&content.chars().take(12_000).collect::<String>());
        if sample.chars().count() >= 18_000 {
            break;
        }
    }
    Ok(detect_language_from_text(&sample))
}

fn mineru_remote_models(local: &[ModelView]) -> Vec<RemoteModel> {
    [
        ("vlm", "VLM"),
        ("pipeline", "Pipeline"),
        ("MinerU-HTML", "MinerU HTML"),
    ]
    .into_iter()
    .map(|(request_name, alias)| RemoteModel {
        request_name: request_name.into(),
        alias: alias.into(),
        added: local.iter().any(|item| item.request_name == request_name),
    })
    .collect()
}

fn detect_language_from_text(text: &str) -> Option<String> {
    let mut latin = 0_u32;
    let mut cjk = 0_u32;
    let mut hiragana_katakana = 0_u32;
    let mut hangul = 0_u32;
    let mut cyrillic = 0_u32;
    let mut arabic = 0_u32;
    let mut vietnamese_marks = 0_u32;
    for character in text.chars() {
        if character.is_ascii_alphabetic() {
            latin += 1;
            continue;
        }
        match character {
            '\u{4E00}'..='\u{9FFF}' => cjk += 1,
            '\u{3040}'..='\u{30FF}' => hiragana_katakana += 1,
            '\u{AC00}'..='\u{D7AF}' => hangul += 1,
            '\u{0400}'..='\u{04FF}' => cyrillic += 1,
            '\u{0600}'..='\u{06FF}' => arabic += 1,
            'ă' | 'â' | 'đ' | 'ê' | 'ô' | 'ơ' | 'ư' | 'Ă' | 'Â' | 'Đ' | 'Ê' | 'Ô' | 'Ơ' | 'Ư' => {
                vietnamese_marks += 1
            }
            _ => {}
        }
    }
    let total_signal = latin + cjk + hiragana_katakana + hangul + cyrillic + arabic;
    if total_signal < 8 {
        return None;
    }
    if hiragana_katakana > 0 && hiragana_katakana + cjk >= total_signal / 3 {
        return Some("ja".into());
    }
    if hangul >= total_signal / 5 {
        return Some("ko".into());
    }
    if cjk >= total_signal / 4 {
        return Some("zh-CN".into());
    }
    if cyrillic >= total_signal / 5 {
        return Some("ru".into());
    }
    if arabic >= total_signal / 5 {
        return Some("ar".into());
    }
    if vietnamese_marks >= 2 {
        return Some("vi".into());
    }
    if latin > 0 {
        return Some("en".into());
    }
    None
}

async fn test_mineru_connectivity(
    client: &Client,
    provider: &ProviderView,
    config: &ProviderRuntimeConfig,
    model_id: &str,
    started: Instant,
    pool: &SqlitePool,
) -> Result<ConnectivityResult, String> {
    let mode = db::mineru_mode(&config.config);
    let result = probe_mineru(client, provider, config, mode).await;
    let latency_ms = started.elapsed().as_millis() as i64;
    let tested_at = unix_timestamp();
    let error = result
        .err()
        .map(|value| value.chars().take(500).collect::<String>());
    let success = error.is_none();
    db::update_test_result(
        pool,
        model_id,
        success,
        latency_ms,
        &tested_at,
        error.as_deref(),
    )
    .await?;
    Ok(ConnectivityResult {
        success,
        latency_ms,
        tested_at,
        error,
    })
}

async fn probe_mineru(
    client: &Client,
    provider: &ProviderView,
    config: &ProviderRuntimeConfig,
    mode: &str,
) -> Result<(), String> {
    let (base_url, suffix) = if mode == "flash" {
        (
            db::mineru_flash_base_url(&config.config),
            "parse/__insitu_connectivity_check__",
        )
    } else {
        if config
            .credential
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            return Err("MinerU Standard mode requires an API Key".into());
        }
        (
            provider.base_url.clone(),
            "extract/task/__insitu_connectivity_check__",
        )
    };
    let url = append_endpoint_suffix(&base_url, suffix);
    let mut request = client.get(url);
    if mode != "flash" {
        request = request.headers(mineru_headers(config)?);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            text.chars().take(500).collect::<String>()
        ));
    }
    mineru_probe_result(mode, &text)
}

fn mineru_headers(config: &ProviderRuntimeConfig) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    if let Some(credential) = config
        .credential
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {credential}"))
                .map_err(|error| format!("Invalid MinerU API Key: {error}"))?,
        );
    }
    for (name, value) in &config.custom_headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("Invalid custom header {name}: {error}"))?;
        headers.insert(
            header_name,
            HeaderValue::from_str(value)
                .map_err(|error| format!("Invalid custom header value for {name}: {error}"))?,
        );
    }
    Ok(headers)
}

fn mineru_probe_result(mode: &str, body: &str) -> Result<(), String> {
    let value: Value =
        serde_json::from_str(body).map_err(|error| format!("Invalid MinerU response: {error}"))?;
    let code = value
        .get("code")
        .map(mineru_code_to_string)
        .unwrap_or_default();
    if code == "0" || code == "-60012" || (mode == "flash" && code == "-30004") {
        return Ok(());
    }
    let message = value
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or("MinerU connectivity probe failed");
    if code == "A0202" || code == "A0211" {
        return Err(format!("MinerU API Key is invalid or expired: {message}"));
    }
    Err(format!("MinerU returned {code}: {message}"))
}

fn mineru_code_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => value.to_string(),
    }
}

fn append_endpoint_suffix(base_url: &str, suffix: &str) -> String {
    let base = base_url
        .split('#')
        .next()
        .unwrap_or(base_url)
        .trim()
        .trim_end_matches('/');
    let suffix = suffix.trim_start_matches('/');
    let suffix_path = suffix.split('?').next().unwrap_or(suffix);
    if base.ends_with(suffix) || (!suffix_path.is_empty() && base.ends_with(suffix_path)) {
        base.to_string()
    } else {
        format!("{base}/{suffix}")
    }
}

pub fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .user_agent("InsituTranslate/0.1.0")
        .build()
        .map_err(|error| error.to_string())
}

fn unix_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn validate_manual_model_request_name(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Model request name is required".into());
    }
    let valid = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/+:@-]*$").expect("static regex");
    if !valid.is_match(trimmed) {
        return Err(
            "Custom model request name may only contain letters, numbers, -, ., _, /, :, + and @"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{mineru_probe_result, mineru_remote_models, validate_manual_model_request_name};
    use crate::domain::ModelView;

    #[test]
    fn manual_model_request_name_preserves_common_provider_names() {
        assert!(validate_manual_model_request_name("MyOrg/Model-V2:free").is_ok());
        assert!(validate_manual_model_request_name("gpt-4.1").is_ok());
        assert!(validate_manual_model_request_name("bad model").is_err());
    }

    #[test]
    fn mineru_static_models_mark_local_entries() {
        let local = vec![ModelView {
            id: "model".into(),
            provider_id: "provider".into(),
            request_name: "vlm".into(),
            alias: "VLM".into(),
            source: "builtin".into(),
            capability_reasoning: false,
            capability_web: false,
            capability_tools: false,
            test_status: "untested".into(),
            latency_ms: None,
            tested_at: None,
            test_error: None,
        }];
        let models = mineru_remote_models(&local);
        assert_eq!(models.len(), 3);
        assert!(models
            .iter()
            .any(|model| model.request_name == "vlm" && model.added));
        assert!(models
            .iter()
            .any(|model| model.request_name == "pipeline" && !model.added));
    }

    #[test]
    fn mineru_probe_accepts_task_not_found_as_reachable() {
        assert!(mineru_probe_result(
            "standard",
            r#"{"code":-60012,"msg":"task not found","data":{}}"#
        )
        .is_ok());
        assert!(
            mineru_probe_result("flash", r#"{"code":-30004,"msg":"bad task id","data":{}}"#)
                .is_ok()
        );
        assert!(mineru_probe_result(
            "standard",
            r#"{"code":"A0202","msg":"invalid token","data":{}}"#
        )
        .is_err());
    }
}
