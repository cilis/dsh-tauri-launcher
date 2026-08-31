# DeepSeek Harness 桌面启动器（Tauri）

一个用于连接本地 DeepSeek Harness 的桌面应用：

- **启动时自动检查**本地是否已安装 DeepSeek Harness（npm 全局包 `@deepseek-ai/dsh`）
- **未安装则自动安装**：加载页面显示安装动画与 npm 实时日志
- **安装完成提示“安装成功”**，随后自动启动并打开 DeepSeek Harness Web 界面
- **安装/启动失败反馈报错信息**，可一键重试
- **系统托盘**提供“打开 DeepSeek Harness”“设置”与“退出”按钮；退出时终止由本应用启动的 DeepSeek Harness 进程（若启动时发现已有实例在运行则接管显示，退出时不终止外部实例）
- 关闭窗口最小化到托盘，应用常驻后台
- **设置窗口**（托盘 → 设置）提供四项开关：
  - **开机启动**：写入/删除 `HKCU\...\Run` 的 `DeepSeekHarness` 注册表值（指向本应用 exe）
  - **创建全局快捷方式**：勾选注册全局快捷键 `Ctrl+Shift+H`（按下即唤起 DeepSeek Harness 主窗口），取消勾选注销快捷键；勾选状态持久化在 exe 同目录的 `.dsh-config.json`，应用启动时自动恢复注册
  - **桌面快捷方式**：勾选在桌面创建 `DeepSeek Harness.lnk`（指向本应用 exe，带图标），取消勾选删除；状态以 .lnk 是否存在为准，无需持久化
  - **退出时结束 DeepSeek Harness 进程**：勾选后退出桌面应用时一并结束 DeepSeek Harness 进程；默认不结束，Harness 继续在后台运行

## 开发

```bash
pnpm install        # 仅安装 tauri CLI
pnpm tauri dev      # 直接以静态 ui/ 目录运行
```

## 构建

```bash
pnpm tauri build
```

前端为 `ui/` 下的纯静态 HTML/CSS/JS（无构建步骤，经 `window.__TAURI__` 全局 API 与后端通信）。

## 环境变量（可选，便于测试）

- `DSH_LAUNCHER_NODE`：覆盖 node.exe 路径
- `DSH_LAUNCHER_NPM_ROOT`：覆盖 npm 全局安装根目录（安装检测与 `dsh` 入口脚本均以此为准）

## 与 Web 设置插件的标记文件协作

DeepSeek Harness Web 设置中的“桌面启动”插件（`@lenorin/dsh-tauri-launcher`）只负责启动/退出本应用；本应用 exe 同目录下的标记文件用于协作（每秒检查一次）：

- `.dsh-heartbeat`：每秒写入当前 Unix 时间戳，供插件在沙箱内判断本应用是否仍在运行（插件侧的新鲜窗口为 4 秒，须与 1 秒写入周期匹配）；
- `.dsh-quit`：内容为 `1` 且修改时间在 60 秒内 → 本应用**仅退出自身**（消费时自删标记文件），保留由其启动的 DeepSeek Harness 进程继续运行（与托盘“退出”的整树清理语义不同）；插件重写内容为 `0` 即取消退出请求。

## 开机启动、全局快捷方式与桌面快捷方式

三项均由本应用自己的设置窗口（托盘 → 设置）控制，与 Web 设置插件无联动：

- **开机启动**：直接写入/删除 `HKCU\...\Run` 的 `DeepSeekHarness` 注册表值（指向本应用 exe）；
- **全局快捷方式**：通过 `tauri-plugin-global-shortcut` 注册/注销 `Ctrl+Shift+H`，按下即显示并聚焦主窗口；勾选状态保存在 exe 同目录的 `.dsh-config.json`，启动时自动恢复；
- **桌面快捷方式**：通过 PowerShell WScript.Shell COM 创建/删除桌面 `DeepSeek Harness.lnk`（桌面路径用 `[Environment]::GetFolderPath('Desktop')` 解析，自动适配 OneDrive 重定向）；状态即 .lnk 存在性。

## 工作原理

1. `check_dsh`：通过 `npm root -g` 定位全局安装目录，检查 `node_modules/@deepseek-ai/dsh/package.json`
2. `install_dsh`：执行 `npm install -g @deepseek-ai/dsh`，逐行转发输出到前端
3. `launch_dsh`：先探测 `127.0.0.1:3080` 是否已有 DSH Web 实例（以页面 `__DSH_BOOT__` 标记为指纹），没有则拉起 `dsh web` 并轮询等待就绪，然后把主窗口导航到本地 Web 界面
4. 托盘“退出”或进程退出时，对由本应用启动的 dsh 进程树执行 `taskkill /T /F` 清理
