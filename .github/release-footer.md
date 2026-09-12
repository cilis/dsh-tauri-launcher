---

### 下载与安装

- **桌面应用（Windows）**：下载下方附件 `dsh-launcher.exe`，替换 `launcher/bin/dsh-launcher.exe` 即可。
- **Web 插件**：`dsh plugin --profile <profile> add @lenorin/dsh-tauri-launcher@{VERSION}`
  （升级前请**完全退出 DSH**：安装走 pnpm，会先删除 `node_modules` 再安装，文件被占用会中途失败）。
- 需要 Node 22+ 与全局 `@deepseek-ai/dsh`；桌面应用启动时会自行检测并提示安装。
- **完整变更**：[{PREV}...{TAG}](https://github.com/cilis/dsh-tauri-launcher/compare/{PREV}...{TAG})

<!-- en -->

### Download & Install

- **Desktop app (Windows)**: download `dsh-launcher.exe` below and replace the copy at
  `launcher/bin/dsh-launcher.exe`.
- **Web plugin**: `dsh plugin --profile <profile> add @lenorin/dsh-tauri-launcher@{VERSION}`
  (quit DSH completely before upgrading — the install runs pnpm, which removes `node_modules`
  first and fails if files are in use).
- Requires Node 22+ and a global `@deepseek-ai/dsh`; the desktop app detects and offers to
  install it.
- **Full changelog**: [{PREV}...{TAG}](https://github.com/cilis/dsh-tauri-launcher/compare/{PREV}...{TAG})
