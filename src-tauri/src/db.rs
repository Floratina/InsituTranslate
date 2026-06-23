use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::domain::{
    AddModelInput, AssistantIconKind, AssistantToolMode, AssistantView, CopyAssistantInput,
    CopyProviderInput, CreateAssistantInput, CreateProviderInput,
    ImportVertexAiServiceAccountInput, ModelView, ProviderProtocol, ProviderPurpose,
    ProviderRuntimeConfig, ProviderView, ReorderAssistantsInput, ReorderProvidersInput,
    SetProviderEnabledInput, UpdateAssistantCustomParametersInput, UpdateAssistantPromptInput,
    UpdateAssistantSettingsInput, UpdateModelInput, UpdateProviderConfigInput,
    UpdateProviderMetadataInput, UpdateVertexAiConfigInput,
};
use crate::secrets;
use crate::vertex_ai;

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const MINERU_PROVIDER_ID: &str = "builtin_document-parsing_mineru";
pub const AGENT_PLATFORM_PROVIDER_ID: &str = "builtin_translation_agent_platform";
pub const MINERU_STANDARD_BASE_URL: &str = "https://mineru.net/api/v4";
pub const MINERU_FLASH_BASE_URL: &str = "https://mineru.net/api/v1/agent";

pub fn default_mineru_config() -> Value {
    json!({
        "mineru": {
            "mode": "standard",
            "flashBaseUrl": MINERU_FLASH_BASE_URL,
        }
    })
}

pub fn default_vertex_ai_config() -> Value {
    vertex_ai::default_config()
}

fn default_provider_config(protocol: ProviderProtocol) -> Value {
    match protocol {
        ProviderProtocol::VertexAi => default_vertex_ai_config(),
        _ => json!({}),
    }
}

pub fn is_mineru_provider(provider: &ProviderView) -> bool {
    provider.id == MINERU_PROVIDER_ID || provider.config.get("mineru").is_some()
}

pub fn mineru_mode(config: &Value) -> &'static str {
    match config
        .get("mineru")
        .and_then(|mineru| mineru.get("mode"))
        .and_then(Value::as_str)
    {
        Some("flash") => "flash",
        _ => "standard",
    }
}

pub fn mineru_flash_base_url(config: &Value) -> String {
    config
        .get("mineru")
        .and_then(|mineru| mineru.get("flashBaseUrl"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(MINERU_FLASH_BASE_URL)
        .to_string()
}

pub fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{nanos:x}{counter:x}")
}

pub async fn connect(path: &std::path::Path) -> Result<SqlitePool, String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .map_err(|error| error.to_string())?;
    migrate(&pool).await?;
    seed_default_assistants(&pool).await?;
    migrate_independent_purposes(&pool).await?;
    seed_builtin_providers(&pool).await?;
    migrate_duplicate_builtins(&pool).await?;
    migrate_translation_only_builtins(&pool).await?;
    seed_mineru_builtin_provider(&pool).await?;
    migrate_builtin_disabled_default(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> Result<(), String> {
    let statements = [
        r#"CREATE TABLE IF NOT EXISTS providers (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            protocol TEXT NOT NULL,
            base_url TEXT NOT NULL,
            use_raw_base_url INTEGER NOT NULL DEFAULT 0,
            auth_type TEXT NOT NULL DEFAULT 'bearer',
            auth_header TEXT NOT NULL DEFAULT 'Authorization',
            config_json TEXT NOT NULL DEFAULT '{}',
            enabled INTEGER NOT NULL DEFAULT 1,
            credential_ref TEXT,
            credential_mask TEXT,
            headers_ref TEXT,
            header_keys_json TEXT NOT NULL DEFAULT '[]',
            avatar TEXT,
            is_builtin INTEGER NOT NULL DEFAULT 0,
            sort_order INTEGER NOT NULL DEFAULT 100,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )"#,
        r#"CREATE TABLE IF NOT EXISTS provider_purposes (
            provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
            purpose TEXT NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 100,
            PRIMARY KEY (provider_id, purpose)
        )"#,
        r#"CREATE TABLE IF NOT EXISTS models (
            id TEXT PRIMARY KEY NOT NULL,
            provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
            request_name TEXT NOT NULL,
            alias TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'manual',
            capability_reasoning INTEGER NOT NULL DEFAULT 0,
            capability_web INTEGER NOT NULL DEFAULT 0,
            capability_tools INTEGER NOT NULL DEFAULT 0,
            test_status TEXT NOT NULL DEFAULT 'untested',
            latency_ms INTEGER,
            tested_at TEXT,
            test_error TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(provider_id, request_name)
        )"#,
        r#"CREATE TABLE IF NOT EXISTS assistants (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            icon_kind TEXT NOT NULL DEFAULT 'emoji',
            icon_value TEXT NOT NULL DEFAULT '🤖',
            purpose TEXT NOT NULL,
            system_prompt TEXT NOT NULL DEFAULT '',
            temperature_enabled INTEGER NOT NULL DEFAULT 0,
            temperature REAL NOT NULL DEFAULT 1,
            top_p_enabled INTEGER NOT NULL DEFAULT 0,
            top_p REAL NOT NULL DEFAULT 1,
            tool_mode TEXT NOT NULL DEFAULT 'function',
            max_tool_calls INTEGER NOT NULL DEFAULT 5,
            custom_parameters_json TEXT NOT NULL DEFAULT '{}',
            sort_order INTEGER NOT NULL DEFAULT 100,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )"#,
        "CREATE INDEX IF NOT EXISTS idx_provider_purposes_purpose ON provider_purposes(purpose)",
        "CREATE INDEX IF NOT EXISTS idx_models_provider ON models(provider_id, sort_order, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_assistants_purpose ON assistants(purpose, sort_order, created_at)",
    ];
    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    add_column_if_missing(pool, "providers", "avatar", "TEXT").await?;
    add_column_if_missing(
        pool,
        "providers",
        "use_raw_base_url",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "providers",
        "config_json",
        "TEXT NOT NULL DEFAULT '{}'",
    )
    .await?;
    add_column_if_missing(
        pool,
        "providers",
        "is_builtin",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "providers",
        "sort_order",
        "INTEGER NOT NULL DEFAULT 100",
    )
    .await?;
    add_column_if_missing(
        pool,
        "provider_purposes",
        "sort_order",
        "INTEGER NOT NULL DEFAULT 100",
    )
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS app_metadata (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)",
    )
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn seed_default_assistants(pool: &SqlitePool) -> Result<(), String> {
    let seeded: Option<String> =
        sqlx::query_scalar("SELECT value FROM app_metadata WHERE key = 'assistant-defaults-v1'")
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?;
    if seeded.is_some() {
        return Ok(());
    }

    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    for purpose in [
        ProviderPurpose::Translation,
        ProviderPurpose::Glossary,
        ProviderPurpose::Proofreading,
        ProviderPurpose::DocumentParsing,
    ] {
        sqlx::query(
            "INSERT INTO assistants (id, name, icon_kind, icon_value, purpose, sort_order) VALUES (?, '默认助手', 'emoji', '🤖', ?, 0)",
        )
        .bind(new_id("assistant"))
        .bind(purpose.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    sqlx::query("INSERT INTO app_metadata (key, value) VALUES ('assistant-defaults-v1', 'done')")
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

async fn migrate_independent_purposes(pool: &SqlitePool) -> Result<(), String> {
    let migrated: Option<String> =
        sqlx::query_scalar("SELECT value FROM app_metadata WHERE key = 'independent-purposes-v1'")
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?;
    if migrated.is_some() {
        return Ok(());
    }

    let provider_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM providers")
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    for provider_id in provider_ids {
        let source_name: String = sqlx::query_scalar("SELECT name FROM providers WHERE id = ?")
            .bind(&provider_id)
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())?;
        let purposes: Vec<String> = sqlx::query_scalar(
            "SELECT purpose FROM provider_purposes WHERE provider_id = ? ORDER BY CASE purpose WHEN 'translation' THEN 0 WHEN 'glossary' THEN 1 WHEN 'proofreading' THEN 2 ELSE 3 END",
        )
        .bind(&provider_id)
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        if purposes.len() <= 1 {
            continue;
        }
        for purpose in purposes.iter().skip(1) {
            clone_provider(
                pool,
                &provider_id,
                ProviderPurpose::parse(purpose)?,
                Some(&source_name),
                true,
            )
            .await?;
        }
        sqlx::query("DELETE FROM provider_purposes WHERE provider_id = ? AND purpose != ?")
            .bind(&provider_id)
            .bind(&purposes[0])
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    }
    normalize_purpose_orders(pool).await?;
    sqlx::query("INSERT INTO app_metadata (key, value) VALUES ('independent-purposes-v1', 'done')")
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn seed_builtin_providers(pool: &SqlitePool) -> Result<(), String> {
    let presets = [
        (
            "builtin_openai",
            "OpenAI",
            ProviderProtocol::OpenaiResponses,
            "https://api.openai.com",
            "openai",
            json!({}),
        ),
        (
            "builtin_gemini",
            "Gemini",
            ProviderProtocol::Gemini,
            "https://generativelanguage.googleapis.com",
            "gemini",
            json!({}),
        ),
        (
            "builtin_agent_platform",
            "Agent Platform",
            ProviderProtocol::VertexAi,
            vertex_ai::DEFAULT_BASE_URL,
            "vertex-ai",
            default_vertex_ai_config(),
        ),
        (
            "builtin_anthropic",
            "Anthropic",
            ProviderProtocol::Anthropic,
            "https://api.anthropic.com",
            "anthropic",
            json!({}),
        ),
        (
            "builtin_deepseek",
            "DeepSeek",
            ProviderProtocol::OpenaiChat,
            "https://api.deepseek.com",
            "deepseek",
            json!({}),
        ),
        (
            "builtin_qwen",
            "Qwen",
            ProviderProtocol::OpenaiChat,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen",
            json!({}),
        ),
        (
            "builtin_openrouter",
            "OpenRouter",
            ProviderProtocol::OpenaiChat,
            "https://openrouter.ai/api/v1",
            "openrouter",
            json!({}),
        ),
        (
            "builtin_ollama",
            "Ollama",
            ProviderProtocol::Ollama,
            "http://localhost:11434/api",
            "ollama",
            json!({}),
        ),
    ];
    let mut inserted_any = false;
    let purpose = ProviderPurpose::Translation;
    for (sort_order, (key, name, protocol, base_url, avatar, config)) in presets.iter().enumerate()
    {
        let id = format!(
            "builtin_{}_{}",
            purpose.as_str(),
            key.trim_start_matches("builtin_")
        );
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM providers WHERE id = ?")
            .bind(&id)
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())?;
        if exists == 0 {
            let (auth_type, auth_header) = authentication_for_protocol(*protocol);
            let inserted = sqlx::query(
                "INSERT INTO providers (id, name, protocol, base_url, auth_type, auth_header, config_json, avatar, is_builtin, enabled, sort_order) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, 0, ?) ON CONFLICT(id) DO NOTHING",
            )
            .bind(&id)
            .bind(name)
            .bind(protocol.as_str())
            .bind(base_url)
            .bind(auth_type)
            .bind(auth_header)
            .bind(config.to_string())
            .bind(avatar)
            .bind(sort_order as i64)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
            if inserted.rows_affected() > 0 {
                inserted_any = true;
            }
        }
        let purpose_inserted = sqlx::query(
            "INSERT INTO provider_purposes (provider_id, purpose, sort_order) VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(&id)
        .bind(purpose.as_str())
        .bind(sort_order as i64)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        inserted_any |= purpose_inserted.rows_affected() > 0;
    }
    if inserted_any {
        normalize_purpose_orders(pool).await?;
    }
    Ok(())
}

async fn seed_mineru_builtin_provider(pool: &SqlitePool) -> Result<(), String> {
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM providers WHERE id = ?")
        .bind(MINERU_PROVIDER_ID)
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    let mut inserted_any = false;
    if exists == 0 {
        let (auth_type, auth_header) = authentication_for_protocol(ProviderProtocol::OpenaiChat);
        let inserted = sqlx::query(
            "INSERT INTO providers (id, name, protocol, base_url, use_raw_base_url, auth_type, auth_header, config_json, avatar, is_builtin, enabled, sort_order) VALUES (?, 'MinerU', ?, ?, 1, ?, ?, ?, 'mineru', 1, 0, 0) ON CONFLICT(id) DO NOTHING",
        )
        .bind(MINERU_PROVIDER_ID)
        .bind(ProviderProtocol::OpenaiChat.as_str())
        .bind(MINERU_STANDARD_BASE_URL)
        .bind(auth_type)
        .bind(auth_header)
        .bind(default_mineru_config().to_string())
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        inserted_any |= inserted.rows_affected() > 0;
    }
    let purpose_inserted = sqlx::query(
        "INSERT INTO provider_purposes (provider_id, purpose, sort_order) VALUES (?, ?, 0) ON CONFLICT DO NOTHING",
    )
    .bind(MINERU_PROVIDER_ID)
    .bind(ProviderPurpose::DocumentParsing.as_str())
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    inserted_any |= purpose_inserted.rows_affected() > 0;

    let model_inserted = sqlx::query(
        "INSERT INTO models (id, provider_id, request_name, alias, source, sort_order) VALUES (?, ?, 'vlm', 'VLM', 'builtin', 0) ON CONFLICT(provider_id, request_name) DO NOTHING",
    )
    .bind(new_id("model"))
    .bind(MINERU_PROVIDER_ID)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    inserted_any |= model_inserted.rows_affected() > 0;

    if inserted_any {
        normalize_purpose_orders(pool).await?;
    }
    Ok(())
}

async fn migrate_duplicate_builtins(pool: &SqlitePool) -> Result<(), String> {
    let migrated: Option<String> = sqlx::query_scalar(
        "SELECT value FROM app_metadata WHERE key = 'deduplicate-translation-builtins-v1'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    if migrated.is_some() {
        return Ok(());
    }

    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    for (canonical_id, legacy_id, default_name) in [
        ("builtin_translation_openai", "builtin_openai", "OpenAI"),
        ("builtin_translation_gemini", "builtin_gemini", "Gemini"),
        (
            AGENT_PLATFORM_PROVIDER_ID,
            "builtin_agent_platform",
            "Agent Platform",
        ),
        (
            "builtin_translation_anthropic",
            "builtin_anthropic",
            "Anthropic",
        ),
        (
            "builtin_translation_deepseek",
            "builtin_deepseek",
            "DeepSeek",
        ),
        ("builtin_translation_qwen", "builtin_qwen", "Qwen"),
        (
            "builtin_translation_openrouter",
            "builtin_openrouter",
            "OpenRouter",
        ),
        ("builtin_translation_ollama", "builtin_ollama", "Ollama"),
    ] {
        let duplicate_ids: Vec<String> = sqlx::query_scalar(
            "SELECT p.id FROM providers p
             JOIN provider_purposes pp ON pp.provider_id = p.id
             WHERE pp.purpose = 'translation'
               AND p.is_builtin = 1
               AND p.id != ?
               AND (p.id = ? OR p.name = ?)
             ORDER BY p.created_at",
        )
        .bind(canonical_id)
        .bind(legacy_id)
        .bind(default_name)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;

        for duplicate_id in duplicate_ids {
            sqlx::query(
                "UPDATE providers SET
                    name = source.name,
                    protocol = source.protocol,
                    base_url = source.base_url,
                    use_raw_base_url = source.use_raw_base_url,
                    auth_type = source.auth_type,
                    auth_header = source.auth_header,
                    enabled = source.enabled,
                    credential_ref = source.credential_ref,
                    credential_mask = source.credential_mask,
                    headers_ref = source.headers_ref,
                    header_keys_json = source.header_keys_json,
                    avatar = source.avatar,
                    updated_at = source.updated_at
                 FROM providers AS source
                 WHERE providers.id = ? AND source.id = ?",
            )
            .bind(canonical_id)
            .bind(&duplicate_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
            sqlx::query(
                "UPDATE models SET provider_id = ?
                 WHERE provider_id = ?
                   AND request_name NOT IN (
                       SELECT request_name FROM models WHERE provider_id = ?
                   )",
            )
            .bind(canonical_id)
            .bind(&duplicate_id)
            .bind(canonical_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
            sqlx::query("DELETE FROM providers WHERE id = ?")
                .bind(&duplicate_id)
                .execute(&mut *transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    sqlx::query(
        "INSERT INTO app_metadata (key, value) VALUES ('deduplicate-translation-builtins-v1', 'done')",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    normalize_purpose_orders(pool).await
}

async fn migrate_translation_only_builtins(pool: &SqlitePool) -> Result<(), String> {
    let migrated: Option<String> = sqlx::query_scalar(
        "SELECT value FROM app_metadata WHERE key = 'translation-only-builtins-v1'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    if migrated.is_some() {
        return Ok(());
    }

    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "DELETE FROM providers WHERE is_builtin = 1 AND id != 'builtin_document-parsing_mineru' AND id IN (
            SELECT provider_id FROM provider_purposes WHERE purpose != 'translation'
        )",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;

    let mut ordered_ids: Vec<String> = sqlx::query_scalar(
        "SELECT p.id FROM providers p
         JOIN provider_purposes pp ON pp.provider_id = p.id
         WHERE pp.purpose = 'translation' AND p.is_builtin = 0
         ORDER BY pp.sort_order, p.created_at",
    )
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    for id in [
        "builtin_translation_openai",
        "builtin_translation_gemini",
        AGENT_PLATFORM_PROVIDER_ID,
        "builtin_translation_anthropic",
        "builtin_translation_deepseek",
        "builtin_translation_qwen",
        "builtin_translation_openrouter",
        "builtin_translation_ollama",
    ] {
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provider_purposes WHERE provider_id = ? AND purpose = 'translation'",
        )
        .bind(id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        if exists > 0 {
            ordered_ids.push(id.to_string());
        }
    }
    for (index, id) in ordered_ids.iter().enumerate() {
        sqlx::query(
            "UPDATE provider_purposes SET sort_order = ? WHERE provider_id = ? AND purpose = 'translation'",
        )
        .bind(index as i64)
        .bind(id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    sqlx::query(
        "INSERT INTO app_metadata (key, value) VALUES ('translation-only-builtins-v1', 'done')",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

async fn migrate_builtin_disabled_default(pool: &SqlitePool) -> Result<(), String> {
    let migrated: Option<String> = sqlx::query_scalar(
        "SELECT value FROM app_metadata WHERE key = 'builtin-disabled-default-v1'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    if migrated.is_some() {
        return Ok(());
    }
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query("UPDATE providers SET enabled = 0 WHERE is_builtin = 1")
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO app_metadata (key, value) VALUES ('builtin-disabled-default-v1', 'done')",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

async fn normalize_purpose_orders(pool: &SqlitePool) -> Result<(), String> {
    for purpose in [
        ProviderPurpose::Translation,
        ProviderPurpose::Glossary,
        ProviderPurpose::Proofreading,
        ProviderPurpose::DocumentParsing,
    ] {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT p.id FROM providers p JOIN provider_purposes pp ON pp.provider_id = p.id WHERE pp.purpose = ? ORDER BY pp.sort_order, p.created_at",
        )
        .bind(purpose.as_str())
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        for (index, id) in ids.iter().enumerate() {
            sqlx::query(
                "UPDATE provider_purposes SET sort_order = ? WHERE provider_id = ? AND purpose = ?",
            )
            .bind(index as i64)
            .bind(id)
            .bind(purpose.as_str())
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn default_base_url(protocol: ProviderProtocol) -> &'static str {
    match protocol {
        ProviderProtocol::OpenaiChat | ProviderProtocol::OpenaiResponses => {
            "https://api.openai.com"
        }
        ProviderProtocol::Anthropic => "https://api.anthropic.com",
        ProviderProtocol::Gemini => "https://generativelanguage.googleapis.com",
        ProviderProtocol::VertexAi => vertex_ai::DEFAULT_BASE_URL,
        ProviderProtocol::Ollama => "http://localhost:11434/api",
    }
}

fn authentication_for_protocol(protocol: ProviderProtocol) -> (&'static str, &'static str) {
    match protocol {
        ProviderProtocol::Anthropic => ("api-key", "x-api-key"),
        ProviderProtocol::Gemini => ("api-key", "x-goog-api-key"),
        ProviderProtocol::VertexAi => ("service-account", "Authorization"),
        ProviderProtocol::Ollama => ("none", "Authorization"),
        ProviderProtocol::OpenaiChat | ProviderProtocol::OpenaiResponses => {
            ("bearer", "Authorization")
        }
    }
}

pub async fn list_assistants(
    pool: &SqlitePool,
    purpose: ProviderPurpose,
) -> Result<Vec<AssistantView>, String> {
    let rows =
        sqlx::query("SELECT * FROM assistants WHERE purpose = ? ORDER BY sort_order, created_at")
            .bind(purpose.as_str())
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
    rows.iter().map(assistant_from_row).collect()
}

pub async fn get_assistant(pool: &SqlitePool, id: &str) -> Result<AssistantView, String> {
    let row = sqlx::query("SELECT * FROM assistants WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Assistant not found".to_string())?;
    assistant_from_row(&row)
}

fn assistant_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<AssistantView, String> {
    let custom_parameters_json: String = row.get("custom_parameters_json");
    Ok(AssistantView {
        id: row.get("id"),
        name: row.get("name"),
        icon_kind: AssistantIconKind::parse(row.get::<String, _>("icon_kind").as_str())?,
        icon_value: row.get("icon_value"),
        purpose: ProviderPurpose::parse(row.get::<String, _>("purpose").as_str())?,
        system_prompt: row.get("system_prompt"),
        temperature_enabled: row.get::<i64, _>("temperature_enabled") != 0,
        temperature: row.get("temperature"),
        top_p_enabled: row.get::<i64, _>("top_p_enabled") != 0,
        top_p: row.get("top_p"),
        tool_mode: AssistantToolMode::parse(row.get::<String, _>("tool_mode").as_str())?,
        max_tool_calls: row.get("max_tool_calls"),
        custom_parameters: serde_json::from_str(&custom_parameters_json)
            .unwrap_or_else(|_| json!({})),
    })
}

pub async fn create_assistant(
    pool: &SqlitePool,
    input: CreateAssistantInput,
) -> Result<AssistantView, String> {
    let id = new_id("assistant");
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query("UPDATE assistants SET sort_order = sort_order + 1 WHERE purpose = ?")
        .bind(input.purpose.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO assistants (id, name, icon_kind, icon_value, purpose, sort_order) VALUES (?, '新助手', 'emoji', '🤖', ?, 0)",
    )
    .bind(&id)
    .bind(input.purpose.as_str())
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    get_assistant(pool, &id).await
}

fn validate_assistant_settings(input: &UpdateAssistantSettingsInput) -> Result<(), String> {
    if input.name.trim().is_empty() {
        return Err("Assistant name is required".into());
    }
    if input.icon_value.trim().is_empty() {
        return Err("Assistant icon is required".into());
    }
    if !input.temperature.is_finite() || !(0.0..=2.0).contains(&input.temperature) {
        return Err("Assistant temperature must be between 0 and 2".into());
    }
    if !input.top_p.is_finite() || !(0.0..=1.0).contains(&input.top_p) {
        return Err("Assistant Top-P must be between 0 and 1".into());
    }
    if input.max_tool_calls < 0 {
        return Err("Assistant maximum tool calls must be non-negative".into());
    }
    Ok(())
}

pub async fn update_assistant_settings(
    pool: &SqlitePool,
    input: UpdateAssistantSettingsInput,
) -> Result<AssistantView, String> {
    validate_assistant_settings(&input)?;
    sqlx::query(
        "UPDATE assistants SET name = ?, icon_kind = ?, icon_value = ?, temperature_enabled = ?, temperature = ?, top_p_enabled = ?, top_p = ?, tool_mode = ?, max_tool_calls = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(input.name.trim())
    .bind(input.icon_kind.as_str())
    .bind(input.icon_value.trim())
    .bind(input.temperature_enabled)
    .bind(input.temperature)
    .bind(input.top_p_enabled)
    .bind(input.top_p)
    .bind(input.tool_mode.as_str())
    .bind(input.max_tool_calls)
    .bind(&input.id)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    get_assistant(pool, &input.id).await
}

pub async fn update_assistant_prompt(
    pool: &SqlitePool,
    input: UpdateAssistantPromptInput,
) -> Result<AssistantView, String> {
    sqlx::query(
        "UPDATE assistants SET system_prompt = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(input.system_prompt)
    .bind(&input.id)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    get_assistant(pool, &input.id).await
}

pub async fn update_assistant_custom_parameters(
    pool: &SqlitePool,
    input: UpdateAssistantCustomParametersInput,
) -> Result<AssistantView, String> {
    if !input.custom_parameters.is_object() {
        return Err("Assistant custom parameters must be a JSON object".into());
    }
    sqlx::query(
        "UPDATE assistants SET custom_parameters_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(input.custom_parameters.to_string())
    .bind(&input.id)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    get_assistant(pool, &input.id).await
}

pub async fn reorder_assistants(
    pool: &SqlitePool,
    input: ReorderAssistantsInput,
) -> Result<Vec<AssistantView>, String> {
    let expected_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM assistants WHERE purpose = ?")
            .bind(input.purpose.as_str())
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())?;
    if expected_count != input.assistant_ids.len() as i64 {
        return Err("Assistant order must contain every assistant in the selected purpose".into());
    }
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    for (index, id) in input.assistant_ids.iter().enumerate() {
        let result =
            sqlx::query("UPDATE assistants SET sort_order = ? WHERE id = ? AND purpose = ?")
                .bind(index as i64)
                .bind(id)
                .bind(input.purpose.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(|error| error.to_string())?;
        if result.rows_affected() != 1 {
            return Err("Assistant order contains an item outside the selected purpose".into());
        }
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    list_assistants(pool, input.purpose).await
}

pub async fn copy_assistant(
    pool: &SqlitePool,
    input: CopyAssistantInput,
) -> Result<AssistantView, String> {
    let source = sqlx::query("SELECT * FROM assistants WHERE id = ?")
        .bind(&input.assistant_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Assistant not found".to_string())?;
    let name = next_assistant_copy_name(
        pool,
        source.get::<String, _>("name").as_str(),
        input.purpose,
    )
    .await?;
    let id = new_id("assistant");
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query("UPDATE assistants SET sort_order = sort_order + 1 WHERE purpose = ?")
        .bind(input.purpose.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO assistants (id, name, icon_kind, icon_value, purpose, system_prompt, temperature_enabled, temperature, top_p_enabled, top_p, tool_mode, max_tool_calls, custom_parameters_json, sort_order) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
    )
    .bind(&id)
    .bind(name)
    .bind(source.get::<String, _>("icon_kind"))
    .bind(source.get::<String, _>("icon_value"))
    .bind(input.purpose.as_str())
    .bind(source.get::<String, _>("system_prompt"))
    .bind(source.get::<i64, _>("temperature_enabled"))
    .bind(source.get::<f64, _>("temperature"))
    .bind(source.get::<i64, _>("top_p_enabled"))
    .bind(source.get::<f64, _>("top_p"))
    .bind(source.get::<String, _>("tool_mode"))
    .bind(source.get::<i64, _>("max_tool_calls"))
    .bind(source.get::<String, _>("custom_parameters_json"))
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    get_assistant(pool, &id).await
}

async fn next_assistant_copy_name(
    pool: &SqlitePool,
    source_name: &str,
    purpose: ProviderPurpose,
) -> Result<String, String> {
    for suffix in 1..10_000 {
        let candidate = format!("{source_name}-{suffix:02}");
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM assistants WHERE purpose = ? AND name = ?")
                .bind(purpose.as_str())
                .bind(&candidate)
                .fetch_one(pool)
                .await
                .map_err(|error| error.to_string())?;
        if exists == 0 {
            return Ok(candidate);
        }
    }
    Err("Unable to allocate a copied assistant name".into())
}

pub async fn delete_assistant(pool: &SqlitePool, id: &str) -> Result<(), String> {
    sqlx::query("DELETE FROM assistants WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn list_providers(
    pool: &SqlitePool,
    purpose_filter: Option<ProviderPurpose>,
) -> Result<Vec<ProviderView>, String> {
    let rows = if let Some(purpose) = purpose_filter {
        sqlx::query(
            "SELECT DISTINCT p.* FROM providers p INNER JOIN provider_purposes pp ON pp.provider_id = p.id WHERE pp.purpose = ? ORDER BY pp.sort_order, p.created_at",
        )
        .bind(purpose.as_str())
        .fetch_all(pool)
        .await
    } else {
        sqlx::query("SELECT * FROM providers ORDER BY created_at")
            .fetch_all(pool)
            .await
    }
    .map_err(|error| error.to_string())?;

    let mut providers = Vec::new();
    for row in rows {
        providers.push(provider_from_row(pool, &row).await?);
    }
    Ok(providers)
}

pub async fn get_provider(pool: &SqlitePool, id: &str) -> Result<ProviderView, String> {
    let row = sqlx::query("SELECT * FROM providers WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    provider_from_row(pool, &row).await
}

async fn provider_from_row(
    pool: &SqlitePool,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ProviderView, String> {
    let id: String = row.get("id");
    let purpose_value: String =
        sqlx::query_scalar("SELECT purpose FROM provider_purposes WHERE provider_id = ? LIMIT 1")
            .bind(&id)
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())?;
    let model_rows =
        sqlx::query("SELECT * FROM models WHERE provider_id = ? ORDER BY sort_order, created_at")
            .bind(&id)
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
    let models = model_rows.iter().map(model_from_row).collect();
    let header_keys_json: String = row.get("header_keys_json");
    let config_json: String = row.get("config_json");
    Ok(ProviderView {
        id,
        name: row.get("name"),
        protocol: ProviderProtocol::parse(row.get::<String, _>("protocol").as_str())?,
        base_url: row.get("base_url"),
        use_raw_base_url: row.get::<i64, _>("use_raw_base_url") != 0,
        config: serde_json::from_str(&config_json).unwrap_or_else(|_| json!({})),
        avatar: row.get("avatar"),
        is_builtin: row.get::<i64, _>("is_builtin") != 0,
        enabled: row.get::<i64, _>("enabled") != 0,
        credential_mask: row.get("credential_mask"),
        custom_header_keys: serde_json::from_str(&header_keys_json).unwrap_or_default(),
        purpose: ProviderPurpose::parse(&purpose_value)?,
        models,
    })
}

fn model_from_row(row: &sqlx::sqlite::SqliteRow) -> ModelView {
    ModelView {
        id: row.get("id"),
        provider_id: row.get("provider_id"),
        request_name: row.get("request_name"),
        alias: row.get("alias"),
        source: row.get("source"),
        capability_reasoning: row.get::<i64, _>("capability_reasoning") != 0,
        capability_web: row.get::<i64, _>("capability_web") != 0,
        capability_tools: row.get::<i64, _>("capability_tools") != 0,
        test_status: row.get("test_status"),
        latency_ms: row.get("latency_ms"),
        tested_at: row.get("tested_at"),
        test_error: row.get("test_error"),
    }
}

pub async fn create_provider(
    pool: &SqlitePool,
    input: CreateProviderInput,
) -> Result<ProviderView, String> {
    if input.name.trim().is_empty() {
        return Err("Provider name is required".into());
    }
    let id = new_id("provider");
    let credential_ref = format!("provider/{id}/credential");
    let (auth_type, auth_header) = authentication_for_protocol(input.protocol);
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO providers (id, name, protocol, base_url, auth_type, auth_header, config_json, credential_ref, avatar) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(input.name.trim())
    .bind(input.protocol.as_str())
    .bind(default_base_url(input.protocol))
    .bind(auth_type)
    .bind(auth_header)
    .bind(default_provider_config(input.protocol).to_string())
    .bind(&credential_ref)
    .bind(input.avatar)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query("UPDATE provider_purposes SET sort_order = sort_order + 1 WHERE purpose = ?")
        .bind(input.purpose.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO provider_purposes (provider_id, purpose, sort_order) VALUES (?, ?, 0)",
    )
    .bind(&id)
    .bind(input.purpose.as_str())
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    get_provider(pool, &id).await
}

pub async fn update_provider_config(
    pool: &SqlitePool,
    input: UpdateProviderConfigInput,
) -> Result<ProviderView, String> {
    let (base_url, use_raw_base_url) =
        normalize_provider_base_url(&input.base_url, input.use_raw_base_url)?;
    let config_json = normalize_provider_config(input.config)?;
    let endpoint_base_url = provider_endpoint_base_url(&base_url);
    if endpoint_base_url.trim().is_empty() {
        return Err("Base URL is required".into());
    }
    url::Url::parse(endpoint_base_url.trim())
        .map_err(|_| "Base URL must be a valid absolute URL")?;
    sqlx::query("UPDATE providers SET base_url = ?, use_raw_base_url = ?, config_json = COALESCE(?, config_json), updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(base_url)
        .bind(use_raw_base_url)
        .bind(config_json)
        .bind(&input.id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    get_provider(pool, &input.id).await
}

fn normalize_provider_base_url(value: &str, requested_raw: bool) -> Result<(String, bool), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Base URL is required".into());
    }
    let marker_raw = trimmed.contains('#');
    Ok((trimmed.to_string(), requested_raw || marker_raw))
}

fn provider_endpoint_base_url(value: &str) -> &str {
    value.split('#').next().unwrap_or(value)
}

fn normalize_provider_config(config: Option<Value>) -> Result<Option<String>, String> {
    let Some(value) = config else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(Some(json!({}).to_string()));
    }
    let Value::Object(mut object) = value else {
        return Err("Provider config must be a JSON object".into());
    };
    if object.contains_key("mineru") {
        let mut mineru = match object.remove("mineru") {
            Some(Value::Object(mineru)) => mineru,
            _ => return Err("MinerU config must be a JSON object".into()),
        };
        let mode = mineru
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("standard")
            .to_string();
        if mode != "standard" && mode != "flash" {
            return Err("MinerU mode must be standard or flash".into());
        }
        let flash_base_url = mineru
            .get("flashBaseUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(MINERU_FLASH_BASE_URL)
            .to_string();
        url::Url::parse(provider_endpoint_base_url(&flash_base_url).trim())
            .map_err(|_| "MinerU Flash Base URL must be a valid absolute URL")?;
        mineru.insert("mode".into(), Value::String(mode));
        mineru.insert("flashBaseUrl".into(), Value::String(flash_base_url));
        object.insert("mineru".into(), Value::Object(mineru));
    }
    if object.contains_key(vertex_ai::CONFIG_KEY) {
        let mut vertex = match object.remove(vertex_ai::CONFIG_KEY) {
            Some(Value::Object(vertex)) => vertex,
            _ => return Err("Agent Platform config must be a JSON object".into()),
        };
        let project_id = vertex
            .get("projectId")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let client_email = vertex
            .get("clientEmail")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let location = vertex
            .get("location")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(vertex_ai::DEFAULT_LOCATION)
            .to_string();
        vertex.remove("privateKey");
        vertex.remove("private_key");
        vertex.remove("serviceAccount");
        vertex.insert("projectId".into(), Value::String(project_id));
        vertex.insert("clientEmail".into(), Value::String(client_email));
        vertex.insert("location".into(), Value::String(location));
        object.insert(vertex_ai::CONFIG_KEY.into(), Value::Object(vertex));
    }
    Ok(Some(Value::Object(object).to_string()))
}

pub async fn update_vertex_ai_config(
    pool: &SqlitePool,
    input: UpdateVertexAiConfigInput,
) -> Result<ProviderView, String> {
    save_vertex_ai_config(
        pool,
        &input.provider_id,
        input.project_id,
        input.location,
        input.client_email,
        input.private_key,
    )
    .await
}

pub async fn import_vertex_ai_service_account(
    pool: &SqlitePool,
    input: ImportVertexAiServiceAccountInput,
) -> Result<ProviderView, String> {
    let parsed = vertex_ai::parse_service_account_json(&input.service_account_json)?;
    let existing = get_provider(pool, &input.provider_id).await?;
    let current_vertex = existing
        .config
        .get(vertex_ai::CONFIG_KEY)
        .and_then(Value::as_object);
    let location = input
        .location
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            current_vertex
                .and_then(|vertex| vertex.get("location"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(vertex_ai::DEFAULT_LOCATION)
        .to_string();
    let project_id = if parsed.project_id.trim().is_empty() {
        current_vertex
            .and_then(|vertex| vertex.get("projectId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        parsed.project_id
    };
    save_vertex_ai_config(
        pool,
        &input.provider_id,
        project_id,
        location,
        parsed.client_email,
        Some(parsed.private_key),
    )
    .await
}

pub async fn get_vertex_ai_private_key(
    pool: &SqlitePool,
    provider_id: &str,
) -> Result<Option<String>, String> {
    let row = sqlx::query("SELECT protocol, credential_ref FROM providers WHERE id = ?")
        .bind(provider_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    let protocol = ProviderProtocol::parse(row.get::<String, _>("protocol").as_str())?;
    if protocol != ProviderProtocol::VertexAi {
        return Err("Private key can only be read from Agent Platform providers".into());
    }
    let credential_ref: Option<String> = row.get("credential_ref");
    match credential_ref {
        Some(reference) => secrets::read(&reference),
        None => Ok(None),
    }
}

async fn save_vertex_ai_config(
    pool: &SqlitePool,
    provider_id: &str,
    project_id: String,
    location: String,
    client_email: String,
    private_key: Option<String>,
) -> Result<ProviderView, String> {
    let row = sqlx::query("SELECT protocol, config_json FROM providers WHERE id = ?")
        .bind(provider_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    let protocol = ProviderProtocol::parse(row.get::<String, _>("protocol").as_str())?;
    if protocol != ProviderProtocol::VertexAi {
        return Err("Agent Platform config can only be saved on Agent Platform providers".into());
    }
    let config_json: String = row.get("config_json");
    let mut object = serde_json::from_str::<Value>(&config_json)
        .unwrap_or_else(|_| json!({}))
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut vertex = object
        .remove(vertex_ai::CONFIG_KEY)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    vertex.insert(
        "projectId".into(),
        Value::String(project_id.trim().to_string()),
    );
    let location = location.trim();
    let location = if location.is_empty() {
        vertex_ai::DEFAULT_LOCATION
    } else {
        location
    };
    vertex.insert("location".into(), Value::String(location.to_string()));
    vertex.insert(
        "clientEmail".into(),
        Value::String(client_email.trim().to_string()),
    );
    object.insert(vertex_ai::CONFIG_KEY.into(), Value::Object(vertex));
    let normalized = normalize_provider_config(Some(Value::Object(object)))?
        .unwrap_or_else(|| json!({}).to_string());

    if let Some(private_key) = private_key {
        let reference = format!("provider/{provider_id}/credential");
        let trimmed = private_key.trim();
        let mask = if trimmed.is_empty() {
            secrets::delete(&reference)?;
            None
        } else {
            let formatted = vertex_ai::format_private_key(trimmed)?;
            secrets::write(&reference, &formatted)?;
            Some(secrets::mask(&formatted))
        };
        sqlx::query("UPDATE providers SET config_json = ?, credential_ref = ?, credential_mask = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(normalized)
            .bind(reference)
            .bind(mask)
            .bind(provider_id)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
    } else {
        sqlx::query(
            "UPDATE providers SET config_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(normalized)
        .bind(provider_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    }
    get_provider(pool, provider_id).await
}

pub async fn update_provider_metadata(
    pool: &SqlitePool,
    input: UpdateProviderMetadataInput,
) -> Result<ProviderView, String> {
    if input.name.trim().is_empty() {
        return Err("Provider name is required".into());
    }
    let (auth_type, auth_header) = authentication_for_protocol(input.protocol);
    sqlx::query("UPDATE providers SET name = ?, protocol = ?, auth_type = ?, auth_header = ?, avatar = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(input.name.trim())
        .bind(input.protocol.as_str())
        .bind(auth_type)
        .bind(auth_header)
        .bind(input.avatar)
        .bind(&input.id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    get_provider(pool, &input.id).await
}

pub async fn set_provider_enabled(
    pool: &SqlitePool,
    input: SetProviderEnabledInput,
) -> Result<ProviderView, String> {
    sqlx::query("UPDATE providers SET enabled = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(input.enabled)
        .bind(&input.id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    get_provider(pool, &input.id).await
}

pub async fn delete_provider(pool: &SqlitePool, id: &str) -> Result<(), String> {
    ensure_deletable_provider(pool, id).await?;
    sqlx::query("DELETE FROM providers WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    secrets::delete(&format!("provider/{id}/credential"))?;
    secrets::delete(&format!("provider/{id}/headers"))?;
    Ok(())
}

async fn ensure_deletable_provider(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let is_builtin: i64 = sqlx::query_scalar("SELECT is_builtin FROM providers WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    if is_builtin != 0 {
        return Err("Built-in providers cannot be deleted".into());
    }
    Ok(())
}

pub async fn reorder_providers(
    pool: &SqlitePool,
    input: ReorderProvidersInput,
) -> Result<Vec<ProviderView>, String> {
    let expected_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM provider_purposes WHERE purpose = ?")
            .bind(input.purpose.as_str())
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())?;
    if expected_count != input.provider_ids.len() as i64 {
        return Err("Provider order must contain every provider in the selected purpose".into());
    }
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    for (index, id) in input.provider_ids.iter().enumerate() {
        let result = sqlx::query(
            "UPDATE provider_purposes SET sort_order = ? WHERE provider_id = ? AND purpose = ?",
        )
        .bind(index as i64)
        .bind(id)
        .bind(input.purpose.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        if result.rows_affected() != 1 {
            return Err("Provider order contains an item outside the selected purpose".into());
        }
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    list_providers(pool, Some(input.purpose)).await
}

pub async fn copy_provider(
    pool: &SqlitePool,
    input: CopyProviderInput,
) -> Result<ProviderView, String> {
    clone_provider(pool, &input.provider_id, input.purpose, None, false).await
}

async fn clone_provider(
    pool: &SqlitePool,
    provider_id: &str,
    purpose: ProviderPurpose,
    exact_name: Option<&str>,
    preserve_builtin: bool,
) -> Result<ProviderView, String> {
    let source = sqlx::query("SELECT * FROM providers WHERE id = ?")
        .bind(provider_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    let source_id: String = source.get("id");
    let source_config_json: String = source.get("config_json");
    if is_mineru_provider_record(&source_id, &source_config_json)
        && purpose != ProviderPurpose::DocumentParsing
    {
        return Err("MinerU providers can only be copied to document parsing".into());
    }
    let source_name: String = source.get("name");
    let name = match exact_name {
        Some(value) => value.to_string(),
        None => next_copy_name(pool, &source_name, purpose).await?,
    };
    let id = new_id("provider");
    let credential_ref = format!("provider/{id}/credential");
    let headers_ref = format!("provider/{id}/headers");
    let source_credential_ref: Option<String> = source.get("credential_ref");
    let source_headers_ref: Option<String> = source.get("headers_ref");
    if let Some(secret) = source_credential_ref
        .as_deref()
        .and_then(|reference| secrets::read(reference).ok().flatten())
    {
        secrets::write(&credential_ref, &secret)?;
    }
    if let Some(headers) = source_headers_ref
        .as_deref()
        .and_then(|reference| secrets::read(reference).ok().flatten())
    {
        secrets::write(&headers_ref, &headers)?;
    }
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query("INSERT INTO providers (id, name, protocol, base_url, use_raw_base_url, auth_type, auth_header, config_json, enabled, credential_ref, credential_mask, headers_ref, header_keys_json, avatar, is_builtin) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(name)
        .bind(source.get::<String, _>("protocol"))
        .bind(source.get::<String, _>("base_url"))
        .bind(source.get::<i64, _>("use_raw_base_url"))
        .bind(source.get::<String, _>("auth_type"))
        .bind(source.get::<String, _>("auth_header"))
        .bind(source.get::<String, _>("config_json"))
        .bind(source.get::<i64, _>("enabled"))
        .bind(&credential_ref)
        .bind(source.get::<Option<String>, _>("credential_mask"))
        .bind(&headers_ref)
        .bind(source.get::<String, _>("header_keys_json"))
        .bind(source.get::<Option<String>, _>("avatar"))
        .bind(if preserve_builtin { source.get::<i64, _>("is_builtin") } else { 0 })
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    if exact_name.is_none() {
        sqlx::query("UPDATE provider_purposes SET sort_order = sort_order + 1 WHERE purpose = ?")
            .bind(purpose.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO provider_purposes (provider_id, purpose, sort_order) VALUES (?, ?, 0)",
        )
        .bind(&id)
        .bind(purpose.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    } else {
        sqlx::query("INSERT INTO provider_purposes (provider_id, purpose, sort_order) VALUES (?, ?, COALESCE((SELECT MAX(sort_order) + 1 FROM provider_purposes WHERE purpose = ?), 0))")
            .bind(&id)
            .bind(purpose.as_str())
            .bind(purpose.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
    }
    sqlx::query("INSERT INTO models (id, provider_id, request_name, alias, source, capability_reasoning, capability_web, capability_tools, test_status, sort_order) SELECT 'model_' || lower(hex(randomblob(16))), ?, request_name, alias, source, capability_reasoning, capability_web, capability_tools, 'untested', sort_order FROM models WHERE provider_id = ?")
        .bind(&id)
        .bind(provider_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    get_provider(pool, &id).await
}

fn is_mineru_provider_record(id: &str, config_json: &str) -> bool {
    id == MINERU_PROVIDER_ID
        || serde_json::from_str::<Value>(config_json)
            .ok()
            .and_then(|value| value.get("mineru").cloned())
            .is_some()
}

async fn next_copy_name(
    pool: &SqlitePool,
    source_name: &str,
    purpose: ProviderPurpose,
) -> Result<String, String> {
    for suffix in 1..10_000 {
        let candidate = format!("{source_name}-{suffix:02}");
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM providers p JOIN provider_purposes pp ON pp.provider_id = p.id WHERE pp.purpose = ? AND p.name = ?",
        )
        .bind(purpose.as_str())
        .bind(&candidate)
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
        if exists == 0 {
            return Ok(candidate);
        }
    }
    Err("Unable to allocate a copied provider name".into())
}

pub async fn replace_credential(
    pool: &SqlitePool,
    provider_id: &str,
    credential: Option<String>,
) -> Result<ProviderView, String> {
    let reference = format!("provider/{provider_id}/credential");
    let mask = if let Some(value) = credential.as_deref().filter(|value| !value.is_empty()) {
        secrets::write(&reference, value)?;
        Some(secrets::mask(value))
    } else {
        secrets::delete(&reference)?;
        None
    };
    sqlx::query("UPDATE providers SET credential_ref = ?, credential_mask = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(reference)
        .bind(mask)
        .bind(provider_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    get_provider(pool, provider_id).await
}

pub async fn replace_headers(
    pool: &SqlitePool,
    provider_id: &str,
    headers_json: Option<String>,
) -> Result<ProviderView, String> {
    let reference = format!("provider/{provider_id}/headers");
    let keys = if let Some(json) = headers_json
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let object: serde_json::Map<String, serde_json::Value> = serde_json::from_str(json)
            .map_err(|error| format!("Headers must be a JSON object: {error}"))?;
        let blocked = ["host", "content-length", "transfer-encoding", "connection"];
        let mut keys = Vec::new();
        for (key, value) in &object {
            if blocked.contains(&key.to_lowercase().as_str()) {
                return Err(format!("Header {key} cannot be overridden"));
            }
            if !value.is_string() {
                return Err(format!("Header {key} must have a string value"));
            }
            keys.push(key.clone());
        }
        keys.sort();
        secrets::write(&reference, json)?;
        keys
    } else {
        secrets::delete(&reference)?;
        Vec::new()
    };
    sqlx::query("UPDATE providers SET headers_ref = ?, header_keys_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(reference)
        .bind(serde_json::to_string(&keys).map_err(|error| error.to_string())?)
        .bind(provider_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    get_provider(pool, provider_id).await
}

pub async fn add_model(pool: &SqlitePool, input: AddModelInput) -> Result<ModelView, String> {
    let id = new_id("model");
    sqlx::query("INSERT INTO models (id, provider_id, request_name, alias, source, sort_order) VALUES (?, ?, ?, ?, ?, COALESCE((SELECT MAX(sort_order) + 1 FROM models WHERE provider_id = ?), 0)) ON CONFLICT(provider_id, request_name) DO UPDATE SET alias = excluded.alias")
        .bind(&id)
        .bind(&input.provider_id)
        .bind(input.request_name.trim())
        .bind(if input.alias.trim().is_empty() { input.request_name.trim() } else { input.alias.trim() })
        .bind(input.source)
        .bind(&input.provider_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    let row = sqlx::query("SELECT * FROM models WHERE provider_id = ? AND request_name = ?")
        .bind(input.provider_id)
        .bind(input.request_name.trim())
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(model_from_row(&row))
}

pub async fn update_model(pool: &SqlitePool, input: UpdateModelInput) -> Result<ModelView, String> {
    sqlx::query("UPDATE models SET alias = ?, capability_reasoning = ?, capability_web = ?, capability_tools = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(input.alias.trim())
        .bind(input.capability_reasoning)
        .bind(input.capability_web)
        .bind(input.capability_tools)
        .bind(&input.id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    get_model(pool, &input.id).await
}

pub async fn get_model(pool: &SqlitePool, id: &str) -> Result<ModelView, String> {
    let row = sqlx::query("SELECT * FROM models WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Model not found".to_string())?;
    Ok(model_from_row(&row))
}

pub async fn delete_model(pool: &SqlitePool, id: &str) -> Result<(), String> {
    sqlx::query("DELETE FROM models WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn runtime_config(pool: &SqlitePool, id: &str) -> Result<ProviderRuntimeConfig, String> {
    let row = sqlx::query("SELECT * FROM providers WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    let credential_ref: Option<String> = row.get("credential_ref");
    let headers_ref: Option<String> = row.get("headers_ref");
    let credential = match credential_ref {
        Some(reference) => secrets::read(&reference)?,
        None => None,
    };
    let custom_headers =
        match headers_ref.and_then(|reference| secrets::read(&reference).ok().flatten()) {
            Some(json) => {
                let object: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&json).map_err(|error| error.to_string())?;
                object
                    .into_iter()
                    .filter_map(|(key, value)| value.as_str().map(|text| (key, text.to_string())))
                    .collect()
            }
            None => Vec::new(),
        };
    let protocol = ProviderProtocol::parse(row.get::<String, _>("protocol").as_str())?;
    let (auth_type, auth_header) = authentication_for_protocol(protocol);
    let config_json: String = row.get("config_json");
    Ok(ProviderRuntimeConfig {
        protocol,
        base_url: row.get("base_url"),
        use_raw_base_url: row.get::<i64, _>("use_raw_base_url") != 0,
        config: serde_json::from_str(&config_json).unwrap_or_else(|_| json!({})),
        auth_type: auth_type.into(),
        auth_header: auth_header.into(),
        credential,
        custom_headers,
    })
}

pub async fn update_test_result(
    pool: &SqlitePool,
    id: &str,
    success: bool,
    latency_ms: i64,
    tested_at: &str,
    error: Option<&str>,
) -> Result<(), String> {
    sqlx::query("UPDATE models SET test_status = ?, latency_ms = ?, tested_at = ?, test_error = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(if success { "success" } else { "failed" })
        .bind(latency_ms)
        .bind(tested_at)
        .bind(error)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AddModelInput, AssistantIconKind, AssistantToolMode, CopyAssistantInput, CopyProviderInput,
        CreateAssistantInput, CreateProviderInput, ImportVertexAiServiceAccountInput,
        ProviderProtocol, ProviderPurpose, ReorderAssistantsInput, ReorderProvidersInput,
        UpdateAssistantCustomParametersInput, UpdateAssistantPromptInput,
        UpdateAssistantSettingsInput, UpdateModelInput, UpdateProviderConfigInput,
        UpdateProviderMetadataInput, UpdateVertexAiConfigInput,
    };

    #[tokio::test]
    async fn persists_provider_relations_and_keeps_model_request_name_immutable() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Test".into(),
                protocol: ProviderProtocol::OpenaiChat,
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create provider");
        let model = add_model(
            &pool,
            AddModelInput {
                provider_id: provider.id.clone(),
                request_name: "fixed-model-id".into(),
                alias: "Fixed".into(),
                source: "manual".into(),
            },
        )
        .await
        .expect("add model");
        let updated = update_model(
            &pool,
            UpdateModelInput {
                id: model.id,
                alias: "Renamed".into(),
                capability_reasoning: true,
                capability_web: false,
                capability_tools: true,
            },
        )
        .await
        .expect("update model");
        assert_eq!(updated.request_name, "fixed-model-id");
        assert_eq!(updated.alias, "Renamed");
        delete_provider(&pool, &provider.id)
            .await
            .expect("delete provider");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM models WHERE provider_id = ?")
            .bind(&provider.id)
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 0);
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn seeds_translation_and_mineru_builtins_and_places_new_custom_providers_first() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");

        let translation = list_providers(&pool, Some(ProviderPurpose::Translation))
            .await
            .expect("translation list");
        assert_eq!(
            translation
                .iter()
                .map(|provider| provider.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "OpenAI",
                "Gemini",
                "Agent Platform",
                "Anthropic",
                "DeepSeek",
                "Qwen",
                "OpenRouter",
                "Ollama",
            ]
        );
        for purpose in [ProviderPurpose::Glossary, ProviderPurpose::Proofreading] {
            assert!(
                list_providers(&pool, Some(purpose))
                    .await
                    .expect("non-translation list")
                    .is_empty(),
                "glossary and proofreading must not contain built-in presets"
            );
        }
        let document_parsing = list_providers(&pool, Some(ProviderPurpose::DocumentParsing))
            .await
            .expect("document parsing list");
        assert_eq!(document_parsing.len(), 1);
        assert_eq!(document_parsing[0].id, MINERU_PROVIDER_ID);
        assert_eq!(document_parsing[0].name, "MinerU");
        assert_eq!(document_parsing[0].base_url, MINERU_STANDARD_BASE_URL);
        assert_eq!(mineru_mode(&document_parsing[0].config), "standard");
        assert_eq!(document_parsing[0].models[0].request_name, "vlm");
        assert!(!document_parsing[0].enabled);

        let first = create_provider(
            &pool,
            CreateProviderInput {
                name: "First custom".into(),
                protocol: ProviderProtocol::OpenaiChat,
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create first custom provider");
        let second = create_provider(
            &pool,
            CreateProviderInput {
                name: "Second custom".into(),
                protocol: ProviderProtocol::OpenaiChat,
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create second custom provider");
        let ordered = list_providers(&pool, Some(ProviderPurpose::Translation))
            .await
            .expect("ordered translation list");
        assert_eq!(ordered[0].id, second.id);
        assert_eq!(ordered[1].id, first.id);

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn imports_updates_and_copies_vertex_ai_service_account_config() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let agent_platform = list_providers(&pool, Some(ProviderPurpose::Translation))
            .await
            .expect("providers")
            .into_iter()
            .find(|provider| provider.id == AGENT_PLATFORM_PROVIDER_ID)
            .expect("agent platform provider");

        let imported = import_vertex_ai_service_account(
            &pool,
            ImportVertexAiServiceAccountInput {
                provider_id: agent_platform.id.clone(),
                location: None,
                service_account_json: json!({
                    "project_id": "vertex-project",
                    "client_email": "svc@vertex-project.iam.gserviceaccount.com",
                    "private_key": "abc"
                })
                .to_string(),
            },
        )
        .await
        .expect("import service account");
        assert_eq!(
            imported.config.pointer("/vertexAi/projectId"),
            Some(&json!("vertex-project"))
        );
        assert_eq!(
            imported.config.pointer("/vertexAi/location"),
            Some(&json!("global"))
        );
        assert!(imported.config.pointer("/vertexAi/privateKey").is_none());
        assert!(imported.credential_mask.is_some());

        let runtime = runtime_config(&pool, &imported.id).await.expect("runtime");
        assert_eq!(runtime.protocol, ProviderProtocol::VertexAi);
        assert!(runtime
            .credential
            .as_deref()
            .unwrap_or_default()
            .contains("-----BEGIN PRIVATE KEY-----"));
        assert_eq!(
            get_vertex_ai_private_key(&pool, &imported.id)
                .await
                .expect("private key")
                .as_deref(),
            runtime.credential.as_deref()
        );

        let updated = update_vertex_ai_config(
            &pool,
            UpdateVertexAiConfigInput {
                provider_id: imported.id.clone(),
                project_id: "vertex-project".into(),
                location: "us-central1".into(),
                client_email: "svc@vertex-project.iam.gserviceaccount.com".into(),
                private_key: None,
            },
        )
        .await
        .expect("update vertex config");
        assert_eq!(
            updated.config.pointer("/vertexAi/location"),
            Some(&json!("us-central1"))
        );
        let copied = copy_provider(
            &pool,
            CopyProviderInput {
                provider_id: updated.id.clone(),
                purpose: ProviderPurpose::Translation,
            },
        )
        .await
        .expect("copy provider");
        let copied_runtime = runtime_config(&pool, &copied.id)
            .await
            .expect("copied runtime");
        assert_eq!(copied_runtime.protocol, ProviderProtocol::VertexAi);
        assert!(copied_runtime.credential.is_some());

        let _ = secrets::delete(&format!("provider/{}/credential", imported.id));
        let _ = secrets::delete(&format!("provider/{}/credential", copied.id));
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn deduplicates_legacy_builtins_without_touching_custom_namesakes() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let custom = create_provider(
            &pool,
            CreateProviderInput {
                name: "Qwen".into(),
                protocol: ProviderProtocol::OpenaiChat,
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create custom namesake");
        sqlx::query(
            "INSERT INTO providers (
                id, name, protocol, base_url, auth_type, auth_header, avatar, is_builtin, enabled
             ) VALUES (
                'builtin_qwen', 'Qwen', 'openai-chat', 'https://legacy-qwen.example/v1',
                'bearer', 'Authorization', 'qwen', 1, 1
             )",
        )
        .execute(&pool)
        .await
        .expect("insert legacy built-in");
        sqlx::query(
            "INSERT INTO provider_purposes (provider_id, purpose, sort_order)
             VALUES ('builtin_qwen', 'translation', 50)",
        )
        .execute(&pool)
        .await
        .expect("assign legacy built-in");
        sqlx::query(
            "INSERT INTO models (id, provider_id, request_name, alias, source)
             VALUES ('legacy-qwen-model', 'builtin_qwen', 'qwen-plus', 'Qwen Plus', 'manual')",
        )
        .execute(&pool)
        .await
        .expect("insert legacy model");
        sqlx::query("DELETE FROM app_metadata WHERE key = 'deduplicate-translation-builtins-v1'")
            .execute(&pool)
            .await
            .expect("reset deduplication marker");
        pool.close().await;

        let migrated = connect(&path).await.expect("reconnect and deduplicate");
        let providers = list_providers(&migrated, Some(ProviderPurpose::Translation))
            .await
            .expect("list after deduplication");
        assert!(
            providers.iter().any(|provider| provider.id == custom.id),
            "custom provider with the same name must remain"
        );
        let qwen_builtins = providers
            .iter()
            .filter(|provider| provider.is_builtin && provider.name == "Qwen")
            .collect::<Vec<_>>();
        assert_eq!(qwen_builtins.len(), 1);
        assert_eq!(qwen_builtins[0].id, "builtin_translation_qwen");
        assert_eq!(qwen_builtins[0].base_url, "https://legacy-qwen.example/v1");
        assert_eq!(qwen_builtins[0].models[0].request_name, "qwen-plus");

        migrated.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn copies_orders_and_allows_editing_but_not_deleting_builtin_providers() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Shared".into(),
                protocol: ProviderProtocol::OpenaiChat,
                purpose: ProviderPurpose::Translation,
                avatar: Some("avatar".into()),
            },
        )
        .await
        .expect("create provider");
        add_model(
            &pool,
            AddModelInput {
                provider_id: provider.id.clone(),
                request_name: "shared-model".into(),
                alias: "Shared Model".into(),
                source: "manual".into(),
            },
        )
        .await
        .expect("add model");

        let first_copy = copy_provider(
            &pool,
            CopyProviderInput {
                provider_id: provider.id.clone(),
                purpose: ProviderPurpose::Glossary,
            },
        )
        .await
        .expect("copy provider");
        let second_copy = copy_provider(
            &pool,
            CopyProviderInput {
                provider_id: provider.id.clone(),
                purpose: ProviderPurpose::Glossary,
            },
        )
        .await
        .expect("copy provider again");
        assert_eq!(first_copy.name, "Shared-01");
        assert_eq!(second_copy.name, "Shared-02");
        assert_eq!(first_copy.purpose, ProviderPurpose::Glossary);
        assert_eq!(first_copy.models.len(), 1);
        assert_eq!(first_copy.models[0].test_status, "untested");

        let ordered = reorder_providers(
            &pool,
            ReorderProvidersInput {
                purpose: ProviderPurpose::Glossary,
                provider_ids: vec![second_copy.id.clone(), first_copy.id.clone()],
            },
        )
        .await
        .expect("reorder copied providers");
        assert_eq!(ordered[0].id, second_copy.id);
        assert_eq!(ordered[1].id, first_copy.id);

        let builtin = list_providers(&pool, Some(ProviderPurpose::Translation))
            .await
            .expect("list")
            .into_iter()
            .find(|item| item.is_builtin)
            .expect("builtin");
        assert!(!builtin.enabled);
        let enabled_builtin = set_provider_enabled(
            &pool,
            SetProviderEnabledInput {
                id: builtin.id.clone(),
                enabled: true,
            },
        )
        .await
        .expect("built-in provider can be enabled");
        assert!(enabled_builtin.enabled);
        assert!(delete_provider(&pool, &builtin.id).await.is_err());
        let edited_builtin = update_provider_metadata(
            &pool,
            UpdateProviderMetadataInput {
                id: builtin.id.clone(),
                name: "Changed".into(),
                protocol: ProviderProtocol::OpenaiChat,
                avatar: None,
            },
        )
        .await
        .expect("built-in metadata can be edited");
        assert_eq!(edited_builtin.name, "Changed");
        let configured_builtin = update_provider_config(
            &pool,
            UpdateProviderConfigInput {
                id: builtin.id.clone(),
                base_url: "https://example.com/custom".into(),
                use_raw_base_url: true,
                config: None,
            },
        )
        .await
        .expect("built-in config can be edited");
        assert_eq!(configured_builtin.base_url, "https://example.com/custom");
        assert!(configured_builtin.use_raw_base_url);
        let raw_marker_builtin = update_provider_config(
            &pool,
            UpdateProviderConfigInput {
                id: builtin.id.clone(),
                base_url: "https://example.com/raw/v1/###".into(),
                use_raw_base_url: false,
                config: None,
            },
        )
        .await
        .expect("raw marker can be edited");
        assert_eq!(
            raw_marker_builtin.base_url,
            "https://example.com/raw/v1/###"
        );
        assert!(raw_marker_builtin.use_raw_base_url);

        pool.close().await;
        let reconnected = connect(&path).await.expect("reconnect");
        let builtins = list_providers(&reconnected, Some(ProviderPurpose::Translation))
            .await
            .expect("list after reconnect")
            .into_iter()
            .filter(|provider| provider.is_builtin)
            .collect::<Vec<_>>();
        assert_eq!(
            builtins.len(),
            8,
            "editing a preset must not seed a duplicate"
        );
        assert_eq!(
            builtins
                .iter()
                .find(|provider| provider.id == builtin.id)
                .expect("edited built-in")
                .name,
            "Changed"
        );

        reconnected.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn seeds_and_persists_mineru_document_parsing_builtin() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let mineru = list_providers(&pool, Some(ProviderPurpose::DocumentParsing))
            .await
            .expect("document parsing list")
            .into_iter()
            .find(|provider| provider.id == MINERU_PROVIDER_ID)
            .expect("mineru builtin");
        assert!(mineru.is_builtin);
        assert!(!mineru.enabled);
        assert_eq!(mineru.avatar.as_deref(), Some("mineru"));
        assert_eq!(mineru.models.len(), 1);
        assert_eq!(mineru.models[0].request_name, "vlm");
        assert!(delete_provider(&pool, &mineru.id).await.is_err());

        let enabled = set_provider_enabled(
            &pool,
            SetProviderEnabledInput {
                id: mineru.id.clone(),
                enabled: true,
            },
        )
        .await
        .expect("enable mineru");
        assert!(enabled.enabled);

        let configured = update_provider_config(
            &pool,
            UpdateProviderConfigInput {
                id: mineru.id.clone(),
                base_url: format!("{MINERU_STANDARD_BASE_URL}/"),
                use_raw_base_url: true,
                config: Some(json!({
                    "mineru": {
                        "mode": "flash",
                        "flashBaseUrl": "https://mineru.net/api/v1/agent/"
                    }
                })),
            },
        )
        .await
        .expect("configure mineru");
        assert_eq!(mineru_mode(&configured.config), "flash");
        assert_eq!(configured.base_url, format!("{MINERU_STANDARD_BASE_URL}/"));
        assert_eq!(
            mineru_flash_base_url(&configured.config),
            "https://mineru.net/api/v1/agent/"
        );
        let copy_error = copy_provider(
            &pool,
            CopyProviderInput {
                provider_id: mineru.id.clone(),
                purpose: ProviderPurpose::Translation,
            },
        )
        .await
        .expect_err("mineru cannot be copied to translation");
        assert!(copy_error.contains("document parsing"));

        pool.close().await;
        let reconnected = connect(&path).await.expect("reconnect");
        let document_parsing = list_providers(&reconnected, Some(ProviderPurpose::DocumentParsing))
            .await
            .expect("document parsing after reconnect");
        assert_eq!(document_parsing.len(), 1);
        assert_eq!(document_parsing[0].id, MINERU_PROVIDER_ID);
        assert!(document_parsing[0].enabled);
        assert_eq!(mineru_mode(&document_parsing[0].config), "flash");
        assert_eq!(document_parsing[0].models.len(), 1);

        reconnected.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn migrates_shared_provider_into_independent_purpose_records_once() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Legacy Shared".into(),
                protocol: ProviderProtocol::OpenaiChat,
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create provider");
        sqlx::query(
            "INSERT INTO provider_purposes (provider_id, purpose, sort_order) VALUES (?, 'glossary', 100)",
        )
        .bind(&provider.id)
        .execute(&pool)
        .await
        .expect("add legacy purpose");
        sqlx::query("DELETE FROM app_metadata WHERE key = 'independent-purposes-v1'")
            .execute(&pool)
            .await
            .expect("reset migration marker");
        pool.close().await;

        let migrated = connect(&path).await.expect("reconnect and migrate");
        let translation = list_providers(&migrated, Some(ProviderPurpose::Translation))
            .await
            .expect("translation list")
            .into_iter()
            .find(|item| item.name == "Legacy Shared")
            .expect("translation copy");
        let glossary = list_providers(&migrated, Some(ProviderPurpose::Glossary))
            .await
            .expect("glossary list")
            .into_iter()
            .find(|item| item.name == "Legacy Shared")
            .expect("glossary copy");
        assert_ne!(translation.id, glossary.id);

        migrated.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn disables_builtins_only_once_and_preserves_later_user_choice() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let builtin = list_providers(&pool, Some(ProviderPurpose::Translation))
            .await
            .expect("list")
            .into_iter()
            .find(|item| item.is_builtin)
            .expect("builtin");
        assert!(!builtin.enabled);
        set_provider_enabled(
            &pool,
            SetProviderEnabledInput {
                id: builtin.id.clone(),
                enabled: true,
            },
        )
        .await
        .expect("enable built-in");
        pool.close().await;

        let reconnected = connect(&path).await.expect("reconnect");
        let enabled: bool = list_providers(&reconnected, Some(ProviderPurpose::Translation))
            .await
            .expect("list after reconnect")
            .into_iter()
            .find(|item| item.id == builtin.id)
            .expect("same builtin")
            .enabled;
        assert!(enabled, "one-time migration must not overwrite user choice");
        reconnected.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn seeds_default_assistants_once_and_preserves_deletions() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        for purpose in [
            ProviderPurpose::Translation,
            ProviderPurpose::Glossary,
            ProviderPurpose::Proofreading,
            ProviderPurpose::DocumentParsing,
        ] {
            let assistants = list_assistants(&pool, purpose)
                .await
                .expect("list assistants");
            assert_eq!(assistants.len(), 1);
            assert_eq!(assistants[0].name, "默认助手");
            assert_eq!(assistants[0].icon_kind, AssistantIconKind::Emoji);
            assert_eq!(assistants[0].icon_value, "🤖");
            assert!(!assistants[0].temperature_enabled);
            assert_eq!(assistants[0].temperature, 1.0);
            assert!(!assistants[0].top_p_enabled);
            assert_eq!(assistants[0].top_p, 1.0);
            assert_eq!(assistants[0].tool_mode, AssistantToolMode::Function);
            assert_eq!(assistants[0].max_tool_calls, 5);
            assert_eq!(assistants[0].custom_parameters, json!({}));
        }
        let translation = list_assistants(&pool, ProviderPurpose::Translation)
            .await
            .expect("translation");
        delete_assistant(&pool, &translation[0].id)
            .await
            .expect("delete default");
        pool.close().await;

        let reconnected = connect(&path).await.expect("reconnect");
        assert!(list_assistants(&reconnected, ProviderPurpose::Translation)
            .await
            .expect("translation after reconnect")
            .is_empty());
        reconnected.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn persists_copies_orders_and_validates_assistant_settings() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let created = create_assistant(
            &pool,
            CreateAssistantInput {
                purpose: ProviderPurpose::Translation,
            },
        )
        .await
        .expect("create assistant");
        let updated = update_assistant_settings(
            &pool,
            UpdateAssistantSettingsInput {
                id: created.id.clone(),
                name: "Translator".into(),
                icon_kind: AssistantIconKind::Lucide,
                icon_value: "languages".into(),
                temperature_enabled: true,
                temperature: 0.7,
                top_p_enabled: true,
                top_p: 0.9,
                tool_mode: AssistantToolMode::Prompt,
                max_tool_calls: 0,
            },
        )
        .await
        .expect("update settings");
        assert_eq!(updated.max_tool_calls, 0);
        update_assistant_prompt(
            &pool,
            UpdateAssistantPromptInput {
                id: created.id.clone(),
                system_prompt: "Translate precisely.".into(),
            },
        )
        .await
        .expect("update prompt");
        let with_custom = update_assistant_custom_parameters(
            &pool,
            UpdateAssistantCustomParametersInput {
                id: created.id.clone(),
                custom_parameters: json!({"service_tier": "flex"}),
            },
        )
        .await
        .expect("update custom parameters");
        assert_eq!(with_custom.name, "Translator");
        assert_eq!(with_custom.temperature, 0.7);
        assert_eq!(with_custom.system_prompt, "Translate precisely.");
        assert_eq!(
            with_custom.custom_parameters,
            json!({"service_tier": "flex"})
        );

        let copied = copy_assistant(
            &pool,
            CopyAssistantInput {
                assistant_id: created.id.clone(),
                purpose: ProviderPurpose::Glossary,
            },
        )
        .await
        .expect("copy assistant");
        assert_eq!(copied.name, "Translator-01");
        assert_eq!(copied.purpose, ProviderPurpose::Glossary);
        assert_eq!(copied.system_prompt, "Translate precisely.");
        assert_eq!(copied.tool_mode, AssistantToolMode::Prompt);
        assert_eq!(copied.custom_parameters, json!({"service_tier": "flex"}));

        let translation = list_assistants(&pool, ProviderPurpose::Translation)
            .await
            .expect("translation assistants");
        let reversed = translation
            .iter()
            .rev()
            .map(|assistant| assistant.id.clone())
            .collect::<Vec<_>>();
        let ordered = reorder_assistants(
            &pool,
            ReorderAssistantsInput {
                purpose: ProviderPurpose::Translation,
                assistant_ids: reversed.clone(),
            },
        )
        .await
        .expect("reorder assistants");
        assert_eq!(
            ordered
                .iter()
                .map(|assistant| assistant.id.clone())
                .collect::<Vec<_>>(),
            reversed
        );

        assert!(update_assistant_settings(
            &pool,
            UpdateAssistantSettingsInput {
                id: created.id.clone(),
                name: String::new(),
                icon_kind: AssistantIconKind::Emoji,
                icon_value: "🤖".into(),
                temperature_enabled: true,
                temperature: 2.1,
                top_p_enabled: true,
                top_p: 1.1,
                tool_mode: AssistantToolMode::Function,
                max_tool_calls: 5,
            },
        )
        .await
        .is_err());
        assert!(update_assistant_settings(
            &pool,
            UpdateAssistantSettingsInput {
                id: created.id.clone(),
                name: "Valid".into(),
                icon_kind: AssistantIconKind::Emoji,
                icon_value: "🤖".into(),
                temperature_enabled: false,
                temperature: 1.0,
                top_p_enabled: false,
                top_p: 1.0,
                tool_mode: AssistantToolMode::Function,
                max_tool_calls: -1,
            },
        )
        .await
        .is_err());
        assert!(update_assistant_custom_parameters(
            &pool,
            UpdateAssistantCustomParametersInput {
                id: created.id,
                custom_parameters: json!([]),
            },
        )
        .await
        .is_err());

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }
}
