//! PDF 编辑核心逻辑（移植自 pdftoy 项目）。
//!
//! 纯 lopdf 实现，直接操作 PDF 内容流，无外部二进制依赖：
//! - 页面提取：按页码范围重建页面树并另存
//! - 文字水印：嵌入系统 CJK 字体（Type0/Identity-H），按网格平铺半透明文字

use std::path::Path;
use std::time::Instant;

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Dictionary, Document, FontData, Object, ObjectId, Stream, StringFormat};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::error::{AppError, AppResult};
use crate::tools::watermark::parse_color;

/// 文字水印参数。
#[derive(Debug, Clone, Deserialize)]
pub struct PdfWatermarkParams {
    /// 水印文字。
    pub text: String,
    /// 字号（PDF 点）。
    pub font_size: f32,
    /// hex 颜色 "#RRGGBB"。
    pub color: String,
    /// 透明度 0.0 ~ 1.0。
    pub opacity: f32,
    /// 旋转角度（度）。
    pub angle: f32,
    /// 密度系数（>=0.2，越大水印越密）。
    pub density: f32,
}

/// 单次 PDF 编辑结果。
#[derive(Debug, serde::Serialize)]
pub struct PdfEditOutcome {
    pub output: String,
    /// 处理页数（提取 = 提取页数；水印 = 全部页数）。
    pub pages: u32,
    pub duration_ms: u64,
}

/// lopdf 错误统一转换为处理失败。
fn lopdf_err(e: lopdf::Error) -> AppError {
    AppError::ConversionFailed(format!("PDF 处理失败: {e}"))
}

/// 确保输出目录存在后保存文档。
fn save_doc(doc: &mut Document, output_path: &Path) -> AppResult<()> {
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(format!("创建输出目录失败: {e}")))?;
        }
    }
    doc.save(output_path)
        .map(|_| ())
        .map_err(|e| AppError::ConversionFailed(format!("保存 PDF 失败: {e}")))
}

/// 打开并校验 PDF（加密直接拒绝）。
fn load_doc(input_path: &Path) -> AppResult<Document> {
    let doc = Document::load(input_path)
        .map_err(|e| AppError::ConversionFailed(format!("打开 PDF 失败: {e}")))?;
    if doc.is_encrypted() {
        return Err(AppError::BadRequest("PDF 已加密，无法处理".into()));
    }
    Ok(doc)
}

// ============================
// 页面提取
// ============================

/// 从源 PDF 中提取指定连续页码范围，生成新的 PDF 文件。
///
/// 页码从 1 开始，`start_page` <= `end_page`。
pub fn extract_pages(
    input_path: &Path,
    output_path: &Path,
    start_page: u32,
    end_page: u32,
) -> AppResult<PdfEditOutcome> {
    let started = Instant::now();
    info!(
        "PDF 页面提取: {} -> {} ({start_page}-{end_page})",
        input_path.display(),
        output_path.display()
    );

    let mut doc = load_doc(input_path)?;
    let pages = doc.get_pages();
    let total = pages.len() as u32;
    debug!("文档共 {total} 页");

    if start_page > end_page {
        return Err(AppError::BadRequest(format!(
            "起始页({start_page})不能大于结束页({end_page})"
        )));
    }
    if start_page < 1 || end_page > total {
        return Err(AppError::BadRequest(format!(
            "页码越界：{start_page}-{end_page} 超出文档总页数 {total}"
        )));
    }

    let page_ids: Vec<ObjectId> = (start_page..=end_page)
        .filter_map(|p| pages.get(&p).copied())
        .collect();

    // 新建 Pages 根节点，把目标页的 Parent 指向它，再更新 Catalog。
    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let new_pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Count" => page_ids.len() as i64,
        "Kids" => Object::Array(kids),
    });
    for &page_id in &page_ids {
        if let Ok(page_dict) = doc.get_object_mut(page_id).and_then(Object::as_dict_mut) {
            page_dict.set("Parent", Object::Reference(new_pages_id));
        }
    }
    doc.catalog_mut()
        .map_err(lopdf_err)?
        .set("Pages", Object::Reference(new_pages_id));

    save_doc(&mut doc, output_path)?;

    let duration_ms = started.elapsed().as_millis() as u64;
    info!("页面提取完成：{} 页，耗时 {duration_ms}ms", page_ids.len());
    Ok(PdfEditOutcome {
        output: output_path.to_string_lossy().to_string(),
        pages: page_ids.len() as u32,
        duration_ms,
    })
}

// ============================
// 文字水印
// ============================

/// 为 PDF 所有页面添加半透明斜向平铺文字水印。
pub fn add_text_watermark(
    input_path: &Path,
    output_path: &Path,
    params: &PdfWatermarkParams,
) -> AppResult<PdfEditOutcome> {
    let started = Instant::now();

    let text = params.text.trim();
    if text.is_empty() {
        return Err(AppError::BadRequest("水印文字不能为空".into()));
    }
    let font_size = params.font_size.clamp(6.0, 200.0);
    let opacity = params.opacity.clamp(0.0, 1.0);
    let density = if params.density >= 0.2 {
        params.density
    } else {
        1.0
    };
    let color = parse_color(&params.color)?;

    info!(
        "PDF 文字水印: {} -> {} text=\"{text}\" opacity={opacity} angle={} size={font_size}",
        input_path.display(),
        output_path.display(),
        params.angle
    );

    let mut doc = load_doc(input_path)?;

    // 扩展图形状态（ExtGState）：填充与描边透明度。
    let gs_id = doc.add_object(dictionary! {
        "Type" => "ExtGState",
        "ca" => Object::Real(opacity),
        "CA" => Object::Real(opacity),
    });

    // 字体：优先嵌入系统 CJK TrueType 字体，否则 ASCII 回退 Helvetica。
    let (font_id, encoded_text, text_width) = create_watermark_font(&mut doc, text, font_size)?;

    let pages = doc.get_pages();
    let total = pages.len();
    info!("文档共 {total} 页，开始添加水印");

    let angle_rad = params.angle.to_radians();
    let (cos_a, sin_a) = (angle_rad.cos(), angle_rad.sin());
    // 文字垂直偏移（约字号三分之一，用于垂直居中）。
    let text_offset_y = -font_size * 0.35;
    // 平铺间距：density 越大越密。
    let step_x = (text_width + font_size * 2.0) / density;
    let step_y = (font_size * 3.5) / density;

    let [r, g, b, _] = color;
    let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);

    for (page_num, page_id) in &pages {
        let (x0, y0, x1, y1) = get_page_mediabox(&doc, *page_id)?;

        // 注入 ExtGState 与 Font 资源到页面。
        doc.add_graphics_state(*page_id, b"WmGs".to_vec(), gs_id)
            .map_err(lopdf_err)?;
        add_font_to_page_resources(&mut doc, *page_id, b"WmFt", font_id)?;

        // 计算平铺网格位置并构建水印内容流，追加到页面（保证水印在最上层）。
        let positions = compute_tile_positions(x0, y0, x1, y1, step_x, step_y);
        let content = build_tiled_watermark_content(
            &encoded_text,
            font_size,
            cos_a,
            sin_a,
            rf,
            gf,
            bf,
            &positions,
            text_width,
            text_offset_y,
        );
        let encoded = content.encode().map_err(lopdf_err)?;
        doc.add_page_contents(*page_id, encoded)
            .map_err(lopdf_err)?;

        debug!("第 {page_num}/{total} 页水印完成（{} 个）", positions.len());
    }

    save_doc(&mut doc, output_path)?;

    let duration_ms = started.elapsed().as_millis() as u64;
    info!("水印完成，耗时 {duration_ms}ms");
    Ok(PdfEditOutcome {
        output: output_path.to_string_lossy().to_string(),
        pages: total as u32,
        duration_ms,
    })
}

/// 创建水印字体。
///
/// 优先加载系统 CJK TrueType 字体，创建 Type0 复合字体（支持中文）；
/// 未找到 CJK 字体时，纯 ASCII 文本回退 PDF 标准 Helvetica，否则明确报错。
///
/// 返回：字体对象 ID、编码后的文字字节、文字总宽度（PDF 点）。
fn create_watermark_font(
    doc: &mut Document,
    text: &str,
    font_size: f32,
) -> AppResult<(ObjectId, Vec<u8>, f32)> {
    if let Some(font_bytes) = load_cjk_font_data() {
        info!("使用 CJK TrueType 字体嵌入水印");

        let font_data = FontData::new(&font_bytes, "WmCJK".to_string());

        // ttf-parser 解析字体，把每个字符映射为 GID 并累计宽度。
        let face = ttf_parser::Face::parse(&font_bytes, 0)
            .map_err(|e| AppError::ConversionFailed(format!("解析系统字体失败: {e}")))?;

        let units_per_em = face.units_per_em() as f32;
        let mut encoded_text = Vec::new();
        let mut text_width = 0.0f32;

        for ch in text.chars() {
            let gid = face.glyph_index(ch).map(|g| g.0 as u16).unwrap_or(0);
            encoded_text.push((gid >> 8) as u8);
            encoded_text.push((gid & 0xFF) as u8);

            if let Some(gid) = face.glyph_index(ch) {
                let advance = face.glyph_hor_advance(gid).unwrap_or(0) as f32;
                text_width += advance * font_size / units_per_em;
            }
        }

        let font_id = build_type0_font(doc, &font_data, &font_bytes)?;
        Ok((font_id, encoded_text, text_width))
    } else if text.is_ascii() {
        warn!("未找到系统 CJK 字体，回退到 Helvetica（仅支持 ASCII）");
        let font_dict = dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        };
        let font_id = doc.add_object(font_dict);
        let encoded_text = text.as_bytes().to_vec();
        // Helvetica 平均字符宽度 ≈ 0.5 * font_size。
        let text_width = text.chars().count() as f32 * font_size * 0.5;
        Ok((font_id, encoded_text, text_width))
    } else {
        Err(AppError::ConversionFailed(
            "未找到系统中文字体，无法为含中文的水印文字编码（请安装微软雅黑或思源黑体）".into(),
        ))
    }
}

/// 构建 PDF Type0 复合字体（支持 CJK）。
///
/// 结构: Type0 → CIDFontType2 → FontDescriptor → FontFile2(TTF)，
/// 编码 Identity-H（2 字节 GID 映射，CID = GID）。
fn build_type0_font(
    doc: &mut Document,
    font_data: &FontData,
    font_bytes: &[u8],
) -> AppResult<ObjectId> {
    let font_name = font_data.font_name.clone();

    let font_stream = Stream::new(
        dictionary! {
            "Length1" => Object::Integer(font_bytes.len() as i64),
        },
        font_bytes.to_vec(),
    );
    let font_file_id = doc.add_object(font_stream);

    let font_descriptor_id = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => Object::Name(font_name.clone().into_bytes()),
        "Flags" => Object::Integer(font_data.flags),
        "FontBBox" => Object::Array(vec![
            Object::Integer(font_data.font_bbox.0),
            Object::Integer(font_data.font_bbox.1),
            Object::Integer(font_data.font_bbox.2),
            Object::Integer(font_data.font_bbox.3),
        ]),
        "ItalicAngle" => Object::Integer(font_data.italic_angle),
        "Ascent" => Object::Integer(font_data.ascent),
        "Descent" => Object::Integer(font_data.descent),
        "CapHeight" => Object::Integer(font_data.cap_height),
        "StemV" => Object::Integer(font_data.stem_v),
        "FontFile2" => Object::Reference(font_file_id),
    });

    let cid_font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => Object::Name(font_name.clone().into_bytes()),
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::String(b"Adobe".to_vec(), StringFormat::Literal),
            "Ordering" => Object::String(b"Identity".to_vec(), StringFormat::Literal),
            "Supplement" => Object::Integer(0),
        },
        "FontDescriptor" => Object::Reference(font_descriptor_id),
        "CIDToGIDMap" => "Identity",
    });

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => Object::Name(font_name.into_bytes()),
        "Encoding" => "Identity-H",
        "DescendantFonts" => Object::Array(vec![Object::Reference(cid_font_id)]),
    });

    Ok(font_id)
}

/// 加载系统 CJK TrueType 字体数据（按优先级尝试常见路径）。
fn load_cjk_font_data() -> Option<Vec<u8>> {
    let candidates: &[&str] = &[
        // Windows: 微软雅黑 / 宋体
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simsun.ttc",
        // macOS: 苹方
        "/System/Library/Fonts/PingFang.ttc",
        // Linux: 思源黑体 / 文泉驿
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
    ];

    for path in candidates {
        if let Ok(data) = std::fs::read(path) {
            info!("加载 CJK 字体: {path}");
            return Some(data);
        }
    }

    None
}

/// 计算平铺网格位置：相邻行交错偏移半个步长，形成菱形布局；
/// 网格向四周各扩展一个步长，保证旋转后边缘也被覆盖。
fn compute_tile_positions(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    step_x: f32,
    step_y: f32,
) -> Vec<(f32, f32)> {
    let mut positions = Vec::new();

    let start_x = x0 - step_x;
    let end_x = x1 + step_x;
    let start_y = y0 - step_y;
    let end_y = y1 + step_y;

    let mut row = 0;
    let mut y = start_y;
    while y <= end_y {
        let x_offset = if row % 2 == 1 { step_x / 2.0 } else { 0.0 };
        let mut x = start_x + x_offset;
        while x <= end_x {
            positions.push((x, y));
            x += step_x;
        }
        y += step_y;
        row += 1;
    }

    positions
}

/// 构建平铺水印内容流：
///
/// ```text
/// q                ← 外层保存状态
/// gs /WmGs         ← 设置透明度（仅一次）
/// [每个位置]:
///   q cm 平移 cm 旋转 BT..ET  Q
/// Q                ← 外层恢复
/// ```
#[allow(clippy::too_many_arguments)]
fn build_tiled_watermark_content(
    encoded_text: &[u8],
    font_size: f32,
    cos_a: f32,
    sin_a: f32,
    r: f32,
    g: f32,
    b: f32,
    positions: &[(f32, f32)],
    text_width: f32,
    text_offset_y: f32,
) -> Content<Vec<Operation>> {
    let mut operations = Vec::new();

    operations.push(Operation::new("q", vec![]));
    operations.push(Operation::new("gs", vec![Object::Name(b"WmGs".to_vec())]));

    for &(x, y) in positions {
        operations.push(Operation::new("q", vec![]));
        // 平移到网格位置。
        operations.push(Operation::new(
            "cm",
            vec![
                1.0f32.into(),
                0.0f32.into(),
                0.0f32.into(),
                1.0f32.into(),
                x.into(),
                y.into(),
            ],
        ));
        // 旋转坐标系。
        operations.push(Operation::new(
            "cm",
            vec![
                cos_a.into(),
                sin_a.into(),
                (-sin_a).into(),
                cos_a.into(),
                0.0f32.into(),
                0.0f32.into(),
            ],
        ));
        // 绘制文字。
        operations.push(Operation::new("BT", vec![]));
        operations.push(Operation::new(
            "Tf",
            vec![Object::Name(b"WmFt".to_vec()), font_size.into()],
        ));
        operations.push(Operation::new("rg", vec![r.into(), g.into(), b.into()]));
        operations.push(Operation::new(
            "Td",
            vec![(-text_width / 2.0).into(), text_offset_y.into()],
        ));
        operations.push(Operation::new(
            "Tj",
            vec![Object::string_literal(encoded_text.to_vec())],
        ));
        operations.push(Operation::new("ET", vec![]));
        operations.push(Operation::new("Q", vec![]));
    }

    operations.push(Operation::new("Q", vec![]));

    Content { operations }
}

/// 把字体资源注入页面 Resources 字典（判空追加，不覆盖已有资源）。
fn add_font_to_page_resources(
    doc: &mut Document,
    page_id: ObjectId,
    font_name: &[u8],
    font_id: ObjectId,
) -> AppResult<()> {
    // Phase 1: 只读检查 Font 条目类型。
    let font_ref_id = {
        let resources = doc.get_or_create_resources(page_id).map_err(lopdf_err)?;
        let resources = resources.as_dict().map_err(lopdf_err)?;
        if resources.has(b"Font") {
            match resources.get(b"Font") {
                Ok(Object::Reference(id)) => Some(*id),
                _ => None, // 直接字典或其他类型
            }
        } else {
            None
        }
    };

    // Phase 2: 修改 Font 字典。
    if let Some(ref_id) = font_ref_id {
        let font_dict = doc
            .get_object_mut(ref_id)
            .map_err(lopdf_err)?
            .as_dict_mut()
            .map_err(lopdf_err)?;
        font_dict.set(font_name, Object::Reference(font_id));
    } else {
        let resources = doc.get_or_create_resources(page_id).map_err(lopdf_err)?;
        let resources = resources.as_dict_mut().map_err(lopdf_err)?;
        if !resources.has(b"Font") {
            resources.set("Font", Dictionary::new());
        }
        let fonts = resources
            .get_mut(b"Font")
            .map_err(lopdf_err)?
            .as_dict_mut()
            .map_err(lopdf_err)?;
        fonts.set(font_name, Object::Reference(font_id));
    }

    Ok(())
}

/// 解析页面 MediaBox（沿 Parent 链向上查找，均未找到时用 A4 默认尺寸）。
fn get_page_mediabox(doc: &Document, page_id: ObjectId) -> AppResult<(f32, f32, f32, f32)> {
    let mut current_id = page_id;

    // 深度限制防止循环引用。
    for _ in 0..50 {
        let page = doc.get_dictionary(current_id).map_err(lopdf_err)?;

        if let Ok(mediabox) = page.get(b"MediaBox") {
            let (_, mediabox) = doc.dereference(mediabox).map_err(lopdf_err)?;
            let arr = mediabox.as_array().map_err(lopdf_err)?;
            if arr.len() < 4 {
                return Err(AppError::ConversionFailed(
                    "无法解析页面尺寸(MediaBox)".into(),
                ));
            }
            let x0 = arr[0].as_float().map_err(lopdf_err)?;
            let y0 = arr[1].as_float().map_err(lopdf_err)?;
            let x1 = arr[2].as_float().map_err(lopdf_err)?;
            let y1 = arr[3].as_float().map_err(lopdf_err)?;
            return Ok((x0, y0, x1, y1));
        }

        match page.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent_id) => current_id = parent_id,
            Err(_) => break,
        }
    }

    warn!("未找到 MediaBox，使用 A4 默认尺寸");
    Ok((0.0, 0.0, 595.0, 842.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_positions_cover_page_and_stagger_rows() {
        // A4 页面，步长 100x100 → 覆盖 [-100, 695] x [-100, 942]。
        let positions = compute_tile_positions(0.0, 0.0, 595.0, 842.0, 100.0, 100.0);
        assert!(!positions.is_empty());
        // 首行无偏移，第二行偏移半个步长。
        let first_row_y = positions[0].1;
        let second = positions.iter().find(|(_, y)| *y > first_row_y).unwrap();
        let first_x = positions.iter().find(|(_, y)| *y == first_row_y).unwrap().0;
        assert!(((second.0 - first_x) - 50.0).abs() < 1e-3);
    }

    #[test]
    fn watermark_rejects_empty_text() {
        let params = PdfWatermarkParams {
            text: "  ".into(),
            font_size: 48.0,
            color: "#808080".into(),
            opacity: 0.3,
            angle: 45.0,
            density: 1.0,
        };
        let err = add_text_watermark(Path::new("nonexistent.pdf"), Path::new("out.pdf"), &params)
            .unwrap_err();
        assert!(err.to_string().contains("水印文字"));
    }

    /// 构造一个 2 页的最小 PDF（A4，各带一个简单内容流与 Helvetica 字体）。
    fn make_sample_pdf(path: &Path) {
        use lopdf::content::Operation;

        let mut doc = Document::new();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let pages_id = doc.add_object(dictionary! {
            "Type" => "Pages",
            "Count" => 0,
            "Kids" => Object::Array(vec![]),
        });
        let mut kids = Vec::new();
        for text in ["Hello", "World"] {
            let content = Content {
                operations: vec![Operation::new(
                    "Tj",
                    vec![Object::string_literal(text.as_bytes().to_vec())],
                )],
            }
            .encode()
            .unwrap();
            let content_id = doc.add_object(Stream::new(dictionary! {}, content));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => Object::Reference(pages_id),
                "MediaBox" => Object::Array(vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(595),
                    Object::Integer(842),
                ]),
                "Contents" => Object::Reference(content_id),
                "Resources" => dictionary! {
                    "Font" => dictionary! { "F1" => Object::Reference(font_id) },
                },
            });
            kids.push(Object::Reference(page_id));
        }
        {
            let pages = doc.get_object_mut(pages_id).unwrap().as_dict_mut().unwrap();
            pages.set("Count", Object::Integer(kids.len() as i64));
            pages.set("Kids", Object::Array(kids));
        }
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc.save(path).unwrap();
    }

    #[test]
    fn extract_and_watermark_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.pdf");
        make_sample_pdf(&src);
        assert_eq!(Document::load(&src).unwrap().get_pages().len(), 2);

        // 页面提取：取第 2 页 → 输出应为 1 页。
        let extracted = dir.path().join("extracted.pdf");
        let outcome = extract_pages(&src, &extracted, 2, 2).unwrap();
        assert_eq!(outcome.pages, 1);
        assert_eq!(Document::load(&extracted).unwrap().get_pages().len(), 1);

        // 页码越界报参数错误。
        let err = extract_pages(&src, &extracted, 1, 3).unwrap_err();
        assert!(err.to_string().contains("越界"));

        // 文字水印：两页内容流都应包含 ExtGState 引用 WmGs。
        let marked = dir.path().join("marked.pdf");
        let params = PdfWatermarkParams {
            text: "内部资料".into(),
            font_size: 48.0,
            color: "#808080".into(),
            opacity: 0.3,
            angle: 45.0,
            density: 1.0,
        };
        let outcome = add_text_watermark(&src, &marked, &params).unwrap();
        assert_eq!(outcome.pages, 2);

        let doc = Document::load(&marked).unwrap();
        for (_, page_id) in doc.get_pages() {
            // get_page_content 返回解码后的页面内容流，水印内容流应包含 ExtGState 名字。
            let content = doc.get_page_content(page_id);
            assert!(!content.is_empty());
            assert!(
                content.windows(4).any(|w| w == b"WmGs"),
                "页面缺少水印透明度操作"
            );
        }
    }
}
