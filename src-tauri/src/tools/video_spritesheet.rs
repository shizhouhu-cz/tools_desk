use std::borrow::Cow;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use image::RgbaImage;
use serde::Deserialize;
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::task::TaskHandle;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// stderr 诊断信息最多保留的字节数。
const STDERR_CAP: usize = 16 * 1024;

/// 帧率格式化：最多两位小数并去掉无意义的尾零（15.00 -> "15"，15.20 -> "15.2"）。
pub(crate) fn fmt_fps(v: f64) -> String {
    format!("{v:.2}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    #[cfg(not(windows))]
    let _ = cmd;
}

/// 图集编码格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteFormat {
    Jpeg,
    Png,
}

impl SpriteFormat {
    pub fn parse(s: &str) -> AppResult<Self> {
        match s.trim().to_lowercase().as_str() {
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            other => Err(AppError::BadRequest(format!(
                "不支持的图集格式: {other} (支持 jpeg/png)"
            ))),
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpriteParams {
    /// 用户请求的采样帧率。
    pub fps: f64,
    /// 单帧最大宽度（只缩不放，等比，高取偶）。
    pub max_frame_width: u32,
    /// 图集边长上限（宽/高共用）。
    pub max_sheet_size: u32,
    pub format: SpriteFormat,
    /// JPEG 质量 1-100。
    pub quality: u8,
    /// 帧数上限（同时充当内存保护阀）。
    pub max_frames: u64,
    /// 是否生成精灵图集（与 GIF 相互独立的可选项，至少二者选一）。
    pub generate_sprite: bool,
    /// 是否生成 GIF（复用 RGBA 帧缓冲，v0.4）。
    pub generate_gif: bool,
    /// GIF 帧率上限（None 或 0 = 跟随采样帧率，不跳帧）。
    pub gif_fps: Option<u32>,
    /// GIF 是否启用 Floyd-Steinberg 抖动。
    pub gif_dither: bool,
}

#[derive(Debug, Clone)]
pub struct GifMeta {
    /// 文件名（位于 staging 目录与 zip 内）。
    pub name: String,
    /// GIF 有效帧率（跳帧后）。
    pub fps: f64,
    pub frames: u64,
    pub dither: bool,
}

#[derive(Debug, Clone)]
pub struct SpriteResult {
    pub zip_path: PathBuf,
    pub staging_dir: PathBuf,
    pub frame_count: u64,
    pub requested_fps: f64,
    pub actual_fps: f64,
    pub downsampled: bool,
    pub pages: u64,
    pub cols: u64,
    pub frame_width: u32,
    pub frame_height: u32,
    /// 视频时长（秒）。
    pub duration_sec: f64,
    /// GIF 产物（generate_gif 时存在）。
    pub gif: Option<GifMeta>,
}

#[derive(Debug, Deserialize)]
struct ProbeOutput {
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

struct VideoInfo {
    width: u32,
    height: u32,
    source_fps: f64,
    duration_sec: f64,
}

/// 解析 "30000/1001" 形式的分数帧率，返回 fps；0/0 或解析失败返回 None。
fn parse_fraction(s: &str) -> Option<f64> {
    let (a, b) = s.split_once('/')?;
    let num: f64 = a.trim().parse().ok()?;
    let den: f64 = b.trim().parse().ok()?;
    if den == 0.0 {
        return None;
    }
    let v = num / den;
    (v > 0.0).then_some(v)
}

fn probe_video(ffprobe: &str, input: &Path) -> AppResult<VideoInfo> {
    let input_str = input.to_string_lossy().to_string();
    let mut cmd = Command::new(ffprobe);
    no_window(&mut cmd);
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        &input_str,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let output = cmd.output().map_err(|e| {
        AppError::Internal(format!(
            "启动 ffprobe 失败: {e}（可运行 pnpm setup:ffmpeg 安装内置 ffmpeg）"
        ))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Internal(format!(
            "ffprobe 探测失败: {}",
            stderr.trim()
        )));
    }

    let probe: ProbeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| AppError::Internal(format!("解析 ffprobe 输出失败: {e}")))?;
    let stream = probe
        .streams
        .first()
        .ok_or_else(|| AppError::BadRequest("文件中没有视频流".into()))?;

    let width = stream.width.unwrap_or(0);
    let height = stream.height.unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(AppError::BadRequest("无法获取视频分辨率".into()));
    }

    let source_fps = stream
        .avg_frame_rate
        .as_deref()
        .and_then(parse_fraction)
        .or_else(|| stream.r_frame_rate.as_deref().and_then(parse_fraction))
        .unwrap_or(30.0);

    let duration_sec = probe
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .or(stream.duration.as_deref())
        .and_then(|d| d.trim().parse::<f64>().ok())
        .filter(|d| *d > 0.0)
        .ok_or_else(|| AppError::BadRequest("无法获取视频时长（文件损坏或不完整）".into()))?;

    Ok(VideoInfo {
        width,
        height,
        source_fps,
        duration_sec,
    })
}

/// 计算实际采样帧率：先钳制到源帧率，再按帧数上限降采样。
/// 返回 (实际帧率, 是否降级, 预估帧数)。
fn compute_actual_fps(
    requested: f64,
    source_fps: f64,
    duration_sec: f64,
    max_frames: u64,
) -> (f64, bool, u64) {
    let clamped = if source_fps > 0.0 {
        requested.min(source_fps)
    } else {
        requested
    }
    .max(0.1);
    let est = (duration_sec * clamped).ceil() as u64;
    if est > max_frames && duration_sec > 0.0 {
        let actual = (max_frames as f64 / duration_sec).min(clamped).max(0.1);
        let est = (duration_sec * actual).ceil() as u64;
        (actual, true, est)
    } else {
        (clamped, false, est)
    }
}

/// 计算抽帧后的单帧尺寸：宽度只缩不放并取偶，高度等比取偶。
/// 显式传给 ffmpeg `scale=W:H`，保证 rawvideo 字节流按确定的帧尺寸切片。
fn compute_frame_size(src_w: u32, src_h: u32, max_w: u32) -> (u32, u32) {
    let w = max_w.min(src_w).max(2);
    let w = if w % 2 == 1 { w - 1 } else { w };
    let h = ((src_h as f64 * w as f64 / src_w as f64) / 2.0)
        .round()
        .max(1.0) as u32
        * 2;
    (w, h)
}

/// 网格布局（方案 §4）：绝不出现单行超长图，宽高受上限约束，末页/末行允许不满。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub cols: u64,
    pub rows: u64,
    pub rows_per_page: u64,
    pub pages: u64,
}

fn compute_layout(n: u64, frame_w: u32, frame_h: u32, max_sheet: u32) -> AppResult<Layout> {
    if n == 0 {
        return Err(AppError::BadRequest("帧数为 0".into()));
    }
    if frame_w as u64 > max_sheet as u64 || frame_h as u64 > max_sheet as u64 {
        return Err(AppError::BadRequest(format!(
            "单帧尺寸 {frame_w}×{frame_h} 超过图集边长上限 {max_sheet}，请降低单帧宽度"
        )));
    }
    let max_cols = max_sheet as u64 / frame_w as u64;
    let cols = (((n as f64).sqrt()).ceil() as u64).clamp(1, max_cols);
    let rows = n.div_ceil(cols);
    let rows_per_page = (max_sheet as u64 / frame_h as u64).max(1);
    let pages = rows.div_ceil(rows_per_page);
    Ok(Layout {
        cols,
        rows,
        rows_per_page,
        pages,
    })
}

/// 读取子进程 stderr（线程内使用），超长时只保留末尾。
fn drain_stderr(stderr: impl Read) -> String {
    let mut all = Vec::new();
    let mut buf = [0u8; 4096];
    let mut rd = stderr;
    while let Ok(n) = rd.read(&mut buf) {
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n]);
        if all.len() > STDERR_CAP * 2 {
            // 只留最后一段，避免大体积驻留
            all.drain(..all.len() - STDERR_CAP);
        }
    }
    String::from_utf8_lossy(&all).trim().to_string()
}

/// 取回全部已注册子进程并等待退出（取消路径收尸，避免句柄泄漏）。
fn reap_children(handle: &TaskHandle) {
    for mut c in handle.take_children() {
        let _ = c.wait();
    }
}

/// 主入口：probe → decode → compose（可选）→ gif（可选）→ pack。
/// 产物写入 `staging`，zip 写入 `output_dir/<stem>_sprite.zip`。
pub fn run(
    handle: &TaskHandle,
    ffmpeg: &str,
    ffprobe: &str,
    input: &Path,
    output_dir: &Path,
    staging: &Path,
    params: &SpriteParams,
) -> AppResult<SpriteResult> {
    // ── ① probe ─────────────────────────────────────────────
    handle.progress_now("probe", 0, 1);
    let info = probe_video(ffprobe, input)?;
    info!(
        "视频探测完成: {}×{}, {} fps, {:.2}s",
        info.width, info.height, info.source_fps, info.duration_sec
    );

    let (actual_fps, downsampled, est_frames) = compute_actual_fps(
        params.fps,
        info.source_fps,
        info.duration_sec,
        params.max_frames,
    );
    let (frame_w, frame_h) = compute_frame_size(info.width, info.height, params.max_frame_width);
    let frame_bytes = frame_w as usize * frame_h as usize * 4;

    // 帧数超上限：在启动 ffmpeg 前降采样，并全链路显式提示（这里先提示进度面板，
    // toast/结果区/data.js/说明.txt 由 command 层与产物补充）。
    if downsampled {
        handle.progress_now("probe", 1, 1);
        handle.progress_msg(
            "decode",
            0,
            est_frames,
            Some(format!(
                "帧数超上限，采样帧率已由 {} 自动降为 {} fps",
                fmt_fps(params.fps),
                fmt_fps(actual_fps)
            )),
        );
    }

    // ── ② decode：rawvideo 管道流式收帧 ─────────────────────
    let input_str = input.to_string_lossy().to_string();
    let mut cmd = Command::new(ffmpeg);
    no_window(&mut cmd);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-y",
        "-i",
        &input_str,
        "-map",
        "0:v:0",
        "-an",
        "-vf",
        &format!("scale={frame_w}:{frame_h},fps={actual_fps:.6}"),
        "-pix_fmt",
        "rgba",
        "-f",
        "rawvideo",
        "pipe:1",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        AppError::Internal(format!(
            "启动 ffmpeg 失败: {e}（可运行 pnpm setup:ffmpeg 安装内置 ffmpeg）"
        ))
    })?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    // 注册后取消时会统一 kill，管道随之关闭、读取循环退出。
    handle.register_child(child);

    let stderr_thread = std::thread::spawn(move || drain_stderr(stderr));

    let est_cap = est_frames.clamp(0, 8000) as usize;
    let mut all: Vec<u8> = Vec::with_capacity(frame_bytes.saturating_mul(est_cap));
    let mut chunk = vec![0u8; 1 << 20];
    let mut last_emit = Instant::now();
    loop {
        if handle.is_cancelled() {
            reap_children(handle);
            return Err(AppError::Cancelled);
        }
        let n = stdout
            .read(&mut chunk)
            .map_err(|e| AppError::Internal(format!("读取 ffmpeg 输出失败: {e}")))?;
        if n == 0 {
            break;
        }
        all.extend_from_slice(&chunk[..n]);
        if last_emit.elapsed() >= Duration::from_millis(100) {
            let frames = (all.len() / frame_bytes) as u64;
            handle.progress("decode", frames, est_frames);
            last_emit = Instant::now();
        }
    }

    // 收尸 ffmpeg 并检查退出状态（stderr 是 rawvideo 模式下唯一的诊断来源）。
    let mut status: Option<ExitStatus> = None;
    for mut c in handle.take_children() {
        match c.wait() {
            Ok(s) if status.is_none() => status = Some(s),
            Ok(_) => {}
            Err(e) => warn!("等待 ffmpeg 退出失败: {e}"),
        }
    }
    let stderr_text = stderr_thread.join().unwrap_or_default();
    if handle.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let ok = status.map(|s| s.success()).unwrap_or(false);
    if !ok {
        return Err(AppError::Internal(format!(
            "ffmpeg 抽帧失败: {}",
            if stderr_text.is_empty() {
                format!("退出码异常（帧尺寸 {frame_w}×{frame_h}）")
            } else {
                stderr_text
            }
        )));
    }

    // 丢弃不完整的尾部字节（正常应为 0）。
    let tail = all.len() % frame_bytes;
    if tail != 0 {
        warn!(
            "rawvideo 流存在 {} 字节尾部残余，已丢弃（帧尺寸可能与预期不符）",
            tail
        );
        all.truncate(all.len() - tail);
    }
    let frame_count = (all.len() / frame_bytes) as u64;
    if frame_count == 0 {
        return Err(AppError::BadRequest(
            "未从视频中抽到任何帧（视频过短或损坏）".into(),
        ));
    }
    info!("抽帧完成: {frame_count} 帧, 单帧 {frame_w}×{frame_h}");
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".into());

    std::fs::create_dir_all(staging)
        .map_err(|e| AppError::Internal(format!("创建暂存目录失败: {e}")))?;

    // ── ③ compose：布局 + 逐页编码（可选项，仅精灵图开启时）──
    let (layout, page_files) = if params.generate_sprite {
        let layout = compute_layout(frame_count, frame_w, frame_h, params.max_sheet_size)?;
        let sheet_w = (layout.cols as u32) * frame_w;
        handle.progress_now("compose", 0, layout.pages);

        let ext = params.format.extension();
        let rows_per_page = layout.rows_per_page;
        let mut page_files: Vec<String> = Vec::with_capacity(layout.pages as usize);
        for page in 0..layout.pages {
            if handle.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            let first_frame = page * rows_per_page * layout.cols;
            let rows_this = (layout.rows - page * rows_per_page).min(rows_per_page);
            let frames_this = frame_count - first_frame;
            let frames_this = frames_this.min(rows_this * layout.cols);
            let sheet_h = (rows_this as u32) * frame_h;

            let encoded = encode_page(
                &all[(first_frame as usize * frame_bytes)..],
                frames_this as usize,
                layout.cols as u32,
                frame_w,
                frame_h,
                sheet_w,
                sheet_h,
                params.format,
                params.quality,
            )?;
            let name = format!("sprite_{page}.{ext}");
            std::fs::write(staging.join(&name), &encoded)
                .map_err(|e| AppError::Internal(format!("写出图集失败: {e}")))?;
            page_files.push(name);

            handle.progress_msg(
                "compose",
                page + 1,
                layout.pages,
                Some(format!(
                    "合成图集 {}/{}（{}×{}）",
                    page + 1,
                    layout.pages,
                    sheet_w,
                    sheet_h
                )),
            );
        }
        (Some(layout), page_files)
    } else {
        (None, Vec::new())
    };
    // ── ③.5 gif：复用 RGBA 帧缓冲生成 GIF（失败则整个任务报错中止）──
    let gif_meta = if params.generate_gif {
        let (gif_fps, gif_indices) = gif_frame_plan(actual_fps, params.gif_fps, frame_count);
        let gif_name = format!("{stem}.gif");
        encode_gif(
            handle,
            staging,
            &gif_name,
            &all,
            &gif_indices,
            frame_bytes,
            frame_w,
            frame_h,
            gif_fps,
            params.gif_dither,
        )?;
        Some(GifMeta {
            name: gif_name,
            fps: gif_fps,
            frames: gif_indices.len() as u64,
            dither: params.gif_dither,
        })
    } else {
        None
    };
    drop(all); // 释放约 1 GB 帧缓冲

    // ── ④ 产物：data.js / player.html / 说明.txt ────────────
    let source_name = input
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".into());
    let (format_name, quality_opt) = match params.format {
        SpriteFormat::Jpeg => ("jpeg", Some(params.quality)),
        SpriteFormat::Png => ("png", None),
    };
    let zip_path = output_dir.join(format!("{stem}_sprite.zip"));
    let created_at = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    // data.js / player.html 是精灵图播放器的配套产物，仅精灵图开启时写出。
    if let Some(layout) = &layout {
        let rows_per_page = layout.rows_per_page;
        let mut pages_meta = Vec::with_capacity(page_files.len());
        for (i, name) in page_files.iter().enumerate() {
            let first = (i as u64) * rows_per_page * layout.cols;
            let count = (frame_count - first).min(rows_per_page * layout.cols);
            let rows = count.div_ceil(layout.cols);
            pages_meta.push(serde_json::json!({
                "file": name,
                "firstFrame": first,
                "frameCount": count,
                "rows": rows,
            }));
        }
        let mut meta = serde_json::Map::new();
        meta.insert("version".into(), serde_json::json!(1));
        meta.insert("source".into(), serde_json::json!(source_name));
        meta.insert("createdAt".into(), serde_json::json!(created_at));
        meta.insert("requestedFps".into(), serde_json::json!(params.fps));
        meta.insert("actualFps".into(), serde_json::json!(actual_fps));
        meta.insert("downsampled".into(), serde_json::json!(downsampled));
        meta.insert("frameCount".into(), serde_json::json!(frame_count));
        meta.insert("frameWidth".into(), serde_json::json!(frame_w));
        meta.insert("frameHeight".into(), serde_json::json!(frame_h));
        meta.insert("cols".into(), serde_json::json!(layout.cols));
        meta.insert("format".into(), serde_json::json!(format_name));
        meta.insert("quality".into(), serde_json::json!(quality_opt));
        meta.insert("pages".into(), serde_json::json!(pages_meta));
        if let Some(g) = &gif_meta {
            meta.insert("gif".into(), serde_json::json!(g.name));
            meta.insert(
                "gifFps".into(),
                serde_json::json!((g.fps * 100.0).round() / 100.0),
            );
            meta.insert("gifFrames".into(), serde_json::json!(g.frames));
            meta.insert("gifDither".into(), serde_json::json!(g.dither));
        }
        let data_js = format!("window.SPRITE_META = {};", serde_json::Value::Object(meta));
        std::fs::write(staging.join("data.js"), data_js)
            .map_err(|e| AppError::Internal(format!("写出 data.js 失败: {e}")))?;

        std::fs::write(
            staging.join("player.html"),
            include_str!("../../assets/player.html"),
        )
        .map_err(|e| AppError::Internal(format!("写出 player.html 失败: {e}")))?;
    }

    let result = SpriteResult {
        zip_path,
        staging_dir: staging.to_path_buf(),
        frame_count,
        requested_fps: params.fps,
        actual_fps,
        downsampled,
        pages: layout.as_ref().map(|l| l.pages).unwrap_or(0),
        cols: layout.as_ref().map(|l| l.cols).unwrap_or(0),
        frame_width: frame_w,
        frame_height: frame_h,
        duration_sec: info.duration_sec,
        gif: gif_meta,
    };

    let readme = build_readme(&result, &source_name, created_at, format_name, quality_opt);
    std::fs::write(staging.join("说明.txt"), readme)
        .map_err(|e| AppError::Internal(format!("写出说明文件失败: {e}")))?;

    // ── ⑤ pack：zip 打包 ────────────────────────────────────
    handle.progress_now("pack", 0, 1);
    std::fs::create_dir_all(output_dir)
        .map_err(|e| AppError::Internal(format!("创建输出目录失败: {e}")))?;
    let zip_file = std::fs::File::create(&result.zip_path)
        .map_err(|e| AppError::Internal(format!("创建 zip 失败: {e}")))?;
    let mut zw = zip::ZipWriter::new(zip_file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut add_entry = |name: &str, path: &Path| -> AppResult<()> {
        let data = std::fs::read(path)
            .map_err(|e| AppError::Internal(format!("读取 {name} 失败: {e}")))?;
        zw.start_file(name, options.clone())
            .map_err(|e| AppError::Internal(format!("写入 zip 条目 {name} 失败: {e}")))?;
        zw.write_all(&data)
            .map_err(|e| AppError::Internal(format!("写入 zip 条目 {name} 失败: {e}")))?;
        Ok(())
    };
    // 精灵图配套产物仅在精灵图模式下存在。
    if result.pages > 0 {
        for name in &page_files {
            add_entry(name, &staging.join(name))?;
        }
        add_entry("data.js", &staging.join("data.js"))?;
        add_entry("player.html", &staging.join("player.html"))?;
    }
    if let Some(g) = &result.gif {
        add_entry(&g.name, &staging.join(&g.name))?;
    }
    add_entry("说明.txt", &staging.join("说明.txt"))?;
    zw.finish()
        .map_err(|e| AppError::Internal(format!("完成 zip 打包失败: {e}")))?;

    Ok(result)
}

/// 把一页帧数据拷入图集缓冲并编码。`frames` 为本页帧的连续字节切片。
fn encode_page(
    frames: &[u8],
    frame_count: usize,
    cols: u32,
    frame_w: u32,
    frame_h: u32,
    sheet_w: u32,
    sheet_h: u32,
    format: SpriteFormat,
    quality: u8,
) -> AppResult<Vec<u8>> {
    let fw = frame_w as usize;
    let fh = frame_h as usize;
    let row_bytes = fw * 4;
    let sheet_w = sheet_w as usize;
    let mut buf = vec![0u8; sheet_w * sheet_h as usize * 4];
    for (i, frame) in frames
        .chunks_exact(row_bytes * fh)
        .take(frame_count)
        .enumerate()
    {
        let row = (i as u32 / cols) as usize;
        let col = (i as u32 % cols) as usize;
        for fy in 0..fh {
            let src = &frame[fy * row_bytes..(fy + 1) * row_bytes];
            let dst = ((row * fh + fy) * sheet_w + col * fw) * 4;
            buf[dst..dst + row_bytes].copy_from_slice(src);
        }
    }
    let img = RgbaImage::from_raw(sheet_w as u32, sheet_h, buf).expect("图集缓冲尺寸匹配");

    let mut out = Vec::new();
    match format {
        SpriteFormat::Png => img
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .map_err(|e| AppError::Internal(format!("PNG 编码失败: {e}")))?,
        SpriteFormat::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
            encoder
                .encode(
                    &rgb,
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| AppError::Internal(format!("JPEG 编码失败: {e}")))?;
        }
    }
    Ok(out)
}

// ── GIF 生成（v0.4）：全局调色板 + 5bit LUT + FS 抖动 + 滚动延迟 ──

/// GIF 帧延迟表：GIF 延迟单位 1/100 秒，无法精确表示任意 fps；
/// 逐帧累计取整（滚动取整），使整段 GIF 的平均帧率与目标一致。
/// 单帧延迟下限钳 2cs（浏览器对 <2cs 的处理不一致）。
fn gif_delay_table(fps: f64, frames: u64) -> Vec<u16> {
    let step = 100.0 / fps.max(0.01);
    let mut out = Vec::with_capacity(frames as usize);
    let mut acc = 0.0f64;
    let mut prev = 0i64;
    for _ in 0..frames {
        acc += step;
        let r = acc.round() as i64;
        out.push((r - prev).max(2) as u16);
        prev = r;
    }
    out
}

/// 跳帧规划：GIF 帧率上限低于采样帧率时等距抽帧。
/// 返回 (GIF 有效帧率, 源帧索引序列)。
fn gif_frame_plan(sampled_fps: f64, gif_fps_cap: Option<u32>, total: u64) -> (f64, Vec<u64>) {
    let all = (0..total).collect::<Vec<_>>();
    match gif_fps_cap {
        None | Some(0) => (sampled_fps, all),
        Some(cap) => {
            let cap = cap as f64;
            if cap >= sampled_fps {
                return (sampled_fps, all);
            }
            let step = (sampled_fps / cap).max(1.0);
            let n = ((total as f64) / step).ceil().max(1.0) as u64;
            let idx = (0..n)
                .map(|gi| ((gi as f64 * step) as u64).min(total - 1))
                .collect();
            (sampled_fps / step, idx)
        }
    }
}

/// 5bit RGB LUT 键：r/g/b 各取高 5 位拼成 15 位索引。
#[inline]
fn lut_key(r: u8, g: u8, b: u8) -> usize {
    ((r as usize >> 3) << 10) | ((g as usize >> 3) << 5) | (b as usize >> 3)
}

/// 以调色板颜色在 5bit 色桶中心的最近邻构建 32768 项查询表（O(1) 索引化）。
fn build_lut(palette: &[[u8; 3]]) -> Vec<u8> {
    let mut lut = vec![0u8; 32768];
    for (key, slot) in lut.iter_mut().enumerate() {
        let r = (((key >> 10) & 31) * 8 + 4) as i32;
        let g = (((key >> 5) & 31) * 8 + 4) as i32;
        let b = ((key & 31) * 8 + 4) as i32;
        let (mut best, mut bd) = (0u8, i32::MAX);
        for (i, p) in palette.iter().enumerate() {
            let dr = r - p[0] as i32;
            let dg = g - p[1] as i32;
            let db = b - p[2] as i32;
            let d = dr * dr + dg * dg + db * db;
            if d < bd {
                bd = d;
                best = i as u8;
            }
        }
        *slot = best;
    }
    lut
}

/// 全帧均匀抽样（≤64 帧 × ≤1024 像素/帧，≤65536 样本）建 NeuQuant 全局调色板。
/// 全部帧共用一块调色板，避免逐帧调色板闪烁；返回 RGB 平面（每色 3 字节）。
fn build_palette(
    all: &[u8],
    indices: &[u64],
    frame_bytes: usize,
    frame_w: u32,
    frame_h: u32,
) -> Vec<[u8; 3]> {
    let pixels_per_frame = frame_w as usize * frame_h as usize;
    let frame_sel = indices.len().min(64);
    let px_stride = pixels_per_frame.div_ceil(1024).max(1);
    let mut samples: Vec<u8> = Vec::with_capacity(frame_sel * 1024 * 4);
    for j in 0..frame_sel {
        let src_i = indices[j * indices.len() / frame_sel] as usize;
        let base = src_i * frame_bytes;
        for pi in (0..pixels_per_frame).step_by(px_stride) {
            let p = &all[base + pi * 4..base + pi * 4 + 4];
            samples.extend_from_slice(p);
        }
    }
    // 网络规模取样本数的向上幂（上限 256），保证 GIF 调色板为 2 的幂。
    let net = (samples.len() / 4)
        .clamp(2, 256)
        .next_power_of_two()
        .min(256);
    let nq = color_quant::NeuQuant::new(10, net, &samples);
    nq.color_map_rgb()
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect()
}

/// Floyd-Steinberg 误差扩散抖动 + LUT 索引化。
/// 误差缓冲两行滚动（免整帧缓冲），系数：右 7/16、下左 3/16、下 5/16、下右 1/16。
fn dither_frame(frame: &[u8], w: usize, h: usize, lut: &[u8], palette: &[[u8; 3]]) -> Vec<u8> {
    let ew = w + 2;
    let mut err = vec![0i32; 2 * ew * 3];
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let cur = (y & 1) * ew * 3;
        let nxt = ((y + 1) & 1) * ew * 3;
        for x in 0..w {
            let p = (y * w + x) * 4;
            let ex = x + 1;
            let e = &err[cur + ex * 3..cur + ex * 3 + 3];
            let r = (frame[p] as i32 + e[0]).clamp(0, 255);
            let g = (frame[p + 1] as i32 + e[1]).clamp(0, 255);
            let b = (frame[p + 2] as i32 + e[2]).clamp(0, 255);
            let idx = lut[lut_key(r as u8, g as u8, b as u8)] as usize;
            out[y * w + x] = idx as u8;
            let (er, eg, eb) = (
                r - palette[idx][0] as i32,
                g - palette[idx][1] as i32,
                b - palette[idx][2] as i32,
            );
            // 右 7/16（滚动行自身）
            let t = &mut err[cur + (ex + 1) * 3..][..3];
            t[0] += er * 7 / 16;
            t[1] += eg * 7 / 16;
            t[2] += eb * 7 / 16;
            if y + 1 < h {
                if x > 0 {
                    // 下左 3/16
                    let t = &mut err[nxt + (ex - 1) * 3..][..3];
                    t[0] += er * 3 / 16;
                    t[1] += eg * 3 / 16;
                    t[2] += eb * 3 / 16;
                }
                // 下 5/16
                let t = &mut err[nxt + ex * 3..][..3];
                t[0] += er * 5 / 16;
                t[1] += eg * 5 / 16;
                t[2] += eb * 5 / 16;
                // 下右 1/16
                let t = &mut err[nxt + (ex + 1) * 3..][..3];
                t[0] += er / 16;
                t[1] += eg / 16;
                t[2] += eb / 16;
            }
        }
        err[cur..cur + ew * 3].fill(0);
    }
    out
}

/// 编码 GIF：建板 → LUT →（可选抖动）逐帧索引化 → 流式写出。
/// 与精灵图同尺寸、无限循环、整帧不透明（disposal 无关紧要）。
#[allow(clippy::too_many_arguments)]
fn encode_gif(
    handle: &TaskHandle,
    staging: &Path,
    name: &str,
    all: &[u8],
    indices: &[u64],
    frame_bytes: usize,
    frame_w: u32,
    frame_h: u32,
    gif_fps: f64,
    dither: bool,
) -> AppResult<()> {
    let n = indices.len();
    handle.progress_now("gif", 0, n as u64);

    let palette = build_palette(all, indices, frame_bytes, frame_w, frame_h);
    let lut = build_lut(&palette);
    let mut palette_bytes = Vec::with_capacity(palette.len() * 3);
    for p in &palette {
        palette_bytes.extend_from_slice(p);
    }

    let path = staging.join(name);
    let file = std::fs::File::create(&path)
        .map_err(|e| AppError::Internal(format!("创建 GIF 失败: {e}")))?;
    let mut enc = gif::Encoder::new(file, frame_w as u16, frame_h as u16, &palette_bytes)
        .map_err(|e| AppError::Internal(format!("初始化 GIF 编码器失败: {e}")))?;
    enc.set_repeat(gif::Repeat::Infinite)
        .map_err(|e| AppError::Internal(format!("设置 GIF 循环失败: {e}")))?;
    let delays = gif_delay_table(gif_fps, n as u64);

    let (fw, fh) = (frame_w as usize, frame_h as usize);
    for (gi, &src_i) in indices.iter().enumerate() {
        if handle.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let frame = &all[src_i as usize * frame_bytes..][..frame_bytes];
        let idx_frame: Vec<u8> = if dither {
            dither_frame(frame, fw, fh, &lut, &palette)
        } else {
            frame
                .chunks_exact(4)
                .map(|p| lut[lut_key(p[0], p[1], p[2])])
                .collect()
        };
        let f = gif::Frame {
            delay: delays[gi],
            dispose: gif::DisposalMethod::Any,
            transparent: None,
            needs_user_input: false,
            top: 0,
            left: 0,
            width: frame_w as u16,
            height: frame_h as u16,
            interlaced: false,
            palette: None,
            buffer: Cow::Owned(idx_frame),
        };
        enc.write_frame(&f)
            .map_err(|e| AppError::Internal(format!("写入 GIF 第 {} 帧失败: {e}", gi + 1)))?;
        handle.progress("gif", gi as u64 + 1, n as u64);
    }
    info!(
        "GIF 生成完成: {name}（{} fps, {n} 帧, 抖动 {}）",
        fmt_fps(gif_fps),
        if dither { "开" } else { "关" }
    );
    Ok(())
}

/// 生成说明.txt 内容。精灵图与 GIF 产物段落按实际选择条件输出。
fn build_readme(
    r: &SpriteResult,
    source_name: &str,
    created_at: String,
    format_name: &str,
    quality: Option<u8>,
) -> String {
    let quality_note = quality.map(|q| format!("，质量 {q}")).unwrap_or_default();
    let fps_line = if r.downsampled {
        format!(
            "- 采样帧率：{} fps（帧数超上限，已自动降为 {} fps）\n- 实际帧率：{} fps",
            fmt_fps(r.requested_fps),
            fmt_fps(r.actual_fps),
            fmt_fps(r.actual_fps)
        )
    } else {
        format!("- 采样帧率：{} fps（未降级）", fmt_fps(r.actual_fps))
    };
    let mut s = String::new();
    s.push_str("视频转精灵图产物说明\n====================\n\n");
    s.push_str(&format!(
        "来源视频：{source_name}\n生成时间：{created_at}\n视频时长：{:.2} 秒\n\n",
        r.duration_sec
    ));
    s.push_str("参数记录：\n");
    s.push_str(&fps_line);
    s.push_str(&format!(
        "\n- 帧数：{} 帧（单帧 {}×{}）\n",
        r.frame_count, r.frame_width, r.frame_height
    ));
    if r.pages > 0 {
        s.push_str(&format!(
            "- 图集：{} 张（{} 格式{quality_note}）\n",
            r.pages, format_name
        ));
        s.push_str(&format!("- 网格：每页 {} 列，按行排布\n", r.cols));
    }
    if let Some(g) = &r.gif {
        s.push_str(&format!(
            "- GIF：{}（{} fps，{} 帧，抖动{}）\n",
            g.name,
            fmt_fps(g.fps),
            g.frames,
            if g.dither { "开" } else { "关" }
        ));
    }
    if r.pages > 0 {
        s.push_str(
            "\n使用方法：解压后双击 player.html 即可在浏览器中播放精灵动画。\n\
             - 空格：播放 / 暂停\n\
             - ← / →：逐帧步进\n\
             - 拖动进度条：跳转帧\n\
             - 倍速：0.25× ~ 4×\n\
             - 循环：可开关\n",
        );
        s.push_str("\ndata.js 中记录了全部元数据（window.SPRITE_META），可被外部脚本读取。\n");
    } else {
        s.push_str("\nGIF 为标准动图格式，可直接插入浏览器、聊天工具或网页使用。\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fraction_rates() {
        assert!((parse_fraction("30000/1001").unwrap() - 29.970_03).abs() < 1e-4);
        assert!((parse_fraction("24/1").unwrap() - 24.0).abs() < 1e-9);
        assert!(parse_fraction("0/0").is_none());
        assert!(parse_fraction("abc").is_none());
    }

    #[test]
    fn fps_clamp_and_downsample() {
        // 源帧率低于请求帧率：钳制到源帧率
        let (fps, ds, est) = compute_actual_fps(24.0, 15.0, 10.0, 2000);
        assert_eq!(fps, 15.0);
        assert!(!ds);
        assert_eq!(est, 150);
        // 正常情况
        let (fps, ds, est) = compute_actual_fps(24.0, 30.0, 10.0, 2000);
        assert_eq!(fps, 24.0);
        assert!(!ds);
        assert_eq!(est, 240);
        // 超上限降采样
        let (fps, ds, est) = compute_actual_fps(24.0, 60.0, 100.0, 2000);
        assert!(ds);
        assert!((fps - 20.0).abs() < 1e-9);
        assert_eq!(est, 2000);
    }

    #[test]
    fn frame_size_only_downscale() {
        // 大视频：缩到 480 宽
        assert_eq!(compute_frame_size(1920, 1080, 480), (480, 270));
        // 小视频：不放大
        assert_eq!(compute_frame_size(320, 240, 480), (320, 240));
        // 奇数宽取偶
        assert_eq!(compute_frame_size(641, 480, 641), (640, 480));
    }

    #[test]
    fn layout_matches_spec_example() {
        // 方案 §4 示例：N=200、480×270、上限 4096 → 8 列 25 行 2 页
        let l = compute_layout(200, 480, 270, 4096).unwrap();
        assert_eq!(l.cols, 8);
        assert_eq!(l.rows, 25);
        assert_eq!(l.rows_per_page, 15);
        assert_eq!(l.pages, 2);
    }

    #[test]
    fn layout_single_frame() {
        let l = compute_layout(1, 480, 270, 4096).unwrap();
        assert_eq!((l.cols, l.rows, l.pages), (1, 1, 1));
    }

    #[test]
    fn layout_rejects_oversize_frame() {
        assert!(compute_layout(10, 5000, 270, 4096).is_err());
        assert!(compute_layout(10, 480, 5000, 4096).is_err());
    }

    #[test]
    fn layout_last_row_partial() {
        // N=10, cols=4 → 3 行；rows_per_page 足够 → 1 页
        let l = compute_layout(10, 480, 100, 4096).unwrap();
        assert_eq!(l.cols, 4);
        assert_eq!(l.rows, 3);
        assert_eq!(l.pages, 1);
    }

    #[test]
    fn gif_delay_table_rolls() {
        // 24fps：步长 4.1667cs，滚动取整序列
        let t = gif_delay_table(24.0, 24);
        assert_eq!(&t[..8], &[4, 4, 5, 4, 4, 4, 4, 4]);
        // 24 帧 × 4.1667cs = 100cs = 1s（telescoping，误差不累积）
        let sum: u32 = t.iter().map(|&d| d as u32).sum();
        assert_eq!(sum, 100);
        // 极高帧率：钳制到最小 2cs
        let t = gif_delay_table(240.0, 10);
        assert!(t.iter().all(|&d| d >= 2));
    }

    #[test]
    fn gif_plan_skips_and_clamps() {
        // 上限 12、采样 24：步长 2
        let (fps, idx) = gif_frame_plan(24.0, Some(12), 21);
        assert!((fps - 12.0).abs() < 1e-9);
        assert_eq!(idx, (0..21).step_by(2).collect::<Vec<_>>());
        // 不设上限：全帧
        let (fps, idx) = gif_frame_plan(15.0, None, 4);
        assert!((fps - 15.0).abs() < 1e-9);
        assert_eq!(idx, vec![0, 1, 2, 3]);
        // 上限高于采样帧率：全帧
        let (fps, idx) = gif_frame_plan(15.0, Some(24), 4);
        assert!((fps - 15.0).abs() < 1e-9);
        assert_eq!(idx.len(), 4);
        // 非整数步长 1.6：ceil(10/1.6)=7 帧，末尾不越界
        let (fps, idx) = gif_frame_plan(16.0, Some(10), 10);
        assert!((fps - 10.0).abs() < 1e-9);
        assert_eq!(idx, vec![0, 1, 3, 4, 6, 8, 9]);
    }

    #[test]
    fn lut_key_buckets() {
        assert_eq!(lut_key(0, 0, 0), 0);
        assert_eq!(lut_key(255, 255, 255), 32767);
        assert_eq!(lut_key(8, 0, 0), 1 << 10);
        assert_eq!(lut_key(0, 8, 0), 1 << 5);
        assert_eq!(lut_key(0, 0, 8), 1);
    }

    #[test]
    fn dither_exact_when_on_palette() {
        // 纯黑/纯白且调色板恰好包含这两色：无误差扩散，索引精确
        let palette = vec![[0u8, 0, 0], [255u8, 255, 255]];
        let lut = build_lut(&palette);
        let frame: Vec<u8> = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let idx = dither_frame(&frame, 2, 1, &lut, &palette);
        assert_eq!(idx, vec![0, 1]);
    }

    #[test]
    fn gif_pipeline_end_to_end() {
        // 合成 8 帧 32×24 渐变 RGBA，走 建板 → LUT → 抖动 → 编码 全流程
        let (w, h, n) = (32u32, 24u32, 8u64);
        let frame_bytes = (w * h * 4) as usize;
        let mut all = Vec::with_capacity(frame_bytes * n as usize);
        for i in 0..n {
            for y in 0..h {
                for x in 0..w {
                    all.extend_from_slice(&[
                        (x as f64 / w as f64 * 255.0) as u8,
                        (i as f64 / n as f64 * 255.0) as u8,
                        (y as f64 / h as f64 * 255.0) as u8,
                        255,
                    ]);
                }
            }
        }
        let indices: Vec<u64> = (0..n).collect();
        let palette = build_palette(&all, &indices, frame_bytes, w, h);
        assert_eq!(palette.len(), 256);
        let lut = build_lut(&palette);
        let delays = gif_delay_table(12.0, n);
        assert_eq!(delays.len(), 8);

        let mut out = Vec::new();
        {
            let mut palette_bytes = Vec::with_capacity(palette.len() * 3);
            for p in &palette {
                palette_bytes.extend_from_slice(p);
            }
            let mut enc = gif::Encoder::new(&mut out, w as u16, h as u16, &palette_bytes).unwrap();
            enc.set_repeat(gif::Repeat::Infinite).unwrap();
            for &src_i in &indices {
                let frame = &all[src_i as usize * frame_bytes..][..frame_bytes];
                let idx_frame = dither_frame(frame, w as usize, h as usize, &lut, &palette);
                let f = gif::Frame {
                    delay: delays[src_i as usize],
                    dispose: gif::DisposalMethod::Any,
                    transparent: None,
                    needs_user_input: false,
                    top: 0,
                    left: 0,
                    width: w as u16,
                    height: h as u16,
                    interlaced: false,
                    palette: None,
                    buffer: Cow::Owned(idx_frame),
                };
                enc.write_frame(&f).unwrap();
            }
        }
        // 解码验证：帧数、尺寸正确
        use image::AnimationDecoder;
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(&out)).unwrap();
        let frames = decoder.into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 8);
        assert_eq!(
            (frames[0].buffer().width(), frames[0].buffer().height()),
            (w, h)
        );
    }
}
