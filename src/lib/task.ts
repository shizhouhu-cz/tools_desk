import { useCallback, useEffect, useRef, useState } from "react";
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { TaskProgress } from "./types";

export interface TaskState {
  taskId: string | null;
  running: boolean;
  stage: string;
  current: number;
  total: number;
  message: string | null;
  /** 任务开始至今的毫秒数（事件携带）。 */
  elapsedMs: number;
  /** 最近一次进度事件的到达时间，用于本地平滑递增耗时。 */
  updatedAt: number;
}

const INITIAL: TaskState = {
  taskId: null,
  running: false,
  stage: "",
  current: 0,
  total: 0,
  message: null,
  elapsedMs: 0,
  updatedAt: 0,
};

/**
 * 订阅 task:progress 事件。传 `tool` 时只接收该工具的进度。
 * invoke promise 结束后请调用 markDone() 复位 running。
 */
export function useTask(tool?: string) {
  const [state, setState] = useState<TaskState>(INITIAL);
  const taskIdRef = useRef<string | null>(null);
  const toolRef = useRef(tool);
  toolRef.current = tool;

  useEffect(() => {
    const unlisten = listen<TaskProgress>("task:progress", (e) => {
      const p = e.payload;
      if (toolRef.current && p.tool !== toolRef.current) return;
      taskIdRef.current = p.task_id;
      setState({
        taskId: p.task_id,
        // done 为收尾阶段：直接结束，避免晚于 invoke 返回到达的事件把面板卡在“完成”。
        running: p.stage !== "done",
        stage: p.stage,
        current: p.current,
        total: p.total,
        message: p.message,
        elapsedMs: p.elapsed_ms,
        updatedAt: Date.now(),
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const markDone = useCallback(() => {
    setState((s) => ({ ...s, running: false }));
  }, []);

  // Tauri 2 会把 Rust 命令的 snake_case 参数映射为 JS 侧的 camelCase key，
  // 因此这里必须传 taskId（而非 task_id），否则报 missing required key taskId。
  const cancel = useCallback(async () => {
    if (taskIdRef.current) {
      await tauriInvoke("cancel_task", { taskId: taskIdRef.current });
    }
  }, []);

  return { ...state, markDone, cancel };
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/** 毫秒时长格式化：0.8s / 12.3s / 1m02s。 */
export function formatDuration(ms: number | null | undefined): string {
  if (!ms || ms <= 0) return "";
  if (ms < 1000) return `${ms} ms`;
  const totalSec = ms / 1000;
  if (totalSec < 60) return `${totalSec.toFixed(1)} s`;
  const m = Math.floor(totalSec / 60);
  const s = Math.round(totalSec % 60);
  return `${m}m${String(s).padStart(2, "0")}s`;
}

export const STAGE_LABELS: Record<string, string> = {
  scan: "扫描文件",
  copy: "复制文件",
  convert: "转换中",
  prepare: "准备中",
  render: "切图中",
  apply: "处理中",
  extract: "拆分中",
  parse: "解析文档",
  llm: "AI 结构恢复",
  store: "入库",
  done: "完成",
};
