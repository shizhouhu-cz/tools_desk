# DOCX 结构化拆分集成计划（方案 B：裁剪版 CLI + embeddable Python）

## Context

把 D:\Code\doc_analyze（Python 项目：DOCX 标书结构化提取，pipeline → SQLite+FTS5）集成到 tools 工具箱作为第 5 个工具。既定决策：

- 砍 server/Agent/LangGraph 全系；保留拆分流水线 + 可选 LLM 结构恢复（httpx 裸调 OpenAI 兼容接口）
- Python embeddable 3.14（与开发 venv 同 minor，避免 ABI 问题）打进 Tauri resources，冷启动子进程方式调用，用完即销毁
- 原项目 doc_analyze 不动；在 tools 下建 `python/docx_split/` 分叉迭代
- LLM 结构恢复为可选开关；关闭时规则模式建树（快速离线）
- 预期体积：安装包 +15~20MB

## 架构

```
前端 docx-structure 页面 → invoke("docx_structure", {inputs, llmEnabled})
  → Rust 串行循环，每文件 spawn_blocking 冷启动:
     resources/python/python.exe -X utf8 -I -m doc_analyze.cli extract <docx> --db <db> --json
  → stdout JSONL（progress/result/error 行）→ Rust 逐行解析 → task:progress 事件
  → SQLite（app_data_dir/docx_split/docx_split.db）
章节树查看: invoke("docx_tree", {docId}) → rusqlite 直查 chapters 表（无需冷启动 python）
取消: TaskManager 扩展 → kill 子进程（stdout EOF → 循环退出 → wait 收尸）
```

## 1. python/docx_split/（从 doc_analyze 裁剪复制）

包名保留 `doc_analyze`（避免 sed 风险）。复制原样：`__init__.py`、`db.py`（connect 加 `busy_timeout=5000`）、`llm/recovery.py`、`llm/__init__.py`、`pipeline/{__init__,features,numbering,treebuild,tables}.py`、tests（features/numbering/tables/treebuild/recovery/e2e；不复制 test_agent）。
不复制：`server.py`、`agent/`、`config.toml`、`design.md`。

改造 4 个文件：

- **config.py**：删 `_ROOT`/tomllib/config.toml 读取（打包后 `parents[2]` 失效）；Settings 精简为 db_path + LLM 5 字段（provider/api_key/base_url/auth_header/recovery_model），全部读 env（`DOC_ANALYZE_*`/`OPENAI_*`）；`load_settings(db_path: Option)` 显式传参
- **cli.py**：删 `query`/`serve`；保留 `ingest`/`tree`/`stats`；新增 `extract` 命令：`extract <path> --db <db> --json`，JSONL stdout 协议：
  - progress 行 `{"type":"progress","stage":"parse|llm|store","current":n,"total":m,"message":"…"}`
  - result 行 `{"type":"result","ok":true,"doc_id":1,"input":"…","stats":{chapters,tables,needs_review,total_paragraphs,elapsed_seconds,…}}`
  - error 行 `{"type":"error","message":"…"}`（末行）；退出码 0 成功/1 失败/3 依赖缺失
  - stderr 全部诊断；stdout 只允许协议行
- **llm/provider.py**：删 MockChatModel/as_chat_model/langchain import；`OpenAICompatProvider.complete_json` 重写为 httpx 裸调 `{base_url}/chat/completions`（temperature=0、重试 2 次、自定义 Authorization 头支持、不传 response_format——与原行为一致）；`MockProvider`/`_extract_json`/`get_provider` 原样
- **pipeline/ingest.py**：`ingest_file` 加可选 `progress: Callable[[str,int,int,str|None],None]` 参数（默认 None），3 个上报点：解析前/后、LLM 分段循环、入库前

**pyproject.toml** 新写：依赖仅 python-docx/lxml/typer/pydantic/httpx。

## 2. scripts/setup-python-runtime.ps1（仿 setup-tauri-bundler.ps1 风格）

目标 `src-tauri/resources/python/`；package.json 加 `setup:python`；.gitignore 加 `src-tauri/resources/python/`。

1. 幂等检查（python.exe + site-packages/doc_analyze/cli.py + `python314._pth` + marker `.runtime-ok`）
2. 下载 embeddable zip：默认 `-Version 3.14.7`，镜像 `registry.npmmirror.com/-/binary/python` → 回退 python.org/ftp
3. 覆写 `python314._pth`：`python314.zip` / `.` / `Lib/site-packages` / `import site`（隔离模式即所需行为）
4. 用开发 venv 的 python `pip install --target …\Lib\site-packages --only-binary=:all: python-docx lxml pydantic typer httpx -i <清华镜像>`（同版本原生 wheel，禁 --platform 交叉）
5. 拷贝 `python/docx_split/src/doc_analyze` → site-packages（先删后拷）
6. 清理：`__pycache__`、`*.pyc`、`*.dist-info`
7. 冒烟：`python -c "import docx,lxml,pydantic,typer,httpx"`；用 python-docx 生成临时 docx → 跑 `extract --json` → 校验 result 行 chapters>=1；通过后写 marker

**tauri.conf.json** resources 改 map 形式（dev 下 resolve 基于/src-tauri/，与脚本写入位置天然一致，一条代码覆盖 dev/prod）：

```json
"resources": {
  "assets/lib/pdfium.dll": "assets/lib/pdfium.dll",
  "resources/python/": "resources/python/"
}
```

## 3. Rust 侧

**task.rs 扩展**（对现有 4 工具零破坏）：

```rust
struct TaskEntry { cancel: Arc<AtomicBool>, procs: Arc<Mutex<Vec<std::process::Child>>> }
// TaskHandle 加 register_child(child) / kill_children()
// cancel(): 置标志 + 遍历 kill；worker 末尾从注册表取出 Child wait() 收尸
```

**tools/docx_split.rs**（python 运行器）：
- `resolve_python_path(app, configured)`：settings.docx_python_path > `resolve("resources/python/python.exe", Resource)` > CARGO_MANIFEST_DIR 兜底（仿 resolve_pdfium_path）
- `llm_env(cfg, llm_enabled)`：恒传 `PYTHONUTF8=1`/`PYTHONIOENCODING=utf-8`；llm_enabled 时传 `DOC_ANALYZE_LLM_PROVIDER`/`OPENAI_BASE_URL`/`OPENAI_API_KEY`/`DOC_ANALYZE_OPENAI_AUTH_HEADER`/`DOC_ANALYZE_RECOVERY_MODEL`（非空才传）
- `run_extract(handle, python_exe, db_path, input, env)`：spawn_blocking 内 `Command::new(python).args(["-X","utf8","-I","-m","doc_analyze.cli","extract",input,"--db",db,"--json"])` + `CREATE_NO_WINDOW(0x08000000)`；child 所有权入注册表（cancel 可杀）；stderr 独立线程收尾部 8KB；stdout BufReader 逐行 `serde_json` 解析（非 JSON 行静默跳过）→ progress 行转 `handle.progress_msg`（message 前缀文件名）→ result 行存 ExtractResult；`wait()` 后非成功取 error 行/stderr 尾部为错误

**commands/docx_structure.rs**：
- `docx_structure(app, tasks, settings, inputs, llm_enabled) -> Vec<DocxOutcome>`：仿 pptx_to_pdf 串行循环（LLM 分段长耗时 + SQLite 单写者，不并发）；单文件失败不中断；取消时 push"任务已取消"失败行并跳过剩余
- `DocxOutcome{input,ok,doc_id,chapters,tables,needs_review,total_paragraphs,error,duration_ms}`
- `docx_tree(doc_id) -> Vec<ChapterNode>`：**rusqlite 直查**（章节树高频交互，python 冷启动 200-500ms 不值得；Cargo.toml 加 `rusqlite = { version = "0.37", features = ["bundled"] }`，bundled 自带 FTS5）；`SELECT … FROM chapters WHERE doc_id=? ORDER BY order_num`，Rust 内按 parent_id 组树
- `docx_documents() -> Vec<DocxDocumentInfo>`：documents 表列表（历史文档回看）
- `resolve_db_path`：settings.docx_db_dir > `app_data_dir()/docx_split/docx_split.db`，create_dir_all

**注册**：commands/mod.rs + lib.rs invoke_handler 加 3 个命令。

## 4. 设置页（config.rs Settings 扩展，serde(default) 兼容旧文件）

`docx_python_path`（默认""=内置运行时）、`docx_db_dir`（""=appdata）、`docx_llm_provider`（"openai"）、`docx_llm_base_url`、`docx_llm_api_key`（明文存储，设置页 password 输入）、`docx_llm_auth_header`（内网网关裸 token 覆盖 Bearer）、`docx_llm_model`（"qwen-plus"）。
设置页新分区「DOCX 结构化拆分」Card；前端 Settings 类型同步 7 字段。

## 5. 前端

- **src/tools/docx-structure/index.tsx**：输入 Card（PathField multiple 选 docx + FileChips）→ 拆分参数 Card（Toggle 启用 LLM 结构恢复 + 当前 LLM 配置摘要/未配置黄字提示）→ RunBar → ProgressPanel → 自定义结果列表（成功行：Badge + 章节数·表格数·待复核数 + 耗时 + 「章节树」按钮；失败行：错误摘要；顶部汇总 + 「查看数据库」openDir）
- **chapter-tree.tsx**：Dialog + 懒加载 `docx_tree`，递归树渲染（缩进 + number_prefix + title，needs_review 标 `*` amber 色，行尾字符数），前端 filter 搜索框
- **历史文档 Card**：`docx_documents` 列表 + 每行章节树按钮
- **registry.tsx**：第 5 项 `{id:"docx-structure", title:"DOCX 结构化拆分", subtitle:"章节树 / 表格 / 全文索引", icon:FileSearch}`
- **task.ts** STAGE_LABELS 加 `parse:"解析文档"/llm:"AI 结构恢复"/store:"入库"/extract:"拆分中"`
- **types.ts**：DocxOutcome/ChapterNode/DocxDocumentInfo + Settings 7 字段

## 6. 实施顺序与验证

| 步骤 | 内容 | 验证 |
|---|---|---|
| 1 | 裁剪复制 + 4 文件改造 + pyproject | `PYTHONPATH=tools\python\docx_split\src`（勿 pip install -e，避免污染原 venv 同名包）跑 `python -m pytest python\docx_split\tests`；真实 docx 跑 extract --json 肉眼验 JSONL + mock 降级 |
| 2 | setup-python-runtime.ps1 + package.json + .gitignore | `pnpm setup:python` 冒烟通过；重跑幂等跳过 |
| 3 | task.rs 扩展 + docx_split.rs + docx_structure.rs + config/settings + 注册 + rusqlite | `cargo check` + `cargo test`（14 现有测试不回归） |
| 4 | tauri.conf.json resources map | tauri dev 下 resolve 命中；跑长文档点取消，确认 python.exe 进程消失 |
| 5 | 前端全套 | `pnpm build`（tsc 零错）；dev 全流程：多选 → mock 跑 → 进度/取消/结果/章节树/历史文档；配置真实网关验证 LLM 模式 |
| 6 | 打包 | `pnpm setup:bundler && pnpm tauri build`；安装包体积记录；安装后实测 |

## 7. 关键风险与对策

1. **_pth 隔离语义**：脚本每次写入并纳入幂等校验；spawn 加 `-I` 双保险；冒烟覆盖
2. **lxml/pydantic-core wheel**：与 venv 同 minor 3.14 + `--only-binary=:all:` 禁源码编译；冒烟 import 验证
3. **杀进程竞态**：Child 所有权入注册表 Mutex 串行化；SQLite WAL + busy_timeout=5000 保证被杀不留腐库
4. **cp936 打碎中文 JSON**：`-X utf8` + `PYTHONUTF8=1` 双保险
5. **stdout 协议污染**：诊断全走 stderr；Rust 对非 JSON 行静默跳过
6. **体积**：运行时+依赖解压约 35-40MB，NSIS LZMA 后约 +15-18MB
7. **api_key 明文**：本机工具场景接受，password 输入框
