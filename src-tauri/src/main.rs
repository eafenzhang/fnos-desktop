// R9：release 不带多余的控制台窗口；debug 保留控制台以便看日志
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod base64;
mod commands;
mod config;
mod injector;
// P0-1：release 下没有控制台（R9），所有 `[fnos]` 日志经 `logfile::flog!` 同时落盘到
// `config_dir()/app.log`。此前安装版的所有日志都掉进无底洞，白屏/加载失败/更新失败
// 这些现场没有任何可回传的文本。
mod logfile;
mod paths;
mod report;
mod tray;
mod updater;

// `Manager` 提供 `get_window`（单实例回调）与 `app_handle`（关闭回调）；
// 建窗用的 `WindowBuilder` / `WebviewBuilder` 已集中到 `commands::build_main_window`
//（首启/重建共用；T14d 起主窗口 = 无边框 Window + 标签页栏/标签页多 Webview）。
use tauri::Manager;

/// 主窗口 label（spec §7）：main.rs / tray.rs / commands.rs 共用，避免字面量散落。
pub const MAIN_WINDOW: &str = "main";

fn main() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_window(MAIN_WINDOW) {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            // 上次全屏中被强杀时任务栏可能仍被藏着：按 marker 恢复，避免没有任务栏。
            commands::restore_taskbar_on_startup();
            // Task 11：`load_config` 的第二个返回值是加载报告（是否从 .bak 回退、是否
            // 读取失败），必须经 `save_and_install_state` 带进 AppState——读配置时状态
            // 还没被 manage，且「读失败」要阻止默认值覆盖磁盘。
            let (cfg, load_report) = commands::load_config(app.handle());
            commands::save_and_install_state(app.handle(), &cfg, load_report);

            // Finding 1：这里不再有 `cfg.shell.home_url.parse().expect("home url")`——
            // 地址经 `resolve_main_url` 逐级校验回落（覆盖 → nasUrl → homeUrl → 默认常量），
            // 手改出来的非法 homeUrl 再也不会变成启动即 panic（release 下 R9 隐藏控制台，
            // panic 的表现就是「双击无反应」）。
            let url = commands::resolve_main_url(&cfg, None);
            // 建窗细节（几何、注入载荷、标题跟随页面、加载日志）与重建路径共用一份定义
            commands::build_main_window(app.handle(), url, &cfg)?;

            tray::install(app.handle())?;
            // T14a：托盘菜单只剩 4 个无状态动作项（显示窗口 / 重新加载 / 系统设置 / 退出），
            // 没有勾选态也没有置灰态，原先「把配置推给菜单」的 `sync_menus` 通路随之删除
            // ——不再存在「菜单状态与 config.json 不一致」这一类缺陷。
            //
            // 仅调试构建：`FNOS_OPEN_SETTINGS=1` 时启动即打开设置窗，便于用 WebView2 CDP
            // 对设置页做自动化验证（发布版没有这段——`debug_assertions` 在 release 下为假）。
            #[cfg(debug_assertions)]
            if std::env::var("FNOS_OPEN_SETTINGS").as_deref() == Ok("1") {
                commands::open_settings(app.handle());
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
                        // 收进托盘前退出全屏：否则任务栏被藏起来、窗口又不可见，用户只能
                        // 靠托盘「显示窗口」找回一个仍然全屏的窗口。
                        commands::leave_fullscreen(app);
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                // gap (a)：tauri 的 label 注册表在 `Destroyed` 才释放，新窗口必须在这里建
                tauri::WindowEvent::Destroyed => {
                    commands::stop_fullscreen_watcher();
                    commands::restore_taskbar();
                    commands::on_main_destroyed(app)
                }
                // T14d：子 Webview 不会随窗口自动重排——最大化 / 还原 / 拖边缘都要重摆。
                // 全屏状态被外部改回普通态（toggle_maximize / Win+Up）时也要把标题栏、
                // Dock、Windows 任务栏拉回来。
                tauri::WindowEvent::Resized(size) => {
                    commands::reconcile_fullscreen(window);
                    commands::relayout_main_window(window, *size)
                }
                // 跨不同缩放比的显示器拖动（100% ⇄ 150%）：tao 发的是 ScaleFactorChanged，
                // **不保证**再补一条 Resized。子 Webview 的 bounds 是按**物理**像素算的，
                // 只处理 Resized 会让 32px 标题栏在换屏后残留旧缩放的条高（表现为标题栏
                // 与内容之间一道缝或盖住内容 1~2px）。这里复用同一个 relayout。
                tauri::WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                    commands::relayout_main_window(window, *new_inner_size)
                }
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
            // T14g（用户要求：检查更新移进设置页 + 实时下载进度）：三条只授设置窗。
            // - get_update_state：轮询读状态（只读，无副作用）；
            // - start_update_check：开一轮检查/下载（重入闸门 + spawn_blocking）；
            // - install_update：安装状态里已下载好的包（路径由宿主写入并二次校验）。
            commands::get_update_state,
            commands::start_update_check,
            commands::install_update,
            // T14d：标签页栏（titlebar Webview）专用的三个命令。
            commands::tab_new,
            commands::tab_switch,
            commands::tab_close,
            // 全屏按钮（titlebar）与 Esc 退出全屏共用；只授 titlebar。
            commands::set_fullscreen,
        ])
        .run(tauri::generate_context!());
    // 进程正常结束（托盘退出 / 最后一个窗口关闭）时兜底还原任务栏；幂等，没藏过就什么都不做。
    commands::restore_taskbar();
    result.expect("fnOS 启动失败");
}
