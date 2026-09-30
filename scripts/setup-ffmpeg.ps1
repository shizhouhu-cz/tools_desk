# 将 ffmpeg / ffprobe 静态构建（BtbN/FFmpeg-Builds win64 gpl 版）经国内镜像下载并解压到
# src-tauri/resources/ffmpeg/，供"视频转精灵图"工具使用（打包时随 resources 进入安装目录）。
#
# 说明：BtbN 的 autobuild 资产会随每次构建更新，SHA 固定值不可维护，因此本脚本
# 通过"解压后实际运行 ffprobe -version"验证产物可用性；如需严格校验可传 -Sha256。
#
# 用法: pnpm setup:ffmpeg
# 可选参数: -Mirror <加速前缀>（默认 https://ghfast.top，失败自动回退直连）
#           -Version <分支名>（默认 master；release 分支如 n8.0 资产名规则不同）
#           -Sha256 <哈希>（可选，严格校验下载的 zip）
param(
  [string]$Mirror = "https://ghfast.top",
  [string]$Version = "master",
  [string]$Sha256 = ""
)

$ErrorActionPreference = "Stop"
$destDir = Join-Path $PSScriptRoot "..\src-tauri\resources\ffmpeg"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "ffmpeg-setup"
# master 分支资产名无版本后缀；release 分支（如 n8.0）资产名带 "-8.0" 后缀
$assetName = if ($Version -eq "master") {
  "ffmpeg-master-latest-win64-gpl.zip"
} else {
  "ffmpeg-$Version-latest-win64-gpl-$($Version.TrimStart('n')).zip"
}
$assetUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$assetName"

# 0) 已就绪则跳过
if ((Test-Path (Join-Path $destDir "ffmpeg.exe")) -and (Test-Path (Join-Path $destDir "ffprobe.exe"))) {
  Write-Host "ffmpeg / ffprobe 已就绪，跳过（$destDir）"
  exit 0
}

New-Item -ItemType Directory -Force $destDir, $tmp | Out-Null
$zip = Join-Path $tmp $assetName

# 1) 下载（镜像前缀 + 直连依次尝试）
$lastError = ""
foreach ($u in @("$Mirror/$assetUrl", $assetUrl)) {
  Write-Host "下载: $u"
  curl.exe -L --max-time 900 --retry 2 -sS -o $zip $u
  if ($LASTEXITCODE -eq 0 -and (Test-Path $zip) -and (Get-Item $zip).Length -gt 10MB) { $lastError = ""; break }
  $lastError = "exit=$LASTEXITCODE"
  Remove-Item $zip -Force -ErrorAction SilentlyContinue
}
if ($lastError) { throw "下载失败($lastError): $assetUrl" }

# 2) 可选严格校验
if ($Sha256 -and (Get-FileHash $zip -Algorithm SHA256).Hash -ne $Sha256) {
  throw "SHA256 校验失败: $zip"
}

# 3) 解压（优先 tar，速度快；失败回退 Expand-Archive）
$stage = Join-Path $tmp "extract"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null
Write-Host "解压中（约 100-200 MB，请稍候）…"
tar -xf $zip -C $stage 2>$null
if ($LASTEXITCODE -ne 0) { Expand-Archive -Force $zip $stage }

# 4) 定位 bin/ffmpeg.exe 与 bin/ffprobe.exe（zip 内有一层版本目录）
$ffmpegSrc = Get-ChildItem $stage -Recurse -Filter "ffmpeg.exe" | Where-Object { $_.DirectoryName -like "*\bin" } | Select-Object -First 1
$ffprobeSrc = Get-ChildItem $stage -Recurse -Filter "ffprobe.exe" | Where-Object { $_.DirectoryName -like "*\bin" } | Select-Object -First 1
if (-not $ffmpegSrc -or -not $ffprobeSrc) { throw "解压内容中未找到 ffmpeg.exe / ffprobe.exe" }
Copy-Item $ffmpegSrc.FullName (Join-Path $destDir "ffmpeg.exe") -Force
Copy-Item $ffprobeSrc.FullName (Join-Path $destDir "ffprobe.exe") -Force

# 5) 实际运行验证
$ver = & (Join-Path $destDir "ffprobe.exe") -version 2>&1 | Select-Object -First 1
if ($ver -notmatch "ffprobe version") { throw "ffprobe 运行验证失败: $ver" }

Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Write-Host "`nffmpeg 已就绪：$destDir"
Write-Host $ver
