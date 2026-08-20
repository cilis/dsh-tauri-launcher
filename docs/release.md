# 安装、构建与发布

## 安装 Web 插件

```bash
# npm（发布后）
dsh plugin --profile <profile> add @lenorin/dsh-tauri-launcher

# GitHub（纯 JS 零构建：git 安装无需 prepare / allowBuilds 授权）
dsh plugin --profile <profile> add github:you/dsh-tauri-launcher

# 本地 checkout / tarball
dsh plugin --profile <profile> add ./dsh-tauri-launcher
dsh plugin --profile <profile> add ./dsh-tauri-launcher-1.0.0.tgz
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
        shortcutName: 'DeepSeek Harness.lnk'
```

## 构建桌面应用

前置：Rust 工具链 + Node 22+。

```powershell
pwsh -File launcher/build.ps1                    # 联网构建
pwsh -File launcher/build.ps1 -Offline -CargoHome D:\path\to\.cargo   # 离线构建
```

产物：`launcher/src-tauri/target/release/dsh-launcher.exe`。

## 发布

### GitHub Release（自动）

推送 `v*` tag 即触发 `.github/workflows/release.yml`：windows-latest 上
`cargo build --release`，产物 `dsh-launcher.exe` 自动附加到 Release。

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
