//! 托盘图标 + 菜单（spec §7）。
//!
//! **T14a 起的菜单是 4 个动作项 + 1 条分隔线**，全部是无状态的瞬时动作：
//!
//! ```text
//! 显示窗口     → 显示 + 前置主窗口（最小化则先还原；不再是显示/隐藏开关）
//! 重新加载     → 以当前配置重建主窗口
//! 系统设置     → 打开设置窗
//! ──────────
//! 退出         → 退出
//! ```
//!
//! 于是 `sync_menus` 这条「把配置推给菜单」的通路**整体消失**了：勾选项「注入 mods」
//! 移出托盘（该开关只在设置窗，经 `set_config` 生效），「打开 NAS」连同它的 `nasUrl`
//! 置灰逻辑一并移除。没有动态状态的菜单不需要同步，也就没有「菜单状态与 config.json
//! 不一致」这一类缺陷可谈。

use crate::{config::Config, MAIN_WINDOW};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Runtime, WebviewWindow,
};

pub const TRAY_ID: &str = "main-tray";

/// 托盘图标 = 上游 fnOS 品牌图标的 32×32 那一枚（T14a）。
///
/// 取代原先内存手绘的 32×32 RGBA 占位图。`include_bytes!` 的是
/// `tools/vendor-mods.ps1` 从 `.ref/fnOS_UI_Mods/icons/icon32.png` 逐字节拷进来的
/// vendored 资产（provenance 记在 `assets/fnos-mods/NOTICE` 的 SHA-256 表里），
/// 不是再手写一份二进制。32×32 正是 Windows 托盘通知区域的需求尺寸，
/// 因此这里不需要 `tauri.conf.json` 的 `bundle.icon` 那套多尺寸资源。
fn icon() -> tauri::Result<Image<'static>> {
    Image::from_bytes(include_bytes!("../assets/fnos-mods/icons/icon32.png"))
}

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
    // Task 11（spec §12.3：「主窗口加载失败/离线 → 内置错误页 + 重试」）：错误页本身没有
    // 任何 IPC 授权，重试入口必须由宿主提供——这里是其中之一（另一个是设置窗状态条的「重试」）。
    let reload = MenuItem::with_id(app, "reload", "重新加载", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "系统设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &reload, &settings, &sep, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon()?)
        .tooltip("fnOS")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "reload" => crate::commands::reload_main_window(app),
            "settings" => crate::commands::open_settings(app),
            "quit" => {
                crate::commands::save_window_geom(app);
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// 菜单「显示窗口」：显示 + `set_focus`，最小化的先还原。
///
/// **不再隐藏**（T14a 之前是「显示 / 隐藏主窗口」开关，`toggle_main` 已随本次改动删除）。
/// 为什么必须先看 `is_minimized()`：最小化的窗口 `is_visible()` 仍是 true，只调
/// `show()` + `set_focus()` 不会把它从任务栏还原出来——用户点了「显示窗口」却什么也
/// 看不到。三步都对结果有贡献：`unminimize` 还原、`show` 兜住「窗口被 hide 到托盘」的
/// 情形（`closeToTray=true` 时关窗走的就是 hide）、`set_focus` 才真正把它带到前台。
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// 把配置里的窗口几何（尺寸 + 位置）套到刚建好的主窗口上。
///
/// 留在 `tray.rs` 是历史位置（与「窗口」相关的窗口操作集中在这两个函数里），由
/// `commands::build_main_window_with` 在建窗时调用。
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
