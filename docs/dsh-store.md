# DSH STORE 上架

记录 `cilis/dsh-tauri-launcher` 在 [DSH STORE](https://dsh.store/) 的上架状态、契约要求与验收方法。
事实来源为 DSH STORE 仓库的公开契约文件、通知 issue 原文与本机实测，逐条标注日期；未经实测的推断
单独写明为待确认。

## 当前状态

- 通知 issue：[DSH-Store#1292](https://github.com/AI-Scarlett/DSH-Store/issues/1292)，2026-10-03 由
  `github-actions[bot]` 自动创建，状态 open、零评论，标签 `author-action-required` 与 `catalog-blocked`。
  该通知按人全局去重，同一作者跨项目只主动联系一次，推修复后不会重发。
- 条目状态：`blocked`。商城不可安装，GitHub 手动安装入口保留。
- 已完成的修改：`9a46eeb`（`chore: manifest 补 DSH 与 Node 兼容性声明`）已合入 `main`；窗口内三个 DSH
  版本的四项验收于 2026-10-09 完成，结果写入 `package.json` 与本文「验收结果」一节。

扫描按 UTC 00:05 / 08:05 / 16:05 每八小时复检默认分支的固定 Commit，推送到 `main` 即被读取，
不需要发版或打 tag。

## 上架契约

### 硬性要求

新增或更新条目必须同时满足（`registry/README.md`）：

1. `repositoryUrl` 是公开的 `https://github.com/<owner>/<repo>`；
2. `manifestPath` 指向的 `package.json` 声明 `dsh.bundle.patch`，单仓库多包可另声明 `installPath`；
3. `commit` 是 40 位不可变 Git Commit；
4. `version` 与该 Commit 的 manifest 一致；
5. `entryIds` 与 Bundle Patch 插入的 DSH ID 一致；
6. 明确列出 `preinstall/install/postinstall/prepare` 生命周期脚本；
7. 不禁用、替换或重复安装任何 `@deepseek-ai/*` 官方组件；
8. `npm run validate:registry` 通过。

目录只接受 GitHub 仓库，不接受 npm-only、任意下载 URL、本地路径或浮动安装目标。

### 两条通道

| 通道 | 准入条件 | 结果 |
| --- | --- | --- |
| `source-verified` | canonical GitHub、完整 Commit、manifest/repository/license 一致、明确文件清单、**无生命周期脚本和运行依赖**、Bundle 与入口唯一，且完整有界运行时源码不含文件、网络、命令、凭据、原生制品或受保护 DSH 行为信号 | 自动生成 Catalog PR 并自动合并 |
| `user-reviewed` | 具备文件、网络、命令、凭据或生命周期能力的合法插件，需完整披露依赖、权限、外部服务与失败边界 | 商城展示实际变化，用户逐次确认安装 |

本插件只能走 `user-reviewed`：回环路由对应网络信号，拉起 exe 对应命令与原生制品信号，读写
`.dsh-heartbeat` / `.dsh-quit` 对应文件信号。四条信号无法通过声明消除。

高权限不构成上架障碍，有两份详情文件实证：`build-dsh-plugin` 与 `dsh-browser-desktop` 的
`updatePolicy` 均为 `user-reviewed`，前者 `status: approved`；后者在 Web UI 内嵌 Chromium、
`permissions.level: medium`，现为 `unlisted`，原因是窗口内无 compatible 记录，与权限无关。
契约另有明示——高权限项目可能仍需保持 `user-reviewed`/`blocked` 状态，声明本身不保证自动批准。

### manifest 字段

各字段的位置与来源：

| 字段 | 说明 |
| --- | --- |
| `dsh.bundle.patch` | 已有，指向 `./cordis.patch.yml` |
| `engines.node` | DSH STORE 详情文件的 `compatibility.node` 逐字取自该字段（据 `build-dsh-plugin` 的 manifest 与详情文件比对：两处均为 `>=22.13.0`） |
| `dsh.compatibility.dsh` | 兼容范围串，展示口径，不承担证据职责 |
| `dsh.compatibility.dshReleases` | 逐版本精确矩阵，值只接受 `compatible`、`incompatible`、`unknown`；未声明的版本按 `unknown` 处理 |
| `dsh.compatibility.dshOperations` | 逐版本 `install`/`start`/`uninstall`/`rollback` 四项证据，缺少记录时保持 `unknown` |

DSH 运行时不读取 `dsh.compatibility`。已验证的读取面只有 `dsh.bundle.patch`、`dsh.profile.bundles`、
`dsh.profile.patchReload`、`dsh.moduleFallback.targets` 与 `dsh.client`（在 DSH 0.1.5-rc.1 的完整依赖树中
grep `dshReleases` 零命中）。

当前已写入 `package.json` 的值：

```json
"compatibility": {
  "dsh": "0.1.5-rc.1 || 0.2.0-rc.1 || 0.2.0-rc.2 || 0.2.1-alpha.1",
  "dshReleases": {
    "0.1.5-rc.1": "compatible",
    "0.2.0-rc.1": "compatible",
    "0.2.0-rc.2": "compatible",
    "0.2.1-alpha.1": "compatible"
  },
  "dshOperations": {
    "0.2.0-rc.1": { "install": "passed", "start": "passed", "uninstall": "passed", "rollback": "passed" },
    "0.2.0-rc.2": { "install": "passed", "start": "passed", "uninstall": "passed", "rollback": "passed" },
    "0.2.1-alpha.1": { "install": "passed", "start": "passed", "uninstall": "passed", "rollback": "passed" }
  }
}
```

`dsh` 范围串逐个列精确版本而不写区间：semver 对预发布版本的区间匹配有坑——`>=0.1.5-rc.1 <0.2.0`
只对 `0.1.5` 这个元组放行预发布，实际不匹配 `0.1.7-rc.x`。官方样板同样逐个 OR。

`0.1.5-rc.1` 的依据是本机 web profile 实际运行 `@lenorin/dsh-tauri-launcher@1.1.2`；窗口内三个版本
的依据是下文「验收结果」的四项实测。`0.1.5-rc.1` 未做该四项验收，故不进 `dshOperations`，缺省保持
`unknown`。

### 版本窗口

自动上下架使用官方 npm `latest` 标签及其之前最近两个未弃用发行版组成的滚动窗口：

- `approved` 条目在窗口三个版本中至少需要一个精确 `dshReleases: compatible` 记录；
- 三者均为 `incompatible`、`unknown` 或缺失时自动转为 `unlisted`；
- 范围匹配但没有精确记录时显示「范围支持·待验证」，不自动标为兼容。

2026-10-08 时的窗口为 `0.2.0-rc.1`、`0.2.0-rc.2`、`0.2.1-alpha.1`：`dsh-browser-desktop` 详情文件的
`statusReason` 直接列出这三个版本，npm dist-tags 实查（`latest` 与 `next` 均为 `0.2.0-rc.2`、
`alpha` 为 `0.2.1-alpha.1`）与此相容。本机全局安装的是 0.1.5-rc.1，不在窗口内。

## 阻塞原因

2026-10-03 通知列出七条确定性原因，现状如下：

| # | 原因（原文） | 现状 |
| --- | --- | --- |
| 1 | DSH compatibility is not explicitly declared | 已消除（`9a46eeb`） |
| 2 | Node.js compatibility is not explicitly declared | 已消除（`9a46eeb`） |
| 3 | package contains unsupported artifacts requiring review: `launcher/bin/dsh-launcher.exe` | 结构性 |
| 4 | fixed-source scan is incomplete | 结构性，由 exe 单文件顶出 |
| 5 | unobserved capability signals are unknown | 待复检 |
| 6 | runtime source contains the network permission signal | 结构性 |
| 7 | runtime source contains the nativeOrExecutableArtifacts permission signal | 结构性 |

第 3、4、6、7 条无法通过 manifest 声明消除，能否从 `blocked` 转入 `user-reviewed` 取决于 DSH STORE 的
判定，契约未给出自动路径。第 1、2 条是否已在复检中消除，需等下一次八小时扫描的 Catalog 结果确认。

窗口三个版本现已各有一条精确 `compatible` 记录，满足契约对 `approved` 条目的兼容性要求；能否上架
仍受前述结构性原因限制。

## 扫描面数据

本机实测（2026-10-08），排除 `.git`、`target`、`gen`、`node_modules`：

- 受版本控制的文件 75 个，合计 14.37 MB；
- `launcher/bin/dsh-launcher.exe` 单文件 13,090,816 字节（约 12.5 MiB），占总体积 91%；
- 排除该文件后剩 74 个文件、1.28 MB，最大文本文件为 `launcher/src-tauri/Cargo.lock`（120,194 字节）。

对照契约的扫描上限：预检最多读取 150 个文本源码文件、单文件 400,000 字节；自动低风险源码面另有
240 个文件、单文件 256 KiB、合计 2 MiB 的上限，超限时报告「扫描面不完整」并失败关闭。

据此，「fixed-source scan is incomplete」由 exe 单文件触发。`.gitignore` 已排除 `launcher/src-tauri/target/`
与 `launcher/src-tauri/gen/`，源码面本身干净。若要让扫描面完整，需要把 exe 移出 Git 仓库并改为按需
获取，代价是失去「装完即用」，属于产品决策。

## 验收结果

2026-10-09 完成。三个窗口版本各建独立临时 `DSH_HOME`，用该版本自身的 CLI 走完 install、start、
uninstall、rollback 四步：

| 版本 | install | start | uninstall | rollback |
| --- | --- | --- | --- | --- |
| `0.2.0-rc.1` | dump 1262 行，含 `desktop-launcher` | 3s 起，`/api/dsh-tauri-launcher/state` 返回 200 `ok:true` | dump 回到 1259 行，条目消失 | 卸载后正常启动，同路由返回 404 |
| `0.2.0-rc.2` | 同上 | 同上 | 同上 | 同上 |
| `0.2.1-alpha.1` | dump 1315 行，含 `desktop-launcher` | 3s 起，返回 200 `ok:true` | dump 回到 1312 行，条目消失 | 卸载后正常启动，返回 404 |

`start` 的判据是插件自己的回环路由返回 200 与 `ok:true`，而不是进程存活——后者只能证明 DSH 起来了，
证明不了插件激活。三个版本的探测响应完全一致，其中 `exe` 字段指向临时包副本而非当前环境的 exe，
隔离边界再次得到印证。

### 验证边界

下列内容**未被这套验收覆盖**，不应据此推断为可用：

- 浏览器半（`lib/client.js` 的设置面板 UI）：全程未开浏览器；
- `/api/dsh-tauri-launcher/set-desktop` 与 `/set-shortcut`：故意未调用。这两个动作会真的拉起或停止
  桌面应用，而 exe 目录探测链（运行中进程、桌面快捷方式目标）是机器级的，在临时环境里触发可能
  反噬当前环境正在使用的桌面应用。

因此本次证据覆盖「可安装、组合正确、插件激活、可干净卸载与回滚」，不覆盖「功能完整可用」。

### 复现方法

每个版本：取到该版本的 dsh → 建独立临时 `DSH_HOME` → 从 web 模板初始化 profile →
`dsh plugin --profile <名字> add <包>` → 起实例 → 探测插件路由 → 卸载 → 再起一次验证回滚。

三个必须注意的点：

1. **profile 必须从 web 模板初始化**（`dsh <名字> --from-default-profile web --dump-config`）。
   `dsh plugin --profile X add` 直接建出的 profile 只含 `@deepseek-ai/dsh-base`，缺
   `@deepseek-ai/dsh-web-app`，插件会停在 `pending (waiting for service: webServer)` 不激活。
2. 本地安装路径不能含空格，见工作区根 `AGENTS.md` 的 `dsh plugin add` 条目。
3. 新版 `dsh web` 有 token 鉴权：先访问 stdout 打印的 `?token=` URL 拿到 cookie，才能请求 `/api/*`。
   启动形式是 `dsh --profile <名字> --port <非 3080> --no-open`，不是 `dsh web ...`。临时 home 的
   隔离边界与两个必避点见根 `AGENTS.md`。

## 出处

- 上架契约：<https://github.com/AI-Scarlett/DSH-Store/blob/main/registry/README.md>
- 通知 issue：<https://github.com/AI-Scarlett/DSH-Store/issues/1292>
- `user-reviewed` 实证一：<https://github.com/AI-Scarlett/DSH-Store/blob/main/registry/catalog/details/build-dsh-plugin.json>
- `user-reviewed` 实证二：<https://github.com/AI-Scarlett/DSH-Store/blob/main/registry/catalog/details/dsh-browser-desktop.json>
- 整改建议中指向的检查工具：<https://github.com/AI-Scarlett/build-dsh-plugin>
