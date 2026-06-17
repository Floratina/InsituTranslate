mod adapters;
mod commands;
mod db;
mod domain;
mod features;
mod glossaries;
mod languages;
// Reserved for the glossary extraction pipeline; intentionally not exposed through IPC yet.
#[allow(dead_code)]
mod glossary_prompt;
mod secrets;
mod system_fonts;
// Shared infrastructure for task-specific document prompt builders.
#[allow(dead_code)]
mod task_prompt;
// Reserved for the document translation pipeline; intentionally not exposed through IPC yet.
#[allow(dead_code)]
mod translation_prompt;
mod translation_tasks;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Unable to resolve app data directory: {error}"))?;
            std::fs::create_dir_all(&app_data)
                .map_err(|error| format!("Unable to create app data directory: {error}"))?;
            let pool =
                tauri::async_runtime::block_on(db::connect(&app_data.join("providers.sqlite3")))
                    .map_err(|error| format!("Unable to initialize provider database: {error}"))?;
            let legacy_workspace_root = translation_tasks::default_workspace_root();
            let workspace_root = app_data.join("translation-workspace");
            translation_tasks::migrate_legacy_workspace(&legacy_workspace_root, &workspace_root)
                .map_err(|error| format!("Unable to migrate translation workspace: {error}"))?;
            let translation_config_pool = tauri::async_runtime::block_on(
                translation_tasks::connect_config_db(&workspace_root),
            )
            .map_err(|error| format!("Unable to initialize translation workspace: {error}"))?;
            let glossary_workspace_root = glossaries::workspace_root(&app_data);
            let glossary_config_pool = tauri::async_runtime::block_on(
                glossaries::connect_config_db(&glossary_workspace_root),
            )
            .map_err(|error| format!("Unable to initialize glossary workspace: {error}"))?;
            tauri::async_runtime::block_on(translation_tasks::rebase_task_index_paths(
                &translation_config_pool,
                &legacy_workspace_root,
                &workspace_root,
            ))
            .map_err(|error| format!("Unable to rebase translation task paths: {error}"))?;
            let client = commands::build_http_client()
                .map_err(|error| format!("Unable to initialize HTTP client: {error}"))?;
            app.manage(commands::AppState {
                pool,
                translation_config_pool,
                translation_workspace_root: workspace_root,
                glossary_config_pool,
                glossary_workspace_root,
                running_translation_task: Default::default(),
                client,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_providers,
            commands::list_assistants,
            commands::create_assistant,
            commands::update_assistant_settings,
            commands::update_assistant_prompt,
            commands::update_assistant_custom_parameters,
            commands::reorder_assistants,
            commands::copy_assistant,
            commands::delete_assistant,
            commands::create_provider,
            commands::update_provider_config,
            commands::update_provider_metadata,
            commands::set_provider_enabled,
            commands::reorder_providers,
            commands::copy_provider,
            commands::delete_provider,
            commands::replace_provider_credential,
            commands::replace_provider_headers,
            commands::fetch_provider_models,
            commands::add_model,
            commands::update_model,
            commands::delete_model,
            commands::test_model_connectivity,
            commands::runtime_chat,
            commands::runtime_chat_stream,
            commands::create_translation_task,
            commands::list_translation_tasks,
            commands::update_translation_task_tags,
            commands::start_translation_task,
            commands::resume_translation_task,
            commands::retranslate_translation_task,
            commands::delete_translation_task,
            commands::get_translation_task_detail,
            commands::get_translation_config,
            commands::update_translation_config,
            commands::list_glossaries,
            commands::import_glossary,
            commands::update_glossary,
            commands::delete_glossary,
            commands::open_glossary_folder,
            commands::export_glossary,
            commands::get_glossary_entries,
            commands::create_glossary_entry,
            commands::update_glossary_entry,
            commands::delete_glossary_entry,
            commands::prepare_auto_glossary_for_task,
            commands::pick_glossary_file,
            commands::pick_translation_files,
            commands::detect_source_language,
            system_fonts::list_system_fonts,
        ])
        .run(tauri::generate_context!())
        .expect("error while running InsituTranslate");
}
