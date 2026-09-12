<#
.SYNOPSIS
    从 CHANGELOG.md 生成指定版本的「发版说明」正文。

.DESCRIPTION
    CHANGELOG.md 是发版说明的唯一来源（规范见 docs/release.md）。本脚本取走指定版本的
    小节（丢掉标题行），按目标裁剪语言，再追加 .github/release-footer.md：

      -Mode github  → 中文段 + --- + 英文段 + 下载安装（中英）
      -Mode gitee   → 仅中文段 + 中文下载安装（Gitee 是中文镜像，不重复英文）

    小节内中英文以独占一行的 <!-- en --> 分隔；缺少该标记、中文正文过短、找不到小节
    都会以非零退出码失败，避免把空说明发上发行版页面。

.EXAMPLE
    pwsh -File .github/scripts/release-notes.ps1 -Tag v1.0.9 -Mode github -DryRun
    pwsh -File .github/scripts/release-notes.ps1 -Tag v1.0.9 -Mode gitee -OutFile notes.gitee.md
    pwsh -File .github/scripts/release-notes.ps1 -ListVersions
#>
[CmdletBinding()]
param(
    [string]$Tag = '',
    [ValidateSet('github', 'gitee')][string]$Mode = 'github',
    [string]$Changelog = '',
    [string]$Footer = '',
    [string]$OutFile = '',
    [switch]$ListVersions,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    Write-Host "[release-notes] 失败：$Message" -ForegroundColor Red
    exit 1
}

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)   # .github/scripts → 仓库根
if (-not $Changelog) { $Changelog = Join-Path $root 'CHANGELOG.md' }
if (-not $Footer) { $Footer = Join-Path $root '.github/release-footer.md' }
if (-not (Test-Path -LiteralPath $Changelog)) { Fail "找不到变更日志：$Changelog" }

$lines = @(Get-Content -LiteralPath $Changelog -Encoding UTF8)

# 小节标题形如 "## v1.0.9" 或 "## v1.0.9 — 2026-09-13"，取标题首个 token 作为版本号
$headings = New-Object System.Collections.ArrayList
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match '^##\s+(.+?)\s*$') {
        $token = ($Matches[1] -split '\s+')[0]
        [void]$headings.Add([pscustomobject]@{
                Index   = $i
                Version = ($token -replace '^[vV]', '')
            })
    }
}

if ($ListVersions) {
    foreach ($h in $headings) { Write-Output ('v' + $h.Version) }
    exit 0
}

$version = ($Tag -replace '^[vV]', '').Trim()
if (-not $version) { Fail '缺少 -Tag（例：-Tag v1.0.9）' }
$tagName = 'v' + $version

$current = $headings | Where-Object { $_.Version -eq $version } | Select-Object -First 1
if (-not $current) {
    Fail "CHANGELOG.md 中没有 $tagName 小节。发版前请先补一节（格式见 docs/release.md）再打 tag。"
}
$nextHeading = $headings | Where-Object { $_.Index -gt $current.Index } | Select-Object -First 1

$rawSection = ''
$sectionEnd = if ($nextHeading) { $nextHeading.Index - 1 } else { $lines.Count - 1 }
if ($sectionEnd -gt $current.Index) {
    $rawSection = ($lines[($current.Index + 1)..$sectionEnd] -join "`n")
}

$parts = $rawSection -split '(?s)<!--\s*en\s*-->'
$cn = $parts[0].Trim()
$en = ''
if ($parts.Count -gt 1) { $en = ($parts[1..($parts.Count - 1)] -join '<!-- en -->').Trim() }

if ($cn.Length -lt 80) { Fail "$tagName 小节的中文正文只有 $($cn.Length) 字符，疑似未写完。" }
if ($Mode -eq 'github' -and $en.Length -lt 40) {
    Fail "$tagName 小节缺少英文段：请用独占一行的 <!-- en --> 分隔（格式见 docs/release.md）。"
}

if (-not (Test-Path -LiteralPath $Footer)) { Fail "找不到说明尾部模板：$Footer" }
$prevTag = if ($nextHeading) { 'v' + $nextHeading.Version } else { '' }
$footerLines = @(Get-Content -LiteralPath $Footer -Encoding UTF8)
if (-not $prevTag) { $footerLines = @($footerLines | Where-Object { $_ -notmatch '\{PREV\}' }) }
$footerText = ($footerLines -join "`n").Replace('{VERSION}', $version).Replace('{TAG}', $tagName)
if ($prevTag) { $footerText = $footerText.Replace('{PREV}', $prevTag) }
if ($Mode -eq 'gitee') {
    $footerText = ($footerText -split '(?s)<!--\s*en\s*-->')[0]
} else {
    $footerText = $footerText -replace '(?m)^\s*<!--\s*en\s*-->\s*$', '---'
}
$footerText = $footerText.Trim()
if (-not $footerText) { Fail "尾部模板处理为空：$Footer" }

$text = if ($Mode -eq 'github') { "$cn`n`n---`n`n$en`n`n$footerText" } else { "$cn`n`n$footerText" }
$text = $text.TrimEnd() + "`n"

if ($OutFile) {
    if (-not [IO.Path]::IsPathRooted($OutFile)) { $OutFile = Join-Path (Get-Location).Path $OutFile }
    [IO.File]::WriteAllText($OutFile, $text, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host ("[release-notes] {0} / {1} → {2}（{3} 字符）" -f $tagName, $Mode, $OutFile, $text.Length)
}
if ($DryRun -or -not $OutFile) { Write-Output $text }

# 显式收尾：调用方（工作流）用 `& $script …; if ($LASTEXITCODE -ne 0)` 判定成败，
# 脚本若不带 exit，$LASTEXITCODE 会残留上一条原生命令的值而误判。
exit 0
