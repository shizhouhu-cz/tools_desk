//! DOCX 结构化拆分命令：批量入库（python 冷启动子进程）+ 章节树/历史文档（rusqlite 直查）。
//!
//! 批量拆分串行执行（LLM 分段长耗时 + SQLite 单写者）；章节树为高频交互，
//! 不值得 python 冷启动 200-500ms，直接 rusqlite 查 chapters 表组树。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tracing::info;

use crate::config::{Settings, SettingsState};
use crate::error::{AppError, AppResult};
use crate::task::TaskManager;
use crate::tools::docx_split;

/// 单文件拆分结果。
#[derive(Debug, Serialize)]
pub struct DocxOutcome {
    pub input: String,
    pub ok: bool,
    pub doc_id: Option<i64>,
    pub chapters: Option<u64>,
    pub tables: Option<u64>,
    pub needs_review: Option<u64>,
    pub total_paragraphs: Option<u64>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// 章节树节点。
#[derive(Debug, Serialize)]
pub struct ChapterNode {
    pub id: i64,
    pub level: i64,
    pub number_prefix: String,
    pub title: String,
    pub needs_review: bool,
    pub content_chars: Option<i64>,
    pub children: Vec<ChapterNode>,
}

/// 历史文档列表项。
#[derive(Debug, Serialize)]
pub struct DocxDocumentInfo {
    pub id: i64,
    pub doc_name: String,
    pub status: String,
    pub total_chars: Option<i64>,
    pub error_msg: Option<String>,
    pub create_time: Option<String>,
}

/// 数据库路径：settings.docx_db_dir > app_data_dir/docx_split/docx_split.db。
fn resolve_db_path(app: &AppHandle, cfg: &Settings) -> AppResult<PathBuf> {
    let base = if cfg.docx_db_dir.trim().is_empty() {
        app.path()
            .app_data_dir()
            .map_err(|e| AppError::Internal(format!("无法定位应用数据目录: {e}")))?
            .join("docx_split")
    } else {
        PathBuf::from(cfg.docx_db_dir.trim())
    };
    std::fs::create_dir_all(&base)
        .map_err(|e| AppError::Internal(format!("创建数据库目录失败: {e}")))?;
    Ok(base.join("docx_split.db"))
}

/// 批量 DOCX 结构化拆分。单文件失败不中断批次。
#[tauri::command]
pub async fn docx_structure(
    app: AppHandle,
    tasks: State<'_, TaskManager>,
    settings: State<'_, SettingsState>,
    inputs: Vec<String>,
    llm_enabled: bool,
) -> AppResult<Vec<DocxOutcome>> {
    if inputs.is_empty() {
        return Err(AppError::BadRequest("未选择输入文件".into()));
    }

    let task = tasks.create(&app, "docx-structure");
    let cfg = settings.0.read().unwrap().clone();
    let python_exe = docx_split::resolve_python_path(&app, &cfg.docx_python_path)?;
    let db_path = resolve_db_path(&app, &cfg)?;
    let envs = docx_split::llm_env(&cfg, llm_enabled);

    info!(
        "docx_structure: {} 个文件, llm={llm_enabled}, python={}",
        inputs.len(),
        python_exe.display()
    );

    let total = inputs.len() as u64;
    let mut outcomes = Vec::with_capacity(inputs.len());
    for (i, input) in inputs.iter().enumerate() {
        task.progress_now("extract", i as u64, total);
        let started = Instant::now();
        if task.is_cancelled() {
            outcomes.push(DocxOutcome {
                input: input.clone(),
                ok: false,
                doc_id: None,
                chapters: None,
                tables: None,
                needs_review: None,
                total_paragraphs: None,
                error: Some("任务已取消".into()),
                duration_ms: 0,
            });
            continue;
        }

        let handle = task.clone();
        let py = python_exe.clone();
        let db = db_path.clone();
        let envs2 = envs.clone();
        let input_owned = input.clone();
        let result = tokio::task::spawn_blocking(move || {
            docx_split::run_extract(&handle, &py, &db, &input_owned, &envs2)
        })
        .await
        .unwrap_or_else(|e| Err(format!("任务线程失败: {e}")));

        let duration_ms = started.elapsed().as_millis() as u64;
        outcomes.push(match result {
            Ok(ok) => {
                let s = ok.stats;
                DocxOutcome {
                    input: input.clone(),
                    ok: true,
                    doc_id: Some(if ok.doc_id != 0 { ok.doc_id } else { s.doc_id }),
                    chapters: Some(s.chapters),
                    tables: Some(s.tables),
                    needs_review: Some(s.needs_review),
                    total_paragraphs: Some(s.total_paragraphs),
                    error: None,
                    duration_ms,
                }
            }
            Err(e) => DocxOutcome {
                input: input.clone(),
                ok: false,
                doc_id: None,
                chapters: None,
                tables: None,
                needs_review: None,
                total_paragraphs: None,
                error: Some(e),
                duration_ms,
            },
        });
    }
    task.progress_now("done", outcomes.len() as u64, total);
    tasks.finish(&task.id);
    Ok(outcomes)
}

/// rusqlite 错误 → 友好错误。
fn db_err(e: rusqlite::Error) -> AppError {
    let msg = e.to_string();
    if msg.contains("no such table") {
        AppError::BadRequest("暂无数据，请先拆分文档".into())
    } else {
        AppError::Internal(format!("数据库查询失败: {msg}"))
    }
}

#[derive(Debug)]
struct RawChapter {
    id: i64,
    parent_id: i64,
    level: i64,
    number_prefix: String,
    title: String,
    needs_review: bool,
    content_chars: Option<i64>,
}

/// 按 parent_id 组树（孤儿节点挂到根），保持 order_num 顺序。
fn build_tree(list: Vec<RawChapter>) -> Vec<ChapterNode> {
    let index: HashMap<i64, usize> = list.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut children_idx: HashMap<i64, Vec<usize>> = HashMap::new();
    let mut root_idx: Vec<usize> = Vec::new();
    for (i, c) in list.iter().enumerate() {
        let parent = if c.parent_id == 0 || !index.contains_key(&c.parent_id) {
            None
        } else {
            Some(c.parent_id)
        };
        match parent {
            Some(p) => children_idx.entry(p).or_default().push(i),
            None => root_idx.push(i),
        }
    }
    fn assemble(
        list: &[RawChapter],
        idx: usize,
        children_idx: &HashMap<i64, Vec<usize>>,
    ) -> ChapterNode {
        let c = &list[idx];
        let children = children_idx
            .get(&c.id)
            .map(|v| v.iter().map(|&i| assemble(list, i, children_idx)).collect())
            .unwrap_or_default();
        ChapterNode {
            id: c.id,
            level: c.level,
            number_prefix: c.number_prefix.clone(),
            title: c.title.clone(),
            needs_review: c.needs_review,
            content_chars: c.content_chars,
            children,
        }
    }
    root_idx
        .iter()
        .map(|&i| assemble(&list, i, &children_idx))
        .collect()
}

/// 查询文档章节树（rusqlite 直查，无需冷启动 python）。
#[tauri::command]
pub async fn docx_tree(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    doc_id: i64,
) -> AppResult<Vec<ChapterNode>> {
    let db_path = {
        let cfg = settings.0.read().unwrap().clone();
        resolve_db_path(&app, &cfg)?
    };
    if !db_path.is_file() {
        return Err(AppError::BadRequest("暂无章节数据，请先拆分文档".into()));
    }
    tokio::task::spawn_blocking(move || -> AppResult<Vec<ChapterNode>> {
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| AppError::Internal(format!("打开数据库失败: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, parent_id, level, COALESCE(number_prefix,''), title, needs_review, \
                 content_chars FROM chapters WHERE doc_id=?1 ORDER BY order_num",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([doc_id], |r| {
                Ok(RawChapter {
                    id: r.get(0)?,
                    parent_id: r.get(1)?,
                    level: r.get(2)?,
                    number_prefix: r.get(3)?,
                    title: r.get(4)?,
                    needs_review: r.get::<_, i64>(5)? != 0,
                    content_chars: r.get(6)?,
                })
            })
            .map_err(db_err)?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(db_err)?);
        }
        Ok(build_tree(list))
    })
    .await
    .map_err(|e| AppError::Internal(format!("查询线程失败: {e}")))?
}

/// 查询单个章节正文内容（content 列可能为 NULL → None）。
#[tauri::command]
pub async fn docx_chapter_content(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    chapter_id: i64,
) -> AppResult<Option<String>> {
    let db_path = {
        let cfg = settings.0.read().unwrap().clone();
        resolve_db_path(&app, &cfg)?
    };
    if !db_path.is_file() {
        return Err(AppError::BadRequest("暂无数据，请先拆分文档".into()));
    }
    tokio::task::spawn_blocking(move || -> AppResult<Option<String>> {
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| AppError::Internal(format!("打开数据库失败: {e}")))?;
        match conn.query_row(
            "SELECT content FROM chapters WHERE id=?1",
            [chapter_id],
            |r| r.get::<_, Option<String>>(0),
        ) {
            Ok(content) => Ok(content),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(db_err(e)),
        }
    })
    .await
    .map_err(|e| AppError::Internal(format!("查询线程失败: {e}")))?
}

/// 历史文档列表（documents 表，倒序）。数据库尚不存在时返回空列表。
#[tauri::command]
pub async fn docx_documents(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> AppResult<Vec<DocxDocumentInfo>> {
    let db_path = {
        let cfg = settings.0.read().unwrap().clone();
        resolve_db_path(&app, &cfg)?
    };
    if !db_path.is_file() {
        return Ok(Vec::new());
    }
    tokio::task::spawn_blocking(move || -> AppResult<Vec<DocxDocumentInfo>> {
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| AppError::Internal(format!("打开数据库失败: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, doc_name, status, total_chars, error_msg, create_time \
                 FROM documents ORDER BY id DESC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(DocxDocumentInfo {
                    id: r.get(0)?,
                    doc_name: r.get(1)?,
                    status: r.get(2)?,
                    total_chars: r.get(3)?,
                    error_msg: r.get(4)?,
                    create_time: r.get(5)?,
                })
            })
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(db_err)?);
        }
        Ok(out)
    })
    .await
    .map_err(|e| AppError::Internal(format!("查询线程失败: {e}")))?
}
