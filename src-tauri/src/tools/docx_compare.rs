//! DOCX 标书比对：内嵌 Python 运行时子进程运行器。
//!
//! 冷启动 `python.exe -X utf8 -I -m doc_analyze.cli compare <A> <B> --json`，
//! stdout 为 JSONL 协议行（progress/result/error），诊断走 stderr；
//! result 携带双文档块树 + 片段 + 统计（可能数 MB，单行读取）。

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::task::TaskHandle;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 片段坐标：块 id + 块原始文本偏移（Python 已完成归一化→原始换算）。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FragLoc {
    pub block: u64,
    pub start: u64,
    pub end: u64,
}

/// 重复片段。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CompareFragment {
    pub a: FragLoc,
    pub b: FragLoc,
    pub length: u64,
    /// high | mid | low
    pub confidence: String,
    /// 连续匹配链长度
    pub chain: u64,
    pub content: String,
    /// 标题 | 正文 | 表格
    pub match_type: String,
}

/// compare --json result 行（docs 块树与 stats 直通 JSON）。
#[derive(Debug, Deserialize)]
pub struct CompareOk {
    pub docs: serde_json::Value,
    pub fragments: Vec<CompareFragment>,
    pub stats: serde_json::Value,
}

/// 冷启动运行双文档 compare，逐行解析 JSONL 协议。阻塞调用，须在 spawn_blocking 中执行。
pub fn run_compare(
    handle: &TaskHandle,
    python_exe: &Path,
    path_a: &str,
    path_b: &str,
    envs: &[(String, String)],
) -> Result<CompareOk, String> {
    let mut cmd = Command::new(python_exe);
    cmd.args(["-X", "utf8", "-I", "-m", "doc_analyze.cli", "compare", path_a, path_b, "--json"])
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

    // stderr 独立线程排空（防管道写满阻塞子进程），尾部收进缓冲用于错误诊断
    let (err_tx, err_rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = stderr {
            let mut reader = BufReader::new(p);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => buf.extend_from_slice(&line),
                }
            }
        }
        let _ = err_tx.send(buf);
    });

    let mut result: Option<CompareOk> = None;
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
                    let stage = v.get("stage").and_then(|s| s.as_str()).unwrap_or("compare");
                    let current = v.get("current").and_then(|c| c.as_u64()).unwrap_or(0);
                    let total = v.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
                    let message = v
                        .get("message")
                        .and_then(|m| m.as_str())
                        .filter(|m| !m.is_empty())
                        .map(|m| m.to_string());
                    handle.progress_msg(stage, current, total, message);
                }
                Some("result") => match serde_json::from_value::<CompareOk>(v) {
                    Ok(r) => result = Some(r),
                    Err(e) => warn!("解析 compare result 行失败: {e}"),
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
