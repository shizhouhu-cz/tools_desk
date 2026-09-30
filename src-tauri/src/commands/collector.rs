use std::path::Path;

use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::task::TaskManager;
use crate::tools::file_collector;

/// 收集源目录（含子目录）下的全部文件到 `_collected_时间戳` 临时目录。
#[tauri::command]
pub async fn collect_files(
    app: AppHandle,
    tasks: State<'_, TaskManager>,
    source_dir: String,
) -> AppResult<file_collector::CollectSummary> {
    let task = tasks.create(&app, "file-collector");
    let handle = task.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        let dest_name = format!(
            "_collected_{}",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        let handle_prog = handle.clone();
        let handle_cancel = handle;
        file_collector::collect(
            Path::new(&source_dir),
            &dest_name,
            &move |cur, total| handle_prog.progress("copy", cur, total),
            &move || handle_cancel.is_cancelled(),
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("任务执行失败: {e}")));

    tasks.finish(&task.id);
    result?
}
