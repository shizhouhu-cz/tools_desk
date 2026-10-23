import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import {
  FileDiff,
  FileSearch,
  FileStack,
  Film,
  FolderInput,
  Images,
  Presentation,
  Scissors,
  Stamp,
  Table2,
  Type,
} from "lucide-react";
import FileCollector from "@/tools/file-collector";
import PptxToPdf from "@/tools/pptx-to-pdf";
import PdfToImages from "@/tools/pdf-to-images";
import PdfWatermark from "@/tools/pdf-watermark";
import PdfExtract from "@/tools/pdf-extract";
import Watermark from "@/tools/watermark";
import DocxStructure from "@/tools/docx-structure";
import DocxCompare from "@/tools/docx-compare";
import SqliteExplorer from "@/tools/sqlite-explorer";
import VideoSpritesheet from "@/tools/video-spritesheet";

export interface ToolMeta {
  id: string;
  path: string;
  title: string;
  subtitle: string;
  description: string;
  icon: LucideIcon;
  element: ReactNode;
}

/** 工具注册表：新增工具 = 加一条记录 + 对应页面组件。 */
export const TOOLS: ToolMeta[] = [
  {
    id: "file-collector",
    path: "/tools/file-collector",
    title: "文件收集器",
    subtitle: "归集目录下所有文件",
    description: "选定目录后，在该目录下新建临时目录，把目录（含子目录）中的全部文件复制进去。",
    icon: FolderInput,
    element: <FileCollector />,
  },
  {
    id: "pptx-to-pdf",
    path: "/tools/pptx-to-pdf",
    title: "PPTX 转 PDF",
    subtitle: "基于本机 LibreOffice",
    description: "把选中的 PPTX/PPT 文件批量转换为 PDF，支持自定义输出目录。",
    icon: Presentation,
    element: <PptxToPdf />,
  },
  {
    id: "pdf-to-images",
    path: "/tools/pdf-to-images",
    title: "PDF 切图片",
    subtitle: "按页渲染为图片",
    description: "把 PDF 按页渲染为 PNG/JPEG/WebP 图片，可指定 DPI 与页码范围。",
    icon: Images,
    element: <PdfToImages />,
  },
  {
    id: "pdf-watermark",
    path: "/tools/pdf-watermark",
    title: "PDF 打水印",
    subtitle: "斜向平铺 / 嵌入中文字体",
    description: "给 PDF 每页添加半透明平铺文字水印，嵌入系统中文字体，单文件另存输出。",
    icon: Type,
    element: <PdfWatermark />,
  },
  {
    id: "pdf-extract",
    path: "/tools/pdf-extract",
    title: "PDF 页面提取",
    subtitle: "按页码范围切割另存",
    description: "从 PDF 中提取指定页码范围，生成新的 PDF 文件，不重新编码、无损输出。",
    icon: Scissors,
    element: <PdfExtract />,
  },
  {
    id: "watermark",
    path: "/tools/watermark",
    title: "图片打水印",
    subtitle: "文字 / 图片 / 平铺",
    description: "批量给图片加文字、图片或平铺水印，支持透明度、旋转与九宫格定位。",
    icon: Stamp,
    element: <Watermark />,
  },
  {
    id: "docx-structure",
    path: "/tools/docx-structure",
    title: "DOCX 结构化拆分",
    subtitle: "章节树 / 表格 / 全文索引",
    description: "把 DOCX 按章节层级结构化入库（SQLite + 全文索引），可选 LLM 结构恢复，支持章节树回看。",
    icon: FileSearch,
    element: <DocxStructure />,
  },
  {
    id: "docx-compare",
    path: "/tools/docx-compare",
    title: "DOCX 标书比对",
    subtitle: "双栏查重 / 片段高亮",
    description: "对两份 DOCX 做文本查重：倒排召回 + 分层比对定位重复片段，双栏同源渲染并高亮，输出重复率与分章节统计。",
    icon: FileDiff,
    element: <DocxCompare />,
  },
  {
    id: "sqlite-explorer",
    path: "/tools/sqlite-explorer",
    title: "SQLite 查询器",
    subtitle: "连接 / 表结构 / SQL 查询",
    description: "连接本机 SQLite 数据库文件，浏览表结构与行数，直接执行 SQL 语句查看结果。",
    icon: Table2,
    element: <SqliteExplorer />,
  },
  {
    id: "video-spritesheet",
    path: "/tools/video-spritesheet",
    title: "视频转精灵图",
    subtitle: "抽帧 / 网格图集 / zip 播放包",
    description: "把视频按采样帧率抽帧生成网格精灵图集，打包含自包含播放器的 zip，工具内可直接预览播放。",
    icon: Film,
    element: <VideoSpritesheet />,
  },
];

export const TOOL_FILES_ICON = FileStack;
