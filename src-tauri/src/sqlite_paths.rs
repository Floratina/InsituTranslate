use std::path::{Path, PathBuf};

use uuid::Uuid;

pub(crate) const SQLITE_DATABASE_PATH_UTF16_LIMIT: usize = 240;

pub(crate) async fn next_sqlite_database_path(
    directory: &Path,
    preferred_stem: &str,
    fallback_stem: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    let initial_candidate =
        sqlite_database_path(directory, preferred_stem, fallback_stem, "", extension)?;
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|error| error.to_string())?;
    for index in 0..10_000 {
        let candidate = if index == 0 {
            initial_candidate.clone()
        } else {
            sqlite_database_path(
                directory,
                preferred_stem,
                fallback_stem,
                &format!("-{index:02}"),
                extension,
            )?
        };
        if tokio::fs::try_exists(&candidate)
            .await
            .map_err(|error| error.to_string())?
        {
            continue;
        }
        return Ok(candidate);
    }
    Err("Unable to allocate a unique SQLite database file name".into())
}

pub(crate) fn uuid_sqlite_temporary_path(
    directory: &Path,
    prefix: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    let filename = format!(".{prefix}-{}.{}", Uuid::new_v4(), extension);
    let candidate = directory.join(filename);
    ensure_sqlite_path_length(&candidate)?;
    Ok(candidate)
}

pub(crate) fn is_uuid_sqlite_temporary_file(path: &Path, prefix: &str, extension: &str) -> bool {
    let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(uuid) = filename
        .strip_prefix(&format!(".{prefix}-"))
        .and_then(|value| value.strip_suffix(&format!(".{extension}")))
    else {
        return false;
    };
    Uuid::parse_str(uuid).is_ok()
}

pub(crate) fn utf16_path_len(path: &Path) -> usize {
    path.to_string_lossy().encode_utf16().count()
}

fn sqlite_database_path(
    directory: &Path,
    preferred_stem: &str,
    fallback_stem: &str,
    suffix: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    let directory_units = utf16_path_len(directory);
    let fixed_units = 1 + suffix.encode_utf16().count() + 1 + extension.encode_utf16().count();
    let available_stem_units = SQLITE_DATABASE_PATH_UTF16_LIMIT
        .checked_sub(directory_units + fixed_units)
        .ok_or_else(sqlite_workspace_too_long)?;
    if available_stem_units == 0 {
        return Err(sqlite_workspace_too_long());
    }
    let stem = fitted_stem(preferred_stem, fallback_stem, available_stem_units)?;
    let candidate = directory.join(format!("{stem}{suffix}.{extension}"));
    ensure_sqlite_path_length(&candidate)?;
    Ok(candidate)
}

fn fitted_stem(
    preferred_stem: &str,
    fallback_stem: &str,
    available_units: usize,
) -> Result<String, String> {
    let preferred = truncate_utf16(preferred_stem, available_units);
    if !preferred.is_empty() {
        return Ok(preferred);
    }
    let fallback = truncate_utf16(fallback_stem, available_units);
    if fallback.is_empty() {
        Err(sqlite_workspace_too_long())
    } else {
        Ok(fallback)
    }
}

fn truncate_utf16(value: &str, max_units: usize) -> String {
    let mut truncated = String::new();
    let mut used_units = 0;
    for character in value.chars() {
        let character_units = character.len_utf16();
        if used_units + character_units > max_units {
            break;
        }
        truncated.push(character);
        used_units += character_units;
    }
    truncated.trim_end_matches([' ', '.']).to_string()
}

fn ensure_sqlite_path_length(path: &Path) -> Result<(), String> {
    if utf16_path_len(path) > SQLITE_DATABASE_PATH_UTF16_LIMIT {
        Err(sqlite_workspace_too_long())
    } else {
        Ok(())
    }
}

fn sqlite_workspace_too_long() -> String {
    "SQLite 工作目录路径过长，无法安全创建数据库文件".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_unicode_without_splitting_utf16_characters() {
        let directory = PathBuf::from(format!("C:\\{}", "a".repeat(210)));
        let candidate =
            sqlite_database_path(&directory, "术语😀表名称很长", "glossary", "-9999", "ing")
                .expect("safe path");
        assert!(utf16_path_len(&candidate) <= SQLITE_DATABASE_PATH_UTF16_LIMIT);
        assert_eq!(
            candidate.extension().and_then(|value| value.to_str()),
            Some("ing")
        );
        assert!(candidate
            .file_stem()
            .and_then(|value| value.to_str())
            .expect("file stem")
            .ends_with("-9999"));
    }

    #[test]
    fn rejects_a_directory_that_consumes_the_entire_budget() {
        let directory = PathBuf::from(format!("C:\\{}", "a".repeat(238)));
        let error = sqlite_database_path(&directory, "task", "task", "", "inp")
            .expect_err("directory must be rejected");
        assert!(error.contains("工作目录路径过长"));
    }

    #[test]
    fn creates_and_recognizes_standard_uuid_temporary_names() {
        let directory = Path::new("C:\\workspace");
        let path =
            uuid_sqlite_temporary_path(directory, "creating", "ing").expect("temporary path");
        assert!(is_uuid_sqlite_temporary_file(&path, "creating", "ing"));
        assert!(!is_uuid_sqlite_temporary_file(
            Path::new("C:\\workspace\\name.creating-glossary_legacy"),
            "creating",
            "ing",
        ));
    }
}
