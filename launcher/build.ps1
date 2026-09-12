# dsh-tauri-launcher 桌面应用一键构建脚本（Windows）。
# 用法：在仓库根目录运行  pwsh -File launcher/build.ps1
# 产物：launcher/src-tauri/target/release/dsh-launcher.exe
#
# 说明：
# - 依赖 Rust 工具链（rustup + MSVC 或 GNU）与 Node.js 22+；
# - 若本机无法联网拉取 crates.io 依赖，可先用 --offline 配合一个预置的
#   CARGO_HOME（例如把已缓存的 .cargo 注册表目录传给 -CargoHome 参数）。

param(
    [switch]$Offline,
    [string]$CargoHome = '',
    # 构建前把版本号同步到三处（package.json / Cargo.toml / tauri.conf.json），
    # 例如 -Bump 1.0.9；留空则不动版本号。
    [string]$Bump = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$srcTauri = Join-Path $PSScriptRoot 'src-tauri'

# 版本号三处同步：用 .NET API 显式 UTF-8（无 BOM）读写，避免 Get-Content /
# Set-Content 在非 UTF-8 默认代码页下损坏含中文的文件（package.json 的
# description 就是中文）。
if ($Bump -ne '') {
    if ($Bump -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$') {
        throw "invalid version: $Bump (expected e.g. 1.0.9)"
    }
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $targets = @(
        @{ File = (Join-Path $root 'package.json');        Pattern = '("version"\s*:\s*")[^"]+(")' },
        @{ File = (Join-Path $srcTauri 'Cargo.toml');      Pattern = '(?m)^(version\s*=\s*")[^"]+(")' },
        @{ File = (Join-Path $srcTauri 'tauri.conf.json'); Pattern = '("version"\s*:\s*")[^"]+(")' }
    )
    foreach ($target in $targets) {
        $text = [System.IO.File]::ReadAllText($target.File, $utf8)
        # 只替换**第一处**匹配：Cargo.toml 中另有依赖项的 version 行
        # （如 [target."cfg(windows)".dependencies.windows] 的 0.61），
        # 全量替换会把它一并改坏（实测离线解析报 windows = "^1.0.9" 失败）。
        $updated = ([regex]$target.Pattern).Replace($text, ('${1}' + $Bump + '${2}'), 1)
        if ($updated -eq $text) { throw "version field not found: $($target.File)" }
        [System.IO.File]::WriteAllText($target.File, $updated, $utf8)
        Write-Host "version -> $Bump : $($target.File)"
    }
    Write-Host 'NOTE: rebuild + sync launcher/bin/dsh-launcher.exe before publishing.'
}

if ($CargoHome -ne '') {
    $env:CARGO_HOME = $CargoHome
    Write-Host "CARGO_HOME = $CargoHome"
}

# 原生命令（cargo）把编译进度写到 stderr：PowerShell 5.1 在
# $ErrorActionPreference='Stop' 下会把它当作终止性的 NativeCommandError，
# 表现为「cargo 实际构建成功、脚本却报失败」。这里临时放宽 EAP，
# 成败一律以 $LASTEXITCODE 判定。
$previousEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
Push-Location $srcTauri
try {
    if ($Offline) {
        Write-Host 'cargo build --release --offline'
        cargo build --release --offline
    } else {
        Write-Host 'cargo build --release'
        cargo build --release
    }
} finally {
    $ErrorActionPreference = $previousEap
    Pop-Location
}
if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }

$exe = Join-Path $srcTauri 'target\release\dsh-launcher.exe'
if (-not (Test-Path -LiteralPath $exe)) {
    throw "构建产物缺失：$exe"
}
Write-Host ''
Write-Host "构建完成：$exe"
Get-Item -LiteralPath $exe | Select-Object Length, LastWriteTime
