# 将 Tauri 打包器所需的 WiX / NSIS / nsis_tauri_utils 工具经国内镜像下载并放置到
# %LOCALAPPDATA%\tauri\，避免 pnpm tauri build 首次打包时从 GitHub 直连下载超时
# （错误通常表现为 failed to bundle project: `timeout: global`）。
#
# 说明：下载 URL 与校验值取自 tauri-cli 2.11.5 二进制内置常量；Tauri CLI 启动时
# 检测到目标目录存在即跳过下载。若升级 CLI 后仍提示下载，请从其错误日志中取新
# URL 并更新下方 $artifacts。
#
# 用法: powershell -ExecutionPolicy Bypass -File scripts/setup-tauri-bundler.ps1
# 可选参数: -Mirror <加速前缀>，默认 https://ghfast.top（直连 GitHub 失败时自动回退直连）
param(
  [string]$Mirror = "https://ghfast.top"
)

$ErrorActionPreference = "Stop"
$tauriDir = Join-Path $env:LOCALAPPDATA "tauri"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "tauri-bundler-setup"
New-Item -ItemType Directory -Force $tmp | Out-Null

function Get-Artifact {
  param([string]$Url, [string]$Sha1, [string]$Sha256, [string]$OutName)
  $file = Join-Path $tmp $OutName
  foreach ($attempt in 1..2) {
    if (Test-Path $file) {
      $ok = $true
      if ($Sha1 -and (Get-FileHash $file -Algorithm SHA1).Hash -ne $Sha1) { $ok = $false }
      if ($Sha256 -and (Get-FileHash $file -Algorithm SHA256).Hash -ne $Sha256) { $ok = $false }
      if ($ok) { return $file }
      Remove-Item $file -Force
    }
    $lastError = ""
    foreach ($u in @("$Mirror/$Url", $Url)) {
      Write-Host "下载: $u"
      curl.exe -L --max-time 300 -sS -o $file $u
      if ($LASTEXITCODE -eq 0 -and (Test-Path $file) -and (Get-Item $file).Length -gt 0) { $lastError = ""; break }
      $lastError = "exit=$LASTEXITCODE"
    }
    if ($lastError) { throw "下载失败($lastError): $Url" }
  }
  throw "SHA 校验失败: $file"
}

# 1) WiX 3.14.1（MSI 打包用，candle.exe / light.exe）
$wixDir = Join-Path $tauriDir "WixTools314"
if (Test-Path (Join-Path $wixDir "candle.exe")) {
  Write-Host "WiX 已就绪，跳过"
} else {
  $zip = Get-Artifact `
    -Url "https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip" `
    -Sha256 "6ac824e1642d6f7277d0ed7ea09411a508f6116ba6fae0aa5f2c7daa2ff43d31" `
    -OutName "wix314-binaries.zip"
  New-Item -ItemType Directory -Force $wixDir | Out-Null
  Expand-Archive -Force $zip $wixDir
  Write-Host "WiX 已安装到 $wixDir"
}

# 2) NSIS 3.11（NSIS 安装包用，makensis.exe）
$nsisDir = Join-Path $tauriDir "NSIS"
if (Test-Path (Join-Path $nsisDir "makensis.exe")) {
  Write-Host "NSIS 已就绪，跳过"
} else {
  $zip = Get-Artifact `
    -Url "https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip" `
    -Sha1 "EF7FF767E5CBD9EDD22ADD3A32C9B8F4500BB10D" `
    -OutName "nsis-3.11.zip"
  $stage = Join-Path $tmp "nsis-extract"
  if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
  Expand-Archive -Force $zip $stage
  New-Item -ItemType Directory -Force $nsisDir | Out-Null
  # zip 内层可能有一层 nsis-3.11 目录，统一展平到 NSIS 根目录
  if (Test-Path (Join-Path $stage "nsis-3.11")) { Copy-Item (Join-Path $stage "nsis-3.11\*") $nsisDir -Recurse -Force }
  else { Copy-Item (Join-Path $stage "*") $nsisDir -Recurse -Force }
  # 清理 tauri build 下载中断留下的半成品目录
  Remove-Item (Join-Path $nsisDir "nsis-3.11") -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item $stage -Recurse -Force
  Write-Host "NSIS 已安装到 $nsisDir"
}

# 3) nsis_tauri_utils.dll（NSIS 打包脚本依赖的 Tauri 插件）
$dllPath = Join-Path $nsisDir "Plugins\x86-unicode\nsis_tauri_utils.dll"
if (Test-Path $dllPath) {
  Write-Host "nsis_tauri_utils.dll 已就绪，跳过"
} else {
  $dll = Get-Artifact `
    -Url "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll" `
    -Sha1 "75197FEE3C6A814FE035788D1C34EAD39349B860" `
    -OutName "nsis_tauri_utils.dll"
  New-Item -ItemType Directory -Force (Split-Path $dllPath) | Out-Null
  Copy-Item $dll $dllPath -Force
  Write-Host "nsis_tauri_utils.dll 已安装"
}

Write-Host "`n全部就绪，可执行 pnpm tauri build"
