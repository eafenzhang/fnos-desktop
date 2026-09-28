//! IPC 契约（spec §8.3）与 Rust 侧内部操作。

use crate::{config, config::Config, injector, paths, tray, MAIN_WINDOW};
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// 设置窗 label（spec §7）：与 `capabilities/default.json` 的 `"windows": ["settings"]` 一致。
pub const SETTINGS_WINDOW: &str = "settings";

pub struct AppState {
    pub config: Mutex<Config>,
    /// 正在「关掉旧主窗口 → 重建新主窗口」的过程中（Finding 1 / gap (a)）。
    ///
    /// 置位期间 `CloseRequested` **不得**再 `prevent_close()`：否则旧窗口永远关不掉，
    /// 而新的同 label 窗口也建不出来（tauri 的 label 注册表要等 `Destroyed` 事件才释放，
    /// 见 `manager/mod.rs:643 on_window_close` ← `app.rs:2714`）。
    pub recreating: AtomicBool,
    /// `reload_main(url)` 的一次性覆盖地址，重建时消费掉。
    pub recreate_url: Mutex<Option<String>>,
    /// 重建前主窗口是否可见；重建后原样恢复（设置窗改注入开关时，主窗口不该突然弹出/消失）。
    pub recreate_visible: AtomicBool,
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
/// 刻意**不**接收 `AppHandle`：`config_view` 用不到 app 句柄（`tauri::webview_version()`
/// 是不需要 app 的自由函数），带上它只会产生 `unused variable: app` 警告（R18 要求 0 警告）。
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
            // gap (b)：spec §8.3 要求 meta 带 WebView2 版本。`tauri::webview_version()`
            // 是 `tauri_runtime_wry::webview_version` 的 re-export
            // （`tauri-2.12.0/src/lib.rs:209` → `wry-0.57.0/src/lib.rs:2317`
            // `pub fn webview_version() -> Result<String>`），取不到时为 `None`
            //（例如 WebView2 运行时缺失），不 panic。
            webview_version: tauri::webview_version().ok(),
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
    let (cfg, needs_reload) = {
        let mut guard = state.config.lock().unwrap();
        let prev = guard.clone();
        let mut merged: Value = serde_json::to_value(&*guard).map_err(|e| e.to_string())?;
        merge(&mut merged, patch);
        let mut next: Config = serde_json::from_value(merged).map_err(|e| e.to_string())?;
        next.normalize();
        // gap (a)：**只有** `injectEnabled` / `homeUrl` 需要重建主窗口（换载荷 / 换地址），
        // 其余字段一律经 `__FNOS_APPLY_CONFIG__` 免刷新生效。
        // 判定放在 `normalize()` 之后：非法 `homeUrl` 被夹成默认值时若与旧值相同，
        // 就不该白重建一次窗口。
        let needs_reload = next.shell.inject_enabled != prev.shell.inject_enabled
            || next.shell.home_url != prev.shell.home_url;
        *guard = next.clone();
        (next, needs_reload)
    };

    let path = Config::config_path();
    cfg.save(&path).map_err(|e| e.to_string())?;

    apply_to_page(&app, &cfg);
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

/// 把 `mods` / `local` 推给页面（免刷新）。
///
/// `injectEnabled` / `homeUrl` **不在**此列：已注册到 WebView2 的
/// `AddScriptToExecuteOnDocumentCreated` 脚本无法在活窗口上替换，只能重建窗口（gap (a)）。
fn apply_to_page<R: Runtime>(app: &AppHandle<R>, cfg: &Config) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let payload = serde_json::json!({
        "mods": &cfg.mods,
        "local": &cfg.local,
    });
    match serde_json::to_string(&payload) {
        Ok(json) => {
            let _ = win.eval(format!(
                "window.__FNOS_APPLY_CONFIG__ && window.__FNOS_APPLY_CONFIG__({json});"
            ));
        }
        // `Config` 全是普通字段，序列化不可能失败；真失败也只是这一次免刷新没生效
        Err(e) => eprintln!("[fnos] 配置推送序列化失败: {e}"),
    }
}

/// 建主窗口：**首次启动与重建共用同一份定义**（地址、几何、注入载荷、标题跟随页面、加载日志）。
///
/// gap (a)：`inject_enabled=false` 时 `build_init_script` 返回空串，此时**不注册**空脚本
/// ——「关掉注入」在 WebView2 里不留任何已注册脚本，这是「取消勾选能真正停掉注入」的前提。
pub fn build_main_window<R: Runtime>(
    app: &AppHandle<R>,
    url: tauri::Url,
    cfg: &Config,
) -> tauri::Result<WebviewWindow<R>> {
    let mut builder = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::External(url))
        .title("fnOS")
        .inner_size(cfg.shell.window.w, cfg.shell.window.h)
        // spec §7「主窗口：标题跟随页面」。tauri/wry **不会**自动把 `document.title`
        // 同步到窗口标题：wry 只在 `WebviewBuilder::with_document_title_changed_handler`
        // 被注册时才转发 `DocumentTitleChanged`（wry-0.57.0/src/webview2/mod.rs:689），
        // 而 tauri 默认不注册（`webview/mod.rs:361` 里 `document_title_changed_handler: None`）。
        // 不注册的话标题恒为上面的 `"fnOS"`。
        .on_document_title_changed(|window, title| {
            let _ = window.set_title(&title);
        })
        // 每次页面加载打一行：既是现场排障手段，也是「重建确实换了一张新窗口」的证据。
        .on_page_load(|_window, payload| {
            eprintln!("[fnos] 主窗口页面已加载: {}", payload.url());
        });

    let script = injector::build_init_script(cfg);
    if !script.is_empty() {
        builder = builder.initialization_script(script);
    }

    let window = builder.build()?;
    tray::apply_window_geom(&window, cfg);
    Ok(window)
}

/// 主窗口地址：`url` 覆盖 → 配置 `homeUrl` → `DEFAULT_HOME_URL`，逐级回落。
///
/// Finding 1：用户可控文本**永不**进入 `expect`。三级都过 `config::parse_web_url`
/// （`home_url_or_default` 本身已兜底），因此只有「常量被改坏」才可能走到最后的
/// `unreachable!`——那条假设由 `config::tests::default_home_url_is_a_valid_web_url` 锁定。
pub fn resolve_main_url(cfg: &Config, url_override: Option<&str>) -> tauri::Url {
    let candidates = [
        url_override.unwrap_or(""),
        cfg.shell.home_url_or_default(),
        config::DEFAULT_HOME_URL,
    ];
    match candidates.into_iter().find_map(config::parse_web_url) {
        Some(url) => url,
        None => unreachable!("DEFAULT_HOME_URL 必须是合法的 http(s) URL"),
    }
}

/// 真正建新主窗口：消费一次性 URL 覆盖与可见性，建完后同步托盘菜单并**无论成败**清
/// `recreating`（否则一次失败会让下次关窗绕过 `closeToTray`）。
fn rebuild_main<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let cfg = current(app);
    let override_url = state.recreate_url.lock().unwrap().take();
    let url = resolve_main_url(&cfg, override_url.as_deref());
    let visible = state.recreate_visible.load(Ordering::SeqCst);

    let built = build_main_window(app, url, &cfg).map_err(|e| e.to_string());
    state.recreating.store(false, Ordering::SeqCst);
    if let Err(e) = tray::sync_menus(app, &cfg) {
        eprintln!("[fnos] 托盘同步失败: {e}");
    }
    let window = built?;
    if !visible {
        let _ = window.hide();
    }
    Ok(())
}

/// 重建主窗口，让 `injectEnabled` / `homeUrl` 立刻生效（gap (a)）。
///
/// 为什么必须换窗口：`initialization_script` 走的是 WebView2 的
/// `AddScriptToExecuteOnDocumentCreated`，同一次导航不会重新注册，窗口活着的期间**无法**
/// 替换；`__FNOS_APPLY_CONFIG__` 只能推 `mods` / `local`。round 0 的
/// `location.reload()` 因此永远关不掉注入。
///
/// 时序：置 `recreating` → 记下可见性 + 存几何 → `close()` 旧窗口 → 关闭请求这次不再被拦
/// → `Destroyed` 回调（`on_main_destroyed`）里建同 label 新窗口 → 清标志 → `sync_menus`。
/// **不能**在 `close()` 之后立刻建：label 注册表在 `Destroyed` 才释放，否则
/// `WindowLabelAlreadyExists`。
pub fn recreate_main_window<R: Runtime>(
    app: &AppHandle<R>,
    url: Option<String>,
) -> Result<(), String> {
    // 先落 URL 覆盖：即便此刻已有一轮重建在飞，这一轮的 `Destroyed` 也会用上它
    if let Some(raw) = url {
        let parsed =
            config::parse_web_url(&raw).ok_or_else(|| format!("非法 URL，拒绝导航：{raw}"))?;
        *app.state::<AppState>().recreate_url.lock().unwrap() = Some(parsed.to_string());
    }

    let state = app.state::<AppState>();
    if state.recreating.swap(true, Ordering::SeqCst) {
        return Ok(()); // 已在重建中：交给那一轮的 `Destroyed` 收尾
    }

    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        // 没有主窗口（closeToTray=false 关掉之后、或上一轮重建失败）：直接建一个
        state.recreate_visible.store(true, Ordering::SeqCst);
        return rebuild_main(app);
    };

    state
        .recreate_visible
        .store(win.is_visible().unwrap_or(true), Ordering::SeqCst);
    save_window_geom(app); // 重建后按老几何摆回原位

    if let Err(e) = win.close() {
        // 关不掉就别留着标志，否则下次真的关窗会绕过 closeToTray
        state.recreating.store(false, Ordering::SeqCst);
        return Err(e.to_string());
    }
    Ok(())
}

/// `main` 窗口的 `Destroyed` 回调（`main.rs` 转发）。
///
/// 只有「重建流程」才会走到建新窗口这一步；用户真的关闭（`closeToTray=false`）时
/// `recreating` 是 false，直接返回、让进程按 tauri 的默认行为退出。
pub fn on_main_destroyed<R: Runtime>(app: &AppHandle<R>) {
    if !app.state::<AppState>().recreating.load(Ordering::SeqCst) {
        return;
    }
    if let Err(e) = rebuild_main(app) {
        eprintln!("[fnos] 主窗口重建失败: {e}");
    }
}

/// 托盘 `CloseRequested` 放行判据：重建中必须允许关闭旧窗口。
pub fn is_recreating<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<AppState>().recreating.load(Ordering::SeqCst)
}

/// `shell.closeToTray`（gap (c)）：默认 `true` = D5 的「关窗隐藏」；`false` = 真的关闭
/// 主窗口（最后一个窗口关闭时 tauri 退出进程）。
pub fn close_to_tray<R: Runtime>(app: &AppHandle<R>) -> bool {
    current(app).shell.close_to_tray
}

#[tauri::command]
pub fn reload_main<R: Runtime>(app: AppHandle<R>, url: Option<String>) -> Result<(), String> {
    // 保持 round 0 的 `Err(String)` 语义：非法 URL 直接报错，不去动窗口。
    // 合法时**真正重建**主窗口（gap (a)），于是设置窗既有的
    // `if (res.needsReload) reloadMain(null)` 流程能拿到一份新载荷。
    recreate_main_window(&app, url)
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
    let prev_shell = cfg.shell.clone();
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
    // 与 `set_config` 同一判据：reset 也可能把 `injectEnabled` / `homeUrl` 拉回默认值
    // （round 0 只在这里补了 `sync_menus`），同样必须重建主窗口才算生效。
    if cfg.shell.inject_enabled != prev_shell.inject_enabled
        || cfg.shell.home_url != prev_shell.home_url
    {
        if let Err(e) = recreate_main_window(&app, None) {
            eprintln!("[fnos] 重置后重建主窗口失败: {e}");
        }
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

/// 托盘「注入 mods」勾选项（gap (a)）。
///
/// round 0 是 `location.reload()`：重载后跑的仍是建窗时注册的那份初始化脚本，取消勾选
/// 关不掉注入。现在改成**重建主窗口**——新窗口只注册 `build_init_script`（关闭时为空串），
/// 旧窗口连同它的注入脚本一起被销毁。
pub fn set_inject_enabled<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let mut cfg = current(app);
    cfg.shell.inject_enabled = enabled;
    cfg.normalize();
    let _ = cfg.save(&Config::config_path());
    *app.state::<AppState>().config.lock().unwrap() = cfg.clone();
    if let Err(e) = tray::sync_menus(app, &cfg) {
        eprintln!("[fnos] 托盘同步失败: {e}");
    }
    if let Err(e) = recreate_main_window(app, None) {
        eprintln!("[fnos] 注入开关重建主窗口失败: {e}");
    }
}

pub fn open_nas<R: Runtime>(app: &AppHandle<R>) {
    let cfg = current(app);
    // Finding 1：`nas_target()` 是唯一判据（与托盘置灰同源）。未配置**或填错**都去设置窗，
    // 不再像 round 0 那样对非法 URL 静默什么都不做。
    let Some(url) = cfg.shell.nas_url_parsed() else {
        eprintln!("[fnos] nasUrl 未配置或非法，改为打开设置窗");
        open_settings(app);
        return;
    };
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.navigate(url);
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 挂载托管状态并落盘（首启调用一次）。
///
/// round 0 里叫 `apply_home_url`，但名字与行为不符——它不「应用」任何地址，只是
/// **保存配置 + 安装 `AppState`**，故改名。（建窗地址由 `resolve_main_url` 决定。）
pub fn save_and_install_state<R: Runtime>(app: &AppHandle<R>, cfg: &Config) {
    let path = Config::config_path();
    let _ = cfg.save(&path);
    app.manage(AppState {
        config: Mutex::new(cfg.clone()),
        recreating: AtomicBool::new(false),
        recreate_url: Mutex::new(None),
        recreate_visible: AtomicBool::new(true),
    });
}

pub fn save_window_geom<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    // 最小化时**不能**保存几何：Windows 对最小化窗口报 `inner_size = 0x0`、
    // `outer_position = -32000,-32000`，存下去就把下一次启动的窗口放到屏幕外
    //（实测：最小化后 WM_CLOSE → config 里留下 w=0/h=0/x=-32000/y=-32000）。
    // 跳过保存即保留上一次的合法几何。这是本轮验证 `toggle_main` 的最小化分支时发现的
    // 附带缺陷（见 task-7-8-fix1-report.md）。
    if win.is_minimized().unwrap_or(false) {
        return;
    }
    let Ok(size) = win.inner_size() else {
        return;
    };
    if size.width == 0 || size.height == 0 {
        return;
    }
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
