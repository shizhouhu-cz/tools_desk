use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tauri::{AppHandle, State};

use crate::commands::FileOutcome;
use crate::config::SettingsState;
use crate::error::{AppError, AppResult};
use crate::task::TaskManager;
use crate::tools::pptx_to_pdf::PptxConverter;

/// 批量 PPTX 转 PDF。单文件失败不中断批次。
#[tauri::command]
pub async fn pptx_to_pdf(
    app: AppHandle,
    tasks: State<'_, TaskManager>,
    settings: State<'_, SettingsState>,
    inputs: Vec<String>,
    out_dir: Option<String>,
) -> AppResult<Vec<FileOutcome>> {
    if inputs.is_empty() {
        return Err(AppError::BadRequest("未选择输入文件".into()));
    }

    let task = tasks.create(&app, "pptx-to-pdf");
    let cfg = settings.0.read().unwrap().clone();
    let converter = PptxConverter::new(
        &cfg.soffice_path,
        cfg.libreoffice_concurrency,
        Duration::from_secs(cfg.libreoffice_timeout_sec.max(5)),
    );

    // 输出目录：未指定时用第一个文件的所在目录。
    let out_dir_path: PathBuf = match out_dir.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(d) => PathBuf::from(d),
        None => Path::new(&inputs[0])
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };

    let total = inputs.len() as u64;
    let mut outcomes = Vec::with_capacity(inputs.len());
    for (i, input) in inputs.iter().enumerate() {
        task.progress_now("convert", i as u64, total);
        let started = Instant::now();
        if task.is_cancelled() {
            outcomes.push(FileOutcome::failure(input, "任务已取消".to_string(), 0));
            continue;
        }
        let handle = task.clone();
        let result = converter
            .convert_file(Path::new(input), &out_dir_path, &move || {
                handle.is_cancelled()
            })
            .await;
        let duration_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(o) => outcomes.push(FileOutcome::success(
                input,
                o.pdf_path,
                o.missing_fonts,
                o.renamed,
                duration_ms,
            )),
            Err(e) => outcomes.push(FileOutcome::failure(input, e.to_string(), duration_ms)),
        }
    }
    task.progress_now("done", outcomes.len() as u64, total);
    tasks.finish(&task.id);
    Ok(outcomes)
}
