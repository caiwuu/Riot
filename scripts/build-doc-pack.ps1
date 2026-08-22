<#
.SYNOPSIS
制作 Riot 文档能力包（Windows x64）。

.DESCRIPTION
和 scripts/build-doc-pack.mjs 产出同构的包，只是二进制来自 Windows 版 Codex 运行时。
必须在 Windows 上跑：artifact-tool 带 skia.node 这样的原生绑定，是按平台编译的，
没法从 macOS 交叉产出。

与 macOS 版的三处实质差异，都是被 Windows 逼出来的：

1. shim 仍然是 bash 脚本。Riot 在 Windows 上用的是 Git for Windows 的 bash
   （见 crates/riot-tools/src/tools/bash.rs），所以模型敲的命令由 bash 执行，
   shim 照常好使。但 CreateProcess 不认无扩展名的脚本，于是——

2. Python 侧不走 shim。skill 脚本用 subprocess 起 soffice / pdftoppm，那条路
   必须是真 .exe。包里放一份 bin/override/native-executables.json 名字到真实
   exe 的映射，presentations 自带的 runtime_helpers.py 和改写过的 render_docx.py
   都从它解析。RUNTIME_BIN_DIR 因此指向 bin/override —— runtime_helpers.py 按
   `bin_dir.parent.parent/native` 定位原生根，这个嵌套深度是它要求的。

3. RUNTIME_NODE 指向真的 node.exe 而不是 shim，理由同上：presentations 的脚本
   会用它起子进程。

.PARAMETER Dependencies
Codex 主运行时的 dependencies 目录。不传就按常见位置探。

.PARAMETER Plugins
Codex 文档插件缓存目录。不传就按常见位置探。

.PARAMETER Out
能力包仓库。成品按平台落到它下面的 win-x64\。不传就取 Riot 仓库旁边的 riot-pkg。

.EXAMPLE
pwsh scripts/build-doc-pack.ps1
pwsh scripts/build-doc-pack.ps1 -StageOnly
pwsh scripts/build-doc-pack.ps1 -Out D:\code\riot-pkg
#>

#Requires -Version 5.1
[CmdletBinding()]
param(
  [string]$Dependencies,
  [string]$Plugins,
  [string]$Out,
  [string]$Version = '0.1.0',
  [switch]$StageOnly,
  [switch]$KeepStage
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$PackName = 'doc-runtime'
$Platform = 'win-x64'

# 能力包仓库。包体走它的 Releases（GitHub 单文件上限 100MB，包放不进去），
# 清单直接从仓库文件读。
$PkgRepoSlug = 'caiwuu/riot-pkg'
$ReleaseTag = "$PackName-v$Version"

$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$Dist = Join-Path $Root 'dist\doc-pack'
$Cache = Join-Path $Dist '.cache'

# 成品落到能力包仓库的 `<包名>\<平台>\`。包名在上层是因为那个仓库以后不止装文档
# 这一个能力 —— 按平台分在最外面的话，一个包的东西会散在各平台目录里。
# 铺出目录和下载缓存留在 dist\，那些是构建中间物，不该进包仓库。
if (-not $Out) { $Out = if ($env:RIOT_PKG_REPO) { $env:RIOT_PKG_REPO } else { Join-Path (Split-Path -Parent $Root) 'riot-pkg' } }
$OutDir = Join-Path (Join-Path $Out $PackName) $Platform

function Step($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }
function Log($m) { Write-Host $m }
function Fail($m) { Write-Host "`n错误: $m`n" -ForegroundColor Red; exit 1 }

if ([Environment]::OSVersion.Platform -ne 'Win32NT') {
  Fail '这个脚本只在 Windows 上产出 win-x64 包。macOS 请用 scripts/build-doc-pack.mjs。'
}

# —— 源：本机的 Codex 运行时 ————————————————————————————————

function Find-First([string[]]$candidates, [string]$what, [string]$hint) {
  foreach ($c in $candidates) {
    if ($c -and (Test-Path -LiteralPath $c)) { return (Resolve-Path -LiteralPath $c).Path }
  }
  Fail "找不到 $what。探过这些位置：`n  $($candidates -join "`n  ")`n$hint"
}

if (-not $Dependencies) {
  $Dependencies = Find-First @(
    (Join-Path $env:LOCALAPPDATA 'codex-runtimes\codex-primary-runtime\dependencies'),
    (Join-Path $env:USERPROFILE '.cache\codex-runtimes\codex-primary-runtime\dependencies'),
    (Join-Path $env:APPDATA 'codex-runtimes\codex-primary-runtime\dependencies')
  ) 'Codex 主运行时' '需要本机装过 Codex 并让它把主运行时下载完（装完随便用一次文档能力即可触发）。也可以用 -Dependencies 直接指路径。'
}
if (-not $Plugins) {
  $Plugins = Find-First @(
    (Join-Path $env:USERPROFILE '.codex\plugins\cache\openai-primary-runtime'),
    (Join-Path $env:LOCALAPPDATA 'codex\plugins\cache\openai-primary-runtime')
  ) 'Codex 文档插件缓存' '需要本机的 Codex 至少用过一次文档能力。也可以用 -Plugins 直接指路径。'
}

# 上游目录布局在不同 Codex 版本间挪过位置，所以一律按可执行文件名去搜，
# 而不是把路径写死。搜不到就报错，别产出一个跑不起来的包。
function Find-Exe([string]$root, [string]$name) {
  $hit = Get-ChildItem -LiteralPath $root -Filter $name -Recurse -File -ErrorAction SilentlyContinue |
    Sort-Object { $_.FullName.Length } | Select-Object -First 1
  if (-not $hit) { Fail "在 $root 下找不到 $name。Codex 运行时的目录结构可能变了。" }
  return $hit.FullName
}

# native\ 下每个组件各占一个顶层目录。给定其中某个 exe，回推出该组件的根。
function Get-ComponentRoot([string]$nativeRoot, [string]$exePath) {
  $target = $nativeRoot.TrimEnd('\')
  $cur = Split-Path -Parent $exePath
  while ($cur -and (Split-Path -Parent $cur).TrimEnd('\') -ine $target) {
    $parent = Split-Path -Parent $cur
    if (-not $parent -or $parent -eq $cur) { Fail "$exePath 不在 $nativeRoot 下面" }
    $cur = $parent
  }
  return $cur
}

# —— 铺出 ————————————————————————————————————————————————

$Stage = Join-Path $Dist "$PackName-$Version-$Platform"
if (Test-Path -LiteralPath $Stage) { Remove-Item -LiteralPath $Stage -Recurse -Force }
New-Item -ItemType Directory -Path $Stage -Force | Out-Null
New-Item -ItemType Directory -Path $Cache -Force | Out-Null

Log "制作 $PackName $Version ($Platform)"
Log "  源运行时: $Dependencies"
Log "  铺出:     $Stage"
Log "  成品:     $OutDir"

function Copy-Tree($src, $dest) {
  New-Item -ItemType Directory -Path (Split-Path -Parent $dest) -Force | Out-Null
  # robocopy 比 Copy-Item 快一个量级，而且不会被长路径卡住。
  # 退出码 < 8 都算成功（1 = 复制了文件，2 = 有额外文件，等等）。
  $null = robocopy $src $dest /E /NFL /NDL /NJH /NJS /NP /R:1 /W:1
  if ($LASTEXITCODE -ge 8) { Fail "复制失败 ($LASTEXITCODE): $src -> $dest" }
  $global:LASTEXITCODE = 0
}

function Size-Of($dir) {
  $sum = (Get-ChildItem -LiteralPath $dir -Recurse -File -ErrorAction SilentlyContinue |
    Measure-Object -Property Length -Sum).Sum
  if (-not $sum) { return 0 }
  return [int64]$sum
}
function Mb($bytes) { '{0:N0}MB' -f ($bytes / 1MB) }
function Report($name) { Log "  ${name}: $(Mb (Size-Of (Join-Path $Stage $name)))" }

# 1. Python ————————————————————————————————————————————————
Step 'Python'
Copy-Tree (Join-Path $Dependencies 'python') (Join-Path $Stage 'python')
# artifact_tool_v2 是 artifact-tool 的 Python 侧实现，Riot 走 MCP（JS 侧），用不上。
# pandas 没有任何 skill 脚本引用。两个加起来 200MB。
foreach ($drop in @('artifact_tool_v2', 'pandas')) {
  Get-ChildItem -LiteralPath (Join-Path $Stage 'python') -Filter "$drop*" -Recurse -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -eq $drop -or $_.Name -like "$drop-*" } |
    ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
}
Get-ChildItem -LiteralPath (Join-Path $Stage 'python') -Filter '__pycache__' -Recurse -Directory -ErrorAction SilentlyContinue |
  ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
$PythonExe = Find-Exe (Join-Path $Stage 'python') 'python.exe'
Report 'python'

# 2. Node 与 artifact-tool ——————————————————————————————————
Step 'Node 与 artifact-tool'
New-Item -ItemType Directory -Path (Join-Path $Stage 'node\node_modules\@oai') -Force | Out-Null
Copy-Item -LiteralPath (Find-Exe (Join-Path $Dependencies 'node') 'node.exe') `
  -Destination (Join-Path $Stage 'node\node.exe')
Copy-Tree (Join-Path $Dependencies 'node\node_modules\@oai\artifact-tool') `
  (Join-Path $Stage 'node\node_modules\@oai\artifact-tool')
$NodeExe = Join-Path $Stage 'node\node.exe'
Report 'node'

# 3. LibreOffice 与 Poppler ————————————————————————————————
# 两个都整棵搬进 native\：Windows 按 exe 所在目录搜 DLL，把 exe 单独拎出来会
# 少一堆动态库。之所以不像 macOS 那样各占一个顶层目录，是因为 runtime_helpers.py
# 在 Windows 分支上硬性要求原生 exe 位于 <RUNTIME_BIN_DIR>\..\..\native 之下。
Step 'LibreOffice 与 Poppler'
$nativeSrc = Join-Path $Dependencies 'native'
$LibreRoot = $null
$PopplerRoot = $null
foreach ($pair in @(@('soffice.exe', 'Libre'), @('pdftoppm.exe', 'Poppler'))) {
  $src = Get-ComponentRoot $nativeSrc (Find-Exe $nativeSrc $pair[0])
  $dest = Join-Path $Stage "native\$(Split-Path -Leaf $src)"
  Copy-Tree $src $dest
  if ($pair[1] -eq 'Libre') { $LibreRoot = $dest } else { $PopplerRoot = $dest }
}
$SofficeExe = Find-Exe $LibreRoot 'soffice.exe'
$PopplerBin = Split-Path -Parent (Find-Exe $PopplerRoot 'pdftoppm.exe')
Report 'native'

# 4. CJK 字体 ——————————————————————————————————————————————
# 打包的 LibreOffice 自带一百多个字体但一个 CJK 都没有，而且构建成看不见系统字体。
# 不补这一步，中文文档会整片渲染成空白 —— 比不渲染更糟，因为模型看到空白会以为
# 是自己排版写错了，然后开始瞎改。
Step 'CJK 字体'
$zip = Join-Path $Cache 'NotoSansCJKsc.zip'
if (-not (Test-Path -LiteralPath $zip) -or (Get-Item -LiteralPath $zip).Length -lt 1MB) {
  Log '  下载 Noto Sans CJK SC（约 90MB，已缓存则跳过）…'
  $url = 'https://github.com/notofonts/noto-cjk/releases/download/Sans2.004/08_NotoSansCJKsc.zip'
  Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
}
$notoDir = Join-Path $Cache 'noto'
if (Test-Path -LiteralPath $notoDir) { Remove-Item -LiteralPath $notoDir -Recurse -Force }
Expand-Archive -LiteralPath $zip -DestinationPath $notoDir -Force

# 字体目录在不同打包版本里位置不一样，与其猜路径，不如找它自带的字体在哪 ——
# 那个目录一定是它会扫的。
$anyFont = Get-ChildItem -LiteralPath $LibreRoot -Include '*.ttf', '*.otf', '*.ttc' -Recurse -File -ErrorAction SilentlyContinue |
  Select-Object -First 1
if (-not $anyFont) { Fail "在 $LibreRoot 下找不到任何字体文件，无法确定字体目录。" }
$fontDir = $anyFont.Directory.FullName
Log "  字体目录: $fontDir"
foreach ($want in @('NotoSansCJKsc-Regular.otf', 'NotoSansCJKsc-Bold.otf')) {
  $f = Get-ChildItem -LiteralPath $notoDir -Filter $want -Recurse -File | Select-Object -First 1
  if (-not $f) { Fail "字体包里没找到 $want" }
  Copy-Item -LiteralPath $f.FullName -Destination (Join-Path $fontDir $want) -Force
  Log "  装入 $want"
}

# 5. shim 与原生清单 ————————————————————————————————————————
# shim 写成 bash 脚本（LF 换行、相对路径转发），因为 Riot 在 Windows 上执行
# 模型命令用的就是 Git bash。Python 侧走下面那份 JSON，不碰 shim。
Step 'shim'
$binDir = Join-Path $Stage 'bin\override'
$pathDir = Join-Path $Stage 'path'
New-Item -ItemType Directory -Path $binDir, $pathDir -Force | Out-Null

function Rel([string]$from, [string]$to) {
  Push-Location -LiteralPath $from
  try {
    $rel = Resolve-Path -LiteralPath $to -Relative
    # Resolve-Path 给后代路径带 `.\` 前缀，给祖先路径直接以 `..\` 开头 ——
    # 无脑砍两个字符会把 `..\..\native\…` 砍成 `\native\…`。
    if ($rel.StartsWith('.\')) { $rel = $rel.Substring(2) }
    return $rel -replace '\\', '/'
  } finally { Pop-Location }
}

function Write-Shim([string]$dir, [string]$name, [string]$target) {
  $rel = Rel $dir $target
  $body = @"
#!/usr/bin/env bash
# 相对路径转发，整个能力包可以整体搬移。
set -euo pipefail
DIR="`$(cd "`$(dirname "`${BASH_SOURCE[0]}")" && pwd)"
exec "`${DIR}/$rel" "`$@"
"@
  # 必须是 LF：CRLF 会让 bash 把 \r 当成命令名的一部分，报 $'\r': command not found。
  [IO.File]::WriteAllText((Join-Path $dir $name), ($body -replace "`r`n", "`n"), (New-Object Text.UTF8Encoding $false))
}

$shims = [ordered]@{
  soffice    = $SofficeExe
  pdftoppm   = Join-Path $PopplerBin 'pdftoppm.exe'
  pdfinfo    = Join-Path $PopplerBin 'pdfinfo.exe'
  pdftocairo = Join-Path $PopplerBin 'pdftocairo.exe'
  pdfimages  = Join-Path $PopplerBin 'pdfimages.exe'
  python3    = $PythonExe
  python     = $PythonExe
  node       = $NodeExe
}
# 别处不提供、放进 PATH 不会遮住用户任何东西的那些。python3 和 node 不进：
# 用户给会话配了 venv 时，一句 `python manage.py` 不该拿到包里这份。
$onPath = @('soffice', 'pdftoppm', 'pdfinfo', 'pdftocairo', 'pdfimages')

foreach ($name in $shims.Keys) {
  $target = $shims[$name]
  if (-not (Test-Path -LiteralPath $target)) { Fail "shim $name 的目标不存在: $target" }
  Write-Shim $binDir $name $target
  if ($onPath -contains $name) { Write-Shim $pathDir $name $target }
}

# Python 侧的名字→真实 exe 映射。runtime_helpers.py 要求解析结果落在
# <pack>\native 之内且以 .exe 结尾，所以这里只放原生二进制，不放解释器。
$manifest = [ordered]@{}
foreach ($name in @('soffice', 'pdftoppm', 'pdfinfo', 'pdftocairo', 'pdfimages')) {
  $manifest[$name] = Rel $binDir $shims[$name]
}
[IO.File]::WriteAllText((Join-Path $binDir 'native-executables.json'),
  (($manifest | ConvertTo-Json -Depth 3) + "`n"), (New-Object Text.UTF8Encoding $false))
Log "  bin/override $($shims.Count) 个 + 原生清单，path/ $($onPath.Count) 个"

# 6. Skills ————————————————————————————————————————————————
Step 'Skills'
$skillNames = @('documents', 'spreadsheets', 'presentations', 'pdf')
$skillsDir = Join-Path $Stage 'skills'
New-Item -ItemType Directory -Path $skillsDir -Force | Out-Null
$runtimeVersion = $null
foreach ($name in $skillNames) {
  $base = Join-Path $Plugins $name
  if (-not (Test-Path -LiteralPath $base)) { Fail "Codex 插件缓存里没有 $name" }
  $ver = Get-ChildItem -LiteralPath $base -Directory | Where-Object { $_.Name -match '^\d' } |
    Sort-Object Name | Select-Object -Last 1
  if (-not $ver) { Fail "$base 下没有版本目录" }
  if (-not $runtimeVersion) { $runtimeVersion = $ver.Name }
  Copy-Tree (Join-Path $ver.FullName "skills\$name") (Join-Path $skillsDir $name)
}
# 改写规则和 macOS 共用一份，用刚提取出来的 node 跑，不要求构建机装 Node。
& $NodeExe (Join-Path $Root 'scripts\doc-pack\adapt-skills.mjs') $skillsDir
if ($LASTEXITCODE -ne 0) { Fail 'skill 改写失败' }

# 7. pack.json ——————————————————————————————————————————————
Step 'pack.json'
function RelToStage([string]$p) { (Rel $Stage $p) }

$packJson = [ordered]@{
  name          = $PackName
  version       = $Version
  platform      = $Platform
  builtAt       = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
  sourceRuntime = $runtimeVersion
  env           = [ordered]@{
    # 指向真 exe 而不是 shim：presentations 的脚本会用它起子进程，
    # 而 CreateProcess 不认无扩展名的 bash 脚本。
    RUNTIME_NODE         = (RelToStage $NodeExe)
    RUNTIME_NODE_MODULES = 'node/node_modules'
    # runtime_helpers.py 按 bin_dir.parent.parent/native 找原生根，这个深度是它要求的。
    RUNTIME_BIN_DIR      = 'bin/override'
  }
  pathPrepend   = @('path')
  # 装完立刻实跑一遍：包里的二进制在用户机器上被杀软或 SmartScreen 拦下的话，
  # 要在他刚点完"安装"的时候就报出来，而不是几天后让模型撞上。
  selfCheck     = @(
    [ordered]@{ command = (RelToStage $PythonExe); args = @('-c', 'import docx, pptx, openpyxl, pdfplumber, reportlab') },
    [ordered]@{ command = (RelToStage $NodeExe); args = @('-v') },
    [ordered]@{ command = (RelToStage $SofficeExe); args = @('--version') },
    [ordered]@{ command = (RelToStage (Join-Path $PopplerBin 'pdftoppm.exe')); args = @('-v') }
  )
  mcpServers    = @(
    [ordered]@{
      id      = 'doc-artifact-tool'
      command = (RelToStage $NodeExe)
      args    = @('node/node_modules/@oai/artifact-tool/dist/artifact-session-mcp/server.mjs')
    }
  )
  skills        = $skillNames
}
[IO.File]::WriteAllText((Join-Path $Stage 'pack.json'),
  (($packJson | ConvertTo-Json -Depth 6) + "`n"), (New-Object Text.UTF8Encoding $false))

$installedSize = Size-Of $Stage
Log "`n铺出完成: $(Mb $installedSize)"

if ($StageOnly) {
  Log '-StageOnly，跳过打包。'
  Log "`n本地验证: & `"$NodeExe`" scripts\doc-pack\verify-pack.mjs `"$Stage`""
  exit 0
}

# 8. 打包 ————————————————————————————————————————————————————
# Windows 10 1803+ 自带 bsdtar，但没有 zstd 命令行工具，压缩交给包里的 Node。
Step '打包 tar.zst'
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$tar = Join-Path $Dist "$PackName-$Version-$Platform.tar"
$tarball = Join-Path $OutDir "$PackName-$Version-$Platform.tar.zst"
Remove-Item -LiteralPath $tar, $tarball -Force -ErrorAction SilentlyContinue
& tar.exe -cf $tar -C $Dist (Split-Path -Leaf $Stage)
if ($LASTEXITCODE -ne 0) { Fail 'tar 打包失败' }
& $NodeExe (Join-Path $Root 'scripts\doc-pack\zstd.mjs') $tar $tarball 19
if ($LASTEXITCODE -ne 0) { Fail 'zstd 压缩失败' }
Remove-Item -LiteralPath $tar -Force

$sha256 = (Get-FileHash -LiteralPath $tarball -Algorithm SHA256).Hash.ToLowerInvariant()
$size = (Get-Item -LiteralPath $tarball).Length
Log "  $(Split-Path -Leaf $tarball)  $(Mb $size)  sha256 $($sha256.Substring(0,16))…"

# 9. 本平台清单 ——————————————————————————————————————————————
# 这份只描述本机造出来的东西。跨平台的合并交给 merge-manifest.mjs —— macOS 的包
# 在另一台机器上，这里既算不出它的 sha256 也不该猜。
Step 'packs.json'
$manifestPath = Join-Path $OutDir 'packs.json'
if (Test-Path -LiteralPath $manifestPath) {
  $m = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
} else {
  $m = [PSCustomObject]@{ schemaVersion = 1; packs = [PSCustomObject]@{} }
}
if (-not $m.packs.PSObject.Properties[$PackName]) {
  $m.packs | Add-Member -NotePropertyName $PackName -NotePropertyValue ([PSCustomObject]@{
      version = $Version; platforms = [PSCustomObject]@{}
    })
}
$entry = $m.packs.$PackName
$entry.version = $Version
$asset = [PSCustomObject]@{
  url           = "https://github.com/$PkgRepoSlug/releases/download/$ReleaseTag/$(Split-Path -Leaf $tarball)"
  sha256        = $sha256
  size          = $size
  installedSize = $installedSize
}
if ($entry.platforms.PSObject.Properties[$Platform]) { $entry.platforms.$Platform = $asset }
else { $entry.platforms | Add-Member -NotePropertyName $Platform -NotePropertyValue $asset }
[IO.File]::WriteAllText($manifestPath, (($m | ConvertTo-Json -Depth 8) + "`n"), (New-Object Text.UTF8Encoding $false))
Log "  $manifestPath"

if (-not $KeepStage) { Remove-Item -LiteralPath $Stage -Recurse -Force }

Log "`n完成。压缩 $(Mb $size)，安装后 $(Mb $installedSize)。"
Log "  $tarball"
Log "`n验证: & `"<解压后的目录>\node\node.exe`" scripts\doc-pack\verify-pack.mjs <解压后的目录>"
Log "发布: & `"$NodeExe`" scripts\doc-pack\publish.mjs"
