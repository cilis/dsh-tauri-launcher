# 启动器自更新（W11）设计与选型

> 状态：设计稿，未实施。写于 2026-10-18，对应 `main` v1.1.4。  
> 本文回答两个问题：W11 到底怎么升；既然 v1.3 要改安装包，现在还值不值得先做一条过渡链路。  
> §4 的流程与参数取自 `tauri-plugin-updater` v2 源码（`plugins/updater/src/updater.rs`、`config.rs`），非推测。

## 1. 现状

| 事实                                                                                                       | 出处                                             |
| -------------------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| 分发两条：npm 包（`files` 含 `launcher/bin`，内含 `dsh-launcher.exe`）；GitHub/Gitee Release 单个 `dsh-launcher.exe` 资产 | `package.json`、`.github/workflows/release.yml` |
| CI 只跑 `cargo build --release` 并上传裸 exe，**没有** `tauri build`，也没有任何签名步骤                                    | `release.yml`                                  |
| 检测已具备：`check_updates` 并行查 npm `dist-tags`（只认 `latest`），返回当前/最新版本、exe 路径、包与 exe 的漂移附注、可复制的升级命令            | `launcher/src-tauri/src/info.rs`               |
| 用户动作仍是手动：去下载页、替换 exe                                                                                     | 关于弹窗现状（v1.1.x）                                 |
| 运行中的 exe **不可覆盖/删除**，但**可以改名**（2026-10-18 实测，见 §5.1）                                                     | 本机实验                                           |
| v1.1.4 起接入 `tauri-plugin-single-instance`，在 builder **最先**注册                                             | `launcher/src-tauri/src/lib.rs`                |
| `bundle.active=true`、`bundle.targets="all"`，但从未在 CI 里产出安装包                                               | `tauri.conf.json`                              |
| 本机没有 tauri CLI（`cargo tauri` 不存在）；`%LOCALAPPDATA%\\tauri` 已有 NSIS 3.11 与 WixTools314                     | 本机实测                                           |
| 本地离线 cargo 缓存**没有** `tauri-plugin-updater`（首次引入需联网）                                                      | 本机实测                                           |

## 2. 结论

1. **不做「退出时自替换」**。它要私有维护一条替换链路（helper 进程、文件锁、单实例拦截、管道、回滚），而 v1.3 安装包一上线整条链路作废。
2. **推荐路线 C：NSIS 安装包 + 官方 `tauri-plugin-updater`**，把 W11 从 v1.2 移到 v1.3 的第一项。替换、校验、重启都由安装器与插件承担，应用侧只剩入口与文案。
3. 只有当 v1.3 要推迟**一个版本窗口以上**时，才考虑路线 B（下次启动时自替换，无 helper）。B 的成本约等于 C 的应用侧改造，且 C 上线后 B 必须删除。

## 3. 三条路线

### 3.1 A · 退出时自替换（roadmap §11.2 原案，不推荐）

关闭应用 → 由复制到 temp 的自己（`--apply-update`）等父进程退出 → `ReplaceFileW` 换掉目标 exe → 拉起新 exe。

不推荐的原因：helper 是同一个 binary，必须在单实例插件注册**之前**分流；从插件拉起时还可能被作业对象连带回收；写入端要避开 DSH 的管道（v1.0.11 的 panic 教训）。这些复杂度只服务一个会被安装包替代的过渡期。

### 3.2 B · 下次启动时自替换（可选备选）

见 §5。没有 helper、没有退出路径里的文件操作，代价是不能「立即重启」。

### 3.3 C · 安装包 + 官方 updater（推荐）

`tauri build` 产 NSIS 安装包与 updater 产物（`.nsis.zip` + `.sig` + `latest.json`）；应用内 `tauri-plugin-updater` 下载、验签、静默安装、重启。这也是 v1.3 原本要做的「新用户一步安装」。

### 3.4 对比

|         | A 退出时自替换            | B 下次启动自替换          | C 安装包 + updater             |
| ------- | ------------------- | ------------------ | --------------------------- |
| 应用侧新代码  | 多（helper、PID 等待、替换） | 中（下载、校验、落位、启动早期交换） | 少（插件接入、入口、文案）               |
| 发布侧改造   | 无                   | 无                  | 大（`tauri build`、签名密钥、清单、资产） |
| 篡改防护    | 无（仅 npm 完整性）        | 无                  | ✅ 签名清单                      |
| 能否立即重启  | ✅                   | ❌（下次启动生效）          | ✅                           |
| 上线后被谁取代 | 被 C 整体作废            | 被 C 删除应用侧逻辑        | 长期方案                        |
| 附带收益    | 无                   | 无                  | 新用户安装、NSIS 静默部署             |

## 4. 路线 C：更新流程

### 4.0 端到端

**发布侧（每次发版）**

1. `tauri build` → `dsh-launcher_<ver>_x64-setup.exe`（NSIS 安装包）。
2. `bundle.createUpdaterArtifacts: true` → `dsh-launcher_<ver>_x64-setup.nsis.zip` + `.nsis.zip.sig`（minisign 签名）。
3. 生成 `latest.json`（静态清单格式，字段与插件源码一致）：

```json
{
  "version": "1.1.5",
  "notes": "CHANGELOG 首句",
  "pub_date": "2026-10-20T00:00:00Z",
  "platforms": {
    "windows-x86_64-nsis": { "url": "https://…/dsh-launcher_1.1.5_x64-setup.nsis.zip", "signature": "<.sig 内容>" },
    "windows-x86_64":      { "url": "https://…/dsh-launcher_1.1.5_x64-setup.nsis.zip", "signature": "<同上>" }
  }
}
```

**两个键都要给**：插件查找顺序是 `{os}-{arch}-{bundle_type}` → `{os}-{arch}`。安装版命中的是 `windows-x86_64-nsis`；裸 exe（bundle 类型未知）只会命中 `windows-x86_64`——这是迁移期老用户能否自助升级的关键。

1. 上传 Release：`setup.exe`、`.nsis.zip`、`.sig`、`latest.json`。

**运行时（用户点「检查更新」）**

| #  | 发生什么                                                                                                                              | 依据                               |
| -- | --------------------------------------------------------------------------------------------------------------------------------- | -------------------------------- |
| 1  | `GET https://github.com/cilis/dsh-tauri-launcher/releases/latest/download/latest.json`；`releases/latest` 天然排除预发布，与现有「只认 latest」一致 | 插件端点轮询                           |
| 2  | 按 target 取平台条目；版本比较（默认只装严格更高的版本，可用 comparator 覆盖）                                                                                 | `updater.rs`                     |
| 3  | 弹窗显示 当前 → 最新、`notes`、体积；两步确认（沿用 W15 的 4 秒确认按钮）                                                                                    | 本应用 UI                           |
| 4  | 下载 `.nsis.zip`（`Started` / `Progress` 事件驱动进度条）                                                                                    | 插件                               |
| 5  | **验签失败即中止**，不会执行任何安装器                                                                                                             | 插件 minisign                      |
| 6  | 解压出安装器，`ShellExecuteW("open", 安装器, 参数, SW_SHOW)`                                                                                  | `install_inner`                  |
| 7  | 参数 = `/P`（passive，默认，只有进度条）或 `/S`（quiet） + `/UPDATE` + `/R` + `/ARGS <本进程原命令行参数>`                                                 | `config.rs`、`updater_parameters` |
| 8  | 紧接着 `std::process::exit(0)`——**应用在这里直接消失**                                                                                        | `install_inner`                  |
| 9  | NSIS 静默覆盖安装（默认 per-user 装到 `%LOCALAPPDATA%\\<productName>`），按 `/R` 重启应用                                                           | 安装器                              |
| 10 | 新实例启动：单实例互斥量已释放；若 DSH 仍在 3080，走「接管外部实例」路径                                                                                         | 现有逻辑                             |

### 4.1 第 8 步的三个副作用（必须写进设计）

- **不走我们的退出路径**：`process::exit(0)` 跳过 drop、事件循环与 RunEvent::Exit，因此窗口不销毁、托盘不清理、心跳不删除，**也不会对本应用拉起的 DSH 执行 `taskkill`**。进程直接消失，托盘图标由 shell 清掉。
- **DSH 会继续运行**：新实例发现 3080 已有进程 → 接管。WebView2 的 cookie 按 app identifier 存在同一数据目录，安装路径变化不影响它；没有 cookie 时走现有提示条「关闭并重启」。
- **「谁拥有 DSH」会丢**：重启后的实例不是那个 DSH 的父进程，托盘「退出」按现有语义不会停它。三个选项：
  1. 接受并写进文档（与「接管外部实例」的既有行为一致）；
  2. 用 `on_before_exit` 钩子先停 DSH（代价：每次升级都重启 DSH，慢启动场景可能几分钟，W17 实测过 192 秒）；
  3. 启动时把 DSH 的 PID 记进配置，新实例发现 PID 匹配则视为「自己的」（推荐，改动小，语义不变）。

### 4.2 安装形态

- `bundle.windows.nsis.installMode`：`currentUser`（默认，装 `%LOCALAPPDATA%`，更新不需管理员）、`perMachine`（`Program Files`，每次更新触发 UAC）、`both`。**推荐 `currentUser`**，与「静默更新」体验一致。
- 该选项也决定 NSIS 是否要求提权：`passive` 模式下如果需要提权会弹系统 UAC 框，`quiet` 模式直接失败。

### 4.3 清单托管

- GitHub：`releases/latest/download/latest.json` 是稳定地址，发版即生效。
- Gitee：**没有**「latest 资产」的稳定 URL（下载链接带 tag）。要用 Gitee 兜底，只能由 CI 把 `latest.json` 推到固定 raw 路径（如仓库 `update/latest.json`），或本次只支持 GitHub。需要在开工前定。

### 4.4 迁移

- v1.1.4 的裸 exe **不含** updater，所以第一次必须手动装一次安装包。两种做法：
  - M1：发一个过渡裸 exe（v1.1.5，内含 updater，清单同时给两个 target 键），老用户点一次「下载并安装」即迁移；
  - M2：不发过渡版，Release 说明里写明「请下载安装包覆盖一次」。
- 迁移后会有**两份 exe**（旧裸 exe + 安装版）。现有 `drift` 附注继续比对「运行中 exe 版本 vs 已装插件版本」，不需要新机制。
- **配置目录会变**：`.dsh-config.json` 以 exe 同目录为界。开机启动在注册表里不受影响，但快捷键、日志开关等会回到默认。需要一次性迁移（首启检测旧目录并复制配置）或在弹窗里提前告知。
- **桌面快捷方式**：应用自建的那条仍指向旧路径；NSIS 会按配置创建开始菜单/桌面快捷方式。迁移期需要在「关于」里提示用户改用新入口。
- **不删除 npm 包内的 exe**：它是插件「桌面启动」开关的默认拉起路径，退役要单独评估。

### 4.5 与现有「检查更新」的关系

- npm 检测保留：它同时负责 DSH 本体（W15 那条线）与 npm 镜像用户。
- 启动器那一行「有没有新版本」以 **updater 清单**为准；两者不一致时，只有清单确认才显示「下载并安装」。
- 失败路径：清单 404/超时 → 退回现有「去下载页」；验签失败 → 明确提示「安装包校验失败」，不执行；安装器非零退出 → 现有版本继续可用（per-user 安装不会破坏旧文件）。

## 5. 路线 B 的最小形态（若需要过渡）

### 5.1 实测依据：运行中的 exe 可以改名

2026-10-18 本机实验（复制 `ping.exe` 到临时目录并运行，再改名）：

```
RENAME_RUNNING_EXE=OK
WRITE_NEW_AT_ORIGINAL_PATH=OK
OLD_PROCESS_STILL_RUNNING=YES
```

即：不能覆盖、不能删除，但能改名，并且原名可以被新文件占用，旧进程继续运行。这一条让「不需要 helper 的替换」成为可能——替换发生在**下次启动的最早期**，此时进程还没有建窗口。

### 5.2 链路

1. 下载：`npm install @lenorin/dsh-tauri-launcher@<latest> --prefix %TEMP%\\dsh-launcher-update-<ts> --ignore-scripts --no-audit --no-fund`。复用现有 npm 通道（镜像、代理、认证自动沿用），`info.rs` 的「不引入 HTTP 依赖」约束不变。
2. 校验：解出包的 `package.json` 版本 == `latest`；PE `FileVersion` 一致；必要时比对文件大小。
3. 落位：复制到 exe 同目录 `dsh-launcher.exe.new`（同卷、同目录，权限与目标一致）。
4. 生效：下次启动，在建窗口 / 注册单实例**之前**——若 `.new` 存在且版本高于当前，自身改名为 `.old`，把 `.new` 移到原名，继续启动；`.old` 留到再下一次启动删除（删不掉就重试）。
5. 失败：移动失败则把 `.old` 改回原名，`.new` 保留可重试；下载/校验失败不改动磁盘上的任何 exe。

### 5.3 与既有机制的交互

- 单实例：不新增进程、不触碰互斥量，只在启动最早期动文件。
- 标记文件：`.dsh-heartbeat` / `.dsh-quit` 以 exe 同目录为界，替换不改变目录。
- 托盘、AppUserModelID、任务栏图标、全局快捷键：路径与 exe 资源不变，重启后自然重建。

### 5.4 明确不做

不发「退出时替换」；不复制自身做 helper；不写 RunOnce / 计划任务；不做差分更新。

## 6. 风险

| 风险                   | 影响                           | 缓解                               |
| -------------------- | ---------------------------- | -------------------------------- |
| 杀软/EDR 短暂锁文件         | B 的改名或移动失败                   | 保留 `.old` 可回退；下次启动重试             |
| 目录不可写（Program Files） | B 无法落位；C 的 perMachine 需要 UAC | 先试写探测；C 用 currentUser 安装         |
| 断电卡在改名与移动之间          | 目标路径短暂缺失                     | 顺序固定为改名→移动；失败窗口毫秒级，`.old` 可人工改回  |
| 清单或资产被篡改             | C 装到恶意包                      | updater 签名清单；B 无此保护，只能依赖 npm 完整性 |
| npm 镜像滞后             | 检测到新版但装不到                    | 明确报错，不替换，保留现有版本                  |
| SmartScreen          | 新产物首次运行提示                    | 现状相同；有证书再补                       |
| 单实例拦截新实例             | 升级后起不来                       | C 由安装器重启，且旧进程已 exit(0)；B 不做即时重启  |

## 7. 验证清单

- 单测：版本比较、路径推导、可写探测、替换计划（全部纯函数）。
- 失败注入：断网、只读目录、版本不符、`.new` 截断、清单签名错误。
- 真机：v1.1.4 → 下一版升级一次，核对关于弹窗版本、PE 版本、安装目录、快捷方式、托盘只有一个、DSH 是否续跑、托盘退出是否仍能停 DSH。
- CI：`cargo test` + `node --check`；C 追加 `tauri build` 与产物校验（安装包 + zip + sig + latest.json 四件齐全）。

## 8. 待拍板

1. 代码签名证书是否已有？没有也可以先发 C，只是首次安装弹 SmartScreen。
2. 安装形态：`currentUser`（推荐）还是 `perMachine`。
3. 迁移走 M1（先发过渡裸 exe）还是 M2（用户手动装一次）。
4. Gitee 是否托管 `latest.json`（决定是否只支持 GitHub 端点）。
5. DSH 归属：接受、`on_before_exit` 停掉、还是记 PID 认领（推荐第三条）。
6. W11 是否正式从 v1.2 移出，改记为 v1.3 首项（roadmap 需要改一行）。
