pub mod collector;
pub mod docx_structure;
pub mod open;
pub mod pdf_edit;
pub mod pdf_to_images;
pub mod pptx_to_pdf;
pub mod settings;
pub mod sqlite_explorer;
pub mod video_spritesheet;
pub mod watermark;

use serde::Serialize;

/// 单文件处理结果（批量工具通用）。
#[derive(Debug, Serialize)]
pub struct FileOutcome {
    pub input: String,
    pub ok: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_fonts: Vec<String>,
    pub renamed: bool,
    /// 该文件的处理耗时（毫秒）。
    pub duration_ms: u64,
}

impl FileOutcome {
    pub fn success(
        input: &str,
        output: String,
        missing_fonts: Vec<String>,
        renamed: bool,
        duration_ms: u64,
    ) -> Self {
        Self {
            input: input.to_string(),
            ok: true,
            output: Some(output),
            error: None,
            missing_fonts,
            renamed,
            duration_ms,
        }
    }

    pub fn failure(input: &str, error: String, duration_ms: u64) -> Self {
        Self {
            input: input.to_string(),
            ok: false,
            output: None,
            error: Some(error),
            missing_fonts: Vec::new(),
            renamed: false,
            duration_ms,
        }
    }
}
