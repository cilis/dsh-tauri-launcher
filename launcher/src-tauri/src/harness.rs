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
    /// 已确认可用的 Web GUI 地址。
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
}

fn tail_text(state: &AppState) -> String {
    state
        .log_tail
        .lock()
        .map(|q| q.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

/// 日志泵：把子进程输出流逐行写入共享环形缓冲（上限 256 行，超出丢最旧）。
/// stdout/stderr 共用同一实现，仅 tag 不同。
fn spawn_log_pump<R>(reader: R, tag: &'static str, tail: Arc<Mutex<VecDeque<String>>>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
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
    // 1) 端口上已有 DeepSeek Harness 实例 → 直接接管（退出时不终止它）。
    if dsh::is_dsh_serving(dsh::DSH_PORT).await {
        let url = dsh::DSH_URL.to_string();
        *state.url.lock().map_err(|_| "应用状态不可用")? = Some(url.clone());
        state.owned.store(false, Ordering::SeqCst);
        return Ok(LaunchInfo {
            url,
            port: dsh::DSH_PORT,
            owned: false,
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

    let tail = state.log_tail.clone();
    if let Some(out) = child.stdout.take() {
        spawn_log_pump(out, "stdout", tail.clone());
    }
    if let Some(err) = child.stderr.take() {
        spawn_log_pump(err, "stderr", tail);
    }

    state.child.lock().await.replace(child);
    *state.pid.lock().map_err(|_| "应用状态不可用")? = pid;
    state.owned.store(true, Ordering::SeqCst);
    let url = dsh::DSH_URL.to_string();

    // 3) 等待 Web GUI 就绪（首次启动可能需要初始化，留足超时）。
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if dsh::is_dsh_serving(dsh::DSH_PORT).await {
            break;
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

    *state.url.lock().map_err(|_| "应用状态不可用")? = Some(url.clone());
    Ok(LaunchInfo {
        url,
        port: dsh::DSH_PORT,
        owned: true,
    })
}
