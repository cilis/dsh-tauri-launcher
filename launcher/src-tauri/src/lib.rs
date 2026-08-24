//! Tauri 主逻辑：命令注册、系统托盘、退出时终止 DeepSeek Harness 进程。

mod dsh;

use std::collections::VecDeque;
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tokio::io::{AsyncBufReadExt, BufReader};
use std::path::PathBuf;
use std::process::Stdio;

/// 与 Web 设置插件（deepseek-harness-tauri）协作的标记文件，位于启动器 exe 同目录：
/// `.dsh-heartbeat` 每秒写入一次时间戳（供插件判断进程存活）；
/// `.dsh-quit` 存在 → 仅退出桌面应用本身。开机启动与全局快捷方式由本应用的设置窗口控制。
const HEARTBEAT_MARKER: &str = ".dsh-heartbeat";
const QUIT_MARKER: &str = ".dsh-quit";
const CONFIG_FILE: &str = ".dsh-config.json";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "DeepSeekHarness";
/// 全局快捷键（按下即唤起主窗口）。设置窗口勾选=注册，取消=注销。
const HOTKEY: &str = "ctrl+shift+h";
/// 应用图标源（512×512 PNG，编译期内嵌）：托盘/窗口/任务栏共用，
/// 避免 Tauri 默认 32×32 窗口图标在高 DPI 下被非整数缩放而发虚。
const APP_ICON_BYTES: &[u8] = include_bytes!("../icons/icon.png");

/// 解码内嵌的应用图标。
fn app_icon() -> tauri::image::Image<'static> {
    tauri::image::Image::from_bytes(APP_ICON_BYTES).expect("应用图标解码失败")
}

/// 应用全局状态：被托管的 dsh 子进程及其诊断信息。
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

/// 桌面应用自身的持久化设置（与 exe 同目录的 `.dsh-config.json`）。
/// 开机启动以注册表为准，无需在此持久化；全局快捷键的勾选状态在此保存。
#[derive(Serialize, Deserialize, Default)]
struct LauncherConfig {
    #[serde(default)]
    pub global_shortcut: bool,
    /// 退出时是否一并结束 DeepSeek Harness 进程（默认 false：只退出启动器）。
    #[serde(default)]
    pub terminate_harness_on_exit: bool,
}

fn config_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|dir| dir.join(CONFIG_FILE))
}

fn load_config() -> LauncherConfig {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<LauncherConfig>(&s).ok())
        .unwrap_or_default()
}

fn save_config(config: &LauncherConfig) {
    if let Some(path) = config_path() {
        if let Ok(json) = serde_json::to_string_pretty(config) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// 查询 Windows 开机自启是否已启用（HKCU Run 键是否存在）。
fn autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = StdCommand::new("reg")
            .args(["query", RUN_KEY, "/v", RUN_NAME])
            .creation_flags(0x0800_0000)
            .output();
        matches!(out, Ok(o) if o.status.success())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 写入/删除 Windows 开机自启（HKCU Run 键，幂等；`reg add /f`）。
/// 由本应用（不受沙箱限制的桌面进程）执行，是开机启动的唯一控制入口。
/// 返回操作后的注册表状态是否与目标一致。
fn set_autostart(enabled: bool) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe_path = exe.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let res = if enabled {
            StdCommand::new("reg")
                .args([
                    "add",
                    RUN_KEY,
                    "/v",
                    RUN_NAME,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &exe_path,
                    "/f",
                ])
                .creation_flags(0x0800_0000)
                .status()
        } else {
            StdCommand::new("reg")
                .args(["delete", RUN_KEY, "/v", RUN_NAME, "/f"])
                .creation_flags(0x0800_0000)
                .status()
        };
        let _ = res;
    }
    autostart_enabled() == enabled
}

/// 注册/注销全局快捷键。注册后按下热键即显示并聚焦主窗口（DeepSeek Harness）。
fn apply_global_shortcut(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let gs = app.global_shortcut();
    if enabled {
        gs.on_shortcut(HOTKEY, move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
        })
        .map_err(|e| format!("注册全局快捷键失败：{e}"))
    } else {
        gs.unregister(HOTKEY)
            .map_err(|e| format!("注销全局快捷键失败：{e}"))
    }
}

/// 桌面目录（只解析一次并缓存，避免每次查询都拉起 PowerShell）。
static DESKTOP_DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// 解析用户桌面目录（`[Environment]::GetFolderPath('Desktop')` 自动处理 OneDrive 重定向）。
fn desktop_dir() -> Option<PathBuf> {
    DESKTOP_DIR
        .get_or_init(|| {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let out = StdCommand::new("powershell")
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        "[Environment]::GetFolderPath('Desktop')",
                    ])
                    .creation_flags(0x0800_0000)
                    .output()
                    .ok()?;
                if !out.status.success() {
                    return None;
                }
                let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if dir.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(dir))
                }
            }
            #[cfg(not(windows))]
            {
                None
            }
        })
        .clone()
}

fn shortcut_path() -> Option<PathBuf> {
    desktop_dir().map(|dir| dir.join("DeepSeek Harness.lnk"))
}

/// 桌面快捷方式是否存在（.lnk 文件本身即状态，无需持久化）。
fn desktop_shortcut_exists() -> bool {
    shortcut_path().map(|p| p.exists()).unwrap_or(false)
}

/// PowerShell 单引号字符串转义：内部单引号翻倍。
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 创建/删除桌面快捷方式（.lnk 指向本应用 exe，含图标），返回操作后状态是否与目标一致。
fn set_desktop_shortcut(enabled: bool) -> bool {
    if !enabled {
        // 删除：文件本就不存在也视为成功（以终态为准）。
        if let Some(p) = shortcut_path() {
            let _ = std::fs::remove_file(&p);
        }
        return desktop_shortcut_exists() == enabled;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let Some(lnk) = shortcut_path() else {
            return false;
        };
        let exe_str = exe.to_string_lossy().into_owned();
        let dir = exe
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        // IconLocation 的逗号必须整体位于引号内，避免被 PowerShell 解析成数组。
        let script = format!(
            "$ws=New-Object -ComObject WScript.Shell; $sc=$ws.CreateShortcut({lnk}); \
             $sc.TargetPath={exe}; $sc.WorkingDirectory={dir}; $sc.IconLocation={icon}; \
             $sc.Description='DeepSeek Harness 桌面启动器'; $sc.Save()",
            lnk = ps_quote(&lnk.to_string_lossy()),
            exe = ps_quote(&exe_str),
            dir = ps_quote(&dir),
            icon = ps_quote(&format!("{exe_str},0")),
        );
        let res = StdCommand::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000)
            .status();
        let _ = res;
    }
    desktop_shortcut_exists() == enabled
}

/// 打开设置窗口（首次点击时创建，之后显示已存在的实例）。
fn show_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    // 无边框：隐藏系统边框与标题栏。窗口拖动依赖页面元素上的
    // data-tauri-drag-region（settings.html），关闭走页面「关闭」按钮（隐藏到托盘）。
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("设置")
        .inner_size(420.0, 540.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .center()
        .build()?;
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.set_icon(app_icon());
    }
    Ok(())
}

/// 放弃对 dsh 子进程的托管（孤儿继续运行），并清空托管状态。同步版本。
fn orphan_harness(app: &AppHandle) {
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

/// 兜底结束占用 DSH 端口的进程（用于“退出时结束 Harness”且实例非本启动器启动的情况）。
fn kill_dsh_port_owner() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "$p = Get-NetTCPConnection -LocalPort {} -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty OwningProcess; if ($p) {{ Stop-Process -Id $p -Force -ErrorAction SilentlyContinue }}",
            dsh::DSH_PORT
        );
        let _ = StdCommand::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000)
            .status();
    }
}

/// 按“退出时是否结束 Harness”设置统一执行的退出动作，由托盘退出、
/// 标记退出与进程退出共用：
/// - 结束：终止本启动器托管的 dsh 进程树，并兜底关闭 DSH 端口进程；
/// - 保留（默认）：放弃托管，Harness 孤儿继续运行。
fn exit_launcher(app: &AppHandle) {
    if load_config().terminate_harness_on_exit {
        cleanup_on_exit(app);
        kill_dsh_port_owner();
    } else {
        orphan_harness(app);
    }
}

/// 销毁主窗口与设置窗口（退出动画期间屏幕上只保留 exiting 进度窗口）。
/// 必须用 destroy() 而非 close()：main/settings 的 CloseRequested 都注册了
/// “隐藏到托盘”，close() 会被拦截；destroy() 绕过拦截直接销毁。
fn close_visible_windows(app: &AppHandle) {
    for label in ["main", "settings"] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.destroy();
        }
    }
}

/// 显示“正在退出 DeepSeek Harness 进程”进度窗口（仅结束 Harness 时使用）。
/// 主窗口会被导航到 DSH Web，故退出反馈用独立小窗口承载；重复调用直接跳过。
fn show_exit_progress(app: &AppHandle) {
    if app.get_webview_window("exiting").is_some() {
        return;
    }
    let result = WebviewWindowBuilder::new(app, "exiting", WebviewUrl::App("exiting.html".into()))
        .title("退出中")
        .inner_size(320.0, 125.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .focused(false)
        .center()
        .build();
    match result {
        Ok(w) => {
            let _ = w.set_icon(app_icon());
        }
        Err(e) => eprintln!("[launcher] 创建退出进度窗口失败：{e}"),
    }
}

/// 统一退出入口：按“退出时结束 Harness”设置执行退出。
/// 需要结束时先显示退出进度窗口，再在后台线程完成清理，
/// 避免同步 taskkill 阻塞 UI 事件循环导致动画白屏。
fn begin_exit(app: &AppHandle) {
    if !load_config().terminate_harness_on_exit {
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
    close_visible_windows(app);
    show_exit_progress(app);
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // 先让退出动画窗口渲染出首帧，再执行同步清理（taskkill / 端口兜底）。
        tokio::time::sleep(Duration::from_millis(200)).await;
        exit_launcher(&handle);
        handle.exit(0);
    });
}

/// 响应“仅退出桌面应用”请求（`.dsh-quit` 标记）：按设置结束或保留 Harness。
async fn quit_via_marker(app: &AppHandle) {
    begin_exit(app);
}

#[tauri::command]
async fn check_dsh() -> dsh::CheckResult {
    dsh::check()
}

#[tauri::command]
async fn install_dsh(app: AppHandle) -> Result<String, String> {
    dsh::install(move |stream: &str, line: &str| {
        let _ = app.emit(
            "install-output",
            InstallLine {
                stream: stream.to_string(),
                line: line.to_string(),
            },
        );
    })
    .await
}

/// 终止由本应用启动的 dsh 进程树（异步版本）。
async fn kill_child(state: &AppState) {
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
async fn stop_dsh(state: State<'_, AppState>) -> Result<(), String> {
    kill_child(&state).await;
    Ok(())
}

/// 设置窗口当前快照：开机启动、全局快捷键、桌面快捷方式、退出行为与热键组合。
#[derive(Serialize, Clone)]
pub struct SettingsSnapshot {
    pub autostart: bool,
    pub global_shortcut: bool,
    pub desktop_shortcut: bool,
    pub terminate_harness_on_exit: bool,
    pub hotkey: String,
}

#[tauri::command]
fn get_settings() -> SettingsSnapshot {
    SettingsSnapshot {
        autostart: autostart_enabled(),
        global_shortcut: load_config().global_shortcut,
        desktop_shortcut: desktop_shortcut_exists(),
        terminate_harness_on_exit: load_config().terminate_harness_on_exit,
        hotkey: HOTKEY.to_string(),
    }
}

#[tauri::command]
fn set_autostart_setting(enabled: bool) -> Result<(), String> {
    if set_autostart(enabled) {
        Ok(())
    } else {
        Err("设置开机启动失败，请检查注册表写入权限。".to_string())
    }
}

#[tauri::command]
fn set_global_shortcut_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    apply_global_shortcut(&app, enabled)?;
    let mut config = load_config();
    config.global_shortcut = enabled;
    save_config(&config);
    Ok(())
}

#[tauri::command]
fn set_desktop_shortcut_setting(enabled: bool) -> Result<(), String> {
    if set_desktop_shortcut(enabled) {
        Ok(())
    } else {
        Err("创建/删除桌面快捷方式失败，请检查桌面目录权限。".to_string())
    }
}

#[tauri::command]
fn set_terminate_harness_on_exit_setting(enabled: bool) -> Result<(), String> {
    let mut config = load_config();
    config.terminate_harness_on_exit = enabled;
    save_config(&config);
    Ok(())
}

/// 关闭（隐藏）设置窗口。窗口的 X 按钮同样触发 CloseRequested → 隐藏到托盘。
#[tauri::command]
fn close_settings(app: AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.hide();
    }
}

/// 确保 DeepSeek Harness 已启动并返回可访问的 Web GUI 地址。
/// 若默认端口上已有实例在运行则直接接管；否则拉起 `dsh web` 并等待就绪。
#[tauri::command]
async fn launch_dsh(state: State<'_, AppState>) -> Result<LaunchInfo, String> {
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
        let tail2 = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut q) = tail2.lock() {
                    if q.len() >= 256 {
                        q.pop_front();
                    }
                    q.push_back(format!("[stdout] {line}"));
                }
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let tail2 = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut q) = tail2.lock() {
                    if q.len() >= 256 {
                        q.pop_front();
                    }
                    q.push_back(format!("[stderr] {line}"));
                }
            }
        });
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

/// 同步清理：托盘退出与进程退出时的兜底，结束由本应用启动的 dsh 进程树。
fn cleanup_on_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    let pid = state.pid.lock().ok().and_then(|mut g| g.take());
    if let Some(pid) = pid {
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
    if let Ok(mut guard) = state.child.try_lock() {
        if let Some(child) = guard.as_mut() {
            let _ = child.start_kill();
        }
    }
    state.owned.store(false, Ordering::SeqCst);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            check_dsh,
            install_dsh,
            launch_dsh,
            stop_dsh,
            get_settings,
            set_autostart_setting,
            set_global_shortcut_setting,
            set_desktop_shortcut_setting,
            set_terminate_harness_on_exit_setting,
            close_settings
        ])
        .setup(|app| {
            // 高分辨率图标：托盘与窗口（任务栏/标题栏）统一从 512×512 源缩放。
            let icon = app_icon();
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_icon(icon.clone());
            }
            let show = MenuItem::with_id(app, "show", "打开 DeepSeek Harness", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &settings, &quit])?;
            TrayIconBuilder::with_id("main-tray")
                .icon(icon)
                .tooltip("DeepSeek Harness")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                    "settings" => {
                        if let Err(e) = show_settings(app) {
                            eprintln!("[launcher] 打开设置窗口失败：{e}");
                        }
                    }
                    "quit" => {
                        begin_exit(app);
                    }
                    _ => {}
                })
                .build(app)?;

            // 启动时按持久化配置恢复全局快捷键注册。
            let handle = app.handle().clone();
            if load_config().global_shortcut {
                if let Err(e) = apply_global_shortcut(&handle, true) {
                    eprintln!("[launcher] 注册全局快捷键失败：{e}");
                }
            }

            // 标记文件轮询：心跳 + 响应“仅退出桌面应用”请求（每秒一次，
            // 退出标记消费延迟 ≤1 秒；插件侧的心跳“新鲜窗口”须与之匹配）。
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tick.tick().await;
                    let Ok(exe) = std::env::current_exe() else {
                        continue;
                    };
                    let Some(dir) = exe.parent() else {
                        continue;
                    };
                    // 心跳：写入当前 Unix 时间戳，供沙箱内的设置插件读取以判断进程存活。
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let _ = std::fs::write(dir.join(HEARTBEAT_MARKER), stamp.to_string());
                    if dir.join(QUIT_MARKER).exists() {
                        let quit_path = dir.join(QUIT_MARKER);
                        // 仅当内容为 "1" 且 60 秒内新鲜才退出；插件用重写内容（"0"）
                        // 取消退出请求，避免依赖外部命令删除文件。
                        let requested = std::fs::read_to_string(&quit_path)
                            .map(|s| s.trim() == "1")
                            .unwrap_or(false);
                        let fresh = std::fs::metadata(&quit_path)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.elapsed().ok())
                            .map(|age| age < Duration::from_secs(60))
                            .unwrap_or(false);
                        if requested && fresh {
                            let _ = std::fs::remove_file(&quit_path);
                            quit_via_marker(&handle).await;
                            break;
                        }
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 关闭窗口时隐藏到系统托盘，退出请使用托盘菜单的“退出”。
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                exit_launcher(app);
            }
        });
}
