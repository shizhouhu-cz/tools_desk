use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tauri::{AppHandle, State};
use tokio::sync::Semaphore;

use crate::commands::FileOutcome;
use crate::config::SettingsState;
use crate::error::{AppError, AppResult};
use crate::task::TaskManager;
use crate::tools::watermark::{WatermarkEngine, WatermarkParams};

/// 批量图片打水印。
/// output_mode: "suffix"（默认，同目录 `{stem}_wm.{ext}`）| "subdir"（`{dir}/watermarked/`）。
#[tauri::command]
pub async fn watermark_images(
    app: AppHandle,
    tasks: State<'_, TaskManager>,
    settings: State<'_, SettingsState>,
    inputs: Vec<String>,
    params: WatermarkParams,
    output_mode: Option<String>,
) -> AppResult<Vec<FileOutcome>> {
    if inputs.is_empty() {
        return Err(AppError::BadRequest("未选择输入图片".into()));
    }

    let task = tasks.create(&app, "watermark");
    let cfg = settings.0.read().unwrap().clone();
    let engine = Arc::new(WatermarkEngine::new(&cfg.font_dir, &cfg.default_font)?);

    let subdir_mode = output_mode.as_deref() == Some("subdir");
    let total = inputs.len() as u64;
    let semaphore = Arc::new(Semaphore::new(4));

    task.progress_now("apply", 0, total);

    // 派发任务，结果按原顺序回填。
    let mut joins = Vec::with_capacity(inputs.len());
    for (idx, input) in inputs.iter().enumerate() {
        let idx = idx as u64;
        if task.is_cancelled() {
            // 队列级取消：未派发的直接记为取消，不占并发额度。
            let outcome = FileOutcome::failure(input, "任务已取消".to_string(), 0);
            joins.push(tauri::async_runtime::spawn(async move { (idx, outcome) }));
            continue;
        }
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let engine = engine.clone();
        let params = params.clone();
        let handle = task.clone();
        let input = input.clone();

        joins.push(tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            let started = Instant::now();
            let outcome = if handle.is_cancelled() {
                FileOutcome::failure(&input, "任务已取消".to_string(), 0)
            } else {
                match run_one(&engine, Path::new(&input), &params, subdir_mode) {
                    Ok((out, renamed)) => {
                        let duration_ms = started.elapsed().as_millis() as u64;
                        FileOutcome::success(&input, out, Vec::new(), renamed, duration_ms)
                    }
                    Err(e) => {
                        let duration_ms = started.elapsed().as_millis() as u64;
                        FileOutcome::failure(&input, e.to_string(), duration_ms)
                    }
                }
            };
            (idx, outcome)
        }));
    }

    let mut outcomes = Vec::with_capacity(joins.len());
    for j in joins {
        let (idx, outcome) = j
            .await
            .map_err(|e| AppError::Internal(format!("任务执行失败: {e}")))?;
        let _ = idx;
        outcomes.push(outcome);
        task.progress("apply", outcomes.len() as u64, total);
    }

    task.progress_now("done", total, total);
    tasks.finish(&task.id);
    Ok(outcomes)
}

fn run_one(
    engine: &WatermarkEngine,
    src: &Path,
    params: &WatermarkParams,
    subdir_mode: bool,
) -> AppResult<(String, bool)> {
    if !src.is_file() {
        return Err(AppError::BadRequest(format!(
            "文件不存在: {}",
            src.display()
        )));
    }
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string();
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "png".to_string());

    let out_dir: PathBuf = if subdir_mode {
        parent.join("watermarked")
    } else {
        parent.to_path_buf()
    };
    let name_prefix = if subdir_mode {
        stem.clone()
    } else {
        format!("{stem}_wm")
    };

    // 重名自动加序号。
    let mut out_path = out_dir.join(format!("{name_prefix}.{ext}"));
    let mut renamed = false;
    let mut n = 1u32;
    while out_path.exists() {
        renamed = true;
        n += 1;
        out_path = out_dir.join(format!("{name_prefix} ({n}).{ext}"));
    }

    let applied = engine.apply_file(src, &out_path, params)?;
    Ok((applied.output_path, renamed))
}
