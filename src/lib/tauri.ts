import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import type {
  ChapterNode,
  CompareResult,
  DocxDocumentInfo,
  DocxOutcome,
  InvokeError,
  SqliteQueryResult,
  SqliteTableInfo,
  SpriteOutcome,
} from "./types";

/** 统一 invoke 封装：错误序列化对象 -> InvokeError，非取消错误自动 toast。 */
export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args);
  } catch (e) {
    const err = normalizeError(e);
    if (err.code !== "cancelled") {
      toast.error(err.message, { description: `错误类型: ${err.code}` });
    }
    throw err;
  }
}

export function normalizeError(e: unknown): InvokeError {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    return e as InvokeError;
  }
  return { code: "unknown", message: String(e) };
}

/** 选择单个目录。 */
export async function pickDirectory(title: string): Promise<string | null> {
  const sel = await open({ directory: true, multiple: false, title });
  return typeof sel === "string" ? sel : null;
}

/** 多选文件。 */
export async function pickFiles(title: string, extensions: string[]): Promise<string[]> {
  const sel = await open({
    multiple: true,
    title,
    filters: [{ name: `${extensions.join(" / ").toUpperCase()} 文件`, extensions }],
  });
  if (Array.isArray(sel)) return sel;
  return sel ? [sel] : [];
}

/** 单选文件。 */
export async function pickFile(title: string, extensions: string[]): Promise<string | null> {
  const sel = await open({
    multiple: false,
    title,
    filters: [{ name: `${extensions.join(" / ").toUpperCase()} 文件`, extensions }],
  });
  return typeof sel === "string" ? sel : null;
}

/** 另存为对话框（单文件输出）。defaultPath 可传完整路径以预填目录和文件名。 */
export async function pickSaveFile(
  title: string,
  defaultPath: string,
  extensions: string[]
): Promise<string | null> {
  const sel = await save({
    title,
    defaultPath,
    filters: [{ name: `${extensions.join(" / ").toUpperCase()} 文件`, extensions }],
  });
  return typeof sel === "string" ? sel : null;
}

/**
 * 用资源管理器打开目录。
 * 走自定义 Rust 命令而非 opener 插件：opener 的路径 ACL 会拒绝
 * 含特殊字符/嵌套较深的路径（如收集器输出的 _collected_ 目录），自定义命令无此限制。
 */
export async function openDir(path: string): Promise<void> {
  try {
    await invoke("open_dir_in_explorer", { path });
  } catch {
    // 错误 toast 已由 invoke 封装显示，这里吞掉避免 uncaught rejection。
  }
}

/** 在资源管理器中定位并选中文件。 */
export async function revealFile(path: string): Promise<void> {
  try {
    await invoke("reveal_in_explorer", { path });
  } catch {
    // 同上。
  }
}

/** 批量 DOCX 结构化拆分（串行执行，进度走 task:progress 事件）。 */
export async function docxStructure(
  inputs: string[],
  llmEnabled: boolean
): Promise<DocxOutcome[]> {
  return invoke<DocxOutcome[]>("docx_structure", { inputs, llmEnabled });
}

/** 查询已入库文档的章节树。 */
export async function docxTree(docId: number): Promise<ChapterNode[]> {
  return invoke<ChapterNode[]>("docx_tree", { docId });
}

/** 历史文档列表（documents 表，倒序）。 */
export async function docxDocuments(): Promise<DocxDocumentInfo[]> {
  return invoke<DocxDocumentInfo[]>("docx_documents");
}

/** 查询单个章节的正文内容（可能为空）。 */
export async function docxChapterContent(chapterId: number): Promise<string | null> {
  return invoke<string | null>("docx_chapter_content", { chapterId });
}

/** SQLite 查询器：扫描应用数据目录（及自定义目录）中的候选数据库文件。 */
export async function sqliteScan(): Promise<string[]> {
  return invoke<string[]>("sqlite_scan");
}

/** 检测文件是否为 SQLite 数据库（读文件头魔数）。 */
export async function sqliteDetect(path: string): Promise<boolean> {
  return invoke<boolean>("sqlite_detect", { path });
}

/** 列出数据库中的用户表及行数。 */
export async function sqliteTables(path: string): Promise<SqliteTableInfo[]> {
  return invoke<SqliteTableInfo[]>("sqlite_tables", { path });
}

/** 执行 SQL：查询语句返回结果表格，其他语句返回影响行数。 */
export async function sqliteQuery(
  path: string,
  sql: string,
  maxRows?: number
): Promise<SqliteQueryResult> {
  return invoke<SqliteQueryResult>("sqlite_query", { path, sql, maxRows });
}

/** 视频转精灵图：抽帧 →（可选）网格图集 / GIF → zip。 */
export async function videoSpritesheet(args: {
  input: string;
  fps: number;
  maxFrameWidth: number;
  maxSheetSize: number;
  format: string;
  quality: number;
  maxFrames: number;
  generateSprite: boolean;
  generateGif: boolean;
  gifFps: number | null;
  gifDither: boolean;
  outputDir: string | null;
}): Promise<SpriteOutcome> {
  return invoke<SpriteOutcome>("video_spritesheet", args);
}

/** DOCX 标书比对：A 基准 / B 待查，进度走 task:progress 事件。 */
export async function docxCompare(
  pathA: string,
  pathB: string
): Promise<CompareResult> {
  return invoke<CompareResult>("docx_compare", { pathA, pathB });
}
