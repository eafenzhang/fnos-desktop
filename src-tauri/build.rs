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
/// Task 13a 的页面上报读取口 `get_page_report` 是第 8 条（只授设置窗、并进 remote-deny）。
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
        ]),
    ))
    .expect("failed to run tauri-build");
}
