# DOCX 标书比对功能 — 当前项目落地可行性评估

> 评估对象：《DOCX 标书文本查重系统技术方案设计文档.md》（以下简称"原方案"）
> 评估背景：在本项目（Tauri 2 + React + 内置 Python 运行时）中实现标书比对，前端将 DOCX DOM 转为 HTML DOM 渲染并高亮重复片段。

## 1. 结论摘要

**可行，且落地成本低于原方案设想。** 但建议做两处关键调整：

1. **比对引擎用 Python 实现，放弃原方案的纯 Rust 路线**。理由：本项目已内置 Python 运行时且 `doc_analyze` 包已具备 DOCX 解析、表格处理、TOC/模板噪声过滤等成熟能力，直接复用；而原方案推荐的 Rust DOCX 解析库（`undoc` / `office_oxide`）生态尚不成熟，`jieba-rs` 分词质量与词典生态也不及 Python 版 jieba。
2. **原方案的全局偏移映射体系（归一化文本 → 原始字符流）可以裁剪**，替换为"块索引 + 块内字符偏移"的轻量锚定方案。因为前端直接渲染 HTML DOM，高亮的落点是 DOM 文本节点，而不是原始 DOCX 字符流——用户的判断成立。

渲染路径采用**同源架构**：Python 解析 DOCX 产出"统一块树"（文档顺序的段落/表格树 + 样式元数据），比对引擎与前端自研渲染器消费同一棵树——渲染文本与比对文本出自同一字符串，高亮一致性由构造保证；重复片段以 `block_id + 块内偏移` 表达，前端用 DOM Range API 圈选。

## 2. 现状盘点：可直接复用的能力

| 已有能力 | 位置 | 对应原方案模块 | 复用度 |
| --- | --- | --- | --- |
| 内置 Python 3.14.7 运行时（含 python-docx、lxml、typer、pydantic） | `src-tauri/resources/python/` | 文档解析层 | 直接复用 |
| DOCX 解析流水线：TOC 域识别、模板噪声过滤、表格网格还原、编号识别、章节建树 | `doc_analyze/pipeline/ingest.py` 等 | 3.1 文档解析与结构化提取 | 部分复用：块流/单元格网格可复用；章节正文（表格已转 Markdown）不可直接作比对输入，见 2.1 |
| Python 子进程任务框架：冷启动 CLI、JSONL 进度协议、stdout 逐行解析、kill 取消 | [docx_split.rs](file:///d:/Code/tools/src-tauri/src/tools/docx_split.rs) | — | 直接复用运行器模式，新增子命令即可 |
| 通用任务系统：TaskManager、`task:progress` 事件、前端 `useTask` Hook | [task.rs](file:///d:/Code/tools/src-tauri/src/task.rs)、[task.ts](file:///d:/Code/tools/src/lib/task.ts) | 结果输出层 | 直接复用 |
| 工具页注册机制 | [registry.tsx](file:///d:/Code/tools/src/tools/registry.tsx) | — | 按现有模式注册新页面 |
| SQLite 落库 + 章节树回看（docx-structure 工具） | `doc_analyze/db.py` | 结果存储 | 可选复用（查重历史记录，非首期必需） |

### 2.1 复用边界：章节正文不可直接用作比对输入，块流与单元格网格可以

`doc_analyze` 的最终产物（`chapters.content`）把表格转成了 Markdown 并拼进正文，这一层**不能**直接做查重输入：

- 表格转换链路：[ingest.py L310-L311](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/ingest.py#L310-L311) 对每个表格执行 `extract_grid` → `to_markdown`；[ingest.py L325](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/ingest.py#L325) 把 Markdown 拼进章节正文（`contents[...].append(md)`）；[ingest.py L337](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/ingest.py#L337) 章节正文 = 段落文本 + Markdown 表格按 `\n` 拼接落库。
- Markdown 化对文本的三重破坏（[tables.py](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/tables.py)）：
  1. `_md_escape`（[L106-L107](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/tables.py#L106-L107)）：`|` → `\|`、换行 → 空格，文本被改写，字符级定位偏移失效；
  2. row_span 拍平（[L120-L123](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/tables.py#L120-L123)）：合并单元格文本复制填充到每个覆盖位置，比对时会产生虚假重复；
  3. 表头行重排（[L125-L131](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/tables.py#L125-L131)）：表头被移到首行，破坏文档原始顺序，依赖顺序的连续匹配链算法失效。
- 章节级拼接丢失段落/单元格边界，无法对应 docx-preview 渲染出的段落/单元格 DOM 元素，高亮锚定（见 4.2）无从建立。

可直接复用的是 Markdown 化之前的中间产物：

| 中间产物 | 位置 | 查重用途 |
| --- | --- | --- |
| 文档顺序块流（已做 TOC 域剔除、模板噪声与空段过滤） | `_iter_body_items`（[ingest.py L96-L102](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/ingest.py#L96-L102)）、`select_body_items`（[L138-L177](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/ingest.py#L138-L177)） | 查重内容块的生成基础 |
| 段落原始文本（`w:t` 全量拼接，未转义） | `paragraph_features`（[features.py L165-L169](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/features.py#L165-L169)） | 正文块文本 |
| 单元格网格（cell 级 text/is_header，未转义） | `extract_grid`（[tables.py L29-L103](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/tables.py#L29-L103)，`_cell_text` 见 [L13-L15](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/tables.py#L13-L15)） | 表格单元格独立成块（对齐设计文档 3.1），与 docx-preview 渲染的单元格一一对应 |
| 段落→章节归属关系 | [ingest.py L295-L306](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/ingest.py#L295-L306) | 分章节统计（只取归属关系，不取拼接正文） |

因此 compare 模块的接入方式：复用 extract 的解析前半段（`Document` 解析 + `select_body_items`），**表格不走 `to_markdown`**，改为把 `extract_grid` 的 cells 逐个作为独立内容块；段落文本用 `paragraph_features[].text`；章节归属仅用于统计维度。

结论：**原方案五层流水线中，文档解析层在本项目已基本存在**（复用的是块流与单元格网格，而非章节正文产物，见 2.1），真正需要新写的只有：归一化/分词、倒排召回、分层比对、前端渲染高亮四块。

## 3. 关键判断一：比对引擎选 Python，不选 Rust

原方案第 7 章按纯 Rust 生态选型（`undoc`/`jieba-rs`/`textdistance`/`ahash`/`stringzilla`）。放到本项目逐一检验：

| 原方案选型 | 问题 | Python 替代 | 评估 |
| --- | --- | --- | --- |
| `undoc` / `office_oxide` 解析 DOCX | 生态不成熟，表格/复杂结构支持存疑，引入即踩坑 | `python-docx`（**项目已在用**，表格、编号、TOC 均已验证） | Python 显著占优 |
| `jieba-rs` 中文分词 | 词典规模、HMM 效果、自定义词典生态弱于原版 | `jieba`（纯 Python，零编译风险，支持自定义词典） | Python 占优 |
| `textdistance`（Rust） | 可用但非必需 | `rapidfuzz`（C++ 扩展，编辑距离/多种 ratio，性能优异）；兜底 `difflib`（标准库） | Python 占优 |
| `ahash` / `stringzilla` | 性能优化件，属锦上添花 | 3-gram 哈希用 Python 内建 `hash()`/整型键 dict 足够；标书文档量级（数千句）纯 Python 可承受 | 持平，非瓶颈 |

**性能量级核算**：标书典型规模数百页 ≈ 数千段落/数万句子。倒排召回 O(m+n)、Levenshtein 仅作用于候选对（漏斗后通常 <5% 句子量），Python 单进程冷启动跑完全流程预计在秒级，符合桌面工具预期。原方案的 SIMD/预分配等优化手段是为超大规模文档设计的，对本场景属过度设计。

**架构收益**：沿用 `docx_split.rs` 的子进程模式，Python 端只需在 `doc_analyze` 包内新增 `compare` 子命令，Rust 端新增一个 command 做透传编排，任务进度/取消/错误上报全部白得。

## 4. 关键判断二：渲染路径与坐标映射的简化

### 4.1 渲染与比对同源：Python 产出统一块树，前端基于块树自渲染（采纳）

单一事实源原则：DOCX 只被 Python 解析**一次**，产出「统一块树」（文档顺序的段落/表格树，携带文本与样式元数据），比对引擎与前端渲染器消费同一棵树：

- 比对引擎：块树文本 → 归一化/分词/召回/比对；
- 前端渲染器：块树 → HTML DOM，每个块元素挂 `data-block-id` 锚点。

粒度取舍——块树应是**简化块树**而非原始 OOXML 透传：

- 把 `document.xml` 原样丢给前端渲染，前端就要处理 styles.xml/numbering.xml/主题/关系/图片/页眉页脚，等于重写一个 docx-preview，工作量不可控；
- 查重审阅场景只需要"结构正确 + 基本样式像原文"的呈现，简化块树 + 轻量渲染器即可满足，且渲染保真的增强可以按层迭代。

| 方案 | 一致性 | 保真度 | 工作量 | 评价 |
| --- | --- | --- | --- | --- |
| **Python 统一块树 + 前端自渲染（推荐）** | 构造性一致：渲染文本与比对文本出自同一字符串 | 中：结构+段落级样式，无分页/页眉页脚/图片 | Python 序列化 + 前端有界渲染器 | 高亮精确性是查重工具核心价值，一致性由构造保证 |
| docx-preview（npm）双解析 | 有风险：与 python-docx 对 tab/br/域的文本处理可能不一致，需指纹校验+降级兜底 | 高：接近 Word 分页原样 | 几乎为零 | 追求分页保真时的备选 |
| mammoth（Python 转 HTML） | 同源 | 低：语义 HTML，丢版式 | 小 | 不满足"像原文"的审阅体验 |

### 4.2 坐标锚定：用"块索引 + 块内偏移"替代全局偏移映射

原方案 3.2 的"全局归一化纯文本 + 全局偏移映射数组"、4.6 的 OffsetMap，服务的是"字符级定位回原始 DOCX 字符流"这一目标。本项目场景下：

- 高亮的最终落点是 **DOM 文本节点**，原始字符流本身不再被消费；
- 每个内容块（段落/单元格）渲染为一个独立的 HTML 元素，块内文本 = 块树携带的段落文本，与 DOM 中的 textContent 严格相等（同源保证）；
- 因此映射退化为：**`(doc, block_index, offset_in_block, length)` 四元组**，前端在对应块元素内遍历文本节点、用 Range API 圈选高亮。

原方案数据模型的相应调整：

| 原字段 | 调整 |
| --- | --- |
| `norm_start / norm_end`（全局归一化偏移） | 改为块内偏移；归一化仅在块内进行 |
| `offset_map: Vec`（归一化→原始位置映射数组） | **删除**。块内高亮直接按"归一化时过滤的字符在块内文本中的位置"做一次简单换算即可（或高亮时对块文本做同样的轻量归一化再定位） |
| `raw_start_a / raw_end_a`（原始文档偏移） | 改为 `block_index + 块内起止`，语义变为"HTML 渲染块内的定位" |

需要注意的唯一换算点：归一化会删字符（空格、全半角折叠等），块内偏移需从归一化坐标换算回块原始文本坐标。这保留一个**块内**的小映射即可，复杂度从"全文档级"降到"单段落级"，实现量很小。

### 4.3 统一块树的 Schema 与 Python 端序列化能力盘点

块树节点（JSON 形态，比 XML 更贴合 Tauri IPC/serde 链路；若坚持 XML 序列化，前端用 `DOMParser` 解析亦可，语义等价）：

```jsonc
// 段落节点
{ "kind": "paragraph", "block": 0, "style": "heading1|body|list",
  "outline": 1, "number_prefix": "第二章", "font_halfpt": 32, "bold": true,
  "align": "center", "text": "……", "runs": [ { "text": "……" } ] }
// 表格节点（cells 递归含段落块，block 编号全局唯一）
{ "kind": "table", "block": 1, "rows": 3, "cols": 4,
  "cells": [ { "row": 0, "col": 1, "rowSpan": 2, "colSpan": 1,
               "isHeader": true, "block": 2, "text": "……" } ] }
```

Python 端序列化原料盘点——绝大部分已有实现，仅需补少量字段：

| 渲染要素 | 来源（已有实现） | 缺口 |
| --- | --- | --- |
| 段落原始文本 | `paragraph_features[].text`（[features.py L165-L169](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/features.py#L165-L169)） | 无 |
| 标题层级 | outline_lvl + 样式尾数（[features.py L255-L263](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/features.py#L255-L263)、[L324-L329](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/features.py#L324-L329)） | 无 |
| 字号/加粗/缩进 | `ParagraphFeatures`（[features.py L56-L69](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/features.py#L56-L69)） | 无 |
| 编号前缀 | `NumberingRenderer.render`（[numbering.py L276-L280](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/numbering.py#L276-L280)，已产出「第二章」「2.1」形态） | 无 |
| 表格结构 | `extract_grid`（[tables.py L29-L103](file:///d:/Code/tools/src-tauri/resources/python/Lib/site-packages/doc_analyze/pipeline/tables.py#L29-L103)，rowSpan/colSpan/is_header 直接映射 HTML 同名属性） | 无 |
| 段落对齐 | — | 需补读 `w:jc`（小改动） |
| run 级斜体/下划线/颜色 | — | v1 整段样式渲染即可，v2 增强 |

前端渲染器职责（有界）：块树 → `h1~h6/p/table/td` + 内联样式；合并单元格 → rowSpan/colSpan；块元素挂 `data-block-id`。图片/文本框/页眉页脚 v1 以占位略过（标书比对以文字内容为核心）。

### 4.4 高亮锚定：从"校验+降级"升级为"构造性一致"

原方案（docx-preview 双解析）的一致性风险——渲染 textContent 与 python-docx 提取文本可能不等（tab/br/域处理差异）——在同源架构下**从构造上消除**：

- 渲染进 DOM 的文本就是块树里的同一个字符串，无需指纹校验、无需降级兜底；
- 定位链条全程确定性：片段 `(block_id, 归一化起止)` → 块内映射换算 → 块原始文本偏移 → 在块元素内按文本节点累计长度圈选 Range（run 拆分导致文本节点有多个，但累计长度与块文本严格相等）；
- 归一化删字符（空格折叠、全半角统一）仍需**块内**小映射做换算，复杂度维持单段落级（见 4.2）；
- 片段坐标建议由 Python 端**预先换算为块原始文本偏移**再输出，前端拿到即可直接圈选，零换算。

## 5. 原方案逐模块裁剪清单

| 原方案模块 | 处置 | 说明 |
| --- | --- | --- |
| 3.1 文档解析与结构化提取 | **复用+精简** | 直接基于 `doc_analyze` 现有解析；内容块抽象保留（block_id 改为文档顺序索引） |
| 3.2 归一化与偏移映射 | **简化** | 归一化逻辑保留（清洗、句子拆分、停用词）；全局偏移映射删除，改块内偏移（见 4.2） |
| 3.3 倒排索引与候选召回 | **保留，Python 重写** | 3-gram 词级碎片 + dict 倒排 + Top-N 召回，纯 Python 实现 |
| 3.4 分层精细比对 | **保留，Python 重写** | Jaccard 初筛 → rapidfuzz/Levenshtein 精算 → 连续匹配链（DP），算法原样迁移 |
| 3.5 字符级定位 | **保留并简化** | LCSubstring/公共子串定位保留；"坐标映射回原文"一步替换为块内偏移换算 |
| 3.6 重复率计算与输出 | **保留，输出目标改变** | "HTML 高亮报告导出"不再是终点——应用内直接渲染高亮；JSON 结构化结果为前端消费服务；加权统计保留 |
| 4.6 OffsetMap 全局映射表 | **删除** | 见 4.2 |
| 7. Rust 技术选型 | **整体替换** | 换 Python 栈（见第 3 节） |
| 8. 阶段四 GUI 规划（egui/Tauri） | **删除** | 本项目本身就是 GUI，双栏高亮即首期功能 |

## 6. 目标架构

```
┌─ 前端 (React 自研块树渲染器) ──────────────────────────────┐
│  新工具页「标书比对」：双栏渲染块树 A/B（data-block-id 锚点）│
│  高亮引擎：按 (block_id, 块内偏移) 遍历文本节点 + Range 圈选 │
│  统计面板：重复率 / 分章节排行 / 高中低置信度片段列表         │
└──────────────▲──────────────────────────────┬─────────────┘
               │ task:progress 事件            │ invoke docx_compare
┌──────────────┴──────────────────────────────▼─────────────┐
│  Rust (src-tauri/src/tools/docx_compare.rs)               │
│  复用 docx_split.rs 运行器模式：子进程 / JSONL / 取消       │
└──────────────┬─────────────────────────────────────────────┘
               │ python -m doc_analyze.cli compare A.docx B.docx --json
┌──────────────▼─────────────────────────────────────────────┐
│  Python (doc_analyze/compare/)                            │
│  ① 解析双文档 → 统一块树（复用现有 pipeline + 样式元数据）   │
│  ② 块内归一化 + jieba 分词 + 句子拆分（基于块树文本）        │
│  ③ 3-gram 倒排索引召回 Top-N 候选句对                      │
│  ④ Jaccard 初筛 → Levenshtein(rapidfuzz) 精算 → 连续匹配链  │
│  ⑤ 字符级公共子串定位 → (block_id, 块内偏移) 片段集合        │
│  ⑥ 重复率统计（分章节/分类型/置信度）                       │
└────────────────────────────────────────────────────────────┘
```

### 新增依赖

| 端 | 依赖 | 说明 |
| --- | --- | --- |
| Python | `jieba` | 纯 Python，无编译风险，需加入 `setup:python` 装配脚本 |
| Python | `rapidfuzz` | 有 C++ 扩展，需验证 cp314 wheel 可用性；**不可用时兜底 `difflib`**（仅候选对精算，性能可接受） |
| npm | 无新增 | 自研块树渲染器为普通 React 组件；不引入 docx-preview（若后续追求分页级保真可作为增强再引入） |

### 数据契约（Python → 前端，result JSON 核心结构）

```jsonc
{
  "docs": { "a": { "tree": [ /* 统一块树，见 4.3 schema */ ] },
             "b": { "tree": [ /* … */ ] } },
  "fragments": [ {
    "a": { "block": 12, "start": 5, "end": 48 },   // 块原始文本偏移（Python 已完成归一化→原始换算）
    "b": { "block": 30, "start": 0, "end": 43 },
    "length": 43, "confidence": "high|mid|low",
    "content": "……", "match_type": "正文|表格|标题"
  } ],
  "stats": { "overall_rate": 0.183, "by_chapter": [...], "by_confidence": {...}, "weighted_rate": 0.21 }
}
```

## 7. 风险清单与缓解

| 风险 | 等级 | 缓解 |
| --- | --- | --- |
| rapidfuzz 无 cp314 wheel | 中 | 装配脚本预检；兜底 `difflib.SequenceMatcher`（候选量小，性能可接受） |
| 自研渲染器保真度上限（无分页/页眉页脚/图片，run 级样式缺失） | 中 | v1 以"结构+段落级样式"呈现，满足查重审阅；图片/页眉页脚占位略过；保真增强列 v2（run 级样式、图片） |
| 大文档块树 JSON 体积（数百页可达数 MB） | 低 | Tauri IPC 可承载；必要时落临时文件经 custom protocol 读取（项目已有 sprite-preview 先例） |
| 嵌套表格 / 合并单元格定位错位 | 中 | 首期对嵌套表降级为单元格级高亮；合并单元格按起点单元格定位 |
| 超大文档（千页级）Python 性能 | 低 | 漏斗架构本身削峰；必要时句子粒度并行；进度事件已有，用户体验不阻塞 |
| 高亮与滚动联动复杂度 | 低 | 首期做"点击片段列表 → 双栏滚动定位"，同步滚动为二期增强 |

## 8. 实施路线（映射原方案四阶段）

1. **阶段一 — 引擎核心**：Python `compare` 模块（解析复用 → 归一化/分词 → 倒排召回 → 三层比对 → 片段 JSON）；Rust command 透传；先用纯文本双栏验证查重效果，不接 docx-preview。
2. **阶段二 — 渲染与高亮**：Python 端块树序列化（补 `w:jc` 等少量字段）+ 前端自研双栏渲染器；高亮引擎（文本节点遍历 + Range，构造性一致、无需校验降级）；片段列表联动定位。
3. **阶段三 — 产品化**：分章节加权统计、阈值/停用词配置（并入设置页）、连续匹配链调参、HTML 报告导出（可选）。
4. **阶段四 — 增强（可选）**：批量比对（目录级）、同步滚动、查重历史落库（复用 SQLite 模式）、PDF 扩展。

## 9. 最终结论

- 原方案的**算法骨架（漏斗式分层过滤 + 连续匹配链）完全成立，原样保留**；
- 其**工程载体（Rust 栈 + 全局偏移映射 + HTML 报告导出）与本项目不适配，整体替换**为：内置 Python 引擎 + 统一块树同源架构（Python 解析一次，比对与渲染消费同一棵树）+ DOM Range 高亮；
- 项目现有的 Python 运行时、`doc_analyze` 解析能力、任务框架、工具页注册机制覆盖了约一半工程量，整体落地风险集中在 **rapidfuzz wheel 可用性**与**自研渲染器保真度**两点：前者有 difflib 兜底，后者按保真度分层推进（结构+段落样式 → run 级样式/图片）。

**建议按第 8 节路线推进，阶段一先行验证查重算法效果后再投入渲染开发。**
