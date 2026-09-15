# 更新日志

> **本文件是发版说明的唯一来源。** 打 `v*` tag 后，CI 会取走对应小节（去掉标题行、
> 追加下载与安装说明）作为 GitHub Release 与 Gitee 发行版的正文；改这里即可同时
> 修两处，不需要手工编辑发行版页面。写法规范见 [docs/release.md](docs/release.md)。

每个版本一节，中文在前、英文在后，中间用 `<!-- en -->` 分隔（Gitee 正文在标记处
截断，因此只显示中文）。小节内按 **新增 / 修复 / 变更 / 内部改进** 分组，无内容的
分组省略；条目写「现象或功能 → 原因/影响 → 解决方式」，面向使用者，不照抄提交标题。

## v1.0.11 — 未发布

修好「设置里的桌面启动开关打开后桌面端起不来、关闭却一直正常」的问题：插件拉起桌面应用
时不再继承 DSH 自己的标准输出句柄（那种句柄在桌面应用退出后已经断开，会让新进程一启动就
崩溃），并把这个类别的失败直接报成设置里的红字提示。

**修复**

- **开关「打开」启不动桌面端，只有「关闭」正常**：插件此前用 `stdio: inherit` 拉起桌面
  应用，而 DSH 自己往往就是被桌面应用拉起的——它的 stdout/stderr 正是桌面应用当时创建的
  管道。桌面应用一退出，这根管道的读端随之消失；此后以 `inherit` 拉起的桌面应用继承到的
  是**已断开的手柄**，一写就 panic 退出（实测 `exit code 101`，进程存活不到 1 秒），于是
  表现为「开关弹回、桌面端没出现」。改为**收集模式**（管道由 DSH 侧持有）后，桌面应用在
  任何启停时序下都能正常启动。
- **失败不再静默**：桌面应用若在 20 秒就绪窗口内退出，设置面板现在直接给出红字
  「桌面应用启动后立即退出（exit code …）」，展开诊断可见新增的 `appExit` 与
  `appOutput`（应用自己的输出）两行——此前只会静默等满 20 秒再显示「已停止」，用户无从
  判断原因。

**内部改进**

- 桌面应用的启动输出改由插件收集（不再继承 DSH 的控制台），随诊断一并展示。

<!-- en -->

### English

Fixes the settings switch failing to start the desktop app while stopping it kept working: the
plugin no longer hands its own standard-output handles to the desktop app (those handles are
already broken once the desktop app has exited, which crashed the new process on startup), and
this class of failure is now reported as a visible error in the settings panel.

**Fixed**

- **Switching "Desktop launch" on did not start the desktop app, while switching it off worked**:
  the plugin used `stdio: inherit` to launch the desktop app, but DSH itself is usually started
  by that very app — its stdout/stderr are the pipes the app created. Once the app exits, those
  pipes lose their reader, so a process launched with `inherit` afterwards receives **broken
  handles**, panics on its first write and exits (measured `exit code 101`, alive for under a
  second), which surfaced as the switch flipping back with no app window. Launching in
  **collect mode** (pipes owned by DSH) starts reliably regardless of start/stop ordering.
- **Failures are no longer silent**: when the desktop app exits inside the 20-second readiness
  window, the panel now shows "the desktop app exited immediately after launch (exit code …)"
  and the diagnostics gained the `appExit` and `appOutput` (the app's own output) lines. Before,
  the panel waited silently for 20 seconds and then reported "stopped" with no explanation.

**Internal**

- The desktop app's startup output is now collected by the plugin (instead of inheriting DSH's
  console) and shown alongside the diagnostics.

## v1.0.10 — 未发布

修好设置里「桌面启动」与实际运行的桌面端不同步的问题：插件现在会自动找到正在运行的
那一份桌面应用实例（不再只认 npm 包内的副本）。本版只改插件，桌面应用行为不变。

**修复**

- **设置里的「桌面启动」状态对不上、开关关不掉也开不起来**：心跳、退出标记与桌面应用
  配置都以「桌面应用 exe 同目录」为界，而插件此前只把 npm 包内的 `launcher/bin` 当作
  候选目录。若实际运行的是另一份副本（例如本地 `build.ps1` 的构建产物），插件就会读到
  空心跳 → 状态显示「状态未知」；关闭开关写出的 `.dsh-quit` 落进错误目录，正在运行的
  实例看不到（关不掉）；打开开关反而拉起第二份副本（与已运行实例抢全局快捷键与外壳
  页面端口）。现在插件按优先级探测候选目录——行配置 `launcherExe` → **运行中的
  `dsh-launcher` 进程目录** → **桌面快捷方式目标目录** → 行配置 `launcherDirs` →
  本次运行中发现过的目录 → 包内 `launcher/bin`——状态判定、退出标记与启动目标因此
  始终对齐到同一个实例；没有实例在运行时行为与旧版完全一致（仍回退到配置目录/包内副本），
  指向已删除副本的失效快捷方式也会自动跳过。

**内部改进**

- 进程目录与快捷方式目标用 PowerShell 采集模式读取（`Get-Process` 取 `Path`、
  WScript.Shell 读 `.lnk` 的 `TargetPath`），结果缓存 5 秒、脚本前置 UTF-8 输出编码；
  探测失败静默回退，不影响旧路径。诊断信息新增 `runningDirs` / `shortcutTarget` 两行。

<!-- en -->

### English

Fixes the settings "Desktop launch" section being out of sync with the desktop app that is
actually running: the plugin now finds the live instance instead of only looking inside the
npm package. Plugin-only release; desktop app behaviour is unchanged.

**Fixed**

- **"Desktop launch" state did not match reality, and the switch could neither stop nor start
  the app**: heartbeats, the quit marker and the desktop app's own config all live next to the
  desktop app executable, while the plugin only treated the packaged `launcher/bin` as a
  candidate directory. When another copy was running (for example a local `build.ps1`
  artifact), the plugin read no heartbeat → "unknown" state; the `.dsh-quit` marker was
  written into the wrong directory so the running instance never saw it (could not be
  stopped); and switching on spawned a second copy that fought the running one for the global
  shortcut and the shell page port. The plugin now probes candidate directories in priority
  order — `launcherExe` config → **the running `dsh-launcher` process directory** → **the
  desktop shortcut target directory** → `launcherDirs` config → directories discovered in this
  run → the packaged `launcher/bin` — so state, quit marker and launch target always line up
  with one instance. With nothing running, behaviour is unchanged (still falls back to the
  configured directory and the packaged copy), and a stale shortcut pointing at a deleted copy
  is skipped automatically.

**Internal**

- Process directory and shortcut target are read through PowerShell in collect mode
  (`Get-Process` for `Path`, WScript.Shell for the `.lnk` `TargetPath`), cached for 5 seconds,
  with a UTF-8 output-encoding preamble; probe failures fall back silently to the old paths.
  Diagnostics gained `runningDirs` / `shortcutTarget` lines.

## v1.0.9 — 未发布

修好标题栏的两个老问题：◀/▶ 点了没反应、切主题后标题栏慢一拍；本版同时完成阶段二
（解耦）与阶段三（打磨）两轮重构，内部实现更整齐，对外行为不变。

**修复**

- **标题栏 ◀/▶ 点击无响应、Windows 系统主题不跟随**：外壳页改用 `127.0.0.1:3081`
  承载后，插件侧靠 `document.referrer` 推导父页面 origin，而 WebView2 下它是空串，
  推导值退化成写死的兜底值 → 所有「外壳 → 插件」的消息被 origin 校验丢弃。改为以
  `event.source === window.parent` 判定身份 + origin 白名单（[#30](https://github.com/cilis/dsh-tauri-launcher/pull/30)）。
- **在设置里切浅色/深色后标题栏慢一拍**（界面已变、标题栏还是上一个主题）：主题读取
  与 DSH 主题写入同处一次同步派发，同步读计算样式拿到的是切换前的值。改为延后到当前
  任务之后读取，并加 DOM 变更观察兜底与同状态去重（[#28](https://github.com/cilis/dsh-tauri-launcher/pull/28)）。
- 心跳新鲜度改为单向判定：系统时钟回拨不再把已停止的桌面应用误判为「运行中」（[#32](https://github.com/cilis/dsh-tauri-launcher/pull/32)）。
- **任务栏图标在浅色任务栏上看不见**：任务栏按钮画的是 exe 内嵌图标（实测：切主题时
  窗口四个图标槽与托盘图标都换新，唯独按钮像素仍是 exe 里的白鲸，连强制重建按钮都不
  变），而白鲸只在深色任务栏上看得清。现把应用图标换成浅/深底都清晰的品牌蓝，并在
  建窗口前设置显式 AppUserModelID，让任务栏按本应用身份取图标。托盘与标题栏/Alt-Tab
  图标继续跟随系统主题。

**内部改进**

- 阶段二：harness 子进程状态收敛为单一三态机、退出路径收敛为单一入口、外链白名单
  单源化（前端只传 key）、postMessage 统一信封并兼容旧字段（[#31](https://github.com/cilis/dsh-tauri-launcher/pull/31)）。
- 阶段三：关键路径吞掉的错误改为留痕（注册表 / PowerShell / taskkill / 窗口类图标槽）、
  设置快照只读一次配置、退出进度窗口样式自持、构建脚本不再把 cargo 的 stderr 进度误报
  为失败、`build.ps1 -Bump` 一条命令同步三处版本号（[#32](https://github.com/cilis/dsh-tauri-launcher/pull/32)）。
- 文档：architecture.md 补齐外壳 ↔ 插件消息协议、协议超时与常量清单，以及标题栏、
  主题跟随链路、会话导航栈、预建窗口约束四节。

<!-- en -->

### English

Fixes two long-standing titlebar issues (◀/▶ doing nothing, and the titlebar lagging one
theme behind) and completes two internal refactor rounds; no user-facing behaviour changes
beyond the fixes.

**Fixed**

- **Titlebar ◀/▶ did not respond and Windows theme was not followed**: after the shell page
  moved to `127.0.0.1:3081`, the plugin inferred the parent origin from `document.referrer`,
  which is empty under WebView2, so the value fell back to a stale constant and every
  shell→plugin message was dropped by the origin check. Messages are now identified by
  `event.source === window.parent` plus an origin allow-list
  ([#30](https://github.com/cilis/dsh-tauri-launcher/pull/30)).
- **Titlebar lagged one theme behind after switching light/dark**: the theme read ran in the
  same synchronous dispatch as DSH's own theme write, so it saw the previous theme. Reads are
  now deferred to the next task, with a DOM mutation observer as a fallback and
  same-state de-duplication ([#28](https://github.com/cilis/dsh-tauri-launcher/pull/28)).
- Heartbeat freshness is now a one-way check: a backwards system-clock adjustment no longer
  reports a stopped desktop app as "running"
  ([#32](https://github.com/cilis/dsh-tauri-launcher/pull/32)).

**Internal**

- Stage 2: single three-state harness process machine, single exit entry point, single source
  for external links, versioned postMessage envelope
  ([#31](https://github.com/cilis/dsh-tauri-launcher/pull/31)).
- Stage 3: previously swallowed errors are now logged (registry / PowerShell / taskkill /
  window-class icon slot), settings snapshot reads config once, exiting window styles are
  self-contained, the build script no longer misreports cargo's stderr progress as a failure,
  and `build.ps1 -Bump` syncs all three version fields
  ([#32](https://github.com/cilis/dsh-tauri-launcher/pull/32)).
- Docs: message protocol, timeout/constant inventory, titlebar, theme-follow chain, session
  navigation stack and pre-built window constraints.

## v1.0.8 — 2026-09-09

适配新版 DSH 的 token + cookie 鉴权（修新版下内嵌页面白屏），并完成第一轮模块化重构。

**修复**

- **新版 DSH 下内嵌页面白屏**（只有一行英文小字）：DSH 会话 cookie 是
  `SameSite=Strict`，而 SameSite 比较站点时忽略端口——外壳页原在 `http://tauri.localhost`，
  与 `127.0.0.1` 跨站，iframe 内 303 响应的 `Set-Cookie` 被 WebView2 拒收。外壳页改由
  本机 `http://127.0.0.1:3081` 静态服务承载，与 DSH 的 3080 同站（[#26](https://github.com/cilis/dsh-tauri-launcher/pull/26)）。
- 外壳页属远程来源，Tauri 对其强制 ACL → `Command check_dsh not allowed by ACL`。新增
  `permissions/launcher.toml` 显式授权全部应用命令（[#26](https://github.com/cilis/dsh-tauri-launcher/pull/26)）。
- 适配新版 `dsh web` 的进程 token + 签名 cookie 鉴权：就绪探测改为双信号（新版 401 +
  捕获 stdout 的 `dsh web: http://…?token=…` 行 / 旧版 `__DSH_BOOT__` 指纹保持原路径），
  窗口用带 token 的地址完成握手（[#24](https://github.com/cilis/dsh-tauri-launcher/pull/24)）。
- 接管外部新版实例时显示鉴权提示条，可「关闭并重启」：两步确认后终止占用端口的外部
  实例并重新拉起托管实例（netstat 定位 PID，只匹配本地监听地址，避免误杀远程客户端）（[#25](https://github.com/cilis/dsh-tauri-launcher/pull/25)）。

**内部改进**

- 阶段一模块化：`lib.rs` 拆分为职责单一模块，`client.js` / `main.js` 函数级模块化（[#23](https://github.com/cilis/dsh-tauri-launcher/pull/23)）。
- 设置窗口描述文案精简；`.gitignore` 补运行时标记文件。

<!-- en -->

### English

Adds support for the new DSH token + cookie authentication (fixing a blank embedded page)
and lands the first module-split refactor.

**Fixed**

- **Blank embedded page with newer DSH builds**: DSH's session cookie is `SameSite=Strict`,
  and SameSite ignores the port when comparing sites — the shell page lived on
  `http://tauri.localhost`, a different site from `127.0.0.1`, so WebView2 rejected the
  `Set-Cookie` of the iframe's 303 response. The shell page is now served from a local
  `http://127.0.0.1:3081` static server, same site as DSH on 3080
  ([#26](https://github.com/cilis/dsh-tauri-launcher/pull/26)).
- Tauri enforces ACLs for remote origins, which broke the shell page with
  `Command check_dsh not allowed by ACL`; `permissions/launcher.toml` now grants every app
  command explicitly ([#26](https://github.com/cilis/dsh-tauri-launcher/pull/26)).
- Readiness probing handles the new `dsh web` process token + signed cookie: dual signal
  (HTTP 401 plus the captured `dsh web: http://…?token=…` stdout line, while the legacy
  `__DSH_BOOT__` fingerprint path is unchanged)
  ([#24](https://github.com/cilis/dsh-tauri-launcher/pull/24)).
- Adopting an externally started instance now shows an auth hint with a two-step
  "close and restart" action that reclaims the port and relaunches a managed instance
  ([#25](https://github.com/cilis/dsh-tauri-launcher/pull/25)).

**Internal**

- Module split: `lib.rs` divided into single-purpose modules, `client.js` / `main.js`
  refactored to function-level structure
  ([#23](https://github.com/cilis/dsh-tauri-launcher/pull/23)).
- Shortened the settings window description; added runtime marker files to `.gitignore`.

## v1.0.7 — 2026-09-06

修复 Windows 11 25H2 上运行时切换主题时任务栏图标不刷新。

**修复**

- **Win11 25H2 任务栏图标运行时切换不刷新**：只发 `WM_SETICON ICON_BIG` 在启动时有效，
  但 25H2 的任务栏在运行时读的是窗口类图标槽（tao/tauri 从不设置该槽）。补
  `SetClassLongPtrW(GCLP_HICON / GCLP_HICONSM)` 双路径（[#20](https://github.com/cilis/dsh-tauri-launcher/pull/20)）。

**内部改进**

- CI：Gitee 附件上传超时放宽到 600s（海外 runner 上传 10MB exe 曾两轮全超 90s），
  外层重试 2 → 3 次（[#22](https://github.com/cilis/dsh-tauri-launcher/pull/22)）。

<!-- en -->

### English

Fixes the taskbar icon not refreshing at runtime on Windows 11 25H2.

**Fixed**

- **Taskbar icon did not refresh at runtime on Win11 25H2**: sending only
  `WM_SETICON ICON_BIG` works at startup, but 25H2's taskbar reads the window-class icon
  slot at runtime (which tao/tauri never sets). Added the
  `SetClassLongPtrW(GCLP_HICON / GCLP_HICONSM)` path alongside it
  ([#20](https://github.com/cilis/dsh-tauri-launcher/pull/20)).

**Internal**

- CI: Gitee asset upload timeout raised to 600s (a 10 MB exe from an overseas runner timed
  out twice at 90s) and retries increased from 2 to 3
  ([#22](https://github.com/cilis/dsh-tauri-launcher/pull/22)).

## v1.0.6 — 2026-09-02

修复桌面端「跟随系统」外观不随 Windows 变化，以及任务栏图标不跟随主题。

**修复**

- **桌面端「跟随系统」外观不随 Windows 变化**：WebView2 需要宿主显式设置首选配色方案，
  而 Tauri/wry 未暴露该接口（wry#806），于是 DSH 读到的 `prefers-color-scheme` 冻结在
  启动值。现在由启动器轮询系统主题 → 外壳转发 → 插件把冻结值覆盖为真实系统值并触发
  重发布（[#18](https://github.com/cilis/dsh-tauri-launcher/pull/18)）。
- **修复过程不改写你的主题偏好**：设置里始终是「跟随系统」，面板不会被改成固定浅色/
  深色；你手动固定主题时该补偿自动让位。
- 任务栏窗口图标不跟随主题：`set_icon` 只覆盖标题栏小图标，补发 `WM_SETICON ICON_BIG`（[#18](https://github.com/cilis/dsh-tauri-launcher/pull/18)）。

**内部改进**

- 文档：更新设置窗口截图；验证清单移除双仓库同步项（桌面应用已单源维护）。

<!-- en -->

### English

Fixes the desktop app not following the Windows theme in "system" appearance mode, plus the
taskbar icon not following the theme.

**Fixed**

- **"Follow system" appearance did not react to Windows theme changes**: WebView2 requires
  the host to set the preferred colour scheme, which Tauri/wry does not expose (wry#806), so
  DSH's `prefers-color-scheme` stayed frozen at its startup value. The launcher now polls the
  system theme, forwards it to the shell and then to the plugin, which overrides the frozen
  value and republishes the theme
  ([#18](https://github.com/cilis/dsh-tauri-launcher/pull/18)).
- **Your theme preference is never rewritten**: the setting stays on "follow system" and the
  panel is not switched to a fixed light/dark value; a manually pinned theme wins over the
  compensation.
- The taskbar icon did not follow the theme: `set_icon` only updates the small titlebar icon,
  so `WM_SETICON ICON_BIG` is now sent as well
  ([#18](https://github.com/cilis/dsh-tauri-launcher/pull/18)).

**Internal**

- Docs: refreshed settings-window screenshots; dropped the dual-repository sync item from the
  verification checklist (the desktop app is maintained from a single source).

## v1.0.5 — 2026-09-01

外观与交互大版本：自定义标题栏（三键、菜单、会话前进/后退）、标题栏与托盘/任务栏图标
跟随主题，并恢复了网页端文件拖拽上传。

**新增**

- **自定义标题栏**：仅在与 DSH 界面同屏时显示，含最小化/最大化/关闭三键与菜单（[#10](https://github.com/cilis/dsh-tauri-launcher/pull/10)）。
- **标题栏 ◀/▶ 会话前进/后退**：DSH 是单页应用，切换会话不产生浏览器历史，因此改用
  应用层会话导航栈；`Alt+←/→` 与鼠标侧键走同一个栈（[#14](https://github.com/cilis/dsh-tauri-launcher/pull/14)）。
- **标题栏跟随 DSH 浅/深主题**：颜色取自 DSH 真实设计 token（内置主题快照的 tokens 是
  空表，只能从计算样式取）（[#14](https://github.com/cilis/dsh-tauri-launcher/pull/14)）。
- 托盘 / 窗口 / 任务栏图标跟随 Windows 系统主题（新增单色图标变体）（[#9](https://github.com/cilis/dsh-tauri-launcher/pull/9)）。
- conversation 区域边框勾勒 + sidebar 融合框架底色（顶线、左边框、左上圆角）（[#11](https://github.com/cilis/dsh-tauri-launcher/pull/11)）。

**修复**

- 主窗口禁用 Tauri 原生拖放拦截，恢复网页端文件拖拽上传（[#8](https://github.com/cilis/dsh-tauri-launcher/pull/8)）。
- 标题栏底部去掉分隔线，与 DSH 页面无缝衔接（[#12](https://github.com/cilis/dsh-tauri-launcher/pull/12)）。
- 退出进度窗口改为启动时预建、退出时复用（与设置窗口同一 WebView2 同步构建死锁根因）。
- 导航键状态字段路径错误，曾导致 ◀/▶ 永远置灰。
- 源码走读问题修复：开机自启引号、exiting 窗口 capability、标记文件转义、主题解析、
  端口兜底、CSP 等（[#13](https://github.com/cilis/dsh-tauri-launcher/pull/13)，由
  [@jermaine7511261](https://github.com/jermaine7511261) 贡献）。

**变更**

- 标题栏菜单去掉字标并更名为「文件」，新增「帮助」菜单（外链官网/文档，走白名单）。

**内部改进**

- 移除导航诊断通道（`/nav-diag` 路由与打点），保留 3s ping 状态自愈与 sessions 重试梯子。
- `package.json` 补 repository / homepage / bugs 字段，仓库根新增 `screenshots.json`（[#5](https://github.com/cilis/dsh-tauri-launcher/pull/5)、[#6](https://github.com/cilis/dsh-tauri-launcher/pull/6)、[#7](https://github.com/cilis/dsh-tauri-launcher/pull/7)）。

<!-- en -->

### English

The look-and-feel release: a custom titlebar (window controls, menu, session back/forward),
theme-following titlebar plus tray/taskbar icons, and web drag-and-drop restored.

**Added**

- **Custom titlebar**, shown only while the DSH UI is on screen, with
  minimise/maximise/close and a menu ([#10](https://github.com/cilis/dsh-tauri-launcher/pull/10)).
- **Session back/forward buttons in the titlebar**: DSH is a single-page app, so switching
  sessions creates no browser history — an application-level session navigation stack is used
  instead, shared by the buttons, `Alt+←/→` and the mouse side buttons
  ([#14](https://github.com/cilis/dsh-tauri-launcher/pull/14)).
- **Titlebar follows the DSH light/dark theme**, reading DSH's real design tokens (the
  built-in theme snapshot ships an empty token table, so computed styles are used)
  ([#14](https://github.com/cilis/dsh-tauri-launcher/pull/14)).
- Tray, window and taskbar icons follow the Windows theme (new monochrome icon variants)
  ([#9](https://github.com/cilis/dsh-tauri-launcher/pull/9)).
- Conversation outline and sidebar frame colour ([#11](https://github.com/cilis/dsh-tauri-launcher/pull/11)).

**Fixed**

- Disabled Tauri's native drag-and-drop interception in the main window, restoring web file
  upload by dragging ([#8](https://github.com/cilis/dsh-tauri-launcher/pull/8)).
- Removed the titlebar's bottom divider so it blends into the DSH page
  ([#12](https://github.com/cilis/dsh-tauri-launcher/pull/12)).
- The exiting progress window is now pre-built at startup and reused on exit (same WebView2
  synchronous-build deadlock as the settings window).
- Fixed a wrong state-field path that kept the back/forward buttons permanently disabled.
- Assorted source-review fixes: autostart quoting, exiting-window capability, marker escaping,
  theme parsing, port fallback, CSP (contributed by
  [@jermaine7511261](https://github.com/jermaine7511261) in
  [#13](https://github.com/cilis/dsh-tauri-launcher/pull/13)).

**Changed**

- Titlebar menu lost its text label, was renamed to "文件", and gained a "帮助" menu with
  allow-listed external links (website/docs).

**Internal**

- Removed the navigation diagnostic channel while keeping the 3s ping self-healing and the
  sessions retry ladder.
- Added repository/homepage/bugs metadata to `package.json` and a root `screenshots.json`
  ([#5](https://github.com/cilis/dsh-tauri-launcher/pull/5),
  [#6](https://github.com/cilis/dsh-tauri-launcher/pull/6),
  [#7](https://github.com/cilis/dsh-tauri-launcher/pull/7)).

## v1.0.4 — 2026-08-25

设置窗口改为无边框、托盘菜单显示桌面端版本号；同时建立起分支 + PR 验证流水线。

**新增**

- 托盘菜单新增桌面端版本号信息项（[#3](https://github.com/cilis/dsh-tauri-launcher/pull/3)）。
- 设置窗口改为无边框（隐藏系统边框与标题栏）（[#3](https://github.com/cilis/dsh-tauri-launcher/pull/3)）。

**内部改进**

- CI：新增分支 / PR 验证流水线（`cargo test` + release 构建 + exe 产物）（[#1](https://github.com/cilis/dsh-tauri-launcher/pull/1)）。
- 文档：新增分支验证工作流（分支模型、PR 流程、验证清单、发版策略）（[#2](https://github.com/cilis/dsh-tauri-launcher/pull/2)）。

<!-- en -->

### English

Frameless settings window and a version entry in the tray menu, plus the introduction of the
branch + PR verification pipeline.

**Added**

- Desktop version entry in the tray menu
  ([#3](https://github.com/cilis/dsh-tauri-launcher/pull/3)).
- Frameless settings window (system border and titlebar hidden)
  ([#3](https://github.com/cilis/dsh-tauri-launcher/pull/3)).

**Internal**

- CI: branch/PR verification pipeline (`cargo test` + release build + exe artifact)
  ([#1](https://github.com/cilis/dsh-tauri-launcher/pull/1)).
- Docs: branch workflow documentation (branch model, PR flow, checklist, release policy)
  ([#2](https://github.com/cilis/dsh-tauri-launcher/pull/2)).

## v1.0.3 — 2026-08-24

退出时新增进度窗口，并让 Gitee 镜像的发行版与附件自动同步。

**新增**

- **退出进度窗口**：退出时若需结束 Harness 进程，显示进度窗口；清理移入后台线程，
  避免动画白屏。

**内部改进**

- CI：新增 Gitee 发行版自动同步（`GITEE_TOKEN`）——创建/更新发行版、上传 exe 附件，
  附件上传带重试与二次确认，失败原因回写发行版描述便于远程诊断，JSON 请求补
  `charset=utf-8` 修复中文乱码。
- 文档：architecture.md 补充退出流程与紧凑进度窗口说明。

<!-- en -->

### English

Adds an exit progress window and automatic release syncing to the Gitee mirror.

**Added**

- **Exit progress window**: shown while the Harness process is being terminated on exit;
  cleanup moved to a background thread to avoid a blank window during the animation.

**Internal**

- CI: automatic Gitee release sync (`GITEE_TOKEN`) — create/update the release, upload the
  exe asset, with retries and a second confirmation; failure reasons are written into the
  release description for remote diagnosis, and `charset=utf-8` was added to JSON requests to
  fix mojibake.
- Docs: exit flow and compact progress window documented in architecture.md.

## v1.0.2 — 2026-08-23

安全与健壮性修复：回环接口加请求来源校验、去掉硬编码路径、npm 包从 437.8MB 收窄到 3.1MB。

**修复**

- **回环接口增加请求来源校验**：回环地址 + `X-Requested-With` 自定义头 / Origin / Referer
  白名单，四个路由统一套用；前端请求全部带该自定义头。
- 删除 `DEFAULT_DIRS` 中的本机绝对路径与 workspaceRoot 兜底分支，仅保留包内
  `launcher/bin`（避免在别人的机器上探测到本机路径）。
- 兜底强杀改为按 exe 路径精确匹配，不再按进程名误杀同名进程。
- `getState` 不再全量诊断，拆出按需拉取的 `/diagnose` 路由；`readJsonBody` 加 1MB 上限；
  exe 目录探测改为并发。

**变更**

- **npm 包体积 437.8MB → 3.1MB**：`files` 从 `launcher/` 收窄为 `launcher/bin`，不再把
  Cargo 构建产物（`target/`）打进包里（原先会导致 `npm publish` 报
  `ERR_STRING_TOO_LONG`）。
- README 注明：修改 Rust 源码后必须重新构建并同步版本号。

**内部改进**

- CI：Release 构建前先跑 `cargo test`；回归测试改为环境无关（用
  `DSH_LAUNCHER_NODE` / `DSH_LAUNCHER_NPM_ROOT` 注入临时目录），修复 CI 上必然失败的问题。

<!-- en -->

### English

Security and robustness fixes: request-origin validation for the loopback API, no hardcoded
paths, and the npm package shrinking from 437.8 MB to 3.1 MB.

**Fixed**

- **Request-origin validation on the loopback API**: loopback address plus an
  `X-Requested-With` header / Origin / Referer allow-list, applied to all four routes; the
  front end sends the custom header on every request.
- Removed machine-specific absolute paths and the `workspaceRoot` fallback from
  `DEFAULT_DIRS`, keeping only the in-package `launcher/bin`.
- The fallback force-kill now matches the exact exe path instead of the process name, so it
  can no longer kill unrelated processes with the same name.
- `getState` no longer returns the full diagnosis (moved to an on-demand `/diagnose` route);
  `readJsonBody` is capped at 1 MB; exe directory probing is concurrent.

**Changed**

- **npm package size 437.8 MB → 3.1 MB**: `files` narrowed from `launcher/` to `launcher/bin`,
  so Cargo build output (`target/`) is no longer published (it used to make `npm publish` fail
  with `ERR_STRING_TOO_LONG`).
- README now states that Rust source changes require a rebuild and a version bump.

**Internal**

- CI: `cargo test` now runs before the release build, and the regression test was made
  environment-independent (`DSH_LAUNCHER_NODE` / `DSH_LAUNCHER_NPM_ROOT` inject a temporary
  install directory), fixing a guaranteed CI failure.

## v1.0.1 — 2026-08-22

从零拉起 DSH 时不再顺带弹出系统默认浏览器。

**修复**

- 拉起 `dsh web` 时增加 `--no-open`：`dsh web` 默认 `openBrowser=true`，每次从零拉起都会
  弹出系统浏览器，与内嵌窗口的体验冲突。

**内部改进**

- README 补充说明。

<!-- en -->

### English

No longer opens the system browser when starting DSH from scratch.

**Fixed**

- Added `--no-open` when spawning `dsh web`: it defaults to `openBrowser=true`, so every cold
  start popped up the system browser, which conflicts with the embedded window experience.

**Internal**

- README updates.

## v1.0.0 — 2026-08-20

首个公开版本：DSH Web 插件 + Tauri 2 桌面启动器（Windows），把桌面端的启停、退出确认与
桌面快捷方式联动做成标准 Web 插件。

**新增**

- **桌面端启动开关**：在「设置 → 桌面启动」一键启动 / 退出桌面应用，点击即反馈、状态秒级同步。
- **快捷方式联动**：开启时自动创建桌面快捷方式，关闭时自动删除，运行中也可手动补建。
- **关闭确认弹窗**：关闭前确认，并提示快捷方式将被删除。
- **状态可视**：运行中 🟢 / 已停止 ⚪ / 状态未知 🟡，出错时显示诊断信息。
- **深色模式适配**：颜色全部走 DSH 主题令牌，深浅色自动适配。
- 设置导航区图标替换为显示器图标（矢量绘制，任意缩放锐利）。
- 桌面应用本体：自动检测并安装全局 `@deepseek-ai/dsh`、拉起 DSH Web、系统托盘、
  开机启动、全局快捷键；仓库同时附带桌面应用完整源码与 Release 自动构建流程。

<!-- en -->

### English

First public release: a DSH Web plugin plus a Tauri 2 desktop launcher (Windows) that turns
starting/stopping the desktop app, exit confirmation and desktop-shortcut handling into a
standard Web plugin.

**Added**

- **Desktop app switch**: start/stop the desktop app from Settings → Desktop Launcher, with
  immediate feedback and sub-second state sync.
- **Shortcut integration**: the desktop shortcut is created when enabled, removed when
  disabled, and can be recreated manually while running.
- **Exit confirmation dialog** which also warns that the shortcut will be removed.
- **Status at a glance**: running 🟢 / stopped ⚪ / unknown 🟡 with diagnostics on error.
- **Dark mode support**: all colours come from DSH theme tokens.
- The settings navigation icon is replaced with a vector-drawn monitor icon.
- The desktop app itself: detects and installs the global `@deepseek-ai/dsh`, starts DSH Web,
  tray icon, autostart and a global hotkey; full source and an automated release build ship
  with the repository.
