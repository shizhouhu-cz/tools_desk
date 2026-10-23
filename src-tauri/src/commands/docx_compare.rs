//! DOCX 标书比对命令：python 冷启动子进程跑 compare，透传块树/片段/统计。

use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, State};
use tracing::info;

use crate::config::SettingsState;
use crate::error::{AppError, AppResult};
use crate::task::TaskManager;
use crate::tools::{docx_compare, docx_split};

/// 比对结果（Rust 侧仅透传，块树/statistics 为直通 JSON）。
#[derive(Debug, Serialize)]
pub struct CompareOutcome {
    pub ok: bool,
    pub docs: serde_json::Value,
    pub fragments: Vec<docx_compare::CompareFragment>,
    pub stats: serde_json::Value,
    pub duration_ms: u64,
}

/// 双文档查重比对。A 为基准文档，B 为待查文档（重复率以 B 视角输出）。
#[tauri::command]
pub async fn docx_compare(
    app: AppHandle,
    tasks: State<'_, TaskManager>,
    settings: State<'_, SettingsState>,
    path_a: String,
    path_b: String,
) -> AppResult<CompareOutcome> {
    if path_a.trim().is_empty() || path_b.trim().is_empty() {
        return Err(AppError::BadRequest("请先选择基准与待查文档".into()));
    }

    let task = tasks.create(&app, "docx-compare");
    let cfg = settings.0.read().unwrap().clone();
    let python_exe = docx_split::resolve_python_path(&app)?;
    // compare 不依赖 LLM；仅透传 UTF-8 环境变量
    let envs = docx_split::llm_env(&cfg, false);

    info!("docx_compare: python={}", python_exe.display());

    let handle = task.clone();
    let py = python_exe.clone();
    let a = path_a.clone();
    let b = path_b.clone();
    let started = Instant::now();
    let result =
        tokio::task::spawn_blocking(move || docx_compare::run_compare(&handle, &py, &a, &b, &envs))
            .await
            .map_err(|e| AppError::Internal(format!("任务线程失败: {e}")))?
            .map_err(|e| {
                if e.contains("取消") {
                    AppError::Cancelled
                } else {
                    AppError::Internal(e)
                }
            })?;

    let duration_ms = started.elapsed().as_millis() as u64;
    info!(
        "docx_compare: {} 片段, {} ms",
        result.fragments.len(),
        duration_ms
    );
    task.progress_now("done", 1, 1);
    tasks.finish(&task.id);

    Ok(CompareOutcome {
        ok: true,
        docs: result.docs,
        fragments: result.fragments,
        stats: result.stats,
        duration_ms,
    })
}
