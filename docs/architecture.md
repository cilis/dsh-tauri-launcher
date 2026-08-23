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

## 状态模型（浏览器侧）

`desktop: true | false | null`（运行中/已停止/状态未知）+ `shortcut: bool`。
切换操作走乐观 UI：点击即切开关位置并显示“正在启动…/正在退出…”，确认后定型；
成功后跳过即时刷新防止心跳窗口内回跳，由 10 秒周期刷新收敛。
