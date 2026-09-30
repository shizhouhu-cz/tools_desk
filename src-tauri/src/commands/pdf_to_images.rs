use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::Semaphore;

use crate::config::SettingsState;
use crate::error::{AppError, AppResult};
use crate::task::TaskManager;
use crate::tools::dify::{sanitize_dir_name, DifyClient};
use crate::tools::pdf_to_images::{
    OutputFormat, PdfRenderer, RenderHooks, RenderParams, MAX_DPI, MIN_DPI,
};

const DIFY_TIMEOUT_SEC: u64 = 60;

/// 批量切图的单 PDF 结果。
#[derive(Debug, Serialize)]
pub struct PdfImagesOutcome {
    pub input: String,
    pub ok: bool,
    /// 输出目录（渲染启动后存在）。
    pub output: Option<String>,
    pub pages: usize,
    /// 实际使用的目录名。
    pub dir_name: String,
    /// 目录名来源：stem（默认文件名）/ dify（工作流生成）/ fallback（工作流失败回退文件名）。
    pub dir_source: &'static str,
    /// 失败原因；成功时若为 Some 则是回退告警。
    pub error: Option<String>,
    /// 该 PDF 的处理耗时（毫秒，含目录名解析）。
    pub duration_ms: u64,
}

/// 批量把 PDF 按页渲染为图片。
/// 每个 PDF 输出到 `{父目录}/{目录名}/`，图片按 0001、0002… 四位数字命名；
/// 目录名默认取 PDF 文件名，启用 Dify 时由工作流根据文件名生成。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pdf_to_images(
    app: AppHandle,
    tasks: State<'_, TaskManager>,
    settings: State<'_, SettingsState>,
    inputs: Vec<String>,
    dpi: Option<u32>,
    format: Option<String>,
    quality: Option<u8>,
    start: Option<usize>,
    end: Option<usize>,
    dify_enabled: Option<bool>,
    dify_base_url: Option<String>,
    dify_api_key: Option<String>,
    dify_prompt: Option<String>,
) -> AppResult<Vec<PdfImagesOutcome>> {
    if inputs.is_empty() {
        return Err(AppError::BadRequest("未选择 PDF 文件".into()));
    }

    let task = tasks.create(&app, "pdf-to-images");
    let cfg = settings.0.read().unwrap().clone();

    let fmt = OutputFormat::parse(format.as_deref().unwrap_or("png"))?;
    let params = RenderParams {
        format: fmt,
        dpi: dpi.unwrap_or(cfg.default_dpi).clamp(MIN_DPI, MAX_DPI),
        quality: quality.unwrap_or(cfg.default_quality),
        start,
        end,
    };

    let dify = match dify_enabled.unwrap_or(false) {
        true => Some(DifyClient::new(
            dify_base_url.as_deref().unwrap_or(""),
            dify_api_key.as_deref().unwrap_or(""),
            Duration::from_secs(DIFY_TIMEOUT_SEC),
        )?),
        false => None,
    };

    let total = inputs.len() as u64;
    let semaphore = Arc::new(Semaphore::new(4));
    let completed = Arc::new(AtomicUsize::new(0));
    // 批内目录名去重：不同 PDF 命名冲突时自动加后缀，避免互相覆盖。
    let used_dirs: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    let library_path = Arc::new(resolve_pdfium_path(&app, &cfg.pdfium_path)?);
    let prompt = dify_prompt.unwrap_or_default();

    task.progress_now("prepare", 0, total);

    let mut joins = Vec::with_capacity(inputs.len());
    for input in &inputs {
        if task.is_cancelled() {
            let input = input.clone();
            joins.push(tauri::async_runtime::spawn(async move {
                PdfImagesOutcome {
                    input,
                    ok: false,
                    output: None,
                    pages: 0,
                    dir_name: String::new(),
                    dir_source: "stem",
                    error: Some("任务已取消".into()),
                    duration_ms: 0,
                }
            }));
            continue;
        }
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let input = input.clone();
        let params = params.clone();
        let dify = dify.clone();
        let prompt = prompt.clone();
        let handle = task.clone();
        let completed = completed.clone();
        let used_dirs = used_dirs.clone();
        let library_path = library_path.clone();
        let max_pages = cfg.max_pages;

        joins.push(tauri::async_runtime::spawn(async move {
            let _permit = permit;
            let started = Instant::now();
            let src = PathBuf::from(&input);

            let stem = src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("pdf")
                .to_string();

            // 1. 目录名：Dify 工作流（可选）或文件名。
            let (dir_name, dir_source, note) = match &dify {
                Some(client) => match client.resolve_dir_name(&stem, &prompt).await {
                    Ok(raw) => match sanitize_dir_name(&raw) {
                        Some(name) => (name, "dify", None),
                        None => (
                            stem.clone(),
                            "fallback",
                            Some("工作流返回的目录名为空，已回退为文件名".to_string()),
                        ),
                    },
                    Err(e) => (
                        stem.clone(),
                        "fallback",
                        Some(format!("目录命名工作流调用失败，已回退为文件名: {e}")),
                    ),
                },
                None => (stem.clone(), "stem", None),
            };

            // 2. 批内去重后的输出目录。
            let parent = src
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let out_dir = {
                let mut used = used_dirs.lock().unwrap();
                let mut cand = parent.join(&dir_name);
                let mut n = 2;
                while used.contains(&cand) {
                    cand = parent.join(format!("{dir_name}_{n}"));
                    n += 1;
                }
                used.push(cand.clone());
                cand
            };

            // 3. 渲染。
            let result = render_one(
                handle.clone(),
                library_path.clone(),
                &src,
                &out_dir,
                &params,
                max_pages,
            )
            .await;

            // 4. 汇总结果并推进度。
            let done_now = completed.fetch_add(1, Ordering::Relaxed) + 1;
            handle.progress_now("render", done_now as u64, total);

            let outcome = match result {
                Ok(pages) => PdfImagesOutcome {
                    input: input.clone(),
                    ok: true,
                    output: Some(out_dir.to_string_lossy().to_string()),
                    pages,
                    dir_name,
                    dir_source,
                    error: note,
                    duration_ms: started.elapsed().as_millis() as u64,
                },
                Err(e) => {
                    // 渲染启动失败时清理空目录（非空目录 remove_dir 会失败，忽略即可）。
                    let _ = fs::remove_dir(&out_dir);
                    let error = match note {
                        Some(n) => format!("{n}；{e}"),
                        None => e.to_string(),
                    };
                    PdfImagesOutcome {
                        input: input.clone(),
                        ok: false,
                        output: None,
                        pages: 0,
                        dir_name,
                        dir_source,
                        error: Some(error),
                        duration_ms: started.elapsed().as_millis() as u64,
                    }
                }
            };
            outcome
        }));
    }

    let mut outcomes = Vec::with_capacity(joins.len());
    for j in joins {
        let outcome = j
            .await
            .map_err(|e| AppError::Internal(format!("任务执行失败: {e}")))?;
        outcomes.push(outcome);
    }

    task.progress_now("done", total, total);
    tasks.finish(&task.id);
    Ok(outcomes)
}

/// 渲染单个 PDF 到指定目录，图片按 `{page:04}.{ext}` 命名。
async fn render_one(
    handle: crate::task::TaskHandle,
    library_path: Arc<PathBuf>,
    src: &Path,
    out_dir: &Path,
    params: &RenderParams,
    max_pages: usize,
) -> AppResult<usize> {
    if !src.is_file() {
        return Err(AppError::BadRequest(format!(
            "文件不存在: {}",
            src.display()
        )));
    }
    let input_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| src.display().to_string());
    let src = src.to_path_buf();
    let out_dir = out_dir.to_path_buf();
    let params = params.clone();

    tauri::async_runtime::spawn_blocking(move || {
        if handle.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let renderer = PdfRenderer::new(&library_path)?;
        fs::create_dir_all(&out_dir)
            .map_err(|e| AppError::Internal(format!("创建输出目录失败: {e}")))?;

        let ext = params.format.extension();
        let renderer = Arc::new(renderer);
        let mut hooks = RenderHooks {
            is_cancelled: &|| handle.is_cancelled(),
            on_progress: &|done, total| {
                handle.progress_msg(
                    "render",
                    done as u64,
                    total as u64,
                    Some(format!("{input_name}: {done}/{total} 页")),
                );
            },
            on_page: &mut |page, bytes| {
                let file = out_dir.join(format!("{page:04}.{ext}"));
                fs::write(&file, bytes)
                    .map_err(|e| AppError::Internal(format!("写出图片失败: {e}")))?;
                Ok(())
            },
        };
        renderer.render_to(&src, &params, max_pages, &mut hooks)
    })
    .await
    .map_err(|e| AppError::Internal(format!("任务执行失败: {e}")))?
}

#[derive(Debug, Serialize)]
pub struct DifyProbe {
    pub ok: bool,
    pub message: String,
}

/// 探测 Dify 工作流接口连通性（GET /parameters）。
#[tauri::command]
pub async fn probe_dify(base_url: String, api_key: String) -> AppResult<DifyProbe> {
    let client = DifyClient::new(&base_url, &api_key, Duration::from_secs(15))?;
    let (ok, message) = client.probe().await;
    Ok(DifyProbe { ok, message })
}

/// 解析 pdfium 动态库路径：显式配置 > resources 打包路径 > exe 旁 > 开发目录。
fn resolve_pdfium_path(app: &AppHandle, configured: &str) -> AppResult<PathBuf> {
    if !configured.trim().is_empty() {
        let p = PathBuf::from(configured);
        if p.is_file() {
            return Ok(p);
        }
        return Err(AppError::Internal(format!(
            "设置中配置的 pdfium 库不存在: {configured}"
        )));
    }

    // 1. Tauri resources（dev 解析到 src-tauri/，release 解析到 $RESOURCE/）。
    if let Ok(p) = app.path().resolve(
        "assets/lib/pdfium.dll",
        tauri::path::BaseDirectory::Resource,
    ) {
        if p.is_file() {
            return Ok(p);
        }
    }

    // 2. exe 同目录（手动放置场景）。
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("pdfium.dll");
            if p.is_file() {
                return Ok(p);
            }
        }
    }

    // 3. 开发/测试环境兜底。
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/lib/pdfium.dll");
    if dev.is_file() {
        return Ok(dev);
    }

    Err(AppError::Internal(
        "找不到 pdfium.dll，请在设置页指定路径".into(),
    ))
}
