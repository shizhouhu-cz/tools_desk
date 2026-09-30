use std::collections::HashMap;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

/// 进度事件名。payload 见 [`TaskProgress`]。
pub const PROGRESS_EVENT: &str = "task:progress";

#[derive(Clone, Serialize)]
pub struct TaskProgress {
    pub task_id: String,
    pub tool: String,
    pub stage: String,
    pub current: u64,
    pub total: u64,
    pub message: Option<String>,
    /// 任务开始至今的毫秒数。
    pub elapsed_ms: u64,
}

/// 一次工具运行的句柄：负责取消标志、进度事件（带节流）、子进程注册。
#[derive(Clone)]
pub struct TaskHandle {
    pub id: String,
    pub tool: String,
    app: AppHandle,
    cancel: Arc<AtomicBool>,
    procs: Arc<Mutex<Vec<Child>>>,
    last_emit: Arc<Mutex<Option<Instant>>>,
    started: Instant,
}

const THROTTLE: Duration = Duration::from_millis(100);

impl TaskHandle {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 注册子进程：cancel 时统一 kill（Child 所有权移交注册表；
    /// worker 末尾用 [`TaskHandle::take_children`] 取回 wait() 收尸）。
    pub fn register_child(&self, child: Child) {
        self.procs.lock().unwrap().push(child);
    }

    /// 取走全部已注册子进程（worker 末尾收尸用）。
    pub fn take_children(&self) -> Vec<Child> {
        std::mem::take(&mut *self.procs.lock().unwrap())
    }

    fn emit(&self, stage: &str, current: u64, total: u64, message: Option<String>) {
        let payload = TaskProgress {
            task_id: self.id.clone(),
            tool: self.tool.clone(),
            stage: stage.to_string(),
            current,
            total,
            message,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
        };
        if let Err(e) = self.app.emit(PROGRESS_EVENT, payload) {
            tracing::warn!("发送进度事件失败: {e}");
        }
    }

    /// 无条件发送进度（阶段切换/完成时用）。
    pub fn progress_now(&self, stage: &str, current: u64, total: u64) {
        self.progress_msg(stage, current, total, None);
    }

    /// 带附加消息的进度（如“xxx.pdf：3/12 页”）。
    pub fn progress_msg(&self, stage: &str, current: u64, total: u64, message: Option<String>) {
        self.emit(stage, current, total, message);
        *self.last_emit.lock().unwrap() = Some(Instant::now());
    }

    /// 节流版进度：每 64 个条目或每 100ms 发一次，避免事件风暴。
    pub fn progress(&self, stage: &str, current: u64, total: u64) {
        if current % 64 == 0 || current == total {
            self.emit(stage, current, total, None);
            *self.last_emit.lock().unwrap() = Some(Instant::now());
            return;
        }
        let mut last = self.last_emit.lock().unwrap();
        if last.is_none_or(|t| t.elapsed() >= THROTTLE) {
            self.emit(stage, current, total, None);
            *last = Some(Instant::now());
        }
    }
}

/// 全局任务表：taskId -> 取消标志 + 子进程注册表。命令创建任务，前端可随时请求取消。
#[derive(Default)]
pub struct TaskManager {
    tasks: Mutex<HashMap<String, TaskEntry>>,
}

struct TaskEntry {
    cancel: Arc<AtomicBool>,
    procs: Arc<Mutex<Vec<Child>>>,
}

impl TaskManager {
    pub fn create(&self, app: &AppHandle, tool: &str) -> TaskHandle {
        let id = uuid::Uuid::new_v4().to_string();
        let entry = TaskEntry {
            cancel: Arc::new(AtomicBool::new(false)),
            procs: Arc::new(Mutex::new(Vec::new())),
        };
        let handle = TaskHandle {
            id: id.clone(),
            tool: tool.to_string(),
            app: app.clone(),
            cancel: entry.cancel.clone(),
            procs: entry.procs.clone(),
            last_emit: Arc::new(Mutex::new(None)),
            started: Instant::now(),
        };
        self.tasks.lock().unwrap().insert(id, entry);
        handle
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.tasks.lock().unwrap().get(id).is_some_and(|e| {
            e.cancel.store(true, Ordering::Relaxed);
            for p in e.procs.lock().unwrap().iter_mut() {
                let _ = p.kill();
            }
            true
        })
    }

    pub fn finish(&self, id: &str) {
        self.tasks.lock().unwrap().remove(id);
    }
}

#[tauri::command]
pub fn cancel_task(tasks: State<'_, TaskManager>, task_id: String) -> bool {
    tasks.cancel(&task_id)
}
