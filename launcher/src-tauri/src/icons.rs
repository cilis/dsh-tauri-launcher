//! 图标资源与图标应用：编译期内嵌的单色图标变体（托盘/窗口/任务栏共用），
//! 以及把图标应用到托盘/窗口的底层细节（含 Win32 任务栏大图标路径）。
//!
//! 关键约束（Win11 25H2 实测）：tauri 的 `set_icon` 底层（tao）只发
//! `WM_SETICON+ICON_SMALL`（标题栏/Alt-Tab）；任务栏按钮读 `ICON_BIG`，
//! 且运行时切换仅发 `WM_SETICON` 不触发任务栏刷新，还须一并更新窗口类
//! 图标槽（`GCLP_HICON`/`GCLP_HICONSM`），详见 [`set_taskbar_icon_big`]。

/// 单色图标变体（512×512 PNG，编译期内嵌）：托盘/窗口/任务栏共用，
/// 避免 Tauri 默认 32×32 窗口图标在高 DPI 下被非整数缩放而发虚。
/// - `icon-black.png`：RGB(15,17,21) 近黑前景，对应系统浅色主题（亮底任务栏可见）。
/// - `icon-white.png`：白色前景，对应系统深色主题（暗底任务栏可见）。
/// 两个文件保留原始 alpha 通道（形状与原 icons/icon.png 一致），仅替换 RGB 通道。
/// 原 `icons/icon.png` 仍由 `tauri::generate_context!()` 用于安装包图标 / 资源管理器大图标。
const ICON_BLACK_BYTES: &[u8] = include_bytes!("../icons/icon-black.png");
const ICON_WHITE_BYTES: &[u8] = include_bytes!("../icons/icon-white.png");

/// 按当前系统主题挑选单色图标：浅色系统返回黑图标，深色系统返回白图标。
pub(crate) fn icon_for_theme(light: bool) -> tauri::image::Image<'static> {
    let bytes = if light { ICON_BLACK_BYTES } else { ICON_WHITE_BYTES };
    tauri::image::Image::from_bytes(bytes).expect("单色图标解码失败")
}

/// 把主题图标应用到单个窗口：小图标（标题栏/Alt-Tab）+ 任务栏大图标。
/// tauri 的 `set_icon` 底层（tao）只发 WM_SETICON+ICON_SMALL，而任务栏按钮
/// 读取的是 ICON_BIG，须另行补发（等价 tao 未对外暴露的 set_taskbar_icon）。
pub(crate) fn apply_window_icon(window: &tauri::WebviewWindow, img: &tauri::image::Image) {
    let _ = window.set_icon(img.clone());
    #[cfg(windows)]
    set_taskbar_icon_big(window, img);
}

/// 设置任务栏按钮图标：WM_SETICON+ICON_BIG 之外同时更新窗口类图标槽
/// （GCLP_HICON/GCLP_HICONSM）双路径。Win11 25H2 实测：启动时设置有效，
/// 但运行时切换仅发 WM_SETICON 不触发新任务栏刷新——类图标槽是任务栏
/// 读图标的另一路径，一并更新（Chromium 换图标的同款做法）。
/// RGBA→BGRA+AND 掩码的构造与 tao `WinIcon::from_rgba` 同构。
/// 创建的 HICON 不主动销毁：替换后旧句柄虽不再被窗口引用，
/// 但 Explorer 渲染缓存可能仍持有；主题切换低频、单份位图约 1MB，进程
/// 生命周期内可控，主动销毁反而有闪烁风险。
#[cfg(windows)]
fn set_taskbar_icon_big(window: &tauri::WebviewWindow, img: &tauri::image::Image) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIcon, SendMessageW, SetClassLongPtrW, GCLP_HICON, GCLP_HICONSM, ICON_BIG, WM_SETICON,
    };

    let rgba = img.rgba();
    let (width, height) = (img.width(), img.height());
    let pixel_count = width as usize * height as usize;
    if rgba.len() != pixel_count * 4 {
        eprintln!("[launcher] 任务栏大图标像素数据长度异常，跳过");
        return;
    }
    let mut bgra = rgba.to_vec();
    let mut and_mask = Vec::with_capacity(pixel_count);
    for px in bgra.chunks_exact_mut(4) {
        // alpha 取反（mod 256）作 AND 掩码，与 tao 实现保持一致
        and_mask.push(px[3].wrapping_sub(u8::MAX));
        px.swap(0, 2); // RGBA → BGRA
    }
    match unsafe { CreateIcon(None, width as i32, height as i32, 1, 32, and_mask.as_ptr(), bgra.as_ptr()) } {
        Ok(hicon) => {
            if let Ok(hwnd) = window.hwnd() {
                unsafe {
                    SendMessageW(
                        hwnd,
                        WM_SETICON,
                        Some(WPARAM(ICON_BIG as usize)),
                        Some(LPARAM(hicon.0 as isize)),
                    );
                    // Win11 25H2 实测：运行时仅 WM_SETICON ICON_BIG 不触发任务栏刷新
                    // （启动时设置有效、切换时不更新）。窗口类图标槽（GCLP_HICON/
                    // GCLP_HICONSM）是任务栏读图标的另一路径，一并更新（Chromium 同款做法）。
                    // 失败留痕（v2 C6）：返回值 0 既可能是「此前无类图标」也可能是失败，
                    // 故先清 last-error，再据 GetLastError 判定。
                    windows::Win32::Foundation::SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
                    SetClassLongPtrW(hwnd, GCLP_HICON, hicon.0 as isize);
                    SetClassLongPtrW(hwnd, GCLP_HICONSM, hicon.0 as isize);
                    let last_error = windows::Win32::Foundation::GetLastError();
                    if last_error.0 != 0 {
                        eprintln!("[launcher] 更新窗口类图标槽失败（GetLastError={}）", last_error.0);
                    }
                }
            }
        }
        Err(e) => eprintln!("[launcher] 创建任务栏大图标失败：{e}"),
    }
}
