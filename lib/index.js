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
 * 可调参数全部走行配置（见 apply 的 config 处理与 README「配置」章节）；
 * 内置候选目录只是默认值，供未配置时自动探测。
 */

import { fileURLToPath } from 'node:url'

export const name = 'desktop-launcher'

/** 路由注册与文件/进程操作所需的主机服务（声明后加载器会等待就绪再 apply）。 */
export const inject = ['webServer', 'fs', 'subprocess']

const STDIO = { stdin: 'ignore', stdout: 'inherit', stderr: 'inherit' }

/** 本包随附的预编译桌面应用 exe 目录（无论安装到哪个 profile 都能定位）。 */
const PACKAGE_BIN = fileURLToPath(new URL('../launcher/bin', import.meta.url))

/** 未配置 launcherDirs 时的默认候选目录（仅包内随附的预编译 exe；本地自行构建的 exe 通过行配置 launcherDirs/launcherExe 指定）。 */
const DEFAULT_DIRS = [PACKAGE_BIN]

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
  let spawnHandle = null
  let shortcutCache = { t: 0, value: false }

  function baseDirs() {
    const list = resolved.launcherDirs.slice()
    if (resolved.launcherExe) {
      const dir = dirOf(resolved.launcherExe)
      if (dir) list.push(dir)
    }
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
    const checked = await Promise.all(baseDirs().map(async (dir) => ((await exeExists(dir)) ? dir : null)))
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
    const dirs = await exeDirs()
    lines.push('exeDirs: ' + JSON.stringify(dirs))
    const exe = await pickExe()
    lines.push('pickExe: ' + (exe || '(null)'))
    if (lastFsError) lines.push('fsError: ' + lastFsError)
    lines.push('markerWrite: ' + (lastMarkerError || 'ok'))
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
        spawnHandle = subprocess.spawn({ argv: [exe], cwd: dir, stdio: STDIO, graceMs: 3000 })
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
