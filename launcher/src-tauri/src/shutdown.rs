//! 退出流程：按「退出时是否结束 Harness」设置执行清理（终止本启动器托管
//! 的 dsh 进程树，或放弃托管让 Harness 孤儿继续运行）、退出进度窗口编排，
//! 以及退出相关命令。
//!
//! 托盘退出、标题栏菜单退出、`.dsh-quit` 标记退出、进程退出四条路径最终
//! 都收敛到 [`begin_exit`] / [`exit_launcher`] 两条入口。

use std::process::Command as StdCommand;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::dsh;
use crate::harness::AppState;
use crate::harness;
use crate::settings;
use crate::windows;

/// 兜底结束占用 DSH 端口的进程（用于“退出时结束 Harness”且实例非本启动器启动的情况）。
fn kill_dsh_port_owner() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "$p = Get-NetTCPConnection -LocalPort {} -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty OwningProcess; \
             if ($p) {{ $proc = Get-Process -Id $p -ErrorAction SilentlyContinue; \
               if ($proc -and $proc.ProcessName -like 'node*') {{ Stop-Process -Id $p -Force -ErrorAction SilentlyContinue }} }}",
            dsh::DSH_PORT
        );
        let _ = StdCommand::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000)
            .status();
    }
}

/// 同步清理：终止由本应用启动的 dsh 进程树（“退出时结束 Harness”第一步）。
/// 与 [`harness::kill_child`] 共用同一状态迁移与终止实现——原先这里有一份
/// 独立的“取 pid → 同步 taskkill → try_lock 子进程 → start_kill”实现，
/// 阶段二收敛后仅保留调用点的差异（同步上下文可直接调用，无需 block_on）。
fn cleanup_on_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    harness::kill_child(&state);
}

/// 按“退出时是否结束 Harness”设置统一执行的退出动作，由托盘退出、
/// 标记退出与进程退出共用：
/// - 结束：终止本启动器托管的 dsh 进程树，并兜底关闭 DSH 端口进程；
/// - 保留（默认）：放弃托管，Harness 孤儿继续运行。
pub(crate) fn exit_launcher(app: &AppHandle) {
    if settings::load_config().terminate_harness_on_exit {
        cleanup_on_exit(app);
        kill_dsh_port_owner();
    } else {
        harness::orphan_harness(app);
    }
}

/// 统一退出入口：按“退出时结束 Harness”设置执行退出。
/// 需要结束时先显示退出进度窗口，再在后台线程完成清理，
/// 避免同步 taskkill 阻塞 UI 事件循环导致动画白屏。
pub(crate) fn begin_exit(app: &AppHandle) {
    if !settings::load_config().terminate_harness_on_exit {
        // 保留 Harness：退出很快，同样走后台任务，避免在异步上下文
        // （标记轮询）内同步执行退出清理。
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            exit_launcher(&handle);
            handle.exit(0);
        });
        return;
    }
    // 幂等：已在退出流程中（如 RunEvent::Exit 二次兜底）则仅做兜底清理。
    let state = app.state::<AppState>();
    if state
        .exiting
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        exit_launcher(app);
        return;
    }
    windows::close_visible_windows(app);
    windows::show_exit_progress(app);
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // 先让退出动画窗口渲染出首帧，再执行同步清理（taskkill / 端口兜底）。
        tokio::time::sleep(Duration::from_millis(200)).await;
        exit_launcher(&handle);
        handle.exit(0);
    });
}

/// 响应“仅退出桌面应用”请求（`.dsh-quit` 标记）：按设置结束或保留 Harness。
/// 标记轮询（markers.rs）中调用；幂等性由 begin_exit 内的 exiting 标记保证。
pub(crate) async fn quit_via_marker(app: &AppHandle) {
    begin_exit(app);
}

/// 主窗口自绘标题栏菜单：退出应用（与托盘「退出」同一条 begin_exit 路径，
/// 含退出动画与“是否结束 Harness”设置逻辑）。
#[tauri::command]
pub(crate) fn quit_app(app: AppHandle) {
    begin_exit(&app);
}
