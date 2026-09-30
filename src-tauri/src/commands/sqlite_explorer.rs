//! SQLite 查询器命令：扫描候选数据库文件、魔数检测、表结构浏览与任意 SQL 执行。
//!
//! 面向本机调试场景：SQL 错误原样透传给前端展示，方便排查。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::types::ValueRef;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::config::SettingsState;
use crate::error::{AppError, AppResult};

/// SQLite 文件魔数（文件头 16 字节）。
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\x00";

/// 表信息（virtual table 等 COUNT 失败时 row_count 记 -1）。
#[derive(Debug, Serialize)]
pub struct SqliteTableInfo {
    pub name: String,
    pub row_count: i64,
}

/// SQL 执行结果。
#[derive(Debug, Serialize)]
pub struct SqliteQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total_rows: i64,
    pub truncated: bool,
    pub rows_affected: Option<i64>,
}

/// 扩展名是否为候选数据库类型。
fn is_candidate(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some("db") | Some("sqlite") | Some("sqlite3") | Some("db3")
    )
}

/// 是否为 SQLite 的附属文件（-wal/-shm/-journal）。
fn is_sidecar(name: &str) -> bool {
    name.ends_with("-wal") || name.ends_with("-shm") || name.ends_with("-journal")
}

/// 扫描候选数据库文件：app_data_dir 深度≤3，另加 settings.docx_db_dir（同样规则）。
/// app_data_dir 获取失败返回空列表不报错；结果去重排序后返回绝对路径列表。
#[tauri::command]
pub async fn sqlite_scan(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> AppResult<Vec<String>> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = app.path().app_data_dir() {
        roots.push(dir);
    }
    let extra = settings.0.read().unwrap().docx_db_dir.trim().to_string();
    if !extra.is_empty() {
        roots.push(PathBuf::from(extra));
    }

    let mut out: Vec<String> = Vec::new();
    for root in roots {
        let walk = walkdir::WalkDir::new(&root)
            .max_depth(3)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok());
        for entry in walk {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if is_sidecar(&name) || !is_candidate(entry.path()) {
                continue;
            }
            if let Some(p) = entry.path().to_str() {
                out.push(p.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// 检测文件是否为 SQLite 数据库（读文件头 16 字节比对魔数）。
#[tauri::command]
pub async fn sqlite_detect(path: String) -> AppResult<bool> {
    if !Path::new(&path).is_file() {
        return Err(AppError::BadRequest("文件不存在".into()));
    }
    let mut buf = [0u8; 16];
    let mut f =
        std::fs::File::open(&path).map_err(|e| AppError::Internal(format!("打开文件失败: {e}")))?;
    let n = f
        .read(&mut buf)
        .map_err(|e| AppError::Internal(format!("读取文件失败: {e}")))?;
    Ok(n == 16 && buf == *SQLITE_MAGIC)
}

/// SQL 标识符加双引号并转义内部双引号。
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// 列出数据库中的用户表及行数。
#[tauri::command]
pub async fn sqlite_tables(path: String) -> AppResult<Vec<SqliteTableInfo>> {
    if !Path::new(&path).is_file() {
        return Err(AppError::BadRequest("数据库文件不存在".into()));
    }
    tokio::task::spawn_blocking(move || -> AppResult<Vec<SqliteTableInfo>> {
        let conn = rusqlite::Connection::open(&path).map_err(|e| AppError::BadRequest(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' \
                 AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
            )
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        let names: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| AppError::BadRequest(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            // virtual table / 权限受限等 COUNT 失败时记 -1，由前端显示 "—"。
            let row_count = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {}", quote_ident(&name)),
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(-1);
            out.push(SqliteTableInfo { name, row_count });
        }
        Ok(out)
    })
    .await
    .map_err(|e| AppError::Internal(format!("查询线程失败: {e}")))?
}

/// 判断 SQL 是否为查询语句：剔除 -- 注释行后取首个单词转小写。
fn is_query_sql(sql: &str) -> bool {
    let joined = sql
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("--"))
        .collect::<Vec<_>>()
        .join(" ");
    let first = joined
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "select" | "with" | "values" | "explain" | "pragma"
    )
}

/// 单元格值 → Option<String>（Blob 只给占位说明，避免输出二进制乱码）。
fn value_to_string(v: ValueRef<'_>) -> Option<String> {
    match v {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(f.to_string()),
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Some(format!("<blob {} bytes>", b.len())),
    }
}

/// 执行任意 SQL：查询语句返回结果表格，其他语句返回影响行数。
/// max_rows 缺省或 ≤0 时取 500，超出则 truncated=true 停止读取。
#[tauri::command]
pub async fn sqlite_query(
    path: String,
    sql: String,
    max_rows: Option<i64>,
) -> AppResult<SqliteQueryResult> {
    if !Path::new(&path).is_file() {
        return Err(AppError::BadRequest("数据库文件不存在".into()));
    }
    let limit = match max_rows {
        Some(n) if n > 0 => n,
        _ => 500,
    };
    tokio::task::spawn_blocking(move || -> AppResult<SqliteQueryResult> {
        let conn =
            rusqlite::Connection::open(&path).map_err(|e| AppError::BadRequest(e.to_string()))?;
        conn.busy_timeout(Duration::from_millis(5000))
            .map_err(|e| AppError::BadRequest(e.to_string()))?;

        if is_query_sql(&sql) {
            let mut stmt =
                conn.prepare(&sql).map_err(|e| AppError::BadRequest(e.to_string()))?;
            let columns: Vec<String> = stmt
                .column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let col_count = columns.len();
            let mut rows_out: Vec<Vec<Option<String>>> = Vec::new();
            let mut total: i64 = 0;
            let mut truncated = false;
            let mut rows = stmt
                .query([])
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            while let Some(row) = rows
                .next()
                .map_err(|e| AppError::BadRequest(e.to_string()))?
            {
                if total >= limit {
                    truncated = true;
                    break;
                }
                let mut out_row = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    let v = row
                        .get_ref(i)
                        .map_err(|e| AppError::BadRequest(e.to_string()))?;
                    out_row.push(value_to_string(v));
                }
                rows_out.push(out_row);
                total += 1;
            }
            Ok(SqliteQueryResult {
                columns,
                rows: rows_out,
                total_rows: total,
                truncated,
                rows_affected: None,
            })
        } else {
            let n = conn
                .execute(&sql, [])
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            Ok(SqliteQueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                total_rows: 0,
                truncated: false,
                rows_affected: Some(n as i64),
            })
        }
    })
    .await
    .map_err(|e| AppError::Internal(format!("执行线程失败: {e}")))?
}
