"""统一块树构建：DOCX → 文档顺序的段落/表格树（渲染与比对同源）。

块编号全局唯一：段落、表格节点、表格单元格各占一个 block id；
比对片段以 (block, 块内偏移) 表达，前端按 data-block-id 锚定。
章节归属走规则模式建树（不调 LLM），仅用于统计维度。
"""
from __future__ import annotations

from pathlib import Path
from typing import Callable

from docx import Document

from doc_analyze.pipeline import features as feats
from doc_analyze.pipeline import numbering as numbering_mod
from doc_analyze.pipeline import tables as tables_mod
from doc_analyze.pipeline.ingest import select_body_items
from doc_analyze.pipeline.treebuild import build_tree

ProgressFn = Callable[[str, int, int, str | None], None]

# w:jc 取值 → CSS text-align（缺省 None = 默认左对齐）
_ALIGN_MAP = {"start": "left", "end": "right", "both": "justify"}


def _chapter_label(node: dict | None) -> str:
    """章节节点 → 统计用短标签（编号 + 标题）。"""
    if node is None:
        return ""
    prefix = (node.get("number_prefix") or "").strip()
    title = (node.get("title") or "").strip()
    return f"{prefix} {title}".strip()


def build_block_tree(
    path: str | Path, progress: ProgressFn | None = None
) -> dict:
    """解析 DOCX → {"tree": [...], "blocks": {block_id: 元数据}}。

    tree 节点：
      段落 {"kind":"paragraph","block":i,"style":"heading|body|list","outline":1..|null,
            "number_prefix","font_halfpt","bold","align","chapter","text"}
      表格 {"kind":"table","block":i,"rows","cols",
            "cells":[{"row","col","rowSpan","colSpan","isHeader","block","chapter","text"}]}
    blocks：参与比对的文本块（段落 + 非空单元格）→ {block_id, kind, text, chapter, match_type}
    （kind: paragraph|cell；match_type: 标题|表格|正文 —— 由 style/kind 推导）
    """
    if progress:
        progress("parse", 0, 0, "解析 DOCX")
    document = Document(str(path))
    style_map = feats.build_style_map(document.styles.element)
    renderer = numbering_mod.NumberingRenderer.from_document(document)

    kept = select_body_items(document)
    kept_p_els = [el for kind, el in kept if kind == "p"]
    para_features = [
        feats.paragraph_features(el, i, style_map) for i, el in enumerate(kept_p_els)
    ]

    # ---- 编号渲染（自动编号前缀，与 ingest 同逻辑）----
    rendered_num: dict[int, str] = {}
    for f in para_features:
        num_id, ilvl = f.num_id, f.ilvl
        if num_id is None and f.style_id:
            mapped = renderer.pstyle_map.get(f.style_id)
            if mapped:
                num_id, ilvl = mapped
        if num_id:
            txt = renderer.render(num_id, ilvl)
            if txt:
                rendered_num[f.para_index] = txt

    # ---- 规则模式章节树（无 LLM）：仅取归属关系 ----
    baseline_font, baseline_indent = feats.compute_baselines(para_features)
    candidates: list[dict] = []
    for f in para_features:
        if not feats.is_candidate(f, baseline_font, baseline_indent):
            continue
        text = f.text.strip()
        number_prefix = None
        title = text
        manual = feats.split_manual_prefix(f.text)
        if manual:
            number_prefix, title = manual
        else:
            auto = rendered_num.get(f.para_index)
            if auto:
                number_prefix = auto
        candidates.append(
            {
                "para_index": f.para_index,
                "text": text,
                "number_prefix": number_prefix,
                "title": title,
                "suggested_level": feats.suggest_level(f),
                "is_anchor": feats.suggest_level(f) is not None,
            }
        )
    nodes = build_tree(candidates, None)
    node_by_para = {n["para_index"]: n for n in nodes}

    # ---- 遍历 body 项 → 块树 ----
    tree: list[dict] = []
    blocks: dict[int, dict] = {}
    next_block = 0
    p_i = 0
    current: dict | None = None

    def para_style(f) -> tuple[str, int | None]:
        lvl = feats.suggest_level(f)
        if lvl is not None:
            return "heading", lvl
        if f.has_numpr or feats.MANUAL_NUM_RE.match(f.text):
            return "list", None
        return "body", None

    for kind, el in kept:
        if kind == "p":
            f = para_features[p_i]
            idx = p_i
            p_i += 1
            node = node_by_para.get(idx)
            if node is not None:
                current = node  # 标题段落：切换当前章节
            style, outline = para_style(f)
            jc = None
            ppr = el.find(feats.W + "pPr")
            if ppr is not None:
                jc_el = ppr.find(feats.W + "jc")
                if jc_el is not None:
                    jc = jc_el.get(feats.W + "val")
            block_id = next_block
            next_block += 1
            chapter = _chapter_label(current)
            tree.append(
                {
                    "kind": "paragraph",
                    "block": block_id,
                    "style": style,
                    "outline": outline,
                    "number_prefix": rendered_num.get(idx),
                    "font_halfpt": f.font_size_halfpt,
                    "bold": f.bold,
                    "align": _ALIGN_MAP.get(jc or "", jc),
                    "chapter": chapter,
                    "text": f.text,
                }
            )
            if f.text.strip():
                blocks[block_id] = {
                    "kind": "paragraph",
                    "text": f.text,
                    "chapter": chapter,
                    "match_type": "标题" if style == "heading" else "正文",
                }
        else:
            grid = tables_mod.extract_grid(el)
            table_block = next_block
            next_block += 1
            chapter = _chapter_label(current)
            cells: list[dict] = []
            for c in grid["cells"]:
                cell_block = next_block
                next_block += 1
                cells.append(
                    {
                        "row": c["row"],
                        "col": c["col"],
                        "rowSpan": c["row_span"],
                        "colSpan": c["col_span"],
                        "isHeader": bool(c["is_header"]),
                        "block": cell_block,
                        "chapter": chapter,
                        "text": c["text"],
                    }
                )
                if c["text"].strip():
                    blocks[cell_block] = {
                        "kind": "cell",
                        "text": c["text"],
                        "chapter": chapter,
                        "match_type": "表格",
                    }
            tree.append(
                {
                    "kind": "table",
                    "block": table_block,
                    "rows": grid["rows"],
                    "cols": grid["cols"],
                    "cells": cells,
                }
            )

    if progress:
        progress("parse", 0, 0, f"块树构建完成（{len(blocks)} 个内容块）")
    return {
        "tree": tree,
        "blocks": blocks,
        "paragraphs": len(kept_p_els),
        "tables": sum(1 for n in tree if n["kind"] == "table"),
    }
