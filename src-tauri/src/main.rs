// R9：release 不带多余的控制台窗口；debug 保留控制台以便看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod base64;
mod commands;
mod config;
mod injector;
mod paths;
mod report;
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
            // Task 11：`load_config` 的第二个返回值是「本次会话是否从 config.json.bak 回退」，
            // 必须经 `save_and_install_state` 带进 AppState（读配置时状态还没被 manage）。
            let (cfg, recovered_from_backup) = commands::load_config(app.handle());
            commands::save_and_install_state(app.handle(), &cfg, recovered_from_backup);

            // Finding 1：这里不再有 `cfg.shell.home_url.parse().expect("home url")`——
            // 地址经 `resolve_main_url` 逐级校验回落（覆盖 → homeUrl → 默认常量），
            // 手改出来的非法 homeUrl 再也不会变成启动即 panic（release 下 R9 隐藏控制台，
            // panic 的表现就是「双击无反应」）。
            let url = commands::resolve_main_url(&cfg, None);
            // 建窗细节（几何、注入载荷、标题跟随页面、加载日志）与重建路径共用一份定义
            commands::build_main_window(app.handle(), url, &cfg)?;

            tray::install(app.handle())?;
            // T14a：托盘菜单只剩 4 个无状态动作项（显示窗口 / 重新加载 / 系统设置 / 退出），
            // 没有勾选态也没有置灰态，原先「把配置推给菜单」的 `sync_menus` 通路随之删除
            // ——不再存在「菜单状态与 config.json 不一致」这一类缺陷。
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
            // 关于页外链：走系统默认浏览器（capability 只授权 settings 窗，见
            // capabilities/default.json；build.rs 的 AppManifest::commands 也必须同步）
            commands::open_url,
            commands::reset_config,
            // Task 11 的状态条数据源（R2：设置窗**查询**，不做页面→宿主上报）。
            // 同样只授予 settings 窗——主窗口（含内置错误页）拿不到任何命令。
            commands::get_page_state,
            // Task 13a：页面上报的读取口。上报本身走 `document.title`（不经 IPC、不需要任何
            // capability，见 src/report.rs），页面**没有任何**命令可调用；这个读命令只授设置窗。
            commands::get_page_report,
            // Task 13b：登录壁纸导入。设置窗自己写不了文件（只有这个命令有文件写入），
            // 同样只授设置窗，并且在 remote-deny.json 里显式 deny（见 build.rs 的注释）。
            commands::import_wallpaper,
            // Task 14b：设置窗托管上游 popup UI 所需的三条。
            // - get_local_store / set_local_store：上游 `chrome.storage.local` 里那些
            //   **本壳配置模型没有对应字段**的扩展本地状态（目前只有更新检查状态），落在
            //   配置目录的 local-store.json；只存字符串、有键名/单值/总量上限、不做路径解析。
            // - request_app_items：**无参数**地请主窗口页面重新汇报一次启动台应用项
            //   （上游 popup 的逐项 UI 会主动要列表，而本壳只有页面侧能发起那次请求）。
            // 三条都只授设置窗，且都在 remote-deny.json 里显式 deny。
            commands::get_local_store,
            commands::set_local_store,
            commands::request_app_items,
        ])
        .run(tauri::generate_context!())
        .expect("fnOS 启动失败");
}
