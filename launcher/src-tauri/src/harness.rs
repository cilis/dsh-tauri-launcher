//! DeepSeek Harness 子进程托管：全局状态 [`AppState`]、检测/安装/启动/停止
//! 命令、日志环形缓冲与日志泵、孤儿化/强杀等子进程生命周期操作。
//! 具体的检测/安装/端口探测逻辑在 [`crate::dsh`]（与 tauri 解耦、可独立测试）。

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::dsh;

/// 事件名：npm 安装日志逐行广播（外壳安装页实时显示）。
pub(crate) const EVENT_INSTALL_OUTPUT: &str = "install-output";

/// 应用全局状态：被托管的 dsh 子进程及其诊断信息。
///
/// 一致性约定（历史实现，见优化报告 v2 B1）：`child`/`pid`/`owned` 三个字段
/// 分散在三种同步原语中，任何操作都必须按“先 child 后 pid、最后 owned”的
/// 顺序推进；`try_lock` vs 阻塞锁的选择取决于调用上下文（异步上下文内禁止
/// 阻塞锁）。阶段二计划收敛为单一 `HarnessProcess` 状态机。
#[derive(Default)]
pub struct AppState {
    /// 由本应用启动的 dsh 子进程。
    pub child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    /// 子进程 PID（用于 taskkill /T 结束整棵进程树）。
    pub pid: Mutex<Option<u32>>,
    /// 该进程是否由本应用启动（true 时退出必须终止；false 表示接管了已有实例）。
    pub owned: AtomicBool,
    /// 已确认可用的 Web GUI 地址（新版自启动时为带 token 的握手 URL）。
    pub url: Mutex<Option<String>>,
    /// 退出流程是否已开始（幂等标记，防止重复显示退出动画/重复清理）。
    pub exiting: AtomicBool,
    /// 子进程最近的输出（诊断用）。
    pub log_tail: Arc<Mutex<VecDeque<String>>>,
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

/// 终止由本应用启动的 dsh 进程树（异步版本）。
pub(crate) async fn kill_child(state: &AppState) {
    let pid = state.pid.lock().ok().and_then(|mut g| g.take());
    if let Some(pid) = pid {
        #[cfg(windows)]
        let res = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .output()
            .await;
        #[cfg(not(windows))]
        let res = tokio::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
            .await;
        let _ = res;
    }
    if let Some(mut child) = state.child.lock().await.take() {
        let _ = child.kill().await;
    }
    state.owned.store(false, Ordering::SeqCst);
}

#[tauri::command]
pub(crate) async fn stop_dsh(state: State<'_, AppState>) -> Result<(), String> {
    kill_child(&state).await;
    Ok(())
}

/// 放弃对 dsh 子进程的托管（孤儿继续运行），并清空托管状态。同步版本。
pub(crate) fn orphan_harness(app: &AppHandle) {
    let state = app.state::<AppState>();
    // 用 try_lock 而非 blocking_lock：本函数可能在 tokio 异步上下文
    // （标记轮询）被调用，阻塞锁会卡死 worker 线程。
    if let Ok(mut guard) = state.child.try_lock() {
        if let Some(child) = guard.take() {
            // forget 阻止 drop 触发 kill_on_drop，孤儿继续运行；句柄由系统回收。
            std::mem::forget(child);
        }
    }
    if let Ok(mut guard) = state.pid.lock() {
        guard.take();
    }
    state.owned.store(false, Ordering::SeqCst);
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
        state.owned.store(false, Ordering::SeqCst);
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

    state.child.lock().await.replace(child);
    *state.pid.lock().map_err(|_| "应用状态不可用")? = pid;
    state.owned.store(true, Ordering::SeqCst);

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
        let exited = state
            .child
            .lock()
            .await
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten());
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
            kill_child(&state).await;
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
    if state.owned.load(Ordering::SeqCst) {
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
