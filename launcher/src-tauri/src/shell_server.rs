//! 桌面外壳（标题栏 + DSH iframe 宿主）的本地静态服务。
//!
//! 为什么外壳不再由 tauri 资产协议（`http://tauri.localhost`）承载：
//! 新版 DSH 的浏览器会话 cookie 带 `SameSite=Strict`（见
//! `dsh-client-connection` 的 `sessionCookie`），而 SameSite 比较「站点」时
//! **忽略端口、只比较主机**。外壳若在 `tauri.localhost`，与 DSH 的
//! `127.0.0.1:3080` 属跨站，WebView2 会拒收 token 握手 303 响应里的
//! `Set-Cookie`，iframe 永远停在 401 白页。把外壳改到 `127.0.0.1:3081`
//! 后二者同站（同主机、不同端口不影响 SameSite），握手与后续请求的
//! cookie 均正常，iframe 可以正常加载 DSH GUI。
//!
//! 因此主窗口的 URL 指向本服务；服务必须在 tauri 建窗口之前启动
//! （见 `main.rs`）。settings/exiting 两个辅助窗口仍走 tauri 资产协议
//! （它们不涉及 DSH cookie）。

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 外壳服务监听端口：与 DSH 的 3080 同主机、不同端口 → SameSite 视为同站。
pub const SHELL_PORT: u16 = 3081;

/// 外壳页 CSP：脚本/样式仅限自身，iframe 仅放行本机 DSH；
/// `connect-src` 放行 tauri IPC（Windows 走 postMessage，保留 ipc: 兜底）。
const SHELL_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost; frame-src http://127.0.0.1:3080";

/// 外壳静态资源：编译期嵌入，运行时无需读盘。
const INDEX_HTML: &str = include_str!("../../ui/index.html");
const MAIN_JS: &str = include_str!("../../ui/main.js");
const STYLES_CSS: &str = include_str!("../../ui/styles.css");

/// 路由：请求方法 + 路径 → (状态码, content-type, 响应体)。
/// 只服务外壳自身需要的三个文件，其余一律 404（最小攻击面）。
fn route(method: &str, path: &str) -> (u16, &'static str, &'static str) {
    if method != "GET" && method != "HEAD" {
        return (405, "text/plain; charset=utf-8", "method not allowed");
    }
    match path {
        "/" | "/index.html" => (200, "text/html; charset=utf-8", INDEX_HTML),
        "/main.js" => (200, "text/javascript; charset=utf-8", MAIN_JS),
        "/styles.css" => (200, "text/css; charset=utf-8", STYLES_CSS),
        _ => (404, "text/plain; charset=utf-8", "not found"),
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    }
}

/// 处理一条连接：只读请求头（上限 8 KiB），写响应后关闭。
async fn handle(mut stream: TcpStream) {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let Ok(read) = stream.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= 8192 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut parts = text.lines().next().unwrap_or_default().split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or("/");
    let (status, content_type, body) = route(method, path);
    let body_bytes = body.as_bytes();
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\ncache-control: no-store\r\ncontent-security-policy: {SHELL_CSP}\r\nconnection: close\r\n\r\n",
        body_bytes.len(),
        reason = reason_phrase(status)
    );
    let _ = stream.write_all(head.as_bytes()).await;
    if method != "HEAD" {
        let _ = stream.write_all(body_bytes).await;
    }
    let _ = stream.shutdown().await;
}

/// 服务循环：每条连接一个 task；accept 失败即返回（由调用方记录日志）。
pub async fn serve(listener: TcpListener) -> io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(handle(stream));
    }
}

/// 启动外壳静态服务（必须在 tauri 建窗口之前调用，否则主窗口加载失败）。
///
/// 端口在**当前线程同步绑定**后才交给异步循环：tauri 建窗口与 WebView 首次
/// 加载紧随 `run()` 之后，若监听尚未就绪，主窗口会直接命中
/// `ERR_CONNECTION_REFUSED` 且不会自动重试（实测表现为白窗）。
/// 绑定失败时仅打印错误：表现为空白窗口，日志给出原因，不阻断进程启动。
pub fn start() {
    let listener = match std::net::TcpListener::bind(("127.0.0.1", SHELL_PORT)) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("[launcher] 外壳服务无法绑定 127.0.0.1:{SHELL_PORT}：{e}");
            return;
        }
    };
    if let Err(e) = listener.set_nonblocking(true) {
        eprintln!("[launcher] 外壳服务设置非阻塞失败：{e}");
        return;
    }
    let _ = std::thread::Builder::new()
        .name("dsh-shell-server".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(e) => {
                    eprintln!("[launcher] 外壳服务运行时创建失败：{e}");
                    return;
                }
            };
            runtime.block_on(async move {
                match TcpListener::from_std(listener) {
                    Ok(listener) => {
                        if let Err(e) = serve(listener).await {
                            eprintln!("[launcher] 外壳服务异常退出：{e}");
                        }
                    }
                    Err(e) => eprintln!("[launcher] 外壳服务接管监听失败：{e}"),
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 路由回归：三个外壳资源可服务、未知路径 404、非 GET/HEAD 405。
    #[test]
    fn route_serves_shell_assets_only() {
        let (status, content_type, body) = route("GET", "/");
        assert_eq!(status, 200);
        assert!(content_type.starts_with("text/html"));
        assert!(body.contains("dsh-frame"), "外壳页应包含 DSH iframe 宿主");

        assert_eq!(route("GET", "/index.html").0, 200);
        assert_eq!(route("GET", "/main.js").1, "text/javascript; charset=utf-8");
        assert_eq!(route("GET", "/styles.css").1, "text/css; charset=utf-8");
        assert_eq!(route("GET", "/../secret").0, 404);
        assert_eq!(route("POST", "/").0, 405);
        assert_eq!(route("HEAD", "/").0, 200);
    }

    /// 端到端回归：真实 TCP 请求外壳页，校验状态行、CSP 与正文长度。
    #[tokio::test]
    async fn serve_answers_shell_page_over_tcp() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("绑定测试端口失败");
        let addr = listener.local_addr().expect("读取端口失败");
        tokio::spawn(async move {
            let _ = serve(listener).await;
        });

        let mut stream = TcpStream::connect(addr).await.expect("连接测试服务失败");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .await
            .expect("写请求失败");
        let mut out = Vec::new();
        stream.read_to_end(&mut out).await.expect("读响应失败");
        let text = String::from_utf8_lossy(&out);

        assert!(text.starts_with("HTTP/1.1 200 OK"), "响应异常：{text}");
        assert!(text.contains("content-security-policy:"), "缺少 CSP 头");
        assert!(text.contains("cache-control: no-store"), "缺少 no-store");
        assert!(text.contains("dsh-frame"), "缺少外壳标记");
    }
}
