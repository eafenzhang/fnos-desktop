//! IPC 契约（spec §8.3）与 Rust 侧内部操作。

use crate::{base64, config, config::Config, injector, paths, report, tray, MAIN_WINDOW};
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
    /// 证据，不是任何权限的来源。每次新建/重建主窗口、切错误页、以及新文档与上报**不同文档**
    /// （origin 或 URL 不同，fix round 1 / Minor 3）时都会清空
    /// （见 [`begin_load`] / [`set_error_state`] / [`on_page_event`]）——旧页面的上报绝不能
    /// 拿来描述新页面。
    pub page_report: Mutex<Option<report::ReportEntry>>,
    /// 最近一次**通过校验**的「应用项列表」上报（Task 13b；见 [`report::is_app_items_report`]）。
    ///
    /// 与 [`Self::page_report`] 完全同构（只在内存里、绑定一个文档、同一套清理点），但**分开
    /// 存**：状态条的强态判据是「最近一次上报是 `FNOS_INJECTION_TRIGGERED`」，而 T13b 主动
    /// 拉取应用项列表会在页面加载之后不断产生新的上报——同槽会让 T13a 的强态在「完美图标」
    /// 打开时永远读不到（功能回归）。分流规则见 [`store_report`]。
    pub app_items_report: Mutex<Option<report::ReportEntry>>,
    /// 进行中的**分片**上报（Task 13b；见 [`crate::report::ChunkAssembly`]）。
    ///
    /// 只在内存里，且**绑定一个文档**（origin + URL）：新的一次加载、切错误页、或文档与它
    /// 对不上时都会清空（见 [`begin_load`] / [`set_error_state`] /
    /// [`drop_report_if_document_changed`] / [`get_page_report`]）。分片本身还有片数、单片字节、
    /// 累计字节与时间窗口四道硬上限——半截载荷永远不会被当成一条完整上报。
    pub chunk_report: Mutex<Option<report::ChunkAssembly>>,
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
/// 只认明确的 `err:true`：解析不出来（页面把标题换成了别的东西 / 探针被 CSP 拦）时
/// 一律**不**降级为失败——宁可漏报，也不要把正常页面误判成加载失败。
///
/// **`detail` 是页面可控的**（探针标题与上报标题走同一条可写通道，见 [`handle_title`]），
/// 而它有四个去处：宿主 stderr 日志（`on_load_probe` / `mark_failed`）、`LoadState.last_error`
/// → 设置窗状态条、内置错误页的 `__FNOS_SET_ERROR__` 载荷。因此它按 fix round 1 /
/// Important 1 的同一条纪律收口（[`sane_probe_detail`]）：控制字符折成空格、连续空白折叠、
/// 按码点截断到 160——与注入脚本 `LOAD_PROBE_JS` 自己做的整形
/// （`m.replace(/\s+/g,' ').slice(0,160)`）同形，所以真实错误页的文案一个字都不会变，而页面
/// 自写的换行不再能在日志里伪造出整行 `[fnos] …`。
fn probe_verdict(payload: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    if value.get("err").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let detail = sane_probe_detail(value.get("detail").and_then(Value::as_str).unwrap_or(""));
    Some(if detail.is_empty() {
        "WebView2 报告该地址无法访问（网络错误）".to_string()
    } else {
        format!("WebView2 报告：{detail}")
    })
}

/// 探针 `detail` 的整形（页面可控文本 → 能安全进日志/UI/错误页的一行文本）。
///
/// 规则与 `LOAD_PROBE_JS` 页面侧的 `m.replace(/\s+/g,' ').slice(0,160)` 对齐：控制字符
/// （C0 / DEL / C1，含换行与制表）与 Unicode 空白（U+2028 / U+2029 / U+0085 …）一律折成**一个**
/// 空格、去首尾空白、按**码点**截断到 160。正文本身保留——排障要看得到页面到底写了什么。
fn sane_probe_detail(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(1024));
    for c in raw.chars() {
        if c.is_control() || c.is_whitespace() || matches!(c, '\u{2028}' | '\u{2029}') {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out.trim().chars().take(PROBE_DETAIL_MAX_CHARS).collect()
}

/// 探针 `detail` 进日志/UI 前的码点上限（= 注入脚本 `LOAD_PROBE_JS` 的 `slice(0,160)`）。
const PROBE_DETAIL_MAX_CHARS: usize = 160;

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
        // gap (a)：**只有**「会改变初始化载荷内容」的键需要重建主窗口（换载荷 / 换地址），
        // 其余字段一律经 `__FNOS_APPLY_CONFIG__` 免刷新生效。
        // 判定放在 `normalize()` 之后：非法 `homeUrl` 被夹成默认值时若与旧值相同，
        // 就不该白重建一次窗口。
        //
        // Task 13b 补上第三、四类**载荷内容**键（同样是建窗时注册的 initialization_script 的
        // 一部分，活窗口上换不掉）：
        // - 完美图标的**开关状态**（`injector::perfect_icon_enabled`）：它决定 14 个 PNG
        //   要不要进载荷（约 +1.0 MiB）；逐项键（scaleSelected/redrawKeys/redrawMap 的**具体
        //   取值**）不进这个判据——它们经 `apply_to_page` 即时生效，不必重建；
        // - 登录壁纸的**文件名**：它决定从配置目录读哪个文件、以什么键嵌进 `binaryAssets`。
        //   导入路径落盘的名字带内容指纹（`config::stored_wallpaper_name`），所以「换了图」
        //   必然表现为「名字变了」，这里比较文件名就够了。
        let needs_reload = next.shell.inject_enabled != prev.shell.inject_enabled
            || next.shell.home_url != prev.shell.home_url
            || injector::perfect_icon_enabled(&next) != injector::perfect_icon_enabled(&prev)
            || next.local.login_wallpaper_file_name != prev.local.login_wallpaper_file_name;
        *guard = next.clone();
        (next, needs_reload)
    };

    let path = Config::config_path();
    cfg.save(&path).map_err(|e| e.to_string())?;

    apply_to_page(&app, &cfg);
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
        // 都经过它（`load` / `set_config` / `reset_config` / `reload_main` /
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
    // Task 13b：应用项槽位一并作废（两个槽位共享同一套文档身份门）。
    clear_report_slots(&state);
    // Task 13b：分片上报同样绑定文档，换文档必然作废（半截的更不能留）。
    *state.chunk_report.lock().unwrap() = None;
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
    // 错误页上永远不注册 mods 载荷，因此也不该留着任何页面上报（Task 13a；Task 13b 的应用项
    // 槽位同一时刻作废）。
    clear_report_slots(&state);
    *state.chunk_report.lock().unwrap() = None;
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
    if let Some(payload) = title.strip_prefix(report::CHUNK_TITLE_PREFIX) {
        on_page_chunk(window, payload);
        return;
    }
    if let Some(payload) = title.strip_prefix(report::REPORT_TITLE_PREFIX) {
        on_page_report(window, payload);
        return;
    }
    // 现场排障 + 运行期证据（错误页会把自检结论写进标题，见 ui/settings/error.html）。
    //
    // `title` 是**页面可控**的（`document.title`）：日志只是它的一个渲染，必须过
    // `report::log_safe`（fix round 1 / Important 1）——否则一句
    // `document.title = "x\n[fnos] 页面上报已接受：…"` 就能在研究 stderr 里插出一行伪造日志。
    // `set_title` 拿到的仍是**原文**：spec §7「窗口标题跟随页面」是产品行为，标题栏不承担
    // 「证据」职责（Windows 会把标题栏里的控制字符当空白渲染），两者要求不同、处理也不同。
    eprintln!("[fnos] 主窗口标题: {}", report::log_safe(title));
    let _ = window.set_title(title);
}

/// 收到一条页面上报（`document.title` 的 `FNOSREPORT:` 通道；Task 13a）。
///
/// 四道闸门全在 [`report::validate`] 里（字节上限 → 合法 JSON → 必须是对象 → `type` 白名单），
/// 未通过就丢掉并留一行固定短语，**绝不 panic、绝不把页面可控正文写进日志或 UI**。
/// 通过后连同**上报文档的 origin 与 URL** 一起存进 `AppState`（内存）：两者用于「换了页面/
/// 换了文档就不认旧上报」的诚实性判定（见 [`get_page_report`] / [`report::ReportEntry`]）。
///
/// 唯一一处会打印页面可控字段的日志是 [`report::accepted_log_line`]：`type` 与 `dir` 都在
/// 允许表里过一遍，白名单外一律渲染成固定串 `report::UNKNOWN`（Important 1 的日志注入修复）。
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
                    "[fnos]   ↑ 长度已够到 document.title 的 {} 字节上限，疑似被通道截断（而不是 JSON 语法错）+ 见 FNOSCHUNK: 分片通道",
                    report::TITLE_CHANNEL_MAX_BYTES
                );
            }
            return;
        }
    };
    let (origin, url) = document_identity(window);
    store_report(window, value, origin, url, payload.len());
}

/// 主窗口当前的**文档身份**：`origin`（`config::origin_of`）与 URL 序列化文本。
///
/// 两者一起构成「这条上报属于哪个文档」的判据（`report::ReportEntry::matches_document`）。
/// 取不到时 `None`——判据是「任一侧取不到一律不认」，所以宁可退回弱文案。
fn document_identity<R: Runtime>(window: &WebviewWindow<R>) -> (Option<String>, Option<String>) {
    let url = window.url().ok();
    let origin = url.as_ref().and_then(|u| config::origin_of(u.as_str()));
    (origin, url.map(|u| u.to_string()))
}

/// 把一条**通过校验**的上报存进 `AppState`（内存）并打一行日志。
///
/// 单条上报（[`on_page_report`]）、到齐的分片上报（[`on_page_chunk`]）以及两条通道的
/// 任何 `type` 都共用这一段：两条通道的「接受」语义必须完全一样，否则设置窗读到的证据会
/// 因来源而异。
///
/// ## 分流（T13b；见 [`report::is_app_items_report`]）
///
/// 「应用项列表」那一类 `type`（`report::APP_ITEMS_TYPES`：`ITEMS` 与上游同一分支的 `TITLES`）
/// 进 [`AppState::app_items_report`]，**其余**进 [`AppState::page_report`]。这样 T13a 的强态判据
/// （最近一次状态槽上报是不是 `FNOS_INJECTION_TRIGGERED`）不会被 T13b 自己发起的列表拉取冲掉。
/// 两个槽位在 [`document_identity`] 上的门完全一致，读取口 [`get_page_report`] 一并返回。
///
/// 日志只能打印白名单里的字段：`type` / `dir` 不在允许表里时渲染成固定串
/// （[`report::accepted_log_line`]），`origin` 由宿主解析、再折成一行（`log_safe`）。
fn store_report<R: Runtime>(
    window: &WebviewWindow<R>,
    value: Value,
    origin: Option<String>,
    url: Option<String>,
    bytes: usize,
) {
    let ty = value.get("type").and_then(Value::as_str).unwrap_or("");
    let dir = value.get("dir").and_then(Value::as_str);
    eprintln!(
        "{}",
        report::accepted_log_line(ty, dir, origin.as_deref(), bytes)
    );
    let state = window.state::<AppState>();
    let entry = report::ReportEntry { value, origin, url };
    let slot = if report::is_app_items_report(&entry.value) {
        &state.app_items_report
    } else {
        &state.page_report
    };
    *slot.lock().unwrap() = Some(entry);
}

/// 清空两个上报槽位（状态槽 + 应用项槽）——它们共享同一套「换文档就作废」的时机。
fn clear_report_slots(state: &AppState) {
    *state.page_report.lock().unwrap() = None;
    *state.app_items_report.lock().unwrap() = None;
}

/// 收到**一片**分片上报（`document.title` 的 `FNOSCHUNK:` 通道；Task 13b）。
///
/// 三道处理顺序刻意固定：先在锁内做「喂一片」（它会做全部边界判定，并在拒绝/完成时清空
/// 半截状态），再在锁外决定日志与存储——锁不跨越任何可能变慢的动作。
///
/// 到齐后**仍然**走 [`report::validate`]：分片通道不是绕过单条通道校验的后门
/// （字节上限、必须是 JSON 对象、`type` 必须在允许表内）。
fn on_page_chunk<R: Runtime>(window: &WebviewWindow<R>, payload: &str) {
    let (origin, url) = document_identity(window);
    let step = {
        let state = window.state::<AppState>();
        let mut slot = state.chunk_report.lock().unwrap();
        report::accept_chunk(
            &mut slot,
            origin.as_deref(),
            url.as_deref(),
            payload,
            std::time::Instant::now(),
        )
    };
    match step {
        report::ChunkStep::Accepted { seq, total } => {
            // `seq` / `total` 都是宿主自己解析出来的数字（不是页面可控文本），可以作为日志字段
            eprintln!("[fnos] 分片上报已接受：seq={seq} total={total}");
        }
        report::ChunkStep::Rejected(reason) => {
            eprintln!(
                "[fnos] 分片上报被拒（{}；这一片 {} 字节）",
                reason.as_str(),
                payload.len()
            );
        }
        report::ChunkStep::Complete(assembled) => {
            match report::validate(&assembled) {
                Ok(value) => {
                    eprintln!("[fnos] 分片上报已到齐（{} 字节）", assembled.len());
                    store_report(window, value, origin, url, assembled.len());
                }
                Err(reason) => {
                    // 组装完成但内容不合法：同样只留固定短语（不打印正文）
                    eprintln!(
                        "[fnos] 分片上报组装后被拒（{}；{} 字节）",
                        reason.as_str(),
                        assembled.len()
                    );
                }
            }
        }
    }
}

/// 新文档与最近一次上报**不是同一个文档**（origin 或 URL 不同）→ 丢掉旧上报。
///
/// 主窗口可以在不改任何配置的情况下换页面（页内链接、`reload_main` 的显式地址、重定向，
/// 甚至是同一个白名单 origin 下的另一个文档）。旧页面上报的「已注入」不能拿来描述新页面——
/// `get_page_report` 也会再判一次，这里是「新文档到达时顺手清掉」，免得一条过期证据一直
/// 躺在内存里。
///
/// fix round 1 / Minor 3：判据从「只看 origin」收紧为「origin **与文档 URL** 都对得上」
/// （[`report::ReportEntry::matches_document`]）。同源换文档时第二个文档完全可能没注入
/// （缺 fnOS 签名），旧证据留在设置窗上就是拿上一张页面描述这一张。
///
/// Task 13b：两个上报槽位（状态槽 + 应用项槽）用的是**同一个**文档身份门，所以一起判、一起清。
fn drop_report_if_document_changed<R: Runtime>(app: &AppHandle<R>, url: &str) {
    let current_origin = config::origin_of(url);
    let state = app.state::<AppState>();
    for slot in [&state.page_report, &state.app_items_report] {
        let mut guard = slot.lock().unwrap();
        let stale = match guard.as_ref() {
            Some(entry) => !entry.matches_document(current_origin.as_deref(), Some(url)),
            None => false,
        };
        if stale {
            *guard = None;
        }
    }
    // Task 13b：分片通道有同样的「文档身份」判据，但它**自己**也判一次（`accept_chunk`
    // 在文档对不上时拒绝续传）。这里顺手清掉，让「换了文档之后内存里立刻没有半截载荷」
    // 成为与读取路径无关的性质。
    let mut chunk = state.chunk_report.lock().unwrap();
    let chunk_stale = match chunk.as_ref() {
        Some(assembly) => !assembly.belongs_to(current_origin.as_deref(), Some(url)),
        None => false,
    };
    if chunk_stale {
        *chunk = None;
    }
}

/// 一个上报槽位在当前文档下的可读副本：文档对不上就**就地清掉**并返回 `None`。
///
/// 「及时丢掉」的第二层：页内导航不走 [`begin_load`]，`on_page_event` 也可能因为世代闸门
/// 而迟到，所以读取路径自己再判一次——判到就清，绝不把旧证据给设置窗。
fn read_slot_for_current_document(
    slot: &Mutex<Option<report::ReportEntry>>,
    origin: Option<&str>,
    url: Option<&str>,
) -> Option<Value> {
    let mut guard = slot.lock().unwrap();
    let stale = match guard.as_ref() {
        Some(entry) => !entry.matches_document(origin, url),
        None => false,
    };
    if stale {
        *guard = None;
        return None;
    }
    guard.as_ref().map(|entry| entry.value.clone())
}

/// 读页面上报（`get_page_report` 的返回体）。
///
/// **只授予设置窗**（`capabilities/default.json`），与 `get_page_state` 同一档：主窗口
/// （含内置错误页）拿不到任何命令授权，所以页面永远无法自己读取或改写这些证据。
///
/// ## 返回体（Task 13b 起是**信封**，不再是单条上报）
///
/// ```json
/// { "report": <最近一条状态相关上报 | null>, "appItems": <最近一条应用项列表上报 | null> }
/// ```
///
/// 为什么要拆两个槽位而不是「最近一条」：T13b 让 shim 在完美图标启用时主动拉取应用项列表，
/// 那条上报会晚于上游的 `FNOS_INJECTION_TRIGGERED`——同槽会把 T13a 的强态判据冲掉（功能
/// 回归）。分流规则的完整论证见 [`report::is_app_items_report`]。
///
/// 两个字段各自独立地受「上报来源 origin == 主窗口当前 origin」**且**「上报时的文档 URL ==
/// 当前文档 URL」的门约束（[`report::ReportEntry::matches_document`]）：取不到当前 URL/origin
/// （窗口已销毁 / 导航还没完成）或文档已换，对应槽位一律清掉并返回 `null`，设置窗于是退回
/// Task 11 的弱文案——宁可不说，也不谎称已注入。
///
/// 返回体**永远是对象**（两个字段都可以是 `null`），调用方不需要再判「回包是不是 null」。
#[tauri::command]
pub fn get_page_report<R: Runtime>(app: AppHandle<R>) -> Value {
    let current_url = app
        .get_webview_window(MAIN_WINDOW)
        .and_then(|w| w.url().ok());
    let current_origin = current_url
        .as_ref()
        .and_then(|u| config::origin_of(u.as_str()));
    let current_url = current_url.map(|u| u.to_string());
    let state = app.state::<AppState>();
    let report = read_slot_for_current_document(
        &state.page_report,
        current_origin.as_deref(),
        current_url.as_deref(),
    );
    let app_items = read_slot_for_current_document(
        &state.app_items_report,
        current_origin.as_deref(),
        current_url.as_deref(),
    );
    serde_json::json!({ "report": report, "appItems": app_items })
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
    // Task 13a：页内导航（`reload_main` 的 `navigate`、页面里的链接、重定向）**不走**
    // `begin_load`，所以上面那段带世代闸门的代码会直接 return——清理必须放在闸门**之前**，
    // 否则新文档与旧上报不同文档（换 origin，或同一个 origin 下的另一篇文档）时那条证据会
    // 一直留着。`get_page_report` 读取时还会再判一次（那时的 URL 已经是新文档），两层都必要：
    // 这里负责「及时丢掉」，那里负责「绝不误报」。
    drop_report_if_document_changed(app, url);
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
/// → 套回显示态 → 清标志（T14a 前这里还要 `sync_menus`，菜单改为无状态后已删除）。
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
/// Task 11：这条路径也是**唯一**的「重试」实现（设置窗状态条、托盘「重新加载」、
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

/// 托盘「重新加载」：按配置的 `homeUrl` 重建主窗口（从错误页恢复的正路之一）。
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
    // 与 `set_config` 同一判据：reset 也可能把 `injectEnabled` / `homeUrl` 拉回默认值，
    // 同样必须重建主窗口才算生效。
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

// ---------- Task 13b：登录壁纸导入 ----------

/// 校验并落盘一份登录壁纸，返回 `(落盘用的文件名, 字节数)`。
///
/// 拆成独立函数是为了能在**临时目录**上单测全部拒绝路径（不必碰真实配置目录）。
/// 五道闸门按「先便宜后昂贵」排序，任何一步失败都**不落盘**，且错误信息里不回显用户输入的
/// 文件名（它是设置窗来的文本；回显只会把它带进 UI/日志）：
///
/// 1. [`config::wallpaper_mime`]：形状（无分隔符/无 `..`/非隐藏文件）+ 扩展名允许表
///    （`png` / `jpg` / `jpeg` / `webp`）——与设置窗 `<input accept>` 同一张表；
/// 2. base64 字符串长度上限（先卡长度，避免给一个几百 MB 的串做无谓解码）；
/// 3. [`crate::base64::decode`] 严格解码；
/// 4. 解码后的字节数上限（[`config::MAX_WALLPAPER_BYTES`]，与注入侧同一个常量）；
/// 5. 落盘名 = [`config::stored_wallpaper_name`]（净化 + 内容指纹），写进 `dir`。
fn store_wallpaper(
    dir: &std::path::Path,
    name: &str,
    data_base64: &str,
) -> Result<(String, usize), String> {
    if !config::is_wallpaper_name(name) {
        return Err("只支持 png / jpg / jpeg / webp，且文件名不能含路径分隔符".to_string());
    }
    // 上面已经保证扩展名在允许表内；这里再取一次是为了做「扩展名与内容一致」的检查
    let ext = config::wallpaper_ext(name).ok_or_else(|| "扩展名不在允许表内".to_string())?;
    let max_b64 = (config::MAX_WALLPAPER_BYTES + 2) / 3 * 4;
    if data_base64.len() > max_b64 {
        return Err(format!(
            "图片超过 {} MiB 上限",
            config::MAX_WALLPAPER_BYTES / 1024 / 1024
        ));
    }
    let bytes =
        base64::decode(data_base64).map_err(|e| format!("图片数据不是合法的 base64：{e}"))?;
    if bytes.is_empty() {
        return Err("图片内容为空".to_string());
    }
    if bytes.len() > config::MAX_WALLPAPER_BYTES {
        return Err(format!(
            "图片超过 {} MiB 上限",
            config::MAX_WALLPAPER_BYTES / 1024 / 1024
        ));
    }
    // 扩展名与内容的一致性只做「够用」的检查：PNG 要求魔数，其余的交给 WebView2 解码失败时
    // 表现为「壁纸没生效」（不额外拒绝，避免把正常的 JPEG 变体挡在门外）。
    if ext == "png" && !bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Err("文件内容不是 PNG（扩展名与内容不符）".to_string());
    }
    let stored = config::stored_wallpaper_name(name, &bytes);
    std::fs::create_dir_all(dir).map_err(|e| format!("配置目录不可写：{e}"))?;
    std::fs::write(dir.join(&stored), &bytes).map_err(|e| format!("写入配置目录失败：{e}"))?;
    Ok((stored, bytes.len()))
}

/// 导入登录壁纸（Task 13b）：本项目**唯一**会按 UI 请求写文件的 IPC 命令，只授予设置窗。
///
/// 为什么要宿主代写：设置窗是普通 webview，没有（也不该有）文件系统权限；而壁纸必须落在
/// **配置目录**里——`injector::load_wallpaper` 只从那里按 `local.loginWallpaperFileName` 读。
/// 于是设置窗把选中的文件读成 base64 交给这里，宿主校验后写盘，并返回**落盘用的名字**；
/// UI 再走既有的 `set_config` 写 `local.loginWallpaperFileName`（导入本身不改任何配置，
/// 两步都由设置窗发起，用户看得见）。
///
/// 落盘名由内容指纹决定（`config::stored_wallpaper_name`），于是「换了图」必然表现为
/// 「配置里的名字变了」——`set_config` 的 `needsReload` 判据因此能发现它，下一次建窗
/// 载荷里嵌的就是新图（需求 A 的壁纸那一半）。
///
/// 参数名走 tauri 的默认 camelCase 约定：JS 侧传 `{ name, dataBase64 }`
/// （`ui/settings/bridge.js::importWallpaper`）。
#[tauri::command]
pub fn import_wallpaper(name: String, data_base64: String) -> Result<String, String> {
    let (stored, bytes) = store_wallpaper(&paths::config_dir(), &name, &data_base64)?;
    // `stored` 是宿主自己算出来的 ASCII 名字（`[a-z0-9-_]` + 扩展名），不含换行、可以直接进日志
    eprintln!("[fnos] 登录壁纸已导入：{stored}（{bytes} 字节）");
    Ok(stored)
}

// ---------- Task 14b：设置窗本地存储 + 「请页面重新汇报应用项」 ----------

/// 设置窗本地存储（`local-store.json`）的键名上限。
pub const MAX_LOCAL_STORE_KEYS: usize = 32;
/// 单值上限。上游唯一真正需要它的是「更新检查状态」（几百字节）；64 KiB 远大于它，
/// 又远小于任何字体文件（本壳不提供字体导入，见 [`LOCAL_STORE_REFUSED_HINT`]）。
pub const MAX_LOCAL_STORE_VALUE_BYTES: usize = 64 * 1024;
/// 整份存储的上限（键 + 值字节数之和）。
pub const MAX_LOCAL_STORE_TOTAL_BYTES: usize = 256 * 1024;

/// 键名形状：一小撮 ASCII 字符，**不含任何路径分隔符、不以 `.` 开头、不含 `..`**。
///
/// 为什么单独一道闸门：这个存储的存在理由就是「上游 popup 需要一份扩展本地状态」，
/// 它的键永远来自上游源码里的字符串常量（`updateCheckState` 等）。把键限成「不可能是
/// 路径」的形状之后，「键 → 文件路径」这类用法在这一层根本表达不出来——宿主只按固定
/// 文件名 [`local_store_path`] 落盘，键只当 JSON 对象的字段名。
/// 前导点一并拒掉（与 `config::is_wallpaper_name` 拒隐藏文件同一种纪律）：`.` 与 `..`
/// 都是目录项，不该出现在一个「键」的位置上。
fn is_local_store_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 64 || key.starts_with('.') {
        return false;
    }
    key.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
        && !key.contains("..")
}

/// 说明文案（拒绝字体数据时也走这里，保证「为什么拒」只有一处事实来源）。
pub const LOCAL_STORE_REFUSED_HINT: &str =
    "本外壳的设置窗本地存储只保存上游 popup 的扩展本地状态（字符串、有大小上限），不接受字体数据";

fn local_store_path() -> std::path::PathBuf {
    paths::config_dir().join("local-store.json")
}

/// 读本地存储：永远是「键 → 字符串」的映射（值由设置窗自己 JSON 编码，宿主不解释它）。
///
/// 文件不存在 / 读不动 / 不是 JSON 对象 → 空存储。**损坏的文件会被改名保留**为
/// `local-store.json.bak`（与 `Config::load_with_report` 对 `config.json` 的做法一致）：
/// 一次静默的重置会让人以为「设置自己丢过」，而留下 .bak 至少可查。
fn load_local_store(path: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    let empty = std::collections::BTreeMap::new();
    let Ok(raw) = std::fs::read_to_string(path) else {
        return empty; // 还没有这份文件是**正常**状态（首次运行）
    };
    let parsed: Option<std::collections::BTreeMap<String, String>> =
        serde_json::from_str(&raw).ok();
    match parsed {
        Some(map) => map,
        None => {
            let bak = path.with_extension("json.bak");
            let _ = std::fs::rename(path, &bak);
            eprintln!(
                "[fnos] 设置窗本地存储损坏，已改名为 {} 并回到空存储",
                bak.display()
            );
            empty
        }
    }
}

/// 落盘（临时文件 + 改名，避免半个文件）。
fn save_local_store(
    path: &std::path::Path,
    store: &std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("配置目录不可写：{e}"))?;
    }
    let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("写入本地存储失败：{e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("替换本地存储失败：{e}"))
}

/// 把一次 patch 套到 store 上（纯函数，便于单测）。
///
/// **先整份校验、再整份应用**：任何一条不合法都返回 `Err` 且**不改动** store
/// ——「一半写入成功」比「整次失败」难排查得多，而上游每次都只发一个键。
fn apply_local_store_patch(
    store: &mut std::collections::BTreeMap<String, String>,
    patch: &std::collections::BTreeMap<String, Option<String>>,
) -> Result<(), String> {
    for (key, value) in patch {
        if !is_local_store_key(key) {
            return Err(
                "本地存储键名不合法（只允许 ASCII 字母/数字与 _ - . :，长度 ≤ 64，且不含路径分隔符）"
                    .to_string(),
            );
        }
        if let Some(text) = value {
            if text.len() > MAX_LOCAL_STORE_VALUE_BYTES {
                return Err(format!(
                    "本地存储单个值超过 {} KiB 上限（{LOCAL_STORE_REFUSED_HINT}）",
                    MAX_LOCAL_STORE_VALUE_BYTES / 1024
                ));
            }
        }
    }

    let mut next = store.clone();
    for (key, value) in patch {
        match value {
            None => {
                next.remove(key);
            }
            Some(text) => {
                next.insert(key.clone(), text.clone());
            }
        }
    }
    if next.len() > MAX_LOCAL_STORE_KEYS {
        return Err(format!("本地存储最多 {MAX_LOCAL_STORE_KEYS} 个键"));
    }
    let total: usize = next.iter().map(|(k, v)| k.len() + v.len()).sum();
    if total > MAX_LOCAL_STORE_TOTAL_BYTES {
        return Err(format!(
            "本地存储总量超过 {} KiB 上限（{LOCAL_STORE_REFUSED_HINT}）",
            MAX_LOCAL_STORE_TOTAL_BYTES / 1024
        ));
    }
    *store = next;
    Ok(())
}

/// 读设置窗本地存储（Task 14b）。**只授设置窗**，且在 `remote-deny.json` 里显式 deny。
///
/// 返回 `{键: 字符串}`：值由设置窗自己 JSON 编码（上游的 `updateCheckState` 是个对象），
/// 宿主不解释它——这样「宿主只存字符串」与「上游存的是对象」两个事实不会互相污染。
#[tauri::command]
pub fn get_local_store() -> Value {
    let store = load_local_store(&local_store_path());
    serde_json::to_value(store).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

/// 写设置窗本地存储（Task 14b）。`null` 值 = 删除该键（与 `commands::merge` 的语义一致）。
///
/// 返回**整份**存储，调用方据此把它当权威值（不猜、不合并自己那份缓存）。
#[tauri::command]
pub fn set_local_store(
    patch: std::collections::BTreeMap<String, Option<String>>,
) -> Result<Value, String> {
    let path = local_store_path();
    let mut store = load_local_store(&path);
    apply_local_store_patch(&mut store, &patch)?;
    save_local_store(&path, &store)?;
    Ok(serde_json::to_value(store).unwrap_or_else(|_| Value::Object(serde_json::Map::new())))
}

/// 请主窗口页面**重新**汇报一次「启动台应用项列表」（Task 14b）。
///
/// 为什么需要它：上游 popup 的逐项 UI 会主动
/// `chrome.tabs.sendMessage({type:'FNOS_GET_LAUNCHPAD_APP_ITEMS'})` 去要列表；而在本壳里，
/// 那份列表只能由**页面侧**的 shim 主动向上游请求、再经标题通道回报
///（`inject/shim.js::requestAppItems`，它只在页面加载时启动一轮有限重试）。设置窗开得比
/// 那一轮晚就永远拿不到列表，用户会一直看到「未读取到应用，先打开启动台再试」——一句
/// 无法兑现的话。这条命令把「再问一次」变成可能：设置窗先读已有的上报槽位，槽位是空的
/// 才调它，然后在一个**有界**的窗口内轮询（`ui/settings/app.js::answerAppItems`）。
///
/// 安全性：**无参数**——不接受任何脚本文本，因此它不是注入面（eval 的是本壳自己注入的
/// 那一行固定代码），也只授设置窗、并在 `remote-deny.json` 里显式 deny。
///
/// 日志只说**宿主做过的事**（fix round 1 / Minor 5）：`WebviewWindow::eval` 是单向的，宿主
/// 拿不到页面里那个钩子的返回值——它可能因为「完美图标没配置 / 页面侧没有钩子 / 3 次额度
/// 用尽」而**什么都不做**（`inject/shim.js::W.__FNOS_REQUEST_APP_ITEMS__`）。旧文案
/// 「已请主窗口页面重新汇报启动台应用项列表」在没有钩子时就是一句没发生过的陈述。页面侧
/// 是否真的把请求送出去，只能由页面自己的上报槽位体现（`get_page_report`）。
#[tauri::command]
pub fn request_app_items<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let win = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or_else(|| "主窗口未打开".to_string())?;
    win.eval("window.__FNOS_REQUEST_APP_ITEMS__ && window.__FNOS_REQUEST_APP_ITEMS__();")
        .map_err(|e| format!("请求页面汇报应用项失败：{e}"))?;
    // 运行期证据（与建窗加载日志同风格）：证明设置窗的请求真的**下发到了页面**。
    // 「页面有没有真的送出去」不在这里断言（宿主观测不到），见上面的注释。
    eprintln!(
        "[fnos] 已向主窗口页面下发重新汇报应用项的钩子调用（页面侧无钩子/无请求可发时不会送出）"
    );
    Ok(())
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
        app_items_report: Mutex::new(None),
        chunk_report: Mutex::new(None),
    });
}

pub fn save_window_geom<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    // 最小化时**不能**保存几何：Windows 对最小化窗口报 `inner_size = 0x0`、
    // `outer_position = -32000,-32000`，存下去就把下一次启动的窗口放到屏幕外
    //（实测：最小化后 WM_CLOSE → config 里留下 w=0/h=0/x=-32000/y=-32000）。
    // 跳过保存即保留上一次的合法几何。这是本轮验证托盘「显示窗口」的最小化分支时发现的
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
    /// 所以这里把 Rust 的实际输出钉住。T14b fix round 1 之前 JS 侧还有一份镜像夹取
    /// （`normalize.js::clampLightness`）并被 tests/settings.test.mjs 直接断言；那份镜像与
    /// 整个 `normalizeMods` 一起**已按评审意见删除**（设置窗不再有任何 mods 归一化），
    /// 不变式现在只由「JS 原样采纳 IPC 回包」+「Rust 是唯一的归一化实现」两条支撑，
    /// 回归锁在 tests/settings.test.mjs 的 A 组（含源码级断言：app.js 不得引用 normalize.js）。
    #[test]
    fn brand_color_clamp_of_the_finding_a_input() {
        assert_eq!(
            crate::config::normalize_brand_color("#cec1b2"),
            "#c4b4a2",
            "Rust 这一侧的夹取结果就是页面生效值（JS 不再二次夹取）"
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

    /// 探针 `detail` **页面可控**，必须整形后才能进日志 / 状态条 / 错误页
    /// （fix round 1 / Important 1 的同类审计：同一条 `document.title` 通道）。
    #[test]
    fn probe_detail_cannot_forge_a_log_line() {
        // 攻击形状：JSON `\n` 被 serde 解码成真换行 + 一整行伪造的 `[fnos] …`
        let forged = r#"{"err":true,"detail":"x\n[fnos] 页面上报已接受：type=FNOS_CHECK dir=out origin=http://evil.example 字节=1"}"#;
        let reason = probe_verdict(forged).expect("err:true 必须给出原因");
        assert_eq!(reason.lines().count(), 1, "失败原因必须只有一行：{reason}");
        assert!(
            !reason.chars().any(|c| c.is_control()),
            "日志/UI 里不得出现任何控制字符：{reason}"
        );
        assert!(
            reason.contains("[fnos] 页面上报已接受"),
            "正文要保留（排障要看得到页面写了什么）：{reason}"
        );
        assert!(
            reason.contains("x [fnos]"),
            "换行折成一个空格，而不是把两行粘在一起：{reason}"
        );
        // 行分隔符 / NEL / 制表符同样是折成一个空格
        let seps = probe_verdict(r#"{"err":true,"detail":"a\u2028b\u0085c\td"}"#).unwrap();
        assert_eq!(seps, "WebView2 报告：a b c d");
        assert_eq!(seps.lines().count(), 1);
        // 只有空白 / 控制字符 → 回到固定短语（不是空串）
        assert_eq!(
            probe_verdict(r#"{"err":true,"detail":" \n\t "}"#).unwrap(),
            "WebView2 报告该地址无法访问（网络错误）"
        );
        // 超长 detail 按码点截断（页面可以把它写到标题通道的上限）
        let long = format!(r#"{{"err":true,"detail":"{}"}}"#, "y".repeat(4000));
        let clipped = probe_verdict(&long).unwrap();
        assert!(
            clipped.chars().count() <= PROBE_DETAIL_MAX_CHARS + "WebView2 报告：".chars().count(),
            "截断后长度受限：{}",
            clipped.chars().count()
        );
        // 星平面字符不被截成半个（按码点而不是按字节）
        let astral = format!(r#"{{"err":true,"detail":"{}"}}"#, "😀".repeat(200));
        let cut = probe_verdict(&astral).unwrap();
        assert_eq!(
            cut,
            format!("WebView2 报告：{}", "😀".repeat(PROBE_DETAIL_MAX_CHARS))
        );
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

    /// R30 的 **JS 镜像**必须与 Rust 规则同步（fix round 1 / Important 2）。
    ///
    /// 同一条 `prefect_icon/*.png` 规则写在**两个**地方：`ui/settings/normalize.js` 的
    /// `PREFECT_ICON_PATH` 与 Rust 归一化时的判定（`config.rs::is_valid_prefect_icon_path`
    /// ← `Config::normalize` 的 `retain`）。任何一个落后于另一个都是**静默丢配置**：
    /// fix round 1 之前 JS 用的是全小写正则，于是 `prefect_icon/Emby.png`
    /// （磁盘上真实存在的 `panIndex.png` 这类名字）会在设置窗那一侧先被丢掉，Rust 的放宽
    /// 规则永远见不到它。
    ///
    /// T14b fix round 1：JS 侧的**生产消费者**已经没有了（退休的 `schema.js` 是最后一个调用
    /// `isPrefectIconPath` / `normalizeModsEntry` 的地方，两个函数连同 `normalizeMods` 一起
    /// 按评审意见删除）。常量与它的大小写语义**保留**，因为它仍是「同一份事实被写在两种语言
    /// 里」的那份镜像，而 `ui/settings/chrome-shim.js::getURL` 现在也依赖同一条大小写不敏感语义
    /// （内嵌资产表大小写敏感，两个名字都要解析到同一份字节）；锁法照旧：逐字锁正则文本 +
    /// 读 `tests/normalize.test.mjs` 的输入表逐行核对。
    ///
    /// 用与 `report_prefix_and_shim_agree` 同一手法做两件事：
    /// ① 逐字锁住 JS 的正则文本（**必须带 `i`**——那正是 R30 要修的那一处）；
    /// ② 读 `tests/normalize.test.mjs` 的**输入表**，逐行断言 20 条与
    ///    `config.rs::tests::redraw_map_regex_is_case_insensitive_only` 同形、同序、同期望
    ///    （`config.rs` 不在本轮改动范围内，所以「两侧同步」的机械锁落在这里）。
    #[test]
    fn prefect_icon_rule_mirror_stays_in_step() {
        let js = include_str!("../../ui/settings/normalize.js");
        assert!(
            js.contains("PREFECT_ICON_PATH = /^prefect_icon\\/[a-z0-9-]+\\.png$/i;"),
            "JS 镜像的正则必须与 Rust 规则同形且带 `i`（大小写不敏感，R30）"
        );
        // 镜像不得只有常量而无人核对：那张输入表（本函数下半段逐行锁着）就是它的实测判据。
        let shim = include_str!("../../ui/settings/chrome-shim.js");
        assert!(
            shim.contains("canonicalAssetPath(raw)") && shim.contains("raw.toLowerCase()"),
            "chrome-shim 的 getURL 必须把路径折成小写（与页面侧 shim / 本条镜像同一条语义）"
        );
        let table = include_str!("../../tests/normalize.test.mjs");
        // 与 config.rs 那份用例**逐行相同**的输入表（顺序也相同）。
        let rows: [(&str, bool); 20] = [
            ("prefect_icon/emby.png", true),
            ("prefect_icon/home-assistant.png", true),
            ("prefect_icon/a1-b2.png", true),
            ("prefect_icon/Emby.png", true),
            ("prefect_icon/emby.PNG", true),
            ("PREFECT_ICON/emby.PnG", true),
            ("Prefect_Icon/Home-Assistant.PNG", true),
            ("../x", false),
            ("prefect_icon/a.png.png", false),
            ("prefect_icon/", false),
            ("prefect_icon/sub/dir.png", false),
            ("other/emby.png", false),
            ("prefect_icon\\\\emby.png", false),
            ("prefect_icon/em by.png", false),
            ("prefect_icon/.png", false),
            ("prefect_icon/emby.png ", false),
            (" prefect_icon/emby.png", false),
            ("prefect_icon/图标.png", false),
            ("图标", false),
            ("prefect_icon/图的.png", false),
        ];
        for (v, want) in rows {
            let needle = format!("row('{v}', {want},");
            assert!(
                table.contains(&needle),
                "tests/normalize.test.mjs 的输入表缺少/改动了这一行（两侧必须同步）：{needle}"
            );
        }
    }

    // ---------- Task 13b：登录壁纸导入 ----------

    /// 每个测试一个独立临时目录（按 tag + pid 命名），与 config.rs 同一套做法。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fnos-cmd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// PNG 魔数 + 一点点数据（够 `store_wallpaper` 的魔数检查）。
    fn png_bytes() -> Vec<u8> {
        let mut v = vec![0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        v.extend_from_slice(&[1, 2, 3, 4, 5]);
        v
    }

    /// 愉快路径：落盘名是宿主自己算的（净化 + 内容指纹）、内容逐字节相等、幂等。
    #[test]
    fn import_wallpaper_writes_the_bytes_under_a_derived_name() {
        let dir = temp_dir("wallpaper-import");
        let bytes = png_bytes();
        let b64 = base64::encode(&bytes);

        let (stored, len) = store_wallpaper(&dir, "我的壁纸 (1).png", &b64).expect("应当成功");
        assert_eq!(len, bytes.len());
        assert!(
            stored.starts_with("wallpaper-1-") || stored.starts_with("1-"),
            "stem 里的非 ASCII 被净化成连字符：{stored}"
        );
        assert!(stored.ends_with(".png"), "{stored}");
        assert!(
            config::is_wallpaper_name(&stored),
            "落盘名必须是安全形状：{stored}"
        );
        let written = std::fs::read(dir.join(&stored)).expect("文件必须真的落盘");
        assert_eq!(written, bytes, "落盘内容必须逐字节相等");

        // 幂等：同一份内容 + 同一个原始名 → 同一个落盘名（不重复堆文件）
        let again = store_wallpaper(&dir, "我的壁纸 (1).png", &b64).expect("应当成功");
        assert_eq!(again.0, stored);
        let count = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, 1, "同一份内容不该留下两个文件");

        // 内容变了 → 名字变了（这是 `needsReload` 能发现「换了图」的前提）
        let mut other = bytes.clone();
        other.push(9);
        let (stored2, _) =
            store_wallpaper(&dir, "我的壁纸 (1).png", &base64::encode(&other)).unwrap();
        assert_ne!(stored2, stored);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);

        // 目录不存在时自己建（配置目录可能被用户删掉）
        let nested = dir.join("nested/deeper");
        let (inner, _) = store_wallpaper(&nested, "a.png", &b64).expect("应当自己建目录");
        assert!(nested.join(&inner).exists(), "嵌套目录里的文件必须真的存在");

        // 扩展名按原样保留
        for (name, ext) in [
            ("a.jpg", "jpg"),
            ("a.jpeg", "jpeg"),
            ("a.webp", "webp"),
            ("a.PNG", "png"),
        ] {
            let body = if ext == "png" {
                bytes.clone()
            } else {
                vec![0xff, 0xd8, 0xff]
            };
            let (n, _) = store_wallpaper(&dir, name, &base64::encode(&body)).unwrap();
            assert!(n.ends_with(&format!(".{ext}")), "{name} -> {n}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 拒绝路径：每一条都**不落盘**，且错误信息里不回显用户输入的文件名。
    #[test]
    fn import_wallpaper_rejects_every_bad_shape_without_writing() {
        let dir = temp_dir("wallpaper-reject");
        let b64 = base64::encode(&png_bytes());

        // ① 扩展名 / 形状
        for bad in [
            "anim.gif",
            "wallpaper",
            "wallpaper.svg",
            "../evil.png",
            "sub/evil.png",
            "sub\\evil.png",
            "C:\\evil.png",
            "evil\nname.png",
            ".png",
        ] {
            assert!(
                store_wallpaper(&dir, bad, &b64).is_err(),
                "bad={bad:?} 必须被拒"
            );
        }
        // 错误信息不得回显用户输入（它会直接进设置窗的界面）
        let err = store_wallpaper(&dir, "CANARY-canary.png\n", &b64).expect_err("必须拒绝");
        assert!(!err.contains("CANARY"), "错误信息不得回显用户输入：{err}");
        assert_eq!(err.lines().count(), 1, "错误信息必须只有一行：{err}");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "被拒的导入不得留下任何文件"
        );

        // ② base64 本身不合法（长度 / 字母表 / padding）
        for bad in ["!!!", "AAAAA", "AA=A", "AAA", "AA==AAA"] {
            assert!(
                store_wallpaper(&dir, "a.png", bad).is_err(),
                "bad={bad:?} 必须被拒"
            );
        }
        // ③ 空内容
        assert!(store_wallpaper(&dir, "a.png", "").is_err());
        // ④ 扩展名与内容不符（声明 png 但不是 PNG 魔数）
        let jpeg = base64::encode(&[0xffu8, 0xd8, 0xff, 0xe0, 0x00]);
        assert!(store_wallpaper(&dir, "a.png", &jpeg).is_err());
        // ⑤ 超过 8 MiB：先被 base64 长度闸门挡下（不进入解码，也就不会分配几十 MB）
        let max_b64 = (config::MAX_WALLPAPER_BYTES + 2) / 3 * 4;
        let huge = "A".repeat(max_b64 + 1);
        let err = store_wallpaper(&dir, "a.png", &huge).expect_err("超限必须被拒");
        assert!(err.contains("8 MiB"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Task 13b 的注册不变式：每个 IPC 命令都必须在**四**个地方出现，且授权面没有扩大。
    ///
    /// 这四条正好是「漏一个」的四种表现：漏 `build.rs` = **构建失败**；漏 `main.rs` = 命令不
    /// 存在；漏 `default.json` = 设置窗调用被 ACL 拒绝（运行期静默失败）；漏 `remote-deny.json`
    /// = 少一层纵深防御。Task 13a 的静态检查脚本（`_t13a-capcheck.ps1`）做的是同一件事，
    /// 这里把它固化成一个每次 `cargo test` 都会跑的机械锁。
    #[test]
    fn every_ipc_command_is_registered_in_all_four_places() {
        const CMDS: [&str; 12] = [
            "get_config",
            "set_config",
            "reload_main",
            "open_config_dir",
            "open_url",
            "reset_config",
            "get_page_state",
            "get_page_report",
            "import_wallpaper",
            "get_local_store",
            "set_local_store",
            "request_app_items",
        ];
        let build = include_str!("../build.rs");
        let main_rs = include_str!("main.rs");
        let default_json = include_str!("../capabilities/default.json");
        let remote_deny = include_str!("../capabilities/remote-deny.json");
        for cmd in CMDS {
            assert!(
                build.contains(&format!("\"{cmd}\"")),
                "build.rs 的 AppManifest::commands 缺少 {cmd}（漏了会构建失败）"
            );
            assert!(
                main_rs.contains(&format!("commands::{cmd}")),
                "main.rs 的 invoke_handler 缺少 {cmd}"
            );
            let kebab = cmd.replace('_', "-");
            assert!(
                default_json.contains(&format!("\"allow-{kebab}\"")),
                "capabilities/default.json 缺少 allow-{kebab}（设置窗会被 ACL 拒绝）"
            );
            if cmd == "open_url" {
                // **既有的、已知的**覆盖缺口（T9 的评审已记录，T13a 的描述里也写明「留待整支
                // 评审 triage，本轮不扩大范围」）：`open_url` 同样没有对远程的授权，所以少一条
                // deny 不构成授权面。本轮不顺手改它——那会把「谁决定的、为什么」变得含糊。
                // 这里显式钉住「只有它是例外」，将来补上 deny 时这条断言会失败并提醒更新描述。
                assert!(
                    !remote_deny.contains("\"deny-open-url\""),
                    "open_url 的 deny 缺口是已知项；若要补上，请同时更新 remote-deny.json 的描述与本断言"
                );
                continue;
            }
            assert!(
                remote_deny.contains(&format!("\"deny-{kebab}\"")),
                "capabilities/remote-deny.json 缺少 deny-{kebab}"
            );
        }
        // 授权面：设置窗的 allow-* 条数恰好等于命令数，远程那份**一个授权都没有**
        assert_eq!(
            default_json.matches("\"allow-").count(),
            CMDS.len(),
            "default.json 的 allow-* 条数必须与命令数一致"
        );
        assert_eq!(
            remote_deny.matches("\"allow-").count(),
            0,
            "remote-deny.json 不得包含任何 allow-*（它只做拒绝）"
        );
        assert!(
            !default_json.contains("\"remote\""),
            "设置窗的 capability 不得有 remote 块"
        );
        assert!(
            default_json.contains("\"windows\": [\"settings\"]"),
            "命令只授予设置窗（label = settings）"
        );
        assert!(
            remote_deny.contains("\"remote\""),
            "远程拒绝必须显式针对 remote 执行上下文"
        );
    }

    /// 分片前缀必须与 `shim.js` 里的字面量同源（Task 13b）——与 `REPORT_TITLE_PREFIX`
    /// 同一手法：不一致的表现是**静默丢数据**（页面写了 `FNOSCHUNK:`，宿主当成普通标题
    /// 镜像到窗口上，分片永远拼不起来）。
    #[test]
    fn chunk_prefix_and_shim_agree() {
        let shim = include_str!("../inject/shim.js");
        assert!(
            !report::CHUNK_TITLE_PREFIX.is_empty()
                && report::CHUNK_TITLE_PREFIX
                    .chars()
                    .all(|c| c.is_ascii_graphic()),
            "分片前缀必须是可打印 ASCII（WebView2 会吃掉控制字符）"
        );
        assert!(
            !report::CHUNK_TITLE_PREFIX.starts_with(report::REPORT_TITLE_PREFIX)
                && !report::REPORT_TITLE_PREFIX.starts_with(report::CHUNK_TITLE_PREFIX),
            "两个控制前缀不得互相包含，否则分支顺序会决定谁被吃掉"
        );
        assert!(
            shim.contains(&format!("'{}'", report::CHUNK_TITLE_PREFIX)),
            "shim.js 里的分片前缀必须由 report::CHUNK_TITLE_PREFIX 逐字而来"
        );
        // 页面侧的分片上限必须与宿主侧**同量级**（页面少切 = 宿主拒收；页面多切 = 白跑）
        assert!(
            shim.contains("REPORT_CHUNK_MAX = 8"),
            "shim 的分片数上限必须与 report::MAX_CHUNKS 一致"
        );
        assert!(
            shim.contains("REPORT_CHUNK_BODY_MAX_BYTES = 3000"),
            "shim 的单片字节上限必须与 report::MAX_CHUNK_BODY_BYTES 一致"
        );
    }

    /// 分片通道不得退化成 IPC：页面侧仍然一个命令都调不动（R70 的不变式）。
    #[test]
    fn chunk_channel_stays_on_the_title_transport() {
        let shim = include_str!("../inject/shim.js");
        assert!(
            !shim.contains("invoke("),
            "分片上报也只能写 document.title，不得改用 IPC"
        );
        assert!(
            shim.contains("setTimeout"),
            "分片必须跨 task 发送（同帧连写会被合并）"
        );
    }

    // ---------- Task 14b：设置窗本地存储 + 页面重汇报钩子 ----------

    /// 键名形状：合法的照收，**任何可能被当成路径的写法一律拒绝**。
    #[test]
    fn local_store_keys_cannot_be_paths() {
        for good in ["updateCheckState", "a.b:c-d_e", "A0"] {
            assert!(is_local_store_key(good), "应当放行：{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "C:/x",
            "a b",
            "键",
            "a\nb",
            &"x".repeat(65),
        ] {
            assert!(!is_local_store_key(bad), "必须拒绝：{bad:?}");
        }
    }

    /// patch 语义：`Some` 写入、`None` 删除；**整份校验在前**，坏键不产生半份写入。
    #[test]
    fn local_store_patch_is_all_or_nothing() {
        let mut store = std::collections::BTreeMap::new();
        let mut patch: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        patch.insert(
            "updateCheckState".to_string(),
            Some("{\"a\":1}".to_string()),
        );
        assert!(apply_local_store_patch(&mut store, &patch).is_ok());
        assert_eq!(
            store.get("updateCheckState").map(String::as_str),
            Some("{\"a\":1}")
        );

        // 同一次 patch 里既有合法键又有非法键 → 整次失败，合法的那个也不写入
        let mut mixed: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        mixed.insert("okKey".to_string(), Some("v".to_string()));
        mixed.insert("../evil".to_string(), Some("v".to_string()));
        assert!(apply_local_store_patch(&mut store, &mixed).is_err());
        assert!(!store.contains_key("okKey"), "非法 patch 不得留下半份写入");

        // None = 删除
        let mut del: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        del.insert("updateCheckState".to_string(), None);
        assert!(apply_local_store_patch(&mut store, &del).is_ok());
        assert!(store.is_empty());
    }

    /// 上限：单值 64 KiB、整份 256 KiB、键数 32 —— 字体数据（几 MB）在**任何**组合下都放不进来。
    #[test]
    fn local_store_bounds_reject_oversized_strings() {
        let mut store = std::collections::BTreeMap::new();
        let mut one: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        one.insert(
            "big".to_string(),
            Some("x".repeat(MAX_LOCAL_STORE_VALUE_BYTES + 1)),
        );
        let err = apply_local_store_patch(&mut store, &one).expect_err("超限必须被拒");
        assert!(err.contains("64 KiB"), "{err}");
        assert!(store.is_empty());

        // 恰好等于上限 → 通过（边界是「≤」而不是「<」）
        let mut exact: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        exact.insert(
            "big".to_string(),
            Some("x".repeat(MAX_LOCAL_STORE_VALUE_BYTES)),
        );
        assert!(apply_local_store_patch(&mut store, &exact).is_ok());
        assert_eq!(store.len(), 1);

        // 键数上限
        let mut many: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        for i in 0..(MAX_LOCAL_STORE_KEYS + 1) {
            many.insert(format!("k{i}"), Some("v".to_string()));
        }
        let err = apply_local_store_patch(&mut std::collections::BTreeMap::new(), &many)
            .expect_err("键数超限必须被拒");
        assert!(err.contains("32"), "{err}");

        // 总量上限（多个值合起来越界）
        let mut total: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        for i in 0..8 {
            total.insert(
                format!("k{i}"),
                Some("y".repeat(MAX_LOCAL_STORE_VALUE_BYTES)),
            );
        }
        let err = apply_local_store_patch(&mut std::collections::BTreeMap::new(), &total)
            .expect_err("总量超限必须被拒");
        assert!(err.contains("256 KiB"), "{err}");
    }

    /// 落盘往返 + 损坏文件的 `.bak` 保留（与 `config.json` 同一套纪律）。
    #[test]
    fn local_store_round_trips_and_keeps_a_broken_file() {
        let dir = temp_dir("local-store");
        let path = dir.join("local-store.json");
        let mut store = std::collections::BTreeMap::new();
        store.insert(
            "updateCheckState".to_string(),
            "{\"lastResult\":\"first\"}".to_string(),
        );
        save_local_store(&path, &store).unwrap();
        let again = load_local_store(&path);
        assert_eq!(again, store);
        // 落盘内容是可读的 JSON 对象（排障时人能直接看）
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("updateCheckState"), "{raw}");
        assert!(serde_json::from_str::<Value>(&raw).unwrap().is_object());

        // 损坏 → 空存储 + 原名保留为 .bak
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load_local_store(&path).is_empty());
        assert!(!path.exists(), "损坏的文件必须被改名移走");
        assert!(dir.join("local-store.json.bak").exists(), ".bak 必须保留");

        // 不存在 → 空存储（首次运行是正常状态，不该留下任何文件）
        let fresh = dir.join("never-written.json");
        assert!(load_local_store(&fresh).is_empty());
        assert!(!fresh.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 页面侧的「重新汇报应用项」钩子必须存在且**不引入任何页面 → 宿主的命令通路**。
    #[test]
    fn app_items_refresh_hook_exists_on_the_page_side_only() {
        let shim = include_str!("../inject/shim.js");
        assert!(
            shim.contains("__FNOS_REQUEST_APP_ITEMS__"),
            "页面侧必须暴露重汇报钩子，宿主 request_app_items 才有的可调"
        );
        let commands = include_str!("commands.rs");
        assert!(
            commands.contains(
                "window.__FNOS_REQUEST_APP_ITEMS__ && window.__FNOS_REQUEST_APP_ITEMS__();"
            ),
            "宿主 eval 的必须正是那一行固定代码（不拼接任何参数）"
        );
        assert!(
            !shim.contains("invoke("),
            "钩子仍然只能写 document.title，不得给页面开命令授权"
        );
        // fix round 1 / Minor 5：3 次额度必须按**真的送出去的请求**结算，而不是按调用次数。
        // 顺序是硬约束：先问「送出去了没有」，再计数——反过来就会把空转的调用也记账。
        let ask = shim
            .find("if (!askForAppItemsNow()) return false;")
            .expect("钩子必须先判定请求有没有真的送出（Minor 5：额度不得被空转消耗）");
        let count = shim
            .find("appItemsManualCalls += 1;")
            .expect("钩子必须仍然有次数上限");
        assert!(
            ask < count,
            "计数的位置必须在「真的送出去了」判定之后（先加一再看结果 = 旧缺陷）"
        );
    }
}
