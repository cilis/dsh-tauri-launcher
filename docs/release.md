# 安装、构建与发布

## 安装 Web 插件

```bash
# npm（发布后）
dsh plugin --profile <profile> add @lenorin/dsh-tauri-launcher

# GitHub（纯 JS 零构建：git 安装无需 prepare / allowBuilds 授权）
dsh plugin --profile <profile> add github:cilis/dsh-tauri-launcher

# 本地 checkout / tarball
dsh plugin --profile <profile> add ./dsh-tauri-launcher
dsh plugin --profile <profile> add ./dsh-tauri-launcher-1.0.3.tgz
```

验证组合层（应看到 `# == dsh-tauri-launcher` 层）：

```bash
dsh --profile <profile> --dump-config
```

重启 DSH Web 进程后生效（组合与客户端 bundle 均在启动时加载）。

卸载：

```bash
dsh plugin --profile <profile> remove @lenorin/dsh-tauri-launcher
```

## 行配置参考

```yaml
- insert:
    - id: desktop-launcher
      name: '@lenorin/dsh-tauri-launcher'
      config:
        launcherExe: ''          # 绝对路径；空 = 自动探测
        launcherDirs: []         # 候选目录；空 = 内置默认候选
        freshSecs: 4             # 心跳新鲜窗口（秒），须大于 1 秒写入周期
```

> 注：原 `shortcutName` 配置项已移除（2026-09）——快捷方式文件名与桌面应用
> `src-tauri/src/settings.rs` 的 `SHORTCUT_NAME` 共用同一常量。

## 构建桌面应用

前置：Rust 工具链 + Node 22+。

```powershell
pwsh -File launcher/build.ps1                    # 联网构建
pwsh -File launcher/build.ps1 -Offline -CargoHome D:\path\to\.cargo   # 离线构建
```

产物：`launcher/src-tauri/target/release/dsh-launcher.exe`。

## 发布

### 发版说明（唯一来源：`CHANGELOG.md`）

发版说明不再由 GitHub 按 PR 标题生成（那种列表会截断长标题、混入 `chore: 版本同步`，
重跑还会追加重复段落），而是**以仓库根 `CHANGELOG.md` 的对应小节为唯一来源**：

| 位置 | 正文 |
| --- | --- |
| GitHub Release | 中文段 + `---` + 英文段 + 下载安装（中英） |
| Gitee 发行版 | 仅中文段 + 中文下载安装 |

写法（每版一节）：

```markdown
## v1.0.9 — 2026-09-13

一句话概述（用户视角：现在能做什么 / 不再出什么问题）。

**修复**

- **现象或功能**：原因/影响 → 解决方式（[#30](https://github.com/cilis/dsh-tauri-launcher/pull/30)）。

<!-- en -->

### English

One-line summary.

**Fixed**

- **Symptom**: cause → fix ([#30](…)).
```

- 标题行固定 `## vX.Y.Z`（可带 ` — YYYY-MM-DD`；未发布写 `未发布`）。**标题行不会进正文**
  （发布页已显示版本与日期），所以 `未发布` 不会被发出去。
- 中文在前、英文在后，中间用**独占一行**的 `<!-- en -->` 分隔——Gitee 正文从这里截断，
  渲染时该行不可见。
- 分组固定为 **新增 / 修复 / 变更 / 内部改进**，无内容的组省略；条目写「现象 → 原因 →
  解决」，附 PR 链接；版本同步、CI 细节、文档微调不要当条目（必要时并进「内部改进」一行）。
- 需要改说明时只改 `CHANGELOG.md`，然后重跑同步（见下），**不必重新打 tag**。

本地预览 / 自检：

```powershell
& ./.github/scripts/release-notes.ps1 -Tag v1.0.9 -Mode github -DryRun   # GitHub 正文
& ./.github/scripts/release-notes.ps1 -Tag v1.0.9 -Mode gitee  -DryRun   # Gitee 正文（仅中文）
& ./.github/scripts/release-notes.ps1 -ListVersions                      # 列出全部版本
```

生成逻辑在 `.github/scripts/release-notes.ps1`，固定尾巴（下载/安装/完整变更链接，
`{VERSION}` / `{TAG}` / `{PREV}` 占位）在 `.github/release-footer.md`。
**该脚本必须保持 UTF-8 BOM**（Windows PowerShell 5.1 会把无 BOM 的 UTF-8 当 ANSI 解析，
中文注释会破坏语法；编辑器写回后需补 BOM，同 `launcher/build.ps1`）。

两道校验：`ci.yml` 会检查 `package.json` 的版本在 `CHANGELOG.md` 里有合格小节（PR 阶段
就拦住「改了版本号没写说明」）；`release.yml` 在构建前生成正文，缺小节**几秒内失败**，
不会发出没有说明的发行版。

回填与修复：`.github/workflows/release-notes.yml` 负责把 CHANGELOG 写进两边发行版的正文。
改动 `CHANGELOG.md`（或尾部模板、生成脚本）推到 `main` 会自动同步；也可以在 Actions →
release-notes → Run workflow 手动触发，填单个 tag（如 `v1.0.8`）或留空同步全部。
它**只更新已存在的发行版，绝不创建**（手动触发时分支名会被当成 tag，创建会造出错误的发行版）。

发版顺序：先 `docs:` 提交补 `CHANGELOG.md` 小节 → 再 `chore:` 版本同步提交 →
`git tag vX.Y.Z && git push origin vX.Y.Z`。

### GitHub Release（自动）

推送 `v*` tag 即触发 `.github/workflows/release.yml`：windows-latest 上
`cargo build --release`，产物 `dsh-launcher.exe` 自动附加到 Release，正文取
`CHANGELOG.md` 对应小节（`body_path`，覆盖式写入）。

### npm（可选）

```bash
npm publish           # 构建产物已在 lib/，files 字段已限定发布内容
```

## 手工安装（无 dsh CLI 时）

1. 把包目录放到 profile 的 `node_modules\dsh-tauri-launcher\`；
2. 在 profile 的 `package.json` 的 `dsh.profile.bundles` 数组追加
   `"dsh-tauri-launcher"`；
3. 重启 DSH Web。

## 常见问题

- **「无法探测桌面应用进程状态」**：桌面应用未运行或 exe 不在候选目录；用
  `config.launcherExe` 指定路径，或看设置区的诊断信息（`exeDirs` / `pickExe`）。
- **关闭后应用仍在运行**：确认桌面应用版本支持退出标记（本仓库 launcher
  ≥ 1 秒轮询版）；插件会在 20 秒后自动强杀兜底。
- **导航图标变回齿轮**：外壳升级改变了设置分区顺序（图标替换依赖分区位置），
  无功能影响，重新校准 CSS 选择器即可。
