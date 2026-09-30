use std::path::Path;

use crate::error::{AppError, AppResult};

fn ensure_exists(path: &str) -> AppResult<()> {
    if !Path::new(path).exists() {
        return Err(AppError::BadRequest(format!(
            "路径不存在或已被移动: {path}"
        )));
    }
    Ok(())
}

/// 用资源管理器打开目录（绕开 opener 插件的路径 ACL，任意盘符/网络路径均可）。
#[tauri::command]
pub fn open_dir_in_explorer(path: String) -> AppResult<()> {
    ensure_exists(&path)?;
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| AppError::Internal(format!("启动资源管理器失败: {e}")))?;
    Ok(())
}

/// 在资源管理器中定位并选中文件。
#[tauri::command]
pub fn reveal_in_explorer(path: String) -> AppResult<()> {
    ensure_exists(&path)?;
    std::process::Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn()
        .map_err(|e| AppError::Internal(format!("启动资源管理器失败: {e}")))?;
    Ok(())
}
