use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use ab_glyph::{Font, FontArc, FontVec, PxScale, ScaleFont};
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{AppError, AppResult};

/// 水印参数。`type` 区分文本/图片/平铺，文本与平铺共用 [`TextWatermarkParams`]。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WatermarkParams {
    Text(TextWatermarkParams),
    Image(ImageWatermarkParams),
    Tile(TextWatermarkParams),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextWatermarkParams {
    pub text: String,
    pub font_family: Option<String>,
    pub font_size: f32,
    /// hex 颜色 "#RRGGBB" 或 "#RRGGBBAA"。
    pub color: String,
    /// 0.0 - 1.0
    pub opacity: f32,
    /// 顺时针角度。
    pub rotation: f32,
    /// 九宫格位置：top_left / top_center / ... / center / bottom_right。
    pub position: String,
    pub spacing_x: Option<u32>,
    pub spacing_y: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageWatermarkParams {
    /// 水印图片路径（png 带透明最佳）。
    pub watermark_path: String,
    /// 相对原图水印的缩放系数，1.0 = 原尺寸。
    pub scale: f32,
    pub opacity: f32,
    pub rotation: f32,
    pub position: String,
}

pub struct WatermarkEngine {
    font_dir: String,
    default_font: Arc<FontArc>,
    /// Cache of fonts loaded on demand for `font_family` requests.
    font_cache: Mutex<HashMap<String, Arc<FontArc>>>,
}

/// 单图处理结果信息。
#[derive(Debug, Serialize)]
pub struct AppliedImage {
    pub output_path: String,
    pub width: u32,
    pub height: u32,
}

impl WatermarkEngine {
    pub fn new(font_dir: &str, default_font: &str) -> AppResult<Self> {
        let font_data = load_font(font_dir, default_font)?;
        let font = FontArc::try_from_vec(font_data)
            .map_err(|e| AppError::WatermarkFailed(format!("字体加载失败: {e}")))?;
        info!("Watermark font loaded ({default_font})");
        Ok(Self {
            font_dir: font_dir.to_string(),
            default_font: Arc::new(font),
            font_cache: Mutex::new(HashMap::new()),
        })
    }

    /// 处理单张图片并写出。输出格式按 `out_path` 扩展名决定：
    /// PNG 保留透明通道；JPEG/WebP 先平铺到白底。
    pub fn apply_file(
        &self,
        input: &Path,
        out_path: &Path,
        params: &WatermarkParams,
    ) -> AppResult<AppliedImage> {
        let img = load_image(input)?;
        let result = match params {
            WatermarkParams::Text(p) => {
                let color = parse_color(&p.color)?;
                self.apply_text(&img, p, color)
            }
            WatermarkParams::Image(p) => self.apply_image(&img, p),
            WatermarkParams::Tile(p) => {
                let color = parse_color(&p.color)?;
                self.apply_tile(&img, p, color)
            }
        }?;

        let ext = out_path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        let (w, h) = result.dimensions();

        match ext.as_str() {
            "jpg" | "jpeg" => {
                let rgb = flatten_white(&result);
                let mut buf = Vec::new();
                let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
                encoder
                    .encode(
                        rgb.as_raw(),
                        rgb.width(),
                        rgb.height(),
                        image::ExtendedColorType::Rgb8,
                    )
                    .map_err(|e| AppError::WatermarkFailed(format!("JPEG 编码失败: {e}")))?;
                write_file(out_path, &buf)?;
            }
            "webp" => {
                let rgb = flatten_white(&result);
                let encoded = libwebp::WebPEncodeRGB(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    rgb.width() * 3, // stride: bytes per row
                    90.0,
                )
                .map_err(|e| AppError::WatermarkFailed(format!("编码图片失败: {e}")))?;
                write_file(out_path, &encoded.to_vec())?;
            }
            // png（含未知扩展名兜底）：保留 alpha。
            _ => {
                let mut buf = Vec::new();
                image::DynamicImage::ImageRgba8(result)
                    .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                    .map_err(|e| AppError::WatermarkFailed(format!("PNG 编码失败: {e}")))?;
                write_file(out_path, &buf)?;
            }
        }

        Ok(AppliedImage {
            output_path: out_path.to_string_lossy().to_string(),
            width: w,
            height: h,
        })
    }

    /// Resolve the font for a watermark request: the requested `font_family`
    /// if it can be loaded (cached), otherwise the default font.
    fn font_for(&self, font_family: &Option<String>) -> Arc<FontArc> {
        let Some(name) = font_family
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return self.default_font.clone();
        };

        let mut cache = self.font_cache.lock().unwrap();
        if let Some(font) = cache.get(name) {
            return font.clone();
        }
        match load_font(&self.font_dir, name).and_then(|data| {
            FontArc::try_from_vec(data)
                .map_err(|e| AppError::WatermarkFailed(format!("字体 '{name}' 解析失败: {e}")))
        }) {
            Ok(font) => {
                info!("Using requested font '{name}' for watermark");
                let font = Arc::new(font);
                cache.insert(name.to_string(), font.clone());
                font
            }
            Err(e) => {
                warn!("Font '{name}' not available ({e}), falling back to default font");
                self.default_font.clone()
            }
        }
    }

    fn apply_text(
        &self,
        img: &DynamicImage,
        params: &TextWatermarkParams,
        color: [u8; 4],
    ) -> AppResult<RgbaImage> {
        let (w, h) = img.dimensions();
        let mut canvas = img.to_rgba8();

        // Render text to a transparent layer.
        let font = self.font_for(&params.font_family);
        let text_layer = render_text(&font, &params.text, params.font_size, color);

        // Rotate if needed.
        let text_layer = if params.rotation.abs() > 0.1 {
            rotate_image(&text_layer, params.rotation)
        } else {
            text_layer
        };

        // Compute position.
        let (tw, th) = text_layer.dimensions();
        let (x, y) = compute_position(w, h, tw, th, &params.position);

        // Alpha blend with opacity.
        blend(&mut canvas, &text_layer, x, y, params.opacity);

        Ok(canvas)
    }

    fn apply_image(
        &self,
        img: &DynamicImage,
        params: &ImageWatermarkParams,
    ) -> AppResult<RgbaImage> {
        let (w, h) = img.dimensions();
        let mut canvas = img.to_rgba8();

        // Load watermark image.
        let wm_path = Path::new(&params.watermark_path);
        let wm_img = load_image(wm_path)?;

        // Scale.
        let wm_img = if params.scale.abs() > 0.001 && (params.scale - 1.0).abs() > 0.001 {
            let new_w = ((wm_img.width() as f32) * params.scale).round().max(1.0) as u32;
            let new_h = ((wm_img.height() as f32) * params.scale).round().max(1.0) as u32;
            wm_img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3)
        } else {
            wm_img
        };

        // Rotate if needed.
        let wm_img = if params.rotation.abs() > 0.1 {
            DynamicImage::ImageRgba8(rotate_image(&wm_img.to_rgba8(), params.rotation))
        } else {
            wm_img
        };

        let wm_rgba = wm_img.to_rgba8();
        let (tw, th) = wm_rgba.dimensions();
        let (x, y) = compute_position(w, h, tw, th, &params.position);

        blend(&mut canvas, &wm_rgba, x, y, params.opacity);

        Ok(canvas)
    }

    fn apply_tile(
        &self,
        img: &DynamicImage,
        params: &TextWatermarkParams,
        color: [u8; 4],
    ) -> AppResult<RgbaImage> {
        let (w, h) = img.dimensions();
        let mut canvas = img.to_rgba8();

        // Render text to a transparent layer.
        let font = self.font_for(&params.font_family);
        let text_layer = render_text(&font, &params.text, params.font_size, color);

        // Rotate if needed.
        let text_layer = if params.rotation.abs() > 0.1 {
            rotate_image(&text_layer, params.rotation)
        } else {
            text_layer
        };

        let (tw, th) = text_layer.dimensions();
        let spacing_x = params.spacing_x.filter(|s| *s > 0).unwrap_or(tw + 50);
        let spacing_y = params.spacing_y.filter(|s| *s > 0).unwrap_or(th + 50);

        let mut y = 0u32;
        while y < h {
            let mut x = 0u32;
            while x < w {
                blend(&mut canvas, &text_layer, x, y, params.opacity);
                x += spacing_x;
            }
            y += spacing_y;
        }

        Ok(canvas)
    }
}

fn write_file(path: &Path, data: &[u8]) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Internal(format!("创建输出目录失败: {e}")))?;
    }
    std::fs::write(path, data)
        .map_err(|e| AppError::Internal(format!("写出 {} 失败: {e}", path.display())))
}

/// 带尺寸上限的图片解码，防 OOM。
fn load_image(path: &Path) -> AppResult<DynamicImage> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| AppError::WatermarkFailed(format!("打开图片失败 {}: {e}", path.display())))?;
    reader = reader
        .with_guessed_format()
        .map_err(|e| AppError::WatermarkFailed(format!("识别图片格式失败: {e}")))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20000);
    limits.max_image_height = Some(20000);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| AppError::WatermarkFailed(format!("解码图片失败 {}: {e}", path.display())))
}

/// JPEG/WebP 不带 alpha：先把结果平铺到白底再转 RGB。
fn flatten_white(img: &RgbaImage) -> image::RgbImage {
    let mut flattened =
        RgbaImage::from_pixel(img.width(), img.height(), Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut flattened, img, 0, 0);
    DynamicImage::ImageRgba8(flattened).to_rgb8()
}

/// Parse a hex color string like "#FF0000" or "#FF0000FF" into RGBA.
pub fn parse_color(s: &str) -> AppResult<[u8; 4]> {
    let hex = |a: &str, b: &str| {
        u8::from_str_radix(&format!("{a}{b}"), 16)
            .map_err(|_| AppError::BadRequest(format!("颜色格式错误: {s}")))
    };
    let s = s.trim_start_matches('#');
    let chars: Vec<char> = s.chars().collect();
    match chars.len() {
        6 | 8 => {
            let get = |i: usize| -> AppResult<u8> {
                hex(&chars[i * 2].to_string(), &chars[i * 2 + 1].to_string())
            };
            let r = get(0)?;
            let g = get(1)?;
            let b = get(2)?;
            let a = if chars.len() == 8 { get(3)? } else { 255 };
            Ok([r, g, b, a])
        }
        _ => Err(AppError::BadRequest(format!("颜色格式错误: {s}"))),
    }
}

/// Load a font file from the given directory.
/// Searches for the font by name, then falls back to system CJK fonts.
fn load_font(font_dir: &str, font_name: &str) -> AppResult<Vec<u8>> {
    // Build candidate font paths — cross-platform.
    let mut candidates: Vec<String> = Vec::new();

    // 1. Try the configured font_dir + font_name combinations.
    for ext in &["ttf", "otf", "ttc"] {
        candidates.push(format!("{font_dir}/{font_name}.{ext}"));
    }
    candidates.push(format!("{font_dir}/{font_name}"));

    // 2. Windows CJK font fallbacks.
    // simhei.ttf is pure TrueType (not TTC) and renders CJK reliably with ab_glyph.
    candidates.push("C:/Windows/Fonts/simhei.ttf".to_string());
    candidates.push("C:/Windows/Fonts/msyh.ttc".to_string());
    candidates.push("C:/Windows/Fonts/simsun.ttc".to_string());
    candidates.push("C:/Windows/Fonts/Deng.ttf".to_string());

    // 3. Linux CJK font fallbacks.
    candidates.push("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc".to_string());
    candidates.push("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".to_string());
    candidates.push("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".to_string());
    candidates.push("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc".to_string());
    candidates.push("/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc".to_string());
    candidates.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".to_string());

    // 4. macOS CJK font fallbacks.
    candidates.push("/System/Library/Fonts/PingFang.ttc".to_string());
    candidates.push("/Library/Fonts/Arial Unicode.ttf".to_string());

    for path in &candidates {
        debug!("Trying font: {path}");
        if let Ok(data) = std::fs::read(path) {
            // Verify the font can be parsed by ab_glyph.
            if FontVec::try_from_vec(data.clone()).is_ok() {
                info!("Loaded font from {path} ({} bytes)", data.len());
                return Ok(data);
            }
            warn!("Font file found but failed to parse: {path}");
        }
    }

    Err(AppError::WatermarkFailed(format!(
        "无法加载字体 '{font_name}'，搜索目录: {font_dir}。\
         请确保系统安装了 CJK 字体（如 SimHei、Noto Sans CJK、微软雅黑等）"
    )))
}

/// Render text onto a transparent RGBA image using ab_glyph.
/// Supports Unicode/CJK characters properly via ab_glyph's outline rasterization.
fn render_text(font: &FontArc, text: &str, size: f32, color: [u8; 4]) -> RgbaImage {
    let scale = PxScale::from(size);
    let scaled_font = font.as_scaled(scale);

    // Layout: compute positions for each glyph and track the total bounding box.
    let ascent = scaled_font.ascent();
    let descent = scaled_font.descent();
    let line_gap = scaled_font.line_gap();
    let line_height = ascent - descent + line_gap;

    let mut laid_out: Vec<ab_glyph::Glyph> = Vec::new();
    let mut caret_x = 0.0f32;
    let mut caret_y = ascent; // first line baseline
    let mut prev_glyph_id: Option<ab_glyph::GlyphId> = None;
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    for ch in text.chars() {
        if ch == '\n' {
            caret_x = 0.0;
            caret_y += line_height;
            prev_glyph_id = None;
            continue;
        }

        let glyph_id = scaled_font.glyph_id(ch);

        // Apply kerning.
        if let Some(prev_id) = prev_glyph_id {
            caret_x += scaled_font.kern(prev_id, glyph_id);
        }

        let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(caret_x, caret_y));

        // Track bounding box using the outlined glyph's pixel bounds.
        if let Some(outlined) = scaled_font.outline_glyph(glyph.clone()) {
            let px_bounds = outlined.px_bounds();
            min_x = min_x.min(px_bounds.min.x);
            min_y = min_y.min(px_bounds.min.y);
            max_x = max_x.max(px_bounds.max.x);
            max_y = max_y.max(px_bounds.max.y);
        }

        laid_out.push(glyph);
        caret_x += scaled_font.h_advance(glyph_id);
        prev_glyph_id = Some(glyph_id);
    }

    if laid_out.is_empty() || min_x == f32::MAX {
        return RgbaImage::new(1, 1);
    }

    // Calculate image dimensions with padding.
    let pad = 2.0f32;
    let offset_x = -min_x + pad;
    let offset_y = -min_y + pad;
    let width = ((max_x - min_x) as u32 + pad as u32 * 2).max(1);
    let height = ((max_y - min_y) as u32 + pad as u32 * 2).max(1);

    let mut img = RgbaImage::new(width, height);

    // Draw each glyph using ab_glyph's built-in rasterization.
    for glyph in &laid_out {
        if let Some(outlined) = scaled_font.outline_glyph(glyph.clone()) {
            let px_bounds = outlined.px_bounds();
            outlined.draw(|x, y, coverage| {
                // (x, y) are relative to px_bounds.min
                let img_x = (px_bounds.min.x + x as f32 + offset_x) as u32;
                let img_y = (px_bounds.min.y + y as f32 + offset_y) as u32;
                if img_x < width && img_y < height {
                    let alpha = (coverage * color[3] as f32) as u8;
                    if alpha > 0 {
                        let pixel = img.get_pixel_mut(img_x, img_y);
                        // Take max alpha for overlapping glyphs.
                        if alpha > pixel[3] {
                            *pixel = Rgba([color[0], color[1], color[2], alpha]);
                        }
                    }
                }
            });
        }
    }

    img
}

/// Rotate an RGBA image by the given angle (degrees, clockwise positive).
/// 双线性采样（premultiplied alpha 空间插值），相比最近邻无明显锯齿。
fn rotate_image(img: &RgbaImage, angle_deg: f32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let angle_rad = angle_deg.to_radians();
    let (cos, sin) = (angle_rad.cos(), angle_rad.sin());

    let new_w = (w as f32 * cos.abs() + h as f32 * sin.abs()).ceil() as u32;
    let new_h = (w as f32 * sin.abs() + h as f32 * cos.abs()).ceil() as u32;

    let mut result = RgbaImage::new(new_w.max(1), new_h.max(1));

    let cx = new_w as f32 / 2.0;
    let cy = new_h as f32 / 2.0;
    let src_cx = w as f32 / 2.0;
    let src_cy = h as f32 / 2.0;

    for y in 0..result.height() {
        for x in 0..result.width() {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            // 逆旋转变换回源坐标。
            let src_x = dx * cos + dy * sin + src_cx;
            let src_y = -dx * sin + dy * cos + src_cy;
            let pixel = bilinear_sample(img, src_x, src_y);
            result.put_pixel(x, y, pixel);
        }
    }

    result
}

/// premultiplied alpha 空间的双线性采样。
fn bilinear_sample(img: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let (w, h) = img.dimensions();
    if x < 0.0 || y < 0.0 || x > (w as f32 - 1.0) || y > (h as f32 - 1.0) {
        return Rgba([0, 0, 0, 0]);
    }
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    // 预乘 alpha 后插值，避免边缘出现黑色光晕。
    let p = |px: u32, py: u32| {
        let c = img.get_pixel(px, py);
        let a = c[3] as f32;
        [c[0] as f32 * a, c[1] as f32 * a, c[2] as f32 * a, a]
    };
    let (p00, p10, p01, p11) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));

    let mut out = [0f32; 4];
    for i in 0..4 {
        out[i] = p00[i] * (1.0 - fx) * (1.0 - fy)
            + p10[i] * fx * (1.0 - fy)
            + p01[i] * (1.0 - fx) * fy
            + p11[i] * fx * fy;
    }

    let a = out[3];
    if a < 0.5 {
        return Rgba([0, 0, 0, 0]);
    }
    let inv = 255.0 / a;
    Rgba([
        (out[0] * inv).round().min(255.0) as u8,
        (out[1] * inv).round().min(255.0) as u8,
        (out[2] * inv).round().min(255.0) as u8,
        a.round().min(255.0) as u8,
    ])
}

/// Compute the top-left position for placing a watermark of size (tw, th) on an image of size (w, h).
fn compute_position(w: u32, h: u32, tw: u32, th: u32, position: &str) -> (u32, u32) {
    let pad = 10u32;
    match position.to_lowercase().as_str() {
        "top_left" | "topleft" => (pad, pad),
        "top_center" | "topcenter" => ((w - tw) / 2, pad),
        "top_right" | "topright" => (w.saturating_sub(tw + pad), pad),
        "middle_left" | "middleleft" => (pad, (h - th) / 2),
        "center" => ((w - tw) / 2, (h - th) / 2),
        "middle_right" | "middleright" => (w.saturating_sub(tw + pad), (h - th) / 2),
        "bottom_left" | "bottomleft" => (pad, h.saturating_sub(th + pad)),
        "bottom_center" | "bottomcenter" => ((w - tw) / 2, h.saturating_sub(th + pad)),
        "bottom_right" | "bottomright" => (w.saturating_sub(tw + pad), h.saturating_sub(th + pad)),
        _ => ((w - tw) / 2, (h - th) / 2), // default center
    }
}

/// Alpha-blend `layer` onto `canvas` at position (x, y) with given `opacity`.
fn blend(canvas: &mut RgbaImage, layer: &RgbaImage, x: u32, y: u32, opacity: f32) {
    let (cw, ch) = canvas.dimensions();
    let (lw, lh) = layer.dimensions();

    for ly in 0..lh {
        for lx in 0..lw {
            let cx = x + lx;
            let cy = y + ly;
            if cx >= cw || cy >= ch {
                continue;
            }

            let src = layer.get_pixel(lx, ly);
            let dst = canvas.get_pixel(cx, cy);

            let src_alpha = (src[3] as f32 / 255.0) * opacity.clamp(0.0, 1.0);
            if src_alpha < 0.001 {
                continue;
            }

            let r = (src[0] as f32 * src_alpha + dst[0] as f32 * (1.0 - src_alpha)) as u8;
            let g = (src[1] as f32 * src_alpha + dst[1] as f32 * (1.0 - src_alpha)) as u8;
            let b = (src[2] as f32 * src_alpha + dst[2] as f32 * (1.0 - src_alpha)) as u8;
            let a = ((src_alpha + (dst[3] as f32 / 255.0) * (1.0 - src_alpha)) * 255.0) as u8;

            canvas.put_pixel(cx, cy, Rgba([r, g, b, a]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_cases() {
        assert_eq!(parse_color("#FF0000").unwrap(), [255, 0, 0, 255]);
        assert_eq!(parse_color("00FF0080").unwrap(), [0, 255, 0, 128]);
        assert_eq!(parse_color("#ffffff").unwrap(), [255, 255, 255, 255]);
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("#GGGGGG").is_err());
    }

    #[test]
    fn rotate_transparent_image() {
        let img = RgbaImage::from_pixel(100, 40, Rgba([255, 0, 0, 255]));
        let rotated = rotate_image(&img, 45.0);
        // 45 度旋转后包围盒对角线长 ≈ (100+40)/√2 ≈ 99
        assert!(rotated.width() >= 95 && rotated.width() <= 105);
        // 中心点应仍为红色（带轻微插值）。
        let center = rotated.get_pixel(rotated.width() / 2, rotated.height() / 2);
        assert!(center[0] > 200 && center[3] > 200);
    }
}
