//! IPC 契约（spec §8.3）与 Rust 侧内部操作。

use crate::{base64, config, config::Config, injector, paths, report, tray, MAIN_WINDOW};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::webview::{PageLoadEvent, WebviewBuilder};
use tauri::window::WindowBuilder;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Runtime, State, Webview, WebviewUrl, WebviewWindowBuilder};
use crate::logfile::flog;

/// 设置窗 label（spec §7）：与 `capabilities/default.json` 的 `"windows": ["settings"]` 一致。
pub const SETTINGS_WINDOW: &str = "settings";

/// 标签页栏 Webview 的 label（T14d）：主窗口里的一个子 Webview，承载自绘标题栏
/// （标签条 + 窗口控制 + 拖拽区）。授权面见 `capabilities/titlebar.json`——它拿的是
/// 窗口控制与三个标签页命令，仅此而已。
pub const TITLEBAR_WEBVIEW: &str = "titlebar";

/// 标签页栏高度（逻辑像素）：所有标签页 Webview 从这条栏的下缘开始铺。
/// 32 = Windows 11 caption 高度（用户反馈 40 太高后定值）。
/// 前端 `ui/settings/titlebar.js` 的 `STRIP_H` 与这里必须一致（有跨语言断言）。
pub const TAB_STRIP_H: f64 = 32.0;

/// 同时打开的标签页上限（含不可关闭的首页）。
///
/// 每个标签页都是一个完整 WebView2 实例；任何远程页面都能 `window.open`。没有上限
/// 就等于把「页面 → 宿主资源耗尽」的开关交给页面。24 个足够正常使用。
const MAX_OPEN_TABS: usize = 24;

/// 弹窗限速：10 秒内最多 5 个。覆盖正常登录/授权流程，挡住脚本无限开窗。
const POPUP_RATE_WINDOW_MS: u64 = 10_000;
const POPUP_RATE_MAX: usize = 5;

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

/// 标签页键盘快捷键的控制前缀（P0-3；页面侧 `inject/keys.js`）。
///
/// 焦点在子 Webview 里，tauri 的 `WindowEvent::KeyboardInput` 收不到这些键；本壳已有的
/// 「页面 → 宿主」通道就是 `document.title`（不占任何 IPC 授权），沿用它能省掉
/// `with_webview` + WebView2 COM 的一整层 unsafe。
pub const KEY_TITLE_PREFIX: &str = "FNOSKEY:";

/// 快捷键的最小生效间隔（毫秒）。
///
/// 任何页面都写得动自己的标题，所以这条通道必须按「可能被刷屏」设计：120ms 内的多个命令
/// 只执行第一个（人类连按 Tag 键的极限在 100ms 以上，180 次/秒的合成风暴会被完全折掉）。
const KEY_COOLDOWN_MS: u64 = 120;

/// 标题日志的最小落盘间隔（毫秒）。页面标题会随未读计数/播放进度频繁变化，逐条写日志
/// 只会白占磁盘和主线程；500ms 足够还原「标题怎么变」，又不至于把日志变成噪声放大器。
const TITLE_LOG_INTERVAL_MS: u64 = 500;

/// 快捷键冷却的**上次执行时刻**（`AppState::last_key_at`）：Unix 毫秒。
///
/// 用 `AtomicU64` + 毫秒时间戳而不是 `Mutex<Instant>`：这条路径被设计成「不获取任何
/// Mutex」，免得页面刷标题时与标签页/配置锁争抢。
pub type KeyClock = AtomicU64;

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
    /// 标签页注册表（T14d）：主窗口里的标签条状态。`order` 是创建顺序（首项恒为
    /// `"main"`——fnOS 主页那一个，**不可关闭**），`titles` 是各标签的显示标题
    /// （来自 `document.title`），`active` 是当前显示的标签 label。
    /// 只在内存里；窗口重建时整体复位（重建出来的窗口只有一个 `main` 标签）。
    pub tabs: Mutex<TabState>,
    /// 下一个标签页的编号（`tab-N` 的 N；跨重建累加，保证 label 不与历史冲突）。
    pub next_tab: AtomicU64,
    /// 下一个**原生弹窗**的编号（`popup-N` 的 N）。
    ///
    /// 与 [`Self::next_tab`] 同构但独立计数：弹窗不是主窗口里的子 Webview，不进标签条。
    /// 只增不减：页面 `window.close()` 走 WebView2 的 `WindowCloseRequested`，窗口会被
    /// 原生销毁而 tauri 的 label 注册表未必立刻释放——永不复用 label 就不会撞上残留项。
    pub next_popup: AtomicU64,
    /// 最近弹窗请求的时间戳（Unix 毫秒），用于 [`POPUP_RATE_WINDOW_MS`] 限速。
    ///
    /// 只保留时间窗内的条目（每次请求前 prune），所以长度上限就是
    /// [`POPUP_RATE_MAX`] + 1，不会无限增长。
    pub popup_recent: Mutex<Vec<u64>>,
    /// 「已为该页面开过标签」的登记表：[`page_key`]（剥 query/fragment 的
    /// scheme+host+port+path）→ 标签 label（T14e）。
    ///
    /// 为什么需要：中继转发流程会**反复** `window.open` 同一地址——每次都开新标签就是
    /// 标签爆炸、主标签停在选择页「进不了桌面」（用户实测）。命中登记表时先校验标签
    /// 还活着（被关过就清掉条目继续开），活着就**复用切换**、绝不新开。
    /// 只在内存里；标签关闭（[`tab_close`]）与窗口重建时清理。
    pub open_tabs: Mutex<std::collections::HashMap<String, String>>,
    /// 初始化脚本缓存（P0-2）：`(内容指纹, 脚本正文)`。
    ///
    /// 为什么需要：`injector::build_init_script` 会读壁纸（最多 8 MiB）、base64 15 个图标
    /// （约 1 MiB）、再拼装约 500 KB 文本。此前**每开一个标签页**（`open_app_tab`）与每次
    /// 建窗都完整重做一遍——开一个第三方应用标签要白付这整份成本，而配置在两次开标签之间
    /// 几乎从不变化。
    ///
    /// 失效判据是 [`injector::script_fingerprint`]：只覆盖真正进脚本的配置项。
    /// 缓存**不跨进程**、也不落盘（脚本正文随 `Config` 内存态同寿命）。
    pub init_script: Mutex<Option<(String, String)>>,
    /// 快捷键命令的上次执行时刻（Unix 毫秒；`0` = 还没执行过）。
    ///
    /// 见 [`KEY_COOLDOWN_MS`]：页面（任何页面）都写得动 `document.title`，这条通道必须假设
    /// 会被刷屏，所以每条命令在执行前先做一次 CAS 冷却——不取任何 Mutex。
    pub last_key_at: KeyClock,
    /// 上次推给标题栏的最大化状态：`0` = 未知（建窗/重建后必须重推）、`1` = 否、`2` = 是。
    ///
    /// 拖拽缩放期间 `Resized` 会高频到来，`window.is_maximized()` 很便宜，但每次 eval 标题栏
    /// 是跨 Webview 的 IPC；用这个状态把「没变」的推送折掉。
    pub last_maximized: AtomicU8,
    /// 标题日志的上次落盘时刻（Unix 毫秒；见 [`TITLE_LOG_INTERVAL_MS`]）。
    pub last_title_log: AtomicU64,
    /// 检查更新的对外状态（T14g：设置页 → 更新）。
    ///
    /// 托盘菜单不再承载更新；一次「检查 → 下载（带进度）→ 就绪 → 安装」的每一步都写
    /// 这里，设置页轮询 `get_update_state` 渲染。状态只在内存里，重启回到 idle。
    pub update: Mutex<crate::updater::UpdateStatus>,
}

/// 主窗口的标签页状态（见 [`AppState::tabs`]）。
pub struct TabState {
    pub order: Vec<String>,
    pub titles: HashMap<String, String>,
    pub active: String,
}

impl TabState {
    fn with_main() -> Self {
        Self {
            order: vec![MAIN_WINDOW.to_string()],
            titles: HashMap::new(),
            active: MAIN_WINDOW.to_string(),
        }
    }
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

/// origin 的 host 是否属于飞牛（裸域 `fnos.net`，以及 [`injector::FNOS_FAMILY_DOMAINS`]
/// 里任一域名的子域）。
///
/// 上游的签名正则要求前导点（`(\.fnos\.net)$`，`content-script.js:2746-2779`），所以
/// **根域官网永远不注入**；这里判「是不是飞牛的域名」，注入与否由 [`is_recognized`] 与
/// 上游判定分层处理（状态条据此区分「官网」与「WebUI」两种说法）。
///
/// 域名族（`fnnas.cn` / `5ddd.com` / `fynas.net`）与 `*.fnos.net` 同等对待：它们是同一套
/// FN Connect 入口的不同域名，上游按这张表自动注入，[`origin_allowed_by_gate`] 也按这张表
/// 放行——三处同源，状态条不会与实际情况打架。
fn is_fnos_net_origin(origin: &str) -> bool {
    let Ok(url) = tauri::Url::parse(origin) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    host == "fnos.net" || injector::is_fnos_family_host(&host)
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
    fn apply_to<R: Runtime>(self, window: &tauri::Window<R>) {
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

pub fn load_config<R: Runtime>(app: &AppHandle<R>) -> (Config, config::LoadReport) {
    let path = Config::config_path();
    let (cfg, report) = Config::load_with_report(&path);
    // 读失败时**绝不**在这里写盘：那会把一份可能完好的配置用默认值覆盖掉。
    // 首启的落盘统一由 `save_and_install_state` 按同一个 report 决定。
    // 首启的装配顺序是「先读配置、再 `manage(AppState)`」，所以这一刻通常还没有 state，
    // 回退标记必须**返回给调用方**并由 `save_and_install_state` 带进去；`try_state` 分支
    // 是给将来「运行期重新装载配置」留的对称路径。
    if let Some(state) = app.try_state::<AppState>() {
        *state.config.lock().unwrap() = cfg.clone();
        state
            .recovered_from_backup
            .store(report.recovered_from_backup, Ordering::SeqCst);
    }
    (cfg, report)
}

pub(crate) fn current<R: Runtime>(app: &AppHandle<R>) -> Config {
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
        // P1-8：判据抽成 [`payload_changed`]，`set_config` 与 `reset_config` 共用。
        // 此前 reset 那份只比 `inject_enabled` / `home_url` 两项，于是「恢复默认」把完美图标
        // 关掉、把登录壁纸清空之后，活窗口的初始化脚本里仍然带着 1 MiB 图标与旧壁纸——
        // 设置窗显示已关闭、页面上却还在生效，直到用户下次重启。
        let needs_reload = payload_changed(&prev, &next);
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

/// 这两份配置的**初始化载荷**是否不同——即「要不要重建主窗口才算生效」。
///
/// 初始化脚本是 WebView2 的 `AddScriptToExecuteOnDocumentCreated`，注册到活 webview 上之后
/// **换不掉**（gap (a)），所以凡是进了这份脚本、又不在免刷新通道里的键，变了就必须重建。
///
/// 四个键（判据全部在 `normalize()` 之后比较，非法值被夹成默认值时不白重建）：
///
/// 1. `shell.injectEnabled`：决定脚本注册不注册（关掉 = 一个字节都不注册）；
/// 2. `shell.homeUrl`：决定建窗地址；
/// 3. `injector::perfect_icon_enabled`：决定 14 个 PNG 要不要进载荷（约 +1.0 MiB）。
///    注意它比的是**开关状态**——逐项键（scaleSelected / redrawKeys / redrawMap 的具体取值）
///    不是载荷结构，经 `apply_to_page` 即时生效，不需要重建；
/// 4. `local.loginWallpaperFileName`：决定从配置目录读哪个文件、以什么键嵌进 `binaryAssets`。
///    导入路径落盘的名字带内容指纹（`config::stored_wallpaper_name`），所以「换了图」
///    必然表现为「名字变了」，比较文件名就够。
///
/// **故意不在判据里**的：`shell.dockAutoHide` 与 `shell.keepAliveMinutes`（`dock.js` /
/// `keepalive.js` 在活页面上由 `apply_to_page` 的 `__FNOS_APPLY_SHELL__` 即时切换），以及
/// `mods.*` 的全部字段（同样走免刷新通道）。
///
/// 抽成函数是为了让 `set_config` 与 `reset_config` **不可能分叉**：这两条路径的判据分叉过一次
/// （reset 少了后两项），表现是「恢复了默认设置，但图标/壁纸要重启才消失」。
fn payload_changed(prev: &Config, next: &Config) -> bool {
    next.shell.inject_enabled != prev.shell.inject_enabled
        || next.shell.home_url != prev.shell.home_url
        || injector::perfect_icon_enabled(next) != injector::perfect_icon_enabled(prev)
        || next.local.login_wallpaper_file_name != prev.local.login_wallpaper_file_name
}

/// 把 `mods` / `local` 推给页面（免刷新）。
///
/// `injectEnabled` / `homeUrl` **不在**此列：已注册到 WebView2 的
/// `AddScriptToExecuteOnDocumentCreated` 脚本无法在活窗口上替换，只能重建窗口（gap (a)）。
fn apply_to_page<R: Runtime>(app: &AppHandle<R>, cfg: &Config) {
    // T14d：每个标签页 Webview 都是独立页面（都可能承载 fnOS / 应用页），配置推送要
    // 铺到**所有**标签页；标签页栏（titlebar）不消费配置，跳过。
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    let win = window.webviews();
    let payload = serde_json::json!({
        "mods": &cfg.mods,
        "local": &cfg.local,
        // T14c：shell 段只发页面消费的键（与 injector 载荷同一条窄化原则——
        // homeUrl/nasUrl/window 是宿主私有，不得借免刷新通道发给页面）。
        // 消费方：shim 的 `__FNOS_APPLY_CONFIG__` → `__FNOS_APPLY_SHELL__`（dock.js / keepalive.js）。
        "shell": {
            "dockAutoHide": cfg.shell.dock_auto_hide,
            "keepAliveMinutes": cfg.shell.keep_alive_minutes,
        },
    });
    match serde_json::to_string(&payload) {
        Ok(json) => {
            for wv in &win {
                if wv.label() == TITLEBAR_WEBVIEW {
                    continue;
                }
                // 只推给真正持有完整载荷的页面（main / 桌面根 / 授权页）。应用标签只有
                // keys.js，第三方标签更不该拿到 mods/local；页面自己定义的
                // `__FNOS_APPLY_CONFIG__` 不能成为配置外泄通道。
                let Ok(url) = wv.url() else { continue; };
                if !should_receive_full_config(cfg, wv.label(), &url) {
                    continue;
                }
                let _ = wv.eval(format!(
                    "window.__FNOS_APPLY_CONFIG__ && window.__FNOS_APPLY_CONFIG__({json});"
                ));
            }
        }
        // `Config` 全是普通字段，序列化不可能失败；真失败也只是这一次免刷新没生效
        Err(e) => flog!("[fnos] 配置推送序列化失败: {e}"),
    }
}

/// 取初始化脚本（带缓存；P0-2）。
///
/// **为什么需要缓存**：`injector::build_init_script` 每次都要读壁纸（最多 8 MiB）、把 15 个
/// 图标 base64 成 1 MiB 的 data URL、再拼装约 500 KB 文本。它原先在**每开一个标签页**与每次
/// 建窗时都被完整调用一遍——也就是说「从启动台点开一个第三方应用」会先白白做一遍这些工作，
/// 而结果通常与上次字节相同。
///
/// **正确性判据**：缓存键是 [`injector::script_fingerprint`]，覆盖所有真正进脚本的配置项
/// （放进指纹的项「只多不少」，宁可多失效一次也不让用户拿到旧脚本）。
///
/// **锁纪律**：只在锁内做「查键 / 写回」，**不在锁内**跑昂贵的 `build_init_script`——
/// 否则并发开两个标签时第二个会被第一个的长耗时卡住。
pub fn init_script_cached<R: Runtime>(app: &AppHandle<R>, cfg: &Config) -> String {
    let key = injector::script_fingerprint(cfg);
    {
        let state = app.state::<AppState>();
        let cached = state.init_script.lock().unwrap();
        if let Some((cached_key, script)) = cached.as_ref() {
            if *cached_key == key {
                return script.clone();
            }
        }
    }
    // 锁外构建（可能读盘，几十毫秒级）
    let script = injector::build_init_script(cfg);
    {
        let state = app.state::<AppState>();
        *state.init_script.lock().unwrap() = Some((key, script.clone()));
    }
    script
}

/// 建主窗口：**首次启动与重建共用同一份定义**（地址、几何、注入载荷、标题跟随页面、加载日志）。
///
/// gap (a)：`inject_enabled=false` 时 `build_init_script` 返回空串，此时**不注册**空脚本
/// ——「关掉注入」在 WebView2 里不留任何已注册脚本，这是「取消勾选能真正停掉注入」的前提。
pub fn build_main_window<R: Runtime>(
    app: &AppHandle<R>,
    url: tauri::Url,
    cfg: &Config,
) -> tauri::Result<tauri::Window<R>> {
    // 首启路径：建窗即可见（`start_visible = true`）。
    build_main_window_with(app, url, cfg, true)
}

/// `build_main_window` 的实体；`start_visible = false` 供重建路径使用——旧的隐藏 / 最小化
/// 窗口不该让新窗口先可见地闪一下（Item 3）。
///
/// T14d：主窗口是**无边框 Window + 多个 Webview**——
///   * `titlebar` Webview：自绘标题栏（标签条 + 窗口控制 + 拖拽区），见
///     `ui/settings/titlebar.html`，铺在窗口顶部 [`TAB_STRIP_H`] 高的一条里；
///   * `main` Webview：fnOS 主页（第一个标签页），铺在标签栏以下的全部区域；
///   * 之后 `window.open` 的外部应用各占一个 `tab-N` Webview（见 [`open_app_tab`]），
///     不再是独立窗口。
///     这要求 tauri 的 `unstable` feature（`Window::add_child`，桌面端可用）。
///     所有子 Webview 都**不会**随窗口 resize 自动重排——[`relayout_main_window`] 在
///     `Resized` 事件里统一摆位。
fn build_main_window_with<R: Runtime>(
    app: &AppHandle<R>,
    url: tauri::Url,
    cfg: &Config,
    start_visible: bool,
) -> tauri::Result<tauri::Window<R>> {
    // 建窗即登记一次「加载开始」：`generation` 会被下面的回调捕获，用于丢弃旧世代的
    // 迟到事件（重建 / 自动重试期间尤其重要）。
    let generation = begin_load(app, url.as_str());
    let mut builder = WindowBuilder::new(app, MAIN_WINDOW)
        .title("fnOS")
        // 不变式（Item 2）：`cfg.shell.window` 已经由 `Config::normalize`
        //（`WindowGeom::clamp_to_usable`）夹成可用几何，而所有进入 `AppState` 的 `Config`
        // 都经过它（`load` / `set_config` / `reset_config` / `reload_main` /
        // `save_window_geom`），所以这里不会再拿到 `0x0` 或负数。
        .inner_size(cfg.shell.window.w, cfg.shell.window.h)
        // T14d 修复轮 3（用户要求「标签页合并到标题栏」）：**无边框 + 一行式自绘标题栏**——
        // 标签与窗口控制（─ □ ×，系统默认观感）同处一条 32px 的栏里，不再有单独的原生
        // 标题行。窗口控制由 titlebar Webview 的按钮经授权的窗口命令承担；拖拽区是栏内
        // 空白。桌面内置应用窗口的样式仍由上游 mod 的「标题栏样式」设置接管，与本栏无关。
        .decorations(false)
        // T14d 修复轮 7（用户报「标题栏下面的线条」）：窗口背景钉成标题栏同色。
        // 无边框窗口未设背景时 Windows 用默认白底——标题栏 Webview 与内容 Webview 之间
        // 任何 1px 的合成缝都会露出白线；背景与栏同色后，缝与栏同色 = 视觉无线。
        .background_color(tauri::window::Color(0x1b, 0x1c, 0x1e, 0xff));
    // 位置建窗时就带上（T14c 修复轮 20）：不让窗口先在 OS 默认位置落脚再跳。
    match (cfg.shell.window.x, cfg.shell.window.y) {
        (Some(x), Some(y)) if x > -10000.0 && y > -10000.0 => {
            builder = builder.position(x, y);
        }
        _ => builder = builder.center(),
    }
    if !start_visible {
        builder = builder.visible(false);
    }
    let window = builder.build()?;

    // 标签页状态复位：重建出来的窗口永远只有一个 `main` 标签（T14d）。
    //
    // P1-7：`open_tabs` 复用登记表也必须一起清空——它的键是「页面 → 标签 label」，而重建
    // 之后 `tab-N` 这些 label 全部不存在了（残留条目会在命中时被「验页面」判否并自愈，
    // 但那要多绕一圈；这里直接清干净，与注释里承诺的语义一致）。
    {
        let state = app.state::<AppState>();
        *state.tabs.lock().unwrap() = TabState::with_main();
        state.open_tabs.lock().unwrap().clear();
        // 新建的标题栏是全新的 Webview：最大化字形状态必须重推一次。
        state.last_maximized.store(0, Ordering::SeqCst);
    }

    // —— 标签页栏：本地资产页，无注入、无上报通道，授权面见
    //    `capabilities/titlebar.json`（拖拽 + 三个标签页命令，仅此而已）。
    let (lw, lh) = logical_inner_size(&window)?;
    let titlebar = WebviewBuilder::new(TITLEBAR_WEBVIEW, WebviewUrl::App("titlebar.html".into()));
    window.add_child(
        titlebar,
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(lw, TAB_STRIP_H),
    )?;

    // —— fnOS 主页（第一个标签页）。
    //
    // spec §7「标题跟随页面」+ Task 11 加载观测 + T14c 修复轮 11 的 window.open 裁定
    // 全部挂在**这一个** Webview 上（与标签页 Webview 共用同一组回调定义，见
    // [`tab_webview_callbacks`]）。`main` 这个 label 同时是注入页的 IPC 身份：
    // `capabilities/default.json` 之外它没有任何授权，远程页面的显式拒绝在
    // `capabilities/remote-deny.json`。
    let script = injector::wrap_origin_script(
        &shell_origins(cfg),
        &init_script_cached(app, cfg),
    );
    let mut main_webview = tab_webview_builder(MAIN_WINDOW, url.clone(), app, Some(generation));
    if !script.is_empty() {
        // gap (a)：`inject_enabled=false` 时 `build_init_script` 返回空串，此时**不注册**
        // 空脚本——「取消勾选能真正停掉注入」的前提。
        main_webview = main_webview.initialization_script(script);
    }
    window.add_child(
        main_webview,
        LogicalPosition::new(0.0, TAB_STRIP_H),
        LogicalSize::new(lw, lh - TAB_STRIP_H),
    )?;

    // 「加载完成」迟迟不来就切错误页（spec §12.3）。看门狗是**独立线程**，睡满即自行退出；
    // 它靠 `generation` 判断自己是否还属于当前这一轮，所以重建 / 自动重试不会留下会误触发
    // 上一轮的定时器，也没有任何常驻计时器（无泄漏）。
    arm_load_watchdog(app, generation);
    // T14d：标签条先推一次初态（main 一个标签），不等第一条标题事件。
    push_tabs(app);
    // 新建标题栏：最大化字形必须初始化（push_tabs 不再顺带推它）。
    push_win_max(app);
    Ok(window)
}

/// 窗口内容区的**逻辑**尺寸（物理 / 缩放），供子 Webview 摆位用。
fn logical_inner_size<R: Runtime>(window: &tauri::Window<R>) -> Result<(f64, f64), tauri::Error> {
    let size = window.inner_size()?;
    let scale = window.scale_factor()?;
    Ok((size.width as f64 / scale, size.height as f64 / scale))
}

/// 标签页 Webview 的回调组（T14d）：主标签与所有 `tab-N` 共用同一套——
/// 标题跟随（兼探针 / 上报通道）、加载日志（加载观测**只属于 `main` 标签**：
/// 错误页 / 看门狗 / 退避是为「fnOS 主页」设计的，外部应用的失败在它自己的标签页里呈现，
/// 不该把整个窗口顶掉），以及 window.open → 标签页的裁定。
///
/// `generation`：`Some` = 主标签（加载事件喂给 [`on_page_event`] 的世代闸门，
/// 值必须是建窗那一刻 [`begin_load`] 发出的世代）；`None` = 普通标签页（只记日志）。
fn tab_webview_builder<R: Runtime>(
    label: &str,
    url: tauri::Url,
    app: &AppHandle<R>,
    generation: Option<u64>,
) -> WebviewBuilder<R> {
    let app_for_title = app.clone();
    let app_for_load = app.clone();
    let app_for_new_window = app.clone();
    let label = label.to_string();
    let opener_label = label.clone();
    WebviewBuilder::new(&label, WebviewUrl::External(url))
        .on_document_title_changed(move |webview, title| {
            handle_title(&webview, &title, &app_for_title);
        })
        .on_page_load(move |_webview, payload_event| {
            if payload_event.event() != PageLoadEvent::Finished {
                return;
            }
            match generation {
                Some(g) => on_page_event(
                    &app_for_load,
                    g,
                    payload_event.event(),
                    payload_event.url().as_str(),
                ),
                None => flog!(
                    "[fnos] 标签页已加载: {}",
                    report::log_safe(payload_event.url().as_str())
                ),
            }
        })
        .on_new_window(move |url, features| {
            new_window_verdict(&app_for_new_window, &opener_label, url, features)
        })
}

/// 「同一页面」的登记键：scheme+host+port+path（**剥掉 query/fragment**）。
///
/// T14e：中继转发流程的重复 `window.open` 常带变化的时间戳/随机参数——不剥的话
/// 精确比对永远不相等，去重失效；而 path 不同就是不同页面（不做跨页合并）。
fn page_key(url: &tauri::Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}

/// 新窗口请求的裁定（`window.open` / `target=_blank`）：**普通应用启动 → 标签页**，
/// **依赖浏览器弹窗语义的 → 原生弹窗**。
///
/// 背景（T14c 修复轮 11，实测）：Hermes Studio 这类「外部 Web 应用」的启动路径就是
/// `window.open('https://hermes-studio.<nas>.fnos.net/', '_blank')`——在浏览器里是新标签页，
/// 而 WebView2 默认把这类请求**丢弃**（点图标什么都没发生）。T14d 起这类请求开成主窗口
/// 标签条里的标签页（[`open_app_tab`]），这是本函数的主路径。
///
/// 但「标签页」不是真正的 `window.open`（T14e 用户实测的两类失败）：
/// - `window.open('')` / `about:blank` 的弹窗流程要求**同步拿回真实 WindowProxy**，
///   再往里 `document.write` 或设 `location`。转交异步开标签后返回值是 `null`，
///   内容永远写不进去——用户看到的就是「新标签页一片空白」；
/// - OAuth / SSO 登录弹窗要求 `window.opener`、opener 轮询 `popup.closed`、
///   登录完成页 `window.close()` 关闭自己。宿主建的标签页没有 opener 关系，且
///   WebView2 只允许**脚本打开的窗口**关闭自己——登录完成后结果永远回不到应用。
///
/// 所以这里做**混合裁定**（[`wants_native_popup`]）：`about:blank`、带窗口尺寸/坐标的
/// `window.open(url, name, features)`、以及登录类地址（`/signin`、`/oauth`、`/auth/`…）
/// 交给 `NewWindowResponse::Create` 建一个**真正的 WebView2 弹窗**——同一环境（cookie /
/// storage 共享）、真实 opener、`window.open` 有返回值、`window.close()` 能关；
/// 其余普通应用启动仍然开标签页，T14d 的形态不变。
///
/// 为什么只有弹窗走 `Create`：`Window::add_child` 在 Windows 的**同步事件回调**里会死锁
/// （tauri 文档的 Known issues 明确警告），而 `on_new_window` 恰恰是同步回调；标签页由
/// [`open_app_tab`] 在异步上下文里创建。`Create` 是 tauri/wry 为这个回调专门支持的窗口
/// 创建路径（wry 先用 `dispatch_handler` 把回调转到消息循环再执行，tauri 文档的示例
/// 就是在这个闭包里 `WebviewWindowBuilder::build()`），不碰 `add_child`。
///
/// 复用闸门（T14e，用户实测「中继转发会一直新开标签」）：中继连接流程会**反复**
/// `window.open` 同一地址——命中登记表（[`page_key`]，见 [`AppState::open_tabs`]）或
/// main 当前页时**切换复用、绝不新开**；登记过的标签被关过则清掉条目照常新开。
/// 弹窗**不走**这条闸门：浏览器对 `window.open` 的语义就是每次一个新窗口，去重会把
/// 「重新发起登录」吞掉。
///
/// 安全边界：只给 `http` / `https` / `about` 放行，其余协议（`file:` / `data:` /
/// `javascript:` …）一律 `Deny` 并留一行日志，且**不**转交。
fn new_window_verdict<R: Runtime>(
    app: &AppHandle<R>,
    opener_label: &str,
    url: tauri::Url,
    features: tauri::webview::NewWindowFeatures,
) -> tauri::webview::NewWindowResponse<R> {
    match url.scheme() {
        "http" | "https" | "about" => {}
        other => {
            flog!("[fnos] 拒绝非 http(s) 的新窗口请求（scheme={other}）：{url}");
            return tauri::webview::NewWindowResponse::Deny;
        }
    }
    // 弹窗必须在**同步回调里当场**建（Create）：一旦转交异步任务，opener 关系与
    // `window.open` 的返回值就永久丢失——那正是空白标签 / 登录结果回不去应用的根因。
    let popup = wants_native_popup(&url, &features);
    flog!(
        "[fnos] 新窗口请求 → {}：{}（size={:?} position={:?}）",
        if popup { "原生弹窗" } else { "标签页" },
        report::log_safe(url.as_str()),
        features.size(),
        features.position()
    );
    if popup {
        if popup_rate_limited(app) {
            flog!(
                "[fnos] 弹窗请求过于频繁（{POPUP_RATE_MAX} 次 / {POPUP_RATE_WINDOW_MS}ms），已拒绝：{url}"
            );
            return tauri::webview::NewWindowResponse::Deny;
        }
        match create_popup_window(app, opener_label, &url, features) {
            Ok(window) => return tauri::webview::NewWindowResponse::Create { window },
            // 建窗失败（label 冲突 / WebView2 拒绝）时退回标签页：至少应用能打开，
            // 只是登录类流程仍会受 opener 限制，日志里留证据。
            Err(e) => flog!("[fnos] 原生弹窗创建失败，回退为标签页：{e}"),
        }
    }
    // 复用闸门（T14e：中继转发一直新开标签的止血）：目标页面已经有活着的标签
    // （登记表命中并验活，或它就是 main 的当前页）→ 切过去，绝不新开。
    {
        let key = page_key(&url);
        let registered = {
            let state = app.state::<AppState>();
            let map = state.open_tabs.lock().unwrap();
            map.get(&key).cloned()
        };
        if let Some(existing) = registered {
            // P1-7：命中之后还要**验页面**，不能只验「这个 webview 还活着」。标签页是完整
            // 浏览器语义，用户完全可以在里面点链接跳到别的地址；旧实现只看存活，于是
            // 「应用页跳走之后，再从启动台点同一入口」会切到一个显示着别的内容的旧标签，
            // 用户看到的是「点了没反应 / 打开的不是这个应用」。
            let still_same_page = app
                .get_webview(&existing)
                .and_then(|wv| wv.url().ok())
                .map(|current| page_key(&current) == key)
                .unwrap_or(false);
            if still_same_page {
                let app_for_switch = app.clone();
                tauri::async_runtime::spawn(async move {
                    activate_tab(&app_for_switch, &existing);
                });
                flog!("[fnos] window.open -> 已有相同页面的标签，复用切换（不新开）：{url}");
                return tauri::webview::NewWindowResponse::Deny;
            }
            // 标签已关 / 已导航到别的页面 → 清掉失效条目，继续走新建
            let state = app.state::<AppState>();
            state.open_tabs.lock().unwrap().remove(&key);
        }
        // main 当前页也算「已打开」（中继选择页 open 到桌面地址时不再多一个桌面标签）
        if let Some(main_wv) = app.get_webview(MAIN_WINDOW) {
            if let Ok(main_url) = main_wv.url() {
                if page_key(&main_url) == key {
                    let app_for_switch = app.clone();
                    tauri::async_runtime::spawn(async move {
                        activate_tab(&app_for_switch, MAIN_WINDOW);
                    });
                    flog!("[fnos] window.open -> 与桌面标签相同，切换复用（不新开）：{url}");
                    return tauri::webview::NewWindowResponse::Deny;
                }
            }
        }
    }
    let app = app.clone();
    let url_for_log = url.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = open_app_tab(&app, url).await {
            flog!("[fnos] 标签页创建失败（已拒绝该请求）：{e}");
        }
    });
    flog!("[fnos] window.open -> 开成标签页：{url_for_log}");
    tauri::webview::NewWindowResponse::Deny
}

/// Unix 毫秒时间戳（弹窗限速用；时钟回拨时返回 0，最多多放行一次）。
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 纯限速判定：先把窗口外的旧记录删掉，再看窗口内是否已达上限。
///
/// 抽成纯函数是为了能在不构造 `AppHandle` 的情况下单测「窗口滑动」行为。
fn popup_rate_exceeded(recent: &mut Vec<u64>, now_ms: u64) -> bool {
    recent.retain(|t| now_ms.saturating_sub(*t) < POPUP_RATE_WINDOW_MS);
    if recent.len() >= POPUP_RATE_MAX {
        return true;
    }
    recent.push(now_ms);
    false
}

/// 当前是否触发弹窗限速（会记录一次请求）。
fn popup_rate_limited<R: Runtime>(app: &AppHandle<R>) -> bool {
    let state = app.state::<AppState>();
    let mut recent = state.popup_recent.lock().unwrap();
    popup_rate_exceeded(&mut recent, now_unix_ms())
}

/// 是否必须开成**原生弹窗**（而不是主窗口标签页）。
///
/// 判据只取决于「这个请求是否依赖真实浏览器弹窗语义」：
/// - `about:blank`：`window.open('')` 后由 opener 同步写内容 / 设地址，标签页接不住；
/// - 带尺寸或坐标：典型的 `window.open(url, name, 'width=…,height=…')` 登录 / OAuth 弹窗；
/// - 登录类地址：即使没写尺寸，`/signin`、`/oauth`、`/api/v1/auth/…` 这类地址也几乎一定
///   依赖 `window.opener` 回传结果（用户实测：在标签页里登录完，应用收不到结果）。
///
/// 其余（应用启动的 `window.open(url, '_blank')` / `target=_blank`）继续开标签页。
fn wants_native_popup(url: &tauri::Url, features: &tauri::webview::NewWindowFeatures) -> bool {
    wants_native_popup_parts(url, features.size().is_some(), features.position().is_some())
}

/// [`wants_native_popup`] 的纯判定部分：不碰平台相关的 `NewWindowFeatures`，便于单测。
fn wants_native_popup_parts(url: &tauri::Url, has_size: bool, has_position: bool) -> bool {
    url.scheme() == "about" || has_size || has_position || is_auth_like_url(url)
}

/// 登录类地址：路径段或主机名带有明确的认证语义。
///
/// 只做**整段**匹配（`/author` 不算 `auth`），避免把普通页面误判成登录弹窗。
fn is_auth_like_url(url: &tauri::Url) -> bool {
    const AUTH_SEGMENTS: &[&str] = &[
        "login",
        "log-in",
        "logon",
        "signin",
        "sign-in",
        "auth",
        "oauth",
        "oauth2",
        "authorize",
        "authorization",
        "sso",
        "saml",
        "openid",
        "cas",
    ];
    let path_hit = url.path().split('/').filter(|s| !s.is_empty()).any(|seg| {
        // `login.aspx` / `signin.html` 这类带扩展名的也按主名比较
        let base = seg.split('.').next().unwrap_or(seg);
        AUTH_SEGMENTS.iter().any(|t| base.eq_ignore_ascii_case(t))
    });
    if path_hit {
        return true;
    }
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    ["login.", "signin.", "auth.", "accounts.", "account."]
        .iter()
        .any(|prefix| host.starts_with(prefix))
}

/// 两个 URL 是否同源（scheme + host + 默认端口规范化后的 port）。
fn same_origin(a: &tauri::Url, b: &tauri::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// 为一个需要真实浏览器语义的新窗口请求建**原生弹窗**。
///
/// 返回的 `WebviewWindow` 交给 `NewWindowResponse::Create`：wry 会把它接到 WebView2 的
/// `NewWindowRequestedEventArgs.SetNewWindow` 上——opener 关系、`window.open` 的返回值、
/// `postMessage` 与 `window.close()` 全部由 WebView2 自己维护。`.window_features(features)`
/// 让新窗口复用 opener 的 WebView2 环境（同一个 user-data 目录 / cookie / storage），
/// 登录状态与标签页共享。
///
/// **注入是带运行时闸门的**：`fnos_site` 只决定要不要注册载荷（判据同时看**目标地址**
/// 与 **opener 当前地址**——fnOS 桌面开出的 `about:blank` 弹窗之后可能被 opener 导航到
/// 任意站点，注册必须跟着走）；载荷本身在每次文档创建时检查 `location`，只有落在闸门
/// 放行集合的 http(s) 文档才执行。这样 fnOS 自己的 `/signin` 授权页（「授权按钮
/// 可见性」等 CSS 修正）仍然拿到完整载荷，而第三方登录页（GitHub / Google / DeepSeek
/// Platform）即使通过了 `fnos_site` 也不会执行 mods/shim，更不会把 `FNOSREPORT:…`
/// 写进原生标题栏。见 [`injector::wrap_origin_script`]。
fn create_popup_window<R: Runtime>(
    app: &AppHandle<R>,
    opener_label: &str,
    url: &tauri::Url,
    features: tauri::webview::NewWindowFeatures,
) -> tauri::Result<tauri::WebviewWindow<R>> {
    let n = app
        .state::<AppState>()
        .next_popup
        .fetch_add(1, Ordering::Relaxed);
    let label = format!("popup-{n}");
    let has_position = features.position().is_some();
    let cfg = current(app);
    let allowed_origins = shell_origins(&cfg);
    // 要不要**注册**载荷：目标地址或 opener 当前地址落在闸门放行集合里。
    //
    // 旧判据是「与主地址同源」，在全新安装的机器上会漏：主地址那时是官网门户
    // `https://fnos.net/`，而桌面的 `/signin` 授权弹窗开在 `https://<设备>.fnos.net/`，
    // 不同源 → 连载荷都不注册，登录页的 mods CSS 修正整批失效。判据换成「闸门是否放行」
    // 之后，与运行时闸门（同一份 `shell_origins` + 域名族）完全对齐：注册面可能更宽，
    // 但每个文档执行与否仍由闸门按 `location` 逐篇决定，第三方登录页依旧一个字节不执行。
    let opener_url = app.get_webview(opener_label).and_then(|wv| wv.url().ok());
    let fnos_site = origin_allowed_by_gate(&allowed_origins, url)
        || opener_url
            .as_ref()
            .is_some_and(|u| origin_allowed_by_gate(&allowed_origins, u));
    let mut builder = WebviewWindowBuilder::new(
        app,
        &label,
        // 与 tauri 文档示例一致：窗口先建在 about:blank，`SetNewWindow` 之后由 WebView2
        // 把请求的目标地址接管进来（页面侧感知不到这个中转）。
        WebviewUrl::External("about:blank".parse().expect("about:blank 是合法 URL")),
    )
    .window_features(features)
    .title(tab_initial_title(url))
    .on_document_title_changed(|window, title| {
        // 弹窗标题跟随页面；但控制标题（探针/上报/分片/快捷键）是「页面 → 宿主」
        // 的回传通道，不是页面标题。主窗口走 handle_title 会分流这些前缀，弹窗没有
        // 那条分流——不在这里跳过的话，`FNOSREPORT:…` 会被 set_title 显示到标题栏。
        if !title.trim().is_empty() && !is_control_title(&title) {
            let _ = window.set_title(&title);
        }
    });
    if !has_position {
        builder = builder.center();
    }
    if fnos_site {
        let script = init_script_cached(app, &cfg);
        if !script.is_empty() {
            // `initialization_script` 按 webview 注册，对弹窗里**每一次文档**都会执行。
            // `fnos_site` 只说明弹窗是从 fnOS 页面开出来的，不能说明最终文档还是 fnOS；
            // 载荷本身必须再过一道运行时 origin 闸门——about:blank 与第三方登录页
            // （如 platform.deepseek.com）不执行 mods/shim。见 injector::wrap_origin_script。
            // 闸门的 origin 清单与注册判据同一份（`shell_origins`；域名族由闸门 JS 自己认）：
            // 旧实现只发主地址那一个 origin，登录弹窗导航到 FN Connect 设备域名时同样会被挡。
            let gated = injector::wrap_origin_script(&allowed_origins, &script);
            if !gated.is_empty() {
                // T14f：WebView2 的 SetNewWindow 导航可能快于初始化脚本注册，直接
                // `window.open('/signin')` 的弹窗首个文档会整份漏掉（实测连
                // `__FNOS_SHELL__` 都没有；about:blank 起步的弹窗不受影响）。
                // 页面加载完成后再补一次；载荷已执行过的文档由 `guard_late_script`
                // 里的 `__FNOS_SHELL__` 判据跳过，不会重复装 dock / windowpos。
                let late = injector::guard_late_script(&gated);
                // 补注入的准入与闸门同一判据（这里用宿主侧的同义函数）：旧实现只认
                // 「主地址的 origin」，全新安装的机器上桌面的 `/signin` 弹窗补注入会被
                // 它挡掉。
                let gate_origins = allowed_origins.clone();
                builder = builder
                    .initialization_script(gated)
                    .on_page_load(move |window, payload| {
                        if payload.event() != PageLoadEvent::Finished {
                            return;
                        }
                        if !origin_allowed_by_gate(&gate_origins, payload.url()) {
                            return;
                        }
                        if let Err(e) = window.eval(&late) {
                            flog!("[fnos] 弹窗补注入失败：{e}");
                        }
                    });
            }
        }
    }
    builder.build()
}

/// 同一个 fnOS origin 下，只有桌面根页面和登录/授权页需要完整 mods 载荷；应用标签
/// （`/seek`、`/p`、`/music`、`/app/...`）只需要标签快捷键。
///
/// 为什么：完整载荷包含 dock / windowpos / keepalive 与上游 content-script，它们只对
/// fnOS 桌面有意义；聊天/相册这类高频变更的应用页里，windowpos 的全局 MutationObserver
/// 等观察器纯属额外开销。登录/授权页仍拿完整载荷（授权按钮可见性等 CSS 修正依赖它）。
/// 调用方已保证 `same_origin(desktop, url)`。
fn is_desktop_or_auth_page(desktop: &tauri::Url, url: &tauri::Url) -> bool {
    desktop.path().trim_end_matches('/') == url.path().trim_end_matches('/')
        || is_auth_like_url(url)
}

/// 闸门放行清单里**来自配置**的那部分 origin：桌面地址 + homeUrl / nasUrl +
/// 上游白名单（`mods.enabledOrigins`）。
///
/// homeUrl 是官网入口（默认 https://fnos.net/），nasUrl 是真实桌面；FN Connect 登录后
/// 主 Webview 可能从前者跳到后者，所以两处都要放行。
///
/// `enabledOrigins` 必须在这里：它是上游自己「这个站点要注入」的白名单，`is_recognized`
/// 与上游的 `isWhitelisted` 都认它——闸门不认就会出现「用户加了白名单、状态条说已识别、
/// 但页面一个字节都没注入」这种自相矛盾的状态。
///
/// 光有这份清单**不够**：全新安装时 nasUrl 为空，FN Connect 的设备域名不可能出现在配置里，
/// 所以放行判据还要加上飞牛域名族——见 [`origin_allowed_by_gate`]。
fn shell_origins(cfg: &Config) -> Vec<String> {
    let mut out = Vec::new();
    let mut add = |url: &tauri::Url| {
        let origin = url.origin().ascii_serialization();
        if !out.contains(&origin) {
            out.push(origin);
        }
    };
    add(&resolve_main_url(cfg, None));
    for raw in [&cfg.shell.home_url, &cfg.shell.nas_url] {
        if let Ok(url) = tauri::Url::parse(raw) {
            add(&url);
        }
    }
    for raw in &cfg.mods.enabled_origins {
        if let Ok(url) = tauri::Url::parse(raw) {
            add(&url);
        }
    }
    out
}

/// 宿主侧的**放行判据**，与页面侧闸门（`injector::wrap_origin_script`）逐条同义：
/// ① origin 在 [`shell_origins`] 里，或 ② host 属于飞牛域名族。
///
/// 两处必须一致：宿主认定「可信」的页面，正是闸门放行的那批文档。分叉会造出两种最难查的
/// 状态——「注入真的生效了、状态条却说是未检测到」与「状态条说已识别、页面却是原生样式」
/// （后者正是用户报的「换了台电脑装完注入不生效」）。
fn origin_allowed_by_gate(origins: &[String], url: &tauri::Url) -> bool {
    if origins
        .iter()
        .any(|allowed| allowed == &url.origin().ascii_serialization())
    {
        return true;
    }
    url.host_str().is_some_and(injector::is_fnos_family_host)
}

fn is_trusted_shell_url(cfg: &Config, url: &tauri::Url) -> bool {
    origin_allowed_by_gate(&shell_origins(cfg), url)
}

/// 这个 webview 当前文档是否有资格拿完整 mods 配置（main / 桌面根页面 / 登录授权页）。
/// 应用标签（/seek、/p、/app/...）只拿 keys.js，不接收 mods/local 配置推送。
fn should_receive_full_config(cfg: &Config, label: &str, url: &tauri::Url) -> bool {
    if !is_trusted_shell_url(cfg, url) {
        return false;
    }
    if label == MAIN_WINDOW {
        return true;
    }
    let desktop = resolve_main_url(cfg, None);
    is_desktop_or_auth_page(&desktop, url)
}

/// 控制标题（PROBE / REPORT / CHUNK）只接受来自**受信 origin** 的主文档。
/// 否则主标签被同页导航到第三方后，对方写一个 `FNOSPROBE:`/`FNOSREPORT:` 就能伪造
/// 加载失败或状态证据。`FNOSKEY` 不在此列：激活标签（含第三方应用）本来就允许发。
fn is_control_origin_trusted<R: Runtime>(app: &AppHandle<R>, webview: &Webview<R>) -> bool {
    webview
        .url()
        .ok()
        .map(|url| is_trusted_shell_url(&current(app), &url))
        .unwrap_or(false)
}

/// 按目标页面选择注入档位：
/// - 桌面根页面 / 登录授权页（且与 fnOS 同源）→ 完整载荷（缓存过的初始化脚本）；
/// - 其他应用标签与第三方标签 → 只注入标签快捷键（[`injector::build_core_script`]）。
///
/// 「同源」的比较对象是 [`resolve_main_url`]（与建窗/重建用的是同一份地址解析），
/// 所以 `nasUrl`/`homeUrl` 的切换不会让这两个入口对同一个标签做出不同判断。
fn init_script_for_tab<R: Runtime>(app: &AppHandle<R>, cfg: &Config, url: &tauri::Url) -> String {
    let desktop = resolve_main_url(cfg, None);
    if same_origin(&desktop, url) && is_desktop_or_auth_page(&desktop, url) {
        let script = init_script_cached(app, cfg);
        injector::wrap_origin_script(&shell_origins(cfg), &script)
    } else {
        injector::build_core_script()
    }
}

/// 为一个 `window.open` 请求（或标签条上的「+」）开一个**标签页**：主窗口里的新 Webview，
/// 铺在标签栏（[`TAB_STRIP_H`]）以下的全部区域，创建即激活。
///
/// - 与主标签共用同一组回调（[`tab_webview_builder`]）；注入按 **origin 分档**
///   （[`init_script_for_tab`]）：fnOS 桌面同源的标签拿完整载荷（mods / dock /
///   keepalive / windowpos），第三方标签只拿标签快捷键；
/// - 标签唯一（`tab-1`、`tab-2`…，进程内计数）；
/// - 标题先取主机名第一段，之后跟随 `document.title`（[`handle_title`]）。
pub async fn open_app_tab<R: Runtime>(app: &AppHandle<R>, url: tauri::Url) -> Result<String, String> {
    let window = app
        .get_window(MAIN_WINDOW)
        .ok_or_else(|| "主窗口未打开".to_string())?;
    // 资源上限：标签页是完整 WebView2 实例，页面可控的 window.open 不能无限开。
    let tab_count = app.state::<AppState>().tabs.lock().unwrap().order.len();
    if tab_count >= MAX_OPEN_TABS {
        flog!("[fnos] 标签页已达上限（{MAX_OPEN_TABS}），拒绝新开：{url}");
        return Err(format!("标签页已达上限（{MAX_OPEN_TABS}）"));
    }
    let n = app
        .state::<AppState>()
        .next_tab
        .fetch_add(1, Ordering::Relaxed);
    let label = format!("tab-{n}");
    let cfg = current(app);
    // P0-2：走缓存，不再每次开标签都重读壁纸 + 重做 1 MiB base64。
    // 第三方标签只拿核心脚本（见 [`init_script_for_tab`]）。
    let script = init_script_for_tab(app, &cfg, &url);
    let mut builder = tab_webview_builder(&label, url.clone(), app, None);
    if !script.is_empty() {
        builder = builder.initialization_script(script);
    }
    let (lw, lh) = logical_inner_size(&window).map_err(|e| e.to_string())?;
    // Windows 上 add_child 只能在异步上下文里调（同步命令 / 事件回调里会死锁）——
    // 本函数是 `async fn`，调用方（`new_window_verdict` 的转交任务 / `tab_new` 命令）
    // 都在异步运行时里。
    window
        .add_child(
            builder,
            LogicalPosition::new(0.0, TAB_STRIP_H),
            LogicalSize::new(lw, lh - TAB_STRIP_H),
        )
        .map_err(|e| format!("创建标签页失败：{e}"))?;
    // P1-7：标签的**初始标题**先取主机名（此后由 `document.title` 接管，见 `handle_title`）。
    // 旧实现什么都不写，于是新标签从创建到页面加载完成的那一两秒里显示的是内部 label
    // （「tab-3」），既不像浏览器也不好认。
    {
        let state = app.state::<AppState>();
        state
            .tabs
            .lock()
            .unwrap()
            .titles
            .insert(label.clone(), tab_initial_title(&url));
    }
    // 登记「该页面已有标签」（T14e 复用闸门）：同一页面再次 window.open 时
    // 切过来而不是新开（中继流程的连续 open 就靠它止血）。
    {
        let state = app.state::<AppState>();
        state
            .open_tabs
            .lock()
            .unwrap()
            .insert(page_key(&url), label.clone());
    }
    activate_tab(app, &label);
    Ok(label)
}

/// 新标签页在页面还没报出 `document.title` 之前显示的标题。
///
/// 取主机名第一段（`hermes-studio.nas.fnos.net` → `hermes-studio`）；`about:blank` 这类
/// 没有主机名的（弹窗流程的起始页）给一句固定文案，绝不回落到内部 label。
///
/// **IP 字面量整段保留**（`192.168.1.10` 不该被截成 `192`）：那不是「名字.域名」结构，
/// 按点切毫无意义，只会让人认不出是哪台机器。
fn tab_initial_title(url: &tauri::Url) -> String {
    if let Some(host) = url.host_str() {
        // 剥掉 IPv6 的方括号形式（`Url::host_str()` 会保留 `[...]`），再判字面量：
        // IPv4 / IPv6 整段保留（`192.168.1.10` 不该被截成无意义的 `192`）。
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if bare.parse::<std::net::IpAddr>().is_ok() {
            return bare.to_string();
        }
        let first = bare.split('.').next().unwrap_or(bare);
        if !first.is_empty() {
            return first.to_string();
        }
    }
    if url.scheme() == "about" {
        return "新标签页".to_string();
    }
    url.scheme().to_string()
}

/// 激活一个标签页：显示它、隐藏其余标签页、更新窗口标题并把标签条状态推给 titlebar。
fn activate_tab<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let state = app.state::<AppState>();
    let mut tabs = state.tabs.lock().unwrap();
    if !tabs.order.iter().any(|l| l == label) {
        tabs.order.push(label.to_string());
    }
    tabs.active = label.to_string();
    drop(tabs);
    let window = match app.get_window(MAIN_WINDOW) {
        Some(w) => w,
        None => return,
    };
    for wv in window.webviews() {
        let wv_label = wv.label().to_string();
        if wv_label == TITLEBAR_WEBVIEW {
            continue;
        }
        if wv_label == label {
            let _ = wv.show();
            let _ = wv.set_focus(); // 切换后键盘/滚动焦点跟到新标签
        } else {
            let _ = wv.hide();
        }
    }
    let title = app
        .state::<AppState>()
        .tabs
        .lock()
        .unwrap()
        .titles
        .get(label)
        .cloned();
    if let Some(t) = title {
        let _ = window.set_title(&t);
    }
    push_tabs(app);
    // 隐藏期间窗口可能改过尺寸；切过来时补一次 bounds（resize 只重排激活标签）。
    set_active_content_bounds(&window, label);
}

/// 把标签条状态推给 titlebar Webview（`__FNOS_TABS_SET__`；标签的增删 / 切换 / 改名都会调）。
fn push_tabs<R: Runtime>(app: &AppHandle<R>) {
    let Some(titlebar) = app.get_webview(TITLEBAR_WEBVIEW) else {
        return;
    };
    let state = app.state::<AppState>();
    let tabs = state.tabs.lock().unwrap();
    let payload = serde_json::json!({
        "tabs": tabs.order.iter().map(|label| {
            serde_json::json!({
                "id": label,
                "title": tabs.titles.get(label).cloned().unwrap_or_else(|| label.clone()),
                "active": *label == tabs.active,
                // `main` 是 fnOS 主页标签，不可关闭（关了它就没有「回桌面」的地方了）
                "canClose": label != MAIN_WINDOW,
            })
        }).collect::<Vec<_>>(),
    });
    drop(tabs);
    match serde_json::to_string(&payload) {
        Ok(json) => {
            let _ = titlebar.eval(format!(
                "window.__FNOS_TABS_SET__ && window.__FNOS_TABS_SET__({json});"
            ));
        }
        Err(e) => flog!("[fnos] 标签条状态序列化失败: {e}"),
    }
}

/// 把窗口「是否最大化」推给标签条（`__FNOS_WIN_MAX_SET__`：切换 □ / 还原按钮字形）。
///
/// 建窗后与每次窗口 resize（[`relayout_main_window`]）调用；状态没变时**不 eval**——
/// 拖拽缩放期间 `Resized` 高频到来，每次跨 Webview IPC 是白费。
fn push_win_max<R: Runtime>(app: &AppHandle<R>) {
    let Some(titlebar) = app.get_webview(TITLEBAR_WEBVIEW) else {
        return;
    };
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    let maximized = window.is_maximized().unwrap_or(false);
    let next: u8 = if maximized { 2 } else { 1 };
    if app
        .state::<AppState>()
        .last_maximized
        .swap(next, Ordering::SeqCst)
        == next
    {
        return;
    }
    let _ = titlebar.eval(format!(
        "window.__FNOS_WIN_MAX_SET__ && window.__FNOS_WIN_MAX_SET__({maximized});"
    ));
}

// ---------- 标签页命令（只授 titlebar Webview，见 capabilities/titlebar.json） ----------

/// 标签条上的「+」：新开一个标签页，加载 fnOS 主页地址（与主标签同一份解析规则）。
#[tauri::command]
pub async fn tab_new<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let url = {
        let cfg = current(&app);
        resolve_main_url(&cfg, None)
    };
    open_app_tab(&app, url).await.map(|_| ())
}

/// 切换到指定标签页（显示它、隐藏其余）。
#[tauri::command]
pub async fn tab_switch<R: Runtime>(app: AppHandle<R>, label: String) -> Result<(), String> {
    let exists = app
        .state::<AppState>()
        .tabs
        .lock()
        .unwrap()
        .order.contains(&label);
    if !exists {
        return Err(format!("标签页不存在：{label}"));
    }
    activate_tab(&app, &label);
    Ok(())
}

/// 关闭指定标签页（`main` 不可关）。关掉的是当前标签时，优先激活它左边的邻居。
#[tauri::command]
pub async fn tab_close<R: Runtime>(app: AppHandle<R>, label: String) -> Result<(), String> {
    if label == MAIN_WINDOW {
        return Err("fnOS 主页标签不可关闭".to_string());
    }
    let (activate, webview) = {
        let state = app.state::<AppState>();
        let mut tabs = state.tabs.lock().unwrap();
        let Some(pos) = tabs.order.iter().position(|l| *l == label) else {
            return Err(format!("标签页不存在：{label}"));
        };
        tabs.order.remove(pos);
        tabs.titles.remove(&label);
        // T14e：同时清掉复用登记表里 value == 该标签的条目
        //（命中的失效条目在 verdict 里也会自愈清理，这里显式清避免残留）
        state
            .open_tabs
            .lock()
            .unwrap()
            .retain(|_, l| l != &label);
        let activate = if tabs.active == label {
            // 优先左邻（浏览器语义），越界回第一个
            Some(tabs.order[pos.saturating_sub(1)].clone())
        } else {
            None
        };
        let webview = app.get_webview(&label);
        (activate, webview)
    };
    if let Some(wv) = webview {
        let _ = wv.close(); // 从窗口移除这个 Webview（资源由 tauri 回收）
    }
    if let Some(next) = activate {
        activate_tab(&app, &next);
    } else {
        push_tabs(&app);
    }
    Ok(())
}

/// 全屏开关（titlebar 的全屏按钮专用；ESC 走 [`exit_fullscreen_if_needed`]）。
///
/// 副作用不止窗口本身：
/// - 主页面里的 fnOS 桌面任务栏（Dock）由 `inject/dock.js` 的
///   `__FNOS_DOCK_FULLSCREEN__` 强制隐藏，并回收它让出的那条宽度；
/// - 标题栏按钮字形经 `__FNOS_WIN_FULLSCREEN_SET__` 推送，避免「点完不知道状态」。
#[tauri::command]
pub fn set_fullscreen<R: Runtime>(app: AppHandle<R>, value: bool) -> Result<(), String> {
    let window = app
        .get_window(MAIN_WINDOW)
        .ok_or_else(|| "主窗口未打开".to_string())?;
    // 不按「与窗口当前状态相同」提前返回：enter/exit 同时负责标题栏/Dock/任务栏的
    // 副作用，重复调用必须幂等且能把「窗口被外部改回普通态」的残留状态拉回来。
    let result = if value {
        enter_fullscreen(&app, &window)
    } else {
        exit_fullscreen(&app, &window)
    };
    result.map_err(|e| format!("切换全屏失败：{e}"))
}

/// Windows 桌面任务栏是否**由本壳**藏起来（只还原自己藏过的，不碰用户/其它程序隐藏的状态）。
#[cfg(windows)]
static TASKBAR_HIDDEN_BY_SHELL: AtomicBool = AtomicBool::new(false);
/// 被本壳隐藏的任务栏 HWND（主屏 `Shell_TrayWnd` + 各副屏 `Shell_SecondaryTrayWnd`）。
#[cfg(windows)]
static HIDDEN_TASKBARS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

#[cfg(windows)]
fn taskbar_marker_path() -> std::path::PathBuf {
    paths::config_dir().join("taskbar-hidden.flag")
}

/// 枚举所有任务栏窗口（主屏 + 副屏）。用 isize 存 HWND，避免 static 里的裸指针 Send 问题。
#[cfg(windows)]
fn taskbar_handles() -> Vec<isize> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW};

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam as *mut Vec<isize>);
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if n > 0 {
            let name = String::from_utf16_lossy(&buf[..n as usize]);
            if name == "Shell_TrayWnd" || name == "Shell_SecondaryTrayWnd" {
                out.push(hwnd as isize);
            }
        }
        1
    }

    let mut out = Vec::new();
    unsafe {
        EnumWindows(Some(collect), &mut out as *mut _ as LPARAM);
    }
    out
}

/// 显示/隐藏 Windows 桌面任务栏（主屏 + 副屏）。
///
/// 这是**系统级**副作用：只在 `IsWindowVisible` 为真时由本壳隐藏；退出全屏、关窗到托盘、
/// 窗口销毁、托盘退出时还原。隐藏期间会在配置目录写 `taskbar-hidden.flag`，若进程被强杀，
/// 下次启动据该标记把任务栏恢复，避免用户永远没有任务栏。
#[cfg(windows)]
fn set_windows_taskbar_visible(visible: bool) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindow, SW_HIDE, SW_SHOW};

    if visible {
        if !TASKBAR_HIDDEN_BY_SHELL.swap(false, Ordering::SeqCst) {
            return;
        }
        let mut hidden = HIDDEN_TASKBARS.lock().unwrap();
        for handle in hidden.drain(..) {
            unsafe {
                ShowWindow(handle as HWND, SW_SHOW);
            }
        }
        let _ = std::fs::remove_file(taskbar_marker_path());
    } else {
        if TASKBAR_HIDDEN_BY_SHELL.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut hidden = HIDDEN_TASKBARS.lock().unwrap();
        hidden.clear();
        for handle in taskbar_handles() {
            if unsafe { IsWindowVisible(handle as HWND) } != 0 {
                hidden.push(handle);
                unsafe {
                    ShowWindow(handle as HWND, SW_HIDE);
                }
            }
        }
        if !hidden.is_empty() {
            let _ = std::fs::write(taskbar_marker_path(), b"1");
        }
    }
}

#[cfg(not(windows))]
fn set_windows_taskbar_visible(_visible: bool) {}

/// 启动时恢复任务栏：上一次进程若在全屏中被强杀，`taskbar-hidden.flag` 还在，
/// 此时无条件恢复所有任务栏并清掉标记（正常退出路径不会留下它）。
pub fn restore_taskbar_on_startup() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOW};

        if taskbar_marker_path().exists() {
            for handle in taskbar_handles() {
                unsafe {
                    ShowWindow(handle as HWND, SW_SHOW);
                }
            }
            let _ = std::fs::remove_file(taskbar_marker_path());
        }
        TASKBAR_HIDDEN_BY_SHELL.store(false, Ordering::SeqCst);
        HIDDEN_TASKBARS.lock().unwrap().clear();
    }
}

/// 兜底还原任务栏（窗口销毁 / 托盘退出 / 主进程正常结束时调用；幂等）。
pub fn restore_taskbar() {
    set_windows_taskbar_visible(true);
}

/// 全屏顶边热区（逻辑像素）：鼠标进入窗口顶部这么多像素时悬浮出标题栏。
const FULLSCREEN_TITLEBAR_HOT_ZONE: f64 = 4.0;
/// 标题栏显示后，鼠标下移超过「标题栏高 + 这么多像素」才收起（防边缘抖动）。
const FULLSCREEN_TITLEBAR_HIDE_SLACK: f64 = 4.0;

/// 全屏悬浮标题栏 watcher 的代次。每次进入/退出全屏自增；旧线程在一个轮询周期内自行退出。
static FULLSCREEN_WATCH_GEN: AtomicU64 = AtomicU64::new(0);
/// 全屏时标题栏当前是否悬浮显示。
static FULLSCREEN_TITLEBAR_SHOWN: AtomicBool = AtomicBool::new(false);
/// 本壳认为当前是否处于全屏（用于检测窗口被外部改回普通态时拉回标题栏/任务栏状态）。
static FULLSCREEN_STATE: AtomicBool = AtomicBool::new(false);

fn stop_fullscreen_watch() {
    FULLSCREEN_WATCH_GEN.fetch_add(1, Ordering::SeqCst);
}

/// 停掉全屏悬浮标题栏 watcher（窗口销毁 / 退出全屏）。
pub fn stop_fullscreen_watcher() {
    stop_fullscreen_watch();
}

/// 鼠标位置（物理屏幕坐标）；无桌面会话 / 非 Windows 时返回 None。
#[cfg(windows)]
fn cursor_screen_pos() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
}

#[cfg(not(windows))]
fn cursor_screen_pos() -> Option<(i32, i32)> {
    None
}

/// 把标题栏子 Webview 提到内容 Webview 之上（Tauri 的 child Webview 没有 z-order API）。
#[cfg(windows)]
fn bring_webview_to_front<R: Runtime>(webview: &tauri::Webview<R>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    let _ = webview.with_webview(|platform| {
        let mut hwnd = Default::default();
        if unsafe { platform.controller().ParentWindow(&mut hwnd) }.is_ok() {
            unsafe {
                SetWindowPos(
                    hwnd.0,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    });
}

#[cfg(not(windows))]
fn bring_webview_to_front<R: Runtime>(_webview: &tauri::Webview<R>) {}

/// 全屏时显示/隐藏悬浮标题栏（位置/尺寸由 [`relayout_main_window`] 维护）。
fn set_fullscreen_titlebar_visible<R: Runtime>(app: &AppHandle<R>, visible: bool) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    if !window.is_fullscreen().unwrap_or(false) {
        return;
    }
    let Some(titlebar) = app.get_webview(TITLEBAR_WEBVIEW) else {
        return;
    };
    if visible {
        let _ = titlebar.show();
        bring_webview_to_front(&titlebar);
        FULLSCREEN_TITLEBAR_SHOWN.store(true, Ordering::SeqCst);
    } else {
        let _ = titlebar.hide();
        FULLSCREEN_TITLEBAR_SHOWN.store(false, Ordering::SeqCst);
    }
}

/// 全屏顶边悬停 watcher：60ms 轮询鼠标位置。
/// 进入顶部热区 → 悬浮标题栏；鼠标离开标题栏区域 → 收起。
fn start_fullscreen_titlebar_watcher<R: Runtime>(app: AppHandle<R>) {
    let generation = FULLSCREEN_WATCH_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        while FULLSCREEN_WATCH_GEN.load(Ordering::SeqCst) == generation {
            let Some(window) = app.get_window(MAIN_WINDOW) else {
                break;
            };
            if !window.is_fullscreen().unwrap_or(false) {
                break;
            }
            let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) else {
                break;
            };
            let scale = window.scale_factor().unwrap_or(1.0);
            let strip = (TAB_STRIP_H * scale) as i32;
            let hot = (FULLSCREEN_TITLEBAR_HOT_ZONE * scale).round() as i32;
            let hide_after = strip + (FULLSCREEN_TITLEBAR_HIDE_SLACK * scale).round() as i32;
            let Some((cx, cy)) = cursor_screen_pos() else {
                break;
            };
            let in_window = cx >= pos.x
                && cx < pos.x + size.width as i32
                && cy >= pos.y
                && cy < pos.y + size.height as i32;
            let in_hot = in_window && cy <= pos.y + hot;
            let in_titlebar = in_window && cy <= pos.y + hide_after;
            let shown = FULLSCREEN_TITLEBAR_SHOWN.load(Ordering::SeqCst);
            if !shown && in_hot {
                let app_for_main = app.clone();
                let _ = app.run_on_main_thread(move || {
                    set_fullscreen_titlebar_visible(&app_for_main, true);
                });
            } else if shown && !in_titlebar {
                let app_for_main = app.clone();
                let _ = app.run_on_main_thread(move || {
                    set_fullscreen_titlebar_visible(&app_for_main, false);
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
    });
}

/// 进入全屏：全屏窗口 + 内容铺满 + 标题栏先隐藏 + 启动顶部悬停 watcher。
fn enter_fullscreen<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::Window<R>,
) -> tauri::Result<()> {
    window.set_fullscreen(true)?;
    FULLSCREEN_STATE.store(true, Ordering::SeqCst);
    if let Ok(size) = window.inner_size() {
        relayout_main_window(window, size);
    }
    if let Some(titlebar) = app.get_webview(TITLEBAR_WEBVIEW) {
        let _ = titlebar.hide();
    }
    FULLSCREEN_TITLEBAR_SHOWN.store(false, Ordering::SeqCst);
    start_fullscreen_titlebar_watcher(app.clone());
    apply_fullscreen_side_effects(app, true);
    Ok(())
}

/// 退出全屏：停 watcher + 恢复普通布局 + 标题栏常显。
fn exit_fullscreen<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::Window<R>,
) -> tauri::Result<()> {
    window.set_fullscreen(false)?;
    FULLSCREEN_STATE.store(false, Ordering::SeqCst);
    stop_fullscreen_watch();
    if let Some(titlebar) = app.get_webview(TITLEBAR_WEBVIEW) {
        let _ = titlebar.show();
    }
    FULLSCREEN_TITLEBAR_SHOWN.store(false, Ordering::SeqCst);
    if let Ok(size) = window.inner_size() {
        relayout_main_window(window, size);
    }
    apply_fullscreen_side_effects(app, false);
    Ok(())
}

/// 退出全屏（若当前全屏）。关窗到托盘 / 窗口重建前调用，避免把任务栏留在一个隐藏窗口后面。
pub fn leave_fullscreen<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    if window.is_fullscreen().unwrap_or(false) {
        let _ = exit_fullscreen(app, &window);
    }
}

/// 窗口的全屏状态被**外部**改变时（双击标题栏触发 toggle_maximize、Win+Up/Down 等），
/// 把标题栏 / Dock / Windows 任务栏拉回与窗口一致。由 `main.rs` 的 Resized 事件调用。
pub fn reconcile_fullscreen<R: Runtime>(window: &tauri::Window<R>) {
    let actual = window.is_fullscreen().unwrap_or(false);
    if actual == FULLSCREEN_STATE.load(Ordering::SeqCst) {
        return;
    }
    let app = window.app_handle();
    if actual {
        let _ = enter_fullscreen(app, window);
    } else {
        let _ = exit_fullscreen(app, window);
    }
}

/// 全屏的页面/标题栏联动（[`set_fullscreen`] 与 ESC 通道共用）。
fn apply_fullscreen_side_effects<R: Runtime>(app: &AppHandle<R>, on: bool) {
    // Windows 任务栏在 fullscreen 下仍会盖在窗口底部（实测截图），必须显式藏掉。
    set_windows_taskbar_visible(!on);
    if let Some(webview) = app.get_webview(MAIN_WINDOW) {
        let _ = webview.eval(format!(
            "window.__FNOS_DOCK_FULLSCREEN__ && window.__FNOS_DOCK_FULLSCREEN__({on});"
        ));
    }
    if let Some(titlebar) = app.get_webview(TITLEBAR_WEBVIEW) {
        let _ = titlebar.eval(format!(
            "window.__FNOS_WIN_FULLSCREEN_SET__ && window.__FNOS_WIN_FULLSCREEN_SET__({on});"
        ));
    }
}

/// 只在确实全屏时退出（ESC 通道用；非全屏的 Esc 留给页面自己）。
fn exit_fullscreen_if_needed<R: Runtime>(app: &AppHandle<R>) {
    leave_fullscreen(app);
}

/// 内容 Webview 的 bounds（全屏时从 y=0 铺满；普通态在 32px 标题栏之下）。
fn content_rect(
    size: tauri::PhysicalSize<u32>,
    scale: f64,
    fullscreen: bool,
) -> tauri::Rect {
    let strip = (TAB_STRIP_H * scale) as i32;
    let (y, h) = if fullscreen {
        (0, size.height)
    } else {
        (strip, size.height.saturating_sub(strip as u32))
    };
    tauri::Rect {
        position: tauri::Position::Physical(tauri::PhysicalPosition::new(0, y)),
        size: tauri::Size::Physical(tauri::PhysicalSize::new(size.width, h)),
    }
}

/// 只把**当前激活**标签摆到内容区。resize 时隐藏标签不重排——它们的页面不该因为
/// 用户拖窗口而白做一次 layout/reflow；等真正切过去时由 [`activate_tab`] 补上。
fn set_active_content_bounds<R: Runtime>(window: &tauri::Window<R>, label: &str) {
    let Ok(size) = window.inner_size() else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let fullscreen = window.is_fullscreen().unwrap_or(false);
    if let Some(wv) = window.webviews().into_iter().find(|wv| wv.label() == label) {
        let _ = wv.set_bounds(content_rect(size, scale, fullscreen));
    }
}

/// 窗口 resize 后把标题栏与**当前激活标签**摆回正确位置（`main.rs` 的 `Resized` 转发）。
/// 子 Webview **不会**随窗口自动重排——最大化 / 还原 / 拖边缘都靠这一趟。
pub fn relayout_main_window<R: Runtime>(window: &tauri::Window<R>, size: tauri::PhysicalSize<u32>) {
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let strip = (TAB_STRIP_H * scale) as i32;
    // 全屏时标题栏是**悬浮层**：内容 Webview 铺满整个窗口（从 y=0 开始），标题栏
    // 仍保持 0..32px 的位置，但只在顶部热区悬停时显示、盖在内容上方。
    let fullscreen = window.is_fullscreen().unwrap_or(false);
    let active = window
        .app_handle()
        .state::<AppState>()
        .tabs
        .lock()
        .unwrap()
        .active
        .clone();
    for wv in window.webviews() {
        let label = wv.label();
        if label == TITLEBAR_WEBVIEW {
            let bounds = tauri::Rect {
                position: tauri::Position::Physical(tauri::PhysicalPosition::new(0, 0)),
                size: tauri::Size::Physical(tauri::PhysicalSize::new(
                    size.width,
                    strip as u32,
                )),
            };
            let _ = wv.set_bounds(bounds);
        } else if label == active.as_str() {
            let _ = wv.set_bounds(content_rect(size, scale, fullscreen));
        }
        // 隐藏标签保持原 bounds；切换时由 set_active_content_bounds 补。
    }
    // 最大化 / 还原都会走 Resized：把新状态推给标签条（切换 □ / 还原按钮字形）。
    // push_win_max 内部按 last_maximized 去重，状态没变不会 eval。
    push_win_max(window.app_handle());
}

/// 建**内置错误页**窗口（Task 11 / spec §12.3「主窗口加载失败/离线」）。
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
) -> tauri::Result<tauri::Window<R>> {
    set_error_state(app, info);
    let payload = serde_json::to_string(info).unwrap_or_else(|_| "{}".into());
    // T14d：错误页与主窗口同构 —— 无边框 Window + 一行式标题栏 Webview（标签 +
    // ─ □ ×）+ 内容 Webview（错误页内容就是 `main` 标签，只是加载的是本地错误资产、
    // 且**没有任何注入**）。
    let mut builder = WindowBuilder::new(app, MAIN_WINDOW)
        .title("fnOS — 页面加载失败")
        .inner_size(cfg.shell.window.w, cfg.shell.window.h)
        .decorations(false)
        // 与 build_main_window_with 同理（修复轮 7）：背景钉标题栏色，webview 缝不显白线
        .background_color(tauri::window::Color(0x1b, 0x1c, 0x1e, 0xff));
    // 与 [`build_main_window_with`] 同理（T14c 修复轮 20）：位置建窗时就带上，
    // 不让错误页窗口也「先在左上角落脚、再跳走」。
    match (cfg.shell.window.x, cfg.shell.window.y) {
        (Some(x), Some(y)) if x > -10000.0 && y > -10000.0 => {
            builder = builder.position(x, y);
        }
        _ => builder = builder.center(),
    }
    if !start_visible {
        builder = builder.visible(false);
    }
    let window = builder.build()?;
    // 与 build_main_window_with 同理：标签状态与复用登记表一起复位（错误页只有一个 main 标签）
    {
        let state = app.state::<AppState>();
        *state.tabs.lock().unwrap() = TabState::with_main();
        state.open_tabs.lock().unwrap().clear();
        state.last_maximized.store(0, Ordering::SeqCst);
    }
    let (lw, lh) = logical_inner_size(&window)?;
    let titlebar = WebviewBuilder::new(TITLEBAR_WEBVIEW, WebviewUrl::App("titlebar.html".into()));
    window.add_child(
        titlebar,
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(lw, TAB_STRIP_H),
    )?;
    let app_for_title = app.clone();
    let error_webview =
        WebviewBuilder::new(MAIN_WINDOW, WebviewUrl::App(ERROR_PAGE_ASSET.into()))
            .on_document_title_changed(move |webview, title| {
                handle_title(&webview, &title, &app_for_title);
            })
            .on_page_load(move |webview, payload_event| {
                if payload_event.event() != PageLoadEvent::Finished {
                    return;
                }
                flog!(
                    "[fnos] 错误页已加载（地址 {}；本页不注册 mods 初始化脚本）",
                    payload_event.url()
                );
                let _ = webview.eval(format!(
                    "window.__FNOS_SET_ERROR__ && window.__FNOS_SET_ERROR__({payload});"
                ));
            });
    window.add_child(
        error_webview,
        LogicalPosition::new(0.0, TAB_STRIP_H),
        LogicalSize::new(lw, lh - TAB_STRIP_H),
    )?;
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

/// 是否是本壳的**控制标题**（探针 / 上报 / 分片 / 标签快捷键）——这些字符串走
/// `document.title` 只是「页面 → 宿主」的传输，不是页面自己的标题。
///
/// 主窗口的 [`handle_title`] 按前缀分流后**不**把它们镜像到窗口标题；任何把
/// `document.title` 直接镜像到原生标题的地方（典型：弹窗的
/// `on_document_title_changed`）都必须先用它过滤。四个前缀集中在这里，避免将来
/// 新增一条控制通道时漏掉某个镜像点。
fn is_control_title(title: &str) -> bool {
    title.starts_with(PROBE_TITLE_PREFIX)
        || title.starts_with(report::CHUNK_TITLE_PREFIX)
        || title.starts_with(report::REPORT_TITLE_PREFIX)
        || title.starts_with(KEY_TITLE_PREFIX)
}

/// 主窗口 `document.title` 变化：两条控制前缀走这里，其余照旧镜像到窗口标题。
///
/// 两个前缀的**副作用必须一致**：控制标题是「页面 → 宿主」的回传通道，不是给用户看的标题
/// ——既不镜像到窗口标题（`set_title`），也不进 UI，只在控制台留一行**固定短语 + 类型**
/// （绝不回显页面可控正文）。Task 11 的探针分支就是这样做的，Task 13a 的上报分支照抄。
pub fn handle_title<R: Runtime>(webview: &Webview<R>, title: &str, app: &AppHandle<R>) {
    // —— P0-4：三条「页面 → 宿主」的控制前缀**只认主标签**。
    //
    // 这三条通道原先只按前缀分流、不看 webview 归属，于是任意一个**第三方应用标签页**
    // 只要写一句 `document.title = 'FNOSPROBE:{"err":true,…}'`，宿主就会 `mark_failed`，
    // 把整个主窗口重建成错误页——所有标签一起消失。上报通道同理：外部页的一条伪造上报
    // 会覆盖 `page_report` 槽位，让设置窗的状态条从「已注入」错乱成别的结论。
    //
    // 归属判据必须是 `MAIN_WINDOW`（fnOS 桌面本体）：错误页/看门狗/退避/状态条证据
    // 全部是围绕「fnOS 主页这一次加载」设计的。`titlebar` 是本地资产页、`tab-*` 是外部
    // 应用页，两者都没有资格替主标签回话。
    let is_main = webview.label() == MAIN_WINDOW;
    if let Some(payload) = title.strip_prefix(PROBE_TITLE_PREFIX) {
        // 探针标题是「页面 → 宿主」的回传通道，不是给用户看的标题：不镜像、只处理。
        if is_main && is_control_origin_trusted(app, webview) {
            on_load_probe(app, payload);
        }
        return;
    }
    if let Some(payload) = title.strip_prefix(report::CHUNK_TITLE_PREFIX) {
        if is_main && is_control_origin_trusted(app, webview) {
            on_page_chunk(webview, payload);
        }
        return;
    }
    if let Some(payload) = title.strip_prefix(report::REPORT_TITLE_PREFIX) {
        if is_main && is_control_origin_trusted(app, webview) {
            on_page_report(webview, payload);
        }
        return;
    }
    // P0-3：标签页快捷键。任何标签页都能发（焦点在哪个标签就由哪个标签发），但**只认
    // 当前激活标签**：后台标签写 `FNOSKEY:close` 关掉用户正在用的标签不是「等价于点标签条」，
    // 而是一条后台页面打断前台操作的通道。冷却/白名单保留为纵深防御。
    if let Some(action) = title.strip_prefix(KEY_TITLE_PREFIX) {
        let is_active = app.state::<AppState>().tabs.lock().unwrap().active == webview.label();
        if is_active {
            handle_tab_key(app, action);
        }
        return;
    }
    // 现场排障 + 运行期证据（错误页会把自检结论写进标题，见 ui/settings/error.html）。
    //
    // `title` 是**页面可控**的（`document.title`）：日志只是它的一个渲染，必须过
    // `report::log_safe`——否则一句 `document.title = "x\n[fnos] 页面上报已接受：…"`
    // 就能在 stderr 里插出一行伪造日志。`set_title` 拿到的仍是**原文**。
    //
    // 先做**变化判定**：很多页面（含 fnOS 自己的保活）会反复写同一个标题；重复的
    // 「写日志 + 全量 eval 推标签条」是页面可控的 UI/磁盘放大器。标题没变就直接返回。
    let label = webview.label();
    if label == TITLEBAR_WEBVIEW {
        return;
    }
    let (changed, active, registered) = {
        let state = app.state::<AppState>();
        let mut tabs = state.tabs.lock().unwrap();
        let registered = tabs.order.iter().any(|l| l == label);
        let changed = !registered || tabs.titles.get(label).map(String::as_str) != Some(title);
        if registered && changed {
            tabs.titles.insert(label.to_string(), title.to_string());
        }
        (changed, tabs.active == label, registered)
    };
    if !changed {
        return;
    }
    if !registered {
        return;
    }
    // 只记录**当前激活标签**的标题（它就是窗口标题）：后台标签的标题照样进标签条，
    // 但不必为每一次未读计数/播放进度变化写一行日志 + 刷一次盘。500ms 节流兜住高频变化。
    if active {
        let now = now_unix_ms();
        let state = app.state::<AppState>();
        let last = state.last_title_log.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= TITLE_LOG_INTERVAL_MS
            && state
                .last_title_log
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            flog!("[fnos] 主窗口标题: {}", report::log_safe(title));
        }
        let _ = webview.window().set_title(title);
    }
    push_tabs(app);
}

/// 处理一条标签页快捷键（P0-3）：`next` / `prev` / `close` / `new`，以及全屏退出
/// `exit-fullscreen`（Esc）。其余一律丢弃。
///
/// ## 为什么这条通道可以接受「页面写得动」
///
/// `document.title` 是页面可控的，任何站点都能写 `FNOSKEY:close`。三个理由让它不构成
/// 安全问题：① 动作只有这几个，且**每一个都等价于用户亲手点标签条 / 按 Esc**（关/切/开
/// 标签、退出全屏），不触碰配置、文件系统、网络或任何 IPC 命令；② 调用方
/// （[`handle_title`]）只接受**当前激活标签**发来的命令——后台页不能替你关掉正在用的
/// 标签；③ 下面的冷却把它压成「120ms 最多一次」，刷屏不产生可感知影响。
///
/// `exit-fullscreen` 走冷却**之前**的分支：它只在确实全屏时动作一次，重复调用是 no-op；
/// 而 Esc 是页面自己的常用键，不该被标签快捷键的冷却窗口吞掉。
///
/// 动作不合法就静默丢弃（不写日志——这条通路人人都能触发，写日志等于给页面一个日志放大器）。
fn handle_tab_key<R: Runtime>(app: &AppHandle<R>, action: &str) {
    use std::sync::atomic::Ordering;
    let action = action.trim();
    if action == "exit-fullscreen" {
        exit_fullscreen_if_needed(app);
        return;
    }
    if !matches!(action, "next" | "prev" | "close" | "new") {
        return;
    }
    // 冷却：CAS 抢占本次「时间片」。取不到（别的线程刚抢过）或还没到点就丢弃。
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = app
        .state::<AppState>()
        .last_key_at
        .load(Ordering::Relaxed);
    if now.saturating_sub(last) < KEY_COOLDOWN_MS {
        return;
    }
    if app
        .state::<AppState>()
        .last_key_at
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return; // 并发的那一个已经执行
    }

    match action {
        "new" => {
            let url = {
                let cfg = current(app);
                resolve_main_url(&cfg, None)
            };
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = open_app_tab(&app, url).await {
                    flog!("[fnos] 快捷键新开标签失败：{e}");
                }
            });
        }
        "close" => {
            let active = app.state::<AppState>().tabs.lock().unwrap().active.clone();
            if active == MAIN_WINDOW {
                return; // 主页标签不可关（与 tab_close 同一条规则，这里直接不动）
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = tab_close(app, active).await {
                    flog!("[fnos] 快捷键关闭标签失败：{e}");
                }
            });
        }
        "next" | "prev" => {
            let target = {
                let state = app.state::<AppState>();
                let tabs = state.tabs.lock().unwrap();
                neighbor_tab(&tabs.order, &tabs.active, action == "next")
            };
            if let Some(label) = target {
                activate_tab(app, &label);
            }
        }
        _ => {}
    }
}

/// 循环取邻居标签（浏览器语义：末尾下一个回到第一个）。
///
/// 抽成纯函数是为了能单测「首尾环绕 / 单标签 / 当前标签不在表里」这几种真实会出现的形状。
fn neighbor_tab(order: &[String], active: &str, forward: bool) -> Option<String> {
    if order.len() < 2 {
        return None; // 只有一个标签：Ctrl+Tab 无事可做
    }
    let pos = order.iter().position(|l| l == active)?;
    let n = order.len();
    let next = if forward {
        (pos + 1) % n
    } else {
        (pos + n - 1) % n
    };
    Some(order[next].clone())
}

/// 收到一条页面上报（`document.title` 的 `FNOSREPORT:` 通道；Task 13a）。
/// 四道闸门全在 [`report::validate`] 里（字节上限 → 合法 JSON → 必须是对象 → `type` 白名单），
/// 未通过就丢掉并留一行固定短语，**绝不 panic、绝不把页面可控正文写进日志或 UI**。
/// 通过后连同**上报文档的 origin 与 URL** 一起存进 `AppState`（内存）：两者用于「换了页面/
/// 换了文档就不认旧上报」的诚实性判定（见 [`get_page_report`] / [`report::ReportEntry`]）。
///
/// 唯一一处会打印页面可控字段的日志是 [`report::accepted_log_line`]：`type` 与 `dir` 都在
/// 允许表里过一遍，白名单外一律渲染成固定串 `report::UNKNOWN`（Important 1 的日志注入修复）。
fn on_page_report<R: Runtime>(window: &Webview<R>, payload: &str) {
    let value = match report::validate(payload) {
        Ok(v) => v,
        Err(reason) => {
            flog!(
                "[fnos] 页面上报被拒（{}；{} 字节）",
                reason.as_str(),
                payload.len()
            );
            // 截断假象：标题通道的实测上限是 4096 字节（见 report::TITLE_CHANNEL_MAX_BYTES），
            // 超过它的上报到达宿主时已经被截断，只能表现为「不是合法 JSON」。把这一情形标出来，
            // 否则后来的人会去追一个并不存在的语法 bug（T13b 的大载荷上报正是高风险场景）。
            if reason == report::Reject::NotJson && report::truncation_suspected(payload.len()) {
                flog!(
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
fn document_identity<R: Runtime>(window: &Webview<R>) -> (Option<String>, Option<String>) {
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
    window: &Webview<R>,
    value: Value,
    origin: Option<String>,
    url: Option<String>,
    bytes: usize,
) {
    let ty = value.get("type").and_then(Value::as_str).unwrap_or("");
    let dir = value.get("dir").and_then(Value::as_str);
    flog!(
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
fn on_page_chunk<R: Runtime>(window: &Webview<R>, payload: &str) {
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
            flog!("[fnos] 分片上报已接受：seq={seq} total={total}");
        }
        report::ChunkStep::Rejected(reason) => {
            flog!(
                "[fnos] 分片上报被拒（{}；这一片 {} 字节）",
                reason.as_str(),
                payload.len()
            );
        }
        report::ChunkStep::Complete(assembled) => {
            match report::validate(&assembled) {
                Ok(value) => {
                    flog!("[fnos] 分片上报已到齐（{} 字节）", assembled.len());
                    store_report(window, value, origin, url, assembled.len());
                }
                Err(reason) => {
                    // 组装完成但内容不合法：同样只留固定短语（不打印正文）
                    flog!(
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
    let current_url = app.get_webview(MAIN_WINDOW).and_then(|w| w.url().ok());
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
    flog!("[fnos] 主窗口页面已加载: {url}");
    // 全屏是宿主侧状态，但 Dock 的隐藏类是页面 DOM 上的：页内导航 / 重载换了一篇文档后
    // 要重新落一次（否则全屏中刷新页面，任务栏会自己冒回来）。
    if let Some(window) = app.get_window(MAIN_WINDOW) {
        if window.is_fullscreen().unwrap_or(false) {
            apply_fullscreen_side_effects(app, true);
        }
    }
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
        flog!("[fnos] URL 判定为 WebView2 错误页：{url}");
        mark_failed(app, generation, url, &reason);
        return;
    }
    // URL 上看不出失败（实测失败导航的 Source 仍是请求地址）→ 让页面自己回答
    flog!("[fnos] 页面 URL 未显示错误页，交由页面自检探针判定: {url}");
    if let Some(webview) = app.get_webview(MAIN_WINDOW) {
        let _ = webview.eval(LOAD_PROBE_JS);
    }
}

/// 页面自检探针的回话；只有明确的「这是错误页」才降级。
///
/// **结论是哪种都要先消费掉槽位**（P0-4 的另一半）：旧实现只在「判定失败」那条路上把
/// `probe_generation` 清成 `None`（`mark_failed` 里），判定**成功**的分支在
/// `probe_verdict` 返回 `None` 时直接 `return`，于是槽位永久停在 `Some(generation)`。
/// 后果是同一世代里任何迟到（或伪造）的探针回话都还能再走一遍 `mark_failed`——而
/// 探针本来就只该回一次话。
fn on_load_probe<R: Runtime>(app: &AppHandle<R>, payload: &str) {
    let verdict = probe_verdict(payload);
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
    // 回话 = 页面自己说「我不是错误页」：这是成功路径，只消费槽位，不做别的。
    let Some(reason) = verdict else {
        return;
    };
    flog!("[fnos] 页面自检判定为加载失败：{reason}");
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
    flog!("[fnos] 主窗口加载失败：{url} — {reason}");
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
        flog!("[fnos] 切换错误页失败: {e}");
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
            flog!("[fnos] {reason}");
            mark_failed(&handle, generation, &url, &reason);
        });
    if let Err(e) = spawned {
        flog!("[fnos] 加载看门狗线程启动失败（超时兜底失效）: {e}");
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
            flog!("[fnos] 错误页自动重试（退避 {delay_ms} ms）");
            if let Err(e) = recreate_main_window(&app, None) {
                flog!("[fnos] 自动重试重建主窗口失败: {e}");
            }
        });
    if let Err(e) = spawned {
        flog!("[fnos] 自动重试线程启动失败: {e}");
    }
}

/// 主窗口地址：`url` 覆盖 → 配置 `nasUrl`（配了就直达 NAS 桌面，T14e）→
/// 配置 `homeUrl` → `DEFAULT_HOME_URL`，逐级回落。
///
/// Finding 1：用户可控文本**永不**进入 `expect`。每一级都过 `config::parse_web_url`
/// （`nas_target` / `home_url_or_default` 本身已兜底），因此只有「常量被改坏」才可能
/// 走到最后的 `unreachable!`——那条假设由 `config::tests::default_home_url_is_a_valid_web_url` 锁定。
pub fn resolve_main_url(cfg: &Config, url_override: Option<&str>) -> tauri::Url {
    // T14e（用户实测「无法进入桌面」）：窗口重建后地址回到 homeUrl（fnos.net 门户），
    // 要再走一遍 FN Connect 连接流程才回 NAS 桌面。用户配置了 `shell.nasUrl` 时优先用它
    // ——「NAS 地址」就是他要的桌面。nas_target() 已做 http(s) 校验（空/非法 → "" →
    // 落到下一级），不会 panic。
    let candidates = [
        url_override.unwrap_or(""),
        cfg.shell.nas_target().unwrap_or(""),
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

    let Some(win) = app.get_window(MAIN_WINDOW) else {
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
        flog!("[fnos] 重新加载主窗口失败: {e}");
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
        flog!("[fnos] 主窗口重建失败: {e}");
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
        .get_webview(MAIN_WINDOW)
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

/// 打开随包的合规件（「关于」页两个路径旁的「查看」按钮）。
///
/// **不接收路径**：`kind` 只认 `license` / `notice` 两个值，实际文件由宿主按
/// [`CompliancePaths`] 自己解析——设置页传不进任何路径，这条命令就不构成「任意文件打开器」。
///
/// 用 `notepad.exe` 而不是 explorer：这两份文件**没有扩展名**，explorer 会弹
/// 「如何打开此文件」让用户自己选；notepad 是确定行为（它们本来就是纯文本）。
#[tauri::command]
pub fn open_compliance<R: Runtime>(app: AppHandle<R>, kind: String) -> Result<(), String> {
    let resolved = CompliancePaths::resolve(&app);
    let path = match kind.as_str() {
        "license" => resolved.license,
        "notice" => resolved.notice,
        other => return Err(format!("未知的合规件：{other}")),
    };
    if !std::path::Path::new(&path).is_file() {
        return Err(format!("合规件不存在：{path}"));
    }
    std::process::Command::new("notepad.exe")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("打开失败：{e}"))?;
    flog!("[fnos] 已打开合规件：{path}");
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
    flog!("[fnos] open_url -> 系统默认浏览器: {url}");
    Ok(())
}

#[tauri::command]
pub fn reset_config<R: Runtime>(app: AppHandle<R>, scope: String) -> Result<ConfigView, String> {
    let prev = current(&app);
    let mut cfg = prev.clone();
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
    // 与 `set_config` **同一个**判据（`payload_changed`）：reset 同样可能把
    // `injectEnabled` / `homeUrl` / 完美图标 / 登录壁纸拉回默认值，全部必须重建主窗口才算生效。
    // 此前这里只比前两项，于是「恢复默认」后活窗口仍带着已关闭的 1 MiB 图标与旧壁纸。
    if payload_changed(&prev, &cfg) {
        if let Err(e) = recreate_main_window(&app, None) {
            flog!("[fnos] 重置后重建主窗口失败: {e}");
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
    let max_b64 = config::MAX_WALLPAPER_BYTES.div_ceil(3) * 4;
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
    flog!("[fnos] 登录壁纸已导入：{stored}（{bytes} 字节）");
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
            flog!(
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
        .get_webview(MAIN_WINDOW)
        .ok_or_else(|| "主窗口未打开".to_string())?;
    win.eval("window.__FNOS_REQUEST_APP_ITEMS__ && window.__FNOS_REQUEST_APP_ITEMS__();")
        .map_err(|e| format!("请求页面汇报应用项失败：{e}"))?;
    // 运行期证据（与建窗加载日志同风格）：证明设置窗的请求真的**下发到了页面**。
    // 「页面有没有真的送出去」不在这里断言（宿主观测不到），见上面的注释。
    flog!(
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
    // T14g（用户要求：参考图重做设置页）：设置页是本壳自己的深色界面（左侧分页导航 +
    // 右侧设置卡片），不再是被上游 popup 写死的 372×522，所以窗口按分页布局定尺寸。
    .inner_size(980.0, 680.0)
    .min_inner_size(820.0, 560.0)
    .resizable(true)
    // 深色页面配深色原生标题栏（Windows 10 1809+ / 11 生效；不支持的版本回落到系统默认）。
    .theme(Some(tauri::Theme::Dark))
    .center()
    .build()
    {
        Ok(_) => {}
        Err(e) => flog!("[fnos] 设置窗创建失败: {e}"),
    }
}

// ---------- 设置页 → 更新（T14g） ----------

/// 读更新状态（设置页轮询；只读，无副作用）。
#[tauri::command]
pub fn get_update_state<R: Runtime>(app: AppHandle<R>) -> crate::updater::UpdateStatus {
    crate::updater::status(&app)
}

/// 开始一次检查更新（有重入闸门：已在跑时直接返回当前状态，不并发下载）。
///
/// HTTP 是阻塞调用，必须丢进阻塞线程池——命令本身立即返回，设置页靠轮询看进度。
#[tauri::command]
pub fn start_update_check<R: Runtime>(app: AppHandle<R>) -> crate::updater::UpdateStatus {
    let before = crate::updater::status(&app);
    if !before.is_running() {
        let app_for_check = app.clone();
        tauri::async_runtime::spawn_blocking(move || crate::updater::run_check(app_for_check));
    }
    crate::updater::status(&app)
}

/// 安装已下载好的更新包并重启（阻塞调用同样进线程池）。
///
/// 只接受状态里由宿主自己写入、且落在配置目录 `updates/` 下的安装包（见
/// `updater::installer_path_is_trusted`）——设置页传不进来任何路径。
#[tauri::command]
pub fn install_update<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    if crate::updater::status(&app).phase != crate::updater::phase::READY {
        return Err("还没有可安装的更新包，请先检查更新".to_string());
    }
    let app_for_install = app.clone();
    tauri::async_runtime::spawn_blocking(move || crate::updater::install_ready(app_for_install));
    Ok(())
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
    report: config::LoadReport,
) {
    let path = Config::config_path();
    if report.read_failed {
        // 读失败 ≠ 新装：本次运行用默认配置，但绝不把默认值写回磁盘——
        // 原文件可能只是被临时占用/拒绝访问，覆盖它就是不可逆的数据丢失。
        flog!(
            "[fnos] 配置读取失败：本次启动使用默认配置且不写盘（原文件保留，路径 {}）",
            path.display()
        );
    } else {
        let _ = cfg.save(&path);
    }
    app.manage(AppState {
        config: Mutex::new(cfg.clone()),
        recreating: AtomicBool::new(false),
        recreate_url: Mutex::new(None),
        recreate_display: Mutex::new(DisplayState::default()),
        load: Mutex::new(LoadState::default()),
        recreate_error: Mutex::new(None),
        recovered_from_backup: AtomicBool::new(report.recovered_from_backup),
        page_report: Mutex::new(None),
        app_items_report: Mutex::new(None),
        chunk_report: Mutex::new(None),
        tabs: Mutex::new(TabState::with_main()),
        next_tab: AtomicU64::new(1),
        next_popup: AtomicU64::new(1),
        popup_recent: Mutex::new(Vec::new()),
        open_tabs: Mutex::new(std::collections::HashMap::new()),
        init_script: Mutex::new(None),
        last_key_at: AtomicU64::new(0),
        last_maximized: AtomicU8::new(0),
        last_title_log: AtomicU64::new(0),
        update: Mutex::new(crate::updater::UpdateStatus::default()),
    });
}

pub fn save_window_geom<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_window(MAIN_WINDOW) else {
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
    // 最大化时同样**不能**保存几何（T14d 实测）：`inner_size` 报的是最大化后的尺寸、
    // `outer_position` 报 (0,0)——存下去下次启动就是一个近似全屏的「普通」窗口
    //（实测：最大化退出 → config 被写成 1936×1018）。跳过保存即保留上一次的普通态几何。
    if win.is_maximized().unwrap_or(false) {
        return;
    }
    // 全屏同理：inner_size / outer_position 是全屏尺寸，存下去下次启动会变成一个
    // 近似全屏的普通窗口。退出全屏后 Tauri 会恢复原几何，不需要这里记。
    if win.is_fullscreen().unwrap_or(false) {
        return;
    }
    let Ok(size) = win.inner_size() else {
        return;
    };
    if size.width == 0 || size.height == 0 {
        return;
    }
    let scale = win.scale_factor().unwrap_or(1.0);
    // 锁内 read-modify-write-save：并发的 set_config/reset_config 不能被这份几何克隆
    // 覆盖（Config::save 的 SAVE_LOCK 只防字节交错，不防丢失更新）。
    let state = app.state::<AppState>();
    let mut cfg = state.config.lock().unwrap();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P0-3：标签页快捷键的邻居计算必须环绕，且单标签/异常态不做事。
    #[test]
    fn neighbor_tab_wraps_and_is_inert_with_a_single_tab() {
        let order: Vec<String> = ["main", "tab-1", "tab-2"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(neighbor_tab(&order, "main", true).as_deref(), Some("tab-1"));
        assert_eq!(neighbor_tab(&order, "tab-2", true).as_deref(), Some("main"), "末尾下一个环绕回第一个");
        assert_eq!(neighbor_tab(&order, "main", false).as_deref(), Some("tab-2"), "第一个上一个环绕到末尾");
        assert_eq!(neighbor_tab(&order, "tab-1", false).as_deref(), Some("main"));
        // 只有一个标签：Ctrl+Tab 无事可做（返回 None 而不是切到自己）
        let single: Vec<String> = vec!["main".to_string()];
        assert_eq!(neighbor_tab(&single, "main", true), None);
        // 当前标签不在表里（竞态：刚被关掉）：不做任何事
        assert_eq!(neighbor_tab(&order, "tab-9", true), None);
        assert_eq!(neighbor_tab(&[], "main", true), None);
    }

    /// P0-3：快捷键通道的安全面——前缀常量与 `inject/keys.js` 逐字一致，白名单之外一律不执行。
    #[test]
    fn tab_key_channel_is_whitelisted() {
        assert_eq!(KEY_TITLE_PREFIX, "FNOSKEY:");
        let js = include_str!("../inject/keys.js");
        assert!(
            js.contains(&format!("var KEY_TITLE_PREFIX = '{KEY_TITLE_PREFIX}';")),
            "页面侧前缀必须与宿主常量逐字一致"
        );
        let src = include_str!("commands.rs");
        let start = src.find("fn handle_tab_key").expect("处理器必须存在");
        let end = src[start..].find("/// 循环取邻居标签").map(|i| start + i).unwrap_or(src.len());
        let body = &src[start..end];
        assert!(
            body.contains(r#"matches!(action, "next" | "prev" | "close" | "new")"#),
            "动作必须在白名单里"
        );
        assert!(
            body.contains("action == \"exit-fullscreen\"") && body.contains("exit_fullscreen_if_needed"),
            "Esc 退出全屏必须走独立分支（不占标签快捷键冷却）"
        );
        assert!(
            body.contains("KEY_COOLDOWN_MS") && body.contains("compare_exchange"),
            "必须有刷屏冷却（页面可控通道）"
        );
        // 不得出现「提升权限」式的动作：这条通道只能切/关/开标签与退出全屏
        for forbidden in ["set_config", "reset_config", "import_wallpaper", "open_url"] {
            assert!(!body.contains(forbidden), "快捷键通道不得触达 {forbidden}");
        }
    }

    /// P1-8：`payload_changed` 是 `set_config` 与 `reset_config` 共用的**唯一**重建判据。
    /// 四个载荷键任一变化都必须判「需要重建」；其余（mods 字段、dockAutoHide、
    /// keepAliveMinutes、window 几何）都不能触发重建。
    #[test]
    fn payload_changed_covers_exactly_the_four_reload_keys() {
        let base = Config::default();
        assert!(!payload_changed(&base, &base.clone()), "没变就不该重建");

        // ① injectEnabled
        let mut c = base.clone();
        c.shell.inject_enabled = !base.shell.inject_enabled;
        assert!(payload_changed(&base, &c), "注入开关变了必须重建");

        // ② homeUrl
        let mut c = base.clone();
        c.shell.home_url = "https://nas.example.com/".into();
        assert!(payload_changed(&base, &c), "启动地址变了必须重建");

        // ③ 完美图标开关（决定 1 MiB 图标进不进载荷）
        let mut c = base.clone();
        c.mods.launchpad_icon_scale_enabled = true;
        assert!(payload_changed(&base, &c), "完美图标开关变了必须重建");

        // ④ 登录壁纸文件名（决定 binaryAssets 里嵌哪张图）
        let mut c = base.clone();
        c.local.login_wallpaper_file_name = Some("wall-abc123.png".into());
        assert!(payload_changed(&base, &c), "登录壁纸变了必须重建");

        // 反例：走免刷新通道的键都**不得**进判据（进了就是白重建一次窗口）
        let mut c = base.clone();
        c.shell.dock_auto_hide = !base.shell.dock_auto_hide;
        c.shell.keep_alive_minutes = 33;
        c.mods.base_preset_enabled = !base.mods.base_preset_enabled;
        c.shell.window.w = base.shell.window.w + 10.0;
        assert!(
            !payload_changed(&base, &c),
            "dockAutoHide / keepAliveMinutes / mods.* / 窗口几何走免刷新或与载荷无关，不得触发重建"
        );
    }

    /// P1-7：新标签页的初始标题取主机名第一段，绝不回落到内部 label（`tab-3`）。
    #[test]
    fn tab_initial_title_prefers_the_host_label() {
        let u = tauri::Url::parse("https://hermes-studio.nas-abc.fnos.net/apps/x?a=1").unwrap();
        assert_eq!(tab_initial_title(&u), "hermes-studio");
        // IP 字面量整段保留（按点切会把 192.168.1.10 变成无意义的「192」）
        let u = tauri::Url::parse("http://192.168.1.10:5666/").unwrap();
        assert_eq!(tab_initial_title(&u), "192.168.1.10");
        let u = tauri::Url::parse("http://[fd00::1]:5666/").unwrap();
        assert_eq!(tab_initial_title(&u), "fd00::1");
        // 单段主机名
        let u = tauri::Url::parse("http://nas:5666/").unwrap();
        assert_eq!(tab_initial_title(&u), "nas");
        // 弹窗起始页没有主机名：给一句固定文案，而不是空串或 tab-N
        let u = tauri::Url::parse("about:blank").unwrap();
        assert_eq!(tab_initial_title(&u), "新标签页");
        // 其它无主机名的 scheme 退化成 scheme 本身，仍然可读
        let u = tauri::Url::parse("data:text/html,x").unwrap();
        assert_eq!(tab_initial_title(&u), "data");
    }

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
        // 域名族的另外三个（同一套 FN Connect 入口的不同域名，上游按同一张表自动注入）
        assert!(is_fnos_net_origin("https://abc.5ddd.com"));
        assert!(is_fnos_net_origin("https://abc.fnnas.cn"));
        assert!(is_fnos_net_origin("https://abc.fynas.net"));
    }

    /// T14f 修复轮 1（用户实测「换了台 Win10 电脑装完注入不生效，还是原来的样式」）。
    ///
    /// 复现路径就是这条测试：全新安装的配置 `nasUrl` 为空、`homeUrl` 是官网门户，
    /// 用户经 FN Connect 进到自己的桌面（`https://<设备 id>.fnos.net/`）。放行判据里
    /// 少了域名族，那一篇文档以及之后每一篇文档都会被闸门整份挡掉。
    ///
    /// 三个来源一起锁：**域名族**（无法预先配置）、**配置过的 origin**（homeUrl / nasUrl）、
    /// **上游白名单**（`mods.enabledOrigins`——`is_recognized` 认它，闸门也必须认，
    /// 否则「加了白名单却什么都没注入」）。
    #[test]
    fn gate_allows_the_fnos_domain_family_and_every_configured_origin() {
        let url = |s: &str| s.parse::<tauri::Url>().expect("测试 URL 合法");
        let fresh = crate::config::Config::default();
        assert!(fresh.shell.nas_url.is_empty(), "全新安装的 nasUrl 必须为空");
        let origins = shell_origins(&fresh);

        // ① 域名族：FN Connect 的设备域名（含设备下的应用子域）
        assert!(origin_allowed_by_gate(&origins, &url("https://ea121314.fnos.net/")));
        assert!(origin_allowed_by_gate(
            &origins,
            &url("https://wechat-on-cloud-main.ea121314.fnos.net/")
        ));
        assert!(origin_allowed_by_gate(&origins, &url("https://abc.5ddd.com/")));
        assert!(origin_allowed_by_gate(&origins, &url("https://abc.fnnas.cn/")));
        assert!(origin_allowed_by_gate(&origins, &url("https://abc.fynas.net/")));
        // ② 默认 homeUrl（官网门户）仍在清单里：主窗口第一站就是它
        assert!(origin_allowed_by_gate(&origins, &url("https://fnos.net/")));
        // 第三方一律不放行（闸门要挡的就是它们）
        for bad in [
            "https://platform.deepseek.com/",
            "https://github.com/login",
            "https://evil-fnos.net/",
            "https://fnos.net.evil.com/",
            "http://127.0.0.1:8793/",
        ] {
            assert!(!origin_allowed_by_gate(&origins, &url(bad)), "不得放行：{bad}");
        }

        // ③ 配置过的 origin：nasUrl 指向的局域网 / 自建地址（本机既有用户的形态）
        let mut with_nas = fresh.clone();
        with_nas.shell.nas_url = "http://192.168.1.10:5666/webui/".into();
        let origins = shell_origins(&with_nas);
        assert!(origin_allowed_by_gate(
            &origins,
            &url("http://192.168.1.10:5666/desktop")
        ));
        assert!(!origin_allowed_by_gate(
            &origins,
            &url("http://192.168.1.10:5667/desktop")
        ));

        // ④ 上游白名单（用户手写的「这个站点要注入」）
        let mut whitelisted = fresh.clone();
        whitelisted.mods.enabled_origins = vec!["http://nas.local:8000".into()];
        let origins = shell_origins(&whitelisted);
        assert!(origin_allowed_by_gate(&origins, &url("http://nas.local:8000/x")));

        // 宿主侧「可信」判据走的是同一个函数：状态条、控制标题、配置推送与闸门同源
        assert!(is_trusted_shell_url(&fresh, &url("https://ea121314.fnos.net/")));
        assert!(!is_trusted_shell_url(&fresh, &url("https://github.com/login")));
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
        assert!(!is_recognized(&raw, Some("http://192.168.1.10:5667")));
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

    /// R30 的 JS 镜像必须与 Rust 规则同步（T14g 修复轮 3：镜像随设置页搬到 `page-items.js`）。
    ///
    /// 同一条 `prefect_icon/*.png` 规则写在两处：`ui/settings/page-items.js` 的
    /// `PREFECT_ICON_PATH_RE` 与 `config.rs::is_valid_prefect_icon_path`。前者决定设置页能
    /// 写进 `launchpadIconRedrawMap` 的路径，后者是 `Config::normalize` 落盘前的判定；
    /// 任何一侧落后都是**静默丢配置**（R30 修的就是 JS 那侧少了大小写不敏感）。
    ///
    /// 行为表在 JS 侧（`tests/page-items.test.mjs` 的 ok/bad 两组），这里锁的是「镜像仍然
    /// 存在且形状与 Rust 相同」——逐字锁正则文本（必须带 `i`）。
    #[test]
    fn prefect_icon_rule_mirror_stays_in_step() {
        let js = include_str!("../../ui/settings/page-items.js");
        assert!(
            js.contains("const PREFECT_ICON_PATH_RE = /^prefect_icon\\/[a-z0-9-]+\\.png$/i;"),
            "JS 镜像的正则必须与 Rust 规则同形且带 `i`（大小写不敏感，R30）"
        );
        assert!(
            js.contains("export function isPrefectIconPath"),
            "镜像必须是对外可用的判据（设置页写入前会走它）"
        );
        // 行为表（大小写、路径穿越、非 ASCII）在 JS 测试里逐行驱动同一个函数；
        // Rust 侧的那张表在 `config.rs::tests::redraw_map_regex_is_case_insensitive_only`。
        let table = include_str!("../../tests/page-items.test.mjs");
        assert!(
            table.contains("isPrefectIconPath(ok)") && table.contains("isPrefectIconPath(bad)"),
            "tests/page-items.test.mjs 必须继续逐行驱动两份清单（否则镜像没人核对）"
        );
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
        let max_b64 = config::MAX_WALLPAPER_BYTES.div_ceil(3) * 4;
        let huge = "A".repeat(max_b64 + 1);
        let err = store_wallpaper(&dir, "a.png", &huge).expect_err("超限必须被拒");
        assert!(err.contains("8 MiB"), "{err}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// titlebar 的四个命令（三个标签页 + 全屏）也必须在 main/build/capability 四处齐全。
    #[test]
    fn titlebar_commands_are_registered_in_main_build_and_capability() {
        let src = include_str!("commands.rs");
        let main_rs = include_str!("main.rs");
        let build = include_str!("../build.rs");
        let cap = include_str!("../capabilities/titlebar.json");
        for (cmd, perm, is_async) in [
            ("tab_new", "allow-tab-new", true),
            ("tab_switch", "allow-tab-switch", true),
            ("tab_close", "allow-tab-close", true),
            ("set_fullscreen", "allow-set-fullscreen", false),
        ] {
            let sig = if is_async {
                format!("pub async fn {cmd}<R: Runtime>")
            } else {
                format!("pub fn {cmd}<R: Runtime>")
            };
            assert!(src.contains(&sig), "commands.rs 缺少 {cmd}");
            assert!(
                main_rs.contains(&format!("commands::{cmd}")),
                "main.rs invoke_handler 缺少 {cmd}"
            );
            assert!(
                build.contains(&format!("\"{cmd}\"")),
                "build.rs AppManifest 缺少 {cmd}"
            );
            assert!(
                cap.contains(&format!("\"{perm}\"")),
                "titlebar.json 缺少 {perm}"
            );
        }
    }

    /// Task 13b 的注册不变式：每个 IPC 命令都必须在**四**个地方出现，且授权面没有扩大。
    ///
    /// 这四条正好是「漏一个」的四种表现：漏 `build.rs` = **构建失败**；漏 `main.rs` = 命令不
    /// 存在；漏 `default.json` = 设置窗调用被 ACL 拒绝（运行期静默失败）；漏 `remote-deny.json`
    /// = 少一层纵深防御。Task 13a 的静态检查脚本（`_t13a-capcheck.ps1`）做的是同一件事，
    /// 这里把它固化成一个每次 `cargo test` 都会跑的机械锁。
    #[test]
    fn every_ipc_command_is_registered_in_all_four_places() {
        const CMDS: [&str; 16] = [
            "get_config",
            "set_config",
            "reload_main",
            "open_config_dir",
            "open_compliance",
            "open_url",
            "reset_config",
            "get_page_state",
            "get_page_report",
            "import_wallpaper",
            "get_local_store",
            "set_local_store",
            "request_app_items",
            // T14g：设置页 → 更新（检查 / 轮询进度 / 安装）
            "get_update_state",
            "start_update_check",
            "install_update",
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
            remote_deny.contains("\"windows\": [\"main\", \"tab-*\", \"popup-*\"]"),
            "远程拒绝能力必须覆盖主标签、应用标签与原生弹窗"
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
        // （修复轮 22 起钩子用 `askForAppItemsNow(true)` 绕过「已配置」门——设置窗明确
        // 来要时必须照发，否则用户永远配不了完美图标；「先结算后计数」的次序不变。）
        let ask = shim
            .find("if (!askForAppItemsNow(true)) return false;")
            .expect("钩子必须先判定请求有没有真的送出（Minor 5：额度不得被空转消耗）");
        let count = shim
            .find("appItemsManualCalls += 1;")
            .expect("钩子必须仍然有次数上限");
        assert!(
            ask < count,
            "计数的位置必须在「真的送出去了」判定之后（先加一再看结果 = 旧缺陷）"
        );
        // 修复轮 22：设置窗开着（宿主来要）时，「完美图标还没配置」不能成为拒绝上报的
        // 理由——列表恰恰是配置完美图标的**前提**。只有页面自己的自动重试循环才受该门约束。
        let gate = shim
            .find("function askForAppItemsNow(force)")
            .expect("askForAppItemsNow 必须带 force 参数");
        let gate_body = &shim[gate..gate + 300];
        assert!(
            gate_body.contains("if (!force && !perfectIconConfigured()) return false;"),
            "「已配置」门只在自动路径生效（force = 设置窗明确来要）"
        );
        let hook = shim.find("W.__FNOS_REQUEST_APP_ITEMS__ = function").expect("钩子存在");
        let hook_body = &shim[hook..hook + 320];
        assert!(
            hook_body.contains("askForAppItemsNow(true)"),
            "手动钩子必须 force 上报（死锁修复）"
        );
    }

    /// T14c：`dockAutoHide` **不进** `needs_reload` 判据（免刷新生效）。
    ///
    /// 判据本身要 `State`/`AppHandle` 才能跑（`set_config` 是命令，本 crate 的单测不带
    /// tauri harness），所以按上面几条的同族手法做**源码级**锁：判据函数
    /// [`payload_changed`] 的函数体里不得出现 `dock_auto_hide`。它的生效通道是
    /// `apply_to_page` 的 `shell.dockAutoHide` → shim 转调 `__FNOS_APPLY_SHELL__`
    /// （`inject/dock.js`）——把它加进判据只会让每次切换白重建一次主窗口。
    #[test]
    fn dock_auto_hide_toggles_without_window_rebuild() {
        let src = include_str!("commands.rs");
        // 锚点必须是**定义**而不是任意引用：`find("…")` 会命中这条测试自己的断言字符串
        // （它就在同一份源码里），于是区间变成「测试内部的一小段」，断言永远为真。
        // 用函数定义的头 + 到下一个顶层 `}` 的区间来锁真正的判据。
        let start = src
            .find("fn payload_changed(prev: &Config, next: &Config) -> bool {")
            .expect("判据函数 payload_changed 必须存在");
        // 函数体的结束：从 `fn payload_changed` 往后找第一个「独占一行的 `}`」。
        // 必须同时兼容 LF / CRLF（源码在 Windows 上是 CRLF，而 `include_str!` 保留原样）。
        let end = src[start..]
            .find("\n}\n")
            .or_else(|| src[start..].find("\r\n}\r\n"))
            .map(|i| start + i)
            .expect("判据函数的函数体必须以独占一行的 } 结束");
        let predicate = &src[start..end];
        assert!(
            predicate.contains("next.shell.inject_enabled"),
            "取到的区间必须真的是判据函数体（锚点失效会让这条断言变成空转）：{predicate}"
        );
        assert!(
            !predicate.contains("dock_auto_hide"),
            "dockAutoHide 必须走免刷新通道，不得进 payload_changed 判据：{predicate}"
        );
        // 判据必须被两条入口**共用**（这正是 P1-8 的修复点：reset 曾经少比两项）
        let set_cfg = src.find("pub fn set_config<R: Runtime>").expect("set_config 必须存在");
        assert!(
            src[set_cfg..].contains("payload_changed(&prev, &next)"),
            "set_config 必须调用共用的 payload_changed"
        );
        let reset = src.find("pub fn reset_config<R: Runtime>").expect("reset_config 必须存在");
        assert!(
            src[reset..].contains("payload_changed(&prev, &cfg)"),
            "reset_config 必须调用**同一个** payload_changed（否则「恢复默认」不重建窗口）"
        );
        // 免刷新通道必须带着这个键：apply_to_page 的 shell 段缺了它，开关就只能对下一个
        // 新窗口生效（对活窗口永远不动手——那不是「即时生效」而是「悄悄不生效」）。
        let apply = src
            .find("fn apply_to_page")
            .expect("apply_to_page 必须存在");
        let apply_body = &src[apply..];
        assert!(
            apply_body.contains("\"dockAutoHide\": cfg.shell.dock_auto_hide"),
            "apply_to_page 的 shell 段必须带上 dockAutoHide（活窗口的免刷新切换通道）"
        );
        assert!(
            !apply_body.contains("\"homeUrl\"") && !apply_body.contains("\"nasUrl\""),
            "免刷新通道同样不得把宿主私有的 homeUrl/nasUrl 发给页面"
        );
    }

    /// T14e（用户实测「中继转发会一直新开标签」）：复用闸门——同一页面绝不重复开标签。
    /// `page_key` 剥掉 query/fragment（中继流程的重复 open 带变化的时间戳参数）。
    #[test]
    fn window_open_reuses_existing_tab_for_same_page() {
        let a = tauri::Url::parse("https://ea121314.fnos.net/relay?ts=111").unwrap();
        let b = tauri::Url::parse("https://ea121314.fnos.net/relay?ts=222&x=3").unwrap();
        assert_eq!(page_key(&a), page_key(&b), "query 不同的同一页面必须同键（中继循环去重靠它）");
        let other = tauri::Url::parse("https://ea121314.fnos.net/desktop").unwrap();
        assert_ne!(page_key(&a), page_key(&other), "不同 path 是不同页面，不得合并");
        let frag = tauri::Url::parse("https://ea121314.fnos.net/relay#s2").unwrap();
        assert_eq!(page_key(&a), page_key(&frag), "fragment 不参与比对");

        let src = include_str!("commands.rs");
        let code_end = src.find("#[cfg(test)]").expect("测试模块存在");
        let code = &src[..code_end];
        // 复用闸门三处：登记表命中并验活、main 当前页复用、命中即切不新建
        assert!(
            code.contains("map.get(&key).cloned()"),
            "必须先查复用登记表"
        );
        assert!(
            code.contains("still_same_page"),
            "命中必须复验标签**当前页面**（P1-7：只看 webview 存活会切到已导航走的旧标签）"
        );
        assert!(
            code.contains("复用切换（不新开）") && code.contains("与桌面标签相同，切换复用"),
            "命中路径必须是切换复用并留日志"
        );
        // 登记与清理
        assert!(
            code.contains("state\n            .open_tabs\n            .lock()\n            .unwrap()\n            .insert(page_key(&url), label.clone());")
                || code.contains(".insert(page_key(&url), label.clone());"),
            "开标签成功后必须登记复用键"
        );
        assert!(
            code.contains(".retain(|_, l| l != &label);"),
            "关标签必须清登记表"
        );
        // P1-7：窗口重建（正常窗 + 错误页）都必须把登记表清空——旧 tab-N label 已不存在
        assert_eq!(
            code.matches("open_tabs.lock().unwrap().clear();").count(),
            2,
            "build_main_window_with 与 build_error_window 都要清空复用登记表"
        );
    }

    /// T14e（用户实测「无法进入桌面」）：启动地址的优先级 = 覆盖 → nasUrl（直达桌面）
    /// → homeUrl → 默认常量；nasUrl 为空/非法时回落，不 panic。
    #[test]
    fn resolve_main_url_prefers_nas_url_when_configured() {
        let mk = |nas: &str, home: &str| {
            let mut c = crate::config::Config::default();
            c.shell.nas_url = nas.into();
            c.shell.home_url = home.into();
            c
        };
        // 配了 nasUrl → 启动直达 NAS 桌面（不经过 fnos.net 门户）
        let cfg = mk("https://ea121314.fnos.net/", "https://fnos.net/");
        assert_eq!(
            resolve_main_url(&cfg, None).as_str(),
            "https://ea121314.fnos.net/"
        );
        // 没配 → homeUrl 照旧（向后兼容：现有用户行为不变）
        let cfg = mk("", "https://fnos.net/");
        assert_eq!(resolve_main_url(&cfg, None).as_str(), "https://fnos.net/");
        // 非法 nasUrl → 回落 homeUrl（用户 typo 不会 panic）
        let cfg = mk("not a url", "https://fnos.net/");
        assert_eq!(resolve_main_url(&cfg, None).as_str(), "https://fnos.net/");
        // url 覆盖永远优先（托盘重新加载等一次性覆盖）
        let cfg = mk("https://ea121314.fnos.net/", "https://fnos.net/");
        assert_eq!(
            resolve_main_url(&cfg, Some("http://other.local:5000/")).as_str(),
            "http://other.local:5000/"
        );
    }

    /// T14c 修复轮 20（用户实测：启动时窗口「从左上侧突然跳到居中」）：位置必须在
    /// **建窗时**就交给 builder。首启路径「建窗即可见」，build 之后才补套位置
    /// （`tray::apply_window_geom`）必然让窗口先在 OS 默认的左上角落脚、再跳到保存的位置。
    #[test]
    fn main_window_is_born_at_its_saved_position() {
        let src = include_str!("commands.rs");
        // 只看测试模块**之前**的代码区（include_str 会把本测试自己的断言字符串也算进去）
        let code_end = src.find("#[cfg(test)]").expect("测试模块存在");
        let code = &src[..code_end];
        // 主窗口 + 错误页两条建窗路径都要在建窗前带上位置（有保存位置 → position）
        assert_eq!(
            code.matches("builder = builder.position(x, y);").count(),
            2,
            "两条建窗路径都必须在建窗时带上保存的位置"
        );
        assert_eq!(
            code.matches("_ => builder = builder.center(),").count(),
            2,
            "首次运行（还没有保存位置）直接居中，同样不在默认位置落脚"
        );
        assert_eq!(
            code.matches("x > -10000.0 && y > -10000.0").count(),
            2,
            "哨兵值防御与 tray::apply_window_geom（tray.rs 里的第三处）一致：-32000 是最小化窗口的坐标"
        );
        // 位置块必须在 build_main_window_with 之内（函数序：主窗口在前、错误页在后）
        let main_at = src.find("fn build_main_window_with").expect("主窗口构建函数存在");
        let first_pos = src.find("builder = builder.position(x, y);").expect("位置块存在");
        assert!(
            main_at < first_pos && first_pos < src.find("fn build_error_window").expect("错误页构建函数存在"),
            "第一条位置块必须落在主窗口构建函数内"
        );
    }

    /// 受信 origin / 完整配置推送范围：主页面、桌面根与授权页；应用页和第三方不拿。
    #[test]
    fn only_shell_origins_get_full_payload_and_config_pushes() {
        let mut cfg = Config::default();
        cfg.shell.home_url = "https://fnos.net/".into();
        cfg.shell.nas_url = "https://nas.example.com/".into();
        let page = |u: &str| tauri::Url::parse(u).unwrap();
        let origins = shell_origins(&cfg);
        assert!(origins.contains(&"https://fnos.net".to_string()));
        assert!(origins.contains(&"https://nas.example.com".to_string()));
        assert!(is_trusted_shell_url(
            &cfg,
            &page("https://nas.example.com/seek")
        ));
        assert!(!is_trusted_shell_url(
            &cfg,
            &page("https://example.com/")
        ));
        assert!(should_receive_full_config(
            &cfg,
            MAIN_WINDOW,
            &page("https://fnos.net/")
        ));
        assert!(should_receive_full_config(
            &cfg,
            "tab-1",
            &page("https://nas.example.com/")
        ));
        assert!(should_receive_full_config(
            &cfg,
            "tab-1",
            &page("https://nas.example.com/signin?client_id=x")
        ));
        assert!(!should_receive_full_config(
            &cfg,
            "tab-1",
            &page("https://nas.example.com/seek/")
        ));
        assert!(!should_receive_full_config(
            &cfg,
            MAIN_WINDOW,
            &page("https://example.com/")
        ));
    }

    /// 应用标签不再拿桌面完整载荷；根页面与登录/授权页仍拿。
    #[test]
    fn app_tabs_get_the_core_script_not_the_desktop_payload() {
        let desktop = tauri::Url::parse("https://nas.example.com/").unwrap();
        let page = |u: &str| tauri::Url::parse(u).unwrap();
        assert!(is_desktop_or_auth_page(
            &desktop,
            &page("https://nas.example.com/")
        ));
        assert!(!is_desktop_or_auth_page(
            &desktop,
            &page("https://nas.example.com/seek/?ver=0.0.1")
        ));
        assert!(!is_desktop_or_auth_page(
            &desktop,
            &page("https://nas.example.com/p")
        ));
        assert!(!is_desktop_or_auth_page(
            &desktop,
            &page("https://nas.example.com/music/")
        ));
        assert!(!is_desktop_or_auth_page(
            &desktop,
            &page("https://nas.example.com/app/Harness/dsh/")
        ));
        assert!(is_desktop_or_auth_page(
            &desktop,
            &page("https://nas.example.com/signin?client_id=x")
        ));
        assert!(is_desktop_or_auth_page(
            &desktop,
            &page("https://nas.example.com/seek/login")
        ));
        let nested = page("https://nas.example.com/fnos/");
        assert!(is_desktop_or_auth_page(
            &nested,
            &page("https://nas.example.com/fnos/")
        ));
        assert!(!is_desktop_or_auth_page(
            &nested,
            &page("https://nas.example.com/fnos/app/x")
        ));
    }

    /// 控制标题是页面 → 宿主的传输，不是页面标题；弹窗镜像标题前必须能识别它们。
    #[test]
    fn control_titles_cover_every_page_to_host_prefix() {
        assert!(is_control_title(&format!(
            "{PROBE_TITLE_PREFIX}{{\"err\":1}}"
        )));
        assert!(is_control_title(&format!(
            "{}{{\"type\":\"FNOS_CHECK\"}}",
            report::CHUNK_TITLE_PREFIX
        )));
        assert!(is_control_title(&format!(
            "{}{{\"type\":\"FNOS_CHECK\"}}",
            report::REPORT_TITLE_PREFIX
        )));
        assert!(is_control_title(&format!("{KEY_TITLE_PREFIX}close")));
        assert!(!is_control_title("DeepSeek 开放平台"));
        assert!(!is_control_title(""));
        assert!(!is_control_title("MiniNas - 飞牛 fnOS"));
    }

    /// T14c 修复轮 11 → T14d 升级 → T14e 弹窗修复：`window.open` / `target=_blank` 走
    /// **混合裁定**——普通应用启动（无窗口特征、非登录地址）开标签页；`about:blank`、
    /// 带尺寸/坐标、登录类地址走 `Create` 建原生弹窗（保留 opener / 返回值 /
    /// `window.close`，修「空白标签」与「登录后结果回不到应用」）。
    ///
    /// 这里锁：协议闸门、两条路径都存在且互斥、标签页仍走 Deny + 异步转交（`add_child`
    /// 在同步回调里会死锁）、三个标签页命令、命令清单同步、标签条授权只给 titlebar、
    /// fnOS 主页标签不可关。
    #[test]
    fn new_window_requests_become_tabs_or_popups() {
        let src = include_str!("commands.rs");
        let verdict = &src[src.find("fn new_window_verdict").expect("裁定函数必须存在")
            ..src.find("pub async fn open_app_tab").expect("标签页创建函数必须存在")];
        assert!(
            verdict.contains("\"http\" | \"https\" | \"about\""),
            "只允许 http(s)/about 开新窗口（file/data/javascript 一律拒绝）"
        );
        assert!(
            verdict.contains("NewWindowResponse::Deny"),
            "标签页路径必须显式 Deny 原生新窗口（标签页由异步任务创建）"
        );
        assert!(
            verdict.contains("NewWindowResponse::Create") && verdict.contains("create_popup_window"),
            "弹窗路径必须走 Create（about:blank / 登录弹窗要求真实 opener 语义）"
        );
        assert!(
            verdict.contains("wants_native_popup"),
            "必须由纯判定函数决定弹窗 / 标签页，不能在裁定里手写一堆 if"
        );
        assert!(
            verdict.contains("tauri::async_runtime::spawn") && verdict.contains("open_app_tab"),
            "标签页请求必须转交给异步任务创建"
        );
        // 弹窗创建：窗口特征（含 WebView2 环境共享）+ 标题跟随 + 同源注入
        let popup = &src[src.find("fn create_popup_window").expect("弹窗创建函数必须存在")
            ..src.find("/// 为一个 `window.open` 请求").expect("标签页创建函数的文档必须紧随其后")];
        assert!(
            popup.contains(".window_features(features)"),
            "弹窗必须带上 NewWindowFeatures（Windows 上靠它共享 WebView2 环境，否则 SetNewWindow 不成立）"
        );
        assert!(
            popup.contains("on_document_title_changed"),
            "弹窗标题必须跟随 document.title"
        );
        assert!(
            popup.contains("is_control_title"),
            "弹窗标题必须过滤控制前缀（FNOSREPORT/FNOSPROBE/FNOSCHUNK/FNOSKEY 不是页面标题）"
        );
        assert!(
            popup.contains("injector::wrap_origin_script"),
            "弹窗载荷必须带运行时 origin 闸门（about:blank / 第三方文档不得执行 mods/shim）"
        );
        assert!(
            popup.contains("initialization_script") && popup.contains("fnos_site"),
            "只有落在闸门放行集合的弹窗才注册初始化脚本（第三方登录页不该被 mods 覆盖）"
        );
        assert!(
            popup.contains("origin_allowed_by_gate"),
            "弹窗的注册判据与运行时闸门必须是同一份判据（全新安装的机器上主地址是官网门户，\
             桌面的 /signin 弹窗与它不同源，按「同源」注册会连载荷都不装）"
        );
        assert!(
            popup.contains("on_page_load")
                && popup.contains("PageLoadEvent::Finished")
                && popup.contains("guard_late_script")
                && popup.contains(".eval("),
            "首个文档可能漏掉初始化脚本：加载完成后必须补注入（且带幂等判据）"
        );
        assert!(
            src.matches(".on_new_window(").count() >= 1,
            "标签页 Webview 都要挂这条裁定（tab_webview_builder 统一组装）"
        );
        // open_app_tab：铺在标签栏以下、激活、复用 tab_webview_builder 与注入载荷
        let open_at = src.find("pub async fn open_app_tab").expect("开标签页函数必须存在");
        let end = src.find("/// 激活一个标签页").expect("激活函数必须紧随其后");
        let body = &src[open_at..end];
        assert!(
            body.contains("TAB_STRIP_H") && body.contains("LogicalPosition::new(0.0, TAB_STRIP_H)"),
            "标签页 Webview 必须铺在标签栏以下的全部区域"
        );
        assert!(
            body.contains("tab_webview_builder(&label, url.clone(), app, None)"),
            "标签页与主标签共用同一组回调（generation = None：不进加载观测管线）"
        );
        assert!(
            body.contains("init_script_for_tab(app, &cfg, &url)"),
            "标签页必须按 origin 选择注入档位（第三方标签不得拿完整 mods/dock/windowpos 载荷）"
        );
        // 分档函数本身：同源 → 缓存过的完整脚本；第三方 → 只含 keys 的核心脚本
        let tier_at = src.find("fn init_script_for_tab").expect("分档函数必须存在");
        let tier = &src[tier_at..tier_at + 600];
        assert!(tier.contains("is_desktop_or_auth_page"));
        assert!(tier.contains("same_origin(&desktop, url)"));
        assert!(tier.contains("init_script_cached(app, cfg)"));
        assert!(tier.contains("wrap_origin_script"));
        assert!(tier.contains("injector::build_core_script()"));
        assert!(
            body.contains("activate_tab(app, &label)"),
            "新标签页创建即激活"
        );
        assert!(
            body.contains("tab_initial_title(&url)"),
            "新标签页必须立刻有可读标题（取主机名），不能等 document.title 才从 tab-N 变过来"
        );
        assert!(
            verdict.contains("still_same_page"),
            "复用闸门必须复验标签**当前页面**（P1-7），不能只验 webview 还活着"
        );
        // 三个标签页命令 + 命令清单同步（build.rs 的 AppManifest::commands）
        for cmd in ["tab_new", "tab_switch", "tab_close"] {
            assert!(
                src.contains(&format!("pub async fn {cmd}<R: Runtime>")),
                "缺少标签页命令：{cmd}"
            );
            let build = include_str!("../build.rs");
            assert!(build.contains(&format!("\"{cmd}\"")), "build.rs 命令清单缺少：{cmd}");
        }
        // 全屏命令（同步命令，不是 async）+ 命令清单 + 页面/标题栏联动
        assert!(
            src.contains("pub fn set_fullscreen<R: Runtime>"),
            "缺少全屏命令：set_fullscreen"
        );
        assert!(
            include_str!("../build.rs").contains("\"set_fullscreen\""),
            "build.rs 命令清单缺少：set_fullscreen"
        );
        let fullscreen_at = src.find("pub fn set_fullscreen").expect("set_fullscreen 必须存在");
        let fullscreen_body = &src[fullscreen_at..fullscreen_at + 1600];
        assert!(
            fullscreen_body.contains("enter_fullscreen(&app, &window)")
                && fullscreen_body.contains("exit_fullscreen(&app, &window)"),
            "全屏命令必须走进入/退出全屏的完整流程"
        );
        assert!(
            src.contains("fn enter_fullscreen<R: Runtime>")
                && src.contains("fn exit_fullscreen<R: Runtime>")
                && src.contains("titlebar.hide()")
                && src.contains("start_fullscreen_titlebar_watcher"),
            "进入全屏必须隐藏标题栏并启动顶部悬停 watcher"
        );
        assert!(
            src.contains("bring_webview_to_front") && src.contains("ParentWindow"),
            "悬浮标题栏必须用 WebView2 子窗口 HWND 提到内容之上"
        );
        assert!(
            src.contains("__FNOS_DOCK_FULLSCREEN__") && src.contains("__FNOS_WIN_FULLSCREEN_SET__"),
            "全屏联动必须调用 dock.js 的隐藏口并回推标题栏状态"
        );
        assert!(
            src.contains("set_windows_taskbar_visible(!on)") && src.contains("pub fn restore_taskbar"),
            "全屏必须隐藏 Windows 任务栏，并提供退出/销毁时的还原口"
        );
        assert!(
            src.contains("restore_taskbar_on_startup")
                && src.contains("taskbar-hidden.flag")
                && src.contains("Shell_SecondaryTrayWnd"),
            "任务栏必须支持副屏，并能从强杀后的 marker 恢复"
        );
        // fnOS 主页标签不可关闭（关了就没有「回桌面」的地方）
        let close_at = src.find("pub async fn tab_close<R: Runtime>").expect("tab_close 必须存在");
        let close_body = &src[close_at..close_at + 900];
        assert!(
            close_body.contains("if label == MAIN_WINDOW"),
            "tab_close 必须拒绝关闭 fnOS 主页标签"
        );
        // 标签条授权只给 titlebar；各标签页承载的第三方页面没有任何授权。
        // 修复轮 3（用户要求「标签页合并到标题栏」）：右上 ─ □ × 在一行式栏内，
        // 窗口控制权限回到标签条（系统默认观感的自绘按钮）。
        let cap = include_str!("../capabilities/titlebar.json");
        assert!(cap.contains("\"titlebar\""), "能力必须只授 titlebar Webview");
        for perm in [
            "core:window:allow-start-dragging",
            "core:window:allow-minimize",
            "core:window:allow-toggle-maximize",
            "core:window:allow-close",
            "allow-tab-new",
            "allow-tab-switch",
            "allow-tab-close",
            "allow-set-fullscreen",
        ] {
            assert!(cap.contains(perm), "标签页栏缺少权限：{perm}");
        }
        assert!(
            !cap.contains("allow-get-config") && !cap.contains("allow-open-url"),
            "标签页栏不得得到任何应用命令授权"
        );
        // 标签条前端：拖拽区 + 标签页命令 + 窗口命令 + 状态推送口（跨语言锚点）
        let tb_js = include_str!("../../ui/settings/titlebar.js");
        for needle in [
            "data-tauri-drag-region",
            "tab_new",
            "tab_switch",
            "tab_close",
            "set_fullscreen",
            "__FNOS_TABS_SET__",
            "__FNOS_WIN_MAX_SET__",
            "__FNOS_WIN_FULLSCREEN_SET__",
            "plugin:window|minimize",
            "plugin:window|toggle_maximize",
            "plugin:window|close",
        ] {
            assert!(tb_js.contains(needle), "标签条脚本缺少：{needle}");
        }
        // 系统默认观感：不跟随上游 mod 的标题栏样式（红绿灯 / 样式开关一律没有）
        for gone in ["traffic", "__FNOS_TABS_BOOT__", "TITLEBAR_STYLE_MAC"] {
            assert!(!tb_js.contains(gone), "标签条脚本不得有样式分支：{gone}");
        }
        assert!(!tb_js.contains("innerHTML"), "标签条脚本不得用 innerHTML");
        // 宿主侧：最大化状态推送口必须存在，且建窗后与 resize 后各推一次
        // （按钮字形 □ ↔ 还原 与系统标题栏语义一致）
        let src = include_str!("commands.rs");
        let code_end = src.find("#[cfg(test)]").expect("测试模块存在");
        let code = &src[..code_end];
        assert!(
            code.contains("fn push_win_max") && code.contains("__FNOS_WIN_MAX_SET__"),
            "宿主必须有最大化状态推送口"
        );
        let push_count = code.matches("push_win_max(").count();
        assert!(
            push_count >= 2,
            "建窗后与 resize 后（relayout）都必须推最大化状态"
        );
        assert!(
            code.contains("last_maximized") && code.contains("swap(next"),
            "最大化状态必须去重，不能每次 resize 都 eval"
        );
        assert!(
            code.contains("is_maximized()"),
            "最大化状态必须来自 Window::is_maximized（不是猜的）"
        );
        let tb_html = include_str!("../../ui/settings/titlebar.html");
        assert!(
            tb_html.contains("data-tauri-drag-region"),
            "标签条 HTML 必须带拖拽区"
        );
        assert!(
            tb_html.contains("id=\"controls\""),
            "一行式标题栏必须带窗口控制区（用户要求合并）"
        );
        // 修复轮 7（用户报「标题栏下面的线条」）：窗口背景 = 标题栏色——
        // 两条建窗路径（主窗 + 错误页窗）都要设，webview 缝才不会露出白线
        assert_eq!(
            code.matches("background_color(tauri::window::Color").count(),
            2,
            "主窗口与错误页窗口都必须设标题栏同色背景"
        );
    }

    /// T14e 混合裁定：哪些新窗口请求必须开成原生弹窗（而不是标签页）。
    ///
    /// 左边两类是用户实测失败过的场景；右边一类是 T14d 的标签页主路径，必须保持不变。
    #[test]
    fn popup_verdict_keeps_app_launches_as_tabs() {
        let url = |s: &str| s.parse::<tauri::Url>().expect("测试 URL 合法");

        // ① about:blank：window.open('') 后由 opener 同步写内容 / 设地址
        assert!(
            wants_native_popup_parts(&url("about:blank"), false, false),
            "about:blank 必须开原生弹窗（标签页接不住 document.write / location 赋值）"
        );
        // ② 带窗口尺寸或坐标：典型登录弹窗 window.open(url, name, 'width=…')
        assert!(
            wants_native_popup_parts(&url("https://example.com/anywhere"), true, false),
            "带尺寸的 window.open 必须开原生弹窗"
        );
        assert!(
            wants_native_popup_parts(&url("https://example.com/anywhere"), false, true),
            "带坐标的 window.open 必须开原生弹窗"
        );
        // ③ 登录类地址（用户日志里出现过的真实入口）
        for s in [
            "https://dsh.r2049.cn/api/v1/auth/q/2_AuHQ1fQUWrPPXRd_tIPKHq",
            "https://ea121314.fnos.net/signin?client_id=Ov23liJ9BDdmuXo5RZMx",
            "https://github.com/login",
            "https://github.com/login/oauth/authorize?client_id=x&state=y",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://example.com/portal/Login.aspx?ReturnUrl=%2f",
        ] {
            assert!(
                wants_native_popup_parts(&url(s), false, false),
                "登录类地址必须开原生弹窗（opener 要把结果回传给应用）：{s}"
            );
        }
        // ④ 普通应用启动 / 外部链接：仍走标签页（T14d 的形态不变）
        for s in [
            "https://dsh.r2049.cn/",
            "https://hermes-studio.ea121314.fnos.net/",
            "https://github.com/dsh-market/dsh-market",
            "https://fnos-apps-store.ea121314.fnos.net/",
            "https://example.com/author",
            "https://example.com/login-guide",
        ] {
            assert!(
                !wants_native_popup_parts(&url(s), false, false),
                "普通页面必须继续开标签页（不能把应用启动改成独立窗口）：{s}"
            );
        }
    }

    /// 同源判据只比较 scheme/host/默认端口：fnOS 主页面开出的 `/signin` 弹窗要命中，
    /// 第三方登录页（GitHub / Google）不能命中。（它仍决定**标签页**的注入档位；
    /// 弹窗的注册判据见下一条——那里不能用「与主地址同源」。）
    #[test]
    fn popup_injection_is_limited_to_the_fnos_origin() {
        let url = |s: &str| s.parse::<tauri::Url>().expect("测试 URL 合法");
        let main = url("https://ea121314.fnos.net/");
        assert!(same_origin(&main, &url("https://ea121314.fnos.net/signin?client_id=x")));
        assert!(same_origin(&url("https://x.fnos.net/a"), &url("https://x.fnos.net/b?q=1")));
        assert!(!same_origin(&main, &url("https://github.com/login")));
        assert!(!same_origin(&main, &url("http://ea121314.fnos.net/")));
        assert!(!same_origin(&main, &url("https://ea121314.fnos.net:8443/")));
    }

    /// 弹窗的**注册**判据不能是「与主地址同源」：全新安装的机器上主地址是官网门户
    /// `https://fnos.net/`，而桌面的 `/signin` 授权弹窗开在 `https://<设备>.fnos.net/`，
    /// 不同源 ⇒ 连载荷都不注册，登录页的 mods CSS 修正整批失效（T14f 修复轮 1）。
    ///
    /// 换用闸门判据后：飞牛域名族与配置过的 origin 都注册；第三方一律不注册。
    #[test]
    fn popup_payload_registration_follows_the_gate() {
        let url = |s: &str| s.parse::<tauri::Url>().expect("测试 URL 合法");
        let fresh = Config::default(); // 全新安装：nasUrl 空、homeUrl = 官网门户
        let allowed = shell_origins(&fresh);
        // 弹窗目标：桌面的授权页（与主地址不同源，但域名族放行）
        assert!(origin_allowed_by_gate(
            &allowed,
            &url("https://ea121314.fnos.net/signin?client_id=smoke")
        ));
        // opener 当前地址同样算数（about:blank 起步、之后再导航的弹窗）
        assert!(origin_allowed_by_gate(
            &allowed,
            &url("https://ea121314.fnos.net/")
        ));
        // 第三方登录页：目标与 opener 都不在放行集合里
        for bad in [
            "https://github.com/login",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://platform.deepseek.com/",
        ] {
            assert!(!origin_allowed_by_gate(&allowed, &url(bad)), "{bad}");
        }
    }

    /// 弹窗限速：10 秒窗口内最多 5 个；窗口滑动后自动放行，旧记录被清理。
    #[test]
    fn popup_rate_limit_slides_and_caps() {
        let mut recent = Vec::new();
        for t in [1_000u64, 1_100, 1_200, 1_300, 1_400] {
            assert!(!popup_rate_exceeded(&mut recent, t), "窗口内前 5 次必须放行：{t}");
        }
        assert!(popup_rate_exceeded(&mut recent, 1_500), "第 6 次必须拒绝");
        assert_eq!(recent.len(), POPUP_RATE_MAX, "被拒绝的请求不入队");
        // 窗口滑过第一条记录后，放行一次；其余仍在窗口内的记录保留
        assert!(
            !popup_rate_exceeded(&mut recent, 1_000 + POPUP_RATE_WINDOW_MS + 1),
            "窗口滑动后必须恢复放行"
        );
        assert_eq!(recent.len(), POPUP_RATE_MAX);
    }

    /// 资源/频率闸门的源码锚点：标题未变不写日志不推标签条；标签页有上限；
    /// `FNOSKEY` 只认当前激活标签（后台页不能关用户正在看的标签）。
    #[test]
    fn title_and_resource_limits_are_guarded() {
        let src = include_str!("commands.rs");
        assert!(src.contains("if !changed {"), "标题未变必须直接返回（不写日志、不推标签条）");
        assert!(src.contains("MAX_OPEN_TABS"), "必须有标签页上限");
        assert!(src.contains("popup_rate_limited"), "必须有弹窗限速");
        let key_at = src
            .find("if let Some(action) = title.strip_prefix(KEY_TITLE_PREFIX)")
            .expect("FNOSKEY 分支必须存在");
        let key_branch = &src[key_at..src.len().min(key_at + 600)];
        assert!(
            key_branch.contains("tabs.lock().unwrap().active == webview.label()")
                && key_branch.contains("if is_active"),
            "FNOSKEY 必须只接受激活标签发来的命令"
        );
    }
}
