//! dsh-tauri-launcher 桌面应用入口：Tauri builder 装配与生命周期接线。
//!
//! 模块职责地图（2026-09 阶段一模块化，行为不变；详见优化报告 v2 阶段一）：
//! - [`dsh`]      — DeepSeek Harness 检测/安装/端口探测（与 tauri 解耦、可独立测试）
//! - [`icons`]    — 单色图标资源与托盘/窗口/任务栏图标应用（Win32 细节收口）
//! - [`windows`]  — 窗口 label 常量、settings/exiting 窗口预建与显示、窗口相关命令
//! - [`settings`] — LauncherConfig 持久化、开机自启、全局快捷键、桌面快捷方式
//! - [`harness`]  — dsh 子进程托管状态与启动/停止/日志
//! - [`shutdown`] — 退出流程（终止/孤儿化 + 退出进度编排）
//! - [`markers`]  — 心跳/退出标记文件轮询
//! - [`theme`]    — 系统主题检测、图标刷新与事件广播
//! - [`tray`]     — 托盘菜单构建与事件分发
//! - [`shell_server`] — 外壳页本地静态服务（127.0.0.1:3081，与 DSH 同站）

mod dsh;
mod harness;
mod icons;
mod markers;
mod settings;
mod shell_server;
mod shutdown;
mod theme;
mod tray;
mod windows;

use tauri::Manager;

use crate::harness::AppState;

/// 显式 AppUserModelID（必须与 `tauri.conf.json` 的 `identifier` 一致，有测试兜底）。
///
/// 为什么需要它：Win11 任务栏按钮的**图标与分组归属「应用标识」**——不显式设置时
/// Windows 按 exe 推导身份，按钮便只画 exe 内嵌图标（2026-09-13 实测：切换系统主题
/// 后窗口图标槽 WM_SETICON/类图标全换了新句柄、托盘图标也跟着变，但按钮像素仍是
/// exe 内嵌的 icon.png，连 DeleteTab+AddTab 重建按钮都不变）。显式 AUMID 让任务栏
/// 按本应用身份取图标，是社区修「任务栏图标不跟随」的标准前置步骤。
/// 必须在**创建任何窗口之前**调用，否则对已存在的窗口无效。
pub(crate) const APP_USER_MODEL_ID: &str = "com.dsh.launcher";

/// 设置显式 AppUserModelID（仅 Windows；失败只留痕，不影响启动）。
#[cfg(windows)]
fn set_app_user_model_id() {
    // 注意 `::windows` 前缀：本文件声明了同名模块 `mod windows`，不加前导 :: 会解析到
    // 自己的模块而不是 windows crate。
    use ::windows::core::PCWSTR;
    use ::windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    let wide: Vec<u16> = APP_USER_MODEL_ID.encode_utf16().chain(std::iter::once(0)).collect();
    let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(wide.as_ptr())) };
    if hr.is_err() {
        eprintln!("[launcher] 设置 AppUserModelID 失败：{hr:?}");
    }
}

/// 启动外壳静态服务。必须在 tauri 建窗口之前调用：主窗口 URL 指向
/// `http://127.0.0.1:3081`（与 DSH 的 127.0.0.1:3080 同站，SameSite=Strict
/// cookie 才可用），服务未就绪时主窗口会加载失败。
pub fn start_shell_server() {
    shell_server::start();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 任务栏图标/分组按应用标识归属：必须在 tauri 建窗口之前设置（见常量注释）。
    #[cfg(windows)]
    set_app_user_model_id();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            harness::check_dsh,
            harness::install_dsh,
            harness::launch_dsh,
            harness::restart_dsh_external,
            harness::stop_dsh,
            settings::get_settings,
            settings::set_autostart_setting,
            settings::set_global_shortcut_setting,
            settings::set_desktop_shortcut_setting,
            settings::set_terminate_harness_on_exit_setting,
            windows::close_settings,
            windows::open_settings_window,
            windows::hide_all_windows,
            windows::open_in_browser,
            shutdown::quit_app
        ])
        .setup(|app| {
            // 启动时按系统主题选择托盘/窗口图标，浅色系统用黑图标、深色系统用白图标。
            // 高分辨率图标：托盘与窗口（任务栏/标题栏）统一从 512×512 源缩放。
            let light = theme::is_light_theme();
            let icon = icons::icon_for_theme(light);
            if let Some(w) = app.get_webview_window(windows::WINDOW_MAIN) {
                icons::apply_window_icon(&w, &icon);
            }
            tray::build_tray(app.handle(), icon)?;

            // 启动时按持久化配置恢复全局快捷键注册。
            let handle = app.handle().clone();
            if settings::load_config().global_shortcut {
                if let Err(e) = settings::apply_global_shortcut(&handle, true) {
                    eprintln!("[launcher] 注册全局快捷键失败：{e}");
                }
            }

            // 预建设置/退出窗口（创建后隐藏）：必须在主窗口 iframe 加载 DSH
            // 之前完成，否则主线程同步 build 第二个 webview 会死锁
            // （约束细节见 windows.rs 模块注释）。
            windows::prebuild_aux_windows(&handle);

            // 标记文件轮询：心跳 + 响应“仅退出桌面应用”请求。
            markers::spawn_marker_task(handle.clone());
            // 系统主题轮询：图标刷新 + 事件广播（独立于心跳任务）。
            theme::spawn_theme_watch(handle);
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
                shutdown::exit_launcher(app);
            }
        });
}

#[cfg(test)]
mod tests {
    /// AppUserModelID 必须与打包标识一致：两者不一致会让任务栏把同一应用当成两个
    /// 身份（图标/固定项各挂一边），这类漂移肉眼很难发现，故用测试钉住。
    #[test]
    fn app_user_model_id_matches_bundle_identifier() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json");
        let text = std::fs::read_to_string(path).expect("读取 tauri.conf.json");
        let json: serde_json::Value = serde_json::from_str(&text).expect("解析 tauri.conf.json");
        assert_eq!(
            json["identifier"].as_str(),
            Some(super::APP_USER_MODEL_ID),
            "tauri.conf.json 的 identifier 与 APP_USER_MODEL_ID 不一致"
        );
    }
}
