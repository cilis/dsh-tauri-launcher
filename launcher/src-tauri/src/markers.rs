//! 标记文件协议（与 Web 设置插件协作，文件位于启动器 exe 同目录）：
//! - `.dsh-heartbeat`：每秒写入 Unix 时间戳，供沙箱内的设置插件读取以判断进程存活；
//! - `.dsh-quit`：内容 `1` 且 60 秒内新鲜 → 仅退出桌面应用本身（消费时自删），
//!   插件用重写内容（`0`）取消退出请求，避免依赖外部命令删除文件。
//!
//! 协议细节（新鲜窗口、超时值）见 `docs/architecture.md`。

use std::time::Duration;

use tauri::AppHandle;

use crate::shutdown;

const HEARTBEAT_MARKER: &str = ".dsh-heartbeat";
const QUIT_MARKER: &str = ".dsh-quit";

/// 启动标记文件轮询任务：每秒写心跳并检查退出标记。
/// 退出标记消费延迟 ≤1 秒；插件侧的心跳“新鲜窗口”须与之匹配。
pub(crate) fn spawn_marker_task(handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let Ok(exe) = std::env::current_exe() else {
                continue;
            };
            let Some(dir) = exe.parent() else {
                continue;
            };
            // 心跳：写入当前 Unix 时间戳，供沙箱内的设置插件读取以判断进程存活。
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = std::fs::write(dir.join(HEARTBEAT_MARKER), stamp.to_string());
            if dir.join(QUIT_MARKER).exists() {
                let quit_path = dir.join(QUIT_MARKER);
                // 仅当内容为 "1" 且 60 秒内新鲜才退出；插件用重写内容（"0"）
                // 取消退出请求，避免依赖外部命令删除文件。
                let requested = std::fs::read_to_string(&quit_path)
                    .map(|s| s.trim() == "1")
                    .unwrap_or(false);
                let fresh = std::fs::metadata(&quit_path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .map(|age| age < Duration::from_secs(60))
                    .unwrap_or(false);
                if requested && fresh {
                    let _ = std::fs::remove_file(&quit_path);
                    shutdown::quit_via_marker(&handle).await;
                    break;
                }
            }
        }
    });
}
