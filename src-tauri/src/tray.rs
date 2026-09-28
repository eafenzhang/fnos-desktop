//! 托盘图标 + 菜单（spec §7）。
//!
//! 菜单状态的**唯一真相是配置**：`sync_menus` 把 `shell.injectEnabled` / `shell.nasUrl`
//! 推给菜单项（勾选态、置灰态），而不是反过来让配置跟随菜单。

use crate::{config::Config, MAIN_WINDOW};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Runtime, WebviewWindow,
};

pub const TRAY_ID: &str = "main-tray";

/// 托盘菜单的托管句柄。
///
/// tauri 2.12 的 `TrayIcon` **没有** `menu()` 取值器（`tauri-2.12.0/src/tray/mod.rs`
/// 只暴露 `set_menu` / `set_tooltip` / `set_icon` / `set_title` / `set_visible` /
/// `set_show_menu_on_left_click` / `set_icon_as_template` / `rect` 这些 setter/getter，
/// 也没有任何「按 id 取菜单」的公开 API），所以 `install` 把自己建的那个 `Menu` 存进
/// 托管状态，`sync_menus` 再按 id 取菜单项。`Menu` / `CheckMenuItem` / `MenuItem` 都是
/// `Arc` 包一层的句柄，且 `menu/mod.rs` 的 `gen_wrappers!` 为它们 `unsafe impl Send + Sync`，
/// 因此放进托管状态是安全的；与 `TrayIconBuilder::menu(&menu)` 内部持有的克隆共享同一个
/// muda 菜单对象，改一处即改到实际显示的菜单。
pub struct TrayMenu<R: Runtime>(pub Menu<R>);

fn icon() -> tauri::Result<Image<'static>> {
    Image::from_bytes(include_bytes!("../icons/icon.png"))
}

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let inject = CheckMenuItem::with_id(app, "inject", "注入 mods", true, true, None::<&str>)?;
    let open_nas = MenuItem::with_id(app, "open_nas", "打开 NAS", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "显示 / 隐藏主窗口", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "系统设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&inject, &open_nas, &toggle, &settings, &sep, &quit])?;

    let inject_for_handler = inject.clone();
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon()?)
        .tooltip("fnOS")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "inject" => {
                let checked = inject_for_handler.is_checked().unwrap_or(true);
                crate::commands::set_inject_enabled(app, checked);
            }
            "open_nas" => crate::commands::open_nas(app),
            "toggle" => toggle_main(app),
            "settings" => crate::commands::open_settings(app),
            "quit" => {
                crate::commands::save_window_geom(app);
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    app.manage(TrayMenu(menu));
    Ok(())
}

/// 托盘菜单状态跟随配置（启动时与配置被设置窗改动时调用）。
pub fn sync_menus<R: Runtime>(app: &AppHandle<R>, cfg: &Config) -> tauri::Result<()> {
    use tauri::menu::MenuItemKind;
    let Some(state) = app.try_state::<TrayMenu<R>>() else {
        return Ok(());
    };
    let menu = &state.0;
    if let Some(MenuItemKind::Check(item)) = menu.get("inject") {
        let _ = item.set_checked(cfg.shell.inject_enabled);
    }
    if let Some(MenuItemKind::MenuItem(item)) = menu.get("open_nas") {
        // Finding 1：合法 URL 才可点（`nas_target()` 已做完整校验，与 `commands::open_nas`
        // 同源），因此不会再有「菜单可点、点了静默无事发生」。
        let _ = item.set_enabled(cfg.shell.nas_target().is_some());
    }
    Ok(())
}

pub fn toggle_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        // 最小化的窗口 `is_visible()` 仍是 true：必须先看 `is_minimized()`，
        // 否则「显示 / 隐藏」会把最小化的窗口直接藏起来，用户只能去托盘再点一次。
        if w.is_minimized().unwrap_or(false) {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            return;
        }
        match w.is_visible() {
            Ok(true) => {
                let _ = w.hide();
            }
            _ => {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }
    }
}

pub fn apply_window_geom<R: Runtime>(w: &WebviewWindow<R>, cfg: &Config) {
    let g = &cfg.shell.window;
    // 防御（消费侧）：权威夹取在 `config::WindowGeom::clamp_to_usable`（`Config::normalize`
    // 必经，Item 2），所以正常路径下这里不会看到 `w/h = 0`；保留这道判断是为了让
    //「0 尺寸的 set_size 会让窗口不可见」这条不可能再从任何来源发生。
    if g.w > 0.0 && g.h > 0.0 {
        let _ = w.set_size(tauri::LogicalSize::new(g.w, g.h));
    }
    if let (Some(x), Some(y)) = (g.x, g.y) {
        // 最小化窗口的位置是 -32000（Windows 的哨兵值），不能当成合法坐标用
        if x > -10000.0 && y > -10000.0 {
            let _ = w.set_position(tauri::LogicalPosition::new(x, y));
        }
    }
}
