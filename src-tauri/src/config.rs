use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

/// 全局设置，持久化到 app_config_dir()/settings.json。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// LibreOffice soffice 可执行文件路径；"soffice" 表示走 PATH 搜索。
    pub soffice_path: String,
    /// 单个文件转换超时（秒）。
    pub libreoffice_timeout_sec: u64,
    /// LibreOffice 并发转换数。
    pub libreoffice_concurrency: usize,
    /// pdfium.dll 路径；留空则按内置 resources 路径解析。
    pub pdfium_path: String,
    /// PDF 渲染默认 DPI。
    pub default_dpi: u32,
    /// 有损编码默认质量 1-100。
    pub default_quality: u8,
    /// 单个 PDF 最大渲染页数。
    pub max_pages: usize,
    /// 水印字体目录。
    pub font_dir: String,
    /// 水印默认字体名。
    pub default_font: String,
    /// docx 拆分数据库目录；留空用 app_data_dir/docx_split/。
    pub docx_db_dir: String,
    /// docx LLM 网关 base_url（OpenAI 兼容）。
    pub docx_llm_base_url: String,
    /// docx LLM API Key（本机工具场景，明文存储）。
    pub docx_llm_api_key: String,
    /// docx LLM 完整 Authorization 头值（内网网关非 Bearer 鉴权时覆盖默认 Bearer <key>）。
    pub docx_llm_auth_header: String,
    /// docx LLM 结构恢复模型名。
    pub docx_llm_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            soffice_path: "soffice".into(),
            libreoffice_timeout_sec: 60,
            libreoffice_concurrency: 2,
            pdfium_path: String::new(),
            default_dpi: 150,
            default_quality: 85,
            max_pages: 200,
            font_dir: "C:/Windows/Fonts".into(),
            default_font: "SimHei".into(),
            docx_db_dir: String::new(),
            docx_llm_base_url: String::new(),
            docx_llm_api_key: String::new(),
            docx_llm_auth_header: String::new(),
            docx_llm_model: "qwen-plus".into(),
        }
    }
}

pub struct SettingsState(pub RwLock<Settings>);

fn settings_file(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Config(e.to_string()))?;
    Ok(dir.join("settings.json"))
}

/// 读取设置；文件不存在或解析失败时返回默认值。
pub fn load(app: &AppHandle) -> Settings {
    settings_file(app)
        .and_then(|p| fs::read_to_string(p).map_err(|e| AppError::Config(e.to_string())))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> AppResult<()> {
    let path = settings_file(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AppError::Config(format!("创建配置目录失败: {e}")))?;
    }
    let text =
        serde_json::to_string_pretty(settings).map_err(|e| AppError::Config(e.to_string()))?;
    fs::write(&path, text).map_err(|e| AppError::Config(format!("写入配置失败: {e}")))
}
