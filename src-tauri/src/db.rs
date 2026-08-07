use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderName, HeaderValue};
use serde::de::{Error as DeError, MapAccess, Visitor};
use serde::Deserializer as _;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::domain::{
    AddModelInput, AssistantIconKind, AssistantView, CopyAssistantInput, CopyProviderInput,
    CreateAssistantInput, CreateProviderInput, ImportVertexAiServiceAccountInput, ModelView,
    ProtocolId, ProtocolStatus, ProviderPurpose, ProviderRuntimeConfig, ProviderView,
    ReorderAssistantsInput, ReorderProvidersInput, RepairProviderProtocolInput,
    SetProviderEnabledInput, UpdateAssistantCustomParametersInput, UpdateAssistantPromptInput,
    UpdateAssistantSettingsInput, UpdateModelInput, UpdateProviderConfigInput,
    UpdateProviderMetadataInput, UpdateVertexAiConfigInput,
};
use crate::providers::capabilities::{
    infer_capabilities, resolve_capabilities, CapabilityId, CapabilityOverridePolicy,
    CapabilityOverrides, CapabilityValue, ModelCapabilities,
};
use crate::providers::config_schema::{config_issues, materialize_defaults, validated_config};
use crate::providers::registry::{
    descriptor_by_id, resolve_input, resolve_persisted, ProtocolDescriptor, ProtocolResolution,
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

fn default_provider_config(descriptor: &ProtocolDescriptor) -> Result<Value, String> {
    let config = match descriptor.config_kind {
        "vertex-ai" => default_vertex_ai_config(),
        _ => json!({}),
    };
    materialize_defaults(config, descriptor.config_fields)
}

fn provider_config_with_defaults(
    descriptor: &ProtocolDescriptor,
    config: Value,
    mineru: bool,
) -> Result<Value, String> {
    let Value::Object(mut object) = config else {
        return Err("Provider config must be a JSON object".into());
    };
    let defaults = if mineru {
        default_mineru_config()
    } else {
        match descriptor.config_kind {
            "vertex-ai" => default_vertex_ai_config(),
            _ => json!({}),
        }
    };
    merge_missing_object_values(&mut object, defaults)?;
    materialize_defaults(Value::Object(object), descriptor.config_fields)
}

fn merge_missing_object_values(
    target: &mut serde_json::Map<String, Value>,
    defaults: Value,
) -> Result<(), String> {
    let Value::Object(defaults) = defaults else {
        return Err("Provider config defaults must be a JSON object".into());
    };
    for (key, default) in defaults {
        match (target.get_mut(&key), default) {
            (Some(Value::Object(current)), Value::Object(nested)) => {
                merge_missing_object_values(current, Value::Object(nested))?;
            }
            (Some(_), _) => {}
            (None, default) => {
                target.insert(key, default);
            }
        }
    }
    Ok(())
}

fn parse_provider_config(id: &str, raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw)
        .map_err(|error| format!("Provider {id} config JSON is invalid: {error}"))
}

fn parse_header_keys(id: &str, raw: &str) -> Result<Vec<String>, String> {
    serde_json::from_str(raw)
        .map_err(|error| format!("Provider {id} header key JSON is invalid: {error}"))
}

#[derive(Debug)]
struct ValidatedHeaders {
    keys: Vec<String>,
    values: Vec<(String, String)>,
}

struct HeaderObjectVisitor;

impl<'de> Visitor<'de> for HeaderObjectVisitor {
    type Value = ValidatedHeaders;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object whose header values are strings")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut values = Vec::new();
        while let Some((name, value)) = map.next_entry::<String, Value>()? {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                A::Error::custom(format!("Header {name} has an invalid name: {error}"))
            })?;
            let normalized = header_name.as_str().to_string();
            if !seen.insert(normalized.clone()) {
                return Err(A::Error::custom(format!(
                    "Header {name} duplicates another header name (case-insensitive)"
                )));
            }
            if FORBIDDEN_CUSTOM_HEADERS.contains(&normalized.as_str()) {
                return Err(A::Error::custom(format!(
                    "Header {name} cannot be overridden"
                )));
            }
            let value = value.as_str().ok_or_else(|| {
                A::Error::custom(format!("Header {name} must have a string value"))
            })?;
            HeaderValue::from_str(value).map_err(|error| {
                A::Error::custom(format!("Header {name} has an invalid value: {error}"))
            })?;
            values.push((name, value.to_string()));
        }
        let mut keys = values
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        keys.sort();
        Ok(ValidatedHeaders { keys, values })
    }
}

const FORBIDDEN_CUSTOM_HEADERS: &[&str] = &[
    "content-type",
    "host",
    "content-length",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
];

fn parse_and_validate_headers_json(raw: &str) -> Result<ValidatedHeaders, String> {
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let headers = deserializer
        .deserialize_map(HeaderObjectVisitor)
        .map_err(|error| format!("Headers must be a JSON object: {error}"))?;
    deserializer
        .end()
        .map_err(|error| format!("Headers must contain exactly one JSON object: {error}"))?;
    Ok(headers)
}

#[derive(Debug)]
struct SecretMutation {
    reference: String,
    previous: Option<String>,
}

impl SecretMutation {
    fn apply(reference: String, replacement: Option<&str>) -> Result<Self, String> {
        let previous = secrets::read(&reference)?;
        match replacement {
            Some(value) => secrets::write(&reference, value)?,
            None => secrets::delete(&reference)?,
        }
        Ok(Self {
            reference,
            previous,
        })
    }

    fn restore(&self) -> Result<(), String> {
        match self.previous.as_deref() {
            Some(value) => secrets::write(&self.reference, value),
            None => secrets::delete(&self.reference),
        }
    }
}

fn secret_database_error(
    provider_id: &str,
    database_error: String,
    mutation: &SecretMutation,
) -> String {
    match mutation.restore() {
        Ok(()) => format!("Provider {provider_id} database update failed: {database_error}"),
        Err(restore_error) => format!(
            "Provider {provider_id} database update failed: {database_error}; secret restoration failed: {restore_error}"
        ),
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
    backfill_model_capabilities(&pool).await?;
    migrate_model_capability_overrides(&pool).await?;
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
        r#"CREATE TABLE IF NOT EXISTS model_capability_overrides (
            model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
            capability_id TEXT NOT NULL,
            value_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (model_id, capability_id)
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
        "CREATE INDEX IF NOT EXISTS idx_model_capability_overrides_model ON model_capability_overrides(model_id)",
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
    add_column_if_missing(
        pool,
        "models",
        "capability_reasoning",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "models",
        "capability_web",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    add_column_if_missing(
        pool,
        "models",
        "capability_tools",
        "INTEGER NOT NULL DEFAULT 0",
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
            "openai-responses",
            "https://api.openai.com",
            "openai",
            json!({}),
        ),
        (
            "builtin_gemini",
            "Gemini",
            "gemini",
            "https://generativelanguage.googleapis.com",
            "gemini",
            json!({}),
        ),
        (
            "builtin_agent_platform",
            "Agent Platform",
            "vertex-ai",
            vertex_ai::DEFAULT_BASE_URL,
            "vertex-ai",
            default_vertex_ai_config(),
        ),
        (
            "builtin_anthropic",
            "Anthropic",
            "anthropic",
            "https://api.anthropic.com",
            "anthropic",
            json!({}),
        ),
        (
            "builtin_deepseek",
            "DeepSeek",
            "openai-chat",
            "https://api.deepseek.com",
            "deepseek",
            json!({}),
        ),
        (
            "builtin_qwen",
            "Qwen",
            "openai-chat",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen",
            json!({}),
        ),
        (
            "builtin_openrouter",
            "OpenRouter",
            "openai-chat",
            "https://openrouter.ai/api/v1",
            "openrouter",
            json!({}),
        ),
        (
            "builtin_ollama",
            "Ollama",
            "ollama",
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
            let descriptor = descriptor_by_id(protocol)?.ok_or_else(|| {
                format!("Builtin provider protocol is not registered: {protocol}")
            })?;
            let (auth_type, auth_header) = authentication_for_protocol(descriptor);
            let config = provider_config_with_defaults(descriptor, config.clone(), false)?;
            let inserted = sqlx::query(
                "INSERT INTO providers (id, name, protocol, base_url, auth_type, auth_header, config_json, avatar, is_builtin, enabled, sort_order) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, 0, ?) ON CONFLICT(id) DO NOTHING",
            )
            .bind(&id)
            .bind(name)
            .bind(protocol)
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
        let descriptor = descriptor_by_id("openai-chat")?
            .ok_or_else(|| "MinerU protocol is not registered: openai-chat".to_string())?;
        let (auth_type, auth_header) = authentication_for_protocol(descriptor);
        let inserted = sqlx::query(
            "INSERT INTO providers (id, name, protocol, base_url, use_raw_base_url, auth_type, auth_header, config_json, avatar, is_builtin, enabled, sort_order) VALUES (?, 'MinerU', ?, ?, 1, ?, ?, ?, 'mineru', 1, 0, 0) ON CONFLICT(id) DO NOTHING",
        )
        .bind(MINERU_PROVIDER_ID)
        .bind(descriptor.id)
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

async fn backfill_model_capabilities(pool: &SqlitePool) -> Result<(), String> {
    let migrated: Option<String> = sqlx::query_scalar(
        "SELECT value FROM app_metadata WHERE key = 'model-capability-backfill-v1'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    if migrated.is_some() {
        return Ok(());
    }

    let rows = sqlx::query(
        "SELECT m.id, m.request_name, m.capability_reasoning, m.capability_web,
                p.protocol, p.base_url
         FROM models m
         JOIN providers p ON p.id = m.provider_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    for row in rows {
        let raw_protocol: String = row.get("protocol");
        let ProtocolResolution::Known(descriptor) = resolve_persisted(&raw_protocol)? else {
            eprintln!(
                "Skipping model capability backfill for unknown provider protocol: {raw_protocol}"
            );
            continue;
        };
        let request_name: String = row.get("request_name");
        let inferred = infer_capabilities(
            descriptor.capability_profile,
            row.get::<String, _>("base_url").as_str(),
            &request_name,
        );
        let legacy = legacy_model_capabilities(&row)?;
        let capability_reasoning = legacy.reasoning() || inferred.reasoning();
        let capability_web = legacy.web() || inferred.web();
        sqlx::query(
            "UPDATE models
             SET capability_reasoning = ?, capability_web = ?,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?",
        )
        .bind(capability_reasoning)
        .bind(capability_web)
        .bind(row.get::<String, _>("id"))
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    sqlx::query(
        "INSERT INTO app_metadata (key, value)
         VALUES ('model-capability-backfill-v1', 'done')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn migrate_model_capability_overrides(pool: &SqlitePool) -> Result<(), String> {
    let migrated: Option<String> = sqlx::query_scalar(
        "SELECT value FROM app_metadata WHERE key = 'model-capability-overrides-v1'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    if migrated.is_some() {
        return Ok(());
    }

    let rows = sqlx::query(
        "SELECT m.id, m.request_name, m.capability_reasoning, m.capability_web,
                p.protocol, p.base_url
         FROM models m
         JOIN providers p ON p.id = m.provider_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    for row in rows {
        let raw_protocol: String = row.get("protocol");
        let ProtocolResolution::Known(descriptor) = resolve_persisted(&raw_protocol)? else {
            eprintln!(
                "Skipping model capability override migration for unknown protocol: {raw_protocol}"
            );
            continue;
        };
        let model_id: String = row.get("id");
        let request_name: String = row.get("request_name");
        let base_url: String = row.get("base_url");
        let inferred = infer_capabilities(descriptor.capability_profile, &base_url, &request_name);
        for capability in CapabilityId::REGISTERED
            .iter()
            .copied()
            .filter(|capability| {
                capability.definition().override_policy == CapabilityOverridePolicy::User
            })
        {
            let legacy_value = legacy_boolean_capability(&row, capability)?;
            let CapabilityValue::Boolean(inferred_value) = inferred.get(capability) else {
                return Err(format!(
                    "Legacy capability {} must be registered as a boolean",
                    capability.as_str()
                ));
            };
            if legacy_value != *inferred_value {
                sqlx::query(
                    "INSERT INTO model_capability_overrides
                     (model_id, capability_id, value_json)
                     VALUES (?, ?, ?)
                     ON CONFLICT(model_id, capability_id) DO NOTHING",
                )
                .bind(&model_id)
                .bind(capability.as_str())
                .bind(if legacy_value { "true" } else { "false" })
                .execute(&mut *transaction)
                .await
                .map_err(|error| error.to_string())?;
            }
        }
    }
    sqlx::query(
        "INSERT INTO app_metadata (key, value)
         VALUES ('model-capability-overrides-v1', 'done')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn legacy_boolean_capability(
    row: &sqlx::sqlite::SqliteRow,
    capability: CapabilityId,
) -> Result<bool, String> {
    match capability {
        CapabilityId::REASONING => Ok(row.get::<i64, _>("capability_reasoning") != 0),
        CapabilityId::WEB => Ok(row.get::<i64, _>("capability_web") != 0),
        _ => Err(format!(
            "Capability {} has no legacy boolean mirror column",
            capability.as_str()
        )),
    }
}

fn legacy_model_capabilities(row: &sqlx::sqlite::SqliteRow) -> Result<ModelCapabilities, String> {
    Ok(ModelCapabilities::legacy(
        legacy_boolean_capability(row, CapabilityId::REASONING)?,
        legacy_boolean_capability(row, CapabilityId::WEB)?,
    ))
}

async fn capability_overrides_for_provider(
    pool: &SqlitePool,
    provider_id: &str,
) -> Result<HashMap<String, CapabilityOverrides>, String> {
    let rows = sqlx::query(
        "SELECT o.model_id, o.capability_id, o.value_json
         FROM model_capability_overrides o
         JOIN models m ON m.id = o.model_id
         WHERE m.provider_id = ?",
    )
    .bind(provider_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut overrides = HashMap::<String, CapabilityOverrides>::new();
    for row in rows {
        let capability_id: String = row.get("capability_id");
        let capability = CapabilityId::from_registered(&capability_id)
            .ok_or_else(|| format!("Unknown capability ID in database: {capability_id}"))?;
        let value_json: String = row.get("value_json");
        let value = serde_json::from_str(&value_json)
            .map_err(|error| format!("Invalid capability override {capability_id}: {error}"))?;
        overrides
            .entry(row.get("model_id"))
            .or_default()
            .insert_json(capability, value)?;
    }
    Ok(overrides)
}

async fn capability_overrides_for_model(
    pool: &SqlitePool,
    model_id: &str,
) -> Result<CapabilityOverrides, String> {
    let rows = sqlx::query(
        "SELECT capability_id, value_json
         FROM model_capability_overrides
         WHERE model_id = ?",
    )
    .bind(model_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut overrides = CapabilityOverrides::default();
    for row in rows {
        let capability_id: String = row.get("capability_id");
        let capability = CapabilityId::from_registered(&capability_id)
            .ok_or_else(|| format!("Unknown capability ID in database: {capability_id}"))?;
        let value_json: String = row.get("value_json");
        let value = serde_json::from_str(&value_json)
            .map_err(|error| format!("Invalid capability override {capability_id}: {error}"))?;
        overrides.insert_json(capability, value)?;
    }
    Ok(overrides)
}

fn registered_descriptor(raw_id: &str) -> Result<&'static ProtocolDescriptor, String> {
    match resolve_persisted(raw_id)? {
        ProtocolResolution::Known(descriptor) => Ok(descriptor),
        ProtocolResolution::Unknown { raw_id, .. } => Err(format!(
            "UnknownProtocol: provider protocol \"{raw_id}\" is unknown or no longer available"
        )),
    }
}

fn authentication_for_protocol(descriptor: &ProtocolDescriptor) -> (&'static str, &'static str) {
    (
        descriptor.auth.strategy.legacy_auth_type(),
        descriptor.auth.strategy.header(),
    )
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
    let id: String = row.get("id");
    let custom_parameters_json: String = row.get("custom_parameters_json");
    let custom_parameters: Value = serde_json::from_str(&custom_parameters_json)
        .map_err(|error| format!("Assistant {id} custom parameters JSON is invalid: {error}"))?;
    if !custom_parameters.is_object() {
        return Err(format!(
            "Assistant {id} custom parameters must be a JSON object"
        ));
    }
    Ok(AssistantView {
        id,
        name: row.get("name"),
        icon_kind: AssistantIconKind::parse(row.get::<String, _>("icon_kind").as_str())?,
        icon_value: row.get("icon_value"),
        purpose: ProviderPurpose::parse(row.get::<String, _>("purpose").as_str())?,
        system_prompt: row.get("system_prompt"),
        temperature_enabled: row.get::<i64, _>("temperature_enabled") != 0,
        temperature: row.get("temperature"),
        top_p_enabled: row.get::<i64, _>("top_p_enabled") != 0,
        top_p: row.get("top_p"),
        custom_parameters,
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
    Ok(())
}

pub async fn update_assistant_settings(
    pool: &SqlitePool,
    input: UpdateAssistantSettingsInput,
) -> Result<AssistantView, String> {
    validate_assistant_settings(&input)?;
    sqlx::query(
        "UPDATE assistants SET name = ?, icon_kind = ?, icon_value = ?, temperature_enabled = ?, temperature = ?, top_p_enabled = ?, top_p = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(input.name.trim())
    .bind(input.icon_kind.as_str())
    .bind(input.icon_value.trim())
    .bind(input.temperature_enabled)
    .bind(input.temperature)
    .bind(input.top_p_enabled)
    .bind(input.top_p)
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
    assistant_from_row(&source)?;
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
        "INSERT INTO assistants (id, name, icon_kind, icon_value, purpose, system_prompt, temperature_enabled, temperature, top_p_enabled, top_p, custom_parameters_json, sort_order) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
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
    let raw_protocol: String = row.get("protocol");
    let config_json: String = row.get("config_json");
    let parsed_config = parse_provider_config(&id, &config_json)?;
    let (protocol, protocol_status, protocol_raw_id, known_protocol) =
        match resolve_persisted(&raw_protocol)? {
            ProtocolResolution::Known(descriptor) => (
                ProtocolId::registered(descriptor.id),
                ProtocolStatus::Available,
                None,
                Some(descriptor),
            ),
            ProtocolResolution::Unknown {
                id: unknown_protocol,
                raw_id,
            } => {
                eprintln!("Loaded provider {id} with unknown protocol: {raw_id}");
                (
                    unknown_protocol,
                    ProtocolStatus::Unknown,
                    Some(raw_id),
                    None,
                )
            }
        };
    let (config, config_issues) = match known_protocol {
        Some(descriptor) => {
            let mineru = id == MINERU_PROVIDER_ID || parsed_config.get("mineru").is_some();
            let config = provider_config_with_defaults(descriptor, parsed_config, mineru)
                .map_err(|error| format!("Provider {id} config is invalid: {error}"))?;
            let issues = config_issues(&config, descriptor.config_fields);
            (config, issues)
        }
        None => (parsed_config, Vec::new()),
    };
    let base_url: String = row.get("base_url");
    let model_rows =
        sqlx::query("SELECT * FROM models WHERE provider_id = ? ORDER BY sort_order, created_at")
            .bind(&id)
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
    let capability_overrides = capability_overrides_for_provider(pool, &id).await?;
    let models = model_rows
        .iter()
        .map(|row| {
            let model_id: String = row.get("id");
            model_from_row(
                row,
                known_protocol,
                &base_url,
                capability_overrides.get(&model_id),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let header_keys_json: String = row.get("header_keys_json");
    let custom_header_keys = parse_header_keys(&id, &header_keys_json)?;
    Ok(ProviderView {
        id,
        name: row.get("name"),
        protocol,
        protocol_status,
        protocol_raw_id,
        base_url,
        use_raw_base_url: row.get::<i64, _>("use_raw_base_url") != 0,
        config,
        config_issues,
        avatar: row.get("avatar"),
        is_builtin: row.get::<i64, _>("is_builtin") != 0,
        enabled: row.get::<i64, _>("enabled") != 0,
        credential_mask: row.get("credential_mask"),
        custom_header_keys,
        purpose: ProviderPurpose::parse(&purpose_value)?,
        models,
    })
}

fn model_from_row(
    row: &sqlx::sqlite::SqliteRow,
    descriptor: Option<&ProtocolDescriptor>,
    base_url: &str,
    overrides: Option<&CapabilityOverrides>,
) -> Result<ModelView, String> {
    let request_name: String = row.get("request_name");
    let capabilities = match descriptor {
        Some(descriptor) => {
            let empty_overrides = CapabilityOverrides::default();
            resolve_capabilities(
                descriptor.capability_profile,
                base_url,
                &request_name,
                overrides.unwrap_or(&empty_overrides),
            )?
        }
        None => legacy_model_capabilities(row)?,
    };
    Ok(ModelView {
        id: row.get("id"),
        provider_id: row.get("provider_id"),
        request_name,
        alias: row.get("alias"),
        source: row.get("source"),
        capabilities,
        test_status: row.get("test_status"),
        latency_ms: row.get("latency_ms"),
        tested_at: row.get("tested_at"),
        test_error: row.get("test_error"),
    })
}

pub async fn create_provider(
    pool: &SqlitePool,
    input: CreateProviderInput,
) -> Result<ProviderView, String> {
    if input.name.trim().is_empty() {
        return Err("Provider name is required".into());
    }
    let descriptor = resolve_input(&input.protocol)?;
    let id = new_id("provider");
    let credential_ref = format!("provider/{id}/credential");
    let (auth_type, auth_header) = authentication_for_protocol(descriptor);
    let config = default_provider_config(descriptor)?;
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO providers (id, name, protocol, base_url, auth_type, auth_header, config_json, credential_ref, avatar) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(input.name.trim())
    .bind(descriptor.id)
    .bind(descriptor.default_base_url)
    .bind(auth_type)
    .bind(auth_header)
    .bind(config.to_string())
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
    let row = sqlx::query("SELECT protocol, config_json FROM providers WHERE id = ?")
        .bind(&input.id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    let descriptor = registered_descriptor(row.get::<String, _>("protocol").as_str())?;
    let current_config_json: String = row.get("config_json");
    let current_config = parse_provider_config(&input.id, &current_config_json)?;
    let mineru = input.id == MINERU_PROVIDER_ID || current_config.get("mineru").is_some();
    let config_json =
        normalize_provider_config(input.config.unwrap_or(current_config), descriptor, mineru)?;
    let endpoint_base_url = provider_endpoint_base_url(&base_url);
    if endpoint_base_url.trim().is_empty() {
        return Err("Base URL is required".into());
    }
    url::Url::parse(endpoint_base_url.trim())
        .map_err(|_| "Base URL must be a valid absolute URL")?;
    sqlx::query("UPDATE providers SET base_url = ?, use_raw_base_url = ?, config_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
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

fn normalize_provider_config(
    value: Value,
    descriptor: &ProtocolDescriptor,
    mineru_provider: bool,
) -> Result<String, String> {
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
    let config = provider_config_with_defaults(descriptor, Value::Object(object), mineru_provider)?;
    Ok(validated_config(config, descriptor.config_fields)?.to_string())
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
    let descriptor = registered_descriptor(row.get::<String, _>("protocol").as_str())?;
    if descriptor.config_kind != "vertex-ai" {
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
    let row =
        sqlx::query("SELECT protocol, config_json, credential_ref FROM providers WHERE id = ?")
            .bind(provider_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Provider not found".to_string())?;
    let descriptor = registered_descriptor(row.get::<String, _>("protocol").as_str())?;
    if descriptor.config_kind != "vertex-ai" {
        return Err("Agent Platform config can only be saved on Agent Platform providers".into());
    }
    let config_json: String = row.get("config_json");
    let mut object = parse_provider_config(provider_id, &config_json)?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("Provider {provider_id} config must be a JSON object"))?;
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
    let normalized = normalize_provider_config(Value::Object(object), descriptor, false)?;

    if let Some(private_key) = private_key {
        let reference = row
            .get::<Option<String>, _>("credential_ref")
            .unwrap_or_else(|| format!("provider/{provider_id}/credential"));
        let trimmed = private_key.trim();
        let formatted = if trimmed.is_empty() {
            None
        } else {
            Some(vertex_ai::format_private_key(trimmed)?)
        };
        let mask = formatted.as_deref().map(secrets::mask);
        let mutation = SecretMutation::apply(reference.clone(), formatted.as_deref())?;
        let database_result: Result<(), String> = async {
            let result = sqlx::query("UPDATE providers SET config_json = ?, credential_ref = ?, credential_mask = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(normalized)
            .bind(reference)
            .bind(mask)
            .bind(provider_id)
            .execute(pool)
            .await
            .map_err(|error| error.to_string())?;
            if result.rows_affected() != 1 {
                return Err("expected exactly one updated provider row".into());
            }
            Ok(())
        }
        .await;
        if let Err(error) = database_result {
            return Err(secret_database_error(provider_id, error, &mutation));
        }
    } else {
        let result = sqlx::query(
            "UPDATE providers SET config_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(normalized)
        .bind(provider_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        if result.rows_affected() != 1 {
            return Err(format!(
                "Provider {provider_id} database update failed: expected exactly one updated provider row"
            ));
        }
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
    let result = sqlx::query(
        "UPDATE providers SET name = ?, avatar = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(input.name.trim())
    .bind(input.avatar)
    .bind(&input.id)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    if result.rows_affected() != 1 {
        return Err("Provider not found".into());
    }
    get_provider(pool, &input.id).await
}

pub async fn repair_provider_protocol(
    pool: &SqlitePool,
    input: RepairProviderProtocolInput,
) -> Result<ProviderView, String> {
    let descriptor = resolve_input(&input.protocol)?;
    let row = sqlx::query("SELECT protocol, config_json FROM providers WHERE id = ?")
        .bind(&input.id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    let raw_protocol: String = row.get("protocol");
    if !matches!(
        resolve_persisted(&raw_protocol)?,
        ProtocolResolution::Unknown { .. }
    ) {
        return Err("Only providers with unknown protocols can be repaired".into());
    }
    let raw_config: String = row.get("config_json");
    let config = parse_provider_config(&input.id, &raw_config)?;
    let mineru = input.id == MINERU_PROVIDER_ID || config.get("mineru").is_some();
    let config = provider_config_with_defaults(descriptor, config, mineru)?;
    let (auth_type, auth_header) = authentication_for_protocol(descriptor);

    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE providers
         SET protocol = ?, auth_type = ?, auth_header = ?, config_json = ?, enabled = 0,
             updated_at = CURRENT_TIMESTAMP
         WHERE id = ?",
    )
    .bind(descriptor.id)
    .bind(auth_type)
    .bind(auth_header)
    .bind(config.to_string())
    .bind(&input.id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE models
         SET test_status = 'untested', latency_ms = NULL, tested_at = NULL, test_error = NULL,
             updated_at = CURRENT_TIMESTAMP
         WHERE provider_id = ?",
    )
    .bind(&input.id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    get_provider(pool, &input.id).await
}

pub async fn set_provider_enabled(
    pool: &SqlitePool,
    input: SetProviderEnabledInput,
) -> Result<ProviderView, String> {
    let raw_protocol: String = sqlx::query_scalar("SELECT protocol FROM providers WHERE id = ?")
        .bind(&input.id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    if input.enabled
        && matches!(
            resolve_persisted(&raw_protocol)?,
            ProtocolResolution::Unknown { .. }
        )
    {
        return Err(format!(
            "Provider protocol \"{raw_protocol}\" is unknown or no longer available"
        ));
    }
    if input.enabled {
        let provider = get_provider(pool, &input.id).await?;
        if !provider.config_issues.is_empty() {
            return Err(format!(
                "Provider config is invalid: {}",
                crate::providers::config_schema::format_issues(&provider.config_issues)
            ));
        }
    }
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
    clone_provider_with_id(
        pool,
        provider_id,
        purpose,
        exact_name,
        preserve_builtin,
        new_id("provider"),
    )
    .await
}

async fn clone_provider_with_id(
    pool: &SqlitePool,
    provider_id: &str,
    purpose: ProviderPurpose,
    exact_name: Option<&str>,
    preserve_builtin: bool,
    id: String,
) -> Result<ProviderView, String> {
    let source = sqlx::query("SELECT * FROM providers WHERE id = ?")
        .bind(provider_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Provider not found".to_string())?;
    let source_id: String = source.get("id");
    let source_config_json: String = source.get("config_json");
    let parsed_source_config = parse_provider_config(&source_id, &source_config_json)?;
    let source_is_mineru =
        source_id == MINERU_PROVIDER_ID || parsed_source_config.get("mineru").is_some();
    if source_is_mineru && purpose != ProviderPurpose::DocumentParsing {
        return Err("MinerU providers can only be copied to document parsing".into());
    }
    let raw_protocol: String = source.get("protocol");
    let (copied_config, copied_enabled) = match resolve_persisted(&raw_protocol)? {
        ProtocolResolution::Known(descriptor) => {
            let config =
                provider_config_with_defaults(descriptor, parsed_source_config, source_is_mineru)?;
            let valid = config_issues(&config, descriptor.config_fields).is_empty();
            (config, source.get::<i64, _>("enabled") != 0 && valid)
        }
        ProtocolResolution::Unknown { .. } => (parsed_source_config, false),
    };
    let source_header_keys_json: String = source.get("header_keys_json");
    let source_header_keys = parse_header_keys(&source_id, &source_header_keys_json)?;
    let source_name: String = source.get("name");
    let name = match exact_name {
        Some(value) => value.to_string(),
        None => next_copy_name(pool, &source_name, purpose).await?,
    };
    let credential_ref = format!("provider/{id}/credential");
    let headers_ref = format!("provider/{id}/headers");
    let source_credential_ref: Option<String> = source.get("credential_ref");
    let source_headers_ref: Option<String> = source.get("headers_ref");
    let source_credential = source_credential_ref
        .as_deref()
        .map(secrets::read)
        .transpose()?
        .flatten();
    let source_headers = source_headers_ref
        .as_deref()
        .map(secrets::read)
        .transpose()?
        .flatten();
    let source_credential_mask: Option<String> = source.get("credential_mask");
    if source_credential_mask.is_some() && source_credential.is_none() {
        return Err(format!(
            "Provider {source_id} credential metadata exists but the credential is missing"
        ));
    }
    if !source_header_keys.is_empty() && source_headers.is_none() {
        return Err(format!(
            "Provider {source_id} header metadata exists but the header secret is missing"
        ));
    }
    let copied_config_json = copied_config.to_string();
    let copied_header_keys_json = serde_json::to_string(&source_header_keys)
        .map_err(|error| format!("Unable to serialize copied provider header keys: {error}"))?;
    let copied_credential_mask = source_credential.as_deref().map(secrets::mask);
    let mut wrote_credential = false;
    let mut wrote_headers = false;
    if let Some(secret) = source_credential.as_deref() {
        secrets::write(&credential_ref, secret)?;
        wrote_credential = true;
    }
    if let Some(headers) = source_headers.as_deref() {
        if let Err(error) = secrets::write(&headers_ref, headers) {
            let cleanup = cleanup_copied_provider_secrets(
                &credential_ref,
                &headers_ref,
                wrote_credential,
                false,
            );
            return Err(combine_operation_and_cleanup_error(
                format!("Unable to copy provider headers: {error}"),
                cleanup,
            ));
        }
        wrote_headers = true;
    }
    let database_result: Result<(), String> = async {
        let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
        sqlx::query("INSERT INTO providers (id, name, protocol, base_url, use_raw_base_url, auth_type, auth_header, config_json, enabled, credential_ref, credential_mask, headers_ref, header_keys_json, avatar, is_builtin) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(&name)
        .bind(&raw_protocol)
        .bind(source.get::<String, _>("base_url"))
        .bind(source.get::<i64, _>("use_raw_base_url"))
        .bind(source.get::<String, _>("auth_type"))
        .bind(source.get::<String, _>("auth_header"))
        .bind(&copied_config_json)
        .bind(copied_enabled)
        .bind(&credential_ref)
        .bind(&copied_credential_mask)
        .bind(&headers_ref)
        .bind(&copied_header_keys_json)
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
        sqlx::query(
            "INSERT INTO model_capability_overrides (model_id, capability_id, value_json)
         SELECT copied.id, overrides.capability_id, overrides.value_json
         FROM models source
         JOIN model_capability_overrides overrides ON overrides.model_id = source.id
         JOIN models copied
           ON copied.provider_id = ? AND copied.request_name = source.request_name
         WHERE source.provider_id = ?",
        )
        .bind(&id)
        .bind(provider_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    .await;
    if let Err(error) = database_result {
        let cleanup = cleanup_copied_provider_secrets(
            &credential_ref,
            &headers_ref,
            wrote_credential,
            wrote_headers,
        );
        return Err(combine_operation_and_cleanup_error(error, cleanup));
    }
    get_provider(pool, &id).await
}

fn cleanup_copied_provider_secrets(
    credential_ref: &str,
    headers_ref: &str,
    wrote_credential: bool,
    wrote_headers: bool,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if wrote_credential {
        if let Err(error) = secrets::delete(credential_ref) {
            errors.push(format!("credential cleanup failed: {error}"));
        }
    }
    if wrote_headers {
        if let Err(error) = secrets::delete(headers_ref) {
            errors.push(format!("header cleanup failed: {error}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn combine_operation_and_cleanup_error(
    operation_error: String,
    cleanup: Result<(), String>,
) -> String {
    match cleanup {
        Ok(()) => operation_error,
        Err(cleanup_error) => format!("{operation_error}; {cleanup_error}"),
    }
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
    let row = sqlx::query("SELECT credential_ref FROM providers WHERE id = ?")
        .bind(provider_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Provider {provider_id} not found"))?;
    let reference = row
        .get::<Option<String>, _>("credential_ref")
        .unwrap_or_else(|| format!("provider/{provider_id}/credential"));
    let replacement = credential.as_deref().filter(|value| !value.is_empty());
    let mask = replacement.map(secrets::mask);
    let mutation = SecretMutation::apply(reference.clone(), replacement)?;
    let database_result: Result<(), String> = async {
        let result = sqlx::query("UPDATE providers SET credential_ref = ?, credential_mask = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(reference)
        .bind(mask)
        .bind(provider_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        if result.rows_affected() != 1 {
            return Err("expected exactly one updated provider row".into());
        }
        Ok(())
    }
    .await;
    if let Err(error) = database_result {
        return Err(secret_database_error(provider_id, error, &mutation));
    }
    get_provider(pool, provider_id).await
}

pub async fn replace_headers(
    pool: &SqlitePool,
    provider_id: &str,
    headers_json: Option<String>,
) -> Result<ProviderView, String> {
    let row = sqlx::query("SELECT headers_ref FROM providers WHERE id = ?")
        .bind(provider_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Provider {provider_id} not found"))?;
    let reference = row
        .get::<Option<String>, _>("headers_ref")
        .unwrap_or_else(|| format!("provider/{provider_id}/headers"));
    let raw = headers_json
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let validated = if let Some(raw) = raw {
        parse_and_validate_headers_json(raw)?
    } else {
        ValidatedHeaders {
            keys: Vec::new(),
            values: Vec::new(),
        }
    };
    let replacement = if validated.values.is_empty() {
        None
    } else {
        raw
    };
    let header_keys_json =
        serde_json::to_string(&validated.keys).map_err(|error| error.to_string())?;
    let mutation = SecretMutation::apply(reference.clone(), replacement)?;
    let database_result: Result<(), String> = async {
        let result = sqlx::query("UPDATE providers SET headers_ref = ?, header_keys_json = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(reference)
        .bind(header_keys_json)
        .bind(provider_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        if result.rows_affected() != 1 {
            return Err("expected exactly one updated provider row".into());
        }
        Ok(())
    }
    .await;
    if let Err(error) = database_result {
        return Err(secret_database_error(provider_id, error, &mutation));
    }
    get_provider(pool, provider_id).await
}

pub async fn add_model(pool: &SqlitePool, input: AddModelInput) -> Result<ModelView, String> {
    let id = new_id("model");
    let provider = sqlx::query("SELECT protocol, base_url FROM providers WHERE id = ?")
        .bind(&input.provider_id)
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
    let descriptor = registered_descriptor(provider.get::<String, _>("protocol").as_str())?;
    let base_url: String = provider.get("base_url");
    let request_name = input.request_name.trim();
    let alias = if input.alias.trim().is_empty() {
        request_name
    } else {
        input.alias.trim()
    };
    let inferred = infer_capabilities(
        descriptor.capability_profile,
        base_url.as_str(),
        request_name,
    );
    sqlx::query("INSERT INTO models (id, provider_id, request_name, alias, source, capability_reasoning, capability_web, sort_order) VALUES (?, ?, ?, ?, ?, ?, ?, COALESCE((SELECT MAX(sort_order) + 1 FROM models WHERE provider_id = ?), 0)) ON CONFLICT(provider_id, request_name) DO UPDATE SET alias = excluded.alias")
        .bind(&id)
        .bind(&input.provider_id)
        .bind(request_name)
        .bind(alias)
        .bind(input.source)
        .bind(inferred.reasoning())
        .bind(inferred.web())
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
    let model_id: String = row.get("id");
    let overrides = capability_overrides_for_model(pool, &model_id).await?;
    model_from_row(&row, Some(descriptor), &base_url, Some(&overrides))
}

pub async fn update_model(pool: &SqlitePool, input: UpdateModelInput) -> Result<ModelView, String> {
    let model = sqlx::query(
        "SELECT m.request_name, p.protocol, p.base_url
         FROM models m
         JOIN providers p ON p.id = m.provider_id
         WHERE m.id = ?",
    )
    .bind(&input.id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "Model not found".to_string())?;
    let descriptor = registered_descriptor(model.get::<String, _>("protocol").as_str())?;
    let base_url: String = model.get("base_url");
    let request_name: String = model.get("request_name");
    let inferred = infer_capabilities(descriptor.capability_profile, &base_url, &request_name);
    for capability_id in input.capabilities.keys() {
        let capability = CapabilityId::from_registered(capability_id)
            .ok_or_else(|| format!("Unknown capability ID: {capability_id}"))?;
        if capability.definition().override_policy != CapabilityOverridePolicy::User {
            return Err(format!(
                "Capability {} cannot be changed by the user",
                capability.as_str()
            ));
        }
    }
    let mut overrides = capability_overrides_for_model(pool, &input.id).await?;
    for capability in CapabilityId::REGISTERED
        .iter()
        .copied()
        .filter(|capability| {
            capability.definition().override_policy == CapabilityOverridePolicy::User
        })
    {
        let value = input
            .capabilities
            .get(capability.as_str())
            .ok_or_else(|| format!("Missing user-editable capability: {}", capability.as_str()))?;
        overrides.remove(capability);
        overrides.insert_user_value(capability, value.clone())?;
    }
    let resolved = resolve_capabilities(
        descriptor.capability_profile,
        &base_url,
        &request_name,
        &overrides,
    )?;

    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query("UPDATE models SET alias = ?, capability_reasoning = ?, capability_web = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(input.alias.trim())
        .bind(resolved.reasoning())
        .bind(resolved.web())
        .bind(&input.id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    for capability in CapabilityId::REGISTERED
        .iter()
        .copied()
        .filter(|capability| {
            capability.definition().override_policy == CapabilityOverridePolicy::User
        })
    {
        let final_value = overrides
            .get(capability)
            .expect("all user-editable capabilities were validated");
        if final_value == inferred.get(capability) {
            sqlx::query(
                "DELETE FROM model_capability_overrides
                 WHERE model_id = ? AND capability_id = ?",
            )
            .bind(&input.id)
            .bind(capability.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
        } else {
            sqlx::query(
                "INSERT INTO model_capability_overrides
                 (model_id, capability_id, value_json)
                 VALUES (?, ?, ?)
                 ON CONFLICT(model_id, capability_id) DO UPDATE SET
                    value_json = excluded.value_json,
                    updated_at = CURRENT_TIMESTAMP",
            )
            .bind(&input.id)
            .bind(capability.as_str())
            .bind(final_value.as_json()?.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
        }
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    get_model(pool, &input.id).await
}

pub async fn get_model(pool: &SqlitePool, id: &str) -> Result<ModelView, String> {
    let row = sqlx::query(
        "SELECT m.*, p.protocol, p.base_url
         FROM models m
         JOIN providers p ON p.id = m.provider_id
         WHERE m.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "Model not found".to_string())?;
    let descriptor = registered_descriptor(row.get::<String, _>("protocol").as_str())?;
    let base_url: String = row.get("base_url");
    let overrides = capability_overrides_for_model(pool, id).await?;
    model_from_row(&row, Some(descriptor), &base_url, Some(&overrides))
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
    let credential_mask: Option<String> = row.get("credential_mask");
    if credential_mask.is_some() && credential.is_none() {
        return Err(format!(
            "Provider {id} credential metadata exists but the credential is missing"
        ));
    }
    let header_keys_json: String = row.get("header_keys_json");
    let header_keys = parse_header_keys(id, &header_keys_json)?;
    let headers_secret = match headers_ref {
        Some(reference) => secrets::read(&reference)?,
        None => None,
    };
    if !header_keys.is_empty() && headers_secret.is_none() {
        return Err(format!(
            "Provider {id} header metadata exists but the header secret is missing"
        ));
    }
    let custom_headers = match headers_secret {
        Some(json) => {
            let validated = parse_and_validate_headers_json(&json)
                .map_err(|error| format!("Provider {id} custom headers are invalid: {error}"))?;
            let actual_keys = validated.keys;
            let mut recorded_keys = header_keys;
            recorded_keys.sort();
            if actual_keys != recorded_keys {
                return Err(format!(
                    "Provider {id} custom header metadata does not match its stored secret"
                ));
            }
            validated.values
        }
        None => Vec::new(),
    };
    let descriptor = registered_descriptor(row.get::<String, _>("protocol").as_str())?;
    let config_json: String = row.get("config_json");
    let config = parse_provider_config(id, &config_json)?;
    let mineru = id == MINERU_PROVIDER_ID || config.get("mineru").is_some();
    let config = provider_config_with_defaults(descriptor, config, mineru)?;
    let issues = config_issues(&config, descriptor.config_fields);
    if !issues.is_empty() {
        return Err(format!(
            "Provider {id} config is invalid: {}",
            crate::providers::config_schema::format_issues(&issues)
        ));
    }
    Ok(ProviderRuntimeConfig {
        protocol: ProtocolId::registered(descriptor.id),
        base_url: row.get("base_url"),
        use_raw_base_url: row.get::<i64, _>("use_raw_base_url") != 0,
        config,
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
        AddModelInput, AssistantIconKind, CopyAssistantInput, CopyProviderInput,
        CreateAssistantInput, CreateProviderInput, ImportVertexAiServiceAccountInput, ProtocolId,
        ProviderPurpose, ReorderAssistantsInput, ReorderProvidersInput,
        RepairProviderProtocolInput, UpdateAssistantCustomParametersInput,
        UpdateAssistantPromptInput, UpdateAssistantSettingsInput, UpdateModelInput,
        UpdateProviderConfigInput, UpdateProviderMetadataInput, UpdateVertexAiConfigInput,
    };
    use std::collections::BTreeMap;

    fn user_capabilities(reasoning: bool, web: bool) -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("reasoning".into(), Value::Bool(reasoning)),
            ("web".into(), Value::Bool(web)),
        ])
    }

    #[test]
    fn custom_header_json_uses_http_validation_and_rejects_unsafe_fields() {
        let valid = parse_and_validate_headers_json(
            r#"{"Authorization":"Custom token","anthropic-version":"custom-version","X-Trace":"value"}"#,
        )
        .expect("valid custom headers");
        assert_eq!(
            valid.keys,
            vec!["Authorization", "X-Trace", "anthropic-version"]
        );
        assert!(valid
            .values
            .contains(&("Authorization".into(), "Custom token".into())));

        for raw in [
            r#"{"bad header":"value"}"#,
            r#"{"X-Test":"line\r\nbreak"}"#,
            r#"{"X-Test":1}"#,
            r#"{"X-Test":"one","X-Test":"two"}"#,
            r#"{"X-Test":"one","x-test":"two"}"#,
            r#"[]"#,
        ] {
            assert!(
                parse_and_validate_headers_json(raw).is_err(),
                "must reject {raw}"
            );
        }

        for name in FORBIDDEN_CUSTOM_HEADERS {
            let raw = format!(
                "{{{}:\"value\"}}",
                serde_json::to_string(name).expect("header name JSON")
            );
            let error = parse_and_validate_headers_json(&raw).expect_err("forbidden header");
            assert!(error.contains(name), "{name}: {error}");
            assert!(error.contains("cannot be overridden"));
        }
    }

    #[tokio::test]
    async fn assistant_custom_parameter_corruption_is_not_downgraded() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let assistant = create_assistant(
            &pool,
            CreateAssistantInput {
                purpose: ProviderPurpose::Translation,
            },
        )
        .await
        .expect("create assistant");

        for raw in ["{broken", "[]", "null"] {
            sqlx::query("UPDATE assistants SET custom_parameters_json = ? WHERE id = ?")
                .bind(raw)
                .bind(&assistant.id)
                .execute(&pool)
                .await
                .expect("store corrupt assistant parameters");
            let error = get_assistant(&pool, &assistant.id)
                .await
                .expect_err("corrupt assistant parameters");
            assert!(error.contains(&assistant.id), "{error}");
            assert!(error.contains("custom parameters"), "{error}");
        }

        sqlx::query("UPDATE assistants SET custom_parameters_json = ? WHERE id = ?")
            .bind(r#"{"nested":{"keep":true}}"#)
            .bind(&assistant.id)
            .execute(&pool)
            .await
            .expect("restore valid assistant parameters");
        assert_eq!(
            get_assistant(&pool, &assistant.id)
                .await
                .expect("valid assistant")
                .custom_parameters,
            json!({"nested": {"keep": true}})
        );

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn unknown_protocol_rows_survive_startup_and_can_be_repaired() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("initial connect");
        let provider_id = new_id("provider");
        let credential_ref = format!("test/{provider_id}/credential");
        let headers_ref = format!("test/{provider_id}/headers");
        sqlx::query("INSERT INTO providers (id, name, protocol, base_url, config_json, credential_ref, headers_ref, header_keys_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&provider_id)
            .bind("Retired provider")
            .bind("retired-chat-v0")
            .bind("https://retired.invalid")
            .bind(r#"{"legacy":{"keep":true}}"#)
            .bind(&credential_ref)
            .bind(&headers_ref)
            .bind(r#"["X-Legacy"]"#)
            .execute(&pool)
            .await
            .expect("insert unknown provider");
        sqlx::query(
            "INSERT INTO provider_purposes (provider_id, purpose, sort_order) VALUES (?, ?, 0)",
        )
        .bind(&provider_id)
        .bind(ProviderPurpose::Translation.as_str())
        .execute(&pool)
        .await
        .expect("insert provider purpose");
        let model_id = new_id("model");
        sqlx::query(
            "INSERT INTO models (id, provider_id, request_name, alias, test_status, latency_ms, tested_at, test_error) VALUES (?, ?, ?, ?, 'success', 42, '2026-08-02', 'old')",
        )
        .bind(&model_id)
        .bind(&provider_id)
        .bind("retired-model")
        .bind("Retired model")
        .execute(&pool)
        .await
        .expect("insert model");
        sqlx::query("DELETE FROM app_metadata WHERE key = 'model-capability-backfill-v1'")
            .execute(&pool)
            .await
            .expect("reset capability migration");
        pool.close().await;

        let pool = connect(&path)
            .await
            .expect("reconnect with unknown protocol");
        let persisted: String = sqlx::query_scalar("SELECT protocol FROM providers WHERE id = ?")
            .bind(&provider_id)
            .fetch_one(&pool)
            .await
            .expect("read persisted protocol");
        assert_eq!(persisted, "retired-chat-v0");

        let provider = get_provider(&pool, &provider_id)
            .await
            .expect("unknown provider remains visible");
        assert!(provider.protocol.is_unknown());
        assert_eq!(provider.protocol_status, ProtocolStatus::Unknown);
        assert_eq!(provider.protocol_raw_id.as_deref(), Some("retired-chat-v0"));
        assert!(provider.enabled);
        assert_eq!(provider.config.pointer("/legacy/keep"), Some(&json!(true)));
        assert_eq!(provider.custom_header_keys, vec!["X-Legacy"]);
        assert_eq!(
            provider.models[0].capabilities.thinking_efforts(),
            &[crate::domain::ThinkingEffort::None]
        );
        assert!(runtime_config(&pool, &provider_id).await.is_err());
        assert!(set_provider_enabled(
            &pool,
            SetProviderEnabledInput {
                id: provider_id.clone(),
                enabled: true,
            },
        )
        .await
        .is_err());

        update_provider_metadata(
            &pool,
            UpdateProviderMetadataInput {
                id: provider_id.clone(),
                name: "Repaired provider".into(),
                avatar: None,
            },
        )
        .await
        .expect("update provider metadata");
        let repaired = repair_provider_protocol(
            &pool,
            RepairProviderProtocolInput {
                id: provider_id.clone(),
                protocol: ProtocolId::registered("test-seventh"),
            },
        )
        .await
        .expect("repair protocol");
        assert_eq!(repaired.protocol_status, ProtocolStatus::Available);
        assert_eq!(repaired.protocol_raw_id, None);
        assert_eq!(repaired.protocol.as_str(), "test-seventh");
        assert!(!repaired.enabled);
        assert_eq!(repaired.config.pointer("/legacy/keep"), Some(&json!(true)));
        assert_eq!(repaired.config.pointer("/mode"), Some(&json!("fast")));
        assert_eq!(repaired.models[0].test_status, "untested");
        assert_eq!(repaired.models[0].latency_ms, None);
        assert_eq!(repaired.models[0].tested_at, None);
        assert_eq!(repaired.models[0].test_error, None);
        let preserved_refs: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT credential_ref, headers_ref FROM providers WHERE id = ?")
                .bind(&provider_id)
                .fetch_one(&pool)
                .await
                .expect("preserved secret references");
        assert_eq!(preserved_refs.0.as_deref(), Some(credential_ref.as_str()));
        assert_eq!(preserved_refs.1.as_deref(), Some(headers_ref.as_str()));
        let second_repair = repair_provider_protocol(
            &pool,
            RepairProviderProtocolInput {
                id: provider_id.clone(),
                protocol: ProtocolId::registered("openai-chat"),
            },
        )
        .await
        .expect_err("known protocol cannot be changed through repair");
        assert!(second_repair.contains("unknown protocols"));

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn test_protocol_schema_defaults_flow_through_storage_updates_and_runtime() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Schema provider".into(),
                protocol: ProtocolId::registered("test-seventh"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create test protocol provider");
        assert!(provider.config_issues.is_empty());
        assert_eq!(provider.config.pointer("/mode"), Some(&json!("fast")));
        assert_eq!(provider.config.pointer("/label"), Some(&json!("seventh")));
        assert_eq!(provider.config.pointer("/limits/retries"), Some(&json!(3)));
        assert_eq!(
            provider.config.pointer("/features/cache"),
            Some(&json!(true))
        );

        let persisted: Value = serde_json::from_str(
            &sqlx::query_scalar::<_, String>("SELECT config_json FROM providers WHERE id = ?")
                .bind(&provider.id)
                .fetch_one(&pool)
                .await
                .expect("persisted config"),
        )
        .expect("valid persisted config");
        assert_eq!(persisted, provider.config);

        sqlx::query("UPDATE providers SET config_json = ? WHERE id = ?")
            .bind(r#"{"unknown":{"kept":true}}"#)
            .bind(&provider.id)
            .execute(&pool)
            .await
            .expect("simulate legacy config");
        let hydrated = get_provider(&pool, &provider.id)
            .await
            .expect("hydrate legacy config");
        assert_eq!(hydrated.config.pointer("/mode"), Some(&json!("fast")));
        assert_eq!(hydrated.config.pointer("/unknown/kept"), Some(&json!(true)));
        let raw_after_read: Value = serde_json::from_str(
            &sqlx::query_scalar::<_, String>("SELECT config_json FROM providers WHERE id = ?")
                .bind(&provider.id)
                .fetch_one(&pool)
                .await
                .expect("config after read"),
        )
        .expect("valid config after read");
        assert!(raw_after_read.get("mode").is_none());
        let runtime = runtime_config(&pool, &provider.id)
            .await
            .expect("runtime config hydrates defaults");
        assert_eq!(runtime.config.pointer("/mode"), Some(&json!("fast")));

        let updated = update_provider_config(
            &pool,
            UpdateProviderConfigInput {
                id: provider.id.clone(),
                base_url: provider.base_url.clone(),
                use_raw_base_url: false,
                config: Some(json!({
                    "mode": "quality",
                    "label": "custom",
                    "limits": {"retries": 5},
                    "features": {"cache": false},
                    "unknown": {"kept": true}
                })),
            },
        )
        .await
        .expect("save valid schema config");
        assert_eq!(updated.config.pointer("/mode"), Some(&json!("quality")));
        assert_eq!(updated.config.pointer("/unknown/kept"), Some(&json!(true)));

        for invalid in [
            json!({"mode": "", "limits": {"retries": 5}, "features": {"cache": false}}),
            json!({"mode": "other", "limits": {"retries": 5}, "features": {"cache": false}}),
            json!({"mode": "fast", "limits": {"retries": "5"}, "features": {"cache": false}}),
            json!({"mode": "fast", "limits": {"retries": 5}, "features": {"cache": "false"}}),
        ] {
            assert!(update_provider_config(
                &pool,
                UpdateProviderConfigInput {
                    id: provider.id.clone(),
                    base_url: provider.base_url.clone(),
                    use_raw_base_url: false,
                    config: Some(invalid),
                },
            )
            .await
            .is_err());
        }
        assert!(update_provider_config(
            &pool,
            UpdateProviderConfigInput {
                id: provider.id.clone(),
                base_url: provider.base_url.clone(),
                use_raw_base_url: false,
                config: Some(json!([])),
            },
        )
        .await
        .is_err());

        set_provider_enabled(
            &pool,
            SetProviderEnabledInput {
                id: provider.id.clone(),
                enabled: false,
            },
        )
        .await
        .expect("disable before simulating invalid legacy config");
        sqlx::query("UPDATE providers SET config_json = ? WHERE id = ?")
            .bind(r#"{"mode":"","unknown":true}"#)
            .bind(&provider.id)
            .execute(&pool)
            .await
            .expect("store schema-invalid legacy config");
        let invalid_provider = get_provider(&pool, &provider.id)
            .await
            .expect("invalid provider remains visible");
        assert_eq!(invalid_provider.config_issues.len(), 1);
        assert_eq!(invalid_provider.config_issues[0].pointer, "/mode");
        assert!(set_provider_enabled(
            &pool,
            SetProviderEnabledInput {
                id: provider.id.clone(),
                enabled: true,
            },
        )
        .await
        .is_err());
        assert!(runtime_config(&pool, &provider.id).await.is_err());

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn corrupt_provider_json_and_header_metadata_are_reported_with_provider_id() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Corrupt metadata".into(),
                protocol: ProtocolId::registered("test-seventh"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create provider");

        sqlx::query("UPDATE providers SET config_json = '{broken' WHERE id = ?")
            .bind(&provider.id)
            .execute(&pool)
            .await
            .expect("corrupt config JSON");
        for error in [
            get_provider(&pool, &provider.id)
                .await
                .expect_err("view must reject corrupt config"),
            runtime_config(&pool, &provider.id)
                .await
                .expect_err("runtime must reject corrupt config"),
        ] {
            assert!(error.contains(&provider.id));
            assert!(error.contains("config JSON"));
        }

        sqlx::query(
            "UPDATE providers SET config_json = '{}', header_keys_json = '{broken' WHERE id = ?",
        )
        .bind(&provider.id)
        .execute(&pool)
        .await
        .expect("corrupt header metadata");
        for error in [
            get_provider(&pool, &provider.id)
                .await
                .expect_err("view must reject corrupt header metadata"),
            runtime_config(&pool, &provider.id)
                .await
                .expect_err("runtime must reject corrupt header metadata"),
        ] {
            assert!(error.contains(&provider.id));
            assert!(error.contains("header key JSON"));
        }

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn runtime_rejects_persisted_forbidden_headers_with_provider_context() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Legacy unsafe headers".into(),
                protocol: ProtocolId::registered("openai-chat"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create provider");
        let reference = format!("test/{}/legacy-headers", provider.id);
        secrets::write(&reference, r#"{"Content-Type":"text/plain"}"#)
            .expect("write isolated legacy header secret");
        sqlx::query("UPDATE providers SET headers_ref = ?, header_keys_json = ? WHERE id = ?")
            .bind(&reference)
            .bind(r#"["Content-Type"]"#)
            .bind(&provider.id)
            .execute(&pool)
            .await
            .expect("store legacy header metadata");

        let error = runtime_config(&pool, &provider.id)
            .await
            .expect_err("forbidden persisted header");
        let _ = secrets::delete(&reference);
        pool.close().await;
        let _ = std::fs::remove_file(path);

        assert!(error.contains(&provider.id), "{error}");
        assert!(error.contains("Content-Type"), "{error}");
        assert!(error.contains("cannot be overridden"), "{error}");
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn provider_secret_mutations_restore_old_values_after_database_failure() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Secret rollback".into(),
                protocol: ProtocolId::registered("openai-chat"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create provider");
        replace_credential(&pool, &provider.id, Some("old-credential".into()))
            .await
            .expect("store old credential");
        let old_headers_json = r#"{"X-Rollback":"old-header"}"#;
        replace_headers(&pool, &provider.id, Some(old_headers_json.into()))
            .await
            .expect("store old headers");

        let vertex = create_provider(
            &pool,
            CreateProviderInput {
                name: "Vertex secret rollback".into(),
                protocol: ProtocolId::registered("vertex-ai"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create vertex provider");
        update_vertex_ai_config(
            &pool,
            UpdateVertexAiConfigInput {
                provider_id: vertex.id.clone(),
                project_id: "project-old".into(),
                location: "global".into(),
                client_email: "old@example.invalid".into(),
                private_key: Some("old-private-key".into()),
            },
        )
        .await
        .expect("store old vertex key");

        let no_prior_secret = create_provider(
            &pool,
            CreateProviderInput {
                name: "No prior secret rollback".into(),
                protocol: ProtocolId::registered("openai-chat"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create provider without prior secret");
        sqlx::query(
            "UPDATE providers SET credential_ref = NULL, credential_mask = NULL WHERE id = ?",
        )
        .bind(&no_prior_secret.id)
        .execute(&pool)
        .await
        .expect("clear legacy credential reference");

        let credential_ref = format!("provider/{}/credential", provider.id);
        let headers_ref = format!("provider/{}/headers", provider.id);
        let vertex_ref = format!("provider/{}/credential", vertex.id);
        let old_vertex_key = secrets::read(&vertex_ref)
            .expect("read old vertex key")
            .expect("old vertex key exists");
        sqlx::query(
            "CREATE TRIGGER fail_provider_secret_update BEFORE UPDATE ON providers BEGIN SELECT RAISE(ABORT, 'forced provider secret update failure'); END",
        )
        .execute(&pool)
        .await
        .expect("create failure trigger");

        let credential_write =
            replace_credential(&pool, &provider.id, Some("new-credential".into())).await;
        let credential_after_write = secrets::read(&credential_ref);
        let credential_clear = replace_credential(&pool, &provider.id, None).await;
        let credential_after_clear = secrets::read(&credential_ref);
        let header_write = replace_headers(
            &pool,
            &provider.id,
            Some(r#"{"X-Rollback":"new-header"}"#.into()),
        )
        .await;
        let headers_after_write = secrets::read(&headers_ref);
        let header_clear = replace_headers(&pool, &provider.id, Some("{}".into())).await;
        let headers_after_clear = secrets::read(&headers_ref);
        let vertex_write = update_vertex_ai_config(
            &pool,
            UpdateVertexAiConfigInput {
                provider_id: vertex.id.clone(),
                project_id: "project-new".into(),
                location: "us-central1".into(),
                client_email: "new@example.invalid".into(),
                private_key: Some("new-private-key".into()),
            },
        )
        .await;
        let vertex_after_write = secrets::read(&vertex_ref);
        let vertex_clear = update_vertex_ai_config(
            &pool,
            UpdateVertexAiConfigInput {
                provider_id: vertex.id.clone(),
                project_id: "project-new".into(),
                location: "us-central1".into(),
                client_email: "new@example.invalid".into(),
                private_key: Some(String::new()),
            },
        )
        .await;
        let vertex_after_clear = secrets::read(&vertex_ref);
        let no_prior_write = replace_credential(
            &pool,
            &no_prior_secret.id,
            Some("must-not-be-orphaned".into()),
        )
        .await;
        let no_prior_ref = format!("provider/{}/credential", no_prior_secret.id);
        let no_prior_after_write = secrets::read(&no_prior_ref);

        sqlx::query("DROP TRIGGER fail_provider_secret_update")
            .execute(&pool)
            .await
            .expect("drop failure trigger");
        let cleared_headers = replace_headers(&pool, &provider.id, Some("{}".into()))
            .await
            .expect("empty header object clears headers");
        let headers_after_successful_clear = secrets::read(&headers_ref);
        let _ = secrets::delete(&credential_ref);
        let _ = secrets::delete(&headers_ref);
        let _ = secrets::delete(&vertex_ref);
        let _ = secrets::delete(&no_prior_ref);
        pool.close().await;
        let _ = std::fs::remove_file(path);

        for result in [
            credential_write.map(|_| ()),
            credential_clear.map(|_| ()),
            header_write.map(|_| ()),
            header_clear.map(|_| ()),
            vertex_write.map(|_| ()),
            vertex_clear.map(|_| ()),
            no_prior_write.map(|_| ()),
        ] {
            let error = result.expect_err("triggered database failure");
            assert!(
                error.contains("forced provider secret update failure"),
                "{error}"
            );
        }
        assert_eq!(
            credential_after_write.expect("read credential after failed write"),
            Some("old-credential".into())
        );
        assert_eq!(
            credential_after_clear.expect("read credential after failed clear"),
            Some("old-credential".into())
        );
        assert_eq!(
            headers_after_write.expect("read headers after failed write"),
            Some(old_headers_json.into())
        );
        assert_eq!(
            headers_after_clear.expect("read headers after failed clear"),
            Some(old_headers_json.into())
        );
        assert_eq!(
            vertex_after_write.expect("read vertex key after failed write"),
            Some(old_vertex_key.clone())
        );
        assert_eq!(
            vertex_after_clear.expect("read vertex key after failed clear"),
            Some(old_vertex_key)
        );
        assert_eq!(
            no_prior_after_write.expect("read missing prior secret after failed write"),
            None
        );
        assert!(cleared_headers.custom_header_keys.is_empty());
        assert_eq!(
            headers_after_successful_clear.expect("read successfully cleared headers"),
            None
        );
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn provider_copy_rejects_missing_secrets_and_cleans_up_after_database_failure() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let source = create_provider(
            &pool,
            CreateProviderInput {
                name: "Credential source".into(),
                protocol: ProtocolId::registered("openai-chat"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create source");
        sqlx::query("UPDATE providers SET credential_mask = 'masked' WHERE id = ?")
            .bind(&source.id)
            .execute(&pool)
            .await
            .expect("simulate missing source credential");
        let missing = copy_provider(
            &pool,
            CopyProviderInput {
                provider_id: source.id.clone(),
                purpose: ProviderPurpose::Glossary,
            },
        )
        .await
        .expect_err("missing source secret must reject copy");
        assert!(missing.contains("credential is missing"));

        replace_credential(&pool, &source.id, Some("copy-secret".into()))
            .await
            .expect("store unique source credential");
        replace_headers(
            &pool,
            &source.id,
            Some(json!({"X-Test-Copy": "header-secret"}).to_string()),
        )
        .await
        .expect("store unique source headers");
        let target_id = new_id("provider-copy-failure-test");
        let target_credential_ref = format!("provider/{target_id}/credential");
        let target_headers_ref = format!("provider/{target_id}/headers");
        sqlx::query(
            "CREATE TRIGGER fail_provider_copy BEFORE INSERT ON providers BEGIN SELECT RAISE(ABORT, 'forced provider copy failure'); END",
        )
        .execute(&pool)
        .await
        .expect("create failure trigger");
        let copy_result = clone_provider_with_id(
            &pool,
            &source.id,
            ProviderPurpose::Glossary,
            None,
            false,
            target_id.clone(),
        )
        .await;
        let target_credential = secrets::read(&target_credential_ref);
        let target_headers = secrets::read(&target_headers_ref);
        let source_credential_ref = format!("provider/{}/credential", source.id);
        let source_headers_ref = format!("provider/{}/headers", source.id);
        let _ = secrets::delete(&source_credential_ref);
        let _ = secrets::delete(&source_headers_ref);
        let _ = secrets::delete(&target_credential_ref);
        let _ = secrets::delete(&target_headers_ref);

        let error = copy_result.expect_err("database failure must reject copy");
        assert!(error.contains("forced provider copy failure"));
        assert_eq!(target_credential.expect("read target credential"), None);
        assert_eq!(target_headers.expect("read target headers"), None);
        let target_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM providers WHERE id = ?")
            .bind(&target_id)
            .fetch_one(&pool)
            .await
            .expect("target row count");
        assert_eq!(target_count, 0);

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn persists_provider_relations_and_keeps_model_request_name_immutable() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Test".into(),
                protocol: ProtocolId::registered("openai-chat"),
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
                capabilities: user_capabilities(true, false),
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
    async fn required_thinking_model_cannot_disable_reasoning_capability() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Claude 5 Test".into(),
                protocol: ProtocolId::registered("anthropic"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("provider");
        let model = add_model(
            &pool,
            AddModelInput {
                provider_id: provider.id,
                request_name: "claude-fable-5".into(),
                alias: String::new(),
                source: "manual".into(),
            },
        )
        .await
        .expect("model");
        assert!(model.capabilities.reasoning());
        assert!(model.capabilities.thinking_required());

        let error = update_model(
            &pool,
            UpdateModelInput {
                id: model.id.clone(),
                alias: "Still required".into(),
                capabilities: user_capabilities(false, model.capabilities.web()),
            },
        )
        .await
        .expect_err("required thinking cannot be disabled");
        assert!(error.contains("requires thinking"));
        assert!(get_model(&pool, &model.id)
            .await
            .expect("model after rejected update")
            .capabilities
            .reasoning());

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn add_model_infers_known_capabilities() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "OpenAI Responses Test".into(),
                protocol: ProtocolId::registered("openai-responses"),
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
                request_name: "gpt-5".into(),
                alias: "GPT-5".into(),
                source: "manual".into(),
            },
        )
        .await
        .expect("add model");

        assert!(model.capabilities.reasoning());
        assert!(model.capabilities.web());
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn model_capability_backfill_preserves_later_manual_changes() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Legacy OpenAI".into(),
                protocol: ProtocolId::registered("openai-responses"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create provider");
        let model_id = new_id("model");
        sqlx::query(
            "INSERT INTO models
             (id, provider_id, request_name, alias, source, capability_reasoning,
              capability_web, capability_tools, sort_order)
             VALUES (?, ?, 'gpt-5', 'GPT-5', 'manual', 0, 0, 0, 0)",
        )
        .bind(&model_id)
        .bind(&provider.id)
        .execute(&pool)
        .await
        .expect("insert legacy model");
        sqlx::query("DELETE FROM app_metadata WHERE key = 'model-capability-backfill-v1'")
            .execute(&pool)
            .await
            .expect("reset backfill marker");

        backfill_model_capabilities(&pool).await.expect("backfill");
        let backfilled = get_model(&pool, &model_id).await.expect("backfilled model");
        assert!(backfilled.capabilities.reasoning());
        assert!(backfilled.capabilities.web());

        update_model(
            &pool,
            UpdateModelInput {
                id: model_id.clone(),
                alias: "GPT-5".into(),
                capabilities: user_capabilities(true, false),
            },
        )
        .await
        .expect("manual update");
        backfill_model_capabilities(&pool)
            .await
            .expect("second backfill skips");
        let manual = get_model(&pool, &model_id).await.expect("manual model");
        assert!(!manual.capabilities.web());

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn capability_override_migration_and_provider_copy_preserve_effective_values() {
        let path =
            std::env::temp_dir().join(format!("insitu-translate-{}.sqlite3", new_id("test")));
        let pool = connect(&path).await.expect("connect");
        let provider = create_provider(
            &pool,
            CreateProviderInput {
                name: "Capability migration".into(),
                protocol: ProtocolId::registered("openai-responses"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create provider");
        let model_id = new_id("model");
        sqlx::query(
            "INSERT INTO models
             (id, provider_id, request_name, alias, capability_reasoning, capability_web)
             VALUES (?, ?, 'gpt-5', 'GPT-5', 0, 0)",
        )
        .bind(&model_id)
        .bind(&provider.id)
        .execute(&pool)
        .await
        .expect("insert legacy model");
        sqlx::query("DELETE FROM app_metadata WHERE key = 'model-capability-overrides-v1'")
            .execute(&pool)
            .await
            .expect("reset override migration");

        migrate_model_capability_overrides(&pool)
            .await
            .expect("migrate overrides");
        let migrated = get_model(&pool, &model_id).await.expect("migrated model");
        assert!(!migrated.capabilities.reasoning());
        assert!(!migrated.capabilities.web());

        let updated = update_model(
            &pool,
            UpdateModelInput {
                id: model_id,
                alias: "GPT-5".into(),
                capabilities: user_capabilities(true, false),
            },
        )
        .await
        .expect("update override");
        assert!(updated.capabilities.reasoning());
        assert!(!updated.capabilities.web());

        sqlx::query(
            "INSERT INTO model_capability_overrides (model_id, capability_id, value_json)
             VALUES (?, 'thinking-effort', '[\"high\"]')",
        )
        .bind(&updated.id)
        .execute(&pool)
        .await
        .expect("store legacy thinking-effort override");
        let stored = get_model(&pool, &updated.id)
            .await
            .expect("stored thinking-effort override");
        assert_eq!(
            stored.capabilities.thinking_efforts(),
            &[crate::domain::ThinkingEffort::High]
        );

        let copied = copy_provider(
            &pool,
            CopyProviderInput {
                provider_id: provider.id,
                purpose: ProviderPurpose::Translation,
            },
        )
        .await
        .expect("copy provider");
        assert!(copied.models[0].capabilities.reasoning());
        assert!(!copied.models[0].capabilities.web());
        assert_eq!(
            copied.models[0].capabilities.thinking_efforts(),
            &[crate::domain::ThinkingEffort::High]
        );

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
                protocol: ProtocolId::registered("openai-chat"),
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
                protocol: ProtocolId::registered("openai-chat"),
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
        let agent_platform = create_provider(
            &pool,
            CreateProviderInput {
                name: "Unique Agent Platform Test".into(),
                protocol: ProtocolId::registered("vertex-ai"),
                purpose: ProviderPurpose::Translation,
                avatar: None,
            },
        )
        .await
        .expect("create unique agent platform provider");

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
        assert_eq!(runtime.protocol.as_str(), "vertex-ai");
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
        assert_eq!(copied_runtime.protocol.as_str(), "vertex-ai");
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
                protocol: ProtocolId::registered("openai-chat"),
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
                protocol: ProtocolId::registered("openai-chat"),
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
                protocol: ProtocolId::registered("openai-chat"),
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
            },
        )
        .await
        .expect("update settings");
        assert_eq!(updated.name, "Translator");
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
