// R9：release 不带多余的控制台窗口；debug 保留控制台以便看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod injector;
mod paths;
mod tray;

// `Manager` 提供 `get_webview_window`（单实例回调）与 `app_handle`（关闭回调）；
// 建窗用的 `WebviewWindowBuilder` 已集中到 `commands::build_main_window`（首启/重建共用）。
use tauri::Manager;

/// 主窗口 label（spec §7）：main.rs / tray.rs / commands.rs 共用，避免字面量散落。
pub const MAIN_WINDOW: &str = "main";

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            let cfg = commands::load_config(app.handle());
            commands::save_and_install_state(app.handle(), &cfg);

            // Finding 1：这里不再有 `cfg.shell.home_url.parse().expect("home url")`——
            // 地址经 `resolve_main_url` 逐级校验回落（覆盖 → homeUrl → 默认常量），
            // 手改出来的非法 homeUrl 再也不会变成启动即 panic（release 下 R9 隐藏控制台，
            // panic 的表现就是「双击无反应」）。
            let url = commands::resolve_main_url(&cfg, None);
            // 建窗细节（几何、注入载荷、标题跟随页面、加载日志）与重建路径共用一份定义
            commands::build_main_window(app.handle(), url, &cfg)?;

            tray::install(app.handle())?;
            // Task 8 Step 3 的合并结果：菜单状态跟随配置。`sync_menus` 是唯一机制
            //（否则勾选态恒为 true、`打开 NAS` 恒为可点，与 config.json 不一致）。
            if let Err(e) = tray::sync_menus(app.handle(), &cfg) {
                eprintln!("[fnos] 托盘同步失败: {e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != MAIN_WINDOW {
                return;
            }
            let app = window.app_handle();
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // 重建流程必须能真的关掉旧窗口，否则同 label 的新窗口建不出来
                    if commands::is_recreating(app) {
                        commands::save_window_geom(app);
                        return; // 不 prevent_close → 窗口被销毁，`Destroyed` 里再建新的
                    }
                    commands::save_window_geom(app);
                    // gap (c)：`shell.closeToTray`（默认 true = D5 的关窗隐藏）。
                    // false 时不拦这次关闭 → 主窗口真的关闭；它是最后一个窗口时
                    // tauri 发出 `ExitRequested` 并结束进程。
                    if commands::close_to_tray(app) {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                // gap (a)：tauri 的 label 注册表在 `Destroyed` 才释放，新窗口必须在这里建
                tauri::WindowEvent::Destroyed => commands::on_main_destroyed(app),
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::reload_main,
            commands::open_config_dir,
            commands::reset_config,
        ])
        .run(tauri::generate_context!())
        .expect("fnOS 启动失败");
}
