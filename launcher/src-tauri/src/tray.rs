//! 系统托盘：菜单构建（打开 Harness / 设置 / 版本 / 退出）、菜单事件分发，
//! 以及左键单击托盘图标的「打开 / 聚焦主窗口」。

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::shutdown;
use crate::windows;

/// 托盘图标 id（主题切换时按 id 取回托盘并更新图标，见 theme.rs）。
pub(crate) const TRAY_ID: &str = "main-tray";
/// 托盘菜单项 id。
const MENU_SHOW: &str = "show";
const MENU_SETTINGS: &str = "settings";
const MENU_VERSION: &str = "version";
const MENU_QUIT: &str = "quit";

/// 构建系统托盘。图标由调用方按当前系统主题传入（启动时及主题切换时）。
/// 菜单项：打开 DeepSeek Harness / 设置 / 版本（禁用态，仅展示）/ 退出。
///
/// 交互：**左键单击 = 打开或聚焦主窗口，菜单改右键弹**。不做双击——左键单击
/// 已经把窗口打开了，双击只会让同一次操作触发两遍（Windows 上双击必先落两次单击）。
pub(crate) fn build_tray(app: &AppHandle, icon: tauri::image::Image<'static>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW, "打开 DeepSeek Harness", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, MENU_SETTINGS, "设置", true, None::<&str>)?;
    // 版本信息项：禁用态（灰字、不可点击），运行时读取 tauri.conf.json 的版本号。
    let version = MenuItem::with_id(
        app,
        MENU_VERSION,
        format!("版本 v{}", app.package_info().version),
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let sep_above_version = PredefinedMenuItem::separator(app)?;
    let sep_above_quit = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&show, &settings, &sep_above_version, &version, &sep_above_quit, &quit],
    )?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("DeepSeek Harness")
        .menu(&menu)
        // 左键留给「打开 / 聚焦」，菜单改右键弹（Windows 托盘的通行做法）
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            MENU_SHOW => open_or_focus_main(app),
            MENU_SETTINGS => {
                if let Err(e) = windows::show_settings(app) {
                    eprintln!("[launcher] 打开设置窗口失败：{e}");
                }
            }
            MENU_QUIT => {
                shutdown::begin_exit(app);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 只认左键**抬起**：Windows 上按下与抬起各来一次事件，取抬起避免重复响应
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_or_focus_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 打开或聚焦主窗口——托盘左键单击与菜单「打开 DeepSeek Harness」共用这一条路径。
/// 三种状态都落到可见可交互：已可见且未最小化 → 只聚焦；最小化 → 还原 + 聚焦；
/// 「窗口 → 关闭窗口」隐藏后 → 显示 + 聚焦（窗口实例仍在，iframe 会话不会重载）。
fn open_or_focus_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(windows::WINDOW_MAIN) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
