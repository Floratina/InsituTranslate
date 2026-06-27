use std::collections::BTreeMap;

pub(crate) fn collect_system_fonts() -> Vec<String> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();

    let mut names = BTreeMap::new();
    for face in database.faces() {
        for (family, _) in &face.families {
            let trimmed = family.trim();
            if !trimmed.is_empty() {
                names
                    .entry(trimmed.to_lowercase())
                    .or_insert_with(|| trimmed.to_string());
            }
        }
    }

    let mut fonts = names.into_values().collect::<Vec<_>>();
    fonts.sort_by_key(|name| name.to_lowercase());
    fonts
}

#[tauri::command]
pub async fn list_system_fonts() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(collect_system_fonts)
        .await
        .map_err(|error| format!("Unable to load system fonts: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_fonts_are_sorted_and_unique() {
        let fonts = collect_system_fonts();
        assert!(!fonts.is_empty());
        assert!(fonts
            .windows(2)
            .all(|pair| pair[0].to_lowercase() <= pair[1].to_lowercase()));
        let unique = fonts
            .iter()
            .map(|font| font.to_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), fonts.len());
    }
}
