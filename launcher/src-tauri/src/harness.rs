//! DeepSeek Harness 子进程托管：全局状态 [`AppState`]、检测/安装/启动/停止
//! 命令、日志环形缓冲与日志泵、孤儿化/强杀等子进程生命周期操作。
//! 具体的检测/安装/端口探测逻辑在 [`crate::dsh`]（与 tauri 解耦、可独立测试）。

use std::collections::VecDeque;
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::dsh;

/// 事件名：npm 安装日志逐行广播（外壳安装页实时显示）。
pub(crate) const EVENT_INSTALL_OUTPUT: &str = "install-output";

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
}

/// 从托管状态中取出的自有子进程，提供「终止」与「放弃」两种归宿。
/// 取出即状态置回 [`HarnessProcess::None`]，后续操作与状态锁无关。
pub(crate) struct OwnedChild {
    child: tokio::process::Child,
    pid: Option<u32>,
}

impl OwnedChild {
    /// 强制终止整棵进程树（Windows `taskkill /T /F`，其他平台 `kill -9`）。
    /// 同步实现：退出路径可能位于非异步上下文（托盘事件、`RunEvent::Exit`），
    /// taskkill 通常几十毫秒，代价可接受；句柄另做 `start_kill` 兜底。
    pub(crate) fn terminate(mut self) {
        if let Some(pid) = self.pid {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = StdCommand::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x0800_0000)
                    .status();
            }
            #[cfg(not(windows))]
            {
                let _ = StdCommand::new("kill").args(["-9", &pid.to_string()]).status();
            }
        }
        // 兜底：taskkill 后进程通常已退出，kill_on_drop 的二次 kill 无害。
        let _ = self.child.start_kill();
    }

    /// 放弃托管：进程作为孤儿继续运行，句柄交由系统回收。
    /// `forget` 阻止 drop 触发 kill_on_drop。
    pub(crate) fn orphan(self) {
        std::mem::forget(self.child);
    }
}

impl HarnessProcess {
    /// 是否由本应用启动并托管（true 时退出必须终止）。
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
            HarnessProcess::Owned { child, pid } => Some(OwnedChild { child, pid }),
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

fn tail_text(state: &AppState) -> String {
    state
        .log_tail
        .lock()
        .map(|q| q.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

/// 日志泵：把子进程输出流逐行写入共享环形缓冲（上限 256 行，超出丢最旧）。
/// stdout/stderr 共用同一实现，仅 tag 不同；`on_line` 在入队前收到原始行，
/// 供调用方做内容匹配（如捕获 `dsh web:` 启动 URL 行）。
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
        }
    });
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
pub(crate) fn kill_child(state: &AppState) {
    if let Some(owned) = state.proc_guard().take_owned() {
        owned.terminate();
    }
}

#[tauri::command]
pub(crate) async fn stop_dsh(state: State<'_, AppState>) -> Result<(), String> {
    kill_child(&state);
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
pub(crate) async fn launch_dsh(state: State<'_, AppState>) -> Result<LaunchInfo, String> {
    launch_dsh_inner(&state).await
}

/// 核心实现（与 tauri 解耦）：被 [`launch_dsh`] 与 [`restart_dsh_external`] 复用。
///
/// 就绪判定为双信号（新旧版本兼容，加法式扩展）：
/// - 旧版（无鉴权）：裸 `GET /` 响应体含 `__DSH_BOOT__` 指纹 → 用干净 URL；
/// - 新版（token+cookie 鉴权）：裸 `GET /` 返回 401，且子进程 stdout 已打印
///   `dsh web: <带 token 的 URL>` 行 → 用该 URL 交给 WebView 完成握手
///   （303 重定向 + 签 cookie），旧版永远不满足此条件、行为不变。
async fn launch_dsh_inner(state: &AppState) -> Result<LaunchInfo, String> {
    // 1) 端口上已有 DeepSeek Harness 实例 → 直接接管（退出时不终止它）。
    let probe = dsh::probe_web(dsh::DSH_PORT).await;
    if probe == dsh::ProbeResult::MarkerHit || probe == dsh::ProbeResult::AuthRequired {
        let url = dsh::DSH_URL.to_string();
        *state.url.lock().map_err(|_| "应用状态不可用")? = Some(url.clone());
        state.proc_guard().adopt();
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

    // 3) 等待 Web GUI 就绪（首次启动可能需要初始化，留足超时）。
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let ready_url = match dsh::probe_web(dsh::DSH_PORT).await {
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
        if Instant::now() >= deadline {
            kill_child(&state);
            return Err(format!("启动超时（180 秒）：\n{}", tail_text(&state)));
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
pub(crate) async fn restart_dsh_external(state: State<'_, AppState>) -> Result<LaunchInfo, String> {
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
    launch_dsh_inner(&state).await
}
