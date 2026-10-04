//! 版本信息与更新检查：外壳「关于」弹窗与「检查更新」菜单的数据源。
//!
//! 设计约束（决策见 docs/roadmap.md 的 D5 / D6）：
//! - 只查两项：① 启动器 + 插件（**同号**，合并为一次 npm 对比）② DSH 本体（独立版本号）；
//! - 网络查询复用既有 npm 工具链（`npm view <pkg> dist-tags --json`），**不引入 HTTP 依赖**。
//!   外壳页的 CSP 是 `default-src 'self'`，页面无法直连 registry，必须由 Rust 侧发起；
//! - 本地读取（当前 exe 路径、已装插件版本）不联网，任何一项失败都不影响其它项；
//! - 与 tauri 解耦的部分（版本解析、profile 推断、文案）写成纯函数，便于单测。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;

use crate::dsh;

/// 启动器 + 插件的 npm 包名。两者**同号**：`build.ps1 -Bump` 同步三处版本号，
/// 且 npm 包自带 exe（`files` 含 `launcher/bin`），故合并为一次对比。
const LAUNCHER_PACKAGE: &str = "@lenorin/dsh-tauri-launcher";
/// npm registry 查询超时：离线或 npm 无响应时不能长时间转圈。
const NPM_QUERY_TIMEOUT: Duration = Duration::from_secs(8);
/// 推断不出 profile 时的回退值（`dsh plugin` 的常规 web profile）。
const DEFAULT_PROFILE: &str = "web";

/// 「关于 Launcher」的三项版本，各自容错：`None` 由前端渲染为「未安装」/「未知」。
#[derive(Serialize, Clone)]
pub struct Versions {
    /// 启动器自身版本（与 tauri.conf.json 同源）。
    pub launcher: String,
    /// 本地安装的 DSH 版本；未安装 → `None`。
    pub dsh: Option<String>,
    /// 本地 Node 版本（`node --version` 原样，含 `v` 前缀）；探测不到 → `None`。
    pub node: Option<String>,
}

/// 单项更新对比：`current` 取自本地，`latest` 来自 registry（查询失败 → `None`）。
#[derive(Serialize, Clone)]
pub struct UpdateItem {
    pub current: Option<String>,
    pub latest: Option<String>,
    /// `latest` 是否严格高于 `current`（语义化比较，预发布低于同号正式版）。
    /// `None` = 缺版本号或版本号不可解析，前端退回按字符串判断。
    pub has_update: Option<bool>,
}

/// 「检查更新」的完整结果——D5 的「出路三件套」（exe 路径 / 下载页 / 升级命令）
/// 所需信息都在这里，前端不需要再拼任何东西。
#[derive(Serialize, Clone)]
pub struct UpdateReport {
    /// 启动器 + 插件（同号，合并为一项）。
    pub launcher: UpdateItem,
    /// DSH 本体（独立版本号）。
    pub dsh: UpdateItem,
    /// 当前**正在运行**的 exe 完整路径——多副本场景下必须显示这一份，
    /// 否则用户会替换错文件（v1.0.10 的「自动发现运行中实例」处理的正是这种场景）。
    pub exe_path: String,
    /// 已安装的插件版本（读 profile 内 package.json，不联网）。
    pub plugin_version: Option<String>,
    /// 仅当运行中的 exe 与已装插件版本不一致时给出的附注（见 [`drift_note`]）。
    pub drift: Option<String>,
    /// 「复制升级命令」按钮的内容。
    pub upgrade_command: String,
    /// 整体失败原因（离线 / npm 缺失 / 超时）；某一项查询失败也在此说明。
    pub error: Option<String>,
}

/// 「关于 Launcher」：三项版本各自独立探测，互不阻塞。
#[tauri::command]
pub(crate) async fn get_versions(app: AppHandle) -> Versions {
    Versions {
        launcher: app.package_info().version.to_string(),
        dsh: dsh::check().version,
        node: node_version(),
    }
}

/// 「检查更新」：本地信息立即取，npm 两项**并行**查询，各自独立超时。
#[tauri::command]
pub(crate) async fn check_updates(app: AppHandle) -> UpdateReport {
    let exe_version = app.package_info().version.to_string();
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let dsh_version = dsh::check().version;
    let installed = installed_plugin();
    let profile = installed
        .as_ref()
        .map(|(p, _)| p.clone())
        .unwrap_or_else(|| DEFAULT_PROFILE.to_string());
    let plugin_version = installed.map(|(_, v)| v);

    // 两项并行：单查约 4 秒（npm 冷启动 + registry 往返），串行会逼近超时上限。
    let (launcher_latest, dsh_latest) = tokio::join!(
        npm_view_latest(LAUNCHER_PACKAGE),
        npm_view_latest(dsh::DSH_PACKAGE),
    );

    let errors: Vec<String> = [&launcher_latest, &dsh_latest]
        .iter()
        .filter_map(|r| r.as_ref().err().cloned())
        .collect();
    let launcher_latest = launcher_latest.ok();
    let dsh_latest = dsh_latest.ok();

    UpdateReport {
        launcher: UpdateItem {
            has_update: has_update(Some(&exe_version), launcher_latest.as_deref()),
            current: Some(exe_version.clone()),
            latest: launcher_latest,
        },
        dsh: UpdateItem {
            has_update: has_update(dsh_version.as_deref(), dsh_latest.as_deref()),
            current: dsh_version,
            latest: dsh_latest,
        },
        exe_path,
        drift: drift_note(&exe_version, plugin_version.as_deref()),
        plugin_version,
        upgrade_command: upgrade_command(&profile),
        error: if errors.is_empty() {
            None
        } else {
            Some(errors.join("；"))
        },
    }
}

/// 本地 Node 版本（`node --version` 原样输出）。
fn node_version() -> Option<String> {
    let node = dsh::node_exe()?;
    dsh::run_capture(&node, &["--version"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 已安装插件的 (profile 名, 版本)。全部来自本地文件读取，不联网。
fn installed_plugin() -> Option<(String, String)> {
    for path in plugin_package_candidates() {
        if let Some(version) = dsh::read_pkg_version(&path) {
            let profile =
                profile_from_path(&path).unwrap_or_else(|| DEFAULT_PROFILE.to_string());
            return Some((profile, version));
        }
    }
    None
}

/// 已装插件 `package.json` 的候选位置：DSH profile（推荐路径）优先，其次 npm 全局。
fn plugin_package_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for home in dsh_home_candidates() {
        if let Ok(entries) = std::fs::read_dir(home.join("profiles")) {
            for entry in entries.flatten() {
                out.push(
                    entry
                        .path()
                        .join("node_modules")
                        .join(LAUNCHER_PACKAGE)
                        .join("package.json"),
                );
            }
        }
    }
    if let Some(root) = dsh::npm_root() {
        let base = Path::new(&root);
        out.push(base.join(LAUNCHER_PACKAGE).join("package.json"));
        out.push(
            base.join("node_modules")
                .join(LAUNCHER_PACKAGE)
                .join("package.json"),
        );
    }
    out
}

/// DSH home 的候选目录（`DSH_HOME` 优先，其次用户主目录下的 `.dsh`）。
fn dsh_home_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(v) = std::env::var("DSH_HOME") {
        if !v.trim().is_empty() {
            out.push(PathBuf::from(v.trim()));
        }
    }
    for key in ["USERPROFILE", "HOME"] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                out.push(PathBuf::from(v.trim()).join(".dsh"));
            }
        }
    }
    out
}

/// 从 `<...>/profiles/<name>/node_modules/...` 提取 profile 名（供升级命令使用）。
fn profile_from_path(path: &Path) -> Option<String> {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let idx = parts.iter().position(|p| p == "profiles")?;
    parts.get(idx + 1).cloned()
}

/// 「复制升级命令」的内容：装最新版插件（包内自带 exe，一次替换两处）。
fn upgrade_command(profile: &str) -> String {
    format!("dsh plugin --profile {profile} add {LAUNCHER_PACKAGE}")
}

/// 漂移附注：只有当用户跑的 exe **不是** npm 包里那份时才可能出现
/// （走 `dsh plugin add` 的推荐路径永远同号），所以它不占检测项，
/// 只在实测不一致时多显示一行（D6）。
fn drift_note(exe_version: &str, plugin_version: Option<&str>) -> Option<String> {
    match plugin_version {
        Some(pv) if pv != exe_version => Some(format!(
            "当前运行的 exe 是 v{exe_version}，已安装插件是 v{pv}——你可能在运行从别处下载的 exe，升级时请替换这一份。"
        )),
        _ => None,
    }
}

/// 解析 `npm view <pkg> dist-tags --json` 的输出为 tag → 版本表。
/// npm 正常时只输出一块 JSON；有告警行时 JSON 仍是一整块，取首尾花括号之间解析。
fn parse_dist_tags(stdout: &str) -> Option<BTreeMap<String, String>> {
    let text = stdout.trim();
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

/// 从 dist-tags 里挑出 `latest` 指向的版本。
///
/// **只看 `latest`**：那是用户默认安装（`dsh plugin add` 不带版本号）拿到的渠道。
/// 不认 `next` / `alpha` / `beta`——它们指向更早的预览，推给用户等于让人主动换到更不稳的
/// 版本（实测 DSH 官方把 `latest` 指向 `0.2.0-rc.2`、`alpha` 指向 `0.2.1-alpha.1`，
/// 早先「current 是预发布就全 tag 取最高」的写法会把 alpha 推给 rc 用户）。
///
/// 值不可解析（或没有 `latest`）→ `None`，由调用方给出「无法解析」。
fn latest_version(tags: &BTreeMap<String, String>) -> Option<String> {
    tags.get("latest")
        .and_then(|raw| semver::Version::parse(raw.trim()).ok())
        .map(|v| v.to_string())
}

/// `latest` 是否严格高于 `current`。任一侧缺失或不可解析 → `None`（前端退回原判断）。
fn has_update(current: Option<&str>, latest: Option<&str>) -> Option<bool> {
    let current = semver::Version::parse(current?).ok()?;
    let latest = semver::Version::parse(latest?).ok()?;
    Some(latest > current)
}

/// 查询 registry 上某个包 `latest` 标签指向的版本，带超时。
/// Windows 上 npm 是 `npm.cmd`，与 dsh.rs 的既有做法一致，经 `cmd /C` 调用。
async fn npm_view_latest(pkg: &str) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.args(["/C", "npm", "view", pkg, "dist-tags", "--json"]);
    hide_console(&mut cmd);

    match tokio::time::timeout(NPM_QUERY_TIMEOUT, cmd.output()).await {
        Err(_) => Err(format!(
            "查询 {pkg} 超时（{} 秒）：npm 无响应，请确认网络可用",
            NPM_QUERY_TIMEOUT.as_secs()
        )),
        Ok(Err(e)) => Err(format!("无法执行 npm（{e}）：请确认 Node.js / npm 已安装")),
        Ok(Ok(out)) if !out.status.success() => {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if detail.is_empty() {
                format!("npm view {pkg} 失败")
            } else {
                detail
            })
        }
        Ok(Ok(out)) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let tags = parse_dist_tags(&text)
                .ok_or_else(|| format!("npm 返回了无法解析的 dist-tags：{pkg}"))?;
            latest_version(&tags)
                .ok_or_else(|| format!("{pkg} 的 dist-tags 里没有可用的 latest 版本号"))
        }
    }
}

/// Windows: 隐藏子进程控制台窗口（与 dsh.rs 的同名辅助一致；
/// 不复用是为了不把既有模块的私有函数改成公开可见性）。
#[cfg(windows)]
fn hide_console(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn hide_console(_cmd: &mut tokio::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 dist-tags 表，供下面几组用例共用。
    fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parse_dist_tags_reads_json_even_after_warning_lines() {
        let clean = parse_dist_tags("{\n  \"latest\": \"1.0.11\",\n  \"next\": \"1.1.0-rc.1\"\n}\n")
            .expect("正常 JSON 应可解析");
        assert_eq!(clean.get("latest").map(String::as_str), Some("1.0.11"));
        assert_eq!(clean.get("next").map(String::as_str), Some("1.1.0-rc.1"));

        let noisy = parse_dist_tags("npm warn something\n{\"latest\":\"1.2.3\"}\n")
            .expect("告警行在前也应可解析");
        assert_eq!(noisy.get("latest").map(String::as_str), Some("1.2.3"));

        assert!(parse_dist_tags("   ").is_none());
        assert!(parse_dist_tags("not json").is_none());
    }

    /// 本次修复的核心用例：rc 用户的 `latest` 更高（1.1.0 > 1.1.0-rc.1），
    /// 应被提示升到正式版——旧实现按字符串比不等，会报「新版本 v1.0.11」，指向的是降级。
    #[test]
    fn latest_version_reads_the_latest_tag() {
        let tags = tags(&[("latest", "1.1.0"), ("next", "1.1.0-rc.1")]);
        assert_eq!(latest_version(&tags).as_deref(), Some("1.1.0"));
    }

    /// **只认 `latest`**：next / alpha / beta 都指向更早的预览，不该推给用户。
    /// 实测 DSH 官方是 `latest`=0.2.0-rc.2、`alpha`=0.2.1-alpha.1——按「全 tag 取最高」
    /// 会把 alpha 推给 rc 用户，等于让人主动换到更不稳的版本。
    #[test]
    fn latest_version_ignores_other_channels() {
        let dsh = tags(&[
            ("alpha", "0.2.1-alpha.1"),
            ("next", "0.2.0-rc.2"),
            ("latest", "0.2.0-rc.2"),
        ]);
        assert_eq!(latest_version(&dsh).as_deref(), Some("0.2.0-rc.2"));

        // 别的 tag 版本更高也不看
        let higher_elsewhere = tags(&[("latest", "1.0.11"), ("next", "1.1.0-rc.3")]);
        assert_eq!(latest_version(&higher_elsewhere).as_deref(), Some("1.0.11"));
    }

    #[test]
    fn latest_version_survives_missing_or_invalid_tags() {
        assert!(latest_version(&BTreeMap::new()).is_none());
        assert!(latest_version(&tags(&[("next", "1.1.0-rc.1")])).is_none());
        assert!(latest_version(&tags(&[("latest", "not-a-version")])).is_none());
    }

    #[test]
    fn has_update_compares_semver_not_strings() {
        // 同号正式版高于预发布——字符串比较会得出相反结论
        assert_eq!(has_update(Some("1.1.0-rc.1"), Some("1.1.0")), Some(true));
        assert_eq!(has_update(Some("1.1.0"), Some("1.1.0-rc.1")), Some(false));
        assert_eq!(has_update(Some("1.0.11"), Some("1.0.11")), Some(false));
        assert_eq!(has_update(Some("1.0.9"), Some("1.0.11")), Some(true));
        // 任一侧缺失或不可解析 → None，前端退回原判断
        assert_eq!(has_update(None, Some("1.0.11")), None);
        assert_eq!(has_update(Some("1.0.11"), None), None);
        assert_eq!(has_update(Some("1.0.11"), Some("nope")), None);
    }

    #[test]
    fn profile_from_path_extracts_profile_name() {
        let p = Path::new(
            "/home/u/.dsh/profiles/web/node_modules/@lenorin/dsh-tauri-launcher/package.json",
        );
        assert_eq!(profile_from_path(p), Some("web".to_string()));
        assert_eq!(profile_from_path(Path::new("/tmp/local/package.json")), None);
    }

    #[test]
    fn upgrade_command_carries_profile_and_package() {
        let c = upgrade_command("web");
        assert!(c.contains("--profile web"), "{c}");
        assert!(c.contains(LAUNCHER_PACKAGE), "{c}");
    }

    #[test]
    fn drift_note_only_when_versions_differ() {
        assert!(drift_note("1.0.11", Some("1.0.11")).is_none());
        assert!(drift_note("1.0.11", None).is_none());
        assert!(drift_note("1.0.11", Some("1.0.9")).is_some());
    }
}
