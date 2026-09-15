/**
 * dsh-tauri-launcher — host half.
 *
 * 在 DSH Web 设置中启动/退出本机 DeepSeek Harness Tauri 桌面应用，并联动
 * 桌面快捷方式（开启自动创建、关闭自动删除、可手动补建）。与桌面应用的
 * 协作协议（launcher exe 同目录）：
 *   - `.dsh-heartbeat`：桌面应用每秒写入的 Unix 时间戳，freshSecs 秒内新鲜视为运行中；
 *   - `.dsh-quit`：内容 `1` 且 60 秒内新鲜 → 桌面应用仅退出自身（消费时自删）。
 *
 * 浏览器侧通过本插件注册的 /api/dsh-tauri-launcher/* 路由通信（仅回环）。
 *
 * 标记协议以 **exe 同目录**为界，而桌面应用可能有多个副本（包内预编译 exe、
 * 本地构建产物）。因此候选目录按优先级自动探测：行配置 launcherExe →
 * 运行中实例目录 → 桌面快捷方式目标目录 → 行配置 launcherDirs →
 * 本会话发现过的目录 → 包内 `launcher/bin`，见 baseDirs。
 *
 * 可调参数全部走行配置（见 apply 的 config 处理与 README「配置」章节）；
 * 内置候选目录只是默认值，供未配置时自动探测。
 */

import { fileURLToPath } from 'node:url'

export const name = 'desktop-launcher'

/** 路由注册与文件/进程操作所需的主机服务（声明后加载器会等待就绪再 apply）。 */
export const inject = ['webServer', 'fs', 'subprocess']

const STDIO = { stdin: 'ignore', stdout: 'inherit', stderr: 'inherit' }

/**
 * 拉起桌面应用时使用的 stdio（**不能用 inherit**）。
 *
 * DSH 自身可能是被桌面应用拉起的：它的 stdout/stderr 就是桌面应用当时创建的管道
 * （`harness.rs` 用 `Stdio::piped()` 拉起 `dsh web`）。桌面应用退出后这根管道的读端
 * 消失，此后以 inherit 拉起的子进程继承的是**已断开的手柄**，一写即 panic 退出
 * （2026-09-15 实测：exit code 101，现象为「设置里开关打开但桌面端起不来」，而关闭
 * 方向正常——关闭只写标记文件）。收集模式让管道由 DSH 这一侧持有，既避开坏句柄，
 * 又把桌面应用启动期的输出留下来供诊断使用。
 */
const APP_STDIO = { stdin: 'ignore', stdout: { maxBytes: 16384 }, stderr: { maxBytes: 16384 } }

/** 本包随附的预编译桌面应用 exe 目录（无论安装到哪个 profile 都能定位）。 */
const PACKAGE_BIN = fileURLToPath(new URL('../launcher/bin', import.meta.url))

/** 未配置 launcherDirs 时的默认候选目录（仅包内随附的预编译 exe；本地自行构建的 exe 通过行配置 launcherDirs/launcherExe 指定）。 */
const DEFAULT_DIRS = [PACKAGE_BIN]

/**
 * 运行中实例 / 桌面快捷方式目标的探测缓存时长（毫秒）。状态轮询与启停等待
 * 循环都会反复取候选目录，探测结果按此窗口复用，避免每次读取都拉起 PowerShell。
 */
const DISCOVERY_TTL_MS = 5000

/** 会话内累积的候选目录上限（含已退出实例的目录）。 */
const MAX_KNOWN_DIRS = 8

/**
 * PowerShell 输出编码前置：Windows PowerShell 5.1 默认按控制台代码页写重定向
 * 输出，非 ASCII 路径会乱码；`ctx.subprocess` 的收集模式按 UTF-8 解码，故显式
 * 钉成 UTF-8（与 `@deepseek-ai/dsh-pwsh-local` 的 ENCODING_PREAMBLE 同做法）。
 */
const PS_UTF8_PREAMBLE = '[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [System.Text.UTF8Encoding]::new($false); '

/** 列出运行中的桌面应用 exe 路径（同名 exe 可能有多份副本同时在跑）。 */
const RUNNING_EXE_SCRIPT =
  'Get-Process -Name dsh-launcher -ErrorAction SilentlyContinue | ForEach-Object { try { $_.Path } catch {} }'

/**
 * 桌面快捷方式文件名。**单一来源**：必须与桌面应用 `src-tauri/src/settings.rs`
 * 的 `SHORTCUT_NAME` 常量一致——两侧各自维护名字会导致插件与桌面应用各建
 * 一个 .lnk（优化报告 v2 B3），因此不再提供行配置。
 */
const SHORTCUT_NAME = 'DeepSeek Harness.lnk'

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const psQuote = (value) => "'" + String(value).replace(/'/g, "''") + "'"

/** exe 所在目录（Windows 反斜杠路径）；无分隔符时返回空串（调用方回退）。 */
const dirOf = (exePath) => {
  const slash = exePath.lastIndexOf('\\')
  return slash > 0 ? exePath.slice(0, slash) : ''
}

function writeJson(res, status, body) {
  const payload = JSON.stringify(body)
  res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' })
  res.end(payload)
}

/** 请求体大小上限（回环接口的防御性约束）。 */
const MAX_BODY_BYTES = 1024 * 1024

async function readJsonBody(req) {
  const chunks = []
  let total = 0
  for await (const chunk of req) {
    total += chunk.length
    if (total > MAX_BODY_BYTES) return {}
    chunks.push(chunk)
  }
  try {
    const parsed = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    return typeof parsed === 'object' && parsed !== null ? parsed : {}
  } catch {
    return {}
  }
}

/** 仅允许本机回环请求（与 dsh-ssh 相同的信任边界）。 */
function isLoopback(req) {
  const address = String(req.socket.remoteAddress ?? '')
  return address === '127.0.0.1' || address === '::1' || address === '::ffff:127.0.0.1'
}

/**
 * CSRF 防线（#4）：回环之外直接拒绝；回环内还须满足任一——
 * 1. 携带本插件约定的自定义头（同源 fetch 可带，跨站 form POST 无法伪造）；
 * 2. Origin 为 DSH 自身页面地址（http://127.0.0.1|localhost|[::1]:port）；
 * 3. Referer 指向 DSH 自身页面地址。
 * 三者全缺（如 <img>/<script> 等无头请求）拒绝。
 */
/** 允许的 DSH 页面 Origin 集合（按端口缓存，避免每个请求重建 Set）。 */
const allowedOriginsByPort = new Map()
function allowedOriginsFor(port) {
  let set = allowedOriginsByPort.get(port)
  if (set === undefined) {
    set = new Set([
      'http://127.0.0.1:' + port,
      'http://localhost:' + port,
      'http://[::1]:' + port,
    ])
    allowedOriginsByPort.set(port, set)
  }
  return set
}

function isTrustedRequest(req, port) {
  if (!isLoopback(req)) return false
  const headers = req.headers || {}
  if (headers['x-requested-with'] === 'dsh-tauri-launcher') return true
  const allowed = allowedOriginsFor(port)
  if (headers.origin && allowed.has(headers.origin)) return true
  const referer = headers.referer
  if (referer) {
    const base = referer.split('/').slice(0, 3).join('/')
    if (allowed.has(base)) return true
  }
  return false
}

export function apply(ctx, config) {
  // 注入的服务由加载器保证在 apply 前就绪（不能在 apply 时用 ctx.get 提前
  // 捕获未就绪的服务，否则会永久拿到 undefined）。
  const fs = ctx.fs
  const subprocess = ctx.subprocess
  const sandboxPolicy = ctx.get('sandboxPolicy')

  const resolved = {
    /** 直接指定桌面应用 exe 路径；空则按 launcherDirs/内置候选自动探测。 */
    launcherExe: config && typeof config.launcherExe === 'string' ? config.launcherExe : '',
    /** 候选 exe 目录列表；为空时使用内置默认候选。 */
    launcherDirs: Array.isArray(config && config.launcherDirs) ? config.launcherDirs.filter((d) => typeof d === 'string') : [],
    /** 心跳“新鲜窗口”（秒）。桌面应用每秒写心跳，默认 4 秒即 4 个周期的余量。 */
    freshSecs: config && typeof config.freshSecs === 'number' && config.freshSecs > 1 ? config.freshSecs : 4,
  }

  let lastFsError = ''
  let lastMarkerError = ''
  let lastProbeError = ''
  let spawnHandle = null
  let shortcutCache = { t: 0, value: false }
  /** 最近一次拉起桌面应用的退出事实（未退出为 null），用于把「起来即死」报出来。 */
  let appExit = null
  /** 该桌面应用进程启动期的输出（收集模式），仅在启动失败时用于诊断。 */
  let appOutput = ''

  /**
   * 已在本次插件运行中发现过的候选目录（最近使用在前）。实例退出后仍保留：
   * 该目录里的心跳文件会变成「过期」而非「不存在」，让状态稳定落在
   * 「已停止」而不是回跳到 null（未知），退出流程的确认逻辑也不会中途丢目标。
   */
  const knownDirs = []
  /** 「运行中实例目录 + 桌面快捷方式目标目录」的探测缓存。 */
  const discovery = { t: 0, runningDirs: [], shortcutDir: '' }

  /**
   * 运行采集模式 PowerShell 并取回 stdout 文本（短探测脚本）。
   * 任何失败（服务缺失、spawn 失败、provider 未提供收集流）都返回空串，
   * 调用方据此回退到既有候选目录——探测不可用不改变原有行为。
   */
  async function capturePowerShell(script) {
    if (!subprocess) return ''
    try {
      const handle = subprocess.spawn({
        argv: ['powershell', '-NoLogo', '-NoProfile', '-NonInteractive', '-Command', PS_UTF8_PREAMBLE + script],
        cwd: process.cwd(),
        stdio: { stdin: 'ignore', stdout: { maxBytes: 16384 }, stderr: { maxBytes: 4096 } },
        graceMs: 3000,
      })
      if (handle && handle.done) await handle.done.catch(() => null)
      const reader = handle && handle.collected ? handle.collected.stdout : null
      if (!reader) return ''
      return String(reader.readFrom(0).text || '')
    } catch (error) {
      lastProbeError = String((error && error.message) || error)
      return ''
    }
  }

  /** 收集模式下该句柄的输出（stdout/stderr 合并、限长），用于诊断「起来即死」。 */
  function collectedText(handle) {
    if (!handle || !handle.collected) return ''
    const parts = []
    for (const stream of ['stdout', 'stderr']) {
      const reader = handle.collected[stream]
      if (!reader) continue
      try {
        const text = String(reader.readFrom(0).text || '').trim()
        if (text) parts.push(stream + ': ' + text)
      } catch {
        // 收集流不可读取时忽略：诊断信息缺一行不影响主流程。
      }
    }
    return parts.join('\n').slice(0, 2000)
  }

  /** 退出事实的可读描述（exit code / signal / 未知原因）。 */
  function describeExit(outcome) {
    if (!outcome) return '未知原因'
    if (typeof outcome.exitCode === 'number') return 'exit code ' + outcome.exitCode
    if (outcome.signal) return 'signal ' + outcome.signal
    return outcome.error ? String(outcome.error) : '未知原因'
  }

  /** 记住一个已发现的候选目录（去重、最近使用在前、限量）。 */
  function rememberDir(dir) {
    if (!dir) return
    const hit = knownDirs.indexOf(dir)
    if (hit >= 0) knownDirs.splice(hit, 1)
    knownDirs.unshift(dir)
    if (knownDirs.length > MAX_KNOWN_DIRS) knownDirs.length = MAX_KNOWN_DIRS
  }

  /**
   * 探测正在运行的桌面应用目录与桌面快捷方式目标目录（TTL 内复用）。
   *
   * 为什么需要：标记协议以 exe 同目录为界，而桌面应用存在多份副本（包内预编译
   * exe、本地构建产物）。只看包内目录时，若实际运行的是另一份副本，就会读到空
   * 心跳 → 状态「未知」、退出标记写进错误目录、开关反而拉起第二份实例。
   */
  async function probe() {
    if (Date.now() - discovery.t < DISCOVERY_TTL_MS) return discovery
    const [runningText, targetText] = await Promise.all([
      capturePowerShell(RUNNING_EXE_SCRIPT),
      capturePowerShell(shortcutTargetScript()),
    ])
    const runningDirs = []
    for (const line of String(runningText).split(/\r?\n/)) {
      const dir = dirOf(line.trim())
      if (dir && !runningDirs.includes(dir)) runningDirs.push(dir)
    }
    const targetLine = String(targetText).split(/\r?\n/).map((line) => line.trim()).filter((line) => line !== '')[0] || ''
    discovery.t = Date.now()
    discovery.runningDirs = runningDirs
    discovery.shortcutDir = dirOf(targetLine)
    for (const dir of runningDirs) rememberDir(dir)
    if (discovery.shortcutDir) rememberDir(discovery.shortcutDir)
    return discovery
  }

  /**
   * 候选 exe 目录，按启动优先级排列：
   * 1. 行配置 launcherExe 所在目录；2. 运行中实例目录；3. 桌面快捷方式目标目录；
   * 4. 行配置 launcherDirs；5. 本会话发现过的目录；6. 包内随附 exe 目录。
   * 只有真的含 dsh-launcher.exe 的目录才会被 exeDirs() 采用——失效的快捷方式
   * 目标会被自动跳过，回退到下一候选。
   */
  async function baseDirs() {
    const found = await probe()
    const list = []
    if (resolved.launcherExe) {
      const dir = dirOf(resolved.launcherExe)
      if (dir) list.push(dir)
    }
    for (const dir of found.runningDirs) list.push(dir)
    if (found.shortcutDir) list.push(found.shortcutDir)
    for (const dir of resolved.launcherDirs) list.push(dir)
    for (const dir of knownDirs) list.push(dir)
    for (const dir of DEFAULT_DIRS) list.push(dir)
    return [...new Set(list)]
  }

  async function exeExists(dir) {
    if (!fs) return false
    try {
      const target = await fs.resolve(dir + '\\dsh-launcher.exe')
      const info = await fs.stat(target)
      return Boolean(info && info.type === 'file')
    } catch (error) {
      lastFsError = String((error && error.message) || error)
      return false
    }
  }

  async function exeDirs() {
    const dirs = await baseDirs()
    const checked = await Promise.all(dirs.map(async (dir) => ((await exeExists(dir)) ? dir : null)))
    return checked.filter((dir) => dir !== null)
  }

  async function fileText(dir, name) {
    if (!fs) return null
    try {
      const target = await fs.resolve(dir + '\\' + name)
      const info = await fs.stat(target)
      if (!info) return null
      const text = await fs.readText(target)
      return String(text).trim()
    } catch (error) {
      lastFsError = String((error && error.message) || error)
      return null
    }
  }

  async function hbState(dir) {
    const text = await fileText(dir, '.dsh-heartbeat')
    if (text === null || text === '') return null
    const stamp = Number(text)
    if (!Number.isFinite(stamp)) return null
    // 单向新鲜度：只认「不早于 now - freshSecs、且不显著超前」的时间戳。
    // 原实现取 Math.abs()，会把**未来时间戳**也判成新鲜——系统时钟回拨或
    // 写入方时钟超前时，残留的心跳文件会被误判为「运行中」（优化报告 v2 C1）。
    // 容 1 秒负向抖动：同一台机器共用系统时钟，正常不会超前。
    const age = Date.now() / 1000 - stamp
    return age > -1 && age < resolved.freshSecs
  }

  async function isRunning() {
    const dirs = await exeDirs()
    const states = []
    for (const dir of dirs) states.push(await hbState(dir))
    if (states.includes(true)) return true
    if (states.length === 0 || states.every((s) => s === null)) return null
    return false
  }

  async function waitFresh(seconds) {
    let sawFile = false
    for (let i = 0; i < seconds * 2; i++) {
      const state = await isRunning()
      if (state === true) return true
      if (state === false) sawFile = true
      await sleep(500)
    }
    return sawFile ? false : null
  }

  async function waitGone(seconds) {
    for (let i = 0; i < seconds * 2; i++) {
      const state = await isRunning()
      if (state === false) return true
      if (state === null) return null
      await sleep(500)
    }
    return false
  }

  async function pickExe() {
    if (resolved.launcherExe) {
      try {
        const target = await fs.resolve(resolved.launcherExe)
        const info = await fs.stat(target)
        if (info && info.type === 'file') return resolved.launcherExe
      } catch (error) {
        lastFsError = String((error && error.message) || error)
      }
    }
    const dirs = await exeDirs()
    if (dirs.length > 0) return dirs[0] + '\\dsh-launcher.exe'
    return null
  }

  /**
   * 执行 PowerShell 脚本。
   * - 默认（`wantExitCode: false`）：等待执行结束，忽略退出码；spawn 失败抛出，
   *   由调用方决定是否记录（写标记等「执行即可」的场景）。
   * - `wantExitCode: true`：返回退出码（无法取得时为 null），失败记入
   *   `lastMarkerError` 供诊断使用（原 runPowerShellExitCode 的语义）。
   */
  async function runPowerShell(script, cwd, { wantExitCode = false } = {}) {
    if (!subprocess) {
      if (wantExitCode) return null
      throw new Error('subprocess 服务不可用')
    }
    try {
      const handle = subprocess.spawn({
        argv: ['powershell', '-NoProfile', '-NonInteractive', '-Command', script],
        // 不带 cwd 时：需要退出码的调用多为绝对路径脚本，用进程 cwd 即可；
        // 其余（写标记/杀进程）落在 exe 同目录，便于相对路径与权限。
        cwd: cwd || (wantExitCode ? process.cwd() : ((await pickExe()) || '')),
        stdio: STDIO,
        graceMs: 3000,
      })
      const outcome = handle && handle.done ? await handle.done.catch(() => null) : null
      if (!wantExitCode) return undefined
      return outcome && typeof outcome.exitCode === 'number' ? outcome.exitCode : null
    } catch (error) {
      if (!wantExitCode) throw error
      lastMarkerError = String((error && error.message) || error)
      return null
    }
  }

  async function quitScriptFor(value) {
    const exe = await pickExe()
    if (!exe) return null
    const dir = dirOf(exe)
    return {
      dir,
      // dir/value 一律经 psQuote 转义（单引号翻倍），避免含单引号的路径破坏脚本。
      script: 'Set-Content -LiteralPath ' + psQuote(dir + '\\.dsh-quit') + ' -Value ' + psQuote(value) + ' -NoNewline',
    }
  }

  /**
   * 写退出标记（`.dsh-quit`）。
   * - `wait: true`（默认）：等待 PowerShell 结束，成功/失败都记入 `lastMarkerError`；
   * - `wait: false`：即发即忘（退出收尾阶段不为写标记增加延迟）。
   * 原 writeQuitMarkerNoWait 与本函数仅此一点差异，合并后由参数区分（v2 C3）。
   */
  async function writeQuitMarker(value, { wait = true } = {}) {
    const job = await quitScriptFor(value)
    if (!job) {
      if (wait) lastMarkerError = 'writeQuitMarker: no exe dir'
      return
    }
    if (!wait) {
      try {
        subprocess.spawn({
          argv: ['powershell', '-NoProfile', '-NonInteractive', '-Command', job.script],
          cwd: job.dir,
          stdio: STDIO,
          graceMs: 3000,
        })
      } catch (error) {
        lastMarkerError = String((error && error.message) || error)
      }
      return
    }
    try {
      await runPowerShell(job.script, job.dir)
      lastMarkerError = ''
    } catch (error) {
      lastMarkerError = String((error && error.message) || error)
    }
  }

  async function quitFileGone() {
    const exe = await pickExe()
    if (!exe) return true
    const dir = dirOf(exe)
    return (await fileText(dir, '.dsh-quit')) === null
  }

  async function quitMarkerIsOne() {
    const exe = await pickExe()
    if (!exe) return false
    const dir = dirOf(exe)
    return (await fileText(dir, '.dsh-quit')) === '1'
  }

  function shortcutExistsScript() {
    return "$desktop=[Environment]::GetFolderPath('Desktop'); $lnk=Join-Path $desktop " + psQuote(SHORTCUT_NAME) + "; if (Test-Path -LiteralPath $lnk) { exit 0 } else { exit 1 }"
  }

  /** 读桌面快捷方式的目标 exe 路径；快捷方式不存在或目标为空时无输出。 */
  function shortcutTargetScript() {
    return "$desktop=[Environment]::GetFolderPath('Desktop'); $lnk=Join-Path $desktop " + psQuote(SHORTCUT_NAME) + '; ' +
      'if (Test-Path -LiteralPath $lnk) { $ws=New-Object -ComObject WScript.Shell; $sc=$ws.CreateShortcut($lnk); if ($sc.TargetPath) { $sc.TargetPath } }'
  }

  function shortcutCreateScript(exe) {
    return "$desktop=[Environment]::GetFolderPath('Desktop'); $lnk=Join-Path $desktop " + psQuote(SHORTCUT_NAME) + "; " +
      "$exe=" + psQuote(exe) + "; " +
      '$ws=New-Object -ComObject WScript.Shell; $sc=$ws.CreateShortcut($lnk); ' +
      '$sc.TargetPath=$exe; $sc.WorkingDirectory=Split-Path $exe; ' +
      "$sc.IconLocation=($exe + ',0'); $sc.Description=" + psQuote('DeepSeek Harness 桌面启动器') + '; ' +
      '$sc.Save(); if (Test-Path -LiteralPath $lnk) { exit 0 } else { exit 1 }'
  }

  function shortcutDeleteScript() {
    return "$desktop=[Environment]::GetFolderPath('Desktop'); $lnk=Join-Path $desktop " + psQuote(SHORTCUT_NAME) + '; ' +
      'Remove-Item -LiteralPath $lnk -Force -ErrorAction SilentlyContinue; ' +
      'if (Test-Path -LiteralPath $lnk) { exit 1 } else { exit 0 }'
  }

  async function shortcutExists() {
    const now = Date.now()
    if (now - shortcutCache.t < 5000) return shortcutCache.value
    const code = await runPowerShell(shortcutExistsScript(), process.cwd(), { wantExitCode: true })
    shortcutCache = { t: Date.now(), value: code === 0 }
    return shortcutCache.value
  }

  async function ensureShortcut() {
    if (await shortcutExists()) return true
    const exe = await pickExe()
    if (!exe) return false
    const code = await runPowerShell(shortcutCreateScript(exe), process.cwd(), { wantExitCode: true })
    shortcutCache = { t: Date.now(), value: code === 0 }
    return code === 0
  }

  async function removeShortcut() {
    if (!(await shortcutExists())) return true
    const code = await runPowerShell(shortcutDeleteScript(), process.cwd(), { wantExitCode: true })
    shortcutCache = { t: Date.now(), value: code !== 0 }
    return code === 0
  }

  async function buildDiag() {
    const lines = []
    lines.push('services: fs=' + !!fs + ' subprocess=' + !!subprocess + ' sandboxPolicy=' + !!sandboxPolicy)
    lines.push('workspaceRoot: ' + (sandboxPolicy ? sandboxPolicy.workspaceRoot : '(none)'))
    lines.push('freshWindowSecs: ' + resolved.freshSecs)
    lines.push('launcherExe: ' + (resolved.launcherExe || '(auto)'))
    lines.push('shortcut: ' + (await shortcutExists()))
    const found = await probe()
    lines.push('runningDirs: ' + JSON.stringify(found.runningDirs))
    lines.push('shortcutTarget: ' + (found.shortcutDir || '(none)'))
    const dirs = await exeDirs()
    lines.push('exeDirs: ' + JSON.stringify(dirs))
    const exe = await pickExe()
    lines.push('pickExe: ' + (exe || '(null)'))
    if (lastFsError) lines.push('fsError: ' + lastFsError)
    if (lastProbeError) lines.push('probeError: ' + lastProbeError)
    lines.push('markerWrite: ' + (lastMarkerError || 'ok'))
    if (appExit) lines.push('appExit: ' + describeExit(appExit))
    if (appOutput) lines.push('appOutput: ' + appOutput)
    for (const dir of dirs) {
      lines.push('heartbeat(' + dir.split('\\').slice(-2).join('\\') + '): ' + String(await hbState(dir)))
    }
    if (dirs.length === 0) lines.push('heartbeat: (no exe dirs)')
    return lines.join('\n')
  }

  async function getState() {
    const running = await isRunning()
    const exe = await pickExe()
    const shortcut = await shortcutExists()
    return { ok: true, desktop: running, shortcut, exe: exe || null }
  }

  async function getDiag() {
    return { ok: true, diag: await buildDiag() }
  }

  async function setDesktop(enabled) {
    appExit = null
    appOutput = ''
    if (enabled) {
      if (await quitMarkerIsOne()) await writeQuitMarker('0')
      const exe = await pickExe()
      if (!exe) {
        return { ok: false, error: '未找到桌面应用可执行文件（可通过行配置 launcherExe 或 launcherDirs 指定）。', diag: await buildDiag() }
      }
      if (!subprocess) {
        return { ok: false, error: 'subprocess 服务不可用，无法启动桌面应用。', diag: await buildDiag() }
      }
      try {
        const dir = dirOf(exe)
        // 拉起前记住该目录：即使探测缓存仍是旧的，心跳扫描也已覆盖新实例的目录。
        rememberDir(dir)
        const handle = subprocess.spawn({ argv: [exe], cwd: dir, stdio: APP_STDIO, graceMs: 3000 })
        spawnHandle = handle
        // 记录退出事实：桌面应用「起来即死」时（例如继承了坏句柄而 panic），
        // 只有退出码 + 它自己的输出能说明原因，否则用户只看到开关弹回而没有解释。
        if (handle && handle.done) {
          handle.done.then(
            (outcome) => { appExit = outcome || { exitCode: null, signal: null } },
            (error) => { appExit = { exitCode: null, signal: null, error: String((error && error.message) || error) } },
          )
        }
      } catch (error) {
        return { ok: false, error: '启动桌面应用失败：' + String((error && error.message) || error), diag: await buildDiag() }
      }
      const state = await waitFresh(20)
      if (state === true) {
        if (!(await ensureShortcut())) {
          lastMarkerError = 'shortcut sync failed (create)'
          return {
            ok: false,
            desktop: true,
            shortcut: false,
            error: '桌面应用已启动，但创建桌面快捷方式失败（可稍后在设置中手动重试）。',
            diag: await buildDiag(),
          }
        }
        return { ok: true, desktop: true, shortcut: true, diag: '' }
      }
      // 未在就绪窗口内看到新鲜心跳：若进程已退出，把退出事实与输出报出来。
      appOutput = collectedText(spawnHandle)
      if (appExit) {
        return {
          ok: false,
          desktop: false,
          shortcut: await shortcutExists(),
          error: '桌面应用启动后立即退出（' + describeExit(appExit) + '）。',
          diag: await buildDiag(),
        }
      }
      if (state === null) return { ok: true, desktop: null, shortcut: await shortcutExists(), diag: '' }
      return { ok: true, desktop: false, shortcut: await shortcutExists(), diag: '' }
    }

    const owned = spawnHandle
    spawnHandle = null
    if (owned) {
      try { owned.terminate() } catch {}
    }
    await writeQuitMarker('1')
    let confirmed = false
    for (let i = 0; i < 12; i++) {
      if (await quitFileGone()) { confirmed = true; break }
      if ((await isRunning()) === false) { confirmed = true; break }
      await sleep(500)
    }
    if (confirmed) {
      if (owned && owned.done) await Promise.race([owned.done.catch(() => {}), sleep(3000)])
      void writeQuitMarker('0', { wait: false })
      if (!(await removeShortcut())) lastMarkerError = 'shortcut sync failed (delete)'
      return { ok: true, desktop: false, shortcut: await shortcutExists(), diag: '' }
    }
    void writeQuitMarker('0', { wait: false })
    const state = await waitGone(20)
    if (state === true) {
      if (!(await removeShortcut())) lastMarkerError = 'shortcut sync failed (delete)'
      return { ok: true, desktop: false, shortcut: await shortcutExists(), diag: '' }
    }
    if (state === null) return { ok: true, desktop: null, shortcut: await shortcutExists(), diag: '' }
    try {
      const exe = await pickExe()
      const dir = (exe && dirOf(exe)) || process.cwd()
      // 仅终止与探测到的 exe 路径匹配的进程，避免误杀其他同名程序。
      const script = "$p = Get-Process -Name dsh-launcher -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq " + psQuote(exe || '') + " }; if ($p) { $p | Stop-Process -Force -ErrorAction SilentlyContinue }"
      await runPowerShell(script, dir)
    } catch (error) {
      lastMarkerError = String((error && error.message) || error)
    }
    const state2 = await waitGone(8)
    if (state2 === true) {
      if (!(await removeShortcut())) lastMarkerError = 'shortcut sync failed (delete)'
      return { ok: true, desktop: false, shortcut: await shortcutExists(), diag: '' }
    }
    return { ok: false, error: '桌面应用仍在运行（退出请求与强制结束均未生效）。', shortcut: await shortcutExists(), diag: await buildDiag() }
  }

  async function setShortcut() {
    const ok = await ensureShortcut()
    if (ok) return { ok: true, shortcut: true, diag: '' }
    return { ok: false, error: '创建桌面快捷方式失败。', shortcut: await shortcutExists(), diag: await buildDiag() }
  }

  ctx.effect(() => {
    /**
     * 统一的信任边界守卫：回环 + 同源（自定义头 / Origin / Referer）校验，
     * 不通过直接 403。原实现把这三行复制进每个 handler（优化报告 v2 C4）。
     */
    const guard = (handler) => async (req, res) => {
      if (!isTrustedRequest(req, ctx.webServer.port)) {
        return writeJson(res, 403, { ok: false, error: 'forbidden' })
      }
      return handler(req, res)
    }

    const routes = [
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/state',
        handler: guard(async (_req, res) => {
          writeJson(res, 200, await getState())
        }),
      },
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/set-desktop',
        handler: guard(async (req, res) => {
          const body = await readJsonBody(req)
          writeJson(res, 200, await setDesktop(Boolean(body.enabled)))
        }),
      },
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/set-shortcut',
        handler: guard(async (_req, res) => {
          writeJson(res, 200, await setShortcut())
        }),
      },
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/diagnose',
        handler: guard(async (_req, res) => {
          writeJson(res, 200, await getDiag())
        }),
      },
    ]
    const disposers = routes.map((route) => ctx.webServer.register(route))
    return () => {
      for (const dispose of disposers) dispose()
    }
  }, 'dsh-tauri-launcher: routes')
}
