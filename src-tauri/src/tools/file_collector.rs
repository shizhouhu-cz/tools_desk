use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use crate::error::{AppError, AppResult};

/// 文件收集结果。
#[derive(Debug, Serialize)]
pub struct CollectSummary {
    pub dest_dir: String,
    pub files: u64,
    pub bytes: u64,
    /// 因重名而改名的文件数。
    pub renamed: u64,
}

/// 递归收集 `source`（含所有子目录）下的全部文件，复制到 `source/dest_name`。
///
/// - 跨子目录同名文件自动改为 `name (1).ext`、`name (2).ext` ...
/// - `on_progress(copied, total)` 报告进度；
/// - `is_cancelled()` 返回 true 时删除目标目录并返回 [`AppError::Cancelled`]。
pub fn collect(
    source: &Path,
    dest_name: &str,
    on_progress: &dyn Fn(u64, u64),
    is_cancelled: &dyn Fn() -> bool,
) -> AppResult<CollectSummary> {
    if !source.is_dir() {
        return Err(AppError::BadRequest(format!(
            "目录不存在: {}",
            source.display()
        )));
    }

    // 1. 目标目录（存在则追加序号，避免覆盖历史收集结果）。
    let mut dest = source.join(dest_name);
    let mut n = 1u32;
    while dest.exists() {
        n += 1;
        dest = source.join(format!("{dest_name}_{n}"));
    }
    fs::create_dir_all(&dest).map_err(|e| AppError::Internal(format!("创建目标目录失败: {e}")))?;

    // 2. 枚举全部文件（排除目标目录自身，防止自拷贝）。
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().starts_with(&dest) {
            continue;
        }
        files.push(entry.into_path());
    }
    let total = files.len() as u64;
    on_progress(0, total);

    // 3. 逐个复制。
    let mut used_names: HashSet<String> = HashSet::with_capacity(files.len());
    let (mut copied, mut bytes, mut renamed) = (0u64, 0u64, 0u64);
    for (i, src) in files.iter().enumerate() {
        if is_cancelled() {
            let _ = fs::remove_dir_all(&dest);
            return Err(AppError::Cancelled);
        }
        let file_name = src
            .file_name()
            .and_then(|f| f.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("file_{i}"));
        let target = unique_dest_name(&dest, &file_name, &mut used_names);
        if target.file_name().and_then(|f| f.to_str()) != Some(file_name.as_str()) {
            renamed += 1;
        }
        match fs::copy(src, &target) {
            Ok(b) => {
                copied += 1;
                bytes += b;
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&dest);
                return Err(AppError::Internal(format!(
                    "复制 {} 失败: {e}",
                    src.display()
                )));
            }
        }
        on_progress(copied, total);
    }

    Ok(CollectSummary {
        dest_dir: dest.to_string_lossy().to_string(),
        files: copied,
        bytes,
        renamed,
    })
}

/// 在 `dest` 目录内为 `file_name` 生成不重复的目标名。
fn unique_dest_name(dest: &Path, file_name: &str, used: &mut HashSet<String>) -> PathBuf {
    if used.insert(file_name.to_string()) {
        return dest.join(file_name);
    }
    let (stem, ext) = split_ext(file_name);
    for n in 1..u32::MAX {
        let candidate = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        if used.insert(candidate.clone()) {
            return dest.join(candidate);
        }
    }
    unreachable!("文件名空间耗尽");
}

fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(idx) if idx > 0 => (&name[..idx], &name[idx + 1..]),
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tools_test_{tag}_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn collects_nested_files() {
        let root = temp_root("nested");
        write(&root.join("a.txt"), b"a");
        write(&root.join("sub/b.txt"), b"bb");
        write(&root.join("sub/deep/c.dat"), b"ccc");

        let summary = collect(&root, "out", &|_, _| {}, &|| false).unwrap();

        assert_eq!(summary.files, 3);
        assert_eq!(summary.bytes, 6);
        assert_eq!(summary.renamed, 0);
        let dest = PathBuf::from(&summary.dest_dir);
        assert!(dest.join("a.txt").is_file());
        assert!(dest.join("b.txt").is_file());
        assert!(dest.join("c.dat").is_file());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn renames_cross_dir_duplicates() {
        let root = temp_root("dup");
        write(&root.join("a/x.txt"), b"1");
        write(&root.join("b/x.txt"), b"22");
        write(&root.join("c/x.txt"), b"333");

        let summary = collect(&root, "out", &|_, _| {}, &|| false).unwrap();

        assert_eq!(summary.files, 3);
        assert_eq!(summary.renamed, 2);
        let dest = PathBuf::from(&summary.dest_dir);
        assert_eq!(fs::read(dest.join("x.txt")).unwrap(), b"1");
        assert_eq!(fs::read(dest.join("x (1).txt")).unwrap(), b"22");
        assert_eq!(fs::read(dest.join("x (2).txt")).unwrap(), b"333");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn chinese_file_names() {
        let root = temp_root("cjk");
        write(&root.join("子目录/报告 最终版.pdf"), b"data");

        let summary = collect(&root, "输出", &|_, _| {}, &|| false).unwrap();

        assert_eq!(summary.files, 1);
        let dest = PathBuf::from(&summary.dest_dir);
        assert!(dest.join("报告 最终版.pdf").is_file());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_cleans_up() {
        let root = temp_root("cancel");
        write(&root.join("a.txt"), b"a");
        write(&root.join("b.txt"), b"b");

        let result = collect(&root, "out", &|_, _| {}, &|| true);

        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(!root.join("out").exists(), "取消后应清理目标目录");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn split_ext_cases() {
        assert_eq!(split_ext("a.tar.gz"), ("a.tar", "gz"));
        assert_eq!(split_ext(".gitignore"), (".gitignore", ""));
        assert_eq!(split_ext("noext"), ("noext", ""));
    }
}
