use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use image::DynamicImage;
use pdfium_render::prelude::*;
use serde::Serialize;
use tracing::info;

use crate::error::{AppError, AppResult};

/// Allowed DPI range for rendering (guards against huge bitmap allocations).
pub const MIN_DPI: u32 = 72;
pub const MAX_DPI: u32 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OutputFormat {
    #[serde(rename = "png")]
    Png,
    #[serde(rename = "jpeg")]
    Jpeg,
    #[serde(rename = "webp")]
    Webp,
}

impl OutputFormat {
    pub fn parse(s: &str) -> AppResult<Self> {
        match s.trim().to_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            "webp" => Ok(Self::Webp),
            other => Err(AppError::BadRequest(format!(
                "不支持的输出格式: {other} (支持 png/jpeg/webp)"
            ))),
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderParams {
    pub format: OutputFormat,
    pub dpi: u32,
    /// 有损格式质量 1-100。
    pub quality: u8,
    /// 1-based 闭区间页码范围；None 表示到头。
    pub start: Option<usize>,
    pub end: Option<usize>,
}

/// 逐页渲染回调。
/// - `on_progress(rendered, total_requested)`：每页渲染完成后调用；
/// - `on_page(page_number, bytes)`：写盘，返回 Err 可中断。
pub struct RenderHooks<'a> {
    pub is_cancelled: &'a dyn Fn() -> bool,
    pub on_progress: &'a dyn Fn(usize, usize),
    pub on_page: &'a mut dyn FnMut(usize, &[u8]) -> AppResult<()>,
}

pub struct PdfRenderer {
    pdfium: Arc<Pdfium>,
}

impl PdfRenderer {
    /// Create a renderer by loading the pdfium dynamic library.
    pub fn new(library_path: &Path) -> AppResult<Self> {
        let bindings = Pdfium::bind_to_library(library_path).map_err(|e| {
            AppError::Internal(format!(
                "加载 pdfium 库 ({}) 失败: {e}",
                library_path.display()
            ))
        })?;
        info!("PdfRenderer initialized from {}", library_path.display());
        Ok(Self {
            pdfium: Arc::new(Pdfium::new(bindings)),
        })
    }

    /// 渲染 PDF 指定页码范围为图片，逐页通过 `hooks.on_page` 落盘（避免全量驻留内存）。
    /// 返回实际渲染页数。
    pub fn render_to(
        &self,
        pdf_path: &Path,
        params: &RenderParams,
        max_pages: usize,
        hooks: &mut RenderHooks,
    ) -> AppResult<usize> {
        let data = std::fs::read(pdf_path)
            .map_err(|e| AppError::Internal(format!("读取 PDF 失败: {e}")))?;
        let document = self
            .pdfium
            .load_pdf_from_byte_slice(&data, None)
            .map_err(|e| AppError::RenderingFailed(format!("加载 PDF 失败: {e}")))?;

        let total_pages = document.pages().len() as usize;

        // Calculate page range (1-based, inclusive).
        let start = params.start.unwrap_or(1);
        if start < 1 || start > total_pages {
            return Err(AppError::BadRequest(format!(
                "start 页码 {start} 超出范围 (1-{total_pages})"
            )));
        }
        let end = params.end.unwrap_or(total_pages);
        if end < start || end > total_pages {
            return Err(AppError::BadRequest(format!(
                "end 页码 {end} 超出范围 (start={start}, total={total_pages})"
            )));
        }
        let requested_pages = end - start + 1;
        if requested_pages > max_pages {
            return Err(AppError::BadRequest(format!(
                "请求页数 {requested_pages} 超过限制 (max_pages={max_pages})"
            )));
        }

        // Collect page indices (0-based for pdfium).
        let page_indices: Vec<usize> = (start - 1..end).collect();
        info!(
            "Rendering PDF pages {}-{end} (total {total_pages}), dpi={}, format={:?}",
            start, params.dpi, params.format
        );

        // Render sequentially (pdfium document borrows from Pdfium, making
        // parallel access across threads complex; sequential is safe and
        // still fast enough for typical PPT page counts).
        for (done, &idx) in page_indices.iter().enumerate() {
            if (hooks.is_cancelled)() {
                return Err(AppError::Cancelled);
            }
            let img_bytes =
                render_page(&document, idx, params.dpi, params.format, params.quality)?;
            (hooks.on_progress)(done + 1, requested_pages);
            (hooks.on_page)(idx + 1, &img_bytes)?;
        }

        Ok(page_indices.len())
    }
}

fn render_page(
    document: &PdfDocument,
    page_idx: usize,
    dpi: u32,
    format: OutputFormat,
    quality: u8,
) -> AppResult<Vec<u8>> {
    // pdfium default is 72 DPI; scale factor = dpi / 72.
    let scale = dpi as f32 / 72.0;

    let page = document
        .pages()
        .get(page_idx as u16)
        .map_err(|e| AppError::RenderingFailed(format!("获取页面 {page_idx} 失败: {e}")))?;

    let target_width = ((page.width().value) as f32 * scale).round() as i32;
    let target_height = ((page.height().value) as f32 * scale).round() as i32;

    let config = PdfRenderConfig::new()
        .set_target_width(target_width)
        .set_target_height(target_height)
        .set_format(PdfBitmapFormat::BGRA)
        .set_clear_color(PdfColor::WHITE);

    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| AppError::RenderingFailed(format!("渲染页面 {page_idx} 失败: {e}")))?;

    // `as_image` returns DynamicImage directly (not a Result).
    let dynamic_img: DynamicImage = bitmap.as_image();

    // Convert BGRA -> RGBA for standard image encoding.
    let rgba = dynamic_img.to_rgba8();

    // Encode to the target format.
    let mut buf = Vec::new();
    match format {
        OutputFormat::Png => {
            rgba.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
                .map_err(|e| AppError::RenderingFailed(format!("PNG 编码失败: {e}")))?;
        }
        OutputFormat::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
            encoder
                .encode(
                    &rgb,
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| AppError::RenderingFailed(format!("JPEG 编码失败: {e}")))?;
        }
        OutputFormat::Webp => {
            rgba.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::WebP)
                .map_err(|e| AppError::RenderingFailed(format!("WebP 编码失败: {e}")))?;
        }
    }

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_format() {
        assert!(matches!(OutputFormat::parse("png"), Ok(OutputFormat::Png)));
        assert!(matches!(OutputFormat::parse("JPG"), Ok(OutputFormat::Jpeg)));
        assert!(matches!(OutputFormat::parse("webp"), Ok(OutputFormat::Webp)));
        assert!(OutputFormat::parse("gif").is_err());
    }
}
