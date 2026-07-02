use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::{Row, SqlitePool};

use crate::document_parsing;

use super::{
    TranslationChunkStatus, GLOBAL_BACKGROUND_BATCH_CHUNKS, GLOBAL_BACKGROUND_TARGET_TOKENS,
    TASKS_DIR,
};

pub(super) async fn previous_translation_context(
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

pub(super) async fn previous_source_context(
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

pub(super) fn global_background_from_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> String {
    let mut background = String::new();
    for text in texts {
        if append_background_text(&mut background, text) {
            break;
        }
    }
    truncate_global_background(&background)
}

pub(super) fn truncate_global_background(background: &str) -> String {
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

pub(super) async fn generate_global_background(pool: &SqlitePool) -> Result<String, String> {
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

pub(super) async fn task_global_background(pool: &SqlitePool) -> Result<Option<String>, String> {
    let background: Option<String> =
        sqlx::query_scalar("SELECT global_background FROM metadata LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
    Ok(background)
}

pub(super) async fn write_task_global_background(
    pool: &SqlitePool,
    background: &str,
) -> Result<(), String> {
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

pub(super) async fn ensure_task_global_background(
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

pub(super) fn estimate_tokens(text: &str) -> u64 {
    document_parsing::count_tokens(text) as u64
}

pub(super) async fn next_inp_path(
    workspace_root: &Path,
    display_name: &str,
) -> Result<PathBuf, String> {
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

pub(super) fn display_name_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("task")
        .to_string()
}

pub(super) fn sanitize_file_stem(value: &str) -> String {
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

pub(super) fn unix_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}
