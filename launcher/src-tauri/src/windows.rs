//! 窗口管理：窗口 label 常量（跨模块共享的单一来源）、设置/退出进度
//! 窗口的预建与显示、退出时的窗口销毁，以及窗口/外壳菜单相关的
//! tauri 命令（设置窗口、在浏览器中打开）。
//!
//! 关键约束（WebView2 死锁，实测）：主窗口 iframe 加载跨源 DSH 之后，
//! 主线程同步 build 第二个 webview 窗口会死锁（build() 永不返回、事件
//! 循环停摆）。因此 settings/exiting 两个辅助窗口必须在启动早期预建
//! （`visible(false)` 防闪现），之后一律复用实例；退出路径绝不做现建兜底。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::dsh;
use crate::harness::AppState;
use crate::icons;
use crate::theme;

/// 主窗口（DSH 外壳 + 标题栏）label。
pub(crate) const WINDOW_MAIN: &str = "main";
/// 设置窗口 label（启动早期预建、之后复用）。
pub(crate) const WINDOW_SETTINGS: &str = "settings";
/// 退出进度窗口 label（启动早期预建、之后复用）。
pub(crate) const WINDOW_EXITING: &str = "exiting";
/// 主题图标需要覆盖的全部窗口 label。
pub(crate) const WINDOW_LABELS: [&str; 3] = [WINDOW_MAIN, WINDOW_SETTINGS, WINDOW_EXITING];

/// 创建设置窗口（预建后隐藏）。
/// 关键约束：主窗口 iframe 加载跨源 DSH 之后，主线程同步 build 第二个
/// webview 窗口会死锁（WebView2 多窗口竞态，实测 build() 永不返回、事件
/// 循环停摆）。因此设置窗口必须在启动早期预建（visible(false) 防闪现），
/// 之后一律复用实例（见 show_settings）。
fn create_settings_window(app: &AppHandle) -> tauri::Result<()> {
    let built = WebviewWindowBuilder::new(app, WINDOW_SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("设置")
        .inner_size(420.0, 540.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        // 预建不可见：避免启动时窗口闪现一帧；首次 show() 才亮相
        .visible(false)
        .center()
        .build()?;
    icons::apply_window_icon(&built, &icons::icon_for_theme(theme::is_light_theme()));
    let _ = built.hide();
    Ok(())
}

/// 打开设置窗口（显示预建实例；兜底分支正常流程不会走到）。
pub(crate) fn show_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(WINDOW_SETTINGS) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    create_settings_window(app)?;
    if let Some(w) = app.get_webview_window(WINDOW_SETTINGS) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    Ok(())
}

/// 创建退出进度窗口（预建后隐藏）。
/// 与设置窗口同一约束：主窗口 iframe 加载跨源 DSH 之后，主线程同步 build
/// 第二个 webview 窗口会死锁（见 create_settings_window 注释）。退出进度
/// 窗口同样必须在启动早期预建，退出时只复用 show()。
fn create_exit_progress_window(app: &AppHandle) -> tauri::Result<()> {
    let built = WebviewWindowBuilder::new(app, WINDOW_EXITING, WebviewUrl::App("exiting.html".into()))
        .title("退出中")
        .inner_size(320.0, 125.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .focused(false)
        // 预建不可见：避免启动时窗口闪现一帧；退出时 show() 才亮相
        .visible(false)
        .center()
        .build()?;
    icons::apply_window_icon(&built, &icons::icon_for_theme(theme::is_light_theme()));
    let _ = built.hide();
    Ok(())
}

/// 显示“正在退出 DeepSeek Harness 进程”进度窗口（仅结束 Harness 时使用）。
/// 只复用启动期预建的实例；**不做现建兜底**——退出路径绝不能依赖一次
/// 可能死锁的同步 build（iframe 已加载 DSH 后主线程 build 会永久卡死），
/// 预建失败时宁可没有进度动画，退出流程照常完成。
pub(crate) fn show_exit_progress(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW_EXITING) {
        let _ = w.show();
    } else {
        eprintln!("[launcher] 退出进度窗口未预建，跳过动画直接退出");
    }
}

/// 销毁主窗口与设置窗口（退出动画期间屏幕上只保留 exiting 进度窗口）。
/// 必须用 destroy() 而非 close()：main/settings 的 CloseRequested 都注册了
/// “隐藏到托盘”，close() 会被拦截；destroy() 绕过拦截直接销毁。
pub(crate) fn close_visible_windows(app: &AppHandle) {
    for label in [WINDOW_MAIN, WINDOW_SETTINGS] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.destroy();
        }
    }
}

/// 启动早期预建 settings/exiting 两个辅助窗口（在 setup 中调用）。
/// 必须在主窗口 iframe 加载 DSH 之前完成（WebView2 死锁约束，见模块注释）。
/// 预建失败仅日志，不中断启动：设置/退出动画缺失是可接受的降级。
pub(crate) fn prebuild_aux_windows(app: &AppHandle) {
    if let Err(e) = create_settings_window(app) {
        eprintln!("[launcher] 预建设置窗口失败：{e}");
    }
    if let Err(e) = create_exit_progress_window(app) {
        eprintln!("[launcher] 预建退出进度窗口失败：{e}");
    }
}

/// 创建主窗口（外壳页 + 标题栏）。
///
/// **为什么由 Rust 建、不让 tauri 按配置自动建**：`tauri.conf.json` 的 main 配置里
/// 写了 `create: false`，为的是能在这里挂 `on_new_window`——WebView2 默认吞掉新窗口
/// 请求（没有 handler 时 DSH 里 `target="_blank"` 与 `window.open` 全都没反应），
/// 而 handler 只能在建窗口那一刻挂，配置自动建的窗口挂不上。窗口属性仍全部读同一份
/// 配置（`from_config`），视觉与尺寸行为不变。
///
/// **调用顺序**：必须在 [`prebuild_aux_windows`] **之后**——主窗口一旦开始加载跨源
/// DSH，主线程再同步 build 其它 webview 会死锁（见模块注释）。
pub(crate) fn build_main_window(app: &AppHandle) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == WINDOW_MAIN)
        .cloned()
        .ok_or(tauri::Error::WindowNotFound)?;

    let _built = WebviewWindowBuilder::from_config(app, &config)?
        .on_new_window(|url, _features| {
            let target = url.as_str();
            if is_external_link(target) {
                if let Err(e) = open_with_system_browser(target) {
                    eprintln!("[launcher] 打开外部链接失败（{target}）：{e}");
                }
            }
            // 一律不开内嵌窗口：守住「不新增 webview」这条硬约束（预建约束 + 死锁风险）
            tauri::webview::NewWindowResponse::Deny
        })
        .build()?;
    Ok(())
}

/// 新窗口请求的目标是否该交给系统浏览器——只放行 http/https，
/// `about:blank` 之类（`window.open()` 的默认目标）直接丢弃。
fn is_external_link(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// 关闭（隐藏）设置窗口。窗口的 X 按钮同样触发 CloseRequested → 隐藏到托盘。
#[tauri::command]
pub(crate) fn close_settings(app: AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW_SETTINGS) {
        let _ = w.hide();
    }
}

/// 外壳菜单「关闭窗口」：隐藏主窗口与设置窗口，保留托盘与 Harness 子进程。
///
/// 🔴 禁止改用 `close_visible_windows()`——那是退出流程专用的 `destroy()`，
/// 窗口销毁后没有重建路径，托盘「打开」会失效（应用变僵尸）。本命令只用
/// `hide()`；恢复路径是托盘 `MENU_SHOW` 与全局快捷键（均为
/// show + unminimize + set_focus）。`hide()` 幂等：重复触发、或设置窗从未
/// 打开过（预建后一直隐藏）都无副作用。
#[tauri::command]
pub(crate) fn hide_all_windows(app: AppHandle) {
    for label in [WINDOW_MAIN, WINDOW_SETTINGS] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.hide();
        }
    }
}

/// 主窗口自绘标题栏菜单：打开设置窗口（显示预建实例）。
#[tauri::command]
pub(crate) fn open_settings_window(app: AppHandle) {
    if let Err(e) = show_settings(&app) {
        eprintln!("[launcher] 打开设置窗口失败：{e}");
    }
}

/// 外壳可请求打开的外链：key → URL（**唯一来源**，前端只传 key）。
/// 优化报告 v2 B4：原先 URL 表同时维护在 Rust 白名单与前端 HELP_URLS，
/// 新增外链要改两处且容易漏；现在前端不持有任何 URL，由本表解析。
const EXTERNAL_LINKS: [(&str, &str); 3] = [
    ("website", "https://www.deepseek.com/harness/"),
    ("docs", "https://deepseek-harness.github.io/deepseek-harness/guide/quickstart"),
    // 「关于 → 检查更新」的下载页：启动器本体只发 GitHub Release（npm 渠道走 dsh plugin add）
    (
        "releases",
        "https://github.com/cilis/dsh-tauri-launcher/releases/latest",
    ),
];

/// 「在浏览器中打开」本地 DSH 的 key：URL 由应用状态给出——新版 DSH 的
/// 地址带进程 token（只有启动它的本应用知道），须原样交给系统浏览器才能
/// 完成鉴权握手；状态为空时回退到默认本地地址。
const LINK_DSL: &str = "dsl";

/// 主窗口自绘标题栏菜单：用系统默认浏览器打开白名单目标。
/// 经 explorer.exe 打开（不走 cmd shell），URL 无解释执行风险；
/// 前端只传 key，URL 由 [`EXTERNAL_LINKS`] 或应用状态解析——
/// 不存在“页面传入任意地址”的路径。
#[tauri::command]
pub(crate) fn open_in_browser(app: AppHandle, key: String) -> Result<(), String> {
    let url = resolve_link(&app, &key)?;
    open_with_system_browser(&url)
}

/// 解析外链 key（URL 的唯一来源）。
fn resolve_link(app: &AppHandle, key: &str) -> Result<String, String> {
    if key == LINK_DSL {
        let current = app
            .state::<AppState>()
            .url
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        return Ok(current.unwrap_or_else(|| dsh::DSH_URL.to_string()));
    }
    EXTERNAL_LINKS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, url)| (*url).to_string())
        .ok_or_else(|| format!("未知的外链标识：{key}"))
}

/// 用系统默认程序打开 URL。
///
/// **为什么用 `ShellExecuteW` 而不是 `explorer.exe`**：explorer 会把带查询串的地址当成
/// 本地路径解析——实测 `explorer "http://127.0.0.1:3080/"` 能进浏览器，而带进程 token 的
/// `http://127.0.0.1:3080/?token=xxx` 会打开文件资源管理器（DSH 新版地址正是后者，所以
/// 「文件 → 在浏览器中打开」会开出资源管理器）。`ShellExecuteW` 是 Windows 官方的
/// 「用默认程序打开」入口，不经过任何 shell 解析，两种形式都稳定。
///
/// `pub(crate)`：除本模块的 [`open_in_browser`] 命令外，主窗口的 `on_new_window`
/// 也用它把 DSH 页面里的外部链接交出去。
pub(crate) fn open_with_system_browser(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use ::windows::core::PCWSTR;
        use ::windows::Win32::UI::Shell::ShellExecuteW;
        use ::windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        let file: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // 返回值语义是 HINSTANCE：按文档 ≤ 32 表示失败（借用了旧的错误码约定）
        let code = result.0 as isize;
        if code <= 32 {
            return Err(format!("调用系统浏览器失败（ShellExecuteW 返回 {code}）"));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = url;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只有 http/https 交给系统浏览器；`window.open()` 的默认目标 `about:blank`
    /// 与 `file:` / `javascript:` 之类一律丢弃。
    #[test]
    fn only_http_links_go_to_system_browser() {
        assert!(is_external_link("https://deepseek-harness.github.io/guide"));
        assert!(is_external_link("http://127.0.0.1:3080/"));
        assert!(!is_external_link("about:blank"));
        assert!(!is_external_link("file:///C:/Windows/System32/"));
        assert!(!is_external_link("javascript:alert(1)"));
        assert!(!is_external_link(""));
    }
}
