mod commands;
mod config;
mod error;
mod task;
mod tools;

use config::SettingsState;
use task::TaskManager;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("tools".into()),
                    }),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Webview),
                ])
                .build(),
        )
        .manage(TaskManager::default())
        .manage(SettingsState(std::sync::RwLock::new(
            config::Settings::default(),
        )))
        .setup(|app| {
            let settings = config::load(app.handle());
            *app.state::<SettingsState>().0.write().unwrap() = settings;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            task::cancel_task,
            commands::collector::collect_files,
            commands::open::open_dir_in_explorer,
            commands::open::reveal_in_explorer,
            commands::pptx_to_pdf::pptx_to_pdf,
            commands::docx_structure::docx_structure,
            commands::docx_structure::docx_tree,
            commands::docx_structure::docx_documents,
            commands::docx_structure::docx_chapter_content,
            commands::docx_compare::docx_compare,
            commands::pdf_to_images::pdf_to_images,
            commands::pdf_to_images::probe_dify,
            commands::pdf_edit::pdf_watermark,
            commands::pdf_edit::pdf_extract_pages,
            commands::watermark::watermark_images,
            commands::sqlite_explorer::sqlite_scan,
            commands::sqlite_explorer::sqlite_detect,
            commands::sqlite_explorer::sqlite_tables,
            commands::sqlite_explorer::sqlite_query,
            commands::settings::get_settings,
            commands::settings::set_settings,
            commands::settings::probe_soffice,
            commands::video_spritesheet::video_spritesheet,
        ])
        // 精灵图预览协议：staging 目录 → iframe 可加载的页面（WebView2 下为 http://sprite-preview.localhost/...）
        .register_uri_scheme_protocol("sprite-preview", |ctx, request| {
            commands::video_spritesheet::sprite_preview_protocol(ctx, request)
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
