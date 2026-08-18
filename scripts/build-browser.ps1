# 构建并打包 riot-browser（CEF 浏览器宿主子进程）—— Windows 版。
#
# 为什么要单独一个脚本:
#
# 1. riot-browser 不在主 workspace 里（见根 Cargo.toml 的 exclude），
#    `cargo build --workspace` 碰不到它;
# 2. CEF 在 Windows 上要求 libcef.dll、icudtl.dat、*.pak、locales 等一整套
#    运行时文件和 exe **平铺在同一个目录** —— 这个脚本负责把它们凑到一起。
#    没有 macOS 那样的 .app 约束，但"文件都在 exe 旁边"这一条同样是硬的:
#    缺 dll 时进程根本起不来，缺资源时卡在 icudtl.dat。
#
# 和 macOS 不同，CEF 的二进制分发包不用手动拉:cef-dll-sys 的 build.rs 会
# 自动下载到 CEF_PATH（约 355MB，首次构建比较久）。但编译它的 C++ wrapper
# 需要 CMake 和 Ninja —— Visual Studio 的"使用 C++ 的桌面开发"工作负载
# 自带这两个，脚本会自动从 VS 里借;要单独装的话:
#   winget install Kitware.CMake Ninja-build.Ninja

$ErrorActionPreference = "Stop"

$CrateDir = (Resolve-Path (Join-Path $PSScriptRoot "..\crates\riot-browser")).Path
if (-not $env:CEF_PATH) {
    # 和 scripts/build-browser.sh 用同一个默认位置，双系统开发时缓存能对上。
    $env:CEF_PATH = Join-Path $env:USERPROFILE ".local\share\cef"
}
$Out = if ($args.Count -ge 1) { $args[0] } else { Join-Path $CrateDir "target\bundle" }

# cmake / ninja 不在 PATH 时，从 Visual Studio 的安装里借。
# 主应用能编说明 MSVC 一定在，而 VS 装 C++ 工作负载时这两个工具都带了 ——
# 只是默认不进 PATH。直接报"缺工具"会让人多装一份重复的。
function Use-VsTool([string]$Name, [string]$VsRelPath) {
    if (Get-Command $Name -ErrorAction SilentlyContinue) { return }
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vs = & $vswhere -latest -products * -property installationPath 2>$null
        if ($vs) {
            $dir = Join-Path $vs $VsRelPath
            if (Test-Path (Join-Path $dir "$Name.exe")) {
                $env:PATH = "$dir;$env:PATH"
                return
            }
        }
    }
    throw "缺 $Name。装 Visual Studio 的'使用 C++ 的桌面开发'工作负载，或 winget install Kitware.CMake Ninja-build.Ninja"
}

Use-VsTool "cmake" "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"
Use-VsTool "ninja" "Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja"

Push-Location $CrateDir
try {
    cargo build
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }
}
finally {
    Pop-Location
}

# [约束] 先删掉旧 bundle，不要原地覆盖。
#
# 正在跑的浏览器进程锁着旧的 exe 和 dll，原地覆盖会在中途撞上"文件被占用"
# —— 一半新一半旧，dll 和 exe 版本对不上，下次启动直接崩，而且报错完全
# 不指向打包。删掉重建换的是新文件，老进程抱着旧文件走完自己的生命周期。
$Bundle = Join-Path $Out "riot-browser"
if (Test-Path $Bundle) { Remove-Item -Recurse -Force $Bundle }
New-Item -ItemType Directory -Force $Bundle | Out-Null

# CEF 的运行时文件在构建时已经被 cef-dll-sys 复制到 target\debug ——
# 从那里凑 bundle，不自己再实现一遍"去哪找 CEF 的哪些文件"。
# 它复制的就是分发包的全部顶层文件 + locales，漏谁都是运行时才炸。
$TargetDebug = if ($env:CARGO_TARGET_DIR) {
    Join-Path $env:CARGO_TARGET_DIR "debug"
} else {
    Join-Path $CrateDir "target\debug"
}

Copy-Item (Join-Path $TargetDebug "riot-browser.exe") $Bundle
Copy-Item (Join-Path $TargetDebug "riot-browser-helper.exe") $Bundle
foreach ($pattern in "*.dll", "*.pak", "*.bin", "*.dat", "*.json") {
    Copy-Item (Join-Path $TargetDebug $pattern) $Bundle -ErrorAction SilentlyContinue
}
Copy-Item (Join-Path $TargetDebug "locales") $Bundle -Recurse

Write-Host "打包完成: $Bundle"
