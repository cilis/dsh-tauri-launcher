//! DeepSeek Harness 管理逻辑：检测安装、npm 自动安装、端口探测。
//! 与 tauri 解耦，便于独立测试。

use std::collections::VecDeque;
use std::env;
use std::path::Path;
use std::process::Command as StdCommand;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;

/// npm 包名：DeepSeek Harness 本体。
pub const DSH_PACKAGE: &str = "@deepseek-ai/dsh";
/// Web GUI 默认端口。
pub const DSH_PORT: u16 = 3080;
/// Web GUI 默认地址。
pub const DSH_URL: &str = "http://127.0.0.1:3080/";
/// dsh web 页面中的指纹标记，用于确认端口上跑的是 DeepSeek Harness。
pub const DSH_MARKER: &str = "__DSH_BOOT__";

/// Windows: 隐藏子进程控制台窗口。
#[cfg(windows)]
fn hide_console_std(cmd: &mut StdCommand) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn hide_console_std(_cmd: &mut StdCommand) {}

#[cfg(windows)]
fn hide_console_tokio(cmd: &mut TokioCommand) {
    cmd.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn hide_console_tokio(_cmd: &mut TokioCommand) {}

/// 同步执行命令并捕获标准输出。
pub fn run_capture(cmd: &str, args: &[&str]) -> Result<String, String> {
    let mut c = StdCommand::new(cmd);
    c.args(args);
    hide_console_std(&mut c);
    let out = c.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if detail.is_empty() {
            format!("命令执行失败：{cmd} {}", args.join(" "))
        } else {
            detail
        })
    }
}

/// 解析 node.exe 完整路径（支持 DSH_LAUNCHER_NODE 覆盖，便于测试）。
pub fn node_exe() -> Option<String> {
    if let Ok(v) = env::var("DSH_LAUNCHER_NODE") {
        if !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
    }
    run_capture("cmd", &["/C", "where", "node"])
        .ok()
        .and_then(|s| s.lines().next().map(str::trim).map(String::from))
        .filter(|s| !s.is_empty())
}

/// npm 全局安装根目录（支持 DSH_LAUNCHER_NPM_ROOT 覆盖，便于测试）。
pub fn npm_root() -> Option<String> {
    if let Ok(v) = env::var("DSH_LAUNCHER_NPM_ROOT") {
        if !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
    }
    run_capture("cmd", &["/C", "npm", "root", "-g"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[derive(Serialize, Clone)]
pub struct CheckResult {
    pub installed: bool,
    pub version: Option<String>,
    pub root: Option<String>,
    pub bin_path: Option<String>,
    pub node_ok: bool,
    pub error: Option<String>,
}

fn read_pkg_version(pkg_json: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pkg_json).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("version").and_then(|v| v.as_str()).map(String::from)
}

/// 检查本地是否已安装 DeepSeek Harness（npm 全局包）。
pub fn check() -> CheckResult {
    if node_exe().is_none() {
        return CheckResult {
            installed: false,
            version: None,
            root: None,
            bin_path: None,
            node_ok: false,
            error: Some(
                "未检测到 Node.js。DeepSeek Harness 依赖 Node.js 运行，请先安装 Node.js（https://nodejs.org/）后重试。"
                    .to_string(),
            ),
        };
    }
    let Some(root) = npm_root() else {
        return CheckResult {
            installed: false,
            version: None,
            root: None,
            bin_path: None,
            node_ok: true,
            error: Some("无法确定 npm 全局安装目录（npm root -g 执行失败），请确认 npm 已正确安装。".to_string()),
        };
    };
    // npm root -g 返回的路径本身以 node_modules 结尾；同时也兼容以
    // npm prefix -g 形态给出的根目录（不含 node_modules），两种都探测。
    let base = Path::new(&root);
    let candidates = [base.join("node_modules").join(DSH_PACKAGE), base.join(DSH_PACKAGE)];
    let Some(pkg_dir) = candidates.iter().find(|d| d.join("package.json").exists()) else {
        return CheckResult {
            installed: false,
            version: None,
            root: Some(root),
            bin_path: None,
            node_ok: true,
            error: None,
        };
    };
    CheckResult {
        installed: true,
        version: read_pkg_version(&pkg_dir.join("package.json")),
        root: Some(root),
        bin_path: Some(pkg_dir.join("lib").join("bin.js").to_string_lossy().into_owned()),
        node_ok: true,
        error: None,
    }
}

/// 执行 `npm install -g @deepseek-ai/dsh`，逐行回调输出，返回安装后的版本号。
/// 设置 DSH_LAUNCHER_NPM_ROOT 时改为安装到该目录（与检测根目录保持一致，便于测试）。
pub async fn install<F>(mut emit: F) -> Result<String, String>
where
    F: FnMut(&str, &str),
{
    let override_root = env::var("DSH_LAUNCHER_NPM_ROOT")
        .ok()
        .filter(|s| !s.is_empty());
    if let Some(root) = override_root.as_deref() {
        std::fs::create_dir_all(root).map_err(|e| format!("无法创建安装目录 {root}：{e}"))?;
        let pkg_json = Path::new(root).join("package.json");
        if !pkg_json.exists() {
            std::fs::write(&pkg_json, b"{\n  \"private\": true\n}\n")
                .map_err(|e| format!("无法写入 {pkg_json:?}：{e}"))?;
        }
    }
    let mut cmd = TokioCommand::new("cmd");
    if let Some(root) = override_root {
        cmd.args([
            "/C",
            "npm",
            "install",
            "--prefix",
            &root,
            DSH_PACKAGE,
            "--no-fund",
            "--no-audit",
            "--loglevel=notice",
        ]);
    } else {
        cmd.args([
            "/C",
            "npm",
            "install",
            "-g",
            DSH_PACKAGE,
            "--no-fund",
            "--no-audit",
            "--loglevel=notice",
        ]);
    }
    cmd.env("npm_config_update_notifier", "false")
        .env("npm_config_fund", "false")
        .env("npm_config_audit", "false")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    hide_console_tokio(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("无法启动 npm：{e}"))?;
    let stdout = child.stdout.take().ok_or("无法读取 npm 输出")?;
    let stderr = child.stderr.take().ok_or("无法读取 npm 输出")?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let tx2 = tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(("stdout".to_string(), line));
        }
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx2.send(("stderr".to_string(), line));
        }
    });

    let mut tail: VecDeque<String> = VecDeque::with_capacity(256);
    let status = loop {
        tokio::select! {
            status = child.wait() => {
                break status.map_err(|e| format!("等待 npm 结束失败：{e}"))?;
            }
            Some((stream, line)) = rx.recv() => {
                if tail.len() >= 256 {
                    tail.pop_front();
                }
                tail.push_back(format!("[{stream}] {line}"));
                emit(&stream, &line);
            }
        }
    };

    if status.success() {
        let after = check();
        after
            .version
            .ok_or_else(|| "npm 报告安装成功，但未能检测到已安装的 DeepSeek Harness。".to_string())
    } else {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "被信号终止".to_string());
        let log = tail.into_iter().collect::<Vec<_>>().join("\n");
        Err(format!("npm install 失败（退出码 {code}）：\n{log}"))
    }
}

/// 新版 DSH 未鉴权 401 响应体中的固定提示文本：证明"服务已就绪但需 token/cookie"。
pub const DSH_AUTH_BODY_MARKER: &str = "dsh web authentication required";

/// 探测结果分类（端口上的服务身份）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// 响应体含 `__DSH_BOOT__` 指纹：旧版 DSH（无鉴权层），可直接使用干净 URL。
    MarkerHit,
    /// 新版 DSH 的鉴权 401：服务已就绪，但页面内容需完成 token→cookie 握手后才有。
    AuthRequired,
    /// 端口有响应但不是 DeepSeek Harness。
    Other,
    /// 连接失败或响应超时：端口上没有可用服务。
    NoResponse,
}

/// 探测 127.0.0.1:port 上运行的 Web 服务身份。
///
/// 发送裸 `GET /`（不带 token/cookie）：
/// - 旧版 DSH 直接返回带 `__DSH_BOOT__` 指纹的 index → [`ProbeResult::MarkerHit`]；
/// - 新版 DSH 对未鉴权请求返回 401（body 含 [`DSH_AUTH_BODY_MARKER`]）→
///   [`ProbeResult::AuthRequired`]，作为"服务器已就绪"的信号；
/// - 其他响应 → [`ProbeResult::Other`]；连接失败/超时 → [`ProbeResult::NoResponse`]。
pub async fn probe_web(port: u16) -> ProbeResult {
    let addr = format!("127.0.0.1:{port}");
    let Ok(conn) = tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await else {
        return ProbeResult::NoResponse;
    };
    let Ok(mut stream) = conn else {
        return ProbeResult::NoResponse;
    };
    let req = format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if tokio::time::timeout(Duration::from_secs(2), stream.write_all(req.as_bytes()))
        .await
        .is_err()
    {
        return ProbeResult::NoResponse;
    }
    let mut buf = Vec::new();
    if tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buf))
        .await
        .is_err()
    {
        return ProbeResult::NoResponse;
    }
    let text = String::from_utf8_lossy(&buf);
    if text.contains(DSH_MARKER) {
        return ProbeResult::MarkerHit;
    }
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());
    if status == Some(401) && text.contains(DSH_AUTH_BODY_MARKER) {
        return ProbeResult::AuthRequired;
    }
    ProbeResult::Other
}

/// 从 `dsh web` 子进程的 stdout 行解析启动 URL（新版 DSH 带 token 查询参数）。
///
/// 只接受回环地址 + 默认端口的 http(s) URL，行内噪声（如 ` (LAN: ...)` 后缀）
/// 自动忽略；不匹配时返回 None——旧版本无论打印与否都不影响调用方
/// （就绪判定自然回退到指纹探测路径）。
pub fn parse_web_url_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("dsh web: ")?;
    let url = rest.split_whitespace().next()?;
    let (scheme, remainder) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = remainder.split(['/', '?', '#']).next()?;
    let (host, port) = authority.rsplit_once(':')?;
    if (host != "127.0.0.1" && host != "localhost") || port != DSH_PORT.to_string() {
        return None;
    }
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 环境变量注入互斥：避免并行测试互相污染 DSH_LAUNCHER_* 覆盖。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 环境无关的回归测试：借助 DSH_LAUNCHER_NODE / DSH_LAUNCHER_NPM_ROOT
    /// 覆盖机制，在临时目录构造一个 fake 全局安装（含 package.json 与
    /// lib/bin.js），验证 check() 能正确解析版本与入口（回归：此前曾把
    /// node_modules 拼接了两次）。不依赖本机真实 npm 全局安装，CI 可跑。
    #[test]
    fn check_detects_install_from_injected_root() {
        let _guard = ENV_LOCK.lock().unwrap();

        let fake_node = std::env::temp_dir().join("dsh-launcher-test-node.exe");
        let fake_root = std::env::temp_dir().join("dsh-launcher-test-npm-root");
        let pkg_dir = fake_root.join("node_modules").join(DSH_PACKAGE);
        std::fs::create_dir_all(pkg_dir.join("lib")).expect("创建 fake 安装目录失败");
        std::fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh","version":"9.9.9-test"}"#,
        )
        .expect("写入 fake package.json 失败");
        std::fs::write(pkg_dir.join("lib").join("bin.js"), "#!/usr/bin/env node
")
            .expect("写入 fake bin.js 失败");

        std::env::set_var("DSH_LAUNCHER_NODE", &fake_node);
        std::env::set_var("DSH_LAUNCHER_NPM_ROOT", &fake_root);

        let result = check();
        assert!(
            result.installed,
            "注入安装目录后 check() 应检测到安装：root={:?} node_ok={} error={:?}",
            result.root, result.node_ok, result.error
        );
        assert_eq!(result.version.as_deref(), Some("9.9.9-test"));
        let bin = result.bin_path.expect("bin_path 应存在");
        assert!(std::path::Path::new(&bin).exists(), "入口脚本不存在：{bin}");

        std::env::remove_var("DSH_LAUNCHER_NODE");
        std::env::remove_var("DSH_LAUNCHER_NPM_ROOT");
    }

    /// probe_web 分类回归：指纹命中 / 鉴权 401 / 其他服务 / 无响应。
    /// 用本机临时 TcpListener 回放固定响应，不依赖真实 DSH 安装，CI 可跑。
    #[tokio::test]
    async fn probe_web_classifies_serving_states() {
        async fn serve_once(response: &'static str) -> u16 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("绑定测试端口失败");
            let port = listener.local_addr().expect("读取端口失败").port();
            tokio::spawn(async move {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = [0u8; 4096];
                let _ = tokio::time::timeout(Duration::from_secs(2), sock.read(&mut buf)).await;
                let _ = tokio::time::timeout(Duration::from_secs(2), sock.write_all(response.as_bytes())).await;
            });
            port
        }

        let port = serve_once(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\n\r\n<html><script>window.__DSH_BOOT__</script></html>",
        )
        .await;
        assert_eq!(probe_web(port).await, ProbeResult::MarkerHit);

        let port = serve_once(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\n\r\ndsh web authentication required; reopen the URL printed by dsh web.\n",
        )
        .await;
        assert_eq!(probe_web(port).await, ProbeResult::AuthRequired);

        let port = serve_once("HTTP/1.1 200 OK\r\n\r\nhello from another app").await;
        assert_eq!(probe_web(port).await, ProbeResult::Other);

        // 端口上没有任何服务：绑定后立刻释放取得空闲端口，探测应判 NoResponse。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定测试端口失败");
        let port = listener.local_addr().expect("读取端口失败").port();
        drop(listener);
        assert_eq!(probe_web(port).await, ProbeResult::NoResponse);
    }

    /// dsh web: URL 行解析：新版带 token、旧版无 token、LAN 后缀、
    /// 噪声行、越界端口与非回环地址。
    #[test]
    fn parse_web_url_line_accepts_loopback_token_urls() {
        assert_eq!(
            parse_web_url_line("dsh web: http://127.0.0.1:3080/?token=abc123"),
            Some("http://127.0.0.1:3080/?token=abc123".to_string())
        );
        assert_eq!(
            parse_web_url_line("dsh web: http://localhost:3080/?token=abc"),
            Some("http://localhost:3080/?token=abc".to_string())
        );
        assert_eq!(
            parse_web_url_line("dsh web: http://127.0.0.1:3080/"),
            Some("http://127.0.0.1:3080/".to_string())
        );
        assert_eq!(
            parse_web_url_line(
                "dsh web: http://127.0.0.1:3080/?token=abc (LAN: http://192.168.1.5:3080/?token=abc)"
            ),
            Some("http://127.0.0.1:3080/?token=abc".to_string())
        );
        // 日志泵传入的是原始行（不带 [stdout] 前缀），带前缀等噪声一律拒绝。
        assert_eq!(
            parse_web_url_line("[stdout] dsh web: http://127.0.0.1:3080/?token=abc"),
            None
        );
        assert_eq!(parse_web_url_line("some other log line"), None);
        assert_eq!(parse_web_url_line("dsh web: not a url"), None);
        assert_eq!(parse_web_url_line("dsh web: http://127.0.0.1:9999/?token=abc"), None);
        assert_eq!(parse_web_url_line("dsh web: http://192.168.1.5:3080/?token=abc"), None);
    }
}
