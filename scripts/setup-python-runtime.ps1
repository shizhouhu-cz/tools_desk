# 将 Python embeddable 运行时 + docx_split 依赖装配到 src-tauri/resources/python/，
# 供 Tauri 以 resources 打包后冷启动子进程调用（python.exe -X utf8 -I -m doc_analyze.cli）。
#
# 用法: powershell -ExecutionPolicy Bypass -File scripts/setup-python-runtime.ps1
# 可选参数:
#   -Version     embeddable 版本，默认 3.14.7（与开发 venv 同 minor，cp314 wheel 兼容）
#   -VenvPython  开发 venv 的 python.exe（仅用其 pip 以 --target 安装依赖，不污染 venv）
#   -Mirror      embeddable zip 镜像前缀，默认 npmmirror，失败自动回退 python.org
#
# 幂等：python.exe + doc_analyze/cli.py + python314._pth + .runtime-ok 四者齐全即跳过；
# 删除 src-tauri/resources/python/ 可强制重建。
param(
  [string]$Version = "3.14.7",
  [string]$VenvPython = "D:\Code\doc_analyze\.venv\Scripts\python.exe",
  [string]$Mirror = "https://registry.npmmirror.com/-/binary/python"
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path $PSScriptRoot -Parent
$target = Join-Path $repoRoot "src-tauri\resources\python"
$pkgSrc = Join-Path $repoRoot "python\docx_split\src\doc_analyze"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "python-runtime-setup"
New-Item -ItemType Directory -Force $tmp | Out-Null

if (-not (Test-Path $VenvPython)) { throw "未找到开发 venv python：$VenvPython" }
if (-not (Test-Path (Join-Path $pkgSrc "cli.py"))) { throw "未找到源包：$pkgSrc" }

$majorminor = "$($Version.Split('.')[0])$($Version.Split('.')[1])"
$pyExe = Join-Path $target "python.exe"
$pthFile = Join-Path $target "python$majorminor._pth"
$marker = Join-Path $target ".runtime-ok"

# 1) 幂等检查
if ((Test-Path $pyExe) -and (Test-Path (Join-Path $target "Lib\site-packages\doc_analyze\cli.py")) -and (Test-Path $pthFile) -and (Test-Path $marker)) {
  Write-Host "Python 运行时已就绪，跳过（删除 $target 可强制重建）"
  exit 0
}

# 2) 下载 embeddable zip（镜像优先，回退 python.org）
$zipName = "python-$Version-embed-amd64.zip"
$zip = Join-Path $tmp $zipName
if (-not ((Test-Path $zip) -and (Get-Item $zip).Length -gt 1MB)) {
  $lastError = ""
  foreach ($u in @("$Mirror/$Version/$zipName", "https://www.python.org/ftp/python/$Version/$zipName")) {
    Write-Host "下载: $u"
    curl.exe -L --max-time 300 -sS -o $zip $u
    if ($LASTEXITCODE -eq 0 -and (Test-Path $zip) -and (Get-Item $zip).Length -gt 1MB) { $lastError = ""; break }
    $lastError = "exit=$LASTEXITCODE"
  }
  if ($lastError) { throw "下载失败($lastError): $zipName" }
}

# 3) 解压到目标目录（整体重建，避免残留半成品）
if (Test-Path $target) { Remove-Item $target -Recurse -Force }
New-Item -ItemType Directory -Force $target | Out-Null
Expand-Archive -Force $zip $target
Write-Host "embeddable 已解压到 $target"

# 4) 覆写 _pth：启用 Lib/site-packages（隔离模式即所需行为，spawn 时另加 -I 双保险）
@"
python$majorminor.zip
.
Lib/site-packages
import site
"@ | Set-Content -Encoding ascii $pthFile

# 5) 用开发 venv 的 pip 以 --target 安装依赖（--only-binary=rapidfuzz：rapidfuzz 须为 cp314 wheel，
#    jieba 仅 sdist 无 wheel（纯 Python 无编译风险），其余依赖 PyPI 上均为 wheel）
$sitePackages = Join-Path $target "Lib\site-packages"
& $VenvPython -m pip install --only-binary=rapidfuzz --upgrade --target $sitePackages `
  python-docx lxml pydantic typer httpx jieba rapidfuzz `
  -i https://pypi.tuna.tsinghua.edu.cn/simple
if ($LASTEXITCODE -ne 0) { throw "pip 安装依赖失败" }

# 6) 拷贝 doc_analyze 包（先删后拷）
$spDoc = Join-Path $sitePackages "doc_analyze"
if (Test-Path $spDoc) { Remove-Item $spDoc -Recurse -Force }
Copy-Item $pkgSrc $spDoc -Recurse -Force

# 7) 清理：__pycache__ / *.pyc / *.dist-info
Get-ChildItem $target -Recurse -Force -Directory -Filter "__pycache__" | Remove-Item -Recurse -Force
Get-ChildItem $target -Recurse -Force -File -Filter "*.pyc" | Remove-Item -Force
Get-ChildItem $target -Recurse -Force -Directory -Filter "*.dist-info" | Remove-Item -Recurse -Force

# 8) 冒烟：依赖可导入 → python-docx 生成临时 docx → extract --json 校验 result 行
& $pyExe -c "import docx, lxml, pydantic, typer, httpx, jieba, rapidfuzz; print('imports-ok')"
if ($LASTEXITCODE -ne 0) { throw "依赖导入冒烟失败" }

$smokeDocx = Join-Path $tmp "smoke.docx"
$smokeDb = Join-Path $tmp "smoke.db"
$smokeOut = Join-Path $tmp "smoke-out.txt"
$smokeErr = Join-Path $tmp "smoke-err.txt"
$smokePy = Join-Path $tmp "smoke-gen.py"
Remove-Item $smokeDb -ErrorAction SilentlyContinue
@"
from docx import Document
d = Document()
d.add_heading('项目概况', level=1)
d.add_paragraph('本项目为示例招标文件，用于运行时冒烟验证。')
d.add_heading('技术要求', level=2)
d.add_paragraph('提供稳定服务。')
t = d.add_table(rows=2, cols=2)
t.cell(0, 0).text = 'A1'
t.cell(0, 1).text = 'B1'
t.cell(1, 0).text = 'A2'
t.cell(1, 1).text = 'B2'
d.add_heading('商务条款', level=1)
d.add_paragraph('付款方式按里程碑。')
d.save(r'$smokeDocx')
"@ | Set-Content -Encoding ascii $smokePy
& $pyExe $smokePy
if ($LASTEXITCODE -ne 0) { throw "生成冒烟 docx 失败" }

& $pyExe -X utf8 -m doc_analyze.cli extract $smokeDocx --db $smokeDb --json 1>$smokeOut 2>$smokeErr
if ($LASTEXITCODE -ne 0) {
  Get-Content $smokeErr | ForEach-Object { Write-Host "  $_" }
  throw "extract 冒烟失败 exit=$LASTEXITCODE"
}
$result = Select-String -Path $smokeOut -Pattern '"type": "result"' | Select-Object -First 1
if (-not $result) { throw "extract 输出缺少 result 行" }
if ($result.Line -notmatch '"ok": true') { throw "extract result 行 ok!=true" }
if ($result.Line -notmatch '"chapters": [1-9]') { throw "extract result 行 chapters<1" }

# 9) 写 marker 收尾
Set-Content -Path $marker -Value "python $Version runtime ok $(Get-Date -Format s)"
$sizeMb = [math]::Round((Get-ChildItem $target -Recurse -Force | Measure-Object Length -Sum).Sum / 1MB, 1)
Write-Host "`nPython 运行时就绪：$target（$sizeMb MB），可执行 pnpm tauri dev / build"
