/** 与 Rust 侧结构一一对应的类型定义。 */

export interface TaskProgress {
  task_id: string;
  tool: string;
  stage: string;
  current: number;
  total: number;
  message: string | null;
  /** 任务开始至今的毫秒数。 */
  elapsed_ms: number;
}

export interface InvokeError {
  code: string;
  message: string;
}

/** collect_files 结果 */
export interface CollectSummary {
  dest_dir: string;
  files: number;
  bytes: number;
  renamed: number;
}

/** 批量工具单文件结果 */
export interface FileOutcome {
  input: string;
  ok: boolean;
  output: string | null;
  error: string | null;
  missing_fonts?: string[];
  renamed: boolean;
  /** 该文件的处理耗时（毫秒）。 */
  duration_ms?: number;
}

/** pdf_to_images 批量结果（每个 PDF 一条） */
export interface PdfImagesOutcome {
  input: string;
  ok: boolean;
  /** 输出目录（渲染启动后存在） */
  output: string | null;
  pages: number;
  /** 实际使用的目录名 */
  dir_name: string;
  /** stem（默认文件名）/ dify（工作流生成）/ fallback（回退文件名） */
  dir_source: "stem" | "dify" | "fallback";
  /** 失败原因；成功时若存在则为回退告警 */
  error: string | null;
  /** 该 PDF 的处理耗时（毫秒）。 */
  duration_ms?: number;
}

/** Dify 接口探测结果 */
export interface DifyProbe {
  ok: boolean;
  message: string;
}

export type WatermarkPosition =
  | "top_left"
  | "top_center"
  | "top_right"
  | "middle_left"
  | "center"
  | "middle_right"
  | "bottom_left"
  | "bottom_center"
  | "bottom_right";

export type WatermarkParams =
  | {
    type: "text";
    text: string;
    font_family: string | null;
    font_size: number;
    color: string;
    opacity: number;
    rotation: number;
    position: WatermarkPosition;
    spacing_x?: number | null;
    spacing_y?: number | null;
  }
  | {
    type: "image";
    watermark_path: string;
    scale: number;
    opacity: number;
    rotation: number;
    position: WatermarkPosition;
  }
  | {
    type: "tile";
    text: string;
    font_family: string | null;
    font_size: number;
    color: string;
    opacity: number;
    rotation: number;
    position: WatermarkPosition;
    spacing_x?: number | null;
    spacing_y?: number | null;
  };

export interface Settings {
  soffice_path: string;
  libreoffice_timeout_sec: number;
  libreoffice_concurrency: number;
  pdfium_path: string;
  default_dpi: number;
  default_quality: number;
  max_pages: number;
  font_dir: string;
  default_font: string;
  docx_db_dir: string;
  docx_llm_base_url: string;
  docx_llm_api_key: string;
  docx_llm_auth_header: string;
  docx_llm_model: string;
}

export interface SofficeProbe {
  ok: boolean;
  resolved_path: string | null;
}

/** docx_structure 批量拆分单文件结果 */
export interface DocxOutcome {
  input: string;
  ok: boolean;
  /** 入库后的文档 id，成功时存在 */
  doc_id: number | null;
  chapters: number | null;
  tables: number | null;
  needs_review: number | null;
  total_paragraphs: number | null;
  error: string | null;
  /** 该文件的处理耗时（毫秒）。 */
  duration_ms: number;
}

/** docx_tree 章节树节点（后端已组好树，按顺序渲染即可） */
export interface ChapterNode {
  id: number;
  level: number;
  number_prefix: string;
  title: string;
  needs_review: boolean;
  content_chars: number | null;
  children: ChapterNode[];
}

/** docx_documents 历史文档列表项 */
export interface DocxDocumentInfo {
  id: number;
  doc_name: string;
  status: string;
  total_chars: number | null;
  error_msg: string | null;
  create_time: string | null;
}

/** sqlite_tables 表信息（virtual table 等 COUNT 失败时 row_count 为 -1） */
export interface SqliteTableInfo {
  name: string;
  row_count: number;
}

/** sqlite_query 执行结果 */
export interface SqliteQueryResult {
  columns: string[];
  rows: (string | null)[][];
  total_rows: number;
  truncated: boolean;
  rows_affected: number | null;
}

/** pdf_watermark / pdf_extract_pages 单文件结果 */
export interface PdfEditOutcome {
  output: string;
  pages: number;
  duration_ms: number;
}

/** pdf_watermark 参数 */
export interface PdfWatermarkParams {
  text: string;
  font_size: number;
  color: string;
  opacity: number;
  angle: number;
  density: number;
}

/** video_spritesheet 结果 */
export interface SpriteOutcome {
  ok: boolean;
  zip_path: string;
  /** 预览页地址（sprite-preview 协议，WebView2 下为 http://sprite-preview.localhost/...） */
  preview_url: string;
  frame_count: number;
  requested_fps: number;
  actual_fps: number;
  downsampled: boolean;
  pages: number;
  cols: number;
  frame_width: number;
  frame_height: number;
  duration_sec: number;
  duration_ms: number;
  /** 帧数超上限降级提示（前端必须显式 toast） */
  warning: string | null;
}
