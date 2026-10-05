/// Task 7/8 必需：`allow-<cmd>` / `deny-<cmd>` 权限**不是**自动发现的。
///
/// `tauri_build::build()` 用的是 `AppManifest::default()`（`commands: &[]`），
/// `tauri-build-2.7.0/src/acl.rs:278` 里 `if manifest.commands.is_empty() { Vec::new() }`
/// 直接跳过 `autogenerate_command_permissions`，于是 `capabilities/default.json` 一旦引用
/// `allow-get-config` 之类就会在构建脚本阶段报
/// `Permission allow-get-config not found, expected one of core:default, ...`。
/// 因此必须在这里逐一列出 IPC 命令（与 `main.rs` 的 `invoke_handler` 逐字一致）。
/// 漏一个的表现是**构建失败**（不是运行期 ACL 拒绝）——`open_url` 就是踩过这个坑之后
/// 加进来的第 6 条，Task 11 的状态条数据源 `get_page_state` 是第 7 条，
/// Task 13a 的页面上报读取口 `get_page_report` 是第 8 条，Task 13b 的登录壁纸导入
/// `import_wallpaper` 是第 9 条（**唯一会写文件的命令**；只授设置窗、并进 remote-deny）。
/// Task 14b 再加三条（设置窗托管上游 popup UI 所需）：`get_local_store` / `set_local_store`
/// （设置窗本地状态，`local-store.json`，只存字符串且有大小上限）与 `request_app_items`
/// （**无参数**，请主窗口页面重新汇报一次启动台应用项；只授设置窗、并进 remote-deny）。
fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_config",
            "set_config",
            "reload_main",
            "open_config_dir",
            "open_url",
            "reset_config",
            "get_page_state",
            "get_page_report",
            "import_wallpaper",
            "get_local_store",
            "set_local_store",
            "request_app_items",
            // T14d：标签页栏（titlebar Webview 专用，见 capabilities/titlebar.json）
            "tab_new",
            "tab_switch",
            "tab_close",
        ]),
    ))
    .expect("failed to run tauri-build");
}
