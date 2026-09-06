//! Windows 系统主题检测与跟随：读取注册表 `AppsUseLightTheme`，变化时
//! 刷新托盘/窗口图标，并把主题经事件广播给主窗口外壳（外壳转发给 iframe
//! 内的插件，由 DSH 主题服务恢复「跟随系统」）。
//!
//! 背景（wry#806）：WebView2 页面的 prefers-color-scheme 只有宿主显式设置
//! PreferredColorScheme 才会随 Windows 变化，而 tauri/wry 未暴露该能力——
//! DSH 的「跟随系统」偏好会被冻结在启动值，故用事件广播 + 插件侧覆盖
//! MediaQueryList.matches 的方式恢复跟随（见 lib/client.js）。

use std::process::Command as StdCommand;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::icons;
use crate::windows::WINDOW_LABELS;
use crate::windows::WINDOW_MAIN;

/// 事件名：系统主题变化广播（外壳 main.js 监听后 postToFrame 给插件）。
pub(crate) const EVENT_SYSTEM_THEME: &str = "launcher-system-theme";

/// 当前 Windows 系统是否使用浅色主题。
/// 读取 `HKCU\...\Themes\Personalize\AppsUseLightTheme`：0=深色，1=浅色；
/// 缺失/失败默认浅色，与历史行为兼容。非 Windows 平台始终视为浅色（图标退化为黑）。
pub(crate) fn is_light_theme() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = StdCommand::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .creation_flags(0x0800_0000)
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                // 输出形如 `    AppsUseLightTheme    REG_DWORD    0x0`；
                // 取最后一个 token 与 "0x1" 精确比较，避免子串误匹配。
                match s.split_whitespace().next_back() {
                    Some("0x1") => true,
                    Some("0x0") => false,
                    // 值缺失或无法解析 → 默认浅色（兼容缺键场景）
                    _ => true,
                }
            }
            _ => true,
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// 把当前主题对应的单色图标应用到托盘和所有已知窗口。
/// 失败仅日志，不中断调用方（轮询任务下次会再尝试）。
pub(crate) fn apply_theme_icons(app: &AppHandle, light: bool) {
    let img = icons::icon_for_theme(light);
    if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
        if let Err(e) = tray.set_icon(Some(img.clone())) {
            eprintln!("[launcher] 切换托盘图标失败：{e}");
        }
    }
    for label in WINDOW_LABELS {
        if let Some(w) = app.get_webview_window(label) {
            icons::apply_window_icon(&w, &img);
        }
    }
}

/// 启动主题轮询任务（独立于心跳任务：心跳 1s 用于标记文件，主题 2s 单独成任务）。
/// 每 2 秒检测一次 Windows 系统主题，与上次相同则跳过；变化时刷新托盘/窗口
/// 图标，并把主题广播给主窗口外壳。跳过首次立即触发（启动时已按当前主题建好图标）。
pub(crate) fn spawn_theme_watch(handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        // 跳过首次立即触发（启动时已按当前主题建好图标）
        tick.tick().await;
        let mut last_light = is_light_theme();
        loop {
            tick.tick().await;
            let now = is_light_theme();
            if now != last_light {
                last_light = now;
                apply_theme_icons(&handle, now);
                // WebView2 页面的 prefers-color-scheme 只有宿主显式设置
                // PreferredColorScheme 才会随 Windows 变化，而 tauri/wry
                // 未暴露该能力（wry#806）：DSH 的「跟随系统」偏好会被冻结
                // 在启动值。把系统主题广播给外壳，由外壳转发 iframe 内的
                // 插件，经 DSH 主题服务恢复跟随（浏览器端 Chromium 原生跟随）。
                let scheme = if now { "light" } else { "dark" };
                if let Some(w) = handle.get_webview_window(WINDOW_MAIN) {
                    let _ = w.emit(EVENT_SYSTEM_THEME, scheme);
                }
            }
        }
    });
}
