use serde::Serialize;
use tauri::{AppHandle, State};

use crate::config::{self, Settings, SettingsState};
use crate::error::AppResult;
use crate::tools::pptx_to_pdf::PptxConverter;
use std::time::Duration;

#[tauri::command]
pub fn get_settings(settings: State<'_, SettingsState>) -> Settings {
    settings.0.read().unwrap().clone()
}

#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    new_settings: Settings,
) -> AppResult<Settings> {
    // 基本合法性归一化。
    let mut s = new_settings;
    s.libreoffice_timeout_sec = s.libreoffice_timeout_sec.clamp(5, 3600);
    s.libreoffice_concurrency = s.libreoffice_concurrency.clamp(1, 8);
    s.default_dpi = s.default_dpi.clamp(72, 600);
    s.default_quality = s.default_quality.clamp(1, 100);
    s.max_pages = s.max_pages.clamp(1, 2000);

    config::save(&app, &s)?;
    *settings.0.write().unwrap() = s.clone();
    Ok(s)
}

#[derive(Debug, Serialize)]
pub struct SofficeProbe {
    pub ok: bool,
    pub resolved_path: Option<String>,
}

/// 探测当前配置的 soffice 是否可用。
#[tauri::command]
pub async fn probe_soffice(settings: State<'_, SettingsState>) -> AppResult<SofficeProbe> {
    let cfg = settings.0.read().unwrap().clone();
    let converter = PptxConverter::new(
        &cfg.soffice_path,
        1,
        Duration::from_secs(cfg.libreoffice_timeout_sec.max(5)),
    );
    let ok = converter.probe().await;
    Ok(SofficeProbe {
        ok,
        resolved_path: None,
    })
}
