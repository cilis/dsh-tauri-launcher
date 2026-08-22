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

/// 探测指定端口上是否正在运行 DeepSeek Harness Web GUI。
pub async fn is_dsh_serving(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let Ok(conn) = tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await else {
        return false;
    };
    let Ok(mut stream) = conn else {
        return false;
    };
    let req = format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if tokio::time::timeout(Duration::from_secs(2), stream.write_all(req.as_bytes()))
        .await
        .is_err()
    {
        return false;
    }
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).contains(DSH_MARKER)
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
}
