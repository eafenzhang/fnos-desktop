//! 页面 → 宿主上报通道（Task 13a）的**纯函数**部分：前缀、上限、允许表、校验。
//!
//! ## 为什么不是 IPC（R70 是硬约束）
//!
//! 本项目的安全不变式是「远程页面**一个命令都调不动**」——`capabilities/` 下没有任何
//! `remote` 块，两轮 ACL 探针的读数写在 `docs/acceptance/M1-M2-验收记录.md` 第 8 条。
//! Task 13 brief 的方案（给 `remote: { urls: ["*"] }` 开一个 `mods_page_report`，再由 Rust
//! `eval` 进**已获全量授权**的设置窗）会当场破坏它。因此复用 Task 11 已经建立、且不涉及
//! 任何授权的原生通路：shim 写 `document.title` 的第二个控制前缀，Rust 的
//! `on_document_title_changed`（`commands.rs::handle_title`）认前缀并校验后存内存。
//! 页面侧没有多出任何一个可调用命令。
//!
//! ## 威胁模型：标题通道是页面**可写**的
//!
//! 任何页面（含远程站点）都能自己写 `document.title`。因此本模块对输入一律按敌意处理：
//! 先卡字节上限，再要求「合法 JSON 对象」，再把 `type` 限制在一张小允许表里，**永不 panic**。
//! 校验通过也只是「某个文档声称了这件事」——它的用途仅是设置窗顶部那句话与调试日志，
//! 不产生任何权限、不触碰文件系统/网络/配置。详见 `docs/…/task-13a-report.md` 的 concerns。
//!
//! ## 日志注入（fix round 1 / Important 1）
//!
//! 上报体是页面可控的，而 stderr 日志是本项目的**评审证据**：一条能换行的页面可控文本可以
//! 伪造出整行 `[fnos] …`。因此本模块同时是「页面可控文本 → 日志」的唯一收口：
//! [`accepted_log_line`]（上报已接受那一行，`dir` 走 [`REPORT_DIRS`] 白名单）、[`log_safe`]
//! （其余需要原样留证但绝不能换行的文本，例如镜像到窗口标题的 `document.title`）。
//! 任何新增的日志点若会打印页面可控文本，都必须从这两个函数里挑一个，不要手写 `{value}`。

use serde_json::Value;

/// 上报控制前缀。
///
/// 与 [`crate::commands::PROBE_TITLE_PREFIX`] 同样是**可打印 ASCII**：WebView2 在把
/// `document.title` 送到宿主之前会吃掉控制字符（Task 11 实测），控制字符前缀会静默失配。
/// 也必须与页面侧 `shim.js` 里的字面量逐字一致（`commands.rs` 的测试锁定这一点）。
pub const REPORT_TITLE_PREFIX: &str = "FNOSREPORT:";

/// 上报体上限（**字节**，UTF-8）。标题通道对页面公开可写，必须假定对面是敌意输入：
/// 超限直接丢弃，不解析、不落日志正文。
///
/// **实测（Task 13a）：这道闸门在真实通道上永远够不到。** WebView2 把送给宿主的
/// `document.title` 截断到 [`TITLE_CHANNEL_MAX_BYTES`]，所以能到达的最大载荷是
/// `4096 - 11 = 4085` 字节（长度阶梯实测见下）。32KiB 因此是**纵深防御**：它对
/// 「将来某个 WebView2 版本放宽了标题长度」以及任何其他调用方仍然成立。
pub const MAX_REPORT_BYTES: usize = 32 * 1024;

/// 标题通道的**实测**上限（**含前缀**的总字节数）——Task 13a 的长度阶梯实测结论。
///
/// 证据：探针页把一条条**合法** JSON 上报写成不同的总标题长度，宿主逐条记录
/// 「接受/被拒 + 收到多少字节」：
///
/// | 页面写的标题总长 | 宿主收到 | 结果 |
/// |---|---|---|
/// | 200 / 1000 / 3000 / 4000 / 4085 / 4096 | 189 / 989 / 2989 / 3989 / 4074 / 4085 | 接受（JSON 完整） |
/// | 4097 / 5000 / 9000 / 40065 | 4085（截断） | 被拒（截断后不是合法 JSON） |
///
/// 也就是说：`document.title` 在到达 `on_document_title_changed` 之前被 WebView2/Chromium
/// 截到 4096 字节（页面侧 `document.title.length` 仍是 40065，见探针上报）。**4096 是这一版
/// WebView2（148.0.3967.54）的实测值，不是文档承诺**；T13b 的「完美图标应用项列表」必须
/// 控制在 ~4000 字节以内，否则在宿主侧表现为「不是合法 JSON」这种截断假象。
pub const TITLE_CHANNEL_MAX_BYTES: usize = 4096;

/// 「这个被拒的载荷是不是被标题长度截断的」——只用于日志提示，不改变任何判定。
///
/// 截断后的载荷一定**不完整**，因此宿主只能看到 [`Reject::NotJson`]；把长度对得上的情形
/// 标出来，免得将来有人去追一个并不存在的 JSON 语法 bug（T13b 的应用项列表正是高风险场景）。
pub fn truncation_suspected(payload_len: usize) -> bool {
    payload_len + REPORT_TITLE_PREFIX.len() >= TITLE_CHANNEL_MAX_BYTES
}

/// 允许转发的上游消息 `type`——逐字取自 vendored 的 `content-script.js`，不是凭空发明的：
///
/// - `FNOS_INJECTION_TRIGGERED`：`content-script.js:2682`，上游在 `startInject()` 末尾
///   自己 `chrome.runtime.sendMessage` 出去的消息（`{type, triggerReason, origin, href, timestamp}`，
///   `:2681-2687`）。这是**唯一一条**上游真正经 `sendMessage` 发出的消息，也是「注入链真的跑到了
///   最后一步」最硬的信号（触发点：`:2995` 的 `auto_whitelist` / `auto_suspected`、`:2845` 的
///   `popup_apply`）。
/// - `FNOS_APPLY` / `FNOS_GET_LAUNCHPAD_APP_ITEMS` / `FNOS_GET_LAUNCHPAD_APP_TITLES` / `FNOS_CHECK`：
///   上游 `chrome.runtime.onMessage` 真正处理的请求类型（`:2809` / `:2854` / `:2855` / `:2872`），
///   应答形状分别是 `{applied:true}`（`:2848`）与 `{items, titles}`（`:2865-2868`）。
///   本壳目前**没有**人从宿主侧发这些请求（配置推送走 `__FNOS_APPLY_CONFIG__`），
///   所以这几个 type 只在「页面自己/将来的桥」发起并得到应答时才会出现在上报里。
pub const REPORT_TYPES: [&str; 5] = [
    "FNOS_INJECTION_TRIGGERED",
    "FNOS_APPLY",
    "FNOS_GET_LAUNCHPAD_APP_ITEMS",
    "FNOS_GET_LAUNCHPAD_APP_TITLES",
    "FNOS_CHECK",
];

/// 「应用项列表」上行通道的 `type`（Task 13b 的逐项 UI 数据源）。
///
/// 单独抽成常量是因为宿主侧对它有一条**额外的分流规则**：它进的是
/// [`crate::commands::AppState::app_items_report`] 这个槽位，而不是状态条的证据槽位
/// （[`crate::commands::AppState::page_report`]）。理由见 [`is_app_items_report`]。
pub const APP_ITEMS_TYPE: &str = "FNOS_GET_LAUNCHPAD_APP_ITEMS";

/// 上游**同一个分支**里的另一个请求类型，应答形状与 [`APP_ITEMS_TYPE`] 逐字相同。
///
/// 上游对这两个 type 的处理完全一样（`content-script.js:2853-2868`：同一个 `if` 里先
/// `collectLaunchpadAppItems()`，再 `sendResponse({items, titles})`），所以从宿主的角度看
/// 它们不是两种东西——必须走同一条上行通道（详见 [`is_app_items_report`] 的分流论证）。
pub const APP_ITEMS_TITLES_TYPE: &str = "FNOS_GET_LAUNCHPAD_APP_TITLES";

/// 进「应用项列表」槽位的 `type` 集合（`ui/settings/app.js` 的 `APP_ITEM_REPORT_TYPES`
/// 必须与它集合相等，`tests/contract.test.mjs` 逐字锁着这件事）。
pub const APP_ITEMS_TYPES: [&str; 2] = [APP_ITEMS_TYPE, APP_ITEMS_TITLES_TYPE];

/// 这条**已通过校验**的上报是不是「应用项列表」通道（Task 13b）。
///
/// ## 为什么必须分流（T13b 审计发现）
///
/// T13a 的状态条强态只有一条判据：**最近一次**上报是 `FNOS_INJECTION_TRIGGERED` 且
/// `dir == 'out'`（`ui/settings/status.js::reportVerdict`）。而 T13b 让 shim 在「完美图标
/// 已启用」时主动向上游要应用项列表，于是同一页面上会先后出现三类上报：
///
/// 1. `FNOS_INJECTION_TRIGGERED`（`dir:'out'`，页面加载后上游自己发出）→ 强态；
/// 2. `FNOS_GET_LAUNCHPAD_APP_ITEMS`（`dir:'out'`，shim 自己发的那条请求）；
/// 3. 同上（`dir:'response'`，上游的 `{items,titles}` 应答）。
///
/// 2、3 都晚于 1。在**单槽**实现下它们会把 1 覆盖掉，于是「完美图标」一旦打开，状态条就
/// 再也回不到「已回报上游注入链触发」这一态——这正是 T13a 那条实测读数在新功能开启时的
/// **功能回归**（需求 D 明确点名「诚实的状态条」不许回归）。
///
/// 分流之后：状态槽只收「不是应用项列表」的上报（`FNOS_INJECTION_TRIGGERED` 因此不会被
/// 覆盖），应用项槽只收 [`APP_ITEMS_TYPES`] 里的两种。两个槽位都仍然受文档身份门约束
/// （[`ReportEntry::matches_document`]），都由 `commands::get_page_report` 一并返回。
///
/// 注意 `dir` **不**参与分流：请求与应答都属于「应用项列表」这件事，设置窗按 `dir` 区分
/// 「已经问过、还没有可用应答」与「拿到了应答但形状不对」（T14b 起这份文案的判据在
/// `ui/settings/app.js::appItemsAnswer`；说明文字由 `chrome-shim.js` 的可见说明框给出）。
///
/// ## 为什么 `FNOS_GET_LAUNCHPAD_APP_TITLES` 也算（fix round 1 / Minor 4）
///
/// 早先这里只认 [`APP_ITEMS_TYPE`]，而设置窗的 `APP_ITEM_REPORT_TYPES` 认两个。两侧不一致
/// 的直接后果是：**任何人真的请求过 titles，那条 `{items,titles}` 应答就会被塞进状态槽**
/// ——正好把上面第 3 条的功能回归重新引回来（而且逐项 UI 也永远读不到它）。
/// 「让两侧一致」有两条路：把 JS 的 titles 收窄，或者把宿主的判据放宽。**选了后者**，理由：
/// 上游对两个 type 的应答**形状逐字相同**（同一个 `if`、同一个 `sendResponse`），把它们当成
/// 两种东西没有任何事实依据；而 shim 的 `sendAppItems`/`responseForReport` 早已按
/// 「两个 type 都算应用项」写好（`inject/shim.js` 的 `isAppItemsType` 同义），收窄 JS 反而会让
/// 页面侧与设置窗对同一份上报有两种解释。判别式因此是「type ∈ [`APP_ITEMS_TYPES`]」，
/// 而不是「type == 某一个」。
pub fn is_app_items_report(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|ty| APP_ITEMS_TYPES.contains(&ty))
}

/// 允许出现在日志里的上报**方向**标签——逐字取自本壳 shim 的两处 `sendReport` 调用点
/// （`inject/shim.js:212` 的 `'out'`、`:218` 的 `'response'`），不是凭空发明的：
///
/// - `out` = 上游自己 `chrome.runtime.sendMessage` 出去的消息（只有 `FNOS_INJECTION_TRIGGERED`
///   一条，`content-script.js:2681`）；
/// - `response` = 上游 `sendResponse` 的应答原文（`{items,titles}` / `{applied:true}`，
///   `content-script.js:2848` / `:2865`）。
///
/// **为什么 `dir` 也要白名单（fix round 1 / Important 1）**：`dir` 与 `type` 一样来自页面可控的
/// 标题通道，且它**目前是唯一会被回显进 `[fnos] …` 日志行的页面可控字符串**。`serde_json`
/// 会把 JSON 的 `\n` 转义解成真换行，于是远程页面只要写一条
/// `{"type":"FNOS_CHECK","dir":"x\n[fnos] 页面上报已接受：…"}`，就能在项目用作**评审证据**的
/// stderr 日志里伪造出整行 `[fnos] …`（单条最多约 4 KB）。这条允许表把日志里可能出现的方向
/// 钉死成两个固定短语；其余一律渲染成 [`UNKNOWN`]（宿主自己的固定串）。
pub const REPORT_DIRS: [&str; 2] = ["out", "response"];

/// 日志里代替「页面可控文本取不到 / 不合法」的**宿主固定串**。
///
/// 它存在的意义是：任何页面可控文本要么命中白名单变成固定短语，要么变成这个常量
/// ——日志行里永远不出现页面写的字符。`commands.rs` 的 `origin` 也用它（origin 由宿主从
/// `window.url()` 解析，本就不是页面可控文本）。
pub const UNKNOWN: &str = "<未知>";

/// 丢弃一条上报的原因（只用于日志，不给页面看——页面根本收不到回执）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// 超过 [`MAX_REPORT_BYTES`]。
    TooLarge,
    /// 不是合法 JSON。
    NotJson,
    /// 是合法 JSON，但不是**对象**（数组/字符串/数字一律拒：上报体是有字段的信封）。
    NotObject,
    /// 缺 `type` 字段，或它不是字符串。
    TypeNotString,
    /// `type` 不在 [`REPORT_TYPES`] 允许表里。
    TypeNotAllowed,
}

impl Reject {
    /// 日志用的固定短语（不拼接任何页面可控文本）。
    pub fn as_str(self) -> &'static str {
        match self {
            Reject::TooLarge => "超过 32KiB 上限",
            Reject::NotJson => "不是合法 JSON",
            Reject::NotObject => "不是 JSON 对象",
            Reject::TypeNotString => "type 不是字符串",
            Reject::TypeNotAllowed => "type 不在允许表内",
        }
    }
}

// ---------- Task 13b：分片上报（一条大载荷拆成多片标题） ----------
//
// 为什么需要它：`document.title` 在到达宿主之前被 WebView2 截到 4096 字节
// （[`TITLE_CHANNEL_MAX_BYTES`] 的长度阶梯实测），而「完美图标」的逐项 UI 需要上游的
// **应用项列表**（`{items:[{key,title,iconSrc}], titles:[…]}`，几十个应用就是十几 KB）。
// 单条通道装不下，于是把同一份 JSON 拆成若干片，每片仍走 `document.title`。
//
// 威胁模型与单条上报**完全相同**（标题是页面可写的），所以这里的每一道闸门都是硬上限，
// 任何一条不满足就整条丢弃、并清掉内存里的半截状态：
//   - 片数 (`total`) ≤ [`MAX_CHUNKS`]、序号必须在 `[0, total)`；
//   - 单片正文 ≤ [`MAX_CHUNK_BODY_BYTES`]（加上前缀与序号后仍远小于 4096）；
//   - 累计 ≤ [`MAX_CHUNK_TOTAL_BYTES`]（= 单条上报的 32 KiB 上限）；
//   - 必须在 [`CHUNK_WINDOW_MS`] 之内到齐（半截状态不会长期占着内存）；
//   - 属于**同一个文档**（origin + URL 逐字相同，判据同 [`ReportEntry::matches_document`]）；
//     换了文档就从 seq=0 重新开始，旧文档的半截状态先被丢掉。
// 组装完成后**仍然**要过一遍 [`validate`]（字节上限 → JSON → 对象 → type 白名单），
// 所以分片通道不会成为绕过单条通道校验的后门。

/// 分片上报的控制前缀，与 [`REPORT_TITLE_PREFIX`] 并列的第二条控制标题。
///
/// 同样必须是**可打印 ASCII**（WebView2 会吃掉控制字符），也必须与页面侧 `shim.js`
/// 里的字面量逐字一致（`commands.rs` 的测试锁定这一点）。
pub const CHUNK_TITLE_PREFIX: &str = "FNOSCHUNK:";

/// 一次分片上报允许的最大片数。8 × 3000 字节 ≈ 24 KiB，落在 32 KiB 总上限内。
pub const MAX_CHUNKS: usize = 8;

/// 单片**正文**的字节上限。
///
/// 标题总长 = 前缀(10) + `seq,total,`(最多 4+1+3+1 = 9) + 正文。留到 4096 还有近 1000 字节
/// 余量：多字节字符（中文应用名）按 UTF-8 字节切，页面侧的切分与宿主侧的判定用同一把尺子。
pub const MAX_CHUNK_BODY_BYTES: usize = 3000;

/// 一次分片上报的**总**字节上限（= 单条上报的上限 [`MAX_REPORT_BYTES`]）。
///
/// **实测上到不了这一步**：8 片 × [`MAX_CHUNK_BODY_BYTES`] = 24000 字节 < 32 KiB，所以它是
/// 纵深防御（与 [`MAX_REPORT_BYTES`] 自己一样——后者也被标题通道的 4096 字节挡在前面）。
/// 留着它的理由：将来有人调大 `MAX_CHUNKS` 或单片上限时，这里仍然是最后一道总闸。
pub const MAX_CHUNK_TOTAL_BYTES: usize = MAX_REPORT_BYTES;

/// 分片必须在这么久之内到齐（从第 0 片算起）。
pub const CHUNK_WINDOW_MS: u64 = 5_000;

/// 丢弃一片分片（或一条组装）的原因——只用于日志的固定短语。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkReject {
    /// 不是 `<seq>,<total>,<body>` 这个形状（缺逗号 / 序号不是数字 / 正文为空）。
    BadHeader,
    /// 序号不在 `[0, total)` 内。
    SeqOutOfRange,
    /// `total` 为 0 或超过 [`MAX_CHUNKS`]。
    TooManyChunks,
    /// 单片正文超过 [`MAX_CHUNK_BODY_BYTES`]。
    ChunkTooLarge,
    /// 累计超过 [`MAX_CHUNK_TOTAL_BYTES`]，或中途改了 `total`。
    TotalTooLarge,
    /// 没有进行中的组装，而这一片的序号不是 0。
    NoStart,
    /// 取不到当前文档的身份（`window.url()` 失败 / 还没导航完）。
    ///
    /// 为什么不「先存着、等身份出来再补」：`get_page_report` 的返回判据是
    /// [`ReportEntry::matches_document`]，一个身份不明的组装**即使到齐也永远不会被设置窗读到**。
    /// 整条丢弃是更诚实的选择（日志里留一行），也避免内存里留一份用不上的载荷。
    NoDocument,
    /// 序号不是这一轮期望的下一片（乱序 / 重放）。
    OutOfOrder,
    /// 距第 0 片已经超过 [`CHUNK_WINDOW_MS`]。
    Stale,
}

impl ChunkReject {
    /// 日志用的固定短语（不拼接任何页面可控文本）。
    pub fn as_str(self) -> &'static str {
        match self {
            ChunkReject::BadHeader => "分片头部不是 seq,total,body",
            ChunkReject::SeqOutOfRange => "分片序号越界",
            ChunkReject::TooManyChunks => "分片总数超过 8",
            ChunkReject::ChunkTooLarge => "单片正文超过 3000 字节",
            ChunkReject::TotalTooLarge => "分片累计超过 32KiB（或中途改了总数）",
            ChunkReject::NoStart => "没有进行中的组装（序号不是 0）",
            ChunkReject::NoDocument => "取不到当前文档的身份（无法归属，整条丢弃）",
            ChunkReject::OutOfOrder => "分片乱序",
            ChunkReject::Stale => "分片超过 5 秒窗口",
        }
    }
}

/// 一片分片的正文字节上限在**通道**上的余量检查（前缀 + 头部都算进去）。
fn chunk_title_len_ok(body: &str) -> bool {
    CHUNK_TITLE_PREFIX.len() + 16 + body.len() <= TITLE_CHANNEL_MAX_BYTES
}

/// 解析一片分片的正文：`<seq>,<total>,<body>`。
///
/// 只在前两个逗号处切分，正文里的逗号不受影响（它是 JSON 文本的一部分）。
/// 返回 `(seq, total, body)`；任何形状/边界不合法 → [`ChunkReject`]。
pub fn parse_chunk(payload: &str) -> Result<(usize, usize, &str), ChunkReject> {
    let mut parts = payload.splitn(3, ',');
    let seq_raw = parts.next().ok_or(ChunkReject::BadHeader)?;
    let total_raw = parts.next().ok_or(ChunkReject::BadHeader)?;
    let body = parts.next().ok_or(ChunkReject::BadHeader)?;
    if body.is_empty() {
        return Err(ChunkReject::BadHeader);
    }
    if body.len() > MAX_CHUNK_BODY_BYTES || !chunk_title_len_ok(body) {
        return Err(ChunkReject::ChunkTooLarge);
    }
    let seq: usize = seq_raw.parse().map_err(|_| ChunkReject::BadHeader)?;
    let total: usize = total_raw.parse().map_err(|_| ChunkReject::BadHeader)?;
    if total == 0 || total > MAX_CHUNKS {
        return Err(ChunkReject::TooManyChunks);
    }
    if seq >= total {
        return Err(ChunkReject::SeqOutOfRange);
    }
    Ok((seq, total, body))
}

/// 一次分片上报的组装状态（只在内存里，跟随一次文档）。
#[derive(Debug, Clone)]
pub struct ChunkAssembly {
    origin: Option<String>,
    url: Option<String>,
    total: usize,
    next_seq: usize,
    bytes: usize,
    started: std::time::Instant,
    parts: Vec<String>,
}

/// 喂一片分片的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkStep {
    /// 已接受，组装还没完成（`seq` 是刚接受的那一片）。
    Accepted { seq: usize, total: usize },
    /// 全部分片到齐：正文（调用方**必须**再走一遍 [`validate`]）。
    Complete(String),
    /// 被拒（内存里的半截状态已被清空）。
    Rejected(ChunkReject),
}

impl ChunkAssembly {
    fn start(
        origin: Option<String>,
        url: Option<String>,
        total: usize,
        now: std::time::Instant,
    ) -> Self {
        Self {
            origin,
            url,
            total,
            next_seq: 0,
            bytes: 0,
            started: now,
            parts: Vec::with_capacity(total),
        }
    }

    /// 这一轮组装是否还属于这个文档（判据与 [`ReportEntry::matches_document`] 完全一致）。
    ///
    /// 调用方（[`accept_chunk`]）保证两侧身份都是 `Some`：身份取不到时那一轮根本不会开始，
    /// 所以这里不需要「两侧都取不到」的分支。
    pub fn belongs_to(&self, origin: Option<&str>, url: Option<&str>) -> bool {
        same_origin(self.origin.as_deref(), origin) && same_document(self.url.as_deref(), url)
    }

    /// 仅供单测：把「已累计字节」人为顶到上限附近。
    ///
    /// 真实通道**到不了**这条闸门（8 × 3000 < 32 KiB，见 [`MAX_CHUNK_TOTAL_BYTES`]），
    /// 但它必须被证明真的会拦——否则将来调大片数上限时这里是静默失效的。
    #[cfg(test)]
    fn seed_bytes_for_test(&mut self, bytes: usize) {
        self.bytes = bytes;
    }

    /// 接受一片；由 [`accept_chunk`] 调用（那里负责「文档变了 / 没有进行中的组装」的分支）。
    fn accept(&mut self, seq: usize, body: &str, now: std::time::Instant) -> ChunkStep {
        if now.duration_since(self.started).as_millis() as u64 > CHUNK_WINDOW_MS {
            return ChunkStep::Rejected(ChunkReject::Stale);
        }
        if seq != self.next_seq {
            return ChunkStep::Rejected(ChunkReject::OutOfOrder);
        }
        if self.bytes + body.len() > MAX_CHUNK_TOTAL_BYTES {
            return ChunkStep::Rejected(ChunkReject::TotalTooLarge);
        }
        self.bytes += body.len();
        self.parts.push(body.to_string());
        self.next_seq += 1;
        if self.next_seq == self.total {
            ChunkStep::Complete(self.parts.concat())
        } else {
            ChunkStep::Accepted {
                seq,
                total: self.total,
            }
        }
    }
}

/// 把一片分片喂给「进行中的组装」`slot`。
///
/// `slot` 的语义是「**当前文档**的半截上报」：文档身份（origin + URL）对不上、或 `slot` 为空时，
/// 只有 `seq == 0` 能开新一轮；其余情况整轮作废。**任何** [`ChunkReject`] 都会把 `slot` 清空
/// （拒绝之后内存里不留半截载荷），`Complete` 也会清空（这一轮结束了）。
///
/// `now` 由调用方注入（`Instant::now()`）：窗口判定因此可以在单测里确定性地构造。
pub fn accept_chunk(
    slot: &mut Option<ChunkAssembly>,
    origin: Option<&str>,
    url: Option<&str>,
    payload: &str,
    now: std::time::Instant,
) -> ChunkStep {
    // 归属不明的分片直接丢：即使到齐也永远不会被 `get_page_report` 返回（见 `ChunkReject::NoDocument`）
    let (Some(origin), Some(url)) = (origin, url) else {
        *slot = None;
        return ChunkStep::Rejected(ChunkReject::NoDocument);
    };
    let (seq, total, body) = match parse_chunk(payload) {
        Ok(v) => v,
        Err(reason) => {
            *slot = None;
            return ChunkStep::Rejected(reason);
        }
    };
    let restart = match slot.as_ref() {
        Some(assembly) => !assembly.belongs_to(Some(origin), Some(url)),
        None => true,
    };
    if restart {
        if seq != 0 {
            *slot = None;
            return ChunkStep::Rejected(ChunkReject::NoStart);
        }
        *slot = Some(ChunkAssembly::start(
            Some(origin.to_string()),
            Some(url.to_string()),
            total,
            now,
        ));
    } else if let Some(assembly) = slot.as_ref() {
        // 中途改 total：整轮作废（否则「先声称 8 片、再只发 2 片」就能骗过一次完成判定）
        if assembly.total != total {
            *slot = None;
            return ChunkStep::Rejected(ChunkReject::TotalTooLarge);
        }
    }
    let step = match slot.as_mut() {
        Some(assembly) => assembly.accept(seq, body, now),
        // 上面的两个分支已经覆盖了 `None`，这里只是「不 panic」的兜底
        None => ChunkStep::Rejected(ChunkReject::NoStart),
    };
    if matches!(step, ChunkStep::Rejected(_) | ChunkStep::Complete(_)) {
        *slot = None;
    }
    step
}

/// 校验一条上报体；通过则返回解析后的 JSON 对象（**原样**，不改一个字段）。
///
/// 刻意**不**在这里深挖载荷结构（`payload` 的形状由消费方 `ui/settings/status.js` 判定，
/// 那里对 `dir` 用的是**严格等于** `'out'` 的比较）：宿主这一层只回答「这是不是一个我们能
/// 安全放进内存、并在设置窗里读出来的东西」。越靠前卡死，后面能戳到的东西越少：
/// 长度 → JSON → 对象 → `type` 白名单。
///
/// `dir` **不在**这里判（fix round 1 的结论，见 [`dir_label`]）：它不进权限、不进配置、
/// 也不进 UI 文本，唯一的用法是状态条里的 `dir === 'out'` 严格比较；它曾经会原样进日志，
/// 现在日志侧由 [`accepted_log_line`] 用 [`REPORT_DIRS`] 收口。把闸门加在这里只会让
/// 「页面写了个奇怪的 dir」从「日志里显示 `<未知>`」变成「整条上报被丢」，观测信息更少而
/// 安全性一样。
pub fn validate(payload: &str) -> Result<Value, Reject> {
    if payload.len() > MAX_REPORT_BYTES {
        return Err(Reject::TooLarge);
    }
    let value: Value = serde_json::from_str(payload).map_err(|_| Reject::NotJson)?;
    if !value.is_object() {
        return Err(Reject::NotObject);
    }
    let ty = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or(Reject::TypeNotString)?;
    if !REPORT_TYPES.contains(&ty) {
        return Err(Reject::TypeNotAllowed);
    }
    Ok(value)
}

/// 日志里可以安全出现的上报**方向**取值：命中 [`REPORT_DIRS`] 原样返回，其余（缺失 / 不是
/// 字符串 / 页面自己编的串）一律返回 [`UNKNOWN`]。
///
/// 返回 `&'static str` 是**类型层面**的保证：调用方拿到的永远不是页面可控文本的切片，
/// 于是 `[fnos] …` 日志行里不可能出现页面写的换行、制表符，或一整行伪造的日志。
pub fn dir_label(raw: Option<&str>) -> &'static str {
    let Some(raw) = raw else {
        return UNKNOWN;
    };
    REPORT_DIRS
        .iter()
        .copied()
        .find(|d| *d == raw)
        .unwrap_or(UNKNOWN)
}

/// 把一段**可能被页面控制**的文本渲染成「一行日志里安全」的形式。
///
/// 唯一目的是防日志注入：控制字符（C0 / DEL / C1）、行分隔符 U+2028 / U+2029 全部转成
/// `\n` / `\u{85}` 这类**可见的转义序列**，于是任何输入都不能在 stderr 里伪造出一条新的
/// `[fnos] …` 行。文本本身**不截断、不丢弃**（它是评审证据），只是不再具备换行能力。
pub fn log_safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                out.push_str(&format!("\\u{{{:04x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// 「页面上报已接受」那**一行**日志的完整渲染（`commands.rs::on_page_report` 只用它）。
///
/// 抽成纯函数有两个好处：① 日志格式只有一处事实来源；② 可以在单测里拿敌意输入直接断言
/// 「这一行不含任何换行」（见 `hostile_dir_cannot_forge_a_log_line`）。
///
/// 三个可变字段逐个收口，任何一个都不允许原样进日志：
/// - `ty`：不在 [`REPORT_TYPES`] 里 → [`UNKNOWN`]（`validate` 已保证过一遍，这里是纵深防御）；
/// - `raw_dir`：不在 [`REPORT_DIRS`] 里 → [`UNKNOWN`]（见 [`dir_label`]）；
/// - `origin`：宿主从 `window.url()` 解析出来的 origin（**不是**页面可控文本），仍然过一遍
///   [`log_safe`]，让「这一行不可能被换行」成为与调用点无关的性质。
pub fn accepted_log_line(
    ty: &str,
    raw_dir: Option<&str>,
    origin: Option<&str>,
    bytes: usize,
) -> String {
    let ty = if REPORT_TYPES.contains(&ty) {
        ty
    } else {
        UNKNOWN
    };
    format!(
        "[fnos] 页面上报已接受：type={ty} dir={} origin={} 字节={bytes}",
        dir_label(raw_dir),
        log_safe(origin.unwrap_or(UNKNOWN)),
    )
}

/// 最近一次被接受的上报，外加「它来自哪个文档」。
///
/// 记来源是为了诚实性：主窗口可以在**不改任何配置**的情况下导航，旧页面上报过的「已注入」
/// 不该被拿来描述新页面。判据分两层：**文档 URL 逐字相同**（fix round 1 / Minor 3）叠加
/// origin 相同（注入与否在上游是按 origin 判定的：白名单是 origin 列表，
/// `content-script.js:2947`）。
///
/// 为什么 origin 相同还不够（Minor 3）：同一个白名单 origin 下可以有任意多个文档
/// （`/big.html` → `/other.html`）。上游的注入是**每个文档**各跑一次
/// `startInject()`，第二个文档完全可能因为缺 fnOS 签名而没注入——此时旧文档的「已上报注入
/// 链触发」留在设置窗上就是**拿上一张页面的证据描述这一张**。origin 只回答「换了站点没有」，
/// 回答不了「换了文档没有」，所以必须再记 URL。
///
/// 同源 SPA 用 `history.pushState` 改 URL 时不会重新加载文档（注入仍然有效），而这里的 URL
/// 判据会把它当成「换了文档」而退回弱文案——这是刻意的方向：观测通道宁可少说，也不拿一条
/// 归属不明的证据去说「已注入」。
#[derive(Debug, Clone)]
pub struct ReportEntry {
    pub value: Value,
    /// 上报时主窗口文档的 origin（`config::origin_of`；解析不出来时 `None`）。
    pub origin: Option<String>,
    /// 上报时主窗口文档的**URL**（`window.url()` 的序列化；取不到时 `None`）。
    pub url: Option<String>,
}

impl ReportEntry {
    /// 这条上报还属不属于「当前主窗口文档」：origin 与文档 URL 都必须能取到且相等。
    ///
    /// 两个判据都要，缺一不可：只比 URL 会让「URL 取不到」的情形（窗口刚销毁 / 还没导航完）
    /// 退化成「随便一条上报都算」，只比 origin 就是 Minor 3 的原缺陷。
    pub fn matches_document(&self, origin: Option<&str>, url: Option<&str>) -> bool {
        same_origin(self.origin.as_deref(), origin) && same_document(self.url.as_deref(), url)
    }
}

/// 上报还属不属于当前文档：**文档 URL 必须都能解析出来且逐字相等**。
///
/// 与 [`same_origin`] 同一套「宁可退回弱文案」的立场：任一侧 `None` 一律判「不属于」。
pub fn same_document(report: Option<&str>, current: Option<&str>) -> bool {
    match (report, current) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// 上报还属不属于当前文档：来源 origin 与当前 origin 必须**都能解析出来且相等**。
///
/// 任一侧取不到（`None`）一律判「不属于」——观测通道宁可退回弱文案，也不拿一条无法归属的
/// 证据去说「已注入」。
pub fn same_origin(report: Option<&str>, current: Option<&str>) -> bool {
    match (report, current) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(type_: &str) -> String {
        format!(
            r#"{{"type":"{type_}","dir":"out","payload":{{"triggerReason":"auto_whitelist"}}}}"#
        )
    }

    /// 前缀必须与 `shim.js` 里的字面量逐字一致，否则上报会被静默当成普通标题丢掉。
    #[test]
    fn prefix_is_printable_ascii_and_unique() {
        assert!(!REPORT_TITLE_PREFIX.is_empty());
        assert!(
            REPORT_TITLE_PREFIX.chars().all(|c| c.is_ascii_graphic()),
            "前缀必须是可打印 ASCII（WebView2 会吃掉控制字符）"
        );
        assert!(
            !REPORT_TITLE_PREFIX.starts_with(crate::commands::PROBE_TITLE_PREFIX),
            "两个控制前缀不得互相包含，否则分支顺序会决定谁被吃掉"
        );
    }

    #[test]
    fn accepts_every_allow_listed_type() {
        for ty in REPORT_TYPES {
            let v = validate(&ok(ty)).unwrap_or_else(|e| panic!("{ty} 应被接受，实为 {e:?}"));
            assert_eq!(v.get("type").and_then(Value::as_str), Some(ty));
            // 原样保留：载荷字段一个都没被改写（含上游自己的 triggerReason）
            assert!(v.get("payload").is_some());
        }
    }

    #[test]
    fn rejects_oversized_payload_before_parsing() {
        // 一个「本来合法」的对象灌到 32KiB 以上：必须在解析前就被长度闸门挡掉
        let big = format!(
            r#"{{"type":"FNOS_CHECK","dir":"out","pad":"{}"}}"#,
            "x".repeat(MAX_REPORT_BYTES)
        );
        assert!(big.len() > MAX_REPORT_BYTES);
        assert_eq!(validate(&big), Err(Reject::TooLarge));
        // 边界：恰好等于上限仍是「超限」还是通过？上限是**含**上限（> 才拒）
        let edge = format!(
            r#"{{"type":"FNOS_CHECK","pad":"{}"}}"#,
            "y".repeat(MAX_REPORT_BYTES - 30)
        );
        assert!(edge.len() <= MAX_REPORT_BYTES);
        assert!(validate(&edge).is_ok(), "等于上限的载荷不该被拒");
    }

    #[test]
    fn rejects_non_json_non_object_and_bad_type() {
        assert_eq!(validate("not json"), Err(Reject::NotJson));
        assert_eq!(validate(""), Err(Reject::NotJson));
        assert_eq!(validate("[]"), Err(Reject::NotObject));
        assert_eq!(validate("\"FNOS_CHECK\""), Err(Reject::NotObject));
        assert_eq!(validate("42"), Err(Reject::NotObject));
        assert_eq!(validate("null"), Err(Reject::NotObject));
        assert_eq!(validate(r#"{"dir":"out"}"#), Err(Reject::TypeNotString));
        assert_eq!(validate(r#"{"type":7}"#), Err(Reject::TypeNotString));
        assert_eq!(
            validate(r#"{"type":"FNOS_PAGE_STATUS"}"#),
            Err(Reject::TypeNotAllowed),
            "brief 里那个凭空的 type 不在允许表内（页面可控文本不得进内存）"
        );
        assert_eq!(
            validate(r#"{"type":"fnos_check"}"#),
            Err(Reject::TypeNotAllowed),
            "type 比较大小写敏感（上游的 type 全是大写）"
        );
        // 原型链式的怪键也必须被当作「不是对象/不是字符串」处理，不得 panic
        assert_eq!(validate(r#"{"type":{"a":1}}"#), Err(Reject::TypeNotString));
    }

    #[test]
    fn same_origin_is_strict() {
        assert!(same_origin(Some("http://a:1"), Some("http://a:1")));
        assert!(!same_origin(Some("http://a:1"), Some("http://b:1")));
        assert!(!same_origin(Some("http://a:1"), None));
        assert!(!same_origin(None, Some("http://a:1")));
        assert!(
            !same_origin(None, None),
            "两侧都取不到 origin 时不得声称「还属于当前文档」"
        );
    }

    /// 截断提示的判据：`payload + 前缀` 够到实测上限就提示（这条日志是给 T13b 排障用的）。
    #[test]
    fn truncation_hint_covers_exactly_the_measured_ceiling() {
        // 实测：4085 字节的载荷（= 4096 总长）能被完整送达；4096 字节的载荷只到 4085
        assert!(truncation_suspected(4085));
        assert!(truncation_suspected(4086));
        assert!(truncation_suspected(MAX_REPORT_BYTES));
        assert!(
            !truncation_suspected(4084),
            "明显短于通道上限的载荷不该被提示成『疑似截断』"
        );
        assert!(!truncation_suspected(200));
    }

    /// **fix round 1 / Important 1 的核心用例**：带真换行的敌意 `dir` 不能伪造日志行。
    ///
    /// 攻击形状：`serde_json` 把 JSON 里的 `\n` 转义解码成真换行，所以页面只要写一条
    /// `{"type":"FNOS_CHECK","dir":"out\n[fnos] 页面上报已接受：…"}`，旧实现（`dir={dir}`
    /// 直接插值）就会在用作评审证据的 stderr 里多出一整行看似宿主自己写的 `[fnos] …`。
    /// 断言的是**那一行日志本身**：它必须只有一个换行（`eprintln!` 末尾那个）、不含任何
    /// 控制字符、也不含页面写的任何文本。
    #[test]
    fn hostile_dir_cannot_forge_a_log_line() {
        let forged = "out\n[fnos] 页面上报已接受：type=FNOS_INJECTION_TRIGGERED dir=out origin=http://evil.example 字节=287";
        let line = accepted_log_line(
            "FNOS_CHECK",
            Some(forged),
            Some("http://127.0.0.1:8796"),
            123,
        );
        assert_eq!(line.lines().count(), 1, "日志必须只有一行：{line}");
        assert!(
            !line.chars().any(|c| c.is_control()),
            "日志行里不得出现任何控制字符：{line}"
        );
        assert!(
            !line.contains("evil.example"),
            "页面可控文本一个字符都不许进日志：{line}"
        );
        assert!(line.contains("dir=<未知>"), "{line}");
        // 合法取值仍按原样打出来（证据格式没变，白名单外的才被中和）
        for d in REPORT_DIRS {
            let ok = accepted_log_line("FNOS_CHECK", Some(d), Some("http://a:1"), 5);
            assert!(ok.contains(&format!("dir={d}")), "{ok}");
            assert_eq!(ok.lines().count(), 1);
        }
        // 走私控制字符：C1 的 NEL、行分隔符 U+2028、以及非字符串 dir（None）都不能换行
        for sneaky in ["out\u{0085}x", "out\u{2028}[fnos] x", "out\u{0007}"] {
            let l = accepted_log_line("FNOS_APPLY", Some(sneaky), None, 1);
            assert_eq!(l.lines().count(), 1, "{sneaky:?} → {l}");
            assert!(!l.chars().any(|c| c.is_control()), "{sneaky:?} → {l}");
            assert!(l.contains("dir=<未知>"), "{l}");
        }
        let missing = accepted_log_line("FNOS_APPLY", None, None, 1);
        assert!(missing.contains("dir=<未知>") && missing.contains("origin=<未知>"));
        // type 也过一遍：不在允许表里 → 固定串（validate 已经拦过，这里是纵深防御）
        let bad_ty = accepted_log_line("FNOS_CHECK\n[fnos] x", Some("out"), None, 1);
        assert!(bad_ty.contains("type=<未知>"), "{bad_ty}");
        assert_eq!(bad_ty.lines().count(), 1);
    }

    /// 允许表必须与 shim 的两处调用点**集合相等**：多了是死条目，少了就是把页面可控文本
    /// 原样写回日志（旧实现的行为）。
    #[test]
    fn allowed_dirs_match_the_shim_call_sites() {
        let shim = include_str!("../inject/shim.js");
        let mut used: Vec<String> = Vec::new();
        for raw in shim.lines() {
            let line = raw.trim();
            // 只看调用点，不看 `function sendReport(type, dir, payload) {` 这个定义
            if !line.contains("sendReport(") || line.starts_with("function ") {
                continue;
            }
            // 调用形状固定：`sendReport(reportType, '<dir>', <payload>)`
            for arg in line.split(',') {
                let arg = arg.trim();
                if arg.len() >= 3 && arg.starts_with('\'') && arg.ends_with('\'') {
                    used.push(arg.trim_matches('\'').to_string());
                }
            }
        }
        used.sort();
        used.dedup();
        assert_eq!(
            used,
            REPORT_DIRS.map(str::to_string).to_vec(),
            "REPORT_DIRS 必须与 shim.js 的 sendReport 调用点集合相等"
        );
    }

    /// `log_safe`：换行/控制字符变成可见转义，其余文本逐字保留（日志是评审证据，不截断）。
    #[test]
    fn log_safe_escapes_every_line_breaker() {
        assert_eq!(log_safe("a\nb"), "a\\nb");
        assert_eq!(log_safe("a\rb"), "a\\rb");
        assert_eq!(log_safe("a\tb"), "a\\tb");
        assert_eq!(log_safe("a\u{0007}b"), "a\\u{0007}b");
        assert_eq!(log_safe("a\u{0085}b"), "a\\u{0085}b");
        assert_eq!(log_safe("a\u{2028}b"), "a\\u{2028}b");
        assert_eq!(log_safe("a\u{2029}b"), "a\\u{2029}b");
        assert_eq!(log_safe("主窗口标题: T13A-BIG"), "主窗口标题: T13A-BIG");
        assert_eq!(log_safe(""), "");
        assert_eq!(log_safe("x\ny\nz").lines().count(), 1);
    }

    /// 文档身份比 origin 更严：同源换文档必须判「不属于」（Minor 3 的原缺陷）。
    #[test]
    fn document_identity_is_stricter_than_origin() {
        assert!(same_document(
            Some("http://a:1/one"),
            Some("http://a:1/one")
        ));
        assert!(
            !same_document(Some("http://a:1/one"), Some("http://a:1/two")),
            "同源不同文档必须不认"
        );
        assert!(!same_document(Some("http://a:1/one"), None));
        assert!(!same_document(None, Some("http://a:1/one")));
        assert!(!same_document(None, None));

        let entry = ReportEntry {
            value: serde_json::json!({ "type": "FNOS_CHECK" }),
            origin: Some("http://a:1".into()),
            url: Some("http://a:1/one".into()),
        };
        assert!(entry.matches_document(Some("http://a:1"), Some("http://a:1/one")));
        assert!(
            !entry.matches_document(Some("http://a:1"), Some("http://a:1/two")),
            "同源换文档必须不认（origin 相同也救不了）"
        );
        assert!(
            !entry.matches_document(Some("http://b:1"), Some("http://a:1/one")),
            "换 origin 必须不认（URL 相同也救不了）"
        );
        assert!(!entry.matches_document(None, None), "两侧都取不到一律不认");
    }

    // ---------- Task 13b：分片上报（一块大载荷拆成多片） ----------

    /// 分片头部的形状与每一条硬边界（都是**安全**边界，不是可用性调优）。
    #[test]
    fn parse_chunk_accepts_only_the_bounded_shape() {
        // 正常形状 + 正文里的逗号不受影响（正文是 JSON，逗号是它的一部分）
        assert_eq!(
            parse_chunk("0,2,{\"a\":1,\"b\":2}").unwrap(),
            (0, 2, "{\"a\":1,\"b\":2}")
        );
        assert_eq!(parse_chunk("1,3,xyz").unwrap(), (1, 3, "xyz"));
        // 序号为 0 之外的数字、前导零都能解析
        assert_eq!(parse_chunk("07,8,abc").unwrap(), (7, 8, "abc"));

        for (payload, want) in [
            ("", ChunkReject::BadHeader),
            ("0", ChunkReject::BadHeader),
            ("0,2", ChunkReject::BadHeader),
            ("0,2,", ChunkReject::BadHeader),
            ("a,2,x", ChunkReject::BadHeader),
            ("0,b,x", ChunkReject::BadHeader),
            ("-1,2,x", ChunkReject::BadHeader),
            ("0,0,x", ChunkReject::TooManyChunks),
            ("0,9,x", ChunkReject::TooManyChunks),
            ("0,100,x", ChunkReject::TooManyChunks),
            ("2,2,x", ChunkReject::SeqOutOfRange),
            ("8,8,x", ChunkReject::SeqOutOfRange),
        ] {
            assert_eq!(parse_chunk(payload), Err(want), "payload={payload:?}");
        }

        // 单片正文上限：3000 恰好可以，3001 拒绝（按**字节**）
        let ok_body = "y".repeat(MAX_CHUNK_BODY_BYTES);
        assert_eq!(
            parse_chunk(&format!("0,8,{ok_body}")).unwrap().2.len(),
            MAX_CHUNK_BODY_BYTES
        );
        let too_big = "y".repeat(MAX_CHUNK_BODY_BYTES + 1);
        assert_eq!(
            parse_chunk(&format!("0,8,{too_big}")),
            Err(ChunkReject::ChunkTooLarge)
        );
        // 多字节字符按字节算：1001 个汉字 = 3003 字节 > 3000
        let cjk = "汉".repeat(1001);
        assert_eq!(cjk.len(), 3003);
        assert_eq!(
            parse_chunk(&format!("0,8,{cjk}")),
            Err(ChunkReject::ChunkTooLarge)
        );
        let cjk_ok = "汉".repeat(1000);
        assert!(parse_chunk(&format!("0,8,{cjk_ok}")).is_ok());

        // 即使正文在上限内，「前缀 + 头部 + 正文」也必须仍在标题通道的实测上限之内
        assert!(chunk_title_len_ok(&"z".repeat(MAX_CHUNK_BODY_BYTES)));
        assert!(CHUNK_TITLE_PREFIX.len() + MAX_CHUNK_BODY_BYTES + 16 < TITLE_CHANNEL_MAX_BYTES);
    }

    /// 一片一片按序喂 → 到齐后正文与页面发出来的**逐字节相同**（含多字节字符与逗号）。
    #[test]
    fn in_order_chunks_reassemble_byte_identically() {
        let body = "{\"type\":\"FNOS_GET_LAUNCHPAD_APP_ITEMS\",\"items\":[{\"title\":\"迅雷\"},{\"title\":\"qBittorrent\"}]}";
        let mid = body.len() / 2;
        // 故意切在多字节字符中间是不可能的（切点由页面按码点选），这里手动保证切点在字符边界
        let cut = (0..=body.len())
            .find(|i| body.is_char_boundary(*i) && *i >= mid)
            .unwrap();
        let chunks = [&body[..cut], &body[cut..]];

        let mut slot = None;
        let now = std::time::Instant::now();
        for (i, chunk) in chunks.iter().enumerate() {
            let step = accept_chunk(
                &mut slot,
                Some("http://a:1"),
                Some("http://a:1/x"),
                &format!("{i},2,{chunk}"),
                now,
            );
            if i == 0 {
                assert_eq!(step, ChunkStep::Accepted { seq: 0, total: 2 });
                assert!(slot.is_some(), "第一片之后必须有进行中的组装");
            } else {
                assert_eq!(step, ChunkStep::Complete(body.to_string()));
                assert!(slot.is_none(), "完成后不得留下半截状态");
            }
        }
        // 组装结果必须过得了单条通道的全部闸门（分片不是绕过 validate 的后门）
        assert!(validate(body).is_ok());
    }

    /// 乱序 / 跳号 / 没有开头：一律拒绝，且**拒绝后内存里不留半截载荷**。
    #[test]
    fn out_of_order_or_startless_chunks_are_rejected_and_clear_the_slot() {
        let now = std::time::Instant::now();
        let doc = (Some("http://a:1"), Some("http://a:1/x"));

        // 没有开头就发第 1 片
        let mut slot = None;
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "1,2,xyz", now),
            ChunkStep::Rejected(ChunkReject::NoStart)
        );
        assert!(slot.is_none());

        // 正常开一轮，然后跳号（0 之后直接 2）
        let mut slot = None;
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "0,3,aaa", now),
            ChunkStep::Accepted { seq: 0, total: 3 }
        );
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "2,3,ccc", now),
            ChunkStep::Rejected(ChunkReject::OutOfOrder)
        );
        assert!(slot.is_none(), "乱序之后必须丢掉半截状态");
        // 丢掉之后连「下一片」也不再被当成续传（必须重新从 0 开始）
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "2,3,ccc", now),
            ChunkStep::Rejected(ChunkReject::NoStart)
        );

        // 重放第 0 片（同一轮里第二次 0）：被当成乱序
        let mut slot = None;
        accept_chunk(&mut slot, doc.0, doc.1, "0,2,aaa", now);
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "0,2,aaa", now),
            ChunkStep::Rejected(ChunkReject::OutOfOrder)
        );
        assert!(slot.is_none());

        // 头部本身不合法（不是 JSON 也不是片段）：同样清空
        let mut slot = None;
        accept_chunk(&mut slot, doc.0, doc.1, "0,2,aaa", now);
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "garbage", now),
            ChunkStep::Rejected(ChunkReject::BadHeader)
        );
        assert!(slot.is_none());
    }

    /// 每一条硬上限都真的拦得住：片数、单片字节、累计字节、时间窗口、中途改 total。
    #[test]
    fn every_hard_bound_is_enforced() {
        let now = std::time::Instant::now();
        let doc = (Some("http://a:1"), Some("http://a:1/x"));

        // 片数：total=9 直接拒（连第一片都不接受）
        let mut slot = None;
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "0,9,aaa", now),
            ChunkStep::Rejected(ChunkReject::TooManyChunks)
        );
        assert!(slot.is_none());

        // 累计：真实通道到不了这条闸门（8 × 3000 = 24000 < 32KiB），所以直接把它顶到上限附近
        // 再喂一片——否则这个守卫只是「写在代码里」，没有任何证据证明它会拦。
        let mut assembly = ChunkAssembly::start(
            Some("http://a:1".into()),
            Some("http://a:1/x".into()),
            2,
            now,
        );
        assembly.seed_bytes_for_test(MAX_CHUNK_TOTAL_BYTES - 1);
        assert_eq!(
            assembly.accept(0, "aaaa", now),
            ChunkStep::Rejected(ChunkReject::TotalTooLarge)
        );
        // 恰好填满则放行（上限是「含」上限：`>` 才拒）
        let mut assembly = ChunkAssembly::start(
            Some("http://a:1".into()),
            Some("http://a:1/x".into()),
            1,
            now,
        );
        assembly.seed_bytes_for_test(MAX_CHUNK_TOTAL_BYTES - 4);
        assert_eq!(
            assembly.accept(0, "aaaa", now),
            ChunkStep::Complete("aaaa".to_string())
        );
        // 再从公开入口确认「真实通道够不到这个上限」：8 片 × 3000 字节应当到齐
        let mut slot = None;
        let body = "z".repeat(MAX_CHUNK_BODY_BYTES);
        for i in 0..8 {
            let step = accept_chunk(
                &mut slot,
                doc.0,
                doc.1,
                &format!("{i},8,{body}"),
                now + std::time::Duration::from_millis(i as u64),
            );
            if i == 7 {
                match step {
                    ChunkStep::Complete(assembled) => assert_eq!(assembled.len(), 24_000),
                    other => panic!("24000 字节 < 32KiB，应当到齐：{other:?}"),
                }
            }
        }
        assert!(slot.is_none(), "完成后必须清空半截状态");

        // 时间窗口：第 0 片之后隔一个窗口再多 1ms
        let mut slot = None;
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "0,2,aaa", now),
            ChunkStep::Accepted { seq: 0, total: 2 }
        );
        assert_eq!(
            accept_chunk(
                &mut slot,
                doc.0,
                doc.1,
                "1,2,bbb",
                now + std::time::Duration::from_millis(CHUNK_WINDOW_MS + 1)
            ),
            ChunkStep::Rejected(ChunkReject::Stale)
        );
        assert!(slot.is_none());

        // 中途改 total：整轮作废
        let mut slot = None;
        accept_chunk(&mut slot, doc.0, doc.1, "0,4,aaa", now);
        assert_eq!(
            accept_chunk(&mut slot, doc.0, doc.1, "1,2,bbb", now),
            ChunkStep::Rejected(ChunkReject::TotalTooLarge)
        );
        assert!(slot.is_none());
    }

    /// **同一个文档才允许续传**：换了文档（同源换 URL / 换 origin）必须重新从 `seq=0` 开始，
    /// 旧文档的半截状态绝不能被新文档接着用（Task 13a 的 Minor 3 判据在分片通道上的延续）。
    #[test]
    fn a_new_document_cannot_continue_the_previous_assembly() {
        let now = std::time::Instant::now();
        let mut slot = None;
        accept_chunk(
            &mut slot,
            Some("http://a:1"),
            Some("http://a:1/one"),
            "0,2,aaa",
            now,
        );
        assert!(slot.is_some());

        // 同 origin、不同文档 → 第 1 片被拒（必须先有本轮的 0）
        assert_eq!(
            accept_chunk(
                &mut slot,
                Some("http://a:1"),
                Some("http://a:1/two"),
                "1,2,bbb",
                now
            ),
            ChunkStep::Rejected(ChunkReject::NoStart)
        );
        assert!(slot.is_none(), "换文档之后不得保留旧文档的半截状态");

        // 同 origin、不同文档，但从 0 开始 → 合法，且是新的一轮
        accept_chunk(
            &mut slot,
            Some("http://a:1"),
            Some("http://a:1/two"),
            "0,2,ccc",
            now,
        );
        let assembly = slot.as_ref().expect("新一轮");
        assert!(assembly.belongs_to(Some("http://a:1"), Some("http://a:1/two")));
        assert!(!assembly.belongs_to(Some("http://a:1"), Some("http://a:1/one")));

        // 换 origin（URL 恰好相同也一样）→ 必须重新开始
        assert_eq!(
            accept_chunk(
                &mut slot,
                Some("http://b:1"),
                Some("http://a:1/two"),
                "1,2,ddd",
                now
            ),
            ChunkStep::Rejected(ChunkReject::NoStart)
        );
        // 取不到文档身份（窗口刚销毁 / 还没导航完）时分片一律丢弃：这种组装**即使到齐也永远
        // 不会被 `get_page_report` 返回**（它的判据是 origin + URL 都对得上），所以不留内存。
        let mut slot = None;
        assert_eq!(
            accept_chunk(&mut slot, None, None, "0,2,aaa", now),
            ChunkStep::Rejected(ChunkReject::NoDocument)
        );
        assert!(slot.is_none());
        assert_eq!(
            accept_chunk(&mut slot, Some("http://a:1"), None, "0,2,aaa", now),
            ChunkStep::Rejected(ChunkReject::NoDocument),
            "只缺一侧身份同样不认（判据必须两侧都在）"
        );
        assert!(slot.is_none());
    }

    /// 组装出来的东西**仍然**要过单条通道的全部闸门：分片通道不是绕过 `validate` 的后门。
    #[test]
    fn assembled_payload_still_has_to_pass_validate() {
        let now = std::time::Instant::now();
        let mut slot = None;
        // 组装出一个「type 不在允许表内」的完整载荷
        let full = r#"{"type":"FNOS_PAGE_STATUS","items":[]}"#;
        let (a, b) = full.split_at(10);
        assert_eq!(
            accept_chunk(
                &mut slot,
                Some("http://a:1"),
                Some("http://a:1/x"),
                &format!("0,2,{a}"),
                now
            ),
            ChunkStep::Accepted { seq: 0, total: 2 }
        );
        match accept_chunk(
            &mut slot,
            Some("http://a:1"),
            Some("http://a:1/x"),
            &format!("1,2,{b}"),
            now,
        ) {
            ChunkStep::Complete(assembled) => {
                assert_eq!(assembled, full);
                assert_eq!(validate(&assembled), Err(Reject::TypeNotAllowed));
            }
            other => panic!("应当到齐：{other:?}"),
        }

        // 组装出一个**合法**的大载荷（模拟应用项列表）：到齐后必须过得了 validate。
        // 标题刻意只用 ASCII：这一条测的是宿主侧的重装，切点落在字符边界内是页面侧的责任
        // （`shim.js` 按**码点**切，`tests/shim.test.mjs` 用中文标题锁住那一条）。
        let mut slot = None;
        let items: Vec<String> = (0..80)
            .map(|i| format!(r#"{{"key":"/app/{i}/icon_1.png","title":"app-{i}","iconSrc":""}}"#))
            .collect();
        let big = format!(
            r#"{{"type":"FNOS_GET_LAUNCHPAD_APP_ITEMS","dir":"response","payload":{{"items":[{}],"titles":[]}}}}"#,
            items.join(",")
        );
        assert!(big.len() > 4000, "必须真的超过单条通道：{}", big.len());
        assert!(big.len() < MAX_CHUNK_TOTAL_BYTES);
        let per = MAX_CHUNK_BODY_BYTES;
        let parts: Vec<&str> = big
            .as_bytes()
            .chunks(per)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect();
        assert!(
            parts.iter().all(|p| !p.is_empty()),
            "切点必须落在字符边界上"
        );
        let total = parts.len();
        assert!(
            (2..=MAX_CHUNKS).contains(&total),
            "这个载荷必须需要多片：{total}"
        );
        for (i, part) in parts.iter().enumerate() {
            let step = accept_chunk(
                &mut slot,
                Some("http://a:1"),
                Some("http://a:1/x"),
                &format!("{i},{total},{part}"),
                now,
            );
            if i + 1 == total {
                match step {
                    ChunkStep::Complete(assembled) => {
                        assert_eq!(assembled, big, "分片重装必须逐字节还原");
                        let value = validate(&assembled).expect("合法的大载荷必须通过");
                        assert_eq!(value["payload"]["items"].as_array().unwrap().len(), 80);
                    }
                    other => panic!("应当到齐：{other:?}"),
                }
            } else {
                assert_eq!(step, ChunkStep::Accepted { seq: i, total });
            }
        }
        assert!(slot.is_none());
    }

    /// **T13b 审计发现的核心用例**（fix round 1 / Minor 4 起覆盖**两个** type）。
    ///
    /// 分流错了的两种表现都很坏：漏分流 = T13a 的状态条强态被自己的列表拉取冲掉（功能回归）；
    /// 分错方向 = 状态槽收到一条永远升不了级的应用项上报。请求（`dir:'out'`）与应答
    /// （`dir:'response'`）**都**算应用项通道——`dir` 只是给 UI 区分文案用的。
    ///
    /// `FNOS_GET_LAUNCHPAD_APP_TITLES` 与 `FNOS_GET_LAUNCHPAD_APP_ITEMS` **同属**这条通道：
    /// 上游在同一个 `if` 里用同一个 `sendResponse({items, titles})` 应答两者
    /// （`content-script.js:2853-2868`），把它们分开处理只会凭空造出「titles 应答冲掉状态条」
    /// 这条回路（Minor 4）。
    #[test]
    fn only_the_app_items_types_are_routed_to_the_app_items_slot() {
        for v in [
            json!({ "type": "FNOS_GET_LAUNCHPAD_APP_ITEMS", "dir": "out", "payload": {} }),
            json!({
                "type": "FNOS_GET_LAUNCHPAD_APP_ITEMS",
                "dir": "response",
                "payload": { "items": [], "titles": [] }
            }),
            json!({ "type": "FNOS_GET_LAUNCHPAD_APP_TITLES", "dir": "out", "payload": {} }),
            json!({
                "type": "FNOS_GET_LAUNCHPAD_APP_TITLES",
                "dir": "response",
                "payload": { "items": [{ "key": "/a/icon_1.png", "title": "x" }], "titles": ["x"] }
            }),
        ] {
            assert!(is_app_items_report(&v), "必须分流到应用项槽：{v}");
        }
        for v in [
            json!({ "type": "FNOS_INJECTION_TRIGGERED", "dir": "out" }),
            json!({ "type": "FNOS_APPLY", "dir": "response" }),
            json!({ "type": "FNOS_CHECK" }),
            json!({ "dir": "out" }),
            json!({ "type": 7 }),
            json!(null),
            json!("FNOS_GET_LAUNCHPAD_APP_ITEMS"),
            json!({ "type": "fnos_get_launchpad_app_items" }),
        ] {
            assert!(
                !is_app_items_report(&v),
                "必须留在状态槽（状态条的强态判据靠它）：{v}"
            );
        }
        // 分流出来的 type 必须在允许表内，否则它永远过不了 `validate`，分流本身没有意义
        for ty in APP_ITEMS_TYPES {
            assert!(
                REPORT_TYPES.contains(&ty),
                "{ty} 必须在 REPORT_TYPES 允许表内"
            );
        }
        // 分流集合**恰好**是这两个（多一个就是死条目，少一个就有上文那条回归）
        assert_eq!(
            APP_ITEMS_TYPES,
            [APP_ITEMS_TYPE, APP_ITEMS_TITLES_TYPE],
            "分流集合只允许这两个 type"
        );
        // 上游真正会回的那条应答（`{items,titles}`）确实能被分流（形状与 content-script.js 一致）
        let resp = json!({
            "type": "FNOS_GET_LAUNCHPAD_APP_ITEMS",
            "dir": "response",
            "payload": { "items": [{ "key": "/a/icon_1.png", "title": "x", "iconSrc": "" }], "titles": ["x"] }
        });
        assert!(validate(&resp.to_string()).is_ok());
        assert!(is_app_items_report(&resp));
    }
}
