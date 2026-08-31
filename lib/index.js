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
import { appendFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

export const name = 'desktop-launcher'

/** 路由注册与文件/进程操作所需的主机服务（声明后加载器会等待就绪再 apply）。 */
export const inject = ['webServer', 'fs', 'subprocess']

const STDIO = { stdin: 'ignore', stdout: 'inherit', stderr: 'inherit' }

/** 本包随附的预编译桌面应用 exe 目录（无论安装到哪个 profile 都能定位）。 */
const PACKAGE_BIN = fileURLToPath(new URL('../launcher/bin', import.meta.url))

/** 未配置 launcherDirs 时的默认候选目录（仅包内随附的预编译 exe；本地自行构建的 exe 通过行配置 launcherDirs/launcherExe 指定）。 */
const DEFAULT_DIRS = [PACKAGE_BIN]

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const psQuote = (value) => "'" + String(value).replace(/'/g, "''") + "'"

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
function isTrustedRequest(req, port) {
  if (!isLoopback(req)) return false
  const headers = req.headers || {}
  if (headers['x-requested-with'] === 'dsh-tauri-launcher') return true
  const allowed = new Set([
    'http://127.0.0.1:' + port,
    'http://localhost:' + port,
    'http://[::1]:' + port,
  ])
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
    /** 桌面快捷方式的文件名。 */
    shortcutName: config && typeof config.shortcutName === 'string' && config.shortcutName !== '' ? config.shortcutName : 'DeepSeek Harness.lnk',
  }

  let lastFsError = ''
  let lastMarkerError = ''
  let spawnHandle = null
  let shortcutCache = { t: 0, value: false }

  function baseDirs() {
    const list = resolved.launcherDirs.slice()
    if (resolved.launcherExe) {
      const slash = resolved.launcherExe.lastIndexOf('\\')
      if (slash > 0) list.push(resolved.launcherExe.slice(0, slash))
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
    const age = Math.abs(Date.now() / 1000 - stamp)
    return age < resolved.freshSecs
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

  async function runPowerShell(script, cwd) {
    if (!subprocess) throw new Error('subprocess 服务不可用')
    const handle = subprocess.spawn({
      argv: ['powershell', '-NoProfile', '-NonInteractive', '-Command', script],
      cwd: cwd || (await pickExe()) || '',
      stdio: STDIO,
      graceMs: 3000,
    })
    if (handle && handle.done) await handle.done.catch(() => {})
  }

  async function runPowerShellExitCode(script, cwd) {
    if (!subprocess) return null
    try {
      const handle = subprocess.spawn({
        argv: ['powershell', '-NoProfile', '-NonInteractive', '-Command', script],
        cwd: cwd || process.cwd(),
        stdio: STDIO,
        graceMs: 3000,
      })
      const outcome = handle && handle.done ? await handle.done.catch(() => null) : null
      return outcome && typeof outcome.exitCode === 'number' ? outcome.exitCode : null
    } catch (error) {
      lastMarkerError = String((error && error.message) || error)
      return null
    }
  }

  async function quitScriptFor(value) {
    const exe = await pickExe()
    if (!exe) return null
    const dir = exe.slice(0, exe.lastIndexOf('\\'))
    return {
      dir,
      // dir/value 一律经 psQuote 转义（单引号翻倍），避免含单引号的路径破坏脚本。
      script: 'Set-Content -LiteralPath ' + psQuote(dir + '\\.dsh-quit') + ' -Value ' + psQuote(value) + ' -NoNewline',
    }
  }

  async function writeQuitMarker(value) {
    const job = await quitScriptFor(value)
    if (!job) {
      lastMarkerError = 'writeQuitMarker: no exe dir'
      return
    }
    try {
      await runPowerShell(job.script, job.dir)
      lastMarkerError = ''
    } catch (error) {
      lastMarkerError = String((error && error.message) || error)
    }
  }

  function writeQuitMarkerNoWait(value) {
    void quitScriptFor(value).then((job) => {
      if (!job) return
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
    })
  }

  async function quitFileGone() {
    const exe = await pickExe()
    if (!exe) return true
    const dir = exe.slice(0, exe.lastIndexOf('\\'))
    return (await fileText(dir, '.dsh-quit')) === null
  }

  async function quitMarkerIsOne() {
    const exe = await pickExe()
    if (!exe) return false
    const dir = exe.slice(0, exe.lastIndexOf('\\'))
    return (await fileText(dir, '.dsh-quit')) === '1'
  }

  function shortcutExistsScript() {
    return "$desktop=[Environment]::GetFolderPath('Desktop'); $lnk=Join-Path $desktop " + psQuote(resolved.shortcutName) + "; if (Test-Path -LiteralPath $lnk) { exit 0 } else { exit 1 }"
  }

  function shortcutCreateScript(exe) {
    return "$desktop=[Environment]::GetFolderPath('Desktop'); $lnk=Join-Path $desktop " + psQuote(resolved.shortcutName) + "; " +
      "$exe=" + psQuote(exe) + "; " +
      '$ws=New-Object -ComObject WScript.Shell; $sc=$ws.CreateShortcut($lnk); ' +
      '$sc.TargetPath=$exe; $sc.WorkingDirectory=Split-Path $exe; ' +
      "$sc.IconLocation=($exe + ',0'); $sc.Description=" + psQuote('DeepSeek Harness 桌面启动器') + '; ' +
      '$sc.Save(); if (Test-Path -LiteralPath $lnk) { exit 0 } else { exit 1 }'
  }

  function shortcutDeleteScript() {
    return "$desktop=[Environment]::GetFolderPath('Desktop'); $lnk=Join-Path $desktop " + psQuote(resolved.shortcutName) + '; ' +
      'Remove-Item -LiteralPath $lnk -Force -ErrorAction SilentlyContinue; ' +
      'if (Test-Path -LiteralPath $lnk) { exit 1 } else { exit 0 }'
  }

  async function shortcutExists() {
    const now = Date.now()
    if (now - shortcutCache.t < 5000) return shortcutCache.value
    const code = await runPowerShellExitCode(shortcutExistsScript(), process.cwd())
    shortcutCache = { t: Date.now(), value: code === 0 }
    return shortcutCache.value
  }

  async function ensureShortcut() {
    if (await shortcutExists()) return true
    const exe = await pickExe()
    if (!exe) return false
    const code = await runPowerShellExitCode(shortcutCreateScript(exe), process.cwd())
    shortcutCache = { t: Date.now(), value: code === 0 }
    return code === 0
  }

  async function removeShortcut() {
    if (!(await shortcutExists())) return true
    const code = await runPowerShellExitCode(shortcutDeleteScript(), process.cwd())
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

  // ---------- 会话导航运行时诊断（临时通道，定位完成后移除） ----------
  const NAV_DIAG_FILE = join(homedir(), '.dsh', 'profiles', 'web', 'tb-nav-diag.log')

  function appendNavDiag(entry) {
    try {
      const line = JSON.stringify({ t: Date.now(), ...entry }) + '\n'
      appendFileSync(NAV_DIAG_FILE, line, 'utf8')
      const size = existsSync(NAV_DIAG_FILE) ? (() => { try { return readFileSync(NAV_DIAG_FILE).length } catch { return 0 } })() : 0
      if (size > 256 * 1024) {
        const text = readFileSync(NAV_DIAG_FILE, 'utf8')
        writeFileSync(NAV_DIAG_FILE, text.slice(-64 * 1024), 'utf8')
      }
    } catch { /* 诊断失败不影响主流程 */ }
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
        const dir = exe.slice(0, exe.lastIndexOf('\\'))
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
      writeQuitMarkerNoWait('0')
      if (!(await removeShortcut())) lastMarkerError = 'shortcut sync failed (delete)'
      return { ok: true, desktop: false, shortcut: await shortcutExists(), diag: '' }
    }
    writeQuitMarkerNoWait('0')
    const state = await waitGone(20)
    if (state === true) {
      if (!(await removeShortcut())) lastMarkerError = 'shortcut sync failed (delete)'
      return { ok: true, desktop: false, shortcut: await shortcutExists(), diag: '' }
    }
    if (state === null) return { ok: true, desktop: null, shortcut: await shortcutExists(), diag: '' }
    try {
      const exe = await pickExe()
      const dir = exe ? exe.slice(0, exe.lastIndexOf('\\')) : process.cwd()
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
    const routes = [
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/state',
        handler: async (req, res) => {
          if (!isTrustedRequest(req, ctx.webServer.port)) return writeJson(res, 403, { ok: false, error: 'forbidden' })
          writeJson(res, 200, await getState())
        },
      },
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/set-desktop',
        handler: async (req, res) => {
          if (!isTrustedRequest(req, ctx.webServer.port)) return writeJson(res, 403, { ok: false, error: 'forbidden' })
          const body = await readJsonBody(req)
          writeJson(res, 200, await setDesktop(Boolean(body.enabled)))
        },
      },
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/set-shortcut',
        handler: async (req, res) => {
          if (!isTrustedRequest(req, ctx.webServer.port)) return writeJson(res, 403, { ok: false, error: 'forbidden' })
          writeJson(res, 200, await setShortcut())
        },
      },
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/diagnose',
        handler: async (req, res) => {
          if (!isTrustedRequest(req, ctx.webServer.port)) return writeJson(res, 403, { ok: false, error: 'forbidden' })
          writeJson(res, 200, await getDiag())
        },
      },
      {
        kind: 'exact',
        path: '/api/dsh-tauri-launcher/nav-diag',
        handler: async (req, res) => {
          if (!isTrustedRequest(req, ctx.webServer.port)) return writeJson(res, 403, { ok: false, error: 'forbidden' })
          const body = await readJsonBody(req)
          appendNavDiag({ stage: String(body.stage ?? 'unknown'), detail: String(body.detail ?? '').slice(0, 400) })
          writeJson(res, 200, { ok: true })
        },
      },
    ]
    const disposers = routes.map((route) => ctx.webServer.register(route))
    return () => {
      for (const dispose of disposers) dispose()
    }
  }, 'dsh-tauri-launcher: routes')
}
