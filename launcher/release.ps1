# dsh-tauri-launcher 桌面应用一键发版脚本（roadmap v1.2 · W9）。
#
# 推荐用法（在仓库根目录）：
#   pwsh -File launcher/release.ps1 -Version 1.2.0 -DryRun    # 干跑：只做只读检查
#   pwsh -File launcher/release.ps1 -Version 1.2.0            # 第一段：分支 + 提交 + 推送
#   pwsh -File launcher/release.ps1 -Version 1.2.0 -TagOnly   # 第二段：合并后在 main 上打 tag
#
# 脚本里的每一步都对应一次实测踩过的坑，改动前先读 docs/release.md 与 roadmap §9.2：
#   1) 发版说明先行——ci.yml 会拦「改了版本号却没写说明」，所以 CHANGELOG 小节是前置条件；
#   2) 运行中的 dsh-launcher 锁着 exe，覆盖报 os error 5（实测被挡过三次）；
#   3) build.ps1 -Bump 只改三处版本号，**不会**把产物同步到 launcher/bin（它只打一行提示）；
#   4) 判据读 PE 版本资源（VersionInfo），不要用字符串搜 exe——PE 里是 UTF-16，UTF-8 搜会漏；
#   5) 提交信息走 UTF-8 文件（git commit -F），命令行 -m 在非 UTF-8 代码页下会把中文写坏；
#   6) tag 用 lightweight（与既有 tag 一致），且只从 main 打、推送即触发 release.yml。

<#
.SYNOPSIS
    把发版的固定动作串成一条命令，每步带判据，失败即停。

.DESCRIPTION
    第一段（默认）按序执行：

      1/7  校验 CHANGELOG.md 的对应小节（发版说明的唯一来源）；
      2/7  环境检查：dsh-launcher 未在运行、当前在 main、工作区无阻塞性改动；
      3/7  build.ps1 -Bump 同步三处版本号并重建 release exe；
      4/7  复制产物到 launcher/bin/dsh-launcher.exe；
      5/7  判据核验：三处版本号完全一致 + exe 的 PE 版本资源相符；
      6/7  按类型分两次提交（docs: CHANGELOG 小节 → chore: 版本同步）；
      7/7  推送发版分支并打印创建 PR 的链接。

    脚本到此为止：**不代替人工开 PR、不代替合并、不碰 npm publish**。main 有分支保护
    （含管理员不豁免），所以合并之后回到 main 拉取，再跑一次加 -TagOnly 打 tag。

.PARAMETER Version
    发行版本号，如 1.2.0 或 1.2.0-rc.1（与 build.ps1 -Bump 同一格式）。

.PARAMETER TagOnly
    第二段：打 lightweight tag 并推送。要求当前分支为 main、工作区干净、三处版本号
    已等于 -Version、CHANGELOG 小节合格、tag 尚不存在、本地 main 与 origin/main 一致。

.PARAMETER DryRun
    干跑：只做只读检查并打印将要执行的动作，不构建、不复制、不提交、不打 tag、不推送。
    全部就绪时退出码 0，有未就绪项时退出码 1。

.PARAMETER SkipBuild
    跳过重建，直接核验现有 exe（PE 版本资源仍必须等于目标版本）。用于复用 CI 产出的 exe；
    要求三处版本号已同步，否则报错。

.PARAMETER NoPush
    只做本地提交，不推送（演练或离线时用）。

.PARAMETER Yes
    跳过推送前的交互确认（无人值守时用）。

.PARAMETER Offline
    透传 build.ps1：cargo build --release --offline。

.PARAMETER CargoHome
    透传 build.ps1：构建前设置 CARGO_HOME（本机离线缓存目录，见工作区 AGENTS.md）。

.EXAMPLE
    pwsh -File launcher/release.ps1 -Version 1.2.0 -DryRun
    pwsh -File launcher/release.ps1 -Version 1.2.0 -Offline -CargoHome ..\.cargo
    pwsh -File launcher/release.ps1 -Version 1.2.0 -TagOnly
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)][string]$Version,
    [switch]$TagOnly,
    [switch]$DryRun,
    [switch]$SkipBuild,
    [switch]$NoPush,
    [switch]$Yes,
    [switch]$Offline,
    [string]$CargoHome = ''
)

$ErrorActionPreference = 'Stop'

# UTF-8（无 BOM）读写：package.json 的 description 是中文，走
# Get-Content / Set-Content 在非 UTF-8 默认代码页下会整文件乱码（同 build.ps1）。
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

$repoRoot = Split-Path -Parent $PSScriptRoot
$tag = 'v' + $Version
$releaseBranch = 'chore/release-' + $tag
$unready = @()

$versionTargets = @(
    @{ Name = 'package.json';    Path = 'package.json';                       Pattern = '"version"\s*:\s*"([^"]+)"' },
    @{ Name = 'Cargo.toml';      Path = 'launcher\src-tauri\Cargo.toml';      Pattern = '(?m)^version\s*=\s*"([^"]+)"' },
    @{ Name = 'tauri.conf.json'; Path = 'launcher\src-tauri\tauri.conf.json'; Pattern = '"version"\s*:\s*"([^"]+)"' }
)

$builtExeRel = 'launcher\src-tauri\target\release\dsh-launcher.exe'
$binExeRel = 'launcher\bin\dsh-launcher.exe'

function Write-Step([string]$Text) {
    Write-Host ''
    Write-Host "== $Text" -ForegroundColor Cyan
}

function Write-Note([string]$Text) {
    Write-Host "   $Text"
}

# 检查未通过：正式执行直接中止；干跑记为「未就绪」后继续，好把问题一次列全。
function Fail([string]$Message) {
    Write-Host ''
    Write-Host "[release] 失败：$Message" -ForegroundColor Red
    if ($DryRun) {
        Write-Host '   [dry-run] 记为未就绪，继续后面的只读检查。' -ForegroundColor Yellow
        $script:unready += $Message
        return
    }
    exit 1
}

function Confirm-Action([string]$Question) {
    if ($DryRun -or $Yes) { return $true }
    $answer = Read-Host "   $Question [y/N]"
    return ($answer -match '^[yY]')
}

function Get-PowerShellExe {
    try {
        $path = (Get-Process -Id $PID).Path
        if ($path) { return $path }
    } catch {
        # 进程路径读不到时退回命令解析
    }
    foreach ($name in @('pwsh', 'powershell')) {
        $cmd = Get-Command $name -ErrorAction SilentlyContinue
        if ($cmd) { return $cmd.Source }
    }
    return 'powershell.exe'
}

# git 把进度与 remote 提示写到 stderr；EAP=Stop 下那会被当成终止性错误
# （build.ps1 对 cargo 也是同样的处理），故临时放宽，成败一律看退出码。
# 参数用显式 string[]：写成 ValueFromRemainingArguments 时，@('a','b') 会被当成
# 单个参数再按 $OFS 拼成一行，git 会把它当子命令名而报
# "git: 'rev-parse --abbrev-ref HEAD' is not a git command"（实测踩过）。
function Invoke-Git {
    param([string[]]$GitArgs)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & git -C $repoRoot @GitArgs 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    $lines = @($output | ForEach-Object { "$_" })
    if ($code -ne 0) {
        Fail ("git {0} 失败（exit {1}）：`n{2}" -f ($GitArgs -join ' '), $code, ($lines -join "`n"))
    }
    return , $lines
}

# 不中止的 git 调用：用于「失败也不影响结论」的探测（fetch / ls-remote）。
function Invoke-GitSoft {
    param([string[]]$GitArgs)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & git -C $repoRoot @GitArgs 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    return [pscustomobject]@{ Ok = ($code -eq 0); Lines = @($output | ForEach-Object { "$_" }) }
}

function Get-DeclaredVersions {
    $found = @()
    foreach ($target in $versionTargets) {
        $file = Join-Path $repoRoot $target.Path
        if (-not (Test-Path -LiteralPath $file)) {
            Fail "找不到版本号文件：$($target.Path)"
            continue
        }
        $text = [System.IO.File]::ReadAllText($file, $utf8NoBom)
        # 只取第一处匹配：Cargo.toml 里 windows crate 的依赖版本是独立的 version 行
        $match = [regex]::Match($text, $target.Pattern)
        if (-not $match.Success) {
            Fail "$($target.Name) 里没找到 version 字段"
            continue
        }
        $found += [pscustomobject]@{ Name = $target.Name; Version = $match.Groups[1].Value }
    }
    return , $found
}

function Get-ExeVersionInfo([string]$RelativePath) {
    $file = Join-Path $repoRoot $RelativePath
    if (-not (Test-Path -LiteralPath $file)) { return $null }
    $info = (Get-Item -LiteralPath $file).VersionInfo
    return [pscustomobject]@{ FileVersion = "$($info.FileVersion)"; ProductVersion = "$($info.ProductVersion)" }
}

# 发版说明校验：release-notes.ps1 用 exit 1 报告失败，放进子进程调用以免影响本会话；
# 用当前宿主 exe 的完整路径，不依赖 PATH（沙箱内也成立）。
function Test-ReleaseNotes([string]$Tag) {
    $script = Join-Path $repoRoot '.github\scripts\release-notes.ps1'
    if (-not (Test-Path -LiteralPath $script)) {
        Fail "找不到发版说明生成脚本：.github\scripts\release-notes.ps1"
        return $false
    }
    $psExe = Get-PowerShellExe
    $outFile = Join-Path $env:TEMP ("dsh-release-notes-{0}.github.md" -f $Tag)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $psExe -NoProfile -File $script -Tag $Tag -Mode github -OutFile $outFile 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    if ($code -ne 0) {
        Fail ("CHANGELOG.md 没有 $Tag 的合格小节——先补一节（格式见 docs/release.md，中文在前、`n" +
              "独占一行的 <!-- en --> 分隔、英文段必填）再做版本同步提交。`n{0}" -f (($output | ForEach-Object { "$_" }) -join "`n"))
        return $false
    }
    Write-Note "小节合格；发版说明预览：$outFile"
    return $true
}

function Test-LauncherNotRunning {
    $procs = @(Get-Process -Name 'dsh-launcher' -ErrorAction SilentlyContinue)
    if ($procs.Count -eq 0) {
        Write-Note '未检测到运行中的 dsh-launcher。'
        return $true
    }
    $lines = @($procs | ForEach-Object {
            $path = '(路径不可读)'
            try { $path = $_.Path } catch { }
            "PID $($_.Id) → $path"
        })
    if ($DryRun) {
        # 干跑不覆盖 exe，进程在跑不算未就绪，只作提醒
        Write-Note '[dry-run] 检测到 dsh-launcher 在运行（正式执行会在此中止）：'
        $lines | ForEach-Object { Write-Note "         $_" }
        return $true
    }
    Fail ("dsh-launcher 正在运行，它锁着 exe，覆盖会报 os error 5——请先从托盘退出再跑：`n" + ($lines -join "`n"))
    return $false
}

function Get-CurrentBranch {
    $lines = Invoke-Git @('rev-parse', '--abbrev-ref', 'HEAD')
    return ("$($lines[0])").Trim()
}

function Test-WorkTreeReady {
    $porcelain = Invoke-Git @('status', '--porcelain')
    # 未跟踪文件（?? 开头）不阻碍发版；CHANGELOG.md 的改动本来就是发版材料，
    # 留到第 6 步提交；其余已跟踪文件的改动一律挡下。
    $blocking = @($porcelain | Where-Object {
            $_ -ne '' -and $_ -notmatch '^\?\?' -and $_ -notmatch '^.{2}\s+CHANGELOG\.md$'
        })
    if ($blocking.Count -gt 0) {
        Fail ("工作区有未提交改动，发版脚本只处理 CHANGELOG.md 与版本号，其余请先提交或还原：`n" + ($blocking -join "`n"))
        return $false
    }
    Write-Note '工作区就绪（未跟踪文件与 CHANGELOG.md 改动不阻塞）。'
    return $true
}

function Enter-ReleaseBranch {
    $branch = Get-CurrentBranch
    if ($branch -eq 'HEAD') {
        Fail '仓库处于 detached HEAD，请先切回 main 再发版。'
        return $branch
    }
    if ($branch -eq $releaseBranch) {
        Write-Note "已在发版分支上（重跑场景）：$releaseBranch"
        return $branch
    }
    if ($branch -ne 'main') {
        Fail "当前分支是 $branch；发版请从 main 开始，脚本会自动切出 $releaseBranch。"
        return $branch
    }
    if ($DryRun) {
        Write-Note "[dry-run] 将从 main 切出发版分支：$releaseBranch"
        return $releaseBranch
    }
    Invoke-Git @('checkout', '-b', $releaseBranch) | Out-Null
    Write-Note "已从 main 切出发版分支：$releaseBranch"
    return $releaseBranch
}

function Commit-Paths([string]$Message, [string[]]$Paths) {
    $existing = @($Paths | Where-Object { Test-Path -LiteralPath (Join-Path $repoRoot $_) })
    if ($existing.Count -eq 0) {
        Write-Note "跳过提交（文件不存在）：$Message"
        return
    }
    if ($DryRun) {
        Write-Note "[dry-run] 将提交：$Message"
        Write-Note "          （$($existing -join '、')）"
        return
    }
    Invoke-Git (@('add', '--') + $existing) | Out-Null
    $staged = @(Invoke-Git @('diff', '--cached', '--name-only') | Where-Object { $_ -ne '' })
    if ($staged.Count -eq 0) {
        Write-Note "无变更，跳过提交：$Message"
        return
    }
    # 提交信息经 UTF-8 文件传入：-m 在非 UTF-8 代码页下会把中文写成乱码
    $msgFile = Join-Path $env:TEMP ("dsh-release-commit-{0}.txt" -f $tag)
    [System.IO.File]::WriteAllText($msgFile, $Message, $utf8NoBom)
    Invoke-Git @('commit', '-F', $msgFile) | Out-Null
    Remove-Item -LiteralPath $msgFile -Force -ErrorAction SilentlyContinue
    Write-Note "已提交：$Message"
}

function Assert-VersionConsistency([string]$Expected, [switch]$CheckExe) {
    $declared = Get-DeclaredVersions
    Write-Note ("三处版本号：" + (@($declared | ForEach-Object { "$($_.Name) = $($_.Version)" }) -join ' / '))
    $mismatched = @($declared | Where-Object { $_.Version -ne $Expected })
    if ($declared.Count -ne 3 -or $mismatched.Count -gt 0) {
        Fail "三处版本号必须恰好三处且都等于 $Expected（-Bump 未生效，或有一处漏改）。"
        return $false
    }
    Write-Note "✓ 三处版本号一致：$Expected"

    if (-not $CheckExe) { return $true }

    $pe = Get-ExeVersionInfo $binExeRel
    if ($null -eq $pe) {
        Fail "找不到产物：$binExeRel（先构建，或去掉 -SkipBuild）。"
        return $false
    }
    # PE 版本资源只放数字段：预发布版本（1.2.0-rc.1）在 exe 里是 1.2.0，故比对基础三段。
    $baseVersion = ($Expected -split '-')[0]
    Write-Note "exe PE 版本：FileVersion=$($pe.FileVersion) / ProductVersion=$($pe.ProductVersion)"
    foreach ($field in @([pscustomobject]@{ Name = 'FileVersion'; Value = $pe.FileVersion },
                         [pscustomobject]@{ Name = 'ProductVersion'; Value = $pe.ProductVersion })) {
        $actual = ([regex]::Match($field.Value, '^\d+\.\d+\.\d+')).Value
        if ($actual -ne $baseVersion) {
            Fail "exe 的 $($field.Name) 是 $($field.Value)，与目标 $Expected 不符——重建产物后再发版（不要用字符串搜 exe 核对版本号，PE 里是 UTF-16）。"
            return $false
        }
    }
    Write-Note "✓ exe PE 版本资源一致：$baseVersion"
    return $true
}

function Test-TagAbsent {
    # 本地检查只读、不联网，干跑也做（能提前发现「tag 已存在」这类冲突）
    $local = @(Invoke-Git @('tag', '--list', $tag))
    if (@($local | Where-Object { $_ -ne '' }).Count -gt 0) {
        Fail "本地已存在 tag $tag。要重发同一版本需先删除该 tag（本地与远端都要）。"
        return $false
    }
    if ($DryRun) {
        Write-Note "[dry-run] 本地无 tag $tag（远端需联网比对，正式执行时进行）。"
        return $true
    }
    $remote = Invoke-GitSoft @('ls-remote', '--tags', 'origin', "refs/tags/$tag")
    if ($remote.Ok -and @($remote.Lines | Where-Object { $_ -ne '' }).Count -gt 0) {
        Fail "远端已存在 tag $tag。"
        return $false
    }
    if (-not $remote.Ok) {
        Write-Note "（远端 tag 检查跳过：$($remote.Lines[0])）"
    }
    Write-Note "tag $tag 尚不存在。"
    return $true
}

# ---------------------------------------------------------------- 入口

if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$') {
    Fail "版本号格式不对：$Version（例：1.2.0 或 1.2.0-rc.1）"
}

Write-Host ''
Write-Host "dsh-tauri-launcher 发版 → $tag" -ForegroundColor Cyan
if ($TagOnly) { Write-Host '   第二段：打 tag 并推送' -ForegroundColor DarkGray }
elseif ($DryRun) { Write-Host '   干跑：不构建、不复制、不提交、不打 tag、不推送' -ForegroundColor DarkGray }

if (-not (Test-Path -LiteralPath (Join-Path $repoRoot '.github\scripts\release-notes.ps1'))) {
    Fail "找不到 .github\scripts\release-notes.ps1——本脚本需放在 launcher/ 下运行（当前推断的仓库根：$repoRoot）。"
}

# ================================================================ 第二段

if ($TagOnly) {
    Write-Step '1/5 分支与工作区'
    $branch = Get-CurrentBranch
    if ($branch -eq 'main') { Write-Note '当前在 main 上。' }
    else { Fail "打 tag 必须在 main 上（当前 $branch）——tag 只从 main 打，否则 release.yml 发布的是分支提交。" }
    Test-WorkTreeReady | Out-Null

    Write-Step '2/5 发版说明'
    Test-ReleaseNotes -Tag $tag | Out-Null

    Write-Step '3/5 版本号与产物'
    if ($SkipBuild) { Write-Note '按 -SkipBuild 跳过重建（只核验现有产物）。' }
    Assert-VersionConsistency -Expected $Version -CheckExe | Out-Null

    Write-Step '4/5 tag 是否已存在'
    Test-TagAbsent | Out-Null

    Write-Step '5/5 打 tag 并推送'
    if ($DryRun) {
        Write-Note "[dry-run] 将执行：git tag $tag"
        Write-Note "[dry-run] 将执行：git push origin $tag"
    } else {
        $sync = Invoke-GitSoft @('fetch', 'origin', 'main')
        if ($sync.Ok) {
            $local = ("$((Invoke-Git @('rev-parse', 'HEAD'))[0])").Trim()
            $remoteMain = ("$((Invoke-Git @('rev-parse', 'origin/main'))[0])").Trim()
            if ($local -eq $remoteMain) { Write-Note "本地 main 与 origin/main 一致（$local）。" }
            else { Fail "main 与 origin/main 不一致（本地 $local / 远端 $remoteMain）——先 git pull 再打 tag。" }
        } else {
            Write-Note "（fetch 失败，跳过与 origin/main 的比对：$($sync.Lines[0])）"
        }

        if (-not (Confirm-Action "将打 tag $tag 并推送（推送即触发 release.yml 发布），继续？")) {
            Fail '用户取消打 tag。'
        }
        Invoke-Git @('tag', $tag) | Out-Null
        Write-Note "已打 lightweight tag：$tag"
        if ($NoPush) {
            Write-Note '已按 -NoPush 跳过推送（tag 已在本地）。'
        } else {
            Invoke-Git @('push', 'origin', $tag) | Out-Null
            Write-Note "已推送 tag：$tag"
        }
    }

    Write-Host ''
    Write-Host '完成。tag 推送后 release.yml 会自动：生成发版说明 → cargo test → 构建 → 发布 GitHub Release → 同步 Gitee 发行版。' -ForegroundColor Green
    Write-Host 'Gitee 侧若提示 tag 未同步，先在 Gitee 执行「强制同步」再重跑该工作流（或手动 dispatch release-notes 补正文）。' -ForegroundColor DarkGray
    Write-Host 'npm publish 不在脚本范围内（账号 2FA），需要时手动执行。' -ForegroundColor DarkGray
    if ($DryRun -and $unready.Count -gt 0) {
        Write-Host ''
        Write-Host ("[dry-run] 结束：{0} 项未就绪（见上）。" -f $unready.Count) -ForegroundColor Yellow
        exit 1
    }
    if ($DryRun) { Write-Host ''; Write-Host '[dry-run] 结束：检查就绪。' -ForegroundColor Green }
    return
}

# ================================================================ 第一段

Write-Step "1/7 校验发版说明（CHANGELOG.md 的 $tag 小节）"
Test-ReleaseNotes -Tag $tag | Out-Null

Write-Step '2/7 环境检查'
Test-LauncherNotRunning | Out-Null
Enter-ReleaseBranch | Out-Null
Test-WorkTreeReady | Out-Null
Test-TagAbsent | Out-Null

Write-Step "3/7 同步版本号并重建（build.ps1 -Bump $Version）"
$declaredNow = Get-DeclaredVersions
$needsBump = @($declaredNow | Where-Object { $_.Version -ne $Version }).Count -gt 0
$buildScript = Join-Path $PSScriptRoot 'build.ps1'
if ($SkipBuild -and $needsBump -and -not $DryRun) {
    Fail "-SkipBuild 要求三处版本号已是 $Version（当前 $(@($declaredNow | ForEach-Object { $_.Version }) -join ' / ')）——先跑一次 launcher/build.ps1 -Bump $Version，或去掉 -SkipBuild。"
}
if ($SkipBuild) {
    Write-Note '按 -SkipBuild 跳过重建，直接核验现有产物。'
} elseif ($DryRun) {
    if ($needsBump) { Write-Note "[dry-run] 将执行：launcher/build.ps1 -Bump $Version（同步三处版本号 + cargo build --release）" }
    else { Write-Note "[dry-run] 将执行：launcher/build.ps1（版本号已同步，不带 -Bump）" }
    if ($Offline) { Write-Note '           （透传 -Offline）' }
    if ($CargoHome) { Write-Note "           （透传 -CargoHome $CargoHome）" }
} else {
    Write-Note '构建中（cargo build --release），可能需要几分钟…'
    $buildArgs = @{}
    if ($needsBump) { $buildArgs.Bump = $Version }
    if ($Offline) { $buildArgs.Offline = $true }
    if ($CargoHome) { $buildArgs.CargoHome = $CargoHome }
    & $buildScript @buildArgs
    Write-Note "构建完成：$builtExeRel"
}

Write-Step '4/7 同步 launcher/bin 产物'
if ($SkipBuild) {
    Write-Note '按 -SkipBuild 跳过产物复制。'
} elseif ($DryRun) {
    Write-Note "[dry-run] 将复制：$builtExeRel → $binExeRel"
} else {
    Copy-Item -LiteralPath (Join-Path $repoRoot $builtExeRel) -Destination (Join-Path $repoRoot $binExeRel) -Force
    Write-Note "已同步：$binExeRel"
}

Write-Step '5/7 判据核验（三处版本号 + PE 版本资源）'
if ($DryRun) {
    $declared = Get-DeclaredVersions
    Write-Note ("当前三处版本号：" + (@($declared | ForEach-Object { "$($_.Name) = $($_.Version)" }) -join ' / '))
    Write-Note "（第 3 步会同步为 $Version；正式执行到这里会断言三处完全一致）"
    $pe = Get-ExeVersionInfo $binExeRel
    if ($null -eq $pe) { Write-Note "（$binExeRel 尚不存在）" }
    else { Write-Note "当前 exe PE 版本：FileVersion=$($pe.FileVersion) / ProductVersion=$($pe.ProductVersion)（重建后应等于 $Version）" }
} else {
    Assert-VersionConsistency -Expected $Version -CheckExe | Out-Null
}

Write-Step '6/7 分类型提交'
Commit-Paths -Message "docs: CHANGELOG 补 $tag 小节" -Paths @('CHANGELOG.md')
Commit-Paths -Message "chore: 版本同步 $Version（三处版本号 + 重建 bin exe）" -Paths @(
    'package.json',
    'launcher\src-tauri\Cargo.toml',
    'launcher\src-tauri\tauri.conf.json',
    'launcher\src-tauri\Cargo.lock',
    $binExeRel
)

Write-Step '7/7 推送发版分支并给出 PR 入口'
$prUrl = ''
if ($NoPush) {
    Write-Note "已按 -NoPush 跳过推送（本地提交已保留在 $releaseBranch）。"
} elseif ($DryRun) {
    Write-Note "[dry-run] 将执行：git push -u origin $releaseBranch"
} else {
    if (-not (Confirm-Action "将推送分支 $releaseBranch 到 origin（触发 CI），继续？")) {
        Fail '用户取消推送。本地提交已保留，确认后手动 push 即可。'
    }
    $pushOut = Invoke-Git @('push', '-u', 'origin', $releaseBranch)
    foreach ($line in $pushOut) {
        $match = [regex]::Match("$line", 'https://[^\s]+/pull/new/[^\s]+')
        if ($match.Success) { $prUrl = $match.Value }
    }
    if (-not $prUrl) {
        $remoteLines = Invoke-Git @('remote', 'get-url', 'origin')
        $remoteUrl = ("$($remoteLines[0])").Trim()
        if ($remoteUrl -match 'github\.com[:/](.+?)(\.git)?$') {
            $prUrl = "https://github.com/$($Matches[1])/pull/new/$releaseBranch"
        }
    }
    Write-Note '已推送发版分支。'
}

Write-Host ''
if ($DryRun) {
    if ($unready.Count -gt 0) {
        Write-Host ("[dry-run] 结束：{0} 项未就绪（见上）。" -f $unready.Count) -ForegroundColor Yellow
        exit 1
    }
    Write-Host '[dry-run] 结束：检查就绪，去掉 -DryRun 即可正式执行。' -ForegroundColor Green
    return
}

Write-Host '下一步（人工）：' -ForegroundColor Green
if ($prUrl) { Write-Host "  1. 打开链接创建 PR：$prUrl" } else { Write-Host '  1. 推送后按 git 输出里的链接创建 PR。' }
Write-Host '  2. 等 CI verify 绿灯 → Rebase and merge 合入 main（保护要求，脚本不代替）。'
Write-Host "  3. 回 main 拉取后跑第二段：pwsh -File launcher/release.ps1 -Version $Version -TagOnly"
Write-Host '  4. tag 推送后 release.yml 自动构建并发布 GitHub Release + Gitee 发行版。'
Write-Host '  5. npm publish 由你手动执行（账号 2FA）。' -ForegroundColor DarkGray
