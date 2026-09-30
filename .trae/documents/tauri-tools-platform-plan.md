# Tauri 工具集平台实施方案

## Context（背景）

在空目录 `d:\Code\tools` 从零搭建一个 Tauri 2 桌面客户端，定位为**可增量扩展的工具集平台**：前端 React + TypeScript + Vite + **shadcn/ui + Tailwind**，后端 Rust。首批实现 4 个工具，其中工具 2/3/4 从成熟参考项目 `D:\Code\pptx_processor`（Rust webserver）移植核心逻辑，去掉 HTTP/multipart/ZIP 层，改为本地路径直进直出。

已确认的决策：
- UI 组件库：shadcn/ui + Tailwind
- 工具1：递归收集所有层级文件（不提供"仅顶层"开关），跨子目录同名文件自动加后缀

## 一、总体架构

```
┌─ React 前端 ─────────────────────────────┐
│ 工具注册表 registry.ts → 路由/侧边栏       │
│ 每个工具一个页面（参数表单+进度+结果）      │
│ invoke 调 command / listen 收进度事件      │
└──────────────┬───────────────────────────┘
               │ IPC
┌─ Rust 后端 ──┴───────────────────────────┐
│ commands/ 薄封装（校验/发进度/聚合结果）    │
│ tools/ 纯 Rust 核心逻辑（不依赖 tauri，可单测）│
│ 基础设施：error / config / task / log      │
└──────────────────────────────────────────┘
```

**新增工具的增量成本 = `tools/` 加一个纯 Rust 模块 + `commands/` 加一个薄封装 + 前端加一个页面 + registry 加一行。**

## 二、项目结构

```
d:\Code\tools
├── src/                          # 前端
│   ├── main.tsx / App.tsx
│   ├── routes.tsx                # 遍历 registry 生成路由
│   ├── lib/tauri.ts              # invoke/listen 薄封装
│   ├── lib/task.ts               # useTask(taskId): {progress, cancel} hook
│   ├── components/ui/            # shadcn 生成（button/card/input/progress/select/checkbox/label/sonner）
│   ├── components/layout/        # Sidebar、ToolShell（统一参数表单+进度条+结果区骨架）
│   └── tools/
│       ├── registry.ts           # ToolMeta[] { id, title, icon, element }
│       ├── file-collector/index.tsx
│       ├── pptx-to-pdf/index.tsx
│       ├── pdf-to-images/index.tsx
│       └── watermark/index.tsx
├── src-tauri/
│   ├── tauri.conf.json           # bundle.resources: ["assets/lib/pdfium.dll"]
│   ├── capabilities/default.json # dialog:allow-open、opener、core:event:default
│   ├── assets/lib/pdfium.dll     # 从 D:\Code\pptx_processor\assets\lib\pdfium.dll 复制
│   └── src/
│       ├── main.rs / lib.rs      # Builder、插件注册、State
│       ├── error.rs              # AppError(thiserror)，Serialize 为 {code,message}
│       ├── config.rs             # settings.json 读写（app_config_dir）
│       ├── task.rs               # TaskManager: taskId → (Arc<AtomicBool> 取消标志)
│       ├── commands/             # mod.rs + 4 个命令文件（薄封装）
│       └── tools/                # 纯 Rust 核心逻辑，不依赖 tauri::
│           ├── file_collector.rs / pptx_to_pdf.rs / pdf_to_images.rs / watermark.rs
└── package.json                  # pnpm 管理
```

### 脚手架初始化
1. `pnpm create tauri-app@latest . --template react-ts --manager pnpm --yes`（Tauri 2.x，模板自带 Vite + TS）
2. 按 shadcn 官方 Vite 指南：`pnpm dlx shadcn@latest init`（Tailwind v4，`@tailwindcss/vite` 进 vite.config.ts，tsconfig 加 paths）
3. npm 依赖：`@tauri-apps/api`、`@tauri-apps/plugin-dialog`、`@tauri-apps/plugin-log`、`@tauri-apps/plugin-opener`、`react-router-dom`
4. Rust crate：`tauri-plugin-dialog`、`tauri-plugin-log`、`tauri-plugin-opener`（均为 2.x 线）；**不引入 plugin-fs**（文件 IO 全在 Rust command 内做，不受 fs scope 限制）；**不用 plugin-store**（设置主要被 Rust 消费，Rust 侧自写 JSON 更简单）
5. 核心移植依赖：`pdfium-render 0.8 (sync)`、`image 0.25`、`ab_glyph 0.2`、`libwebp 0.1`、`walkdir`、`tokio`、`thiserror`、`tracing`、`serde`

## 三、Tauri 2 关键机制

| 机制 | 方案 |
|---|---|
| **进度上报** | 单一事件 `task://progress`，payload `{taskId, tool, stage: scan\|copy\|convert\|render\|done, current, total, message?}`。Rust 侧 `ProgressReporter` 按每 64 个文件或每 100ms 节流 emit；前端 `useTask` hook 按 taskId 过滤订阅 |
| **取消** | `TaskManager` 存 `HashMap<String, Arc<AtomicBool>>`；worker 在每文件/每页复制前检查，取消即清理半成品返回 `Cancelled`。soffice 子进程用 `tokio::select! { child.wait(), cancel ⇒ child.kill() }` |
| **目录选择** | 前端 JS 侧 `@tauri-apps/plugin-dialog` 的 `open({directory:true})`，路径作为表单参数传入 command（capability 只开 `dialog:allow-open`） |
| **pdfium.dll 打包** | `tauri.conf.json` → `bundle.resources: ["assets/lib/pdfium.dll"]`；运行时统一 `app.path().resolve("assets/lib/pdfium.dll", BaseDirectory::Resource)`（dev 解析到 src-tauri/，release 到 $RESOURCE/，写法一致）；`cargo test` 环境回退 `CARGO_MANIFEST_DIR`；加载失败错误信息带绝对路径 |
| **设置持久化** | `config.rs` 读写 `app_config_dir()/settings.json`：`{soffice_path, dpi:150, quality:85, max_pages:200, timeout_sec, concurrency}`；提供 `get_settings`/`set_settings` command + 设置页 |
| **日志** | `tauri-plugin-log` → Stdout + LogDir + Webview；设置页展示 log_dir 便于排障 |

## 四、四个工具实现要点

### 工具1 文件收集器（全新实现）
- command：`collect_files(source_dir: String) -> CollectSummary {dest_dir, files, bytes, renamed}`
- 流程：walkdir 递归统计全部文件（得 total）→ 在源目录下建临时目录 `_collected_20260921_153000`（遍历排除自身防自拷贝）→ 逐文件 `fs::copy`，字节级累计进度；跨子目录重名 → `name(1).ext` 递增去重；取消时删除整个目标目录
- 单测：嵌套目录、跨子目录同名、中文文件名、取消中断

### 工具2 PPTX→PDF（移植 converter.rs）
- 移植源：`D:\Code\pptx_processor\src\services\pptx_to_pdf\converter.rs`（约 450 行，质量高，**逐段保留**：soffice.com 优先解析、独立 `-env:UserInstallation` 临时 profile、预写 registrymodifications.xcu 抑制打印机弹窗、CREATE_NO_WINDOW、timeout 杀进程、stderr 缺字体解析、`--version` 健康探测、Semaphore 并发）
- 改造：入口 `convert(pptx_data:&[u8])` → `convert_file(input:&Path, out_dir:&Path) -> ConvertOutcome{pdf_path, missing_fonts}`，去掉 bytes 往返与临时目录搬运
- command：`pptx_to_pdf(inputs: Vec<String>, out_dir: Option<String>)`，逐文件 emit 进度，返回 `Vec<FileOutcome>` 聚合（单文件失败不中断批次）
- 冲突策略：目标同名写 `名(1).pdf` 并在结果中标注；设置页提供 soffice 路径探测按钮

### 工具3 PDF→图片（移植 renderer.rs）
- 移植源：`D:\Code\pptx_processor\src\services\pdf_to_images\renderer.rs`
- 保留：`Pdfium::bind_to_library` 加载、BGRA+白底、scale=dpi/72、DPI 限 72–600、页码 1-based 闭区间、max_pages=200、顺序渲染（pdfium 借用限制）
- 改造：`render() -> Vec<(page, bytes)>` → 回调式 `render_to(pdf, params, |page, bytes| 写盘)` 流式落盘，避免 200 页大图全驻内存；JPEG/WebP 编码放 `spawn_blocking`
- command：`pdf_to_images(path, dpi?, format: png|jpeg|webp, quality?, start?, end?)`
- 输出：子目录 `{父目录}/{文件名}_pages/`，文件 `{stem}_p{:03}.{ext}`，返回 `{out_dir, pages, files}`

### 工具4 图片打水印（移植 engine.rs）
- 移植源：`D:\Code\pptx_processor\src\services\watermark\engine.rs`（Text/Image/Tile 三模式、ab_glyph 光栅化、字体查找链 simhei.ttf→msyh.ttc、`parse_color`、白底 flatten）
- 改造：`apply(&[u8])` → `apply_file(&Path, out_path, params, cancel, cb)`，去掉 multipart
- command：`watermark_images(inputs: Vec<String>, params, output_mode: "suffix"|"subdir")`
- 输出：默认同目录加后缀 `{stem}_wm.{ext}`（**保留原格式**：PNG 输出 PNG 保 alpha，JPEG/WebP 白底 flatten），可选输出到 `{dir}/watermarked/`
- 批量：Semaphore(2–4) 并行（图片间无 pdfium 借用问题），进度按文件计数

## 五、相对参考实现的优化清单

**桌面化改造**：bytes→路径流式处理；进度/取消贯穿所有工具；"打开输出目录"按钮（plugin-opener）；设置页 soffice 探测；拖拽输入（P1 可选）；水印实时预览（P2，用 0.25 倍缩放图复用 apply 逻辑生成，不为预览复刻 canvas 绘制）。

**代码质量**：
1. 水印旋转最近邻 → 双线性采样（改 engine.rs 的 rotate_image，消锯齿）
2. 批量水印并行化（参考实现单请求串行）
3. AppError 用 thiserror 统一 + 可 Serialize，前端按 code 出 toast
4. 图片解码尺寸上限防 OOM
5. 进度节流避免事件风暴
6. 保留 soffice 队列超时与"服务繁忙"提示

## 六、实施里程碑

| 阶段 | 内容 | 验证 |
|---|---|---|
| M0 脚手架 | create-tauri-app + shadcn + 侧边栏三页导航骨架 | `pnpm tauri dev` 窗口可用 |
| M1 基础设施 | error/task/config/log + useTask hook + 设置页 | mock command 推进度、取消生效、日志落盘 |
| M2 工具1 | file_collector + 单测 | `cargo test` + 实测中文/深嵌套路径 |
| M3 工具2 | 移植 converter + soffice 探测 | dev 内真实 pptx 转换、超时/缺字体提示 |
| M4 工具3 | pdfium 集成 + resources 打包 | dev 与 release 两种路径均加载 dll 成功 |
| M5 工具4 | 移植 engine + 批量并行 | 中文字体、平铺、旋转效果比对参考服务 |
| M6 打包 | NSIS + WebView2 bootstrapper | 安装包在无开发环境机器跑通 4 工具 |

## 七、风险与注意事项

- **杀毒/Defender**：soffice.com 无头调用、批量复制可能被实时扫描拖慢，文档注明白名单建议；未签名安装包首次运行触发 SmartScreen
- **路径**：中文/空格路径全链路测试；长路径 >260 字符失败时错误信息提示，必要时目标加 `\\?\` 前缀
- **pdfium**：pdfium-render 0.8 需与随附 dll 版本匹配，锁定复制参考项目那份 pdfium.dll；加载失败要有明确弹窗
- **CSP**：dev 模式放行 Vite HMR 的 `ws://localhost:*` 与 `ipc: http://ipc.localhost`；生产保持最小 CSP
- **体积**：pdfium.dll 约 7MB，安装包预计 15–25MB；release profile 沿用参考项目 `lto=true, strip=true`
- **写入位置**：输出永远写用户显式选择的目录，避免 Program Files 写入失败
