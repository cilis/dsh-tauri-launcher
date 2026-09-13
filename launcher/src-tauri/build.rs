fn main() {
    // 图标会被嵌进 exe 资源，但 tauri-build 只为 tauri.conf.json / 资源目录声明
    // rerun-if-changed，**不含 icons/**：改完图标做增量构建会沿用上一次的资源，
    // exe 仍是旧图标（实测踩过：换成品牌蓝图标后，构建产物里还是白色鲸鱼）。
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
