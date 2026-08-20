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
    [string]$CargoHome = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$srcTauri = Join-Path $PSScriptRoot 'src-tauri'

if ($CargoHome -ne '') {
    $env:CARGO_HOME = $CargoHome
    Write-Host "CARGO_HOME = $CargoHome"
}

Push-Location $srcTauri
try {
    if ($Offline) {
        Write-Host 'cargo build --release --offline'
        cargo build --release --offline
    } else {
        Write-Host 'cargo build --release'
        cargo build --release
    }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}

$exe = Join-Path $srcTauri 'target\release\dsh-launcher.exe'
if (-not (Test-Path -LiteralPath $exe)) {
    throw "构建产物缺失：$exe"
}
Write-Host ''
Write-Host "构建完成：$exe"
Get-Item -LiteralPath $exe | Select-Object Length, LastWriteTime
