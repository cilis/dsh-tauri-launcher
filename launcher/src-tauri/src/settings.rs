//! 桌面应用自身设置：`LauncherConfig` 持久化（exe 同目录 `.dsh-config.json`）、
//! Windows 开机自启（HKCU Run 键）、全局快捷键注册、桌面快捷方式创建，
//! 以及设置窗口对应的全部 tauri 命令。

use std::path::PathBuf;
use std::process::Command as StdCommand;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::windows::WINDOW_MAIN;

/// 配置文件（与 exe 同目录）。
const CONFIG_FILE: &str = ".dsh-config.json";
/// 开机自启注册表位置（当前用户 Run 键）。
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// 开机自启注册表值名。
const RUN_NAME: &str = "DeepSeekHarness";
/// 全局快捷键（按下即唤起主窗口）。设置窗口勾选=注册，取消=注销。
const HOTKEY: &str = "ctrl+shift+h";
/// 桌面快捷方式文件名（状态即 .lnk 存在性）。
const SHORTCUT_NAME: &str = "DeepSeek Harness.lnk";

/// 桌面应用自身的持久化设置（与 exe 同目录的 `.dsh-config.json`）。
/// 开机启动以注册表为准，无需在此持久化；全局快捷键的勾选状态在此保存。
#[derive(Serialize, Deserialize, Default)]
pub(crate) struct LauncherConfig {
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

pub(crate) fn load_config() -> LauncherConfig {
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
    // Run 键值必须用引号包裹：路径含空格（Program Files / 含空格用户目录）时，
    // 不带引号会被 Explorer 截断在第一个空格处，导致开机自启失效。
    let quoted_exe = format!("\"{exe_path}\"");
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
                    &quoted_exe,
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
pub(crate) fn apply_global_shortcut(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let gs = app.global_shortcut();
    if enabled {
        gs.on_shortcut(HOTKEY, move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                if let Some(w) = app.get_webview_window(WINDOW_MAIN) {
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
    desktop_dir().map(|dir| dir.join(SHORTCUT_NAME))
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
pub(crate) fn get_settings() -> SettingsSnapshot {
    SettingsSnapshot {
        autostart: autostart_enabled(),
        global_shortcut: load_config().global_shortcut,
        desktop_shortcut: desktop_shortcut_exists(),
        terminate_harness_on_exit: load_config().terminate_harness_on_exit,
        hotkey: HOTKEY.to_string(),
    }
}

#[tauri::command]
pub(crate) fn set_autostart_setting(enabled: bool) -> Result<(), String> {
    if set_autostart(enabled) {
        Ok(())
    } else {
        Err("设置开机启动失败，请检查注册表写入权限。".to_string())
    }
}

#[tauri::command]
pub(crate) fn set_global_shortcut_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    apply_global_shortcut(&app, enabled)?;
    let mut config = load_config();
    config.global_shortcut = enabled;
    save_config(&config);
    Ok(())
}

#[tauri::command]
pub(crate) fn set_desktop_shortcut_setting(enabled: bool) -> Result<(), String> {
    if set_desktop_shortcut(enabled) {
        Ok(())
    } else {
        Err("创建/删除桌面快捷方式失败，请检查桌面目录权限。".to_string())
    }
}

#[tauri::command]
pub(crate) fn set_terminate_harness_on_exit_setting(enabled: bool) -> Result<(), String> {
    let mut config = load_config();
    config.terminate_harness_on_exit = enabled;
    save_config(&config);
    Ok(())
}
