// R9：release 不带多余的控制台窗口；debug 保留控制台以便看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod injector;
mod paths;
mod tray;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

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
            commands::apply_home_url(app.handle(), &cfg);

            let window = WebviewWindowBuilder::new(
                app,
                MAIN_WINDOW,
                WebviewUrl::External(cfg.shell.home_url.parse().expect("home url")),
            )
            .title("fnOS")
            .inner_size(cfg.shell.window.w, cfg.shell.window.h)
            .initialization_script(injector::build_init_script(&cfg))
            // spec §7「主窗口：标题跟随页面」。tauri/wry **不会**自动把 `document.title`
            // 同步到窗口标题：wry 只在 `WebviewBuilder::with_document_title_changed_handler`
            // 被注册时才转发 `DocumentTitleChanged`（wry-0.57.0/src/webview2/mod.rs:689），
            // 而 tauri 默认不注册（`webview/mod.rs:361` 里 `document_title_changed_handler: None`）。
            // 不注册的话标题恒为下面的 `"fnOS"`。
            .on_document_title_changed(|window, title| {
                let _ = window.set_title(&title);
            })
            .build()?;

            tray::apply_window_geom(&window, &cfg);
            tray::install(app.handle())?;
            // Task 8 Step 3 的合并结果：菜单状态跟随配置。`commands::set_inject_checked` 是
            // 「muda 自己会同步」的空实现，不能把配置推给菜单，故启动时直接调 `sync_menus`
            // （否则勾选态恒为 true、`打开 NAS` 恒为可点，与 config.json 不一致）。
            if let Err(e) = tray::sync_menus(app.handle(), &cfg) {
                eprintln!("[fnos] 托盘同步失败: {e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == MAIN_WINDOW {
                    api.prevent_close();
                    commands::save_window_geom(window.app_handle());
                    let _ = window.hide();
                }
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
