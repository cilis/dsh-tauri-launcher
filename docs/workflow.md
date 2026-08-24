# 分支验证工作流（Branch Workflow）

> 目标：`main` 始终保持可发布状态。新特性/修复先在分支上验证（CI 自动测试 +
> 构建 + 人工冒烟），确认后再经 PR 合入 `main`；发版 tag 只从 `main` 上打。
> 配套 CI：[`.github/workflows/ci.yml`](../.github/workflows/ci.yml)（push 到任何分支
> 或目标为 `main` 的 PR 即触发 `verify` 任务）。

## 分支模型

- **`main`** — 稳定分支，始终可发布。**只通过 PR 合并进入**（分支保护已开启，
  含管理员）；`v*` tag 只从这里打。
- **工作分支** — 短生命周期，按主题命名，用完即删：
  - `feat/<主题>` — 新功能（对应 roadmap 条目）
  - `fix/<主题>` — 缺陷修复 / 安全修复
  - `docs/<主题>` — 纯文档改动
  - `ci/<主题>` — CI / 工作流改动
  - `chore/<主题>` — 工程杂项（.gitignore、构建脚本等）
- 不设长期 `dev` 分支（单人开发无必要，短分支直汇 main 即可）。

## 标准流程（每项改动）

1. **建分支**：`git checkout -b feat/<主题>`（从最新 `main` 切出）。
2. **分类型提交**：沿用既有提交粒度规则——功能、修复、版本同步、文档、CI
   分别成独立提交；一个 PR 内可以包含多个不同类型的提交，但每个提交只含
   同一类型的改动。
3. **推送分支**：`git push -u origin feat/<主题>`。push 即触发 CI
   （无需先建 PR）。
4. **开 PR**：push 输出会给出创建链接（形如
   `https://github.com/cilis/dsh-tauri-launcher/pull/new/feat/<主题>`），
   网页点开即可（本仓库未安装 gh CLI）。
5. **等 CI 绿灯**：`verify` 任务跑 `cargo test` + `cargo build --release`。
   失败就在分支上追加修复提交，PR 自动重跑；**红着绝不合并**。
6. **人工冒烟验证**（详见下节清单），需要 exe 时从 PR 页面底部
   **Artifacts** 下载 `dsh-launcher-win-x64`。
7. **合并**：用 **Rebase and merge**（保持线性历史、逐条保留类型化提交）。
8. **删分支**：合并后删除远端与本地分支。

## 人工验证清单（合并不前必须逐项过）

- [ ] CI `verify` 绿灯；
- [ ] 涉及 Rust 逻辑的改动：下载 PR Artifacts 里的 `dsh-launcher.exe`
      替换本地 `launcher/bin/dsh-launcher.exe` 做冒烟（或本地离线重建）；
- [ ] 涉及 GUI 行为的改动：**以用户肉眼确认为准**（自动化窗口枚举在本机
      沙箱内不可靠，勿仅凭探测下结论）；
- [ ] 涉及 Web 插件（`lib/`）的改动：重新打包/安装进 profile
      （`~/.dsh/profiles/web/node_modules/@lenorin/dsh-tauri-launcher`）后
      在 Web 设置面板验证；
- [ ] 双仓库同步项：`dsh-launcher`（独立项目）与本仓库 `launcher/` 共享的
      核心逻辑（如 `--no-open` 参数）两边都要改、都验证。

## 版本同步与发版策略

- **发版前置**：特性经 PR 合入 `main` 后，若要发版，先在 `main` 上补一个
  **版本同步提交**（三处版本号统一：`package.json` / `Cargo.toml` /
  `tauri.conf.json`，并重建同步 `launcher/bin/dsh-launcher.exe`——可直接
  用 CI main 分支构建的产物），该提交同样走 PR。
- **打 tag**：`git tag vX.Y.Z && git push origin vX.Y.Z`，触发
  [`release.yml`](../.github/workflows/release.yml)：cargo test → 构建 →
  GitHub Release → 自动同步 Gitee 发行版（GITEE_TOKEN）。
- **Gitee 镜像**：分支/tag 在 Gitee 手动「强制同步」后出现；发行版由
  release.yml 自动创建，无需手动传附件。

## 分支保护（main，已配置/待配置）

Settings → Branches → `main` 规则：

- ✅ Require a pull request before merging（不勾 Require approvals，单人自审自并）
- ✅ Require status checks to pass → `verify`
- ✅ Require branches to be up to date before merging
- ✅ Do not allow bypassing the above settings（含管理员，紧急修复也走 PR）

tag 无有效保护手段，依靠约定「只从 main 打 tag」+ release.yml 自带的
cargo test 作为最后防线。

## 常见情况

- **直推 `main` 被拒**：分支保护生效后的预期行为，改走 PR。
- **分支落后 main**：`git fetch && git rebase origin/main` 后重推
  （保护要求 up to date）。
- **紧急修复**：`fix/<主题>` 同样走 PR，流程不变（保护不豁免管理员）。
