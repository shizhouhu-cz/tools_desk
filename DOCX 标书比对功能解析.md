# DOCX 标书比对功能解析

> 面向 code review 的实现说明：代码清单、消息传递过程、数据流动。
> 方案依据：[DOCX 标书比对功能可行性评估.md](file:///d:/Code/tools/DOCX%20标书比对功能可行性评估.md)（第 6 节数据契约、第 8 节实施路线）。

---

## 1. 核心设计思想：统一块树同源架构

传统比对工具的痛点是"比对坐标"与"渲染坐标"来自两套解析器，高亮容易错位。本功能的解法是**只解析一次**：

```
DOCX 文件
   │  python-docx 解析（仅此一处）
   ▼
统一块树 tree[]（段落 / 表格 / 单元格，各自携带全局唯一 block id）
   │
   ├──► 前端自研渲染器：按块树顺序渲染，节点挂 data-block-id
   │
   └──► 比对引擎：基于块树文本做归一化 → 句拆分 → 召回 → 精比
        片段坐标 = (block id, 块【原始文本】内偏移 start/end)
```

- **块内归一化**：比对在归一化文本（NFKC、全半角统一、中文标点映射、空白折叠）上进行，但对外的片段坐标已通过 `norm_to_raw` 映射数组**换算回块原始文本偏移**。
- **前端零换算**：拿到片段后直接 `text.slice(start, end)` 圈选 `<mark>`，高亮一致性由构造保证（有单测断言 `块文本[_start:_end] === content`）。
- 由此，评估文档中"比对基准文本 → 原文字符流的全局映射"一整层复杂度被消除。

---

## 2. 代码修改清单

### 2.1 新增 — Python 比对引擎（源码目录，装配时拷贝进内置运行时）

| 文件 | 作用 |
|---|---|
| [compare/\_\_init\_\_.py](file:///d:/Code/tools/python/docx_split/src/doc_analyze/compare/__init__.py) | 包声明。 |
| [compare/normalize.py](file:///d:/Code/tools/python/docx_split/src/doc_analyze/compare/normalize.py) | 块内归一化与句拆分。`normalize(text) → (归一化文本, norm_to_raw)`：isspace 优先折叠空白、NFKC + 中文标点 translate 一对一映射（不破坏偏移）、零宽/控制字符删除、尾部 strip。`split_sentences(norm)`：句末标点切分（`.` 两侧均为数字不切，保护小数/编号）、句首编号前缀剥离（多级数字可直接剥，单级序号必须带分隔符，保护"3年以上"）、纯编号/无实义句剔除。 |
| [compare/blocks.py](file:///d:/Code/tools/python/docx_split/src/doc_analyze/compare/blocks.py) | 统一块树构建（渲染与比对同源的唯一解析点）。复用 doc_analyze 既有管线：`select_body_items`（TOC/噪声过滤）、`paragraph_features`（w:t 全量拼接 + 字体/加粗特征）、`NumberingRenderer`（自动编号前缀）、规则模式 `build_tree(candidates, None)`（章节归属，不调 LLM）。段落节点直读 `pPr/w:jc` 映射为 CSS text-align；表格走 `extract_grid`（cell 级 rowSpan/colSpan/isHeader）。产出 `tree[]`（渲染用）+ `blocks{}`（仅参与比对的非空文本块，含 match_type 标题/正文/表格）。**表格不做 to_markdown**，避免 MD 污染比对文本。 |
| [compare/engine.py](file:///d:/Code/tools/python/docx_split/src/doc_analyze/compare/engine.py) | 查重比对引擎（漏斗式分层，见 §4）。jieba/rapidfuzz 均 try/except 兜底（字符级 tokenizer / 纯 Python DP），缺包不崩。 |
| [tests/test_compare.py](file:///d:/Code/tools/python/docx_split/tests/test_compare.py) | 9 个单测：归一化映射契约 ×3、句拆分 ×2、端到端检出/坐标切片一致性/统计有效性/无交集文档低误报 ×4。 |

### 2.2 新增 — Rust 侧（Tauri）

| 文件 | 作用 |
|---|---|
| [src-tauri/src/tools/docx_compare.rs](file:///d:/Code/tools/src-tauri/src/tools/docx_compare.rs) | Python 子进程运行器。冷启动 `python.exe -X utf8 -I -m doc_analyze.cli compare <A> <B> --json`，CREATE_NO_WINDOW；`register_child` 注册子进程供取消时杀进程树；stderr 独立线程排空（防管道写满阻塞子进程）并留尾部 8KB 诊断缓冲；逐行解析 stdout JSONL（progress/result/error）；收尸取退出码。强类型仅 `CompareFragment`（需要序列化回传），`docs`/`stats` 为 `serde_json::Value` 直通。 |
| [src-tauri/src/commands/docx_compare.rs](file:///d:/Code/tools/src-tauri/src/commands/docx_compare.rs) | Tauri command `docx_compare(app, tasks, settings, path_a, path_b)`。参数校验 → `tasks.create("docx-compare")` 建任务 → `resolve_python_path` 解析内置 Python → `llm_env(&cfg, false)`（compare 不用 LLM，只透传 UTF-8 环境变量）→ `spawn_blocking` 跑运行器（不占 async 线程）→ 错误含"取消"映射 `AppError::Cancelled` → `progress_now("done")` + `tasks.finish` → 返回 `CompareOutcome`。 |

### 2.3 新增 — 前端工具页

| 文件 | 作用 |
|---|---|
| [src/tools/docx-compare/index.tsx](file:///d:/Code/tools/src/tools/docx-compare/index.tsx) | 工具页。`renderMarked`：按 (block,start) 排序合并重叠区间后切片渲染 `<mark>`（置信度配色：high 红 / mid 琥珀 / low 蓝）；`buildMarks`：片段按 a/b 侧分组为 `Map<block, Mark[]>`；`ParaBlock`：heading 加粗/字号（font_halfpt/2 pt）/对齐（四合法值窄化）/list 缩进/编号前缀拼接；`TableBlock`：cellMap + occupied 集合还原 rowSpan/colSpan，表头底色；`DocPane`：sticky 文件名标题；片段列表点击 `jump(i)` 双栏 `scrollIntoView` + `cmp-flash` 闪烁动画（offsetWidth 强制 reflow 重启动画）。运行校验：两文件必选、不可相同；取消按 `code === "cancelled"` 静默 toast。 |

### 2.4 修改 — 既有文件

| 文件 | 改动 | 作用 |
|---|---|---|
| [doc_analyze/cli.py](file:///d:/Code/tools/python/docx_split/src/doc_analyze/cli.py#L78-L123) | 新增 `compare` 子命令（extract 与 ingest 之间） | CLI 入口：`--json` 时 stdout 仅输出协议行（progress/result/error），人读模式输出重复率摘要；依赖缺失 exit 3、比对失败 exit 1。 |
| [scripts/setup-python-runtime.ps1](file:///d:/Code/tools/scripts/setup-python-runtime.ps1#L67-L87) | pip 依赖行 + 冒烟导入 | `--only-binary=:all:` 收窄为 `--only-binary=rapidfuzz`（rapidfuzz 必须用 cp314 wheel；jieba 仅 sdist，纯 Python 无编译风险，允许源码安装）；依赖列表与冒烟导入加 `jieba rapidfuzz`。 |
| [src-tauri/src/tools/mod.rs](file:///d:/Code/tools/src-tauri/src/tools/mod.rs) | `pub mod docx_compare;` | 注册工具模块。 |
| [src-tauri/src/commands/mod.rs](file:///d:/Code/tools/src-tauri/src/commands/mod.rs) | `pub mod docx_compare;` | 注册命令模块。 |
| [src-tauri/src/lib.rs](file:///d:/Code/tools/src-tauri/src/lib.rs#L46) | invoke_handler 注册 `docx_compare` | 暴露 IPC 命令。 |
| [src/lib/types.ts](file:///d:/Code/tools/src/lib/types.ts#L223-L311) | 新增 `CompareFragLoc / CompareFragment / CompareStats / CompareParaNode / CompareCellNode / CompareTableNode / CompareNode / CompareResult` | 与 Rust/Python 数据契约一一对应的 TS 类型。 |
| [src/lib/tauri.ts](file:///d:/Code/tools/src/lib/tauri.ts#L163-L169) | 新增 `docxCompare(pathA, pathB)` | `invoke("docx_compare", { pathA, pathB })` 封装。 |
| [src/lib/task.ts](file:///d:/Code/tools/src/lib/task.ts#L104-L106) | `STAGE_LABELS` 加 `index/recall/match` | 进度面板阶段中文标签："构建索引 / 候选召回 / 精细比对"。 |
| [src/tools/registry.tsx](file:///d:/Code/tools/src/tools/registry.tsx#L103-L110) | 注册工具项（FileDiff 图标，路由 `/tools/docx-compare`） | 工具列表与路由接入。 |

---

## 3. 消息传递全过程（时序）

```
┌──────────┐   invoke("docx_compare",{pathA,pathB})   ┌─────────────────────────┐
│ 前端工具页 │ ───────────────────────────────────────► │ Rust command            │
│ index.tsx │                                          │ commands/docx_compare.rs│
└──────────┘                                           └───────────┬─────────────┘
     ▲                                                             │
     │ task:progress 事件（useTask("docx-compare") 按 tool 过滤）     │ 1. tasks.create() 建任务
     │    ◄── TaskManager.emit ◄─ handle.progress_msg ◄─┐           │ 2. resolve_python_path()
     │                                                  │           │ 3. llm_env(cfg,false)
     │                                                  │           │ 4. spawn_blocking ──┐
     │                                                  │           ▼                     ▼
     │                                                  │   ┌───────────────────────────────────┐
     │                                                  └───┤ tools/docx_compare.rs 运行器       │
     │                                                      │  stdout 逐行 JSONL：               │
     │                                                      │   progress → handle.progress_msg  │
     │                                                      │   result   → CompareOk            │
     │                                                      │   error    → 记录待用             │
     │                                                      └───────────────┬───────────────────┘
     │                                                                      ▼
     │                                                      ┌───────────────────────────────────┐
     │                                                      │ 内置 Python 子进程（冷启动）        │
     │                                                      │ python -X utf8 -I -m doc_analyze  │
     │                                                      │   .cli compare A B --json         │
     │                                                      │                                   │
     │                                                      │ stdout（协议行，唯一出口）：        │
     │                                                      │   {"type":"progress",...}         │
     │                                                      │   {"type":"result","ok":true,     │
     │                                                      │     "docs":..., "fragments":...,  │
     │                                                      │     "stats":...}                  │
     │                                                      │   {"type":"error","message":...}  │
     │                                                      │ stderr（诊断，jieba 日志已静默）    │
     │                                                      └───────────────────────────────────┘
     │
     │  invoke promise resolve：CompareOutcome{ok,docs,fragments,stats,duration_ms}
     ◄────────────────────────────────────────────────────────────（大 JSON 单次 IPC 回传）
```

**时序要点：**

1. **发起**：前端 `run()` 校验（两文件必选、不可相同）→ `docxCompare()` → `invoke`。
2. **建任务**：Rust `tasks.create(&app, "docx-compare")`；前端 `useTask("docx-compare")` 依 tool 名过滤 `task:progress` 事件驱动 ProgressPanel（阶段标签见 `STAGE_LABELS`）。
3. **子进程协议**：stdout 是唯一协议通道（JSONL，每行一个 JSON 对象）；一切诊断信息（python traceback、jieba 构建日志等）走 stderr。Rust 对 stdout 中非 JSON 行静默跳过兜底。
4. **取消**：前端 `task.cancel()` → TaskManager kill 注册的子进程树 → 运行器 `handle.is_cancelled()` 返回"任务已取消" → command 层映射 `AppError::Cancelled`（序列化 `{code:"cancelled"}`）→ 前端 catch 后 toast.info，不弹错误。
5. **收尾**：`take_children().wait()` 收尸取退出码；退出码 0 但无 result 行也报错（"python 未输出 result 行"）；非 0 时错误详情优先取 error 行，否则取 stderr 尾部 300 字符。
6. **事件顺序不保证**（项目既有经验）：任务完成以后端 `done` 阶段 + invoke promise resolve 为准，不依赖 progress 事件次序。

---

## 4. Python 比对引擎数据流（engine.run_compare）

```
build_block_tree(A)          build_block_tree(B)          ← parse 阶段（progress: parse 1/2, 2/2）
      │ tree + blocks               │
      └──────────┬──────────────────┘
                 ▼
        _build_sents（index 阶段）
  每个内容块：normalize → (norm, mapping)
  split_sentences → Sent{seq, block, start/end(归一化), raw_start/raw_end(映射换算), words, grams}
  words  = jieba 分词 − 停用词 − 纯标点 token（jieba 会切出 "," "." 等）
  grams  = 词级 3-gram hash（<3 词时整句词序作单一碎片）
                 ▼
        _recall（recall 阶段，B 句每 200 句报进度）
  A 句 grams 建倒排索引 → B 句逐句查命中 Counter
  score = 命中碎片数 / B 句碎片数 ≥ 0.3，取 Top-5 → 候选对 (ia,ib) 集合
                 ▼
        三层精比（match 阶段）
  ① 词集 Jaccard ≥ 0.25（初筛）
  ② Levenshtein 相似度 = 1 − dist/max_len ≥ 0.6（精算，rapidfuzz）
  ③ 连续匹配链 DP：confirmed[(ia,ib)] 存在且 (ia−1, ib−1) 也存在 → 链长 +1
                 ▼
        _locate_fragments
  每对确认句：SequenceMatcher(autojunk=False) 公共子串
  → _merge_blocks（a/b 两维间隔均 ≤3 合并）
  → 片段长度 ≥ 8 归一化字符（滤短噪声）
  → norm→raw 映射换算回块原始偏移
  → fragment{a:{block,start,end}, b:{...}, length, confidence, chain, content, match_type}
  confidence：链 ≥3 或 (lev≥0.99 且句 ≥20 字) → high；链 =2 或 (lev≥0.9 且 ≥12 字) → mid；否则 low
  排序：按 B 侧 (block, start)
                 ▼
        _compute_stats
  rate_a/rate_b：块内区间并集去重后覆盖字符 / 总有效字符（B 视角为主指标）
  by_confidence：分级片段数与覆盖字符
  by_chapter：B 视角按章节聚合（covered 降序）
  elapsed_seconds
                 ▼
  {"docs":{"a":{"name","tree"},"b":{...}}, "fragments":[...], "stats":{...}}
  ── cli.py 以 {"type":"result","ok":true, ...} 单行写入 stdout
```

**关键工程取舍：**

- **无全局偏移映射**：归一化仅在块内进行，片段坐标永远是 `(block, 块内原始偏移)` 四元组，前端无需任何换算层。
- **短片段过滤是特性不是缺陷**：`MIN_FRAG_CHARS=8` 会滤掉如"双电源冗余"这类 <8 字的巧合重复，属有意降噪（单测按此设计）。
- **置信度两级修正**：完全复制的孤立整句曾因链长=1 被标 low，故补充 `(lev_sim, 句长)` 提升规则（见 `_confidence`）。
- **依赖兜底**：jieba 缺包 → 字符级正则 tokenizer；rapidfuzz 缺包 → 纯 Python DP（仅候选对精算，量小可承受）。

## 5. 前端渲染与高亮数据流

```
CompareResult
   │
   ├─ fragments ──► buildMarks() ──► { a: Map<block, Mark[]>, b: Map<block, Mark[]> }
   │                                  （Mark = {start, end, confidence}，原始文本偏移）
   │
   └─ docs.a/b.tree ──► DocPane 逐节点渲染
        ├─ paragraph → ParaBlock：style/字号/对齐/编号前缀，data-block-id={block}
        │     文本 = number_prefix + text，renderMarked(text, marks) 切片渲染 <mark>
        └─ table → TableBlock：cellMap + occupied 还原合并单元格
              每个非空 cell data-block-id={cell.block}，renderMarked(cell.text, ...)

片段列表点击 jump(i) → scrollToBlock(paneRef, f.a/f.b.block)
   → querySelector(`[data-block-id="${block}"]`)
   → scrollIntoView(center) + cmp-flash 闪烁 1.4s（offsetWidth 强制 reflow 重启）
```

- 同一 block 内多片段先按 start 排序、重叠区间合并，再切片渲染，保证 `<mark>` 互不嵌套。
- 高亮颜色与片段列表 Badge 配色一致（红/琥珀/蓝 = high/mid/low），视觉上可直接对应。

## 6. 数据契约（跨层一览）

```ts
// 片段坐标：块 id + 块原始文本偏移（Python 已换算，前端零换算）
interface CompareFragLoc { block: number; start: number; end: number }

interface CompareFragment {
  a: CompareFragLoc; b: CompareFragLoc;   // a=基准侧，b=待查侧
  length: number;                          // 归一化字符长度
  confidence: "high" | "mid" | "low";
  chain: number;                           // 连续匹配链长度
  content: string;                         // = A 侧块文本 [start, end) 切片
  match_type: string;                      // 标题 | 正文 | 表格
}

// 块树节点：段落（style: heading|body|list，align 已映射 CSS 值）
interface CompareParaNode { kind:"paragraph"; block; style; outline; number_prefix;
                          font_halfpt; bold; align; chapter; text }
// 表格节点（cells 携带合并信息；表格节点自身也占一个 block id）
interface CompareTableNode { kind:"table"; block; rows; cols;
                          cells: { row; col; rowSpan; colSpan; isHeader; block; chapter; text }[] }
```

不变量（有单测保证）：`docs.*.tree` 中任意 `block` 的文本在 `[start,end)` 上的切片 === `content`。

## 7. Review 关注点与已知边界

| 项 | 说明 |
|---|---|
| 同源一致性 | 片段坐标与块树同源于 `build_block_tree`，无第二解析器；`test_run_compare_offsets_align_with_block_text` 断言切片一致。 |
| result 单行体积 | docs 块树 + 片段可能数 MB，JSONL 单行读取 + 单次 IPC 回传；超大标书（上千页）如遇卡顿，优先考虑压缩/分片，而非改协议。 |
| 直通 JSON | Rust 侧 `docs`/`stats` 为 `serde_json::Value` 直通（不做强类型），新增 Python 字段时前端 `types.ts` 同步即可，Rust 无需改动。 |
| 取消语义 | 错误消息含"取消"即映射 `AppError::Cancelled`（约定式匹配，与 docx_split 同模式）；前端以 `code === "cancelled"` 区分。 |
| 未覆盖的渲染保真 | 样式链级 w:jc（如 Heading 样式居中）、run 级混排样式、图片占位、页眉页脚均未渲染——块树只保段落/单元格级元数据，属阶段三增强项。 |
| 配置化未做 | 阈值（RECALL_MIN/JACCARD_MIN/LEV_MIN 等）、停用词为 engine.py 内置常量，阶段三并入设置页。 |
| 短重复漏报 | ≥8 字才成片段；招标文件常见的短关键参数重复（如"10毫秒"）不会单独成片段，但会作为长片段一部分被覆盖。 |

## 8. 验证记录

- Python：`pytest python/docx_split/tests` — 9 个 compare 单测全过（全套 153 passed；1 failed 为预置 e2e 缺真实样本文件，与本次无关）。
- CLI 冒烟：内置运行时 `python -X utf8 -I -m doc_analyze.cli compare A B --json` 输出完整 JSONL 协议。
- Rust：`cargo check` 通过（仅预置 dead_code warning）、`cargo test` 34 passed。
- 前端：`tsc` 与 `vite build` 通过。
- 应用级实测：`pnpm tauri dev` 启动正常，工具页待人工用真实标书验证。
