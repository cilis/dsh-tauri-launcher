//! DeepSeek Harness 子进程托管：全局状态 [`AppState`]、检测/安装/启动/停止
//! 命令、日志环形缓冲与日志泵、孤儿化/强杀等子进程生命周期操作。
//! 具体的检测/安装/端口探测逻辑在 [`crate::dsh`]（与 tauri 解耦、可独立测试）。
//!
//! 另含「PID 认领」：本应用拉起的 dsh PID 会被记进 exe 同目录配置，下次启动
//! 遇到端口已被占用时用它分辨「端口上跑的是上一次自己拉起的」还是「别人的实例」。

use std::collections::VecDeque;
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::dsh;

/// 事件名：npm 安装日志逐行广播（外壳安装页实时显示）。
pub(crate) const EVENT_INSTALL_OUTPUT: &str = "install-output";

/// 事件名：启动等待的阶段进度（外壳启动卡片实时显示阶段与已等待秒数）。
pub(crate) const EVENT_LAUNCH_PROGRESS: &str = "launch-progress";

/// 启动等待的**软超时**（耐心线）：到点不再当作异常，只把进度标记为「较慢」
/// 并让外壳提示「仍在继续」。
///
/// 为什么需要：DSH 0.2.0 起 `dsh web` 的 token URL 行改在**整棵插件树 settle
/// 之后**才打印（0.1.5 时代启动后即打印），插件多的 profile 实测要 190 秒以上
/// （2026-10-12 实测：dsh-purge 单独就把启动从 3 秒拉到 202 秒）。旧实现在 180 秒
/// 直接终止进程，于是「等三分钟 → 失败 → 重试再等三分钟」永远成功不了。
const LAUNCH_SOFT_TIMEOUT_SECS: u64 = 180;

/// 启动等待的**硬超时**：真正的失败上限，超过才终止进程并报错。
/// 取 10 分钟——比实测最慢案例（约 200 秒）留足余量，又不至于挂死。
const LAUNCH_HARD_TIMEOUT_SECS: u64 = 600;

/// 进度事件节流：等待循环每 500ms 一轮，每两轮广播一次（即每秒一次）。
/// 事件载荷很小（阶段 + 秒数），每秒一次的 IPC 开销可忽略。
const PROGRESS_EMIT_EVERY: u32 = 2;

/// 托管状态：本应用与 dsh 子进程的关系（单一锁下的三态机）。
///
/// 取代原先分散在 `child` / `pid` / `owned` 三个字段、三种同步原语里的隐式
/// 约定（见优化报告 v2 B1：调用方必须记住“先 child 后 pid、最后 owned”的
/// 顺序，且要按上下文选择 try_lock 或阻塞锁）。现在状态迁移整体发生，锁也
/// 统一为 std Mutex——所有操作都是同步的（子进程探测 `try_wait` 本身同步；
/// 终止动作先取出句柄再执行，不跨 await 持锁），因此不再有阻塞锁 vs
/// try_lock 的区分。
#[derive(Default)]
pub enum HarnessProcess {
    /// 未托管任何实例（尚未启动，或已终止/已放弃）。
    #[default]
    None,
    /// 接管的外部实例：本应用不拥有它，退出时不终止。
    Adopted,
    /// 由本应用启动并托管：持有子进程句柄与 PID（退出时可终止整棵进程树）。
    Owned {
        child: tokio::process::Child,
        /// 子进程 PID；极少数情况下取不到（进程已被回收）时为 None，
        /// 此时只能按句柄终止，无法 taskkill 整棵进程树。
        pid: Option<u32>,
    },
    /// 认领的实例：端口上跑的是**上一次本应用拉起的** dsh（配置里的 PID 仍存活、
    /// 映像路径与当前 node.exe 一致）。本应用拿不到它的句柄，只有 PID，
    /// 但退出时应当终止它——这正是与「别人的实例」的区别。
    Claimed {
        pid: u32,
    },
}

/// 从托管状态中取出的自有子进程，提供「终止」与「放弃」两种归宿。
/// 取出即状态置回 [`HarnessProcess::None`]，后续操作与状态锁无关。
pub(crate) struct OwnedChild {
    /// 本应用拉起的子进程句柄；PID 认领来的实例没有句柄（只有 PID）。
    child: Option<tokio::process::Child>,
    pid: Option<u32>,
}

impl OwnedChild {
    /// 强制终止整棵进程树（Windows `taskkill /T /F`，其他平台 `kill -9`）。
    /// 同步实现：退出路径可能位于非异步上下文（托盘事件、`RunEvent::Exit`），
    /// taskkill 通常几十毫秒，代价可接受；句柄另做 `start_kill` 兜底。
    /// 失败留痕：原实现静默丢弃 taskkill 结果，无法区分「进程树没杀干净」与
    /// 「本就没有可杀进程」（优化报告 v2 C6）。
    pub(crate) fn terminate(self) {
        if let Some(pid) = self.pid {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let res = StdCommand::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x0800_0000)
                    .status();
                match res {
                    Ok(status) if status.success() => {}
                    Ok(status) => log::warn!("taskkill 退出码 {:?}", status.code()),
                    Err(e) => log::warn!("无法执行 taskkill：{e}"),
                }
            }
            #[cfg(not(windows))]
            {
                match StdCommand::new("kill").args(["-9", &pid.to_string()]).status() {
                    Ok(status) if status.success() => {}
                    Ok(status) => log::warn!("kill 退出码 {:?}", status.code()),
                    Err(e) => log::warn!("无法执行 kill：{e}"),
                }
            }
        }
        // 兜底：taskkill 后进程通常已退出，kill_on_drop 的二次 kill 无害。
        if let Some(mut child) = self.child {
            let _ = child.start_kill();
        }
    }

    /// 放弃托管：进程作为孤儿继续运行，句柄交由系统回收。
    /// `forget` 阻止 drop 触发 kill_on_drop（认领来的实例没有句柄，无需处理）。
    pub(crate) fn orphan(self) {
        if let Some(child) = self.child {
            std::mem::forget(child);
        }
    }
}

impl HarnessProcess {
    /// 是否由本应用**本次运行**启动并托管。
    ///
    /// 认领来的实例（[`HarnessProcess::Claimed`]）不算：接管路径的其它行为
    /// （外壳的 401 引导、`restart_dsh_external` 的「关闭并重启」）必须保持现状，
    /// 认领只影响退出时的终止。它靠 [`HarnessProcess::take_owned`] 被终止。
    pub(crate) fn is_owned(&self) -> bool {
        matches!(self, HarnessProcess::Owned { .. })
    }

    /// 记录“接管了外部实例”（本应用不拥有它）。
    /// 若此前托管过自有进程，先放弃之（不终止）——与历史行为一致：
    /// 接管路径不会终止此前拉起的实例。
    pub(crate) fn adopt(&mut self) {
        if let Some(previous) = self.take_owned() {
            previous.orphan();
        }
        *self = HarnessProcess::Adopted;
    }

    /// 记录「端口上的实例是上一次本应用拉起的」（PID 认领成功）。
    /// 与 [`HarnessProcess::adopt`] 一致：先放弃此前托管的自有进程（不终止）。
    pub(crate) fn claim(&mut self, pid: u32) {
        if let Some(previous) = self.take_owned() {
            previous.orphan();
        }
        *self = HarnessProcess::Claimed { pid };
    }

    /// 记录“由本应用启动”的托管进程（同时取代旧状态）。
    pub(crate) fn own(&mut self, child: tokio::process::Child, pid: Option<u32>) {
        if let Some(previous) = self.take_owned() {
            previous.orphan();
        }
        *self = HarnessProcess::Owned { child, pid };
    }

    /// 取出自有子进程并置回未托管；无自有进程时返回 None。
    pub(crate) fn take_owned(&mut self) -> Option<OwnedChild> {
        match std::mem::take(self) {
            HarnessProcess::Owned { child, pid } => Some(OwnedChild {
                child: Some(child),
                pid,
            }),
            // 认领来的实例只有 PID：终止能力保留（退出/taskkill 整棵树），无句柄可放弃。
            HarnessProcess::Claimed { pid } => Some(OwnedChild {
                child: None,
                pid: Some(pid),
            }),
            other => {
                *self = other;
                None
            }
        }
    }

    /// 自有子进程是否已退出（同步探测，供启动等待循环使用）。
    pub(crate) fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        match self {
            HarnessProcess::Owned { child, .. } => child.try_wait().ok().flatten(),
            _ => None,
        }
    }
}

/// 应用全局状态：托管的 dsh 子进程状态、就绪地址与诊断信息。
#[derive(Default)]
pub struct AppState {
    /// 托管的 dsh 子进程状态（三态机，见 [`HarnessProcess`]）。
    pub proc: Mutex<HarnessProcess>,
    /// 已确认可用的 Web GUI 地址（新版自启动时为带 token 的握手 URL）。
    pub url: Mutex<Option<String>>,
    /// 退出流程是否已开始（幂等标记，防止重复显示退出动画/重复清理）。
    pub exiting: AtomicBool,
    /// 启动等待是否被用户取消（外壳「取消」按钮置位）。
    /// 每次启动等待开始时清零；只由等待循环读取并自行终止进程，
    /// 避免「取消」与循环两条路径同时操作子进程状态。
    pub launch_cancelled: AtomicBool,
    /// 子进程最近的输出（诊断用）。
    pub log_tail: Arc<Mutex<VecDeque<String>>>,
}

impl AppState {
    /// 访问托管状态。锁中毒（持锁线程 panic）时取回内部值继续——
    /// 启动/退出路径不应因 poison 而卡死，语义等同历史实现的“忽略锁错误”。
    pub(crate) fn proc_guard(&self) -> std::sync::MutexGuard<'_, HarnessProcess> {
        self.proc.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Serialize, Clone)]
pub struct InstallLine {
    pub stream: String,
    pub line: String,
}

#[derive(Serialize, Clone)]
pub struct LaunchInfo {
    pub url: String,
    pub port: u16,
    pub owned: bool,
    /// 接管的是新版实例且无法确定 WebView 是否已持有鉴权 cookie
    /// （true 时页面可能显示 401，外壳据此给出引导提示）。
    pub auth_uncertain: bool,
}

/// 启动进度载荷（事件 [`EVENT_LAUNCH_PROGRESS`]）。
#[derive(Serialize, Clone)]
pub struct LaunchProgress {
    /// 阶段：`spawning`（刚拉起进程）/ `loading`（插件加载中）/ `serving`
    /// （服务已就绪、等界面）/ `cancelled`（用户取消）。
    pub stage: String,
    /// 已等待秒数：由后端计算，前端直接显示，避免本地计时漂移。
    pub elapsed_secs: u64,
    /// 是否已超过软超时（前端据此提示「启动较慢」并露出日志入口）。
    pub slow: bool,
}

/// 启动等待的阶段判定（纯函数，便于回归测试）。
///
/// 端口已有响应（401 鉴权或旧版指纹）说明 `webserver` 已绑定，此后剩下的
/// 是等整棵插件树 settle 才会打印的 token URL 行——这两段在实测里是分开的
/// （dsh-purge 那次：152 秒绑定、又 39 秒才出 URL），所以值得区分展示。
fn launch_stage(probe: dsh::ProbeResult) -> &'static str {
    match probe {
        dsh::ProbeResult::MarkerHit | dsh::ProbeResult::AuthRequired => "serving",
        _ => "loading",
    }
}

/// 广播一次启动进度。`app` 为 None 时静默跳过（无 UI 的调用路径不受影响）。
fn emit_launch_progress(app: Option<&AppHandle>, stage: &str, elapsed: Duration, slow: bool) {
    let Some(app) = app else { return };
    let _ = app.emit(
        EVENT_LAUNCH_PROGRESS,
        LaunchProgress {
            stage: stage.to_string(),
            elapsed_secs: elapsed.as_secs(),
            slow,
        },
    );
}

fn tail_text(state: &AppState) -> String {
    state
        .log_tail
        .lock()
        .map(|q| q.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

/// 日志泵：把子进程输出流逐行写入共享环形缓冲（上限 256 行，超出丢最旧）**并落盘**。
/// stdout/stderr 共用同一实现，仅 tag 不同；`on_line` 在入队前收到原始行，
/// 供调用方做内容匹配（如捕获 `dsh web:` 启动 URL 行）。
///
/// 两份记录的内容有意不同：内存缓冲保留**原文**（启动失败/超时要把最近日志随错误
/// 信息返回外壳，那是当场排障用的）；落盘那份对 `token=` 打码——该 token 每次启动
/// 都换、对事后排障无用，留在文件里被贴到 issue 时反而是外泄。
fn spawn_log_pump<R, F>(reader: R, tag: &'static str, tail: Arc<Mutex<VecDeque<String>>>, on_line: F)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    F: Fn(&str) + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            on_line(&line);
            if let Ok(mut q) = tail.lock() {
                if q.len() >= 256 {
                    q.pop_front();
                }
                q.push_back(format!("[{tag}] {line}"));
            }
            // target 固定为 "dsh"：日志文件里一眼能分辨哪些行来自子进程。
            log::info!(target: "dsh", "[{tag}] {}", crate::logging::sanitize_token(&line));
        }
    });
}

// ---- PID 认领：把「本应用拉起的实例」与「别人的实例」分开 ----

/// 写入/清除配置里的 dsh PID（`None` = 清除该字段）。
///
/// 字段本身属于 `settings::LauncherConfig`：设置窗保存时整体重写 JSON，字段若不在
/// 结构体里就会被写没，认领随之退化成「外部实例」（退出不再停 DSH）。
fn save_recorded_pid(pid: Option<u32>) {
    crate::settings::set_recorded_dsh_pid(pid);
}

/// 读配置里记录的 dsh PID（没有记录或值不合法 → None）。
fn recorded_pid() -> Option<u32> {
    crate::settings::recorded_dsh_pid()
}

/// 查询进程的映像完整路径；进程已退出或查不到（无权限/系统进程）→ None。
///
/// 用 PowerShell 而不是新增 Win32 依赖：项目里其它「读系统状态」的地方
/// （reg / netstat / 桌面目录）都走同一条路子，只多一次几百毫秒的探测，
/// 且只在「端口已被占用」这一条启动路径上发生。
#[cfg(windows)]
fn process_image_path(pid: u32) -> Option<String> {
    use std::os::windows::process::CommandExt;
    // .Path 对无权访问的进程会是空串——那正是我们要的「判定不清」。
    let script = format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).Path");
    let out = StdCommand::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

#[cfg(not(windows))]
fn process_image_path(_pid: u32) -> Option<String> {
    None
}

/// 规范化用于比较的映像路径：Windows 大小写不敏感、两种分隔符混用，
/// 其余平台只去掉首尾空白与尾部分隔符。
fn normalize_image_path(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(|c| c == '\\' || c == '/');
    #[cfg(windows)]
    let normalized = trimmed.replace('/', "\\").to_lowercase();
    #[cfg(not(windows))]
    let normalized = trimmed.to_string();
    normalized
}

/// PID 认领判定（纯函数，便于回归测试）。
///
/// 三个条件同时成立才算「自有进程」：配置里有 PID、该 PID 现在还查得到映像、
/// 且映像与当前要拉起的 node.exe 是同一路径。任何一项不满足一律保守当作外部
/// 实例——误杀别人的进程比漏杀更糟。
fn claims_own_process(
    recorded: Option<u32>,
    live_image: Option<&str>,
    node_path: Option<&str>,
) -> bool {
    let (Some(pid), Some(live), Some(node)) = (recorded, live_image, node_path) else {
        return false;
    };
    // PID 0 是系统空闲进程，不可能是我们的子进程。
    if pid == 0 {
        return false;
    }
    !live.trim().is_empty() && normalize_image_path(live) == normalize_image_path(node)
}

#[tauri::command]
pub(crate) async fn check_dsh() -> dsh::CheckResult {
    dsh::check()
}

#[tauri::command]
pub(crate) async fn install_dsh(app: AppHandle) -> Result<String, String> {
    dsh::install(move |stream: &str, line: &str| {
        let _ = app.emit(
            EVENT_INSTALL_OUTPUT,
            InstallLine {
                stream: stream.to_string(),
                line: line.to_string(),
            },
        );
    })
    .await
}

/// 终止由本应用启动的 dsh 进程树（未托管任何自有进程时为空操作）。
/// 启动超时、`stop_dsh` 命令与退出清理共用本函数。
pub(crate) fn terminate_harness(state: &AppState) {
    if let Some(owned) = state.proc_guard().take_owned() {
        // 进程已被杀：旧 PID 可能被系统复用，留着记录会造成下一次启动误认领。
        save_recorded_pid(None);
        owned.terminate();
    }
}

#[tauri::command]
pub(crate) async fn stop_dsh(state: State<'_, AppState>) -> Result<(), String> {
    terminate_harness(&state);
    Ok(())
}

/// 放弃对 dsh 子进程的托管（孤儿继续运行），并清空托管状态。
pub(crate) fn orphan_harness(app: &AppHandle) {
    let state = app.state::<AppState>();
    // 先取出结果，令 MutexGuard 在本语句结束时即释放（其生命周期短于 state 守卫）。
    let owned = state.proc_guard().take_owned();
    if let Some(owned) = owned {
        owned.orphan();
    }
}

/// 确保 DeepSeek Harness 已启动并返回可访问的 Web GUI 地址。
/// 若默认端口上已有实例在运行则直接接管；否则拉起 `dsh web` 并等待就绪。
#[tauri::command]
pub(crate) async fn launch_dsh(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LaunchInfo, String> {
    launch_dsh_inner(&state, Some(&app)).await
}

/// 取消正在进行的启动等待（外壳启动卡片上的「取消」按钮）。
///
/// 只置位标志：等待循环下一轮自行终止刚拉起的进程并返回 `Err`，
/// 避免取消路径与等待循环同时操作子进程状态。
#[tauri::command]
pub(crate) fn cancel_launch(state: State<'_, AppState>) {
    state.launch_cancelled.store(true, Ordering::SeqCst);
}

/// 核心实现（与 tauri 解耦）：被 [`launch_dsh`] 与 [`restart_dsh_external`] 复用。
///
/// 就绪判定为双信号（新旧版本兼容，加法式扩展）：
/// - 旧版（无鉴权）：裸 `GET /` 响应体含 `__DSH_BOOT__` 指纹 → 用干净 URL；
/// - 新版（token+cookie 鉴权）：裸 `GET /` 返回 401，且子进程 stdout 已打印
///   `dsh web: <带 token 的 URL>` 行 → 用该 URL 交给 WebView 完成握手
///   （303 重定向 + 签 cookie），旧版永远不满足此条件、行为不变。
async fn launch_dsh_inner(
    state: &AppState,
    app: Option<&AppHandle>,
) -> Result<LaunchInfo, String> {
    // 每轮启动等待都从「未取消」开始：标志是全局的，上一轮用户点过的取消
    // 不能污染这一轮（否则升级流程里的第二次 launch 会被瞬间取消）。
    state.launch_cancelled.store(false, Ordering::SeqCst);

    // 1) 端口上已有 DeepSeek Harness 实例 → 直接接管（退出时不终止它）。
    let probe = dsh::probe_web(dsh::DSH_PORT).await;
    if probe == dsh::ProbeResult::MarkerHit || probe == dsh::ProbeResult::AuthRequired {
        let url = dsh::DSH_URL.to_string();
        *state.url.lock().map_err(|_| "应用状态不可用")? = Some(url.clone());
        // PID 认领：上一次本应用拉起、退出时被孤儿化的 dsh 仍占着端口时，
        // 配置里的 PID 必须「仍存活 + 映像与当前 node.exe 一致」才算自有进程
        // （此时退出仍 taskkill 它）。其余情形维持既有「接管外部实例」行为不变：
        // 不写 url 之外的状态，也不动 owned / auth_uncertain 的语义。
        let recorded = recorded_pid();
        let node = dsh::node_exe();
        let live_image = recorded.and_then(process_image_path);
        let claimed = claims_own_process(recorded, live_image.as_deref(), node.as_deref());
        match (claimed, recorded) {
            (true, Some(pid)) => state.proc_guard().claim(pid),
            _ => state.proc_guard().adopt(),
        }
        return Ok(LaunchInfo {
            url,
            port: dsh::DSH_PORT,
            owned: false,
            // 新版实例的 token 只有启动它的进程知道：接管后能否渲染取决于
            // WebView 是否已持有有效签名 cookie（此前成功握过手即可无缝接管）。
            auth_uncertain: probe == dsh::ProbeResult::AuthRequired,
        });
    }

    // 2) 拉起本地安装的 `dsh web`。
    let check = dsh::check();
    if !check.installed {
        return Err("DeepSeek Harness 尚未安装，请先完成安装。".to_string());
    }
    if check.node_ok == false {
        return Err("未检测到 Node.js，无法启动 DeepSeek Harness。".to_string());
    }
    let bin = check.bin_path.clone().ok_or("找不到 dsh 入口脚本（lib/bin.js）。")?;
    let node = dsh::node_exe().ok_or("未找到 node.exe，请确认 Node.js 已正确安装。")?;

    let mut cmd = tokio::process::Command::new(&node);
    cmd.arg(&bin)
        .arg("web")
        // 桌面启动器用内嵌 WebView 展示 GUI，禁止 dsh web 再打开系统默认浏览器
        // （dsh web 的 openBrowser 默认 true：重启后无既有实例时，每次拉起都会弹浏览器）。
        .arg("--no-open")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| format!("无法启动 dsh 进程：{e}"))?;
    let pid = child.id();
    // 持久化 PID：退出若是「保留」模式，进程会被孤儿化继续跑，下次启动靠它认领。
    save_recorded_pid(pid);

    // 新版 DSH 的启动 URL（带进程 token）只经 stdout 的 `dsh web:` 行对外；
    // 日志泵逐行回调匹配并捕获，旧版不打印该行时保持 None、自然走指纹路径。
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let tail = state.log_tail.clone();
    let capture_slot = captured.clone();
    if let Some(out) = child.stdout.take() {
        spawn_log_pump(out, "stdout", tail.clone(), move |line| {
            if let Some(url) = dsh::parse_web_url_line(line) {
                if let Ok(mut slot) = capture_slot.lock() {
                    *slot = Some(url);
                }
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        spawn_log_pump(err, "stderr", tail, |_| {});
    }

    state.proc_guard().own(child, pid);

    // 3) 等待 Web GUI 就绪，并每秒广播一次阶段进度（外壳据此显示活状态）。
    //
    //    实测有两条可分辨的时间线（2026-10-12，慢 profile）：端口先绑定
    //    （`serving`），之后还要等整棵插件树 settle 才打印 token URL 行——
    //    分别是 152 秒与 39 秒。因此 180 秒不再是死刑线，到点只标记 `slow`；
    //    真正的失败上限见 [`LAUNCH_HARD_TIMEOUT_SECS`]。
    let started = Instant::now();
    let soft = Duration::from_secs(LAUNCH_SOFT_TIMEOUT_SECS);
    let hard = Duration::from_secs(LAUNCH_HARD_TIMEOUT_SECS);
    let mut tick: u32 = 0;
    emit_launch_progress(app, "spawning", Duration::ZERO, false);

    loop {
        let probe = dsh::probe_web(dsh::DSH_PORT).await;
        let ready_url = match probe {
            dsh::ProbeResult::MarkerHit => Some(dsh::DSH_URL.to_string()),
            // 401 证明新版服务已就绪；只有同时捕获到 token URL 才算可用。
            dsh::ProbeResult::AuthRequired => captured.lock().ok().and_then(|slot| slot.clone()),
            _ => None,
        };
        if let Some(url) = ready_url {
            *state.url.lock().map_err(|_| "应用状态不可用")? = Some(url.clone());
            return Ok(LaunchInfo {
                url,
                port: dsh::DSH_PORT,
                owned: true,
                auth_uncertain: false,
            });
        }

        // 用户取消：只在这里处理（标志由 `cancel_launch` 置位），避免取消路径
        // 与等待循环同时操作子进程状态。
        if state.launch_cancelled.load(Ordering::SeqCst) {
            terminate_harness(&state);
            emit_launch_progress(app, "cancelled", started.elapsed(), false);
            return Err("已取消启动。".to_string());
        }

        let exited = state.proc_guard().try_wait();
        if let Some(status) = exited {
            let reason = tail_text(&state);
            let hint = if reason.contains("EADDRINUSE") {
                "\n提示：端口 3080 已被其他程序占用，请先释放该端口。"
            } else {
                ""
            };
            return Err(format!(
                "DeepSeek Harness 进程提前退出（{status}）{hint}\n{reason}"
            ));
        }

        let elapsed = started.elapsed();
        if elapsed >= hard {
            terminate_harness(&state);
            return Err(format!(
                "启动超时（{} 秒）：\n{}",
                hard.as_secs(),
                tail_text(&state)
            ));
        }

        tick = tick.wrapping_add(1);
        if tick % PROGRESS_EMIT_EVERY == 0 {
            emit_launch_progress(app, launch_stage(probe), elapsed, elapsed >= soft);
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// 接管外部实例后的一键重启：终止占用端口的外部实例，再按自启动路径
/// 拉起由本应用托管的实例（此后退出应用时会一并终止它）。
///
/// 仅用于"接管了非本应用启动的实例、但 WebView 无鉴权 cookie、页面停在
/// 401"的场景；由外壳提示条上的「关闭并重启」按钮显式触发（用户确认后）。
#[tauri::command]
pub(crate) async fn restart_dsh_external(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LaunchInfo, String> {
    if state.proc_guard().is_owned() {
        return Err("当前实例由本启动器托管，无需重启接管。".to_string());
    }
    // 端口仍被占用 → 终止外部实例并等待释放（taskkill 返回后进程退出可能有延迟）。
    if !matches!(dsh::probe_web(dsh::DSH_PORT).await, dsh::ProbeResult::NoResponse) {
        let killed = dsh::kill_processes_on_port(dsh::DSH_PORT).await?;
        if killed == 0 {
            return Err("未能终止占用端口 3080 的进程，请手动关闭该实例后重试。".to_string());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if matches!(dsh::probe_web(dsh::DSH_PORT).await, dsh::ProbeResult::NoResponse) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if !matches!(dsh::probe_web(dsh::DSH_PORT).await, dsh::ProbeResult::NoResponse) {
            return Err("端口 3080 上的外部实例未能及时退出，请手动关闭后重试。".to_string());
        }
    }
    launch_dsh_inner(&state, Some(&app)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 阶段判定：端口无响应 → `loading`；已有响应（新版 401 或旧版指纹）→
    /// `serving`。实测这两段分开约 39 秒（152s 绑定 / 191s 出 URL），
    /// 外壳据此显示不同文案，所以映射关系值得钉住。
    #[test]
    fn launch_stage_maps_probe_results() {
        assert_eq!(launch_stage(dsh::ProbeResult::NoResponse), "loading");
        assert_eq!(launch_stage(dsh::ProbeResult::Other), "loading");
        assert_eq!(launch_stage(dsh::ProbeResult::AuthRequired), "serving");
        assert_eq!(launch_stage(dsh::ProbeResult::MarkerHit), "serving");
    }

    /// 软超时是「耐心线」而非「死刑线」：它必须低于硬超时（留出继续等待的
    /// 余地），又不低于实测最慢案例的量级（dsh-purge 那次 202 秒）——否则正常
    /// 的慢启动会被误标成异常。
    #[test]
    fn soft_timeout_is_patient_but_below_hard_timeout() {
        assert!(
            LAUNCH_SOFT_TIMEOUT_SECS < LAUNCH_HARD_TIMEOUT_SECS,
            "软超时 {LAUNCH_SOFT_TIMEOUT_SECS}s 必须小于硬超时 {LAUNCH_HARD_TIMEOUT_SECS}s"
        );
        assert!(
            LAUNCH_SOFT_TIMEOUT_SECS >= 180,
            "软超时不应低于实测最慢案例的量级（约 190–200s）"
        );
    }

    /// 进度节流参数必须为正：`tick % PROGRESS_EMIT_EVERY` 除零会 panic。
    #[test]
    fn progress_emit_interval_is_nonzero() {
        assert!(PROGRESS_EMIT_EVERY > 0);
    }

    /// PID 认领判定：三个条件（有记录 / 进程活着 / 映像与 node.exe 同路径）
    /// 必须同时成立；任何一项缺失都保守当外部实例（宁可漏杀，不可误杀）。
    #[test]
    fn claim_requires_live_node_image() {
        let node = r"C:\Program Files\nodejs\node.exe";
        assert!(claims_own_process(Some(4321), Some(node), Some(node)));

        // 进程已退出 / 查不到映像路径 → 外部
        assert!(!claims_own_process(Some(4321), None, Some(node)));
        assert!(!claims_own_process(Some(4321), Some(""), Some(node)));
        // 没有记录 / 找不到当前 node → 外部
        assert!(!claims_own_process(None, Some(node), Some(node)));
        assert!(!claims_own_process(Some(4321), Some(node), None));
        // PID 0 是系统进程，永不认领
        assert!(!claims_own_process(Some(0), Some(node), Some(node)));
        // 映像不同（端口被另一个 node 程序占了）→ 外部
        assert!(!claims_own_process(
            Some(4321),
            Some(r"C:\other-app\node.exe"),
            Some(node)
        ));
    }

    /// Windows 的路径比较必须忽略大小写与分隔符差异：认领判定靠它，
    /// 大小写不一致就判成外部实例，等于「自有进程永远认不回来」。
    #[test]
    fn image_path_comparison_is_case_and_separator_insensitive() {
        let node = r"C:\Program Files\nodejs\node.exe";
        // 尾部分隔符与首尾空白都不该影响判定
        assert!(claims_own_process(
            Some(7),
            Some(" C:/Program Files/nodejs/ "),
            Some("C:/Program Files/nodejs")
        ));
        assert!(claims_own_process(Some(7), Some(format!("  {node}  ").as_str()), Some(node)));
        #[cfg(windows)]
        assert!(claims_own_process(
            Some(7),
            Some("c:/program files/NODEJS/NODE.EXE"),
            Some(node)
        ));
    }
}
