//! 启动器自身更新（应用自更新）：`check_launcher_update` / `install_launcher_update`。
//!
//! 只走官方插件的 **Rust API**（不引入 JS 插件、不新增 webview）：端点、公钥等
//! 配置由 `tauri.conf.json` 的 `plugins.updater` 提供，本模块只做两件事——
//! 把插件错误翻成「前缀：中文说明」，把下载进度变成节流事件。
//!
//! 为什么错误要带前缀：外壳按前缀决定提示与是否提供重试（网络可重试、验签
//! 失败只能重下），字符串里塞英文原文给不了这个判别力。

use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Error as UpdaterError, UpdaterExt};

/// 事件名：下载/验签/安装进度（外壳更新卡片据此画进度条）。
pub(crate) const EVENT_UPDATE_PROGRESS: &str = "update-progress";

/// 进度事件的最小间隔：下载分包极密（回环/千兆链路可到毫秒级），不节流会把
/// IPC 打满并淹没事件循环；200ms 既够流畅，也不会压制管道。
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(200);

/// `check_launcher_update` 的返回值。字段名与形状由前端约定，勿改。
#[derive(Serialize, Clone)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: String,
    pub notes: Option<String>,
    pub date: Option<String>,
    pub size: Option<u64>,
}

/// 进度事件载荷（字段名由前端约定，勿改）。
#[derive(Serialize, Clone)]
pub struct UpdateProgress {
    /// `downloading` | `verifying` | `installing`
    pub phase: String,
    pub received: u64,
    pub total: Option<u64>,
}

/// 失败分类：与外壳约定的五个前缀一一对应。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FailKind {
    Network,
    Manifest,
    Signature,
    Io,
    Unsupported,
}

impl FailKind {
    /// 机器可读前缀（外壳据此分支，是全模块唯一的稳定契约）。
    fn prefix(self) -> &'static str {
        match self {
            FailKind::Network => "network",
            FailKind::Manifest => "manifest",
            FailKind::Signature => "signature",
            FailKind::Io => "io",
            FailKind::Unsupported => "unsupported",
        }
    }

    /// 前缀后的人话说明（给用户看的那一半）。
    fn label(self) -> &'static str {
        match self {
            FailKind::Network => "无法访问更新服务，请检查网络或代理设置",
            FailKind::Manifest => "更新清单不可用（端点或清单内容有问题）",
            FailKind::Signature => "更新包验签失败，已拒绝安装",
            FailKind::Io => "本地读写或安装失败",
            FailKind::Unsupported => "当前系统或架构不受更新器支持",
        }
    }
}

/// 把插件错误归类。`Error` 是 `#[non_exhaustive]`，其余变体按「本地失败」兜底
/// ——这类错误的共同点是「重试同一份包也不会好」，与网络可重试不同。
fn classify(err: &UpdaterError) -> FailKind {
    use UpdaterError::*;
    match err {
        Reqwest(_)
        | Network(_)
        | Http(_)
        | InvalidHeaderValue(_)
        | InvalidHeaderName(_) => FailKind::Network,
        EmptyEndpoints
        | ReleaseNotFound
        | TargetNotFound(_)
        | TargetsNotFound(_)
        | Semver(_)
        | Serialization(_)
        | UrlParse(_)
        | FormatDate
        | InsecureTransportProtocol => FailKind::Manifest,
        Minisign(_)
        | Base64(_)
        | SignatureUtf8(_)
        | SignedVersionMismatch { .. }
        | MissingSignedVersion => FailKind::Signature,
        UnsupportedArch | UnsupportedOs => FailKind::Unsupported,
        _ => FailKind::Io,
    }
}

/// 统一错误出口：`前缀：中文说明（英文原文）`。
fn fail(err: &UpdaterError) -> String {
    let kind = classify(err);
    format!("{}：{}（{err}）", kind.prefix(), kind.label())
}

/// 清单里声明的安装包大小。
///
/// 为什么可能是 `None`：Tauri 官方清单只有 `url` 与 `signature`，没有 `size`。
/// 服务端额外提供了才读得到——读不到就让前端显示「大小未知」，不编造数字。
fn size_from_manifest(raw: &serde_json::Value, target: &str) -> Option<u64> {
    raw.get("platforms")
        .and_then(|p| p.get(target))
        .and_then(|p| p.get("size"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| raw.get("size").and_then(serde_json::Value::as_u64))
}

/// 广播一次进度（发送失败只可能是窗口已关闭，忽略）。
fn emit_progress(app: &AppHandle, phase: &str, received: u64, total: Option<u64>) {
    let _ = app.emit(
        EVENT_UPDATE_PROGRESS,
        UpdateProgress {
            phase: phase.to_string(),
            received,
            total,
        },
    );
}

/// 查询启动器自身是否有新版本。
#[tauri::command]
pub(crate) async fn check_launcher_update(app: AppHandle) -> Result<UpdateCheck, String> {
    let current = app.package_info().version.to_string();
    let updater = app.updater().map_err(|e| fail(&e))?;
    match updater.check().await {
        Ok(Some(update)) => Ok(UpdateCheck {
            current: update.current_version.clone(),
            latest: update.version.clone(),
            notes: update.body.clone(),
            date: update.date.map(|d| d.to_string()),
            size: size_from_manifest(&update.raw_json, &update.target),
        }),
        // 已是最新：latest 回填当前版本（外壳据此显示「已是最新」，不需要额外布尔字段）。
        Ok(None) => Ok(UpdateCheck {
            current: current.clone(),
            latest: current,
            notes: None,
            date: None,
            size: None,
        }),
        Err(e) => Err(fail(&e)),
    }
}

/// 下载 + 验签 + 交给安装器（Windows 上安装器启动成功后进程会退出）。
#[tauri::command]
pub(crate) async fn install_launcher_update(app: AppHandle) -> Result<(), String> {
    // Windows 安装器路径在插件内部直接 `std::process::exit(0)`，绕过
    // `RunEvent::Exit`——不在这里补一次退出清理，「退出时结束 DSH」会在自更新时
    // 静默失效。cleanup_before_exit 是插件原本会做的日志收尾，一并保留。
    let exit_handle = app.clone();
    let updater = app
        .updater_builder()
        .on_before_exit(move || {
            crate::shutdown::exit_launcher(&exit_handle);
            exit_handle.cleanup_before_exit();
        })
        .build()
        .map_err(|e| fail(&e))?;

    let update = updater
        .check()
        .await
        .map_err(|e| fail(&e))?
        .ok_or_else(|| "manifest：当前已是最新版本，没有可安装的更新。".to_string())?;

    // 下载 + 验签（验签在 `download` 内部完成），期间按 200ms 节流广播进度。
    let mut received: u64 = 0;
    let mut last_emit: Option<Instant> = None;
    let progress_app = app.clone();
    let bytes = update
        .download(
            move |delta: usize, total: Option<u64>| {
                received += delta as u64;
                let due = last_emit.is_none_or(|t| t.elapsed() >= PROGRESS_EMIT_INTERVAL);
                if due {
                    last_emit = Some(Instant::now());
                    emit_progress(&progress_app, "downloading", received, total);
                }
            },
            // 下载结束回调不必做别的事：下面的 `verifying` 事件就是它的对外表达。
            || {},
        )
        .await
        .map_err(|e| fail(&e))?;

    let done = bytes.len() as u64;
    let total = Some(done);
    emit_progress(&app, "verifying", done, total);
    emit_progress(&app, "installing", done, total);
    update.install(&bytes).map_err(|e| fail(&e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 前缀是外壳的机器可读契约：常见失败必须落到对应前缀上，不能一律 io。
    #[test]
    fn error_prefixes_match_failure_category() {
        assert_eq!(classify(&UpdaterError::EmptyEndpoints), FailKind::Manifest);
        assert_eq!(
            classify(&UpdaterError::Network("boom".into())),
            FailKind::Network
        );
        assert_eq!(
            classify(&UpdaterError::SignatureUtf8("bad".into())),
            FailKind::Signature
        );
        assert_eq!(classify(&UpdaterError::UnsupportedArch), FailKind::Unsupported);
        assert_eq!(classify(&UpdaterError::UnsupportedOs), FailKind::Unsupported);
        assert_eq!(
            classify(&UpdaterError::Io(std::io::Error::other("disk"))),
            FailKind::Io
        );

        // 对外文案：前缀打头，后接中文说明
        let manifest = fail(&UpdaterError::EmptyEndpoints);
        assert!(manifest.starts_with("manifest："), "{manifest}");
        let net = fail(&UpdaterError::Network("boom".into()));
        assert!(net.starts_with("network："), "{net}");
        let sig = fail(&UpdaterError::SignatureUtf8("bad".into()));
        assert!(sig.starts_with("signature："), "{sig}");
    }

    /// 大小来自清单（可选）：官方清单没有 size，只读服务端真的声明了的那个。
    #[test]
    fn size_from_manifest_reads_declared_size_only() {
        let raw: serde_json::Value = serde_json::json!({
            "version": "1.2.0",
            "platforms": {
                "windows-x86_64": { "url": "https://e/x.exe", "signature": "s", "size": 12345 }
            }
        });
        assert_eq!(size_from_manifest(&raw, "windows-x86_64"), Some(12345));
        assert_eq!(size_from_manifest(&raw, "linux-x86_64"), None);
        assert_eq!(
            size_from_manifest(&serde_json::json!({ "size": 7 }), "windows-x86_64"),
            Some(7)
        );
        assert_eq!(size_from_manifest(&serde_json::json!({}), "windows-x86_64"), None);
    }
}
