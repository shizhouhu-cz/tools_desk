//! DOCX 结构化拆分：内嵌 Python 运行时子进程运行器。
//!
//! 每个文件冷启动 `python.exe -X utf8 -I -m doc_analyze.cli extract <docx> --db <db> --json`，
//! stdout 为 JSONL 协议行（progress/result/error），人类可读诊断一律走 stderr；
//! 用完即销毁，无常驻进程。

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tracing::warn;

use crate::config::Settings;
use crate::error::{AppError, AppResult};
use crate::task::TaskHandle;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// extract --json result 行中的 stats 字段。
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ExtractStats {
    #[serde(default)]
    pub doc_id: i64,
    #[serde(default)]
    pub chapters: u64,
    #[serde(default)]
    pub tables: u64,
    #[serde(default)]
    pub needs_review: u64,
    #[serde(default)]
    pub total_paragraphs: u64,
    #[serde(default)]
    pub elapsed_seconds: f64,
}

/// 成功解析出的 result 行。
#[derive(Debug, Deserialize, Serialize)]
pub struct ExtractOk {
    #[serde(default)]
    pub doc_id: i64,
    #[serde(default)]
    pub stats: ExtractStats,
}

/// 解析 python.exe 路径：resources 打包路径 > 开发目录兜底。
/// （dev 下 Resource 解析到 src-tauri/，与 setup-python-runtime.ps1 写入位置一致。）
pub fn resolve_python_path(app: &AppHandle) -> AppResult<PathBuf> {
    if let Ok(p) = app.path().resolve(
        "resources/python/python.exe",
        tauri::path::BaseDirectory::Resource,
    ) {
        if p.is_file() {
            return Ok(p);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/python/python.exe");
    if dev.is_file() {
        return Ok(dev);
    }
    Err(AppError::Internal(
        "找不到内置 Python 运行时，请先执行 pnpm setup:python".into(),
    ))
}

/// 构建子进程环境变量。恒定 UTF-8 双保险；启用 LLM 时透传网关配置（非空才传）。
pub fn llm_env(cfg: &Settings, llm_enabled: bool) -> Vec<(String, String)> {
    let mut env = vec![
        ("PYTHONUTF8".to_string(), "1".to_string()),
        ("PYTHONIOENCODING".to_string(), "utf-8".to_string()),
    ];
    if !llm_enabled {
        return env;
    }
    // provider 固定 openai：UI 开关开=走真 LLM，关=python 端默认 mock，无中间态
    let mut pass = |key: &str, v: &str| {
        if !v.trim().is_empty() {
            env.push((key.to_string(), v.trim().to_string()));
        }
    };
    pass("DOC_ANALYZE_LLM_PROVIDER", "openai");
    pass("OPENAI_BASE_URL", &cfg.docx_llm_base_url);
    pass("OPENAI_API_KEY", &cfg.docx_llm_api_key);
    pass("DOC_ANALYZE_OPENAI_AUTH_HEADER", &cfg.docx_llm_auth_header);
    pass("DOC_ANALYZE_RECOVERY_MODEL", &cfg.docx_llm_model);
    env
}

/// 冷启动运行单文件 extract，逐行解析 JSONL 协议。阻塞调用，须在 spawn_blocking 中执行。
///
/// Child 所有权先移交 TaskHandle 注册表（cancel 可 kill），stdout EOF 后取回 wait() 收尸。
pub fn run_extract(
    handle: &TaskHandle,
    python_exe: &Path,
    db_path: &Path,
    input: &str,
    envs: &[(String, String)],
) -> Result<ExtractOk, String> {
    let mut cmd = Command::new(python_exe);
    cmd.args([
        "-X",
        "utf8",
        "-I",
        "-m",
        "doc_analyze.cli",
        "extract",
        input,
        "--db",
    ])
    .arg(db_path)
    .arg("--json")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("python 进程启动失败: {e}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    handle.register_child(child);

    let file_name = Path::new(input)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(input)
        .to_string();

    // stderr 独立线程排空（防管道写满阻塞子进程）；
    // 以 [llm] 开头的诊断行实时转发到进度消息，其余收进尾部缓冲
    let (err_tx, err_rx) = mpsc::channel::<Vec<u8>>();
    let stderr_handle = handle.clone();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = stderr {
            let mut reader = BufReader::new(p);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        buf.extend_from_slice(&line);
                        let s = String::from_utf8_lossy(&line).trim().to_string();
                        if s.starts_with("[llm]") {
                            stderr_handle.progress_msg("llm", 0, 0, Some(s));
                        }
                    }
                }
            }
        }
        let _ = err_tx.send(buf);
    });

    let mut result: Option<ExtractOk> = None;
    let mut error_line: Option<String> = None;
    if let Some(out) = stdout {
        for line in BufReader::new(out).lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue; // 非 JSON 行静默跳过（协议要求诊断走 stderr，此处兜底）
            };
            match v.get("type").and_then(|t| t.as_str()) {
                Some("progress") => {
                    let stage = v.get("stage").and_then(|s| s.as_str()).unwrap_or("extract");
                    let current = v.get("current").and_then(|c| c.as_u64()).unwrap_or(0);
                    let total = v.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
                    let message = v
                        .get("message")
                        .and_then(|m| m.as_str())
                        .filter(|m| !m.is_empty())
                        .map(|m| format!("{file_name}: {m}"));
                    handle.progress_msg(stage, current, total, message);
                }
                Some("result") => match serde_json::from_value::<ExtractOk>(v) {
                    Ok(r) => result = Some(r),
                    Err(e) => warn!("解析 result 行失败: {e}"),
                },
                Some("error") => {
                    error_line = v
                        .get("message")
                        .and_then(|m| m.as_str())
                        .map(|m| m.to_string())
                        .filter(|m| !m.trim().is_empty());
                }
                _ => {}
            }
        }
    }

    // 收尸并取退出码
    let mut code: Option<i32> = None;
    for mut c in handle.take_children() {
        if let Ok(s) = c.wait() {
            code = s.code();
        }
    }
    let stderr_bytes = err_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();
    let tail = stderr_bytes.len().min(8192);
    let stderr_text =
        String::from_utf8_lossy(&stderr_bytes[stderr_bytes.len() - tail..]).to_string();

    if handle.is_cancelled() {
        return Err("任务已取消".to_string());
    }
    if code == Some(0) {
        return result.ok_or_else(|| "python 未输出 result 行".to_string());
    }
    let detail = error_line.or_else(|| {
        let t = stderr_text.trim();
        (!t.is_empty()).then(|| t.chars().take(300).collect::<String>())
    });
    Err(detail.unwrap_or_else(|| format!("python 退出码 {code:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_settings() -> Settings {
        Settings::default()
    }

    #[test]
    fn test_llm_env_disabled() {
        let env = llm_env(&fake_settings(), false);
        assert!(env.iter().any(|(k, v)| k == "PYTHONUTF8" && v == "1"));
        assert!(env.iter().all(|(k, _)| k != "OPENAI_API_KEY"));
    }

    #[test]
    fn test_llm_env_enabled_passes_nonempty() {
        let mut cfg = fake_settings();
        cfg.docx_llm_base_url = "http://gw.internal/v1".into();
        cfg.docx_llm_api_key = " sk-test ".into();
        cfg.docx_llm_model = "qwen-plus".into();
        // auth_header 留空：不传
        let env = llm_env(&cfg, true);
        // provider 恒为 openai
        assert!(env.contains(&("DOC_ANALYZE_LLM_PROVIDER".into(), "openai".into())));
        assert!(env.contains(&("OPENAI_BASE_URL".into(), "http://gw.internal/v1".into())));
        // trim 后传值
        assert!(env.contains(&("OPENAI_API_KEY".into(), "sk-test".into())));
        assert!(env.contains(&("DOC_ANALYZE_RECOVERY_MODEL".into(), "qwen-plus".into())));
        assert!(env
            .iter()
            .all(|(k, _)| k != "DOC_ANALYZE_OPENAI_AUTH_HEADER"));
    }

    #[test]
    fn test_extract_stats_deserialize() {
        let line = r#"{"type":"result","ok":true,"doc_id":1,"input":"a.docx","stats":{"doc_id":1,"total_paragraphs":100,"candidates":30,"anchors":20,"pending":10,"chapters":20,"needs_review":5,"tables":3,"elapsed_seconds":1.2}}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let ok: ExtractOk = serde_json::from_value(v).unwrap();
        assert_eq!(ok.doc_id, 1);
        assert_eq!(ok.stats.chapters, 20);
        assert_eq!(ok.stats.tables, 3);
        assert_eq!(ok.stats.needs_review, 5);
    }
}
