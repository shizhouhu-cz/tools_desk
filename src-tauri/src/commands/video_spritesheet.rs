use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::task::TaskManager;
use crate::tools::video_spritesheet::{self, SpriteFormat, SpriteParams};

/// 单视频转精灵图结果。
#[derive(Debug, Serialize)]
pub struct SpriteOutcome {
    pub ok: bool,
    pub zip_path: String,
    /// 预览页地址（sprite-preview 自定义协议，WebView2 下为 http://sprite-preview.localhost/...）。
    pub preview_url: String,
    pub frame_count: u64,
    pub requested_fps: f64,
    pub actual_fps: f64,
    pub downsampled: bool,
    pub pages: u64,
    pub cols: u64,
    pub frame_width: u32,
    pub frame_height: u32,
    pub duration_sec: f64,
    pub duration_ms: u64,
    /// 非静默降级提示（帧数超上限），前端需 toast 显式提示。
    pub warning: Option<String>,
}

/// 视频转精灵图：抽帧 → 网格图集 → zip（含 data.js / player.html / 说明.txt）。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn video_spritesheet(
    app: AppHandle,
    tasks: State<'_, TaskManager>,
    input: String,
    fps: Option<f64>,
    max_frame_width: Option<u32>,
    max_sheet_size: Option<u32>,
    format: Option<String>,
    quality: Option<u8>,
    max_frames: Option<u64>,
    output_dir: Option<String>,
) -> AppResult<SpriteOutcome> {
    let src = PathBuf::from(&input);
    if !src.is_file() {
        return Err(AppError::BadRequest(format!("文件不存在: {input}")));
    }

    let task = tasks.create(&app, "video-spritesheet");
    let task_id = task.id.clone();
    let started = Instant::now();

    let params = SpriteParams {
        fps: fps.unwrap_or(24.0).clamp(0.1, 240.0),
        max_frame_width: max_frame_width.unwrap_or(480).clamp(16, 4096),
        max_sheet_size: max_sheet_size.unwrap_or(4096).clamp(256, 16384),
        format: SpriteFormat::parse(format.as_deref().unwrap_or("jpeg"))?,
        quality: quality.unwrap_or(85).clamp(1, 100),
        max_frames: max_frames.unwrap_or(2000).max(1),
    };

    let output_dir = match output_dir {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d),
        _ => src
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };

    // staging：应用数据目录 preview/<taskId>/；新任务覆盖旧 staging（仅保留最近一次产物）。
    let preview_root = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(format!("获取应用数据目录失败: {e}")))?
        .join("preview");
    if preview_root.exists() {
        let _ = std::fs::remove_dir_all(&preview_root);
    }
    let staging = preview_root.join(&task_id);

    let (ffmpeg, ffprobe) = resolve_ffmpeg(&app);

    let handle = task.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        video_spritesheet::run(
            &handle,
            &ffmpeg,
            &ffprobe,
            &src,
            &output_dir,
            &staging,
            &params,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("任务执行失败: {e}")))??;

    tasks.finish(&task.id);
    task.progress_now("done", 1, 1);

    let warning = result.downsampled.then(|| {
        format!(
            "帧数超上限，采样帧率已由 {} 自动降为 {} fps（data.js 与说明.txt 中已注明）",
            video_spritesheet::fmt_fps(result.requested_fps),
            video_spritesheet::fmt_fps(result.actual_fps)
        )
    });

    Ok(SpriteOutcome {
        ok: true,
        zip_path: result.zip_path.to_string_lossy().to_string(),
        preview_url: format!("http://sprite-preview.localhost/{task_id}/player.html"),
        frame_count: result.frame_count,
        requested_fps: result.requested_fps,
        actual_fps: result.actual_fps,
        downsampled: result.downsampled,
        pages: result.pages,
        cols: result.cols,
        frame_width: result.frame_width,
        frame_height: result.frame_height,
        duration_sec: result.duration_sec,
        duration_ms: started.elapsed().as_millis() as u64,
        warning,
    })
}

/// ffmpeg / ffprobe 定位：resources/ffmpeg/（dev 解析 src-tauri、release 解析 $RESOURCE）
/// > exe 旁 > 开发目录兜底；都未命中则退回裸命令名交给系统 PATH。
fn resolve_ffmpeg(app: &AppHandle) -> (String, String) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(p) = app
        .path()
        .resolve("resources/ffmpeg", tauri::path::BaseDirectory::Resource)
    {
        dirs.push(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("resources/ffmpeg"));
            dirs.push(dir.join("ffmpeg"));
        }
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ffmpeg"));

    for dir in dirs {
        let (f, fp) = (dir.join("ffmpeg.exe"), dir.join("ffprobe.exe"));
        if f.is_file() && fp.is_file() {
            return (
                f.to_string_lossy().to_string(),
                fp.to_string_lossy().to_string(),
            );
        }
    }
    ("ffmpeg".into(), "ffprobe".into())
}

// ── sprite-preview 自定义协议：把 staging 目录映射为可被 iframe 加载的页面 ──

const PREVIEW_MIME: &[(&str, &str)] = &[
    ("html", "text/html; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("txt", "text/plain; charset=utf-8"),
];

fn preview_mime(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    PREVIEW_MIME
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, m)| *m)
        .unwrap_or("application/octet-stream")
}

/// 百分号解码（%xx；UTF-8 序列按字节还原）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn not_found(msg: &str) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(404)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(msg.as_bytes().to_vec())
        .unwrap()
}

/// sprite-preview 协议处理器：`http://sprite-preview.localhost/<taskId>/<file>` →
/// 应用数据目录 `preview/<taskId>/<file>`。
pub fn sprite_preview_protocol<R: tauri::Runtime>(
    ctx: tauri::UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let app = ctx.app_handle();
    let uri = request.uri().to_string();
    // WebView2 下形如 http://sprite-preview.localhost/<taskId>/player.html
    let path = match uri.split_once("sprite-preview.localhost/") {
        Some((_, rest)) => rest.split(['?', '#']).next().unwrap_or(""),
        // 非 Windows 平台为 sprite-preview://<host>/<path>
        None => request.uri().path(),
    };
    let path = percent_decode(path);

    let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() < 2 {
        return not_found("无效的预览路径");
    }
    let file = segments.pop().unwrap();
    let task_id = segments[0];
    // 仅允许单层 taskId 目录 + 文件名，禁止路径穿越。
    if task_id.contains("..") || file.contains("..") || file.contains('\\') {
        return not_found("无效的预览路径");
    }

    let root = match app.path().app_data_dir() {
        Ok(d) => d.join("preview").join(task_id),
        Err(_) => return not_found("无法定位预览目录"),
    };
    let target = root.join(file);
    match std::fs::read(&target) {
        Ok(data) => tauri::http::Response::builder()
            .status(200)
            .header("Content-Type", preview_mime(file))
            .header("Cache-Control", "no-store")
            .body(data)
            .unwrap(),
        Err(_) => not_found("预览文件不存在"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_sequences() {
        assert_eq!(percent_decode("%E8%AF%B4%E6%98%8E.txt"), "说明.txt");
        assert_eq!(percent_decode("player.html"), "player.html");
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("bad%2"), "bad%2");
    }

    #[test]
    fn mime_lookup() {
        assert_eq!(preview_mime("player.html"), "text/html; charset=utf-8");
        assert_eq!(preview_mime("data.js"), "text/javascript; charset=utf-8");
        assert_eq!(preview_mime("sprite_0.jpg"), "image/jpeg");
        assert_eq!(preview_mime("说明.txt"), "text/plain; charset=utf-8");
    }
}
