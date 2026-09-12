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

## 启动/退出时序

**启动**：残留退出标记为 `1` 时先取消 → `subprocess.spawn` 拉起 exe（stdio 对象
`{stdin:'ignore', stdout:'inherit', stderr:'inherit'}`，graceMs 3000）→ 等心跳
新鲜（20 秒窗口）→ 若快捷方式缺失则创建。

**退出**：写 `.dsh-quit`=`1` → 双信号确认（≤6 秒）→ 清除残留标记 → 删除桌面
快捷方式；确认失败走 `Stop-Process -Name dsh-launcher -Force` 强杀（TerminateProcess，
不影响 Harness 进程）。

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

## 外壳页承载与 DSH 鉴权适配（2026-09-09）

桌面应用主窗口**不再**直接使用 tauri 资产协议（`http://tauri.localhost`），改由内置
静态服务承载于 `http://127.0.0.1:3081`（`src-tauri/src/shell_server.rs`，编译期
`include_str!` 内嵌 `ui/` 三文件）。

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
| 启动就绪等待 | 180 秒 | `harness.rs` | 等 `dsh web` 就绪 |
| 接管重启端口释放等待 | 10 秒 | `harness.rs` | 终止外部实例后等端口 |
| 外壳 ping 周期 | 3 秒 | `ui/main.js` | 导航状态自愈 |
| 服务重试梯子 | 12 × 500ms | `lib/client.js` | theme / sessions 就绪重试 |
| 跳转锁超时 | 1.5 秒 | `lib/client.js` | 会话栈 pendingJump 兜底 |
| 快捷方式存在性缓存 | 5 秒 | `lib/index.js` | 减少 PowerShell 调用 |

## 状态模型（浏览器侧）

`desktop: true | false | null`（运行中/已停止/状态未知）+ `shortcut: bool`。
切换操作走乐观 UI：点击即切开关位置并显示“正在启动…/正在退出…”，确认后定型；
成功后跳过即时刷新防止心跳窗口内回跳，由 10 秒周期刷新收敛。
