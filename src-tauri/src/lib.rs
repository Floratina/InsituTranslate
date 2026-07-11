mod adapters;
mod commands;
mod db;
mod diagnostics;
mod document_parsing;
mod domain;
mod features;
mod glossaries;
mod languages;
mod pdf_parsing;
// Reserved for the glossary extraction pipeline; intentionally not exposed through IPC yet.
#[allow(dead_code)]
mod glossary_prompt;
mod secrets;
mod settings;
mod system_fonts;
mod task_scheduler;
// Shared infrastructure for task-specific document prompt builders.
#[allow(dead_code)]
mod task_prompt;
// Reserved for the document translation pipeline; intentionally not exposed through IPC yet.
#[allow(dead_code)]
mod translation_prompt;
#[path = "translation/mod.rs"]
mod translation_tasks;
mod vertex_ai;

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
            diagnostics::initialize_backend_log(&app.handle())
                .map_err(|error| format!("Unable to initialize diagnostics log: {error}"))?;
            let pool =
                tauri::async_runtime::block_on(db::connect(&app_data.join("providers.sqlite3")))
                    .map_err(|error| format!("Unable to initialize provider database: {error}"))?;
            let settings_pool = tauri::async_runtime::block_on(settings::connect(
                &app_data.join("settings.sqlite3"),
            ))
            .map_err(|error| format!("Unable to initialize settings database: {error}"))?;
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
            let scheduler_preferences = tauri::async_runtime::block_on(
                settings::get_task_scheduler_preferences(&settings_pool),
            )
            .map_err(|error| format!("Unable to load task scheduler preferences: {error}"))?;
            let task_scheduler = task_scheduler::TaskScheduler::start(
                task_scheduler::TaskSchedulerContext {
                    app: app.handle().clone(),
                    provider_pool: pool.clone(),
                    config_pool: translation_config_pool.clone(),
                    settings_pool: settings_pool.clone(),
                    glossary_config_pool: glossary_config_pool.clone(),
                    glossary_workspace_root: glossary_workspace_root.clone(),
                    workspace_root: workspace_root.clone(),
                    client: client.clone(),
                },
                scheduler_preferences.max_active_tasks,
            );
            app.manage(commands::AppState {
                pool,
                settings_pool,
                translation_config_pool,
                translation_workspace_root: workspace_root,
                glossary_config_pool,
                glossary_workspace_root,
                translation_task_creation_jobs: Default::default(),
                translation_task_staged_creations: Default::default(),
                client,
            });
            app.manage(task_scheduler);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_appearance_preferences,
            commands::update_appearance_preferences,
            commands::get_task_scheduler_preferences,
            commands::open_backend_console,
            commands::get_cached_system_fonts,
            commands::refresh_system_fonts_cache,
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
            commands::update_vertex_ai_config,
            commands::import_vertex_ai_service_account,
            commands::get_vertex_ai_private_key,
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
            commands::start_translation_task_creation,
            commands::cancel_translation_task_creation,
            commands::publish_translation_task_creation,
            commands::list_translation_tasks,
            commands::import_translation_task,
            commands::update_translation_task_name,
            commands::update_translation_task_tags,
            commands::update_translation_task_info,
            commands::open_translation_task_folder,
            commands::export_translation_task,
            commands::dispatch_scheduler_action,
            commands::delete_translation_task,
            commands::delete_translation_tasks,
            commands::get_translation_task_detail,
            commands::get_translation_task_summary,
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
            commands::pick_translation_task_file,
            commands::pick_translation_files,
            commands::detect_source_language,
            system_fonts::list_system_fonts,
        ])
        .run(tauri::generate_context!())
        .expect("error while running InsituTranslate");
}
