//! release 版日志落盘（P0-1）。
//!
//! ## 为什么需要它
//!
//! `main.rs` 的 `windows_subsystem = "windows"`（R9）让安装版**没有控制台**，于是全仓
//! `eprintln!("[fnos] …")` 在 NSIS 装出来的那份二进制里全部**掉进无底洞**。而这些日志恰好是
//! 白屏 / 加载失败 / `window.open` 裁定 / 标签创建失败 / 更新失败这些场景的唯一现场证据：
//! 装的是 release 版，用户（往往是作者自己在另一台机器上）能看到的现象只有一个画面，没有任何
//! 可供回传的文本。此前唯一的办法是换装 debug 版——等于把已经写好的一整条观测链作废。
//!
//! ## 做法
//!
//! [`emit`] 同时做两件事：**照旧写 stderr**（debug 版的控制台行为一个字节都不变）与**追加写
//! `config_dir()/app.log`**。调用点统一走 [`flog!`] 宏，因此「新增一条日志」不需要在两条通道
//! 之间做选择，也不会出现「debug 有、release 没有」的二义性。
//!
//! ## 边界（故意保守）
//!
//! - **绝不 panic、绝不阻塞调用方**：所有 IO 错误一律 `let _ =` 吞掉。排障设施自身把进程搞崩
//!   是最坏的一种失败。
//! - **单文件上限 [`MAX_LOG_BYTES`]**：超了就把旧的轮转成 `app.log.1`（覆盖上一份），再从空的
//!   `app.log` 写起。所以磁盘占用恒为 ≤ 2 × 上限，长期常驻也不会把用户盘写满。
//! - **可关**：`FNOS_LOG=0` 完全关闭落盘（stderr 不受影响）。缺省**开**——这个项目的排障
//!   诉求远大于「一个自用工具多写几 KB」的成本。
//! - 时间戳用 **UTC**（`…Z` 后缀，不带时区猜测）：运行时不引入 `chrono`/`time` 依赖，也不去
//!   读注册表时区。日志用于「按顺序还原发生了什么」，UTC 单调可读即可。
//!
//! ## 隐私
//!
//! 日志里出现的一切文本都来自既有的调用点，而那些点在写入前已经过
//! `report::log_safe` / `report::accepted_log_line` 收口（页面可控文本不可能伪造换行）。
//! 本模块**只负责搬运**，不做也不该做二次清洗。

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// 单个日志文件的字节上限（4 MiB）。轮转后磁盘占用 ≤ 8 MiB。
pub const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// 日志文件名（落在配置目录下，与 `config.json` 同级，便于「打开配置目录」直接取走）。
pub const LOG_FILE_NAME: &str = "app.log";

/// 落盘状态。`None` 表示**不落盘**（被 `FNOS_LOG=0` 关掉，或首次打开文件失败）。
struct Sink {
    path: PathBuf,
    file: Option<std::fs::File>,
    /// 已写字节数（用于轮转判定；不依赖 `metadata`，省一次系统调用）。
    written: u64,
}

static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();

/// 是否启用落盘。缺省启用；`FNOS_LOG=0` / `false` / `off` 关闭。
///
/// 抽成纯函数是为了能单测「哪些取值算关」，不必真的去写用户目录。
fn logging_enabled_from(raw: Option<&str>) -> bool {
    match raw {
        None => true,
        Some(v) => {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        }
    }
}

fn sink() -> &'static Mutex<Option<Sink>> {
    SINK.get_or_init(|| {
        // 测试进程**不落盘**：单测里会跑到生产代码的日志点（读壁纸失败、注入载荷组装……），
        // 那些日志不该在开发者/CI 机器的 `%APPDATA%\com.fnos.desktop\app.log` 里留痕——
        // 既污染真实排障日志，也让测试产生文件副作用。stderr 照旧（`cargo test -- --nocapture`
        // 仍能看到同一行）。
        if cfg!(test) {
            return Mutex::new(None);
        }
        let enabled = logging_enabled_from(std::env::var("FNOS_LOG").ok().as_deref());
        if !enabled {
            return Mutex::new(None);
        }
        let path = crate::paths::config_dir().join(LOG_FILE_NAME);
        Mutex::new(Some(Sink {
            path,
            // 惰性打开：第一次真的写日志时才碰磁盘（首启路径不该因为排障设施多一次 IO）
            file: None,
            written: 0,
        }))
    })
}

/// 打开（必要时轮转）日志文件。任何一步失败都返回 `None` 并**不再重试**——排障设施
/// 不能因为磁盘满 / 无权限而在每条日志上重试一次。
fn open_for_append(sink: &mut Sink) -> Option<std::fs::File> {
    if let Some(dir) = sink.path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 已有文件且超过上限 → 轮转成 `.1`（覆盖上一份）。轮转失败就退化成「继续追加」：
    // 写日志比维持仪式感重要。
    if let Ok(meta) = std::fs::metadata(&sink.path) {
        if meta.len() >= MAX_LOG_BYTES {
            let mut old = sink.path.clone();
            old.set_file_name(format!("{LOG_FILE_NAME}.1"));
            let _ = std::fs::rename(&sink.path, &old);
        }
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sink.path)
        .ok()?;
    sink.written = std::fs::metadata(&sink.path).map(|m| m.len()).unwrap_or(0);
    Some(file)
}

/// 写一行日志（**不含**换行；本函数负责补 `\n`）。
///
/// 调用方一律经 [`flog!`] 进来，不要直接调。
pub fn emit(line: &str) {
    // 通道①：stderr（debug 版控制台）。release 下没有控制台，写进去也只是丢掉——但它
    // 足够便宜，且能让「同一个二进制在 debug/release 下行为一致」这件事不出现分支。
    eprintln!("{line}");

    // 通道②：落盘。任何异常都静默（见模块头「边界」）。
    let Ok(mut guard) = sink().lock() else {
        return;
    };
    let Some(sink) = guard.as_mut() else {
        return;
    };
    if sink.file.is_none() {
        sink.file = open_for_append(sink);
        if sink.file.is_none() {
            return;
        }
    }
    // 单条上限：页面可控文本理论上已被上游收口，这里是纵深防御——一条超长行不该
    // 独占整个轮转配额。
    let mut text = String::with_capacity(line.len() + 32);
    text.push_str(&timestamp_utc(SystemTimeSeconds::now()));
    text.push(' ');
    for c in line.chars().take(MAX_LINE_CHARS) {
        // 换行/回车折成空格：日志行是「按行读」的，一条日志绝不能变成两条。
        text.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
    text.push('\n');
    if let Some(file) = sink.file.as_mut() {
        if file.write_all(text.as_bytes()).is_err() {
            // 写失败（磁盘满 / 文件被独占）→ 放弃这个 sink，后续日志只走 stderr
            sink.file = None;
            return;
        }
        sink.written += text.len() as u64;
        let _ = file.flush();
    }
    if sink.written >= MAX_LOG_BYTES {
        // 达到上限：先关掉，下一次写入时由 open_for_append 轮转
        sink.file = None;
    }
}

/// 单条日志的字符上限（防止一条超长文本吃掉整个轮转配额）。
const MAX_LINE_CHARS: usize = 8192;

/// 极简 UTC 时间源。`SystemTime` 秒数 → 日历日期用 Howard Hinnant 的 civil_from_days
/// 算法（无依赖、无时区、无 panic 面）。
struct SystemTimeSeconds {
    secs: i64,
    millis: u32,
}

impl SystemTimeSeconds {
    fn now() -> Self {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => Self {
                secs: d.as_secs() as i64,
                millis: d.subsec_millis(),
            },
            // 系统时钟早于 1970（不该发生）：退回 0，日志仍可用
            Err(_) => Self { secs: 0, millis: 0 },
        }
    }
}

/// `YYYY-MM-DD HH:MM:SS.mmmZ`。
fn timestamp_utc(t: SystemTimeSeconds) -> String {
    let days = t.secs.div_euclid(86_400);
    let rem = t.secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!(
        "{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}.{ms:03}Z",
        ms = t.millis
    )
}

/// 1970-01-01 起的天数 → (年, 月, 日)。Howard Hinnant `civil_from_days`。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + i64::from(m <= 2), m as u32, d as u32)
}

/// 写一条 `[fnos]` 日志（stderr + `app.log`）。
///
/// 语法与 `eprintln!` 完全一致，只是多了一路落盘。**新增日志一律用它**，不要用
/// `eprintln!`——否则 release 版又会少一条现场证据。
macro_rules! flog {
    ($($arg:tt)*) => {{
        crate::logfile::emit(&format!($($arg)*));
    }};
}

pub(crate) use flog;

#[cfg(test)]
mod tests {
    use super::*;

    /// 缺省开；只有明确的关值才算关。任何「看起来像关」的模糊取值都保持开——
    /// 排障设施宁多写，不可静默失效。
    #[test]
    fn logging_is_enabled_unless_explicitly_disabled() {
        assert!(logging_enabled_from(None));
        assert!(logging_enabled_from(Some("")));
        assert!(logging_enabled_from(Some("1")));
        assert!(logging_enabled_from(Some("true")));
        assert!(logging_enabled_from(Some("yes")));
        assert!(!logging_enabled_from(Some("0")));
        assert!(!logging_enabled_from(Some(" 0 ")));
        assert!(!logging_enabled_from(Some("false")));
        assert!(!logging_enabled_from(Some("OFF")));
    }

    /// 时间戳格式固定为 `YYYY-MM-DD HH:MM:SS.mmmZ`，且跨日/闰年边界都算对
    /// （日志要能按时间排序，格式一歪就没法读了）。
    #[test]
    fn timestamps_are_fixed_width_utc() {
        let epoch = timestamp_utc(SystemTimeSeconds { secs: 0, millis: 0 });
        assert_eq!(epoch, "1970-01-01 00:00:00.000Z");
        // 2026-11-16T14:23:26.789Z（本文件写作时的时间点，用作已知答案）
        let t = timestamp_utc(SystemTimeSeconds {
            secs: 1_794_839_006,
            millis: 789,
        });
        assert_eq!(t, "2026-11-16 14:23:26.789Z");
        // 闰日：2024-02-29T00:00:00Z
        assert_eq!(
            timestamp_utc(SystemTimeSeconds {
                secs: 1_709_164_800,
                millis: 0
            }),
            "2024-02-29 00:00:00.000Z"
        );
        // 定宽：任何时刻都是 24 个字符，便于 `sort`/`grep` 对齐
        assert_eq!(t.len(), 24);
    }

    /// `civil_from_days` 是纯函数，直接钉住几个已知点（含 1970 之前的负数天）。
    #[test]
    fn civil_dates_match_known_answers() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(365), (1971, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
