use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::error::{AppError, AppResult};

/// 单个文件转换结果。
#[derive(Debug, Serialize)]
pub struct ConvertOutcome {
    pub pdf_path: String,
    /// LibreOffice stderr 中解析出的缺失字体（提示用户，不影响成功）。
    pub missing_fonts: Vec<String>,
    /// 目标重名而改名。
    pub renamed: bool,
}

/// On Windows, resolve the correct soffice executable.
/// LibreOffice ships three variants:
///   - soffice.exe  (GUI app, pops up console windows)
///   - soffice.com  (console app, redirects stdout/stderr properly)
///   - soffice.bin  (actual binary)
/// We prefer soffice.com on Windows to avoid popup dialogs.
/// Bare "soffice" names are resolved via PATH search.
fn resolve_soffice_path(configured: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let p = Path::new(configured);
        let file_name = p.file_name().and_then(|f| f.to_str()).unwrap_or("");

        // Bare "soffice" (no directory): search PATH for soffice.com, then soffice.exe.
        // Command::new("soffice") would otherwise resolve to the GUI soffice.exe,
        // which can flash windows even with CREATE_NO_WINDOW.
        let is_bare = file_name.eq_ignore_ascii_case("soffice")
            && p.parent().is_none_or(|parent| parent.as_os_str().is_empty());
        if is_bare {
            if let Some(found) = find_soffice_on_path() {
                return found;
            }
            return configured.to_string();
        }

        // Configured path points at soffice / soffice.exe: prefer sibling soffice.com.
        if file_name.eq_ignore_ascii_case("soffice")
            || file_name.eq_ignore_ascii_case("soffice.exe")
        {
            let com_path = p.with_file_name("soffice.com");
            if com_path.exists() {
                return com_path.to_string_lossy().to_string();
            }
        }
    }
    configured.to_string()
}

#[cfg(target_os = "windows")]
fn find_soffice_on_path() -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in std::env::split_paths(&path_var) {
        if let Some(found) = ["soffice.com", "soffice.exe"]
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.exists())
        {
            return Some(found.to_string_lossy().to_string());
        }
    }
    None
}

pub struct PptxConverter {
    soffice_path: String,
    semaphore: Arc<Semaphore>,
    conversion_timeout: Duration,
}

impl PptxConverter {
    pub fn new(soffice_path: &str, concurrency: usize, conversion_timeout: Duration) -> Self {
        let concurrency = concurrency.max(1);
        let soffice_path = resolve_soffice_path(soffice_path);
        if soffice_path != soffice_path.trim() {
            debug!("soffice path resolved: {soffice_path}");
        }
        info!(
            "PptxConverter ready, soffice={soffice_path}, concurrency={concurrency}, timeout={}s",
            conversion_timeout.as_secs()
        );
        Self {
            soffice_path,
            semaphore: Arc::new(Semaphore::new(concurrency)),
            conversion_timeout,
        }
    }

    /// 把 `input`（pptx/ppt）转换为 PDF 写到 `out_dir`。
    pub async fn convert_file(
        &self,
        input: &Path,
        out_dir: &Path,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> AppResult<ConvertOutcome> {
        if !input.is_file() {
            return Err(AppError::BadRequest(format!(
                "文件不存在: {}",
                input.display()
            )));
        }

        // 1. 每次转换一个独立临时目录：隔离 UserInstallation profile，结束后自动清理。
        let temp = tempfile::tempdir()
            .map_err(|e| AppError::Internal(format!("创建临时目录失败: {e}")))?;
        let output_dir = temp.path().join("output");
        std::fs::create_dir_all(&output_dir)
            .map_err(|e| AppError::Internal(format!("创建输出目录失败: {e}")))?;

        // 2. 并发许可。
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AppError::Internal(format!("Semaphore 获取失败: {e}")))?;

        // 3. 独立 userdir 避免 profile 锁竞争；预写 profile 抑制打印机弹窗。
        let user_installation = user_installation_uri(temp.path());
        seed_printer_profile(temp.path());

        // 4. 调 soffice，转换结果先落临时 output 目录。
        let cmd = self.build_convert_command(&user_installation, &output_dir, input);
        let output = self.run_soffice(cmd, is_cancelled).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("soffice failed, stderr: {stderr}");
            return Err(AppError::ConversionFailed(format!(
                "soffice 退出码 {:?}: {}",
                output.status.code(),
                stderr.chars().take(500).collect::<String>()
            )));
        }

        let tmp_pdf = read_output_pdf(&output_dir)?;
        let fonts = parse_missing_fonts(&String::from_utf8_lossy(&output.stderr));

        // 5. 移动到目标位置，重名自动加序号。
        let (dest, renamed) = unique_pdf_dest(out_dir, input)?;
        std::fs::copy(&tmp_pdf, &dest)
            .map_err(|e| AppError::Internal(format!("写出 PDF 失败: {e}")))?;

        info!(
            "PPTX -> PDF done: {} (missing fonts: {})",
            dest.display(),
            fonts.len()
        );
        Ok(ConvertOutcome {
            pdf_path: dest.to_string_lossy().to_string(),
            missing_fonts: fonts,
            renamed,
        })
    }

    /// 探测 soffice 是否可用（`soffice --version`）。
    pub async fn probe(&self) -> bool {
        let mut cmd = Command::new(&self.soffice_path);
        cmd.arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        matches!(
            timeout(PROBE_TIMEOUT, cmd.output()).await,
            Ok(Ok(output)) if output.status.success()
        )
    }

    /// Build the soffice conversion command.
    /// Key flags to avoid Windows dialogs:
    ///   --nologo      : suppress startup logo / splash screen
    ///   --nodefault   : do not attempt to start default app
    ///   --nolockcheck : skip lock file check (we use isolated userdir anyway)
    fn build_convert_command(
        &self,
        user_installation: &str,
        output_dir: &Path,
        input_path: &Path,
    ) -> Command {
        let mut cmd = Command::new(&self.soffice_path);
        cmd.arg("--headless")
            .arg("--nologo")
            .arg("--norestore")
            .arg("--nodefault")
            .arg("--nolockcheck")
            .arg(format!("-env:UserInstallation={user_installation}"))
            .arg("--convert-to")
            .arg("pdf")
            .arg("--outdir")
            .arg(output_dir)
            .arg(input_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    /// Spawn soffice and wait for it, enforcing conversion timeout and cancel.
    /// Kills the child on timeout/cancel to avoid orphaned soffice instances.
    async fn run_soffice(
        &self,
        mut cmd: Command,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> AppResult<std::process::Output> {
        let mut child = cmd
            .spawn()
            .map_err(|e| AppError::ConversionFailed(format!("soffice 进程启动失败: {e}")))?;

        // Take stdout/stderr pipes so we can read them after wait() completes.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let cancel_wait = async {
            loop {
                if is_cancelled() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        };

        let status = tokio::select! {
            r = child.wait() => match r {
                Ok(status) => status,
                Err(e) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(AppError::ConversionFailed(format!("soffice 进程等待失败: {e}")));
                }
            },
            _ = cancel_wait => {
                let _ = child.start_kill();
                let _ = child.wait().await; // reap the process
                return Err(AppError::Cancelled);
            }
            _ = tokio::time::sleep(self.conversion_timeout) => {
                warn!(
                    "soffice timed out after {}s, killing process",
                    self.conversion_timeout.as_secs()
                );
                let _ = child.start_kill();
                let _ = child.wait().await; // reap the process
                return Err(AppError::ConversionFailed(format!(
                    "soffice 转换超时 ({}s)",
                    self.conversion_timeout.as_secs()
                )));
            }
        };

        // Read stdout/stderr from the pipes.
        let stdout_bytes = read_pipe_to_end(stdout_pipe).await;
        let stderr_bytes = read_pipe_to_end(stderr_pipe).await;

        Ok(std::process::Output {
            status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
        })
    }
}

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Build the `-env:UserInstallation` URI for a temp dir.
/// On Windows, paths use backslashes which break the file:// URI scheme:
/// convert to forward slashes and use file:/// (three slashes for absolute).
fn user_installation_uri(temp: &Path) -> String {
    let userdir_str = temp
        .join("userdir")
        .to_string_lossy()
        .replace('\\', "/");
    if userdir_str.starts_with('/') {
        format!("file://{userdir_str}")
    } else {
        // Windows absolute path like "C:/Users/..." needs file:///
        format!("file:///{userdir_str}")
    }
}

/// Pre-create the LibreOffice user profile with a configuration that
/// suppresses printer access. When a document carries printer settings
/// (or the default printer is an unreachable network printer), Windows
/// pops up a "waiting for printer connection" dialog which blocks
/// headless conversion forever.
///
/// Verified on LibreOffice (Windows, headless): the profile layout is
/// `<UserInstallation>/user/registrymodifications.xcu`, and only *valid*
/// items survive a run. `LoadPrinter=false` ("do not load printer settings
/// with the document") is the setting that actually prevents the printer
/// connection attempt; DisablePrcPrinterDetection is kept for older
/// LibreOffice builds where it is recognized.
fn seed_printer_profile(temp: &Path) {
    let profile_dir = temp.join("userdir").join("user");
    std::fs::create_dir_all(&profile_dir).ok();
    let registrymodifications_path = profile_dir.join("registrymodifications.xcu");
    let registry_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <item oor:path="/org.openoffice.Office.Common/Save/Document"><prop oor:name="LoadPrinter" oor:op="fuse"><value>false</value></prop></item>
  <item oor:path="/org.openoffice.Office.Common/Misc"><prop oor:name="DisablePrcPrinterDetection" oor:op="fuse"><value>true</value></prop></item>
  <item oor:path="/org.openoffice.Office.Common/Misc"><prop oor:name="UseSystemPrintDialog" oor:op="fuse"><value>false</value></prop></item>
</oor:items>"#;
    std::fs::write(&registrymodifications_path, registry_xml).ok();
}

/// 在 `out_dir` 里为源文件生成不冲突的 `stem.pdf` 目标路径。
/// 返回 (目标路径, 是否改名)。
fn unique_pdf_dest(out_dir: &Path, input: &Path) -> AppResult<(PathBuf, bool)> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AppError::BadRequest("文件名无法解析".to_string()))?;
    let mut dest = out_dir.join(format!("{stem}.pdf"));
    let mut renamed = false;
    let mut n = 1u32;
    while dest.exists() {
        renamed = true;
        n += 1;
        dest = out_dir.join(format!("{stem} ({n}).pdf"));
    }
    Ok((dest, renamed))
}

/// Read the converted PDF: `input.pdf` by name, falling back to the first
/// `.pdf` found in the output directory (name may differ).
fn read_output_pdf(output_dir: &Path) -> AppResult<PathBuf> {
    let fallback_stem_pdf = output_dir.join("input.pdf");
    if fallback_stem_pdf.is_file() {
        return Ok(fallback_stem_pdf);
    }
    if let Ok(entries) = std::fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "pdf") {
                return Ok(entry.path());
            }
        }
    }
    Err(AppError::ConversionFailed(
        "soffice 完成但未找到输出 PDF".to_string(),
    ))
}

/// Drain a child process pipe to the end, returning its bytes (empty if absent).
async fn read_pipe_to_end(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf).await;
    }
    buf
}

/// Parse LibreOffice stderr to extract missing font names.
///
/// LibreOffice outputs messages like:
///   `LibreOffice can not find font: 微软雅黑`
///   `font '微软雅黑' not found, substituting...`
fn parse_missing_fonts(stderr: &str) -> Vec<String> {
    let mut fonts = Vec::new();
    // Simple string matching instead of complex regex to avoid
    // raw-string escaping issues.
    for line in stderr.lines() {
        let lower = line.to_lowercase();
        let name = if lower.contains("can not find font") {
            // Use the LAST colon so a "warning:" prefix does not swallow the name.
            after_last_colon(line).or_else(|| extract_quoted_or_after(line, "font"))
        } else if lower.contains("not found") && lower.contains("font") {
            extract_quoted_or_after(line, "font")
        } else if lower.contains("missing font") {
            after_last_colon(line).or_else(|| extract_quoted_or_after(line, "font"))
        } else if lower.contains("substituting font") {
            extract_quoted_or_after(line, "font")
        } else {
            None
        };

        if let Some(n) = name {
            let n = n.trim_matches(|c: char| c == '\'' || c == '"' || c == ',' || c == '.');
            if !n.is_empty() && !fonts.contains(&n.to_string()) {
                fonts.push(n.to_string());
            }
        }
    }

    debug!("Parsed missing fonts: {:?}", fonts);
    fonts
}

/// Text after the last colon in `line` (trimmed), if any.
fn after_last_colon(line: &str) -> Option<&str> {
    let after = line
        .rfind(':')
        .map(|idx| line[idx + 1..].trim())
        .unwrap_or("");
    if after.is_empty() { None } else { Some(after) }
}

/// Try to extract a font name from a line: first try text between quotes,
/// then try text after the keyword.
fn extract_quoted_or_after<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    // Try single quotes first.
    if let (Some(s), Some(e)) = (line.find('\''), line.rfind('\'')) {
        if s < e {
            return Some(&line[s + 1..e]);
        }
    }
    // Try double quotes.
    if let (Some(s), Some(e)) = (line.find('"'), line.rfind('"')) {
        if s < e {
            return Some(&line[s + 1..e]);
        }
    }
    // Try text after keyword + colon.
    let lower = line.to_lowercase();
    let kw_lower = keyword.to_lowercase();
    if let Some(idx) = lower.find(&kw_lower) {
        let rest = &line[idx + kw_lower.len()..];
        let rest = rest.trim_start_matches(|c: char| c == ' ' || c == ':' || c == '\t');
        if !rest.is_empty() {
            return Some(rest.split_whitespace().next().unwrap_or(""));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_missing_fonts() {
        let stderr = "warning: LibreOffice can not find font: 微软雅黑\nfont 'Calibri Light' not found\n";
        let fonts = parse_missing_fonts(stderr);
        assert!(fonts.contains(&"微软雅黑".to_string()));
        assert!(fonts.contains(&"Calibri Light".to_string()));
    }

    #[test]
    fn test_unique_pdf_dest() {
        let dir = tempfile::tempdir().unwrap();
        let input = Path::new("D:/docs/演示.pptx");

        let (dest1, renamed1) = unique_pdf_dest(dir.path(), input).unwrap();
        assert!(dest1.ends_with("演示.pdf"));
        assert!(!renamed1);

        std::fs::write(&dest1, b"x").unwrap();
        let (dest2, renamed2) = unique_pdf_dest(dir.path(), input).unwrap();
        assert!(dest2.ends_with("演示 (2).pdf"));
        assert!(renamed2);
    }

    #[test]
    fn test_user_installation_uri() {
        let temp = tempfile::tempdir().unwrap();
        let uri = user_installation_uri(temp.path());
        assert!(uri.starts_with("file:///"));
        assert!(!uri.contains('\\'));
        assert!(uri.ends_with("/userdir"));
    }
}
