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
/// 刻意**不**在这里深挖载荷结构（`dir` / `payload` 的形状由消费方 `ui/settings/status.js`
/// 判定）：宿主这一层只回答「这是不是一个我们能安全放进内存、并在设置窗里读出来的东西」。
/// 越靠前卡死，后面能戳到的东西越少：长度 → JSON → 对象 → `type` 白名单。
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

/// 最近一次被接受的上报，外加「它来自哪个文档」。
///
/// 记来源是为了诚实性：主窗口可以在**不改任何配置**的情况下导航，旧页面上报过的「已注入」
/// 不该被拿来描述新页面。判据是 origin 相同（注入与否在上游是按 origin 判定的：白名单是
/// origin 列表，`content-script.js:2947`）。同源 SPA 跳转不重新加载文档、注入仍然生效，
/// 因此同源保留是**正确**的，不是将就。
#[derive(Debug, Clone)]
pub struct ReportEntry {
    pub value: Value,
    /// 上报时主窗口文档的 origin（`config::origin_of`；解析不出来时 `None`）。
    pub origin: Option<String>,
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
}
