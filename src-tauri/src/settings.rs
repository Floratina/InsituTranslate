use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::system_fonts;

const APPEARANCE_NAMESPACE: &str = "appearance";
const APPEARANCE_KEY: &str = "preferences";
const SYSTEM_NAMESPACE: &str = "system";
const SYSTEM_FONTS_KEY: &str = "fonts";
const SCHEDULER_NAMESPACE: &str = "scheduler";
const SCHEDULER_KEY: &str = "preferences";
const DEFAULT_MAX_ACTIVE_TASKS: usize = 1;
const MAX_ACTIVE_TASKS_LIMIT: usize = 4;
const DEFAULT_CUSTOM_THEME_COLOR: &str = "#16B8C4";
const SYSTEM_FONT_VALUE: &str = "system";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ColorMode {
    Light,
    Dark,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeId {
    Sky,
    Iris,
    Pine,
    Lagoon,
    Sand,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppearancePreferences {
    pub color_mode: ColorMode,
    pub theme_id: ThemeId,
    pub custom_theme_color: String,
    pub font_family: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppearancePreferencesState {
    pub preferences: AppearancePreferences,
    pub stored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FontCacheRefresh {
    pub changed: bool,
    pub fonts: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSchedulerPreferences {
    pub max_active_tasks: usize,
}

impl Default for TaskSchedulerPreferences {
    fn default() -> Self {
        Self {
            max_active_tasks: DEFAULT_MAX_ACTIVE_TASKS,
        }
    }
}

impl Default for AppearancePreferences {
    fn default() -> Self {
        Self {
            color_mode: ColorMode::System,
            theme_id: ThemeId::Sky,
            custom_theme_color: DEFAULT_CUSTOM_THEME_COLOR.into(),
            font_family: SYSTEM_FONT_VALUE.into(),
        }
    }
}

pub async fn connect(path: &Path) -> Result<SqlitePool, String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .map_err(|error| error.to_string())?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS settings_entries (
            namespace TEXT NOT NULL,
            key TEXT NOT NULL,
            value_json TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (namespace, key)
        )"#,
    )
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn read_entry(
    pool: &SqlitePool,
    namespace: &str,
    key: &str,
) -> Result<Option<String>, String> {
    sqlx::query_scalar("SELECT value_json FROM settings_entries WHERE namespace = ? AND key = ?")
        .bind(namespace)
        .bind(key)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())
}

async fn write_entry(
    pool: &SqlitePool,
    namespace: &str,
    key: &str,
    value: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO settings_entries (namespace, key, value_json, updated_at)
         VALUES (?, ?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(namespace, key)
         DO UPDATE SET value_json = excluded.value_json, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(namespace)
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn normalize_hex_color(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let without_hash = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if without_hash.len() == 3 && without_hash.chars().all(|part| part.is_ascii_hexdigit()) {
        let mut result = String::from("#");
        for part in without_hash.chars() {
            result.push(part.to_ascii_uppercase());
            result.push(part.to_ascii_uppercase());
        }
        return Some(result);
    }
    if without_hash.len() == 6 && without_hash.chars().all(|part| part.is_ascii_hexdigit()) {
        return Some(format!("#{}", without_hash.to_ascii_uppercase()));
    }
    None
}

fn normalize_appearance_preferences(
    input: AppearancePreferences,
) -> Result<AppearancePreferences, String> {
    let custom_theme_color = normalize_hex_color(&input.custom_theme_color)
        .ok_or_else(|| "Custom theme color is invalid".to_string())?;
    let font_family = input.font_family.trim();
    if font_family.is_empty()
        || font_family.len() > 255
        || font_family.chars().any(char::is_control)
    {
        return Err("Font family is invalid".into());
    }
    Ok(AppearancePreferences {
        color_mode: input.color_mode,
        theme_id: input.theme_id,
        custom_theme_color,
        font_family: font_family.to_string(),
    })
}

pub async fn get_appearance_preferences(
    pool: &SqlitePool,
) -> Result<AppearancePreferencesState, String> {
    let Some(value) = read_entry(pool, APPEARANCE_NAMESPACE, APPEARANCE_KEY).await? else {
        return Ok(AppearancePreferencesState {
            preferences: AppearancePreferences::default(),
            stored: false,
        });
    };
    let preferences = serde_json::from_str::<AppearancePreferences>(&value)
        .map_err(|error| format!("Stored appearance preferences are invalid: {error}"))?;
    Ok(AppearancePreferencesState {
        preferences: normalize_appearance_preferences(preferences)?,
        stored: true,
    })
}

pub async fn update_appearance_preferences(
    pool: &SqlitePool,
    input: AppearancePreferences,
) -> Result<AppearancePreferences, String> {
    let preferences = normalize_appearance_preferences(input)?;
    let value = serde_json::to_string(&preferences).map_err(|error| error.to_string())?;
    write_entry(pool, APPEARANCE_NAMESPACE, APPEARANCE_KEY, &value).await?;
    Ok(preferences)
}

fn normalize_task_scheduler_preferences(
    input: TaskSchedulerPreferences,
) -> Result<TaskSchedulerPreferences, String> {
    if !(1..=MAX_ACTIVE_TASKS_LIMIT).contains(&input.max_active_tasks) {
        return Err(format!(
            "Maximum active tasks must be between 1 and {MAX_ACTIVE_TASKS_LIMIT}"
        ));
    }
    Ok(input)
}

pub async fn get_task_scheduler_preferences(
    pool: &SqlitePool,
) -> Result<TaskSchedulerPreferences, String> {
    let Some(value) = read_entry(pool, SCHEDULER_NAMESPACE, SCHEDULER_KEY).await? else {
        return Ok(TaskSchedulerPreferences::default());
    };
    let preferences = serde_json::from_str::<TaskSchedulerPreferences>(&value)
        .map_err(|error| format!("Stored task scheduler preferences are invalid: {error}"))?;
    normalize_task_scheduler_preferences(preferences)
}

pub async fn update_task_scheduler_preferences(
    pool: &SqlitePool,
    input: TaskSchedulerPreferences,
) -> Result<TaskSchedulerPreferences, String> {
    let preferences = normalize_task_scheduler_preferences(input)?;
    let value = serde_json::to_string(&preferences).map_err(|error| error.to_string())?;
    write_entry(pool, SCHEDULER_NAMESPACE, SCHEDULER_KEY, &value).await?;
    Ok(preferences)
}

fn parse_font_cache(value: &str) -> Result<Vec<String>, String> {
    let fonts = serde_json::from_str::<Vec<String>>(value)
        .map_err(|error| format!("Stored system font cache is invalid: {error}"))?;
    validate_fonts(fonts)
}

fn validate_fonts(fonts: Vec<String>) -> Result<Vec<String>, String> {
    if fonts
        .iter()
        .any(|font| font.trim().is_empty() || font.chars().any(char::is_control))
    {
        return Err("Stored system font cache contains invalid font names".into());
    }
    Ok(fonts)
}

pub async fn get_cached_system_fonts(pool: &SqlitePool) -> Result<Vec<String>, String> {
    read_entry(pool, SYSTEM_NAMESPACE, SYSTEM_FONTS_KEY)
        .await?
        .map(|value| parse_font_cache(&value))
        .transpose()
        .map(|fonts| fonts.unwrap_or_default())
}

pub async fn refresh_system_fonts_cache(pool: &SqlitePool) -> Result<FontCacheRefresh, String> {
    let fonts = tauri::async_runtime::spawn_blocking(system_fonts::collect_system_fonts)
        .await
        .map_err(|error| format!("Unable to load system fonts: {error}"))?;
    update_system_fonts_cache(pool, fonts).await
}

async fn update_system_fonts_cache(
    pool: &SqlitePool,
    fonts: Vec<String>,
) -> Result<FontCacheRefresh, String> {
    let fonts = validate_fonts(fonts)?;
    let current = get_cached_system_fonts(pool).await?;
    if current == fonts {
        return Ok(FontCacheRefresh {
            changed: false,
            fonts: None,
        });
    }
    let value = serde_json::to_string(&fonts).map_err(|error| error.to_string())?;
    write_entry(pool, SYSTEM_NAMESPACE, SYSTEM_FONTS_KEY, &value).await?;
    Ok(FontCacheRefresh {
        changed: true,
        fonts: Some(fonts),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::Value;
    use sqlx::Row;

    use super::*;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_db_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("insitu-settings-{nanos:x}{counter:x}.sqlite3"))
    }

    #[tokio::test]
    async fn settings_migrate_and_read_default_appearance() {
        let path = temp_db_path();
        let pool = connect(&path).await.expect("connect settings");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'settings_entries'",
        )
        .fetch_one(&pool)
        .await
        .expect("count tables");
        assert_eq!(count, 1);
        let preferences = get_appearance_preferences(&pool)
            .await
            .expect("default appearance");
        assert_eq!(preferences.preferences, AppearancePreferences::default());
        assert!(!preferences.stored);
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn appearance_update_persists_normalized_preferences() {
        let path = temp_db_path();
        let pool = connect(&path).await.expect("connect settings");
        let saved = update_appearance_preferences(
            &pool,
            AppearancePreferences {
                color_mode: ColorMode::Dark,
                theme_id: ThemeId::Custom,
                custom_theme_color: "16b8c4".into(),
                font_family: " Segoe UI ".into(),
            },
        )
        .await
        .expect("save appearance");
        assert_eq!(saved.custom_theme_color, "#16B8C4");
        assert_eq!(saved.font_family, "Segoe UI");
        let loaded = get_appearance_preferences(&pool)
            .await
            .expect("load appearance");
        assert_eq!(loaded.preferences, saved);
        assert!(loaded.stored);
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn scheduler_preferences_default_to_one_active_task() {
        let path = temp_db_path();
        let pool = connect(&path).await.expect("connect settings");

        let preferences = get_task_scheduler_preferences(&pool)
            .await
            .expect("default scheduler preferences");

        assert_eq!(preferences, TaskSchedulerPreferences::default());
        assert_eq!(preferences.max_active_tasks, 1);
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn scheduler_preferences_persist_values_between_one_and_four() {
        let path = temp_db_path();
        let pool = connect(&path).await.expect("connect settings");

        let saved = update_task_scheduler_preferences(
            &pool,
            TaskSchedulerPreferences {
                max_active_tasks: 4,
            },
        )
        .await
        .expect("save scheduler preferences");
        let loaded = get_task_scheduler_preferences(&pool)
            .await
            .expect("load scheduler preferences");

        assert_eq!(saved.max_active_tasks, 4);
        assert_eq!(loaded, saved);
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn scheduler_preferences_reject_out_of_range_values() {
        let path = temp_db_path();
        let pool = connect(&path).await.expect("connect settings");

        assert!(update_task_scheduler_preferences(
            &pool,
            TaskSchedulerPreferences {
                max_active_tasks: 0,
            },
        )
        .await
        .is_err());
        assert!(update_task_scheduler_preferences(
            &pool,
            TaskSchedulerPreferences {
                max_active_tasks: 5,
            },
        )
        .await
        .is_err());
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn font_cache_writes_first_value_and_skips_unchanged_refresh() {
        let path = temp_db_path();
        let pool = connect(&path).await.expect("connect settings");
        let first = update_system_fonts_cache(&pool, vec!["Arial".into(), "Segoe UI".into()])
            .await
            .expect("first cache update");
        assert!(first.changed);
        assert_eq!(first.fonts, Some(vec!["Arial".into(), "Segoe UI".into()]));
        let timestamp: String =
            sqlx::query("SELECT updated_at FROM settings_entries WHERE namespace = ? AND key = ?")
                .bind(SYSTEM_NAMESPACE)
                .bind(SYSTEM_FONTS_KEY)
                .fetch_one(&pool)
                .await
                .expect("first timestamp")
                .get("updated_at");
        let second = update_system_fonts_cache(&pool, vec!["Arial".into(), "Segoe UI".into()])
            .await
            .expect("second cache update");
        assert!(!second.changed);
        assert_eq!(second.fonts, None);
        let unchanged_timestamp: String =
            sqlx::query("SELECT updated_at FROM settings_entries WHERE namespace = ? AND key = ?")
                .bind(SYSTEM_NAMESPACE)
                .bind(SYSTEM_FONTS_KEY)
                .fetch_one(&pool)
                .await
                .expect("second timestamp")
                .get("updated_at");
        assert_eq!(timestamp, unchanged_timestamp);
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn font_cache_returns_changed_list_when_fonts_differ() {
        let path = temp_db_path();
        let pool = connect(&path).await.expect("connect settings");
        update_system_fonts_cache(&pool, vec!["Arial".into()])
            .await
            .expect("first cache update");
        let changed = update_system_fonts_cache(&pool, vec!["Arial".into(), "Verdana".into()])
            .await
            .expect("changed cache update");
        assert!(changed.changed);
        assert_eq!(changed.fonts, Some(vec!["Arial".into(), "Verdana".into()]));
        assert_eq!(
            get_cached_system_fonts(&pool).await.expect("cached fonts"),
            vec!["Arial".to_string(), "Verdana".to_string()],
        );
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn raw_settings_entry_accepts_future_namespaces() {
        let path = temp_db_path();
        let pool = connect(&path).await.expect("connect settings");
        write_entry(
            &pool,
            "future",
            "setting",
            &Value::String("ok".into()).to_string(),
        )
        .await
        .expect("write future setting");
        let value = read_entry(&pool, "future", "setting")
            .await
            .expect("read future setting");
        assert_eq!(value, Some(Value::String("ok".into()).to_string()));
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }
}
