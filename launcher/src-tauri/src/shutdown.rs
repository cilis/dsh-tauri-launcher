//! 退出流程：按「退出时是否结束 Harness」处置托管的进程、编排退出动画，
//! 以及退出相关命令。
//!
//! 退出触发路径（托盘菜单、标题栏菜单、`.dsh-quit` 标记、进程退出事件）
//! 收敛到两个入口：
//! - [`begin_exit`]：用户/插件发起的退出（幂等 + 退出动画 + 后台收尾）；
//! - [`exit_launcher`]：最终清理（同步，可安全用于 `RunEvent::Exit` 等
//!   非异步上下文）。
//! 两者最终都经 [`dispose_harness`]，处置方式由 [`ExitMode`] 决定。

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

/// 退出时对 Harness 的处置方式（由 `.dsh-config.json` 的设置决定）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitMode {
    /// 保留：放弃托管，Harness 作为孤儿继续运行（默认）。
    Keep,
    /// 结束：终止本启动器托管的进程树，并兜底关闭占用 DSH 端口的进程。
    Terminate,
}

impl ExitMode {
    /// 读取当前设置（唯一读取点：退出决策只在这里落一次）。
    pub(crate) fn from_config() -> Self {
        if settings::load_config().terminate_harness_on_exit {
            ExitMode::Terminate
        } else {
            ExitMode::Keep
        }
    }
}

/// 退出清理（同步）：按模式处置托管的 Harness。
/// 幂等——重复调用时状态已是“未托管”，第二次为空操作。
fn dispose_harness(app: &AppHandle, mode: ExitMode) {
    match mode {
        ExitMode::Terminate => {
            let state = app.state::<AppState>();
            harness::terminate_harness(&state);
            // 兜底：端口上的实例可能是接管来的外部实例，不在托管状态里。
            kill_dsh_port_owner();
        }
        ExitMode::Keep => harness::orphan_harness(app),
    }
}

/// 最终清理入口（同步）：按当前设置处置托管的 Harness。
/// 供 `RunEvent::Exit` 与 [`begin_exit`] 的后台任务调用；非异步上下文安全。
pub(crate) fn exit_launcher(app: &AppHandle) {
    dispose_harness(app, ExitMode::from_config());
}

/// 统一退出入口：按设置执行退出。需要结束时先显示退出进度窗口，再在后台
/// 任务中完成清理，避免同步 taskkill 阻塞 UI 事件循环导致动画白屏。
/// 退出决策只读一次设置，并把模式传给最终清理（避免重复读盘解析）。
pub(crate) fn begin_exit(app: &AppHandle) {
    let mode = ExitMode::from_config();
    if mode == ExitMode::Keep {
        // 保留 Harness：退出很快，同样走后台任务，避免在异步上下文
        // （标记轮询）内同步执行退出清理。
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            dispose_harness(&handle, ExitMode::Keep);
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
        dispose_harness(app, ExitMode::Terminate);
        return;
    }
    windows::close_visible_windows(app);
    windows::show_exit_progress(app);
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // 先让退出动画窗口渲染出首帧，再执行同步清理（taskkill / 端口兜底）。
        tokio::time::sleep(Duration::from_millis(200)).await;
        dispose_harness(&handle, ExitMode::Terminate);
        handle.exit(0);
    });
}

/// 主窗口自绘标题栏菜单：退出应用（与托盘「退出」同一条 begin_exit 路径，
/// 含退出动画与“是否结束 Harness”设置逻辑）。
#[tauri::command]
pub(crate) fn quit_app(app: AppHandle) {
    begin_exit(&app);
}
