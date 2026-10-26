# 架构说明

## 组成

`dsh-tauri-launcher` 是一个**双面 Web 插件**（组合包），加一个**Tauri 桌面应用工程**：

```
┌──────────────────────────── DSH Web 进程（Node） ────────────────────────────┐
│  宿主半 lib/index.js（cordis 插件，inject: webServer）                        │
│    · /api/dsh-tauri-launcher/state            GET  状态+诊断                  │
│    · /api/dsh-tauri-launcher/set-desktop      POST 启动/退出桌面应用          │
│    · /api/dsh-tauri-launcher/set-shortcut     POST 创建桌面快捷方式           │
│    （所有路由仅接受回环请求）                                                  │
└──────────────────────────────────────────────────────────────────────────────┘
        ▲ 同源 fetch（127.0.0.1:3080）              ▲ subprocess 服务（无沙箱）
        │                                           │ spawn / PowerShell
┌───────┴─────────────────────┐        ┌────────────┴──────────────┐
│  浏览器半 lib/client.js      │        │  桌面应用（Tauri）          │
│  · settings.section 分区     │        │  · 1 秒轮询：写心跳+查退出  │
│    「桌面启动」               │        │  · 托盘/设置：自启、快捷键  │
│  · 开关/按钮/弹窗/诊断       │        │  · 退出标记消费即退出      │
└─────────────────────────────┘        └───────────────────────────┘
```

## 标记文件协议（桌面应用 exe 同目录）

| 文件 | 写入方 | 语义 |
| --- | --- | --- |
| `.dsh-heartbeat` | 桌面应用（每秒） | Unix 时间戳；插件在 `freshSecs` 秒内读到新鲜值即判定运行中；文件残留不删，靠时间戳判活 |
| `.dsh-quit` | 插件 | 内容 `1` 且 60 秒内新鲜 → 桌面应用**仅退出自身**（Harness 进程保留），消费时自行删除；插件在确认退出后写回 `0` 取消残留标记 |

设计要点：

- 插件跑在沙箱内，`tasklist` 等进程探测不可用 → 用文件时间戳判活；
- 心跳新鲜窗口必须**大于写入周期**（默认 4 秒 vs 1 秒写入），留出读取抖动余量；
- 退出确认双信号：退出标记被自删（快速确认，≤3.5 秒）或心跳过期（兜底，~4-6 秒）；标记未消费且心跳过期则 `Stop-Process` 强杀兜底。

### 候选目录探测（协议落在哪一份副本上）

标记文件与桌面应用配置都在 exe 同目录，所以「插件认哪一份副本」直接决定状态能否
同步：桌面应用可能有多个副本（npm 包内 `launcher/bin`、本地 `build.ps1` 产物），
而 DSH 是被哪一份拉起的并不确定。插件按优先级探测候选目录：

1. 行配置 `launcherExe` 所在目录；
2. 运行中的 `dsh-launcher` 进程目录（`Get-Process -Name dsh-launcher` 取 `Path`）；
3. 桌面快捷方式 `DeepSeek Harness.lnk` 的目标目录（WScript.Shell 读 `TargetPath`）；
4. 行配置 `launcherDirs`；
5. 本次插件运行中发现过的目录；6. 包内 `launcher/bin`。

- `exeDirs()` 只保留**确实含 `dsh-launcher.exe`** 的目录，失效的快捷方式目标自动跳过；
  `pickExe()` 取列表首项，因此“启动哪一份”也遵循同一优先级（快捷方式目标优先于包内副本）；
- 运行中实例与快捷方式目标用 subprocess 探测（**采集模式**：`stdio.stdout =
  { maxBytes }` + `handle.collected.stdout.readFrom(0)`），脚本前置
  `[Console]::OutputEncoding = UTF8`——PowerShell 5.1 默认按控制台代码页写重定向输出，
  非 ASCII 路径会乱码；
- 探测结果缓存 5 秒（状态轮询与启停等待循环都会反复取候选目录）；探测失败时
  静默回退到第 4/6 项，不改变旧版行为；
- 发现过的目录会话内**记住**（最近使用在前，上限 8）：实例退出后其心跳文件变成
  「过期」而非「不存在」，状态因此稳定落在「已停止」，退出流程也不会中途丢目标。

## 启动/退出时序

**启动**：残留退出标记为 `1` 时先取消 → `subprocess.spawn` 拉起 exe（stdio 用**收集
模式**：`{stdin:'ignore', stdout:{maxBytes}, stderr:{maxBytes}}`，graceMs 3000；理由见
「关键实现约束」第 6 条）→ 等心跳新鲜（20 秒窗口）→ 若快捷方式缺失则创建。拉起前把该
exe 目录记入候选列表，避免探测缓存仍是旧值时漏看新实例的心跳；应用若在就绪窗口内退出，
把退出码与它自己的输出作为错误返回（`appExit` / `appOutput` 两行进诊断）。

**退出**：写 `.dsh-quit`=`1`（写进**探测到的实例目录**，见上节）→ 双信号确认
（≤6 秒）→ 清除残留标记 → 删除桌面快捷方式；确认失败走
`Stop-Process -Name dsh-launcher -Force` 强杀（TerminateProcess，不影响 Harness 进程）。

桌面应用侧：勾选「退出时结束 Harness 进程」时，托盘/标记退出统一经 `begin_exit`
——先销毁主窗口与设置窗口（`close_visible_windows`，屏幕上只留反馈窗口），再弹出
紧凑 dialog 式独立 `exiting` 进度窗口（spinner + “正在退出 DeepSeek Harness 进程…”），
然后在后台线程执行 taskkill 清理，避免同步终止阻塞 UI 导致动画白屏；未勾选（默认）
则立即退出、不显示动画。

## 快捷方式联动

- 快捷方式状态 = 桌面 `.lnk` 文件是否存在（与桌面应用自带设置同源，天然同步）；
- 创建/删除/存在性判断走 subprocess+PowerShell（WScript.Shell / Test-Path /
  Remove-Item），桌面路径用 `[Environment]::GetFolderPath('Desktop')` 解析
  （适配 OneDrive 重定向），退出码回传结果，带 5 秒存在性缓存。

## 关键实现约束（踩过的坑）

1. `subprocess.spawn` 的 `stdio` 必须是**对象**（每流 `ignore/pipe/inherit`），
   字符串会触发 `undefined.maxBytes` 校验错误；`graceMs` 必填且为正有限数。
2. 插件直写非工作区路径会被沙箱拒绝 → 所有“写”操作（标记文件、快捷方式）
   一律走无沙箱的 subprocess 子进程。
3. 关闭流程**不能覆写心跳文件**——覆写会让“等待退出”探测立即误判“已停止”，
   进而在桌面应用轮询前取消退出标记。
4. 设置分区导航图标由外壳按分区 id 硬编码映射，未知 id 回退齿轮；本插件用
   CSS（导航第 5 项）替换为显示器图标——依赖分区排序，外壳升级后可能失效，
   属可接受降级（退回齿轮）。
5. 客户端错误需**常驻显示**（自动刷新不得清空），诊断信息仅在出错时展示。
6. **拉起桌面应用必须用收集模式 stdio，不能用 `inherit`**（2026-09-15 实测）：DSH 自身
   可能是被桌面应用拉起的，它的 stdout/stderr 就是桌面应用创建的管道（`harness.rs` 用
   `Stdio::piped()` 拉起 `dsh web`）。桌面应用退出后读端消失，此后以 `inherit` 拉起的
   子进程继承的是**已断开的手柄**，一写即 panic 退出（实测 `exit code 101`；现象是「设置
   里打开开关桌面端起不来，关闭方向却一直正常」——关闭只写标记文件）。收集模式让管道由
   DSH 侧持有，顺带把应用启动期输出留给诊断。

## 外壳页承载与 DSH 鉴权适配（2026-09-09）

桌面应用主窗口**不再**直接使用 tauri 资产协议（`http://tauri.localhost`），改由内置
静态服务承载于 `http://127.0.0.1:3081`（`src-tauri/src/shell_server.rs`，编译期
`include_str!` 内嵌 `ui/` 四文件：index.html / main.js / styles.css / vars.css）。

- **共用色板**：`ui/vars.css` 是三页共用的**静态**默认色板（`--c-*` 变量），与
  `--tb-*`（运行时主题覆盖，main.js 按 DSH 主题写入）分工互补。三个 HTML 里它必须
  排在各自样式表**之前**——`:root` 优先级相同、后加载的覆盖先加载的，顺序反了会把
  页面自己的样式压掉（`shell_server.rs` 有测试钉住这条顺序）。settings / exiting 走
  tauri 资产协议、直接读盘取它，只有外壳页需要 `/vars.css` 路由。
- **为什么换**：新版 `dsh web` 的会话 cookie 为 `SameSite=Strict`，而 SameSite 比较
  「站点」时**忽略端口、只比主机**。`tauri.localhost` 与 DSH 的 `127.0.0.1:3080`
  跨站 → WebView2 拒收 iframe 内 token 握手 303 响应的 `Set-Cookie` → iframe 永久
  白页（仅显示一行 401 英文提示）。改到 `127.0.0.1:3081` 后二者同站（同主机、不同
  端口），cookie 正常签发与发送。
- **启动顺序**：服务在 `main.rs` 中**同步 bind 完成后**才让 tauri 建窗口；异步 bind
  会与建窗口竞态，主窗口命中 `ERR_CONNECTION_REFUSED` 且不重试（表现为白窗）。
- **ACL**：外壳页对 tauri 而言是远程 origin，其应用命令必须显式授权
  （`permissions/launcher.toml` 的 `allow-launcher-commands` + capability 的
  `remote.urls`），否则 IPC 报 `Command X not allowed by ACL`。
- **鉴权握手**：`launch_dsh` 捕获子进程 stdout 的 `dsh web: <token URL>` 行并导航
  iframe 完成 token→cookie 交换；旧版 DSH 仍走裸 `GET /` 的 `__DSH_BOOT__` 指纹路径。
  接管外部实例但 WebView2 无 cookie 时，外壳提示条提供「关闭并重启」
  （`restart_dsh_external`：netstat 定位占用进程 → taskkill → 自启动完成握手）。
- settings/exiting 两个辅助窗口仍走 tauri 资产协议（不涉及 DSH cookie）。
- **外壳 origin 与插件侧校验（2026-09-12 修正）**：外壳改由 `127.0.0.1:3081` 承载
  后，插件**不能只依赖** `document.referrer` 推导父 origin——实测（DSH 0.1.5 +
  WebView2）iframe 经 token 303 握手后 `document.referrer` 为**空串**，推导值退化
  为兜底常量 `http://tauri.localhost`，与外壳实际 origin 不符，导致「外壳 → 插件」
  的消息（导航命令、系统主题）被 `event.origin` 校验全部丢弃（症状：◀/▶ 点击无
  响应、Windows 主题不跟随；探针实测 34 次 ping 全部 match=false）。
  现行为：`isShellMessage()` 以 `event.source === window.parent` 作身份校验，origin
  按「referrer 推导值 **或** 已知外壳白名单（`http://127.0.0.1:3081`、
  `http://tauri.localhost`）」收口。**改动外壳端口时须同步更新插件侧
  `SHELL_ORIGINS`**（两侧常量需保持一致）。

## 剪贴板（2026-10-21）

**现象**：外壳窗口里选中文字按 Ctrl+C、或点 DSH 自带的复制按钮，剪贴板都没有内容，
按钮也没有「已复制」反馈。

**根因两处，缺一不可**：

1. **跨源 iframe 的 Permissions Policy**：外壳在 `127.0.0.1:3081`、DSH 在
   `127.0.0.1:3080`，端口不同即不同 **origin**（SameSite 只比主机，权限策略不认这套）。
   `clipboard-write` 的默认白名单是 `self`，iframe 不声明 `allow` 时
   `navigator.clipboard.writeText()` 一律抛 `NotAllowedError`；DSH 的复制按钮
   （`zo()`）随后的 `document.execCommand('copy')` 兜底同样拿不到剪贴板，
   于是按钮连「已复制」状态都不出现。
2. **宿主没打开剪贴板权限**：tauri 的 `WebviewAttributes.clipboard` 默认 **false**，
   wry 只在它为 true 时才注册 WebView2 的 `PermissionRequested` 处理器
   （wry `webview2/mod.rs` 的 `if attributes.clipboard` 分支），且该处理器只自动放行
   `CLIPBOARD_READ`。主窗口 builder 从未调用 `enable_clipboard_access()`。

**修复**：`ui/index.html` 的 iframe 加 `allow="clipboard-read; clipboard-write"`；
`windows.rs` 的主窗口 builder 链上加 `.enable_clipboard_access()`。

**注意**：

- `enable_clipboard_access()` 只作用于**它所在的 builder**。设置窗与退出进度窗各自独立，
  目前不需要剪贴板，未开。
- 以后若再嵌新的跨源 iframe（端口不同也算），同样要显式写 `allow`——默认不继承父页的
  权限策略，症状是 API 静默抛错而不是报权限错误。
- Ctrl+C 是浏览器原生编辑命令，不走 `navigator.clipboard`；本次一并修的是它依赖的
  剪贴板通道，改完需真机复测两条路径。

## 外壳 ↔ 插件消息协议（postMessage）

外壳页（`ui/main.js`，承载于 `http://127.0.0.1:3081`）与 iframe 内插件的双向通道。
两侧各自定义同一套常量（外壳与插件的 `MSG`），字段以本节为准：

| 方向 | type | payload | 旧字段（兼容期） |
| --- | --- | --- | --- |
| 外壳 → 插件 | `navCommand` | `{ dir: 'back' \| 'forward' \| 'ping' }` | `__tbNav` |
| 外壳 → 插件 | `systemTheme` | `{ scheme: 'light' \| 'dark' }` | `__tbSystemTheme` |
| 插件 → 外壳 | `navStatus` | `{ back, forward }` | `__tbNavStatus` |
| 插件 → 外壳 | `themeSync` | `{ scheme, bg, fg, menuBg, menuBorder, sep, danger }` | `__dshLauncherTheme: 1` + 同名字段 |

信封格式 `{ v: 1, type, payload }`。**兼容期**（2026-09 起）：发送端双发（信封 +
旧字段），接收端双解析（`readEnvelope(data, type)` 优先，失配回退旧字段），未知
`type` 静默忽略——因此旧 exe + 新插件、新 exe + 旧插件均可工作；下个大版本移除
旧字段。

其他约定：

- 双方都校验 `event.origin`：外壳比对 iframe URL 的 origin，插件比对由
  `document.referrer` 推导的父 origin（兜底 `http://tauri.localhost`）；
- 插件 → 外壳用 `postMessage(..., '*')` 投递（WebView2 对虚拟主机的精确
  targetOrigin 匹配有丢弃嫌疑），安全性由接收端 origin 校验保证；
- 浏览器直开（非 iframe）时插件自动不启用导航与系统主题跟随。

## 协议超时与常量清单

| 常量 | 值 | 位置 | 语义 |
| --- | --- | --- | --- |
| 心跳写入周期 | 1 秒 | `markers.rs` | 桌面应用写 `.dsh-heartbeat` |
| 心跳新鲜窗口 | 4 秒（`freshSecs`） | `lib/index.js` | 插件判定“运行中” |
| 退出标记新鲜窗口 | 60 秒 | `markers.rs` | `.dsh-quit` 内容 `1` 的时效 |
| 启动等待心跳 | 20 秒 | `lib/index.js` | 拉起 exe 后的就绪窗口 |
| 退出确认 | 12 × 500ms（≈6 秒） | `lib/index.js` | 双信号确认上限 |
| 启动就绪软超时 | 180 秒 | `harness.rs` | 到点不再当失败，只标记「较慢」并继续等 |
| 启动就绪硬超时 | 600 秒 | `harness.rs` | 真正的失败上限（终止进程并报错） |
| 启动进度广播周期 | 1 秒（2 × 500ms 循环） | `harness.rs` | `launch-progress` 事件节流 |
| 接管重启端口释放等待 | 10 秒 | `harness.rs` | 终止外部实例后等端口 |
| 外壳 ping 周期 | 3 秒 | `ui/main.js` | 导航状态自愈 |
| 服务重试梯子 | 12 × 500ms | `lib/client.js` | theme / sessions 就绪重试 |
| 跳转锁超时 | 1.5 秒 | `lib/client.js` | 会话栈 pendingJump 兜底 |
| 快捷方式存在性缓存 | 5 秒 | `lib/index.js` | 减少 PowerShell 调用 |

## 标题栏与窗口外壳

主窗口无边框（`decorations: false`）：DSH 就绪前显示启动卡片（检查/安装/启动/错误
各 phase），就绪后隐藏卡片、显示 40px 自绘标题栏并用 iframe 承载 DSH（不再整页跳转）。

- **三键**：最小化 / 最大化还原 / 关闭；关闭被 Rust 侧 `CloseRequested` 拦截为「隐藏到托盘」，
  真正退出走托盘菜单或标题栏菜单的「退出」；
- **◀/▶**：会话导航（见下节），另支持 Alt+←/→ 与鼠标侧键；可用性由插件回传后置灰；
- **标题栏菜单**：文件（设置 / 重新加载页面 / 在浏览器中打开 / 退出）· 窗口（关闭窗口）·
  关于（关于 Launcher / 检查更新）· 帮助（官网 / 文档）。面板与条目由
  `ui/main.js::setupMenus()` 的数组（`MENUS` / `MENU_ITEMS`）驱动互斥与绑定——
  **新增菜单 = 一个 `.menu-anchor` + 一条 `MENUS` 数据**，按钮与面板按 `.menu-btn` /
  `.menu-panel` **class** 定位（不是 id），故 `styles.css` 无需改动，互斥逻辑也无需改动。
  「关闭窗口」走 Rust 命令 `hide_all_windows`（`hide()`，**不是**退出流程的 `destroy()`），
  恢复走托盘「打开」或全局快捷键；
  外链 URL 表唯一维护在 Rust 侧（`EXTERNAL_LINKS`），前端只传 key（`dsl` 为本地 DSH，
  地址取自应用状态以带上 token；`releases` 为启动器下载页）；实际打开统一走
  `windows::open_with_system_browser` 里的 `ShellExecuteW`——**不要换回 `explorer.exe`**：
  它会把带查询串的地址当成本地路径，`http://127.0.0.1:3080/?token=xxx` 会开出文件资源
  管理器（2026-10-05 实测：裸地址进浏览器、带 token 的进资源管理器）；
- **窗口拖动**：`data-tauri-drag-region`；capability 必须显式包含
  `core:window:allow-start-dragging`（`core:window:default` 不含，缺失时拖动被静默拒绝）。

## 关于弹窗与更新检查

「关于」菜单的两项共用一个**页内 overlay**（`index.html` 的 `#about-overlay`），
数据来自 `info.rs` 的两个命令：

- **「关于 Launcher」**（`get_versions`）：三项版本各自独立探测、各自容错——启动器
  （`app.package_info().version`）、DSH（`dsh::check().version`）、Node
  （`node --version`）。未安装显示「未安装」、探测不到显示「未知」，任一项失败都不影响弹窗；
- **「检查更新」**（`check_updates`，点菜单项会直接开弹窗并自动开始检查）：
  - 只查**两项**：① 启动器 + 插件（**同号**——`build.ps1 -Bump` 同步三处版本号，npm 包自带
    exe，拆两项等于把同一个数字展示两遍）② DSH 本体（独立版本号）。两项在 Rust 侧**并行**查询；
  - 查询复用既有 npm 工具链（`npm view <pkg> dist-tags --json`，Windows 上经 `cmd /C`），
    **不引入 HTTP 依赖**——外壳页的 CSP 是 `default-src 'self'`，页面无法直连 registry，
    必须由 Rust 发起；
  - **只看 `latest` + semver 比较**（`latest_version` / `has_update`）：`latest` 正是
    `dsh plugin add` 不带版本号时拿到的渠道，也就是用户默认安装的那一版；`next` / `alpha` /
    `beta` 指向更早的预览，一律不认（实测 DSH 官方是 `latest`=0.2.0-rc.2、`alpha`=
    0.2.1-alpha.1，按「全 tag 取最高」会把 alpha 推给 rc 用户）。比较用 `semver`——预发布
    低于同号正式版，字符串比较会得出相反结论（旧实现曾因此把 rc 用户报成「有新版本
    v1.0.11」，那其实是降级）。`UpdateItem.has_update` 把结论带给前端，缺失时前端退回
    字符串判断（旧版行为）；
  - 超时 **8 秒**（本机实测单次约 3.7 秒），失败给明确原因（超时 / npm 缺失 / registry 报错），
    不长时间转圈；
  - 真有新版本时才给出**出路三件套**：**当前正在运行的 exe 完整路径**（`current_exe()`，
    多副本场景下必须是这一份，否则用户会替换错文件）、「打开下载页」（`releases` 外链）、
    「复制升级命令」（`dsh plugin --profile <p> add @lenorin/dsh-tauri-launcher`，profile 名
    从已装插件路径推断，回退 `web`）；
  - **漂移不占检测项**：只有当运行中的 exe 与已装插件版本不一致（自己从 GitHub 下载 exe 的场景）
    时才多显示一行附注；
  - **不做后台轮询**——只在用户点击时发起，避免无谓网络请求。
- **为什么是 overlay 而不是新窗口**：任何新 webview 都必须遵守「启动早期预建」约束
  （主窗口 iframe 加载跨源 DSH 后同步 build 第二个 webview 会死锁），成本与风险都远高于一个
  overlay。
- 关闭方式三种：Esc、点遮罩、点关闭按钮（`.hidden` 自带 `!important`，不受 overlay 的
  `display: flex` 影响）。

### DSH 一键升级（2026-10-07，roadmap W15）

链路是**停 → 装 → 起**，三步全部复用既有命令，**没有新增后端命令、也没动 ACL**：

1. `stop_dsh` 终止本启动器托管的进程树；
2. `install_dsh`（即 `npm install -g @deepseek-ai/dsh`）——**必须先停再装**：npm 覆盖包文件时
   若 DSH 仍在运行，可能因文件占用而失败；
3. `launch_dsh` 拉起新实例，用它返回的地址走与启动流程同一条 `showFrame(url)` 重新导航 iframe。

实现要点：

- **入口按需出现**：只有 `check_updates` 报出 DSH 自己有新版时才显示「升级 DSH」按钮（升的是
  DSH 本体，与「启动器 + 插件」那条线彼此独立）。
- **两步确认**：第一次点击只把按钮切成「确认升级？会重启 DSH」，4 秒内再点才执行——升级会
  中断用户正在用的实例，不能做成一步。用自绘的两步按钮而不是 `window.confirm`，不依赖
  原生对话框在 WebView 里的可用性。
- **进度**：订阅 `install-output` 事件（Rust 侧 `install_dsh` 逐行 emit），只保留最后 8 行；
  失败时日志留在屏幕上，npm 的报错原因就在里面。
- **接管外部实例时**：`stop_dsh` 对「接管的外部实例」是空操作（不终止别人的进程），此时
  npm install 可能因占用失败——错误原样透传给用户，不假装成功。
- `launch_dsh` 返回 `auth_uncertain` 时给出「关闭并重启」指引，与外壳既有的 401 提示条同源。
- 升级成功后顺带刷新三版本号与「检查更新」结果，避免弹窗里留着陈旧数字。

## 标题栏主题跟随链路

目标：标题栏与 DSH 配色一致，并在 DSH 偏好为「跟随系统」时随 Windows 主题切换。

1. 插件读 DSH **真实生效**的 token（`getComputedStyle(document.body)` 的 `--dsw-*`；
   内置主题快照的 tokens 是空表，只能读计算样式，必要时手动解一层 `var()` 链），
   连同 `scheme` 经 postMessage 发给外壳；
2. 外壳 `applyTitlebarTheme()` 逐项写入 `--tb-*` 变量，颜色经 `pickColor()` 形状校验，
   非法值回退内置色板；
3. **读取必须延后到当前任务之后**（`setTimeout 0`）：DSH 的 ThemePresenter 写
   `body[data-ds-dark-theme]` 与本插件的 `theme/change` 监听器同处一次**同步**派发，
   谁先注册不保证，同步读会拿到上一个主题的 token（表现为标题栏滞后一个主题）；
   另有 `body` 主题属性的 MutationObserver 兜底与同状态签名去重；
4. 插件缺席（未安装 / 浏览器直开）时，外壳回退 `matchMedia('(prefers-color-scheme: dark)')`；
   插件一旦上报过主题（`pluginThemeApplied`）即不再介入；
5. **菜单与弹窗背景取「窗口同色」**（`--tb-menu-bg` = 标题栏背景），**不**取 DSH 的 overlay
   token——后者在深色主题下是 bluish-700 中灰，压在近黑的标题栏上过于突兀（浅色下也差着一档）。
   同色后层次由既有的边框与阴影承担。插件仍在上报 `menuBg`，外壳已不透传该字段
   （协议字段保留，待大版本清理）。

## Windows 系统主题跟随

WebView2 的 `prefers-color-scheme` 只有宿主显式设置 `PreferredColorScheme` 才会随
Windows 变化，而 tauri/wry 未暴露该能力（wry#806）→ DSH 的 `system` 偏好会冻结在
启动值，`matchMedia` 的 change 监听永不触发。补偿链路：

- Rust 每 2 秒查注册表 `AppsUseLightTheme`，变化时刷新托盘/窗口图标并向主窗口
  `emit("launcher-system-theme")`；
- 外壳转发给 iframe 内插件；插件在偏好为 `system` 且未处于该方案时，用
  `Object.defineProperty` 覆盖 ThemeRuntime `media.matches` 为真实系统值并 `publish()`。

**偏好零改写**：不调用 `setTheme()`，用户设置面板不会被改动；用户固定了具体主题时让位。

### 任务栏按钮图标的来源（2026-09-13 实测，重要）

**Win11 任务栏按钮画的是「应用标识」的图标，不是窗口图标。** 实测（25H2 / 26200.9445）：
切换系统主题时，主窗口的四个图标槽（`WM_GETICON` BIG/SMALL、`GCLP_HICON/HICONSM`）
全部换成了新句柄，托盘图标也跟着变，但任务栏按钮纹丝不动——连
`ITaskbarList::DeleteTab + AddTab` 强制重建按钮也不变。

取按钮像素比对后确认：按钮主体是 `(250,250,250)`，正好等于 exe 内嵌的
`icons/icon.png`，而窗口图标当时是 `(15,17,21)`（浅色主题用黑鲸）→ **任务栏画的是
exe 内嵌图标**。所以：

- `set_icon`（ICON_SMALL）、`WM_SETICON ICON_BIG`、`GCLP_HICON/HICONSM` 这几条路径
  实际影响的是**标题栏与 Alt-Tab**，改不动任务栏按钮（v1.0.6/v1.0.7 两次修复因此都
  没治本；深色任务栏上白色鲸鱼看着正常，一切到浅色就“失效”）；
- 托盘图标走的是另一条 shell 通道，**是**跟随主题的；
- 任务栏按钮要显示什么，只能由 **exe 内嵌图标**（`icons/icon.ico`）决定，运行时不改；
  因此应用图标本身必须是**浅色/深色任务栏都看得清**的配色（现为品牌蓝 `#4D6BFE`），
  `icon-black.png`/`icon-white.png` 只服务于托盘与窗口图标；
- `main.rs` 之外，`run()` 在**建窗口之前**调用
  `SetCurrentProcessExplicitAppUserModelID("com.dsh.launcher")`（与
  `tauri.conf.json` 的 `identifier` 一致，有单元测试钉住）：不显式设置时 Windows 按
  exe 推导身份，任务栏更容易只认 exe 图标；这也是社区修任务栏图标问题的标准前置步骤。

## 会话导航栈（应用层）

DSH 是 React SPA，切会话不产生浏览器历史（`history.back()` 会退回 iframe 加载前的空白页），
故用应用层双栈实现前进/后退：

- **状态源**：插件 `ctx.get('sessions').list`（SnapshotStore：`getSnapshot()` 给出
  `current` 与 `currentAddress`，`subscribe(fn)` 返回退订函数）；跳转用
  `open(id)` / `openSubagent(address)`；
- **语义**：非跳转来源的 current 变化 → 上一个会话压 backStack 并清空 forwardStack；
  ◀ 弹 backStack（当前会话压 forwardStack），▶ 反向；栈上限 50、连续重复跳过；
- **锁**：跳转前记录 pendingJump，避免自身触发的变化被当成用户切换重复压栈；
  1.5 秒超时兜底解锁并重新同步快照；
- **状态回传**：插件在初始化与每次变化后上报 `navStatus`，外壳据此置灰按钮；
  外壳每 3 秒 ping 一次实现自愈（单次消息丢失可恢复）。

## 设置 / 退出进度窗口与预建约束

主窗口 iframe 加载跨源 DSH 之后，主线程**同步 build 第二个 webview 会死锁**
（WebView2 多窗口竞态：实测 `build()` 永不返回、事件循环停摆）。因此：

- `settings` 与 `exiting` 两个辅助窗口在启动早期**预建**（`visible(false)` 防闪现），
  之后只 `show()` 复用；**退出路径绝不做现建兜底**（预建失败宁可没有动画）；
- `exiting` 为紧凑 dialog（`skip_taskbar` + `always_on_top`），由 `exiting.js` 按内容
  高度 `setSize`（需要 capability `core:window:allow-set-size`）；其样式自带于
  `exiting.css`，不依赖外壳样式表；
- 退出动画期间先 `destroy()` 主窗口与设置窗口（`close()` 会被「隐藏到托盘」拦截），
  再显示进度窗口；清理（taskkill 等）放到后台任务，避免阻塞 UI 导致动画白屏。
- **主窗口同样由 Rust 建，且必须排在预建之后**（2026-10-02）：`tauri.conf.json` 的
  main 配置是 `create: false`，tauri 不自动建它，改由 `windows::build_main_window`
  用 `from_config` 读同一份配置建——**为的是能挂 `on_new_window`**。WebView2 默认吞掉
  新窗口请求，没有 handler 时 DSH 页面里 `target="_blank"` 与 `window.open` 全都没
  反应；而 handler 只能在建窗口那一刻挂，配置自动建的窗口挂不上。窗口属性仍全部读
  同一份配置，视觉与尺寸行为不变。`setup()` 的顺序是 `prebuild_aux_windows` →
  `build_main_window` → 图标 / 托盘：辅助窗口排在前面是因为主窗口一旦开始加载跨源
  DSH 就会触发上面的死锁，托盘排在后面是因为「打开」要能取到主窗口。新窗口请求一律
  返回 `Deny`（不开内嵌窗口），只把 http/https 交给系统浏览器。

## 系统托盘交互（2026-10-03）

托盘图标（`tray.rs` 的 `build_tray`）的鼠标约定：

- **左键单击 = 打开或聚焦主窗口**：`on_tray_icon_event` 取 `TrayIconEvent::Click` 且
  `MouseButton::Left` + `MouseButtonState::Up`，与菜单「打开 DeepSeek Harness」共用
  `open_or_focus_main`——已可见且未最小化只聚焦、最小化则还原、「关闭窗口」隐藏后
  重新显示（窗口实例仍在，iframe 会话不重载）。
- **菜单改右键弹**（`show_menu_on_left_click(false)`）；改之前是左键单击弹菜单。
- **不做双击**：左键单击已经把窗口打开了，双击只会让同一次操作触发两遍——Windows 上
  双击必先落两次单击事件，两个分支都处理等于重复调用。
- 只认**抬起**（`MouseButtonState::Up`）：Windows 下按下与抬起各来一次事件。

## 日志落盘与设置窗日志页（2026-10-07）

接官方 `tauri-plugin-log`（不是自研 writer）：轮转、归档保留、本地时区、fern 格式化
都由它提供；各处诊断输出改用 `log` 宏后同时进 stdout 与文件。

- **落点**：`%LOCALAPPDATA%\dsh-launcher\logs\launcher.log`。用 `TargetKind::Folder`
  精确指定目录——不用插件的 `LogDir`（那是 `%LOCALAPPDATA%\<bundleIdentifier>\logs`）。
- **轮转**：插件按**单文件大小**轮转（`max_file_size` 设为 5 MB），归档名
  `launcher_YYYY-MM-DD_HH-MM-SS.log`，`RotationStrategy::KeepSome(5)` 保留最近 5 个。
  它**没有按天切割**——roadmap §4.1 里「按天 + 大小」的写法据此修正；定位某天的日志
  看归档名即可。
- **开关**：插件的 targets 在启动时固定、运行时无法增删（全局 logger 也只能设置
  一次），所以「写入日志文件」开关做成文件目标上的过滤条件（读 `logging::ENABLED`），
  切换后下一条日志即生效，**不需要重启**。
- **不可写降级（重要）**：插件 setup 里对 `Folder` 目标会先 `create_dir_all` 再打开
  文件，任一步失败都返回 `Err`，而插件 setup 失败会让 tauri 的 `build()` 直接失败、
  应用起不来。所以挂文件目标**之前**先用 `logging::is_writable()` 探测（建目录 + 写
  探测文件），不可写就不挂文件目标、只保留 stdout。
- **token 脱敏**：新版 `dsh web` 会向 stdout 打印带进程 token 的 URL，该 token 每次
  启动都换、对排障无用，落进文件再被贴进 issue 等于外泄；因此日志泵
  （`harness::spawn_log_pump`）写日志前统一过 `sanitize_token()`。内存环形缓冲
  （`AppState::log_tail`，256 行）**保持原文**——它承载「启动失败/超时把最近日志随
  错误信息返回外壳」这条当场排障路径，两份记录分工不同。
- **依赖代价**：新增 `tauri-plugin-log` 与 `fern`，本机 `.cargo` 离线缓存原本没有，
  首次构建需要联网拉一次（`cargo fetch`，可走 127.0.0.1:7890 代理），之后 `-Offline`
  照旧。cargo 实际解析到 2.9.2（2.10.0 因依赖约束未被选中），MSRV 1.77.2，当前工具链
  1.91.1 无压力。
- **设置窗日志页**：开关 + 目录 + 当前文件大小/归档数 + 「打开日志目录」。目录与体量由
  `get_log_info` 给出，打开走 `open_log_dir`（复用 `open_with_system_browser`，不新增
  打开方式）。窗口高度随之由 540 调到 624——设置窗 `resizable(false)`，加内容必须同步
  改 `windows.rs` 的 `inner_size`。
- **ACL 自动化**：`lib.rs` 的 `every_command_is_authorized_in_acl` 把「Rust 声明的命令
  都在 `permissions/launcher.toml` 授权」钉住，替代原先手工跑的审计脚本（漏授权**不
  编译报错**，只在运行时以 `Command X not allowed by ACL` 被拒）。

## 启动等待的阶段进度与耐心线（2026-10-13）

**背景**：DSH 0.2.0 起，`dsh web` 的 token URL 行（`dsh web: http://127.0.0.1:3080/?token=…`）
改在**整棵插件树 settle + 启动审计通过之后**才打印——`dsh-web-app/lib/index.js` 的
`announceReady` 被移进了 `loader.await()` 之后的分支（0.1.5 是启动后即打印）。
而启动器的就绪判定要求「裸 `GET /` 返回 401 **且**已捕获该行」，于是等待时长
完全取决于插件加载速度。

**实测（2026-10-12，`dsh web --no-open --port <空闲端口>`，计时到 token URL 行出现）**：

| 配置 | 端口监听 | token URL 行 |
| --- | --- | --- |
| 干净 `DSH_HOME`（零插件） | T+2s | T+2s |
| 官方 base + web-app | T+2s | T+2s |
| 用户 profile 全量 15 个 bundle | T+151s | **T+192s** |
| 全量**减去 `dsh-purge`** | T+4s | **T+7s** |
| 只加 `dsh-purge` | T+163s | **T+202s** |

结论：慢的**全部来自第三方插件**，其中 `dsh-purge`（v1.1.67）一个就占 185 秒；
`@linxin666/dsh-web-all`（聚合 19 个 UI 插件）只占 7 秒、`@lenorin/dsh-tauri-launcher`
单独 4 秒。当时 180 秒超时正好卡在门外，于是「等三分钟 → 失败 → 重试再等三分钟」
永远成功不了——这是本项要解决的核心问题。

**现在的行为**（`harness.rs`）：

1. **软超时 180 秒**（`LAUNCH_SOFT_TIMEOUT_SECS`）不再是死刑线：到点只在进度事件里置
   `slow`，外壳把文案改成「启动较慢，仍在继续」并露出「打开日志目录」入口。
2. **硬超时 600 秒**（`LAUNCH_HARD_TIMEOUT_SECS`）才是真正的失败上限。
3. **每秒广播 `launch-progress`**（载荷 `{stage, elapsed_secs, slow}`，节流 2 × 500ms 循环）：
   - `spawning`：刚拉起进程；
   - `loading`：进程活着、端口未监听（插件加载中，实测最长 152 秒）；
   - `serving`：端口已响应（401 或旧版指纹）但尚未拿到 token URL（等整棵树 settle，实测 39 秒）；
   - `cancelled`：用户点了取消。
4. **取消**（`cancel_launch` 命令）：只置位 `AppState.launch_cancelled`，由等待循环下一轮
   自行 `terminate_harness` 并返回 `Err("已取消启动。")`——两条路径不同时操作子进程状态。
   每轮启动开始时清零该标志，否则升级流程里的第二次 `launch_dsh` 会被上一轮的取消打断。

**为什么 `loading` / `serving` 要分开**：这两段在实测里确实分开（152s / 39s），
前者慢是插件加载、后者慢是等审计与 settle；对用户而言「正在加载插件」与
「服务已就绪，正在准备界面」是不同的预期管理——前者可能还要几分钟，后者基本是收尾。

**为什么等待时间无法从启动器侧缩短**：token 只经 `dsh web` 的 stdout 对外，
启动器必须等它才能让 WebView 完成 token→cookie 握手；跳过这一行只会得到 401 白页。
所以优化方向是「让等待可见 + 可取消 + 不把耐心当失败」，而不是取消等待。

**关联**：日志落盘（W10）让「打开日志目录」在慢启动时可用；`terminate_harness_on_exit`
默认 `false`（`ExitMode::Keep`）使 DSH 在启动器退出后作为孤儿常驻，下次启动走
`probe_web` 接管路径、秒开——**慢只发生在冷启动**（开机后首次、DSH 崩溃后，
或用户显式开启了「退出时结束 DeepSeek Harness 进程」）。

## 单实例保护（2026-10-15）

**现象**：重复点击桌面快捷方式，托盘通知区域里堆起多个启动器图标；窗口看起来「正常」，
没有任何报错。

**根因（两层）**：

1. **没有任何单实例检查**——每点一次快捷方式就起一个全新的 `dsh-launcher.exe`，每个进程
   都会创建自己的托盘图标。
2. **第二个实例的外壳服务绑定失败被静默降级**：`shell_server.rs` 里
   `TcpListener::bind(("127.0.0.1", 3081))` 失败时只 `log::error!` 然后 `return`，
   既不 panic 也不退出。于是第二个实例照样建主窗口——而它的 URL 也是
   `http://127.0.0.1:3081`，**加载的其实是第一个实例的外壳页**（同源，浏览器不区分是
   谁在服务）。两个实例还共写同一个日志文件，所以从外面看一切正常。

**修复**：接入官方 `tauri-plugin-single-instance`。第二个实例启动时在回调里把已有主窗口
带到前台，随后新进程自行退出。

**约束与注意**：

- **必须在 builder 里最先注册**——官方明确要求「插件按注册顺序运行，这个插件要排在
  最前」，所以要写在 `tauri-plugin-global-shortcut` 与 `logging::plugin()` 之前。
- 回调按 `windows::WINDOW_MAIN` 取窗口后 `show` / `unminimize` / `set_focus`；
  取不到窗口（首个实例还没建完窗口）时静默跳过，不报错。
- **平台支持**：Windows / Linux / macOS；**Android / iOS 不支持**（README 表格）。本应用
  只做 Windows，不受影响。
- **该插件要求 Rust ≥ 1.90**（本机工具链 1.91.1 满足）。
- **`Cargo.lock` 会多出 44 个包**。其中 zbus / async-* 一族是 **Linux D-Bus 侧的传递
  依赖**，Windows 构建不会编译它们，只是锁文件变大；`tempfile`、`windows-sys 0.60`、
  `windows-targets 0.53` 等则在本平台确实参与构建。
- **不引入 JS 侧依赖、不动 capabilities**：该插件没有前端 API，只在 Rust 侧生效。

**第二个实例是「静默」的（2026-10-16 用户实测修正）**：`main.rs` 里
`start_shell_server()` 在 `run()` 之前执行，而**日志插件是在 `run()` 的 Builder 里才注册**
——第二个实例 bind 3081 失败、走到 `shell_server.rs` 的 `log::error!` 时，logger 还没就位，
`log` crate 因无接收方**直接丢弃该事件**。于是它的完整生命周期是：bind 失败不吭声 →
tauri 初始化 → 单实例插件拦下 → 退出，**日志文件里没有任何痕迹**。

**连带诊断陷阱**：同一原因使 `shell_server.rs` 里那几条 `log::error!`（bind 失败 /
非阻塞设置失败）在本应用的实际启动顺序下**永远落不到日志文件**。真出现「外壳服务没起来
导致白窗」时，**不要指望日志给出原因**——那几条消息目前只在 stdout 可见（且窗口是
`windows_subsystem = "windows"`，release 下没有控制台）。要让它们可诊断，需把日志初始化
提到 `start_shell_server()` 之前，或在 bind 失败处改用 eprintln + 自建落盘。

## 启动器自更新与 PID 认领（2026-10-21）

**链路**：`check_launcher_update` 查清单 → `install_launcher_update` 下载 + 验签 → 交给
NSIS 安装器 → 插件内部 `std::process::exit(0)`。应用侧只碰 `tauri-plugin-updater` 的
**Rust API**，不引 JS 插件、不新增 webview；端点与公钥都在 `tauri.conf.json` 的
`plugins.updater`。下载进度按 200ms 节流发 `update-progress`，载荷
`{phase: downloading|verifying|installing, received, total}`。

**错误契约**：所有失败返回 `前缀：中文说明（英文原文）`，前缀只有
`network|manifest|signature|io|unsupported`——外壳靠前缀决定文案与是否给重试入口（网络
可重试，验签失败只能重下）。

**PID 认领**：`LauncherConfig` 里新增可选字段 `dsh_pid`（记本应用拉起的 dsh 子进程）。
启动时若 3080 已被占用，只有三个条件同时成立才认作「上一次自己拉起的」：记录存在且
> 0、该 PID 仍查得到映像路径、映像与当前 `dsh::node_exe()` 规范化后相等（Windows 忽略
大小写与分隔符）。任一不成立一律 `adopt()`（外部实例，退出不动它）——**误杀别人的进程
比漏杀更糟**。

**为什么字段放在 `LauncherConfig` 而不是单独文件**：设置窗每次保存都会把整份
`.dsh-config.json` 重写一遍，字段不在结构体里就会被写没，认领随之退化成外部实例。

**与退出策略的关系**：插件安装前会回调 `on_before_exit`，那里调用既有的
`shutdown::exit_launcher`——所以自更新走的是**同一套**退出语义：默认「保留」模式把 dsh
孤儿化，新实例起来后靠 PID 认领接管；只有用户显式开启「退出时结束 DSH」才会在更新前
结束它。

**发布侧**：`release.yml` 拆成 `build-launcher`（裸 exe，先发）与 `package-nsis`
（`needs` 串联，产 NSIS 安装包 + `.nsis.zip` + `.sig` + `latest.json`）。清单里同时给
`windows-x86_64-nsis` 与 `windows-x86_64` 两个键，后者服务迁移期的裸 exe。签名私钥来自
secret `TAURI_SIGNING_PRIVATE_KEY`，缺它时安装包不会产出（裸 exe 仍照常发布）。

**顺序约束**：workflow 只对「tag 所指 commit」生效，改了 `release.yml` 必须先进 main
再打下一个 tag；`latest.json` 里的下载地址钉住具体 tag，老清单不会因后续发版失效。

## 状态模型（浏览器侧）

`desktop: true | false | null`（运行中/已停止/状态未知）+ `shortcut: bool`。
切换操作走乐观 UI：点击即切开关位置并显示“正在启动…/正在退出…”，确认后定型；
成功后跳过即时刷新防止心跳窗口内回跳，由 10 秒周期刷新收敛。

## 附录：静默失效排查表

**本表只做索引，不重述原理**——每条的原理与实测数据在上文对应章节，这里只回答
「看到什么现象，该去查哪一节、先用哪条命令排除」。

Tauri 侧最贵的 bug 不是崩溃，而是**配错了却不报错**：功能像没接线一样失效，日志干净，
只能靠猜。本表把已踩过的收敛成「症状 → 真因 → 去哪一节」。

| 症状 | 真因（一句话） | 详见 |
| --- | --- | --- |
| 浏览器网页能拖拽上传，桌面端拖上去毫无反应 | WebView2 的 `dragDropEnabled` 默认 `true`，吞掉 HTML5 拖放事件、只发原生事件 | `tauri.conf.json` 的 `windows[].dragDropEnabled: false` |
| 运行时换图标（`set_icon` / `WM_SETICON` / `GCLP_HICON` 都试过）任务栏按钮就是不变 | 任务栏按钮画的是 **exe 内嵌图标**，运行时改不动；那些 API 只影响标题栏与 Alt-Tab | §任务栏按钮图标的来源 |
| iframe 永久白页、只有一行英文小字；手写 HTML 却能显示 | 会话 cookie 是 `SameSite=Strict`，比「站点」时**忽略端口、只比主机** → 跨站拒收 `Set-Cookie` | §外壳页承载与 DSH 鉴权适配 |
| 窗口拖不动（`data-tauri-drag-region` 已挂，无任何报错） | `core:window:default` **不含** `allow-start-dragging`，缺失即静默拒绝 | §标题栏与窗口外壳 |
| 应用命令 IPC 返回 `Command X not allowed by ACL` | 新增/重命名 `#[tauri::command]` 后没同步 `permissions/launcher.toml`；**漏了不会编译报错** | `permissions/launcher.toml` 顶部注释 + AGENTS.md §7.1 命令 5（可执行核查） |

### 核查命令的位置

两条可执行核查命令——**命令 5（Rust 注册的命令是否都已在 ACL 声明）**与
**命令 6（capability 里哪些 `core:window` 权限已在 `core:default` 内）**——
连同「审计要做两个方向」的实例，登记在 `launcher/AGENTS.md` 的 **§7.1 冻结基线**。
本节只做索引，不在事实文档里重复验证流程。
