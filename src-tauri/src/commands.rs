//! IPC 契约（spec §8.3）与 Rust 侧内部操作。

use crate::{config::Config, injector, paths, tray, MAIN_WINDOW};
use serde::Serialize;
use serde_json::Value;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindowBuilder};

/// 设置窗 label（spec §7）：与 `capabilities/default.json` 的 `"windows": ["settings"]` 一致。
pub const SETTINGS_WINDOW: &str = "settings";

pub struct AppState {
    pub config: Mutex<Config>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub shell_version: String,
    pub mods_commit: String,
    pub mods_version: String,
    pub config_path: String,
    pub webview_version: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub schema_version: u32,
    pub mods: crate::config::ModsConfig,
    pub local: crate::config::LocalConfig,
    pub shell: crate::config::ShellConfig,
    pub meta: Meta,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetResult {
    pub config: ConfigView,
    pub needs_reload: bool,
}

pub fn load_config<R: Runtime>(app: &AppHandle<R>) -> Config {
    let path = Config::config_path();
    let cfg = Config::load(&path);
    let _ = cfg.save(&path);
    if let Some(state) = app.try_state::<AppState>() {
        *state.config.lock().unwrap() = cfg.clone();
    }
    cfg
}

fn current<R: Runtime>(app: &AppHandle<R>) -> Config {
    app.state::<AppState>().config.lock().unwrap().clone()
}

/// 组装 IPC 返回的配置视图。
///
/// 刻意**不**接收 `AppHandle`：本任务里 `meta.webviewVersion` 仍是 `None`（brief 如此，
/// 见报告「已知缺口」），`config_view` 用不到 app 句柄，带上它只会产生
/// `unused variable: app` 警告（R18 要求 0 警告）。
fn config_view(cfg: &Config) -> ConfigView {
    ConfigView {
        schema_version: cfg.schema_version,
        mods: cfg.mods.clone(),
        local: cfg.local.clone(),
        shell: cfg.shell.clone(),
        meta: Meta {
            shell_version: injector::SHELL_VERSION.into(),
            mods_commit: injector::MODS_COMMIT.into(),
            // R38：与注入载荷同源，字面量只允许在 injector.rs 里出现一次
            mods_version: injector::MODS_VERSION.into(),
            config_path: Config::config_path().display().to_string(),
            webview_version: None,
        },
    }
}

#[tauri::command]
pub fn get_config<R: Runtime>(app: AppHandle<R>) -> ConfigView {
    let cfg = current(&app);
    config_view(&cfg)
}

#[tauri::command]
pub fn set_config<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    patch: Value,
) -> Result<SetResult, String> {
    let mut cfg = {
        let mut guard = state.config.lock().unwrap();
        let mut merged: Value = serde_json::to_value(&*guard).map_err(|e| e.to_string())?;
        merge(&mut merged, patch);
        let mut next: Config = serde_json::from_value(merged).map_err(|e| e.to_string())?;
        next.normalize();
        *guard = next.clone();
        next
    };
    cfg.normalize();

    let path = Config::config_path();
    cfg.save(&path).map_err(|e| e.to_string())?;

    let needs_reload = apply_to_page(&app, &cfg);
    if let Err(e) = tray::sync_menus(&app, &cfg) {
        eprintln!("[fnos] 托盘同步失败: {e}");
    }
    Ok(SetResult {
        config: config_view(&cfg),
        needs_reload,
    })
}

/// 递归浅合并：`patch` 里的 `null` 表示删除该键（回到默认由 normalize 兜底）
fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    b.remove(&k);
                } else {
                    merge(b.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        (b, p) => *b = p,
    }
}

/// 把配置推给页面（免刷新）；返回是否需要整页重载
fn apply_to_page<R: Runtime>(app: &AppHandle<R>, cfg: &Config) -> bool {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return false;
    };
    let _ = win.eval(format!(
        "window.__FNOS_APPLY_CONFIG__ && window.__FNOS_APPLY_CONFIG__({});",
        serde_json::to_string(&serde_json::json!({
            "mods": &cfg.mods,
            "local": &cfg.local,
        }))
        .unwrap()
    ));
    false
}

#[tauri::command]
pub fn reload_main<R: Runtime>(app: AppHandle<R>, url: Option<String>) -> Result<(), String> {
    let cfg = current(&app);
    let target = url.unwrap_or_else(|| cfg.shell.home_url.clone());
    let parsed = tauri::Url::parse(&target).map_err(|e| e.to_string())?;
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        let cfg2 = current(&app);
        let _ = win.eval("window.__FNOS_SHELL__ && (window.__FNOS_SHELL__.__reload = true);");
        win.navigate(parsed).map_err(|e| e.to_string())?;
        let _ = injector::build_init_script(&cfg2);
    }
    Ok(())
}

#[tauri::command]
pub fn open_config_dir() -> Result<(), String> {
    let dir = paths::config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn reset_config<R: Runtime>(app: AppHandle<R>, scope: String) -> Result<ConfigView, String> {
    let mut cfg = current(&app);
    match scope.as_str() {
        "mods" => cfg.mods = Default::default(),
        "shell" => {
            let window = cfg.shell.window.clone();
            cfg.shell = Default::default();
            cfg.shell.window = window;
        }
        _ => {
            let window = cfg.shell.window.clone();
            cfg = Config::default();
            cfg.shell.window = window;
        }
    }
    cfg.normalize();
    cfg.save(&Config::config_path())
        .map_err(|e| e.to_string())?;
    *app.state::<AppState>().config.lock().unwrap() = cfg.clone();
    apply_to_page(&app, &cfg);
    if let Err(e) = tray::sync_menus(&app, &cfg) {
        eprintln!("[fnos] 托盘同步失败: {e}");
    }
    Ok(config_view(&cfg))
}

// ---------- Rust 内部（非 IPC） ----------

pub fn open_settings<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(SETTINGS_WINDOW) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    match WebviewWindowBuilder::new(
        app,
        SETTINGS_WINDOW,
        WebviewUrl::App("settings.html".into()),
    )
    .title("fnOS 设置")
    .inner_size(900.0, 640.0)
    .build()
    {
        Ok(_) => {}
        Err(e) => eprintln!("[fnos] 设置窗创建失败: {e}"),
    }
}

pub fn set_inject_enabled<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let mut cfg = current(app);
    cfg.shell.inject_enabled = enabled;
    let _ = cfg.save(&Config::config_path());
    *app.state::<AppState>().config.lock().unwrap() = cfg.clone();
    set_inject_checked(app, enabled);
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.eval("location.reload();");
    }
}

pub fn set_inject_checked<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let _ = (app, enabled); // muda 的 CheckMenuItem 状态随点击自动切换；外部变更由 tray::sync_menus 处理
}

pub fn open_nas<R: Runtime>(app: &AppHandle<R>) {
    let cfg = current(app);
    if cfg.shell.nas_url.trim().is_empty() {
        open_settings(app);
        return;
    }
    if let Ok(url) = cfg.shell.nas_url.parse::<tauri::Url>() {
        if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
            let _ = w.navigate(url);
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

pub fn apply_home_url<R: Runtime>(app: &AppHandle<R>, cfg: &Config) {
    let path = Config::config_path();
    let _ = cfg.save(&path);
    app.manage(AppState {
        config: Mutex::new(cfg.clone()),
    });
}

pub fn save_window_geom<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let Ok(size) = win.inner_size() else {
        return;
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    let mut cfg = current(app);
    cfg.shell.window.w = size.width as f64 / scale;
    cfg.shell.window.h = size.height as f64 / scale;
    if let Ok(pos) = win.outer_position() {
        cfg.shell.window.x = Some(pos.x as f64 / scale);
        cfg.shell.window.y = Some(pos.y as f64 / scale);
    }
    let _ = cfg.save(&Config::config_path());
    *app.state::<AppState>().config.lock().unwrap() = cfg;
}
