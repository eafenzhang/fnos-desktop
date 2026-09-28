//! IPC 契约（spec §8.3）与 Rust 侧内部操作。

use crate::{config, config::Config, injector, paths, report, tray, MAIN_WINDOW};
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// 设置窗 label（spec §7）：与 `capabilities/default.json` 的 `"windows": ["settings"]` 一致。
pub const SETTINGS_WINDOW: &str = "settings";

/// 内置错误页的资产名（相对 `tauri.conf.json` 的 `frontendDist` = `../ui/settings`）。
///
/// brief 写的是 `src-tauri/src/error_page.html`，但 `WebviewUrl::App` 只能解析**应用资产**，
/// 编译进二进制旁边的 `.html` 根本不会被 App 协议服务；同理 `ui/error.html`（brief 里
/// 「inside the configured frontendDist」的另一种读法）也不在 `ui/settings/` 之下，
/// 会 404。故落在 `ui/settings/error.html`——它同时满足「是 ui 下的页面」（不是 Rust 源码
/// 目录里的死文件）与「在 frontendDist 内」（能被服务）。
pub const ERROR_PAGE_ASSET: &str = "error.html";

/// 主窗口一次加载的看门狗超时（spec §12.3：`on_page_load` 迟迟不来 → 错误页）。
const LOAD_TIMEOUT_SECS: u64 = 20;

/// 错误页的自动重试退避：15s → 30s → 60s → 120s，第 5 次失败起不再自动重试。
///
/// 是**唯一**的退避事实来源：错误页正文里的「退避：15s → 30s → 60s → 120s」也由它生成
/// （`ErrorInfo::retry_schedule`），页面不写死这份计划，免得两处漂移。
const AUTO_RETRY_DELAYS_MS: [u64; 4] = [15_000, 30_000, 60_000, 120_000];

/// 页面自检探针回传用的 `document.title` 前缀。
///
/// **必须是可打印 ASCII**：WebView2 在把 `document.title` 送到宿主之前会处理掉控制字符
/// （实测：`\u0001FNOSPROBE\u0001…` 到达宿主时控制字符已经不见，`strip_prefix` 静默失配，
/// 失败被当成了一个普通标题）。改用「真实网页标题不会以它开头」的可打印哨兵。
pub const PROBE_TITLE_PREFIX: &str = "FNOSPROBE:";

/// 注入到主窗口页面里的自检探针：回答「你是不是 Chromium 的网络错误页」。
///
/// 为什么需要它（Task 11 的实测结论，见报告）：wry 把 `PageLoadEvent::Finished` 直接挂在
/// WebView2 的 `NavigationCompleted` 上，却**丢掉了 `IsSuccess`**
/// （`wry-0.57.0/src/webview2/mod.rs:726-737` 只取 URL），所以「连接被拒绝 / 端口被拦」这类
/// **快速失败**同样会发 `Finished`；而 `PageLoadPayload::url()` 用的是
/// `ICoreWebView2::Source`，导航失败后它**仍然是请求的那个地址**
/// （实测：`http://127.0.0.1:1/` 失败后上报的就是它自己，而不是 `chrome-error://`）。
/// 所以「靠 URL 认错误页」只是一条廉价短路，真正干活的判定必须来自页面内部。
///
/// 而 20s 看门狗（spec 的「Finished 迟迟不来」）只能抓住「卡住不返回」，抓不住「立刻失败」。
/// 于是补一条页面自检：Chromium 网络错误页有稳定的 `#main-frame-error` 或
/// `<body class="neterror">`，把结论塞进 `document.title`，宿主再通过已有的
/// `on_document_title_changed` 通道读回来（这是主窗口唯一一条「页面 → 宿主」的既有通路，
/// 且不需要给主窗口任何 IPC 授权）。
///
/// 判定为「正常页面」时把标题还原（延迟 800ms 是为了让 WebView2 至少上报过一次——
/// 同一帧内两次改动会被合并成最后一次）；判定为错误页时**不还原**：宿主会拦下这个标题
/// 并换成自己的窗口标题，页面本身也没有值得保留的标题。
pub const LOAD_PROBE_JS: &str = concat!(
    "(function(){try{var d=document,ok=false;",
    "try{ok=!!d.getElementById('main-frame-error')",
    "||!!(d.body&&/(^|\\s)neterror(\\s|$)/.test(d.body.className||''));}catch(e){}",
    "var m='';try{var el=d.getElementById('main-message')||d.getElementById('main-content');",
    "if(el)m=String(el.textContent||'');}catch(e){}",
    "if(!m){try{m=String((d.body&&d.body.innerText)||'');}catch(e){}}",
    "m=m.replace(/\\s+/g,' ').slice(0,160);",
    "var t0=String(d.title||'');",
    "d.title='FNOSPROBE:'+JSON.stringify({err:ok,detail:m});",
    "if(!ok)setTimeout(function(){try{",
    "if(String(d.title).indexOf('FNOSPROBE:')===0)d.title=t0;}catch(e){}},800);",
    "}catch(e){}})();"
);

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
    /// 重建前主窗口的显示态，重建后原样恢复（Item 3）。整组一次性读取，
    /// 避免三个独立原子量在中途被改得不一致。
    pub recreate_display: Mutex<DisplayState>,
    /// 主窗口加载观测（Task 11：状态条 + 错误页）。
    pub load: Mutex<LoadState>,
    /// 下一次重建要建成**内置错误页**（`Some` = 错误页 + 空初始化脚本）。
    ///
    /// 与 `recreate_url` 同一套一次性语义：`rebuild_main` 取走即清。
    pub recreate_error: Mutex<Option<ErrorInfo>>,
    /// 本次会话是否从 `config.json.bak` 恢复过（`get_config` 的 `meta.recoveredFromBackup`）。
    /// 是**会话级**状态，不随 `set_config` / `reset_config` 复位——用户要的是「这次启动发生过
    /// 配置损坏」这个事实，而不是「当前这份配置是否完好」。
    pub recovered_from_backup: AtomicBool,
    /// 最近一次**通过校验**的页面上报（Task 13a；见 [`crate::report`]）。
    ///
    /// **只在内存里**：不落盘、不进 `Config`、不给页面回执。它是「页面声称注入链已触发」的
    /// 证据，不是任何权限的来源。每次新建/重建主窗口、切错误页、以及新文档与上报不同源时
    /// 都会清空（见 [`begin_load`] / [`set_error_state`] / [`on_page_event`]）——旧页面的
    /// 上报绝不能拿来描述新页面。
    pub page_report: Mutex<Option<report::ReportEntry>>,
}

/// 主窗口一次加载所处的阶段（Task 11）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPhase {
    /// 已建窗，还没收到 `PageLoadEvent::Finished`（看门狗在跑）。
    Loading,
    /// 收到了 `Finished`，且没有任何失败信号。
    Loaded,
    /// 已判定失败（错误页就是这一态的表现）。
    Failed,
}

/// 主窗口当前这一次加载的观测状态。
///
/// 一次「加载」= 一次建窗（含重建 / 自动重试）。`generation` 每次建窗自增，任何在飞的
/// 看门狗线程与页面探针都用它做归属校验：旧世代的结果一律丢弃，于是重建、自动重试与
/// 迟到的页面事件不会互相污染，也不会留下会误触发上一轮的定时器（线程睡满即自行退出，
/// 不是常驻计时器）。
#[derive(Debug, Clone)]
pub struct LoadState {
    pub generation: u64,
    /// 本次加载的目标 URL（错误页期间仍是**失败的那个地址**，而不是 error.html）。
    pub url: String,
    pub phase: LoadPhase,
    pub last_error: Option<String>,
    /// **连续**失败次数（下一次成功加载后的再下一次建窗才清零，见 `begin_load`）：
    /// 决定自动重试的退避档位与「重试已用尽」。
    pub failures: u32,
    /// 当前主窗口显示的是内置错误页。
    pub error_page: bool,
    /// 已发出页面自检探针、正在等它回话的世代（`None` = 没有在飞的探针）。
    pub probe_generation: Option<u64>,
    /// 错误页上计划中的自动重试秒数（`None` = 没有计划中的重试）。
    pub next_retry_seconds: Option<u64>,
}

impl Default for LoadState {
    fn default() -> Self {
        Self {
            generation: 0,
            url: String::new(),
            // 还没有建过窗：不是「正在加载」，也不该被当成失败
            phase: LoadPhase::Loaded,
            last_error: None,
            failures: 0,
            error_page: false,
            probe_generation: None,
            next_retry_seconds: None,
        }
    }
}

/// 交给错误页渲染的一份失败说明（`__FNOS_SET_ERROR__` 的入参）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorInfo {
    /// 失败的地址（不是 error.html 自己）。
    pub url: String,
    pub reason: String,
    pub failures: u32,
    pub next_retry_seconds: Option<u64>,
    /// 自动重试已用尽（错误页据此改文案，不再显示倒计时）。
    pub retry_stopped: bool,
    /// 退避计划（秒），供页面写出「退避：15s → 30s → 60s → 120s」。
    pub retry_schedule: Vec<u64>,
}

/// 「加载失败」的原因判定：只有 WebView2 自己的错误页 URL 才算。
///
/// wry 丢掉了 `IsSuccess`（见 [`LOAD_PROBE_JS`] 的注释），能拿到的最硬信号就是导航后的
/// 文档 URL：失败的导航会落到 `chrome-error://chromewebdata/`。
fn load_failure_reason(url: &str) -> Option<String> {
    if url.starts_with("chrome-error://") {
        return Some("WebView2 无法打开该地址（网络错误或地址不可达）".to_string());
    }
    None
}

/// 页面自检探针的回话 → 失败原因；`None` = 判定为「确实加载成功了」。
///
/// 只认明确的 `err: true`：解析不出来（页面把标题换成了别的东西 / 探针被 CSP 拦）时
/// 一律**不**降级为失败——宁可漏报，也不要把正常页面误判成加载失败。
fn probe_verdict(payload: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    if value.get("err").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let detail = value
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    Some(if detail.is_empty() {
        "WebView2 报告该地址无法访问（网络错误）".to_string()
    } else {
        format!("WebView2 报告：{detail}")
    })
}

/// origin 的 host 是否属于飞牛（`fnos.net` 或 `*.fnos.net`）。
///
/// 上游的签名正则要求前导点（`(\.fnos\.net)$`，`content-script.js:2746-2779`），所以
/// **根域官网永远不注入**；这里判「是不是飞牛的域名」，注入与否由 [`is_recognized`] 与
/// 上游判定分层处理（状态条据此区分「官网」与「WebUI」两种说法）。
fn is_fnos_net_origin(origin: &str) -> bool {
    let Ok(url) = tauri::Url::parse(origin) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    host == "fnos.net" || host.ends_with(".fnos.net")
}

/// 是否就是**官网根域**（`https://fnos.net`）：它与 `*.fnos.net` 的 WebUI 是两种状态。
fn is_official_home_origin(origin: &str) -> bool {
    let Ok(url) = tauri::Url::parse(origin) else {
        return false;
    };
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("fnos.net"))
}

/// 当前页算不算「检测到了 fnOS WebUI」（spec §12.3 的状态条判据）。
///
/// 三个来源：官网根域 ∪ `mods.enabledOrigins` ∪ `shell.nasUrl` 的 origin。后者在
/// `Config::normalize` 里已经并入白名单，这里再查一次是为了「手工搭出来、没跑过 normalize
/// 的 Config」（单测）也给出同一个答案——两处判据同源，不会出现「设置窗说未检测到、
/// 但注入确实生效」。
///
/// 白名单比较**大小写不敏感**：上游按 `location.origin` 严格比较（cs:2948），而
/// `normalize()` 落盘时已统一小写；这里再宽松一次是为了内存里被手改过的那份也不误判。
fn is_recognized(cfg: &Config, origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    if is_fnos_net_origin(origin) {
        return true;
    }
    if cfg
        .mods
        .enabled_origins
        .iter()
        .any(|o| o.eq_ignore_ascii_case(origin))
    {
        return true;
    }
    cfg.shell
        .nas_url_parsed()
        .map(|u| u.origin().ascii_serialization())
        .as_deref()
        == Some(origin)
}

/// 第 `failures` 次连续失败之后该等多久再自动重试；`None` = 不再自动重试。
fn auto_retry_delay_ms(failures: u32) -> Option<u64> {
    if failures == 0 {
        return None;
    }
    AUTO_RETRY_DELAYS_MS.get(failures as usize - 1).copied()
}

/// 重建时必须原样保留的窗口显示态（Item 3）。
///
/// 只看 `is_visible()` 是不够的：**最小化的窗口 `is_visible()` 仍是 true**，
/// 于是「最小化时切换注入开关」会把窗口弹到前台；最大化态则必须显式记下来，
/// 否则重建回来的是一张普通窗口。这里刻意只是一个三字段的值 + 两个动作，
/// 不引入状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayState {
    pub visible: bool,
    pub minimized: bool,
    pub maximized: bool,
}

impl Default for DisplayState {
    fn default() -> Self {
        Self {
            visible: true,
            minimized: false,
            maximized: false,
        }
    }
}

impl DisplayState {
    /// 建窗时是否直接可见：隐藏态与最小化态都先隐藏建窗，否则新窗口会在
    /// `minimize()` / `hide()` 生效之前先闪一下前台。
    fn start_visible(self) -> bool {
        self.visible && !self.minimized
    }

    /// 把旧窗口的显示态原样套到新窗口上。顺序即优先级：
    /// 最小化（不 `show()` / 不 `set_focus()`，否则会把用户主动最小化的窗口提到前台）
    /// → 隐藏（保持隐藏）→ 最大化（还原最大化）→ 普通可见窗口（建窗时已可见，无需动作）。
    fn apply_to<R: Runtime>(self, window: &WebviewWindow<R>) {
        if self.minimized {
            let _ = window.minimize();
        } else if !self.visible {
            let _ = window.hide();
        } else if self.maximized {
            let _ = window.maximize();
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub shell_version: String,
    pub mods_commit: String,
    pub mods_version: String,
    pub config_path: String,
    pub webview_version: Option<String>,
    /// 随包许可全文的**实际**位置（spec §10 / Ruling R54）。安装后落在资源目录
    /// （Windows 上即主程序所在目录）的 `fnos-mods/LICENSE`。
    pub license_path: String,
    /// 随包 NOTICE（来源仓库 + commit + SHA-256 + 包装性改动清单）的实际位置。
    pub notice_path: String,
    /// 本次会话是否从 `config.json.bak` 恢复过（Task 11 / spec §12.3 的「配置损坏」一行）。
    /// 设置窗状态条据此**回显**，而不是让一次静默回退只留在磁盘上的 `.bak` 里。
    pub recovered_from_backup: bool,
}

/// 随包合规件在**本次运行**下的真实路径（spec §10：分发必须保留许可全文）。
///
/// 为什么单独一个类型：`config_view` 刻意不接 `AppHandle`（见它的注释），而解析资源目录
/// 需要 `app.path()`。于是由持有句柄的调用方先解析好再传进去——比给 `config_view` 塞一个
/// `&AppHandle` 更贴合它「纯组装」的定位。
#[derive(Debug, Clone)]
pub struct CompliancePaths {
    pub license: String,
    pub notice: String,
}

impl CompliancePaths {
    pub fn resolve<R: Runtime>(app: &AppHandle<R>) -> Self {
        // 取不到资源目录时 `compliance_path` 自己回落到源码树路径，这里 `ok()` 即可
        let dir = app.path().resource_dir().ok();
        Self {
            license: paths::compliance_path(dir.as_deref(), paths::LICENSE_RESOURCE),
            notice: paths::compliance_path(dir.as_deref(), paths::NOTICE_RESOURCE),
        }
    }
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

pub fn load_config<R: Runtime>(app: &AppHandle<R>) -> (Config, bool) {
    let path = Config::config_path();
    let (cfg, report) = Config::load_with_report(&path);
    let _ = cfg.save(&path);
    // 首启的装配顺序是「先读配置、再 `manage(AppState)`」，所以这一刻通常还没有 state，
    // 回退标记必须**返回给调用方**并由 `save_and_install_state` 带进去；`try_state` 分支
    // 是给将来「运行期重新装载配置」留的对称路径。
    if let Some(state) = app.try_state::<AppState>() {
        *state.config.lock().unwrap() = cfg.clone();
        state
            .recovered_from_backup
            .store(report.recovered_from_backup, Ordering::SeqCst);
    }
    (cfg, report.recovered_from_backup)
}

fn current<R: Runtime>(app: &AppHandle<R>) -> Config {
    app.state::<AppState>().config.lock().unwrap().clone()
}

/// `config_view` 的 `meta.recoveredFromBackup` 取值处（会话级状态，不在 `Config` 里）。
fn recovered_from_backup<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<AppState>()
        .recovered_from_backup
        .load(Ordering::SeqCst)
}

/// 组装 IPC 返回的配置视图。
///
/// 刻意**不**接收 `AppHandle`：`config_view` 用不到 app 句柄（`tauri::webview_version()`
/// 是不需要 app 的自由函数），带上它只会产生 `unused variable: app` 警告（R18 要求 0 警告）。
/// 两个例外都由唯一持有 `AppHandle` 的调用方读出来后传进来：`recovered_from_backup`
/// （不是配置字段）与 `compliance`（资源目录要 `app.path()`）。
fn config_view(
    cfg: &Config,
    recovered_from_backup: bool,
    compliance: &CompliancePaths,
) -> ConfigView {
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
            // spec §10 / R54：关于页必须能指出**随包**许可全文与 NOTICE 的真实位置
            license_path: compliance.license.clone(),
            notice_path: compliance.notice.clone(),
            recovered_from_backup,
        },
    }
}

#[tauri::command]
pub fn get_config<R: Runtime>(app: AppHandle<R>) -> ConfigView {
    let cfg = current(&app);
    let recovered = recovered_from_backup(&app);
    config_view(&cfg, recovered, &CompliancePaths::resolve(&app))
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
        config: config_view(
            &cfg,
            recovered_from_backup(&app),
            &CompliancePaths::resolve(&app),
        ),
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
    // 首启路径：建窗即可见（`start_visible = true`）。
    build_main_window_with(app, url, cfg, true)
}

/// `build_main_window` 的实体；`start_visible = false` 供重建路径使用——旧的隐藏 / 最小化
/// 窗口不该让新窗口先可见地闪一下（Item 3）。
fn build_main_window_with<R: Runtime>(
    app: &AppHandle<R>,
    url: tauri::Url,
    cfg: &Config,
    start_visible: bool,
) -> tauri::Result<WebviewWindow<R>> {
    // 建窗即登记一次「加载开始」：`generation` 会被下面的回调捕获，用于丢弃旧世代的
    // 迟到事件（重建 / 自动重试期间尤其重要）。
    let generation = begin_load(app, url.as_str());
    let mut builder = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::External(url))
        .title("fnOS")
        // 不变式（Item 2）：`cfg.shell.window` 已经由 `Config::normalize`
        //（`WindowGeom::clamp_to_usable`）夹成可用几何，而所有进入 `AppState` 的 `Config`
        // 都经过它（`load` / `set_config` / `reset_config` / `set_inject_enabled` /
        // `save_window_geom`），所以这里不会再拿到 `0x0` 或负数。
        .inner_size(cfg.shell.window.w, cfg.shell.window.h)
        // spec §7「主窗口：标题跟随页面」。tauri/wry **不会**自动把 `document.title`
        // 同步到窗口标题：wry 只在 `WebviewBuilder::with_document_title_changed_handler`
        // 被注册时才转发 `DocumentTitleChanged`（wry-0.57.0/src/webview2/mod.rs:689），
        // 而 tauri 默认不注册（`webview/mod.rs:361` 里 `document_title_changed_handler: None`）。
        // 不注册的话标题恒为上面的 `"fnOS"`。Task 11 起这条通路还多一个用途：页面自检探针
        // 的回话（见 `LOAD_PROBE_JS` / `handle_title`）。
        .on_document_title_changed(|window, title| handle_title(&window, &title))
        // 每次页面加载打一行：既是现场排障手段，也是「重建确实换了一张新窗口」的证据。
        .on_page_load(move |window, payload| {
            on_page_event(
                window.app_handle(),
                generation,
                payload.event(),
                payload.url().as_str(),
            );
        });

    let script = injector::build_init_script(cfg);
    if !script.is_empty() {
        builder = builder.initialization_script(script);
    }
    if !start_visible {
        builder = builder.visible(false);
    }

    let window = builder.build()?;
    tray::apply_window_geom(&window, cfg);
    // 「加载完成」迟迟不来就切错误页（spec §12.3）。看门狗是**独立线程**，睡满即自行退出；
    // 它靠 `generation` 判断自己是否还属于当前这一轮，所以重建 / 自动重试不会留下会误触发
    // 上一轮的定时器，也没有任何常驻计时器（无泄漏）。
    arm_load_watchdog(app, generation);
    Ok(window)
}

/// 建**内置错误页**窗口（Task 11 / spec §12.3「主窗口加载失败/离线」）。
///
/// 与[`build_main_window_with`] 的关键差别是**不注册任何 initialization_script**：mods 载荷
/// 绝不在错误页运行。这里刻意不为了省几行复用正常路径再「传个空串」——那条路径的语义是
/// 「injectEnabled 关了所以不注册」，与「错误页永远不注册」是两件事，混在一起将来很容易被
/// 一次重构抹掉。上游的签名判定同样不参与：错误页是应用自己的资产，与候选站点无关。
///
/// 失败说明（地址 / 原因 / 退避计划）通过 `PageLoadEvent::Finished` 后的 `eval` 交给页面：
/// 主窗口没有任何 IPC 授权（capability 只授 `settings` 窗），错误页不可能自己去查。
fn build_error_window<R: Runtime>(
    app: &AppHandle<R>,
    info: &ErrorInfo,
    cfg: &Config,
    start_visible: bool,
) -> tauri::Result<WebviewWindow<R>> {
    set_error_state(app, info);
    let payload = serde_json::to_string(info).unwrap_or_else(|_| "{}".into());
    let mut builder =
        WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App(ERROR_PAGE_ASSET.into()))
            .title("fnOS — 页面加载失败")
            .inner_size(cfg.shell.window.w, cfg.shell.window.h)
            .on_document_title_changed(|window, title| handle_title(&window, &title))
            .on_page_load(move |window, payload_event| {
                if payload_event.event() != PageLoadEvent::Finished {
                    return;
                }
                eprintln!(
                    "[fnos] 错误页已加载（地址 {}；本页不注册 mods 初始化脚本）",
                    payload_event.url()
                );
                let _ = window.eval(format!(
                    "window.__FNOS_SET_ERROR__ && window.__FNOS_SET_ERROR__({payload});"
                ));
            });
    if !start_visible {
        builder = builder.visible(false);
    }
    let window = builder.build()?;
    tray::apply_window_geom(&window, cfg);
    Ok(window)
}

/// 登记一次「加载开始」：自增世代、记下目标 URL、进入 `Loading`。
///
/// 连续失败计数在这里决定去留：上一轮就是 `Failed`（自动重试 / 用户重试）时**继续累计**，
/// 于是退避按 15s → 30s → 60s → 120s 递进、第 5 次停下来；上一轮正常（`Loaded`）时清零。
fn begin_load<R: Runtime>(app: &AppHandle<R>, url: &str) -> u64 {
    let state = app.state::<AppState>();
    let generation = {
        let mut load = state.load.lock().unwrap();
        let keep_failures = load.phase == LoadPhase::Failed;
        load.generation = load.generation.wrapping_add(1);
        load.url = url.to_string();
        load.phase = LoadPhase::Loading;
        load.error_page = false;
        load.last_error = None;
        load.probe_generation = None;
        load.next_retry_seconds = None;
        if !keep_failures {
            load.failures = 0;
        }
        load.generation
    };
    // Task 13a：新的一次加载 = 换了一张窗口/文档，旧上报不再属于当前页面（绝不沿用）。
    *state.page_report.lock().unwrap() = None;
    generation
}

/// 把观测状态改写成「正在显示错误页」（错误页窗口建好之前调用）。
fn set_error_state<R: Runtime>(app: &AppHandle<R>, info: &ErrorInfo) -> u64 {
    let state = app.state::<AppState>();
    let generation = {
        let mut load = state.load.lock().unwrap();
        load.generation = load.generation.wrapping_add(1);
        load.url = info.url.clone();
        load.phase = LoadPhase::Failed;
        load.error_page = true;
        load.probe_generation = None;
        load.last_error = Some(info.reason.clone());
        load.next_retry_seconds = info.next_retry_seconds;
        load.generation
    };
    // 错误页上永远不注册 mods 载荷，因此也不该留着任何页面上报（Task 13a）。
    *state.page_report.lock().unwrap() = None;
    generation
}

/// 主窗口 `document.title` 变化：两条控制前缀走这里，其余照旧镜像到窗口标题。
///
/// 两个前缀的**副作用必须一致**：控制标题是「页面 → 宿主」的回传通道，不是给用户看的标题
/// ——既不镜像到窗口标题（`set_title`），也不进 UI，只在控制台留一行**固定短语 + 类型**
/// （绝不回显页面可控正文）。Task 11 的探针分支就是这样做的，Task 13a 的上报分支照抄。
pub fn handle_title<R: Runtime>(window: &WebviewWindow<R>, title: &str) {
    if let Some(payload) = title.strip_prefix(PROBE_TITLE_PREFIX) {
        // 探针标题是「页面 → 宿主」的回传通道，不是给用户看的标题：不镜像、只处理。
        on_load_probe(window.app_handle(), payload);
        return;
    }
    if let Some(payload) = title.strip_prefix(report::REPORT_TITLE_PREFIX) {
        on_page_report(window, payload);
        return;
    }
    // 现场排障 + 运行期证据（错误页会把自检结论写进标题，见 ui/settings/error.html）。
    eprintln!("[fnos] 主窗口标题: {title}");
    let _ = window.set_title(title);
}

/// 收到一条页面上报（`document.title` 的 `FNOSREPORT:` 通道；Task 13a）。
///
/// 四道闸门全在 [`report::validate`] 里（字节上限 → 合法 JSON → 必须是对象 → `type` 白名单），
/// 未通过就丢掉并留一行固定短语，**绝不 panic、绝不把页面可控正文写进日志或 UI**。
/// 通过后连同**上报文档的 origin** 一起存进 `AppState`（内存）：origin 用于「换了页面就不认
/// 旧上报」的诚实性判定（见 [`get_page_report`]）。
fn on_page_report<R: Runtime>(window: &WebviewWindow<R>, payload: &str) {
    let value = match report::validate(payload) {
        Ok(v) => v,
        Err(reason) => {
            eprintln!(
                "[fnos] 页面上报被拒（{}；{} 字节）",
                reason.as_str(),
                payload.len()
            );
            // 截断假象：标题通道的实测上限是 4096 字节（见 report::TITLE_CHANNEL_MAX_BYTES），
            // 超过它的上报到达宿主时已经被截断，只能表现为「不是合法 JSON」。把这一情形标出来，
            // 否则后来的人会去追一个并不存在的语法 bug（T13b 的大载荷上报正是高风险场景）。
            if reason == report::Reject::NotJson && report::truncation_suspected(payload.len()) {
                eprintln!(
                    "[fnos]   ↑ 长度已够到 document.title 的 {} 字节上限，疑似被通道截断（而不是 JSON 语法错）",
                    report::TITLE_CHANNEL_MAX_BYTES
                );
            }
            return;
        }
    };
    // 类型只用于日志，且一定来自允许表（validate 已保证），不可能是页面随手写的长文本。
    let ty = value.get("type").and_then(Value::as_str).unwrap_or("?");
    let dir = value.get("dir").and_then(Value::as_str).unwrap_or("?");
    let origin = window
        .url()
        .ok()
        .and_then(|u| config::origin_of(u.as_str()));
    // 日志先打（`origin` 随后就随 entry 一起进内存了：不 clone、不留第二份）
    eprintln!(
        "[fnos] 页面上报已接受：type={ty} dir={dir} origin={} 字节={}",
        origin.as_deref().unwrap_or("<未知>"),
        payload.len()
    );
    let state = window.state::<AppState>();
    *state.page_report.lock().unwrap() = Some(report::ReportEntry { value, origin });
}

/// 新文档与最近一次上报不同源 → 丢掉旧上报。
///
/// 主窗口可以在不改任何配置的情况下换页面（页内链接、托盘「打开 NAS」、重定向）。旧页面上报的
/// 「已注入」不能拿来描述新页面——`get_page_report` 也会再判一次，这里是「新文档到达时顺手清掉」，
/// 免得一条过期证据一直躺在内存里。
fn drop_report_if_origin_changed<R: Runtime>(app: &AppHandle<R>, url: &str) {
    let current = config::origin_of(url);
    let state = app.state::<AppState>();
    let mut slot = state.page_report.lock().unwrap();
    let stale = match slot.as_ref() {
        Some(entry) => !report::same_origin(entry.origin.as_deref(), current.as_deref()),
        None => false,
    };
    if stale {
        *slot = None;
    }
}

/// 读最近一次页面上报（`get_page_report` 的返回体；`None` = 没有可用的上报）。
///
/// **只授予设置窗**（`capabilities/default.json`），与 `get_page_state` 同一档：主窗口
/// （含内置错误页）拿不到任何命令授权，所以页面永远无法自己读取或改写这条证据。
///
/// 只有「上报来源 origin == 主窗口当前 origin」时才返回内容：取不到当前 origin（窗口已销毁 /
/// URL 解析不出来）一律 `None`，设置窗于是退回 Task 11 的弱文案——宁可不说，也不谎称已注入。
#[tauri::command]
pub fn get_page_report<R: Runtime>(app: AppHandle<R>) -> Option<Value> {
    let current = app
        .get_webview_window(MAIN_WINDOW)
        .and_then(|w| w.url().ok())
        .and_then(|u| config::origin_of(u.as_str()));
    let state = app.state::<AppState>();
    let slot = state.page_report.lock().unwrap();
    let entry = slot.as_ref()?;
    if !report::same_origin(entry.origin.as_deref(), current.as_deref()) {
        return None;
    }
    Some(entry.value.clone())
}

/// 主窗口页面加载事件。
///
/// 只看 `Finished`：`Started` 每次导航都会来，没有判定价值。**注意 wry 的
/// `PageLoadEvent::Finished` 挂在 WebView2 的 `NavigationCompleted` 上却丢掉了
/// `IsSuccess`**（wry-0.57.0/src/webview2/mod.rs:726-737 只取 URL），所以「连接被拒绝」
/// 这类快速失败同样会发 `Finished`。于是有两条判定通道：
///
/// 1. 导航后的文档 URL 是 WebView2 的错误页（`chrome-error://`）→ 直接判失败；
/// 2. 页面自检探针（[`LOAD_PROBE_JS`]）回报 Chromium 网络错误页的 DOM 标记 → 判失败
///    （这是快速失败的主通道，通道 1 是它之前的廉价短路）。
///
/// 两条都没报警时按「加载成功」处理，并且**不设任何兜底降级**：宁可不报，也不要把正常
/// 页面判成加载失败（真·卡死不返回的情形由看门狗兜住）。
pub fn on_page_event<R: Runtime>(
    app: &AppHandle<R>,
    generation: u64,
    event: PageLoadEvent,
    url: &str,
) {
    if event != PageLoadEvent::Finished {
        return;
    }
    eprintln!("[fnos] 主窗口页面已加载: {url}");
    // Task 13a：页内导航（托盘「打开 NAS」的 `navigate`、页面里的链接、重定向）**不走**
    // `begin_load`，所以上面那段带世代闸门的代码会直接 return——清理必须放在闸门**之前**，
    // 否则新文档与旧上报不同源时那条证据会一直留着。`get_page_report` 读取时还会再判一次
    // （那时的 URL 已经是新文档），两层都必要：这里负责「及时丢掉」，那里负责「绝不误报」。
    drop_report_if_origin_changed(app, url);
    {
        let state = app.state::<AppState>();
        let mut load = state.load.lock().unwrap();
        if load.generation != generation || load.phase != LoadPhase::Loading {
            return; // 旧世代的迟到事件（重建 / 自动重试竞态）
        }
        // 收到 Finished = 这一次加载有结果了 → 看门狗退场（它的判据是 `phase == Loading`）
        load.phase = LoadPhase::Loaded;
        load.url = url.to_string();
        load.probe_generation = Some(generation);
    }
    if let Some(reason) = load_failure_reason(url) {
        eprintln!("[fnos] URL 判定为 WebView2 错误页：{url}");
        mark_failed(app, generation, url, &reason);
        return;
    }
    // URL 上看不出失败（实测失败导航的 Source 仍是请求地址）→ 让页面自己回答
    eprintln!("[fnos] 页面 URL 未显示错误页，交由页面自检探针判定: {url}");
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.eval(LOAD_PROBE_JS);
    }
}

/// 页面自检探针的回话；只有明确的「这是错误页」才降级。
fn on_load_probe<R: Runtime>(app: &AppHandle<R>, payload: &str) {
    let Some(reason) = probe_verdict(payload) else {
        return;
    };
    let (generation, url) = {
        let state = app.state::<AppState>();
        let mut load = state.load.lock().unwrap();
        // 只认「为这一世代发出、且尚在本轮内」的探针回话
        if load.probe_generation != Some(load.generation) || load.phase != LoadPhase::Loaded {
            return;
        }
        load.probe_generation = None;
        (load.generation, load.url.clone())
    };
    eprintln!("[fnos] 页面自检判定为加载失败：{reason}");
    mark_failed(app, generation, &url, &reason);
}

/// 判定失败：记账（连续失败次数 / 原因）并切到内置错误页。
fn mark_failed<R: Runtime>(app: &AppHandle<R>, generation: u64, url: &str, reason: &str) {
    {
        let state = app.state::<AppState>();
        let mut load = state.load.lock().unwrap();
        if load.generation != generation || load.phase == LoadPhase::Failed {
            return;
        }
        load.phase = LoadPhase::Failed;
        load.error_page = false;
        load.url = url.to_string();
        load.last_error = Some(reason.to_string());
        load.failures = load.failures.saturating_add(1);
        load.probe_generation = None;
    }
    eprintln!("[fnos] 主窗口加载失败：{url} — {reason}");
    show_error_page(app, url, reason);
}

/// 切换主窗口到内置错误页，并安排一次自动重试。
///
/// 复用既有的重建机制（销毁旧窗口 → `Destroyed` 回调里建同 label 新窗口），只是这一轮的
/// 建窗目标是错误页 + 空初始化脚本（`recreate_error` → `rebuild_main`）。
fn show_error_page<R: Runtime>(app: &AppHandle<R>, failed_url: &str, reason: &str) {
    // 退避档位由「连续失败次数」决定；用尽后不再自动重试（错误页正文会如实说明）
    let failures = {
        let state = app.state::<AppState>();
        let mut load = state.load.lock().unwrap();
        load.next_retry_seconds = auto_retry_delay_ms(load.failures).map(|ms| ms / 1000);
        load.failures
    };
    let delay = auto_retry_delay_ms(failures);
    let info = ErrorInfo {
        url: failed_url.to_string(),
        reason: reason.to_string(),
        failures,
        next_retry_seconds: delay.map(|ms| ms / 1000),
        retry_stopped: delay.is_none(),
        retry_schedule: AUTO_RETRY_DELAYS_MS.iter().map(|ms| ms / 1000).collect(),
    };
    *app.state::<AppState>().recreate_error.lock().unwrap() = Some(info);
    if let Err(e) = trigger_recreate(app) {
        eprintln!("[fnos] 切换错误页失败: {e}");
        // 建不出来就别把错误页目标挂在那里，否则下一次重建会莫名其妙变成错误页
        let _ = app
            .state::<AppState>()
            .recreate_error
            .lock()
            .unwrap()
            .take();
        return;
    }
    if let Some(delay_ms) = delay {
        schedule_auto_retry(app.clone(), delay_ms);
    }
}

/// 加载看门狗：`LOAD_TIMEOUT_SECS` 内没有 `Finished` → 按失败处理（spec §12.3）。
fn arm_load_watchdog<R: Runtime>(app: &AppHandle<R>, generation: u64) {
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("fnos-load-watchdog".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(LOAD_TIMEOUT_SECS));
            let (stale, url) = {
                let state = handle.state::<AppState>();
                let load = state.load.lock().unwrap();
                (
                    load.generation != generation || load.phase != LoadPhase::Loading,
                    load.url.clone(),
                )
            };
            if stale {
                return;
            }
            let reason = format!("页面在 {LOAD_TIMEOUT_SECS} 秒内没有完成加载（超时）");
            eprintln!("[fnos] {reason}");
            mark_failed(&handle, generation, &url, &reason);
        });
    if let Err(e) = spawned {
        eprintln!("[fnos] 加载看门狗线程启动失败（超时兜底失效）: {e}");
    }
}

/// 错误页的自动重试（退避见 [`AUTO_RETRY_DELAYS_MS`]）。
///
/// 认领判据是「仍在错误页、且这次计划还没被消费」：用户先手动重试（`recreate_main_window`
/// 会把 `error_page` 置回 false）时这里安静退出，不会多建一张窗口。线程睡满即退出。
fn schedule_auto_retry<R: Runtime>(app: AppHandle<R>, delay_ms: u64) {
    let spawned = std::thread::Builder::new()
        .name("fnos-auto-retry".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(delay_ms));
            let go = {
                let state = app.state::<AppState>();
                let mut load = state.load.lock().unwrap();
                if load.error_page
                    && load.phase == LoadPhase::Failed
                    && load.next_retry_seconds.is_some()
                {
                    load.next_retry_seconds = None;
                    true
                } else {
                    false
                }
            };
            if !go {
                return;
            }
            eprintln!("[fnos] 错误页自动重试（退避 {delay_ms} ms）");
            if let Err(e) = recreate_main_window(&app, None) {
                eprintln!("[fnos] 自动重试重建主窗口失败: {e}");
            }
        });
    if let Err(e) = spawned {
        eprintln!("[fnos] 自动重试线程启动失败: {e}");
    }
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

/// 真正建新主窗口：消费一次性 URL 覆盖 / 错误页目标与显示态，建完后同步托盘菜单并
/// **无论成败**清 `recreating`（否则一次失败会让下次关窗绕过 `closeToTray`）。
fn rebuild_main<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let cfg = current(app);
    let override_url = state.recreate_url.lock().unwrap().take();
    let error_info = state.recreate_error.lock().unwrap().take();
    let display = *state.recreate_display.lock().unwrap();

    let built = match error_info {
        Some(info) => build_error_window(app, &info, &cfg, display.start_visible()),
        None => {
            let url = resolve_main_url(&cfg, override_url.as_deref());
            build_main_window_with(app, url, &cfg, display.start_visible())
        }
    }
    .map_err(|e| e.to_string());
    state.recreating.store(false, Ordering::SeqCst);
    if let Err(e) = tray::sync_menus(app, &cfg) {
        eprintln!("[fnos] 托盘同步失败: {e}");
    }
    let window = built?;
    // Item 3：隐藏态保持隐藏、最小化态保持最小化（不抢前台）、最大化态还原最大化。
    // 三者都不做 `set_focus()`——重建是配置变更的副作用，不该替用户切换前台窗口。
    display.apply_to(&window);
    Ok(())
}

/// 触发一次「销毁旧主窗口 → `Destroyed` 回调里重建」。
///
/// 调用方负责先把这一轮的目标摆好（`recreate_url` / `recreate_error`）。
/// 时序：置 `recreating` → 记下显示态（可见 / 最小化 / 最大化）+ 存几何 → `close()` 旧窗口
/// → 关闭请求这次不再被拦 → `Destroyed` 回调（`on_main_destroyed`）里建同 label 新窗口
/// → 套回显示态 → 清标志 → `sync_menus`。
/// **不能**在 `close()` 之后立刻建：label 注册表在 `Destroyed` 才释放，否则
/// `WindowLabelAlreadyExists`。
fn trigger_recreate<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.recreating.swap(true, Ordering::SeqCst) {
        return Ok(()); // 已在重建中：交给那一轮的 `Destroyed` 收尾
    }

    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        // 没有主窗口（closeToTray=false 关掉之后、或上一轮重建失败）：直接建一个可见窗口
        *state.recreate_display.lock().unwrap() = DisplayState::default();
        return rebuild_main(app);
    };

    // Item 3：显示态必须在关窗**之前**一次性捕获。`is_minimized()` 要单独看——
    // 最小化窗口的 `is_visible()` 仍是 true；`is_maximized()` 不看则重建后丢最大化态。
    *state.recreate_display.lock().unwrap() = DisplayState {
        visible: win.is_visible().unwrap_or(true),
        minimized: win.is_minimized().unwrap_or(false),
        maximized: win.is_maximized().unwrap_or(false),
    };
    save_window_geom(app); // 重建后按老几何摆回原位

    if let Err(e) = win.close() {
        // 关不掉就别留着标志，否则下次真的关窗会绕过 closeToTray
        state.recreating.store(false, Ordering::SeqCst);
        return Err(e.to_string());
    }
    Ok(())
}

/// 重建主窗口，让 `injectEnabled` / `homeUrl` 立刻生效（gap (a)），或从错误页回到真实页面。
///
/// 为什么必须换窗口：`initialization_script` 走的是 WebView2 的
/// `AddScriptToExecuteOnDocumentCreated`，同一次导航不会重新注册，窗口活着的期间**无法**
/// 替换；`__FNOS_APPLY_CONFIG__` 只能推 `mods` / `local`。round 0 的
/// `location.reload()` 因此永远关不掉注入。
///
/// Task 11：这条路径也是**唯一**的「重试」实现（设置窗状态条、托盘「重新加载主窗口」、
/// 错误页自动重试都走它）。它显式清掉待建的错误页目标——用户/调度要的是真实页面。
pub fn recreate_main_window<R: Runtime>(
    app: &AppHandle<R>,
    url: Option<String>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    // 要真实页面：丢掉任何挂着的错误页目标（否则重试会原地回到错误页）
    let _ = state.recreate_error.lock().unwrap().take();
    // 先落 URL 覆盖：即便此刻已有一轮重建在飞，这一轮的 `Destroyed` 也会用上它
    if let Some(raw) = url {
        let parsed =
            config::parse_web_url(&raw).ok_or_else(|| format!("非法 URL，拒绝导航：{raw}"))?;
        *state.recreate_url.lock().unwrap() = Some(parsed.to_string());
    }
    trigger_recreate(app)
}

/// 托盘「重新加载主窗口」：按配置的 `homeUrl` 重建主窗口（从错误页恢复的正路之一）。
pub fn reload_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = recreate_main_window(app, None) {
        eprintln!("[fnos] 重新加载主窗口失败: {e}");
    }
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

/// `get_page_state` 的返回体（Task 11 状态条的**唯一**数据来源）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageStateView {
    /// 主窗口当前地址；错误页期间是**失败的那个地址**（不是 error.html）。
    pub url: String,
    /// `url` 的 origin（解析不出来时 `None`）——「把当前页加入白名单」用它。
    pub origin: Option<String>,
    /// 是否算「检测到了 fnOS WebUI」（官网根域 / 白名单 / nasUrl 的 origin）。
    pub recognized: bool,
    /// 是否就是官网根域（`fnos.net`）：它按设计**不**注入，状态条要单独说明。
    pub official_home: bool,
    /// 上一次加载是否失败（错误页 = 这一态的表现）。
    pub load_failed: bool,
    /// 当前主窗口是否为内置错误页。
    pub error_page: bool,
    /// 已建窗但还没收到 `PageLoadEvent::Finished`。
    pub loading: bool,
    /// 失败原因（给状态条/错误页用）。
    pub last_error: Option<String>,
    /// 错误页上计划中的自动重试秒数。
    pub next_retry_seconds: Option<u64>,
    /// 注入开关的当前值（状态条要写「注入开关：开启/关闭」）。
    pub inject_enabled: bool,
}

/// 读主窗口的观测状态。**只读**、且只授予设置窗（`capabilities/default.json`）——
/// 主窗口（含内置错误页）没有任何命令授权，绝不出现「页面自己上报状态」的通道（R2）。
#[tauri::command]
pub fn get_page_state<R: Runtime>(app: AppHandle<R>) -> PageStateView {
    let cfg = current(&app);
    let snapshot = app.state::<AppState>().load.lock().unwrap().clone();
    // 活着的窗口 URL 优先（它能反映重定向 / 页内导航）；错误页例外——那时 URL 是应用资产
    // （`http://tauri.localhost/error.html`），对用户没有意义。
    let live = app
        .get_webview_window(MAIN_WINDOW)
        .and_then(|w| w.url().ok())
        .map(|u| u.to_string());
    let url = if snapshot.error_page {
        snapshot.url.clone()
    } else {
        live.unwrap_or_else(|| snapshot.url.clone())
    };
    let origin = config::origin_of(&url);
    let official_home = origin.as_deref().is_some_and(is_official_home_origin);
    PageStateView {
        recognized: !snapshot.error_page && is_recognized(&cfg, origin.as_deref()),
        url,
        origin,
        official_home,
        load_failed: snapshot.phase == LoadPhase::Failed,
        error_page: snapshot.error_page,
        loading: snapshot.phase == LoadPhase::Loading,
        last_error: snapshot.last_error.clone(),
        next_retry_seconds: snapshot.next_retry_seconds,
        inject_enabled: cfg.shell.inject_enabled,
    }
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

/// 用**系统默认浏览器**打开外部链接（spec §10：关于页的上游仓库链接）。
///
/// 为什么需要它（Review finding D）：关于页的 `<a>` 在既有架构里是死链——
/// 不加 `target="_blank"` 时 Chromium 会在**设置窗自身**里导航到 GitHub（UI 被顶掉，
/// 而 `capabilities/default.json` 只授权本地来源，加载后的远程页面调不动任何命令）；
/// 加了 `target="_blank"` 时 wry 在 `new_window_handler` 为 `None` 时直接
/// `args.SetHandled(true)`（wry-0.57.0/src/webview2/mod.rs 的 `NewWindowRequested` 分支，
/// tauri 默认不注册），新窗口请求被静默吞掉 = 点了不跳转。正路是把「打开外部链接」
/// 交给 shell，UI 只负责 `preventDefault` + `invoke('open_url')`。
///
/// **安全不变式**：URL 先过 `config::parse_web_url`（scheme 必须是 `http`/`https`、
/// 必须带 host，且判定前 trim）。`javascript:` / `data:` / `file:` / `mailto:` /
/// 无 scheme（`nas.example.com:8000`）一律 `Err`——绝不会有未校验文本进入进程创建。
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let parsed =
        external_url(&url).ok_or_else(|| format!("只允许 http/https 链接，已拒绝：{url}"))?;
    open_in_browser(parsed.as_str())
}

/// `open_url` 的校验部分，单独拆出来是为了能单测（断言拒绝路径不必真的开浏览器）。
fn external_url(raw: &str) -> Option<tauri::Url> {
    config::parse_web_url(raw)
}

/// 把已经校验过的 http(s) URL 交给系统默认浏览器。
///
/// 选 `explorer <url>` 而不是 `cmd /C start "" <url>`：前者把 URL 作为**一个**
/// `CreateProcess` 参数直接交给 explorer（ShellExecute 语义），完全不经过 cmd.exe 的
/// 参数/元字符解析（`&`、`^`、`%`、`!` 都不需要转义，也不需要 `start` 那个
/// 「第一个带引号的参数会被当成窗口标题」的坑）。后者等于把「已校验文本」再交给一个
/// 命令行解释器去解析，多一层不需要的解析面。`open_config_dir` 用的也是同一个
/// `explorer` 进程，两处行为一致、可预期。
///
/// `explorer.exe` 的退出码不表示成败（对 URL 这类转发恒为 1），所以只 `spawn` 不 `wait`。
fn open_in_browser(url: &str) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(url)
        .spawn()
        .map_err(|e| format!("打开系统默认浏览器失败：{e}"))?;
    // 排障 + 验证用：一次成功的「交给系统浏览器」在应用 stderr 留一行。运行时验证靠它
    // 确认点击真的走到了这条命令（而不是被 webview 吞掉），与建窗时的页面加载日志同风格。
    eprintln!("[fnos] open_url -> 系统默认浏览器: {url}");
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
    Ok(config_view(
        &cfg,
        recovered_from_backup(&app),
        &CompliancePaths::resolve(&app),
    ))
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
///
/// `recovered_from_backup` 只能从这里进来：`load_config` 跑的时候 `AppState` 还没被
/// `manage`，那时读不到托管状态（Task 11 的配置损坏回显就靠这个参数带过来）。
pub fn save_and_install_state<R: Runtime>(
    app: &AppHandle<R>,
    cfg: &Config,
    recovered_from_backup: bool,
) {
    let path = Config::config_path();
    let _ = cfg.save(&path);
    app.manage(AppState {
        config: Mutex::new(cfg.clone()),
        recreating: AtomicBool::new(false),
        recreate_url: Mutex::new(None),
        recreate_display: Mutex::new(DisplayState::default()),
        load: Mutex::new(LoadState::default()),
        recreate_error: Mutex::new(None),
        recovered_from_backup: AtomicBool::new(recovered_from_backup),
        page_report: Mutex::new(None),
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
    // Item 2：这里直接复用归一化的夹取规则，保证**内存 state 与磁盘**一样永远是可用几何
    //（`build_main_window` / `apply_window_geom` 消费的就是 state 里这份值）。
    // 窗口被拖到小于下限时内存/磁盘记下限值，比将来拿一个不可用尺寸去建窗安全。
    cfg.shell.window.clamp_to_usable();
    let _ = cfg.save(&Config::config_path());
    *app.state::<AppState>().config.lock().unwrap() = cfg;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `open_url` 的闸门：只有绝对 http(s) URL 能过。
    #[test]
    fn external_url_accepts_only_absolute_http_and_https() {
        assert!(external_url("https://github.com/aurysian-yan/fnOS_UI_Mods").is_some());
        assert!(external_url("http://nas.local:5666").is_some());
        // 判定前 trim（与 `parse_web_url` 一致），且路径/查询串不参与判定
        assert!(external_url("  http://nas.local:8000/ui/index.html?a=1  ").is_some());

        for bad in [
            "nas.example.com:8000", // 无 scheme：`Url::parse` 当相对地址 → 解析失败
            "nas:8000",             // scheme 合法但不是 http(s)
            "localhost:8000",
            "mailto:a@b.c",
            "data:text/html,<script>alert(1)</script>",
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "ftp://nas.local",
            "http://", // 没有 host
            "https://",
            "",
            "   ",
        ] {
            assert!(external_url(bad).is_none(), "应当拒绝：{bad:?}");
        }
    }

    /// 命令级拒绝路径（不触发任何进程创建，因此可以进单测）。
    #[test]
    fn open_url_rejects_non_http_without_spawning_anything() {
        for bad in [
            "javascript:alert(1)",
            "nas.example.com:8000",
            "mailto:a@b.c",
        ] {
            let err = open_url(bad.into()).expect_err("必须返回 Err");
            assert!(
                err.contains("http/https"),
                "错误信息应说明只允许 http(s)：{err}"
            );
        }
    }

    /// finding A 的算术前提：Rust 把 `#cec1b2` 夹成什么，以及那个值是不是不动点。
    ///
    /// 设置窗「显示值 = 生效值」的不变式就靠「Rust 已归一化、JS 不再夹一次」成立，
    /// 所以这里把 Rust 的实际输出钉住；JS 侧的对应断言（`clampLightness('#cec1b2')`
    /// 与 `clampLightness(那个值)`）在 tests/settings.test.mjs。
    #[test]
    fn brand_color_clamp_of_the_finding_a_input() {
        assert_eq!(
            crate::config::normalize_brand_color("#cec1b2"),
            "#c4b4a2",
            "JS clampLightness('#cec1b2') 也是 #c4b4a2，两侧必须同值"
        );
        // fix round 2：Rust 侧改成幂等（明度在区间内时原样返回），夹取结果因此是**不动点**。
        // 于是 `Config::save` 里那次多余的 `normalize()`（config.rs，注释写的是「幂等」）
        // 不再改动生效值——磁盘 == 内存 == 注入载荷 == 页面生效值。
        // 修前这里是 `"#c4b4a1"`：`#c4b4a2` 的 L = 179/255 = 0.70196 会被二次夹取。
        assert_eq!(
            crate::config::normalize_brand_color("#c4b4a2"),
            "#c4b4a2",
            "夹取必须是不动点，否则 save() 会让 config.json 与生效值差一个通道"
        );
        // JS 侧（ui/settings/normalize.js 的 `clampLightness`）仍不是不动点，本轮不动它；
        // 不变式改由「JS 只夹用户刚输入的值、绝不夹 Rust 归一化过的值」维持（fix round 1 的 A）。
    }

    // ---------- Task 12：合规件路径的 IPC 契约 ----------

    /// 关于页按 `meta.licensePath` / `meta.noticePath` 读；键名必须是 camelCase
    /// （`#[serde(rename_all = "camelCase")]` 是唯一来源，写错就会静默回落成源码路径）。
    #[test]
    fn meta_json_keys_are_camel_case() {
        let meta = Meta {
            shell_version: "0.1.0".into(),
            mods_commit: "483c3e2".into(),
            mods_version: "1.0.2".into(),
            config_path: r"C:\cfg\config.json".into(),
            webview_version: Some("139.0.0.0".into()),
            license_path: r"C:\app\fnos-mods\LICENSE".into(),
            notice_path: r"C:\app\fnos-mods\NOTICE".into(),
            recovered_from_backup: false,
        };
        let json = serde_json::to_value(&meta).expect("Meta 必须可序列化");
        assert_eq!(json["licensePath"], r"C:\app\fnos-mods\LICENSE");
        assert_eq!(json["noticePath"], r"C:\app\fnos-mods\NOTICE");
        // 既有字段不变（设置窗的「关于」页读的就是这些键）
        assert_eq!(json["webviewVersion"], "139.0.0.0");
        assert_eq!(json["configPath"], r"C:\cfg\config.json");
        assert!(json.get("license_path").is_none(), "键名必须是 camelCase");
    }

    // ---------- Task 11：状态条 / 错误页的纯判据 ----------

    /// `*.fnos.net`（含根域）的识别：上游签名正则要求前导点，所以根域与子域必须分开处理。
    #[test]
    fn fnos_net_host_is_recognized_by_suffix_only() {
        assert!(is_fnos_net_origin("https://fnos.net"));
        assert!(is_fnos_net_origin("http://fnos.net"));
        assert!(is_fnos_net_origin("https://abc.fnos.net"));
        assert!(is_fnos_net_origin("https://a.b.fnos.net:8000"));
        // 后缀必须落在**标签边界**上：`evil-fnos.net` / `fnos.net.evil.com` 都不是飞牛域名
        assert!(!is_fnos_net_origin("https://evil-fnos.net"));
        assert!(!is_fnos_net_origin("https://fnos.net.evil.com"));
        assert!(!is_fnos_net_origin("http://127.0.0.1:8793"));
        assert!(!is_fnos_net_origin("not an origin"));
        assert!(!is_fnos_net_origin("chrome-error://chromewebdata/"));
    }

    /// 「官网根域」与「WebUI 子域」是两种状态：前者按设计不注入，后者才可能是 WebUI。
    #[test]
    fn official_home_is_the_bare_root_domain_only() {
        assert!(is_official_home_origin("https://fnos.net"));
        assert!(is_official_home_origin("http://fnos.net"));
        assert!(!is_official_home_origin("https://abc.fnos.net"));
        assert!(!is_official_home_origin("http://127.0.0.1:8793"));
        assert!(!is_official_home_origin("garbage"));
    }

    /// 判定「当前页是不是 fnOS WebUI」：官网根域 ∪ 白名单 ∪ nasUrl 的 origin（大小写不敏感）。
    #[test]
    fn recognized_covers_official_whitelist_and_nas_url() {
        let mut cfg = Config::default();
        assert!(!is_recognized(&cfg, None), "没有任何 URL 时不得声称命中");
        assert!(
            is_recognized(&cfg, Some("https://fnos.net")),
            "官网根域属于「检测到了 fnOS」（只是不注入）"
        );
        assert!(is_recognized(&cfg, Some("https://abc.fnos.net")));
        assert!(!is_recognized(&cfg, Some("http://127.0.0.1:8793")));

        cfg.mods.enabled_origins = vec!["http://nas.local:5666".into()];
        assert!(is_recognized(&cfg, Some("http://nas.local:5666")));
        assert!(
            is_recognized(&cfg, Some("http://NAS.LOCAL:5666")),
            "白名单匹配必须大小写不敏感（上游按 location.origin 比较）"
        );
        assert!(!is_recognized(&cfg, Some("http://nas.local:80")));

        // nasUrl 的 origin：即便这份 Config 没跑过 `normalize()`（例如测试里手搭的）也算命中
        let mut raw = Config::default();
        raw.shell.nas_url = "http://192.168.1.10:5666/webui/".into();
        assert!(is_recognized(&raw, Some("http://192.168.1.10:5666")));
        assert!(is_recognized(&raw, Some("http://192.168.1.10:5667")) == false);
    }

    /// 自动重试退避：15s → 30s → 60s → 120s，第 5 次起不再重试（错误页要如实写出来）。
    #[test]
    fn auto_retry_backoff_is_bounded() {
        assert_eq!(auto_retry_delay_ms(0), None, "没有失败就不该有重试计划");
        assert_eq!(auto_retry_delay_ms(1), Some(15_000));
        assert_eq!(auto_retry_delay_ms(2), Some(30_000));
        assert_eq!(auto_retry_delay_ms(3), Some(60_000));
        assert_eq!(auto_retry_delay_ms(4), Some(120_000));
        assert_eq!(
            auto_retry_delay_ms(5),
            None,
            "退避用尽后必须停止（不能无限重试）"
        );
        assert_eq!(auto_retry_delay_ms(99), None);
    }

    /// wry 丢掉 `NavigationCompleted::IsSuccess`，所以「导航失败」只能靠 URL 认出来。
    #[test]
    fn load_failure_is_detected_from_the_chrome_error_url() {
        assert!(load_failure_reason("chrome-error://chromewebdata/").is_some());
        assert!(load_failure_reason("http://127.0.0.1:8793/index.html").is_none());
        assert!(load_failure_reason("https://fnos.net/").is_none());
    }

    /// 页面自检探针（走 document.title 回传）的判定：只有明确的 `err:true` 才算失败。
    #[test]
    fn probe_verdict_only_fails_on_explicit_error() {
        let hit = probe_verdict(r#"{"err":true,"detail":"127.0.0.1 拒绝了我们的连接请求"}"#)
            .expect("err:true 必须判定为失败");
        assert!(
            hit.contains("127.0.0.1"),
            "必须带上 WebView2 给出的原因：{hit}"
        );
        assert!(
            probe_verdict(r#"{"err":false,"detail":""}"#).is_none(),
            "正常页面不得被判成失败"
        );
        assert!(
            probe_verdict("not json").is_none(),
            "解析不出来时不得擅自降级"
        );
        assert!(probe_verdict("[]").is_none());
        // err:true 但拿不到文案：仍要给出一个可读的原因，而不是空串
        let bare = probe_verdict(r#"{"err":true,"detail":""}"#).expect("err:true 仍须失败");
        assert!(!bare.trim().is_empty());
    }

    /// 宿主解析探针用的前缀必须与注入脚本里那个字面量**同源**，否则失败会被静默吞掉。
    #[test]
    fn probe_prefix_and_script_agree() {
        assert!(!PROBE_TITLE_PREFIX.is_empty());
        // JS 里可打印 ASCII 直接写，控制字符写成 `\uXXXX` 转义（见 LOAD_PROBE_JS）
        let escaped: String = PROBE_TITLE_PREFIX
            .chars()
            .map(|c| {
                if c.is_ascii_graphic() {
                    c.to_string()
                } else {
                    format!("\\u{:04x}", c as u32)
                }
            })
            .collect();
        assert!(
            LOAD_PROBE_JS.contains(&format!("'{}'", escaped)),
            "注入脚本里的前缀必须由 PROBE_TITLE_PREFIX 逐字转义而来：{escaped}"
        );
        assert!(LOAD_PROBE_JS.contains("main-frame-error"));
        assert!(!LOAD_PROBE_JS.contains("__FNOS_SHELL__"));
    }

    /// 上报前缀必须与 `shim.js` 里的字面量**同源**（Task 13a）。
    ///
    /// 两处不一致的表现是**静默丢数据**：页面写了 `FNOSREPORT:` 而宿主只认别的串，标题
    /// 就被当成普通标题镜像到窗口上（用户看到窗口标题变成一坨 JSON），上报却永远收不到。
    /// 与 `probe_prefix_and_script_agree` 同一手法：从常量逐字转义出 JS 字面量再在源码里找。
    #[test]
    fn report_prefix_and_shim_agree() {
        let shim = include_str!("../inject/shim.js");
        let escaped: String = report::REPORT_TITLE_PREFIX
            .chars()
            .map(|c| {
                if c.is_ascii_graphic() {
                    c.to_string()
                } else {
                    format!("\\u{:04x}", c as u32)
                }
            })
            .collect();
        assert!(
            shim.contains(&format!("'{escaped}'")),
            "shim.js 里的上报前缀必须由 report::REPORT_TITLE_PREFIX 逐字转义而来：{escaped}"
        );
        // 上报通道**不得**退化成 IPC：页面侧只允许写标题，不许出现任何 invoke 调用。
        assert!(
            !shim.contains("invoke("),
            "上报通道不得改用 IPC（那会给远程页面开命令授权，见 R70）"
        );
        // 允许表里的 type 必须逐字出现在 vendored 的上游源码里（不是凭空发明的字符串）。
        let upstream = include_str!("../assets/fnos-mods/content-script.js");
        for ty in report::REPORT_TYPES {
            assert!(
                upstream.contains(&format!("'{ty}'")),
                "允许表里的 {ty} 必须在 vendored 的 content-script.js 里真实存在"
            );
        }
    }
}
