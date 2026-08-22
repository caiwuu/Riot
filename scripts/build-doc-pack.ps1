<#
.SYNOPSIS
制作 Riot 文档能力包（Windows x64）。

.DESCRIPTION
和 scripts/build-doc-pack.mjs 产出同构的包，只是二进制来自 Windows 版 Codex 运行时。
必须在 Windows 上跑：artifact-tool 带 skia.node 这样的原生绑定，是按平台编译的，
没法从 macOS 交叉产出。

与 macOS 版的四处实质差异，都是被 Windows 逼出来的：

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

4. LibreOffice 不来自 Codex 运行时。Windows 版的 bundle 压根不带它：runtime.json
   里 libreOfficeVersion 是 null，native\ 下只有 poppler、git 那几个，连 Codex 自己
   的 native-executables.json 都没给 soffice 建映射。macOS 版才有 libreoffice-headless。
   Codex 在 Windows 上是指望用户自己装官方版、靠 PATH 找到 soffice 的；能力包不能
   这么赌，所以直接从上游拉官方 MSI，msiexec /a 解包后裁掉用不上的部分再装进来。

.PARAMETER Dependencies
Codex 主运行时的 dependencies 目录。不传就按常见位置探。

.PARAMETER LibreOffice
现成的 LibreOffice 目录（内含 program\soffice.exe），用来跳过下载与解包。
传了就原样取用，不做裁剪。

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
  [string]$LibreOffice,
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

# 上游只发 GPG 签名不发 sha256 文件，所以校验和钉在这儿。换版本时要一起改，
# 对不上就让构建失败 —— 镜像站是第三方的，静默拿到一份被换过的 MSI 更糟。
$LibreOfficeVersion = '26.2.5'
$LibreOfficeSha256 = 'f15ba07bfcb0186986cf3171063506f5d207c11f8cc051ba0d135209e9e915f9'

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

# —— 源：上游的 LibreOffice ————————————————————————————————

# 解包后 1.5GB，其中八成跟"把文档渲染成 PDF"没关系。下面几项都是交互式使用
# 才碰得到的，渲染链路不经过它们。裁完约 600MB。
$LibreOfficeDrop = @(
  'share\extensions',   # 各语言的拼写词典，460MB。渲染出的 PDF 里没有波浪线。
  'program\resource',   # 123 种界面语言的翻译，260MB。headless 不显示界面，英文还是内建的。
  'share\registry\res', # 同上，配置项的本地化文本。
  'share\gallery',      # 剪贴画库。
  'share\wizards',      # Basic 写的交互式向导。
  'help',
  'readmes'
)

function Get-LibreOfficeRoot {
  if ($LibreOffice) {
    if (-not (Test-Path -LiteralPath (Join-Path $LibreOffice 'program\soffice.exe'))) {
      Fail "-LibreOffice 指的目录下没有 program\soffice.exe: $LibreOffice"
    }
    return (Resolve-Path -LiteralPath $LibreOffice).Path
  }

  # 解包一次要一分多钟，裁完打个标记，重跑构建就直接复用。标记放在解包目录外面，
  # 否则它会跟着一起被复制进包里。
  $root = Join-Path $Cache "libreoffice-$LibreOfficeVersion"
  $stamp = "$root.trimmed"
  if (Test-Path -LiteralPath $stamp) {
    Log "  复用已解包的 $LibreOfficeVersion"
    return $root
  }

  $msiName = "LibreOffice_${LibreOfficeVersion}_Win_x86-64.msi"
  $msi = Join-Path $Cache $msiName
  if (-not (Test-Path -LiteralPath $msi)) {
    Log "  下载 LibreOffice $LibreOfficeVersion（约 356MB，已缓存则跳过）…"
    # 用 curl.exe 而不是 Invoke-WebRequest：后者在 PS 5.1 上会把整个响应缓进内存，
    # 几百 MB 慢得离谱。它和下面打包用的 tar.exe 一样，Win10 1803+ 自带。
    & curl.exe -L --fail --retry 3 -o $msi `
      "https://download.documentfoundation.org/libreoffice/stable/$LibreOfficeVersion/win/x86_64/$msiName"
    if ($LASTEXITCODE -ne 0) { Fail "LibreOffice 下载失败 ($LASTEXITCODE)" }
  }
  $got = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($got -ne $LibreOfficeSha256) {
    Fail ("MSI 校验和不符。`n  期望: $LibreOfficeSha256`n  实际: $got`n" +
      "删掉 $msi 重跑。若上游确实换了版本，同步更新脚本顶部的 `$LibreOfficeSha256。")
  }

  if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
  New-Item -ItemType Directory -Path $root -Force | Out-Null
  Log '  msiexec /a 解包（约一分钟）…'
  # /a 是"管理员安装"，只把文件按目录树铺开，不写注册表也不要求提权。
  $msiexec = Start-Process msiexec.exe -Wait -PassThru -ArgumentList @(
    '/a', "`"$msi`"", '/qn', "TARGETDIR=`"$root`"")
  if ($msiexec.ExitCode -ne 0) { Fail "msiexec 解包失败 ($($msiexec.ExitCode))" }

  Log '  裁剪…'
  # /a 会在 TARGETDIR 里留一份精简过的 MSI 副本，包里不需要。
  Remove-Item -LiteralPath (Join-Path $root $msiName) -Force -ErrorAction SilentlyContinue
  foreach ($rel in $LibreOfficeDrop) {
    Remove-Item -LiteralPath (Join-Path $root $rel) -Recurse -Force -ErrorAction SilentlyContinue
  }
  # 图标主题每套 3-16MB，headless 一套都用不上；但全删了 soffice 起来会抱怨找不到
  # 默认主题，留下 colibre 那套。
  Get-ChildItem -LiteralPath (Join-Path $root 'share\config') -Filter 'images_*.zip' -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -ne 'images_colibre.zip' } |
    ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }

  New-Item -ItemType File -Path $stamp -Force | Out-Null
  return $root
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

# 3. Poppler 与 LibreOffice ————————————————————————————————
# 两个都整棵搬进 native\：Windows 按 exe 所在目录搜 DLL，把 exe 单独拎出来会
# 少一堆动态库。之所以不像 macOS 那样各占一个顶层目录，是因为 runtime_helpers.py
# 在 Windows 分支上硬性要求原生 exe 位于 <RUNTIME_BIN_DIR>\..\..\native 之下。
Step 'Poppler'
$nativeSrc = Join-Path $Dependencies 'native'
$PopplerSrc = Get-ComponentRoot $nativeSrc (Find-Exe $nativeSrc 'pdftoppm.exe')
$PopplerRoot = Join-Path $Stage "native\$(Split-Path -Leaf $PopplerSrc)"
Copy-Tree $PopplerSrc $PopplerRoot
$PopplerBin = Split-Path -Parent (Find-Exe $PopplerRoot 'pdftoppm.exe')

# LibreOffice 得自己去上游拿，Codex 的 Windows 运行时不带（见文件头第 4 条）。
Step 'LibreOffice'
$LibreRoot = Join-Path $Stage 'native\libreoffice'
Copy-Tree (Get-LibreOfficeRoot) $LibreRoot
$SofficeExe = Find-Exe $LibreRoot 'soffice.exe'
# 真正要执行的是 soffice.com。soffice.exe 属于 GUI 子系统，起完 soffice.bin 就
# 立刻返回，subprocess 会在转换完成前拿到退出码，然后读到一个还没写完的 PDF。
# .com 是控制台包装，会等子进程结束并转发退出码。上游走 PATH 时是靠 PATHEXT 里
# .COM 排在 .EXE 前面隐式拿到它的，改成绝对路径后这个便宜就没了，得自己指明。
$SofficeCom = Join-Path (Split-Path -Parent $SofficeExe) 'soffice.com'
if (-not (Test-Path -LiteralPath $SofficeCom)) { Fail "soffice.exe 旁边没有 soffice.com: $SofficeExe" }
Report 'native'

# 4. CJK 字体 ——————————————————————————————————————————————
# LibreOffice 自带一百多个字体，一个 CJK 都没有。它在 Windows 上确实看得见系统字体，
# 但英文版 Windows 未必装了中文字体，赌不起：缺字的下场是中文整片渲染成空白 ——
# 比不渲染更糟，因为模型看到空白会以为是自己排版写错了，然后开始瞎改。自带一份，
# 渲染结果就跟机器无关了。
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

# 字体目录在不同打包版本里位置不一样，与其猜路径，不如找它自带的字体扎堆在哪 ——
# 那个目录一定是它会扫的。取数量最多的那个，别被散落在别处的个别字体带偏。
# （注意别用 -Include：它和 -LiteralPath 搭配时不生效，会把整棵树的文件都放过来，
# 于是"第一个文件"落在根目录，字体就装到了一个 LibreOffice 根本不扫的地方。）
$fontGroup = Get-ChildItem -LiteralPath $LibreRoot -Recurse -File -ErrorAction SilentlyContinue |
  Where-Object { @('.ttf', '.otf', '.ttc') -contains $_.Extension.ToLowerInvariant() } |
  Group-Object -Property DirectoryName | Sort-Object Count -Descending | Select-Object -First 1
if (-not $fontGroup) { Fail "在 $LibreRoot 下找不到任何字体文件，无法确定字体目录。" }
$fontDir = $fontGroup.Name
Log "  字体目录: $fontDir（自带 $($fontGroup.Count) 个）"
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

$shims = [ordered]@{ soffice = $SofficeCom }
# Codex 的 Windows poppler 只装了 pdftoppm 和 pdfinfo，它自己的 native-executables.json
# 也只列这两个。pdftocairo / pdfimages 没有任何 skill 引用，所以有就带上、没有就算了，
# 而不是写死一张名单然后在这儿翻车。
foreach ($tool in @('pdftoppm', 'pdfinfo', 'pdftocairo', 'pdfimages')) {
  $exe = Join-Path $PopplerBin "$tool.exe"
  if (Test-Path -LiteralPath $exe) { $shims[$tool] = $exe }
}
# 这两个不一样：pdf skill 的正文里直接写着 `pdftoppm -png`、`pdfinfo`，缺了就是坏的。
foreach ($required in @('pdftoppm', 'pdfinfo')) {
  if (-not $shims.Contains($required)) { Fail "poppler 里没有 $required.exe: $PopplerBin" }
}
$shims['python3'] = $PythonExe
$shims['python'] = $PythonExe
$shims['node'] = $NodeExe

# 别处不提供、放进 PATH 不会遮住用户任何东西的那些。python3 和 node 不进：
# 用户给会话配了 venv 时，一句 `python manage.py` 不该拿到包里这份。
$onPath = @('soffice', 'pdftoppm', 'pdfinfo', 'pdftocairo', 'pdfimages') |
  Where-Object { $shims.Contains($_) }

foreach ($name in $shims.Keys) {
  $target = $shims[$name]
  if (-not (Test-Path -LiteralPath $target)) { Fail "shim $name 的目标不存在: $target" }
  Write-Shim $binDir $name $target
  if ($onPath -contains $name) { Write-Shim $pathDir $name $target }
}

# Python 侧的名字→真实 exe 映射。runtime_helpers.py 要求解析结果落在
# <pack>\native 之内且以 .exe 结尾，所以这里只放原生二进制，不放解释器。
# soffice 也因此只能写 .exe：那条后缀断言是无条件的，眼下 presentations 只拿
# poppler 那几个，但没必要留个将来会炸的坑。真正执行时由 render_docx.py 提升到
# 同目录的 .com（见 adapt-skills.mjs），所以这里指 .exe 不影响转换。
$manifest = [ordered]@{}
foreach ($name in $onPath) {
  $manifest[$name] = Rel $binDir $(if ($name -eq 'soffice') { $SofficeExe } else { $shims[$name] })
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
    # 走 .com：soffice.exe 是 GUI 子系统，--version 不会往父控制台写东西。
    [ordered]@{ command = (RelToStage $SofficeCom); args = @('--version') },
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
