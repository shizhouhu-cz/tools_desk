//! PDF 编辑命令：文字水印、页面提取（单文件另存）。

use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::tools::pdf_edit::{self, PdfEditOutcome, PdfWatermarkParams};

/// 给 PDF 每页添加平铺文字水印。
#[tauri::command]
pub async fn pdf_watermark(
    input: String,
    output: String,
    params: PdfWatermarkParams,
) -> AppResult<PdfEditOutcome> {
    run_blocking(move || {
        pdf_edit::add_text_watermark(Path::new(&input), Path::new(&output), &params)
    })
    .await
}

/// 按页码范围提取 PDF 页面。
#[tauri::command]
pub async fn pdf_extract_pages(
    input: String,
    output: String,
    start_page: u32,
    end_page: u32,
) -> AppResult<PdfEditOutcome> {
    run_blocking(move || {
        pdf_edit::extract_pages(Path::new(&input), Path::new(&output), start_page, end_page)
    })
    .await
}

/// 阻塞任务放到独立线程池执行，避免卡 UI。
async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Internal(format!("任务执行失败: {e}")))?
}
