//! 日志落盘（roadmap v1.2 W10）：接入 `tauri-plugin-log`，把 dsh 子进程输出与
//! 应用自身的诊断输出一并写入 `%LOCALAPPDATA%\dsh-launcher\logs\`。
//!
//! 为什么用官方插件而不是自己写 writer：轮转、归档保留、本地时区、格式化都由它
//! 提供，各处诊断只要改用 `log` 宏就同时进 stdout 与文件，不必维护两套写盘路径。
//! 代价是插件的 targets 在启动时固定、运行时无法增删，所以「日志开关」做成文件
//! target 上的过滤条件——切换后下一条日志即生效，且不需要（也不可能）重建
//! logger：全局 logger 只能设置一次。
//!
//! 轮转语义（与 roadmap 原文的偏差已记在 §4.1 实现注记）：插件按**单文件大小**
//! 轮转，归档名带完整时间戳（`launcher_YYYY-MM-DD_HH-MM-SS.log`），`KeepSome(n)`
//! 保留最近 n 个归档；它没有「按天切割」。定位某天的日志看归档名即可。
//!
//! 与既有内存缓冲的分工：`harness::AppState::log_tail`（256 行环形缓冲）**保持
//! 不变**——启动失败/超时要把最近日志随错误信息返回外壳，那条路径依赖进程内
//! 缓冲。落盘是给「事后报障」用的第二份证据，两者互补。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::plugin::TauriPlugin;
use tauri::Runtime;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

/// 落盘开关的运行时状态（设置窗可切换，立即生效）。
static ENABLED: AtomicBool = AtomicBool::new(true);

/// `%LOCALAPPDATA%` 下的固定目录名：不随 exe 位置变化，装在 Program Files 下也写得进。
const LOG_DIR_NAME: &str = "dsh-launcher";
/// 日志文件主干名：当前文件 `launcher.log`，归档 `launcher_<时间戳>.log`。
const LOG_FILE_STEM: &str = "launcher";
/// 单文件上限。插件默认只有 40 KB（几秒的 dsh 启动输出就撑满），这里放到 5 MB。
const MAX_FILE_SIZE: u128 = 5 * 1024 * 1024;
/// 归档保留个数（不含正在写的那个）。
const KEEP_ARCHIVES: usize = 5;

/// 日志目录：`%LOCALAPPDATA%\dsh-launcher\logs`。
/// `LOCALAPPDATA` 缺失时（极罕见）退回 exe 同目录的 `logs/`，仍保证有落点。
pub(crate) fn log_dir() -> PathBuf {
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        if !base.is_empty() {
            return PathBuf::from(base).join(LOG_DIR_NAME).join("logs");
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
}

/// 当前正在写的日志文件。
fn active_log_path() -> PathBuf {
    log_dir().join(LOG_FILE_STEM).with_extension("log")
}

/// 归档文件名的判据（与插件的轮转命名一致：`launcher_<时间戳>.log`）。
fn is_archive_name(name: &str) -> bool {
    name.starts_with(&format!("{LOG_FILE_STEM}_")) && name.ends_with(".log")
}

fn archive_count(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| is_archive_name(&entry.file_name().to_string_lossy()))
        .count()
}

/// 目录可写性探测：能建目录、且能写一个探测文件才算可写。
///
/// **必须在挂文件 target 之前判定**：插件 setup 里对 `TargetKind::Folder` 会
/// `create_dir_all` 再打开文件，任一步失败都让它返回 `Err`，而插件 setup 失败会
/// 让 tauri 建窗口之前的 `build()` 直接失败——日志不能把应用拖死。所以不可写时
/// 干脆不挂文件 target，降级为「只输出 stdout」。
fn is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".dsh-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 落盘是否启用。
pub(crate) fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 设置落盘开关（设置窗命令调用）。
pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// 对 `token=` 的值打码。
///
/// DSH 新版启动时向 stdout 打印 `dsh web: http://127.0.0.1:3080/?token=<随机>`。
/// 该 token 每次启动都换、对排障没有价值，但被写进日志再贴到 issue 里等于外泄，
/// 所以只有「对外留存」的这一路脱敏；内存环形缓冲与随错误信息返回外壳的内容
/// 保持原文（那条路径是当场排障用的）。
pub(crate) fn sanitize_token(line: &str) -> String {
    const KEY: &str = "token=";
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(pos) = rest.find(KEY) {
        let value_start = pos + KEY.len();
        out.push_str(&rest[..value_start]);
        let tail = &rest[value_start..];
        let end = tail
            .find(|c: char| matches!(c, '&' | ' ' | '"' | '\'' | '\r' | '\n'))
            .unwrap_or(tail.len());
        if end > 0 {
            out.push_str("***");
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// 构造日志插件：stdout 与（目录可写时的）文件两个目标。
pub(crate) fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let dir = log_dir();
    let mut builder = tauri_plugin_log::Builder::new()
        .clear_targets()
        // 控制台输出不受开关影响：桌面应用通常没有控制台，留着是为了「从终端启动」
        // 时能直接看到，零成本。
        .target(Target::new(TargetKind::Stdout));

    if is_writable(&dir) {
        // 文件目标：开关做成过滤条件，切换立即生效。
        builder = builder.target(
            Target::new(TargetKind::Folder {
                path: dir,
                file_name: Some(LOG_FILE_STEM.to_string()),
            })
            .filter(|_| is_enabled()),
        );
    } else {
        // 降级路径：不挂文件目标，应用照常运行（只是没有落盘证据）。
        eprintln!("[launcher] 日志目录不可写，本次运行只输出到 stdout");
    }

    builder
        .max_file_size(MAX_FILE_SIZE)
        .rotation_strategy(RotationStrategy::KeepSome(KEEP_ARCHIVES))
        // 本地时区：日志时间要能直接和用户描述的现象对上。
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .level(log::LevelFilter::Info)
        .build()
}

/// 设置窗「日志」页的快照。
#[derive(serde::Serialize, Clone)]
pub struct LogInfo {
    pub enabled: bool,
    /// 日志目录（展示与「打开日志目录」用）。
    pub dir: String,
    /// 当前日志文件路径。
    pub active_file: String,
    /// 当前文件字节数（尚未产生时为 0）。
    pub active_size: u64,
    /// 归档文件个数。
    pub archives: usize,
    /// 目录是否可用（与插件挂文件目标时的判据同一个函数，避免两处判断不一致）。
    pub writable: bool,
}

#[tauri::command]
pub(crate) fn get_log_info() -> LogInfo {
    let dir = log_dir();
    let active = active_log_path();
    LogInfo {
        enabled: is_enabled(),
        writable: is_writable(&dir),
        active_size: std::fs::metadata(&active).map(|m| m.len()).unwrap_or(0),
        archives: archive_count(&dir),
        dir: dir.display().to_string(),
        active_file: active.display().to_string(),
    }
}

/// 打开日志目录（复用外壳既有的「用系统默认程序打开」链路，不新增打开方式）。
#[tauri::command]
pub(crate) fn open_log_dir() -> Result<(), String> {
    let dir = log_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建日志目录：{e}"))?;
    crate::windows::open_with_system_browser(&dir.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_values_are_masked_but_other_text_survives() {
        let line = "dsh web: http://127.0.0.1:3080/?token=abc123 已就绪";
        let masked = sanitize_token(line);
        assert!(!masked.contains("abc123"), "token 值必须被打码：{masked}");
        assert!(masked.contains("token=***"));
        assert!(masked.ends_with("已就绪"), "非 token 部分要原样保留：{masked}");
    }

    #[test]
    fn masking_stops_at_delimiters_and_handles_several_values() {
        assert_eq!(sanitize_token("a?token=x&b=1"), "a?token=***&b=1");
        assert_eq!(sanitize_token("token=a token=b"), "token=*** token=***");
        assert_eq!(sanitize_token("no secrets here"), "no secrets here");
        // 空值不打码，避免把「token=」这种残缺文本改得面目全非
        assert_eq!(sanitize_token("token="), "token=");
    }

    #[test]
    fn log_dir_always_ends_with_logs_folder() {
        let dir = log_dir();
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("logs"));
    }

    #[test]
    fn archive_name_detection_matches_plugin_rotation() {
        // 判据错了会让归档计数与保留策略对不上（插件的归档名带时间戳下划线）
        assert!(is_archive_name("launcher_2026-10-06_01-23-45.log"));
        assert!(!is_archive_name("launcher.log"));
        assert!(!is_archive_name("launcher_notes.txt"));
        assert!(!is_archive_name("other_2026-10-06.log"));
    }

    /// 不可写目录必须走降级分支而不是让插件 setup 失败（那会让整个应用起不来）。
    /// 这里用「路径上已有同名文件」构造必然失败的建目录场景，跨平台稳定。
    #[test]
    fn unwritable_dir_is_detected() {
        let base = std::env::temp_dir().join("dsh-logging-probe-test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::write(&base, b"not a directory").expect("写入占位文件");
        let dir = base.join("logs");
        assert!(!is_writable(&dir), "路径被文件占住时应判定为不可写");
        let _ = std::fs::remove_file(&base);
    }
}
