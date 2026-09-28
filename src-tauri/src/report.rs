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
}
