// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 外壳静态服务先于 tauri 建窗口启动：主窗口 URL 指向 http://127.0.0.1:3081。
    dsh_launcher_lib::start_shell_server();
    dsh_launcher_lib::run()
}
