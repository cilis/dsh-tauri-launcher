<#
.SYNOPSIS
    生成 Tauri 官方 updater 的静态清单 latest.json。

.DESCRIPTION
    读取 tauri build 产出的 *-setup.nsis.zip.sig，写出官方静态清单：

      { version, notes, pub_date,
        platforms: { "windows-x86_64-nsis": { url, signature },
                     "windows-x86_64":      { url, signature } } }

    两个平台键故意指向**同一个** .nsis.zip：新版（按安装形态分键）取
    windows-x86_64-nsis，迁移期的旧客户端取裸键 windows-x86_64，两者都能命中同一产物。

    发布侧只做清单，不碰签名：.sig 缺失或版本号非法时以退出码 1 结束并给出可读原因，
    避免把一份客户端无法校验的清单发上发行版。

.EXAMPLE
    pwsh -File .github/scripts/latest-json.ps1 -Version 1.1.6 -Notes "见发行版说明" \
        -OutFile dist/latest.json -AssetBaseUrl https://github.com/cilis/dsh-tauri-launcher/releases/download/v1.1.6 \
        -BundleDir dist
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Notes = '',
    [Parameter(Mandatory = $true)][string]$OutFile,
    [Parameter(Mandatory = $true)][string]$AssetBaseUrl,
    # 签名产物所在目录（tauri 的 bundle/nsis 产物收集后的目录）。
    [string]$BundleDir = '.'
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    Write-Host "[latest-json] 失败：$Message" -ForegroundColor Red
    exit 1
}

# --- 版本号校验 -------------------------------------------------------------
# 允许 tag 形式（v1.1.6）与 semver 预发布后缀（1.1.6-rc.1）。形状不对会让客户端
# 的版本比较得出错误结论，因此直接失败而不是照抄。
$semver = $Version.Trim()
if ($semver -match '^[vV]') { $semver = $semver.Substring(1) }
if ($semver -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?(\+[0-9A-Za-z.]+)?$') {
    Fail "版本号非法：“$Version”（应形如 1.1.6 或 1.1.6-rc.1）。"
}

# --- 定位签名产物 -----------------------------------------------------------
if (-not (Test-Path -LiteralPath $BundleDir)) { Fail "找不到产物目录：$BundleDir" }
$bundleFull = (Resolve-Path -LiteralPath $BundleDir).Path

$sigs = @(Get-ChildItem -LiteralPath $bundleFull -Filter '*.nsis.zip.sig' -File)
if ($sigs.Count -eq 0) {
    Fail "在 $bundleFull 里找不到 *.nsis.zip.sig（未设置 TAURI_SIGNING_PRIVATE_KEY 时不会生成签名产物）。"
}
if ($sigs.Count -gt 1) {
    $names = ($sigs | ForEach-Object { $_.Name }) -join ', '
    Fail "目录里有多个签名产物（$names），无法判断用哪一个；请只保留本次构建的产物。"
}

$zipName = $sigs[0].Name -replace '\.sig$', ''          # xxx_x64-setup.nsis.zip
$zipPath = Join-Path $bundleFull $zipName
if (-not (Test-Path -LiteralPath $zipPath)) { Fail "签名存在但找不到对应安装包：$zipName" }

# .sig 是单行 base64；首尾空白必须去掉，客户端对签名串的空白敏感。
$signature = (Get-Content -LiteralPath $sigs[0].FullName -Raw).Trim()
if (-not $signature) { Fail "签名文件是空的：$($sigs[0].Name)" }

# --- 组装清单 ---------------------------------------------------------------
$base = $AssetBaseUrl.TrimEnd('/')
# 文件名可能含空格（productName 带空格），拼进 URL 前转义。
$assetUrl = '{0}/{1}' -f $base, [Uri]::EscapeDataString($zipName)
$pubDate = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')

$manifest = [ordered]@{
    version   = $semver
    notes     = $Notes
    pub_date  = $pubDate
    platforms = [ordered]@{
        'windows-x86_64-nsis' = [ordered]@{ url = $assetUrl; signature = $signature }
        'windows-x86_64'      = [ordered]@{ url = $assetUrl; signature = $signature }
    }
}

$json = $manifest | ConvertTo-Json -Depth 6
if ($json -notmatch '"windows-x86_64-nsis"') { Fail "清单结构异常：缺少 windows-x86_64-nsis 平台键。" }

$outFull = [IO.Path]::GetFullPath($OutFile)
$outDir = Split-Path -Parent $outFull
if ($outDir -and -not (Test-Path -LiteralPath $outDir)) {
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null
}
# 无 BOM：Tauri 用 serde_json 解析，BOM 会被判为非法 JSON。
[IO.File]::WriteAllText($outFull, $json + "`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Host "[latest-json] 已写出 $outFull"
Write-Host "  version  : $semver"
Write-Host "  pub_date : $pubDate"
Write-Host "  asset    : $zipName"
Write-Host "  url      : $assetUrl"
exit 0
