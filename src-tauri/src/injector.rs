//! 纯函数：把配置与 vendored 资源拼成一段 initialization_script。
//! 不碰 Tauri API —— 便于单测与快照锁定。
//!
//! 唯一的文件 IO 是**登录壁纸**（Task 13b）：`build_init_script` 会把
//! `local.loginWallpaperFileName` 指的文件从配置目录读进内存。为了让测试保持封闭，
//! 读文件被隔离在 [`load_wallpaper_from`]（显式传目录），而纯组装逻辑在
//! [`build_init_script_with`]（接收已读好的壁纸）——单测只调后者。

use crate::base64;
use crate::config::{self, Config};
use crate::paths;
use crate::report;
use serde::Serialize;
use serde_json::{json, Value};

pub const SHELL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MODS_COMMIT: &str = "483c3e2e217faebc1be45b4e824865854a61e3dd";
/// 上游 fnOS_UI_Mods 的版本号（R38）：`injector` 的载荷与 `commands::ConfigView.meta`
/// 共用同一个常量，避免字面量在两处漂移。
pub const MODS_VERSION: &str = "1.0.2";

/// 登录壁纸的大小上限（与 `config::MAX_WALLPAPER_BYTES` 同一个数）——超限时跳过并记一行日志。
pub const MAX_WALLPAPER_BYTES: usize = config::MAX_WALLPAPER_BYTES;

const SHIM_JS: &str = include_str!("../inject/shim.js");
const BOOTSTRAP_JS: &str = include_str!("../inject/bootstrap.js");
const DOCK_JS: &str = include_str!("../inject/dock.js");
const CONTENT_SCRIPT_JS: &str = include_str!("../assets/fnos-mods/content-script.js");

struct Assets {
    files: &'static [(&'static str, &'static str)],
}

/// 14 个「完美图标」PNG（Task 13b）：`(binaryAssets 的键, 原始字节)`。
///
/// **键一律小写**，因为 shim 的查表键是小写化的（`inject/shim.js` 的
/// `assetIndex[String(k).toLowerCase()]`），而磁盘上的文件名是 camelCase
/// （`prefect_icon/panIndex.png`）——写 camelCase 的键本身也能查到（两边都小写化后相等），
/// 但让载荷里的键与「上游 `launchpadIconRedrawMap` 的规范写法」一致更不容易出错：
/// `prefect_icon/<小写名>.png` 正是 `icon-map.json` 的取值形态。
///
/// 表必须与 vendored 目录**集合相等**：`tests::icon_table_matches_the_vendored_directory`
/// 真的去读目录，双向比对（少一个 = 图标不可用，多一个 = 指向不存在的文件）。
pub(crate) fn prefect_icons() -> &'static [(&'static str, &'static [u8])] {
    &[
        (
            "prefect_icon/alist.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/alist.png"),
        ),
        (
            "prefect_icon/emby.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/emby.png"),
        ),
        (
            "prefect_icon/home-assistant.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/home-assistant.png"),
        ),
        (
            "prefect_icon/icloud.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/icloud.png"),
        ),
        (
            "prefect_icon/it-tools.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/it-tools.png"),
        ),
        (
            "prefect_icon/kodi.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/kodi.png"),
        ),
        (
            "prefect_icon/one-panel.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/one-panel.png"),
        ),
        (
            "prefect_icon/oray-hsk.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/oray-hsk.png"),
        ),
        (
            // 磁盘上是 camelCase 的 `panIndex.png`（icon-map.json 里的规范名是 `panindex`）
            "prefect_icon/panindex.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/panIndex.png"),
        ),
        (
            "prefect_icon/qbittorrent.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/qbittorrent.png"),
        ),
        (
            "prefect_icon/quarkpan.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/quarkpan.png"),
        ),
        (
            "prefect_icon/syncthing.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/syncthing.png"),
        ),
        (
            "prefect_icon/transmission.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/transmission.png"),
        ),
        (
            "prefect_icon/xunlei.png",
            include_bytes!("../assets/fnos-mods/prefect_icon/xunlei.png"),
        ),
    ]
}

/// 完美图标是否**真的配置了**——这是「要不要把 1.1 MiB 的图标资源塞进初始化脚本」的唯一判据。
///
/// 三个键里任何一个非默认就意味着用户开了这项功能：
/// - `launchpadIconScaleEnabled`：总开关（上游 `setLaunchpadIconScaleOnDom(enabled)` 的第一道门槛）；
/// - `launchpadIconRedrawMap`：逐项重绘的映射（`redrawKeys` 必须能在它里面查到值才生效，
///   所以只看 map 就够）；
/// - `launchpadIconScaleSelectedKeys`：逐项缩放的选中项（上游在总开关关闭时也会归一化它，
///   说明它本身可以独立于开关被写下来）。
///
/// **为什么这个判据必须两侧一致**：`commands::set_config` 用它决定 `needsReload`
/// （载荷内容变了就必须重建窗口），`build_init_script` 用它决定要不要嵌入图标。
/// 两处一旦分叉，用户会看到「开关打开了但页面没有任何图标」这种最难排障的状态。
pub fn perfect_icon_enabled(cfg: &Config) -> bool {
    cfg.mods.launchpad_icon_scale_enabled
        || !cfg.mods.launchpad_icon_redraw_map.is_empty()
        || !cfg.mods.launchpad_icon_scale_selected_keys.is_empty()
}

/// 一份已读进内存的登录壁纸。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperAsset {
    /// `binaryAssets` 的键 = **小写**文件名（shim 的查表键小写化）。
    pub key: String,
    /// 由扩展名推导的 mime（`config::wallpaper_mime`）。
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

impl WallpaperAsset {
    /// 完整的 `data:<mime>;base64,…`。
    ///
    /// shim 的 `getURL` 对 `binaryAssets` 里的值**原样透传**以 `data:` 开头者（其余按裸 base64
    /// 包装）——所以 mime 在这里就定下来，不依赖 shim 的扩展名表。
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, base64::encode(&self.bytes))
    }
}

/// 「读文件之前」这道门的结论（纯函数的返回值，见 [`precheck_wallpaper_len`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Precheck {
    /// 长度与文件类型都可以读。
    Readable,
    /// 不是一个普通文件（目录 / 设备 / 命名管道……）。
    NotAFile,
    /// 超过 [`MAX_WALLPAPER_BYTES`]。
    TooLarge,
    /// 长度为 0。
    Empty,
}

/// **读文件之前**的准入判定：只吃 `metadata` 就能拿到的两个事实（长度、是不是普通文件）。
///
/// ## 为什么这道门必须在 `fs::read` **之前**（Review fix round 1 / Important）
///
/// 壁纸的文件名是**用户可编辑**的配置值，而设置窗明确提供「打开配置目录」——所以配置目录里
/// 完全可能被放进（或软链到）一个巨大的文件。旧实现先 `std::fs::read` 再比
/// `bytes.len() > MAX_WALLPAPER_BYTES`，于是超限文件会被**整份分配**进内存：
/// 需求承诺的是「记一行日志、优雅跳过」，而分配失败是 abort（整个进程直接死）。
/// `fs::metadata` 只看目录项，是 O(1) 的；长度已知之后，超限文件一个字节都不会被读。
///
/// `is_file` 同样在这里判：目录/设备也能通过 `metadata`，但 `read` 它们要么失败要么没有意义。
/// 注意 `fs::metadata` **跟随软链**（等价于 `stat`），所以「软链到一个超大的普通文件」也会
/// 在这里被长度挡住——这正是不希望把链接目标整份读进来的场景。
///
/// 抽成纯函数是为了让单测直接驱动边界值（0 / 恰好上限 / 上限 +1 / 非普通文件），
/// 不必每次都造 8 MiB 的真文件；`precheck_rejects_oversize_empty_and_non_files` 驱动边界，
/// `load_wallpaper_skips_missing_oversized_and_illegal_files` 用**真文件** + 读数计数器佐证。
fn precheck_wallpaper_len(len: u64, is_file: bool) -> Precheck {
    if !is_file {
        return Precheck::NotAFile;
    }
    if len > MAX_WALLPAPER_BYTES as u64 {
        return Precheck::TooLarge;
    }
    if len == 0 {
        return Precheck::Empty;
    }
    Precheck::Readable
}

// 测试用：**本线程**对壁纸文件做系统调用的次数（两个计数器分开）。
//
// 存在的唯一目的是让两条性质成为**可断言的事实**，而不是只断言「结果是空的」：
// ① 「超限时一次字节都没读」= `stat > 0` 而 `read == 0`；
// ② 「注入关闭时一次文件 IO 都没做」= 两个都是 0。
// 用 thread-local 而不是全局原子量：cargo 的测试线程池会并行跑用例，全局计数会互相干扰。
// （写成 `//` 而不是 `///`：下面是宏调用，rustdoc 不给宏展开生成文档，`///` 会触发
//  unused_doc_comments —— 而本项目的门禁要求 0 warning。）
#[cfg(test)]
thread_local! {
    static WALLPAPER_STAT_ATTEMPTS: std::cell::Cell<usize> = std::cell::Cell::new(0);
    static WALLPAPER_READ_ATTEMPTS: std::cell::Cell<usize> = std::cell::Cell::new(0);
}

#[cfg(test)]
fn wallpaper_stat_attempts() -> usize {
    WALLPAPER_STAT_ATTEMPTS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn wallpaper_read_attempts() -> usize {
    WALLPAPER_READ_ATTEMPTS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_wallpaper_io_counters() {
    WALLPAPER_STAT_ATTEMPTS.with(|c| c.set(0));
    WALLPAPER_READ_ATTEMPTS.with(|c| c.set(0));
}

/// 生产代码里没有任何调用点（`#[cfg(test)]`）——见上面两个计数器。
#[cfg(test)]
fn note_wallpaper_stat_attempt() {
    WALLPAPER_STAT_ATTEMPTS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn note_wallpaper_read_attempt() {
    WALLPAPER_READ_ATTEMPTS.with(|c| c.set(c.get() + 1));
}

/// 从 `dir` 读登录壁纸；任何一步不满足都返回 `None` 并记一行日志（**永不 panic**）。
///
/// 校验顺序：配置里有名字 → 名字形状与扩展名合法（[`config::wallpaper_mime`]）→
/// **`metadata` 预检（普通文件 / 非空 / 不超上限，见 [`precheck_wallpaper_len`]）** → 读文件 →
/// 读完之后再核一次大小（TOCTOU：预检与读之间文件可能被换掉或长大；这一步只保证**结果**正确，
/// 「不整份分配」由预检保证）。
///
/// 之所以把形状判定也放在读文件之前：不合法时连 `Path::join` 都不做（名字里不可能有分隔符，
/// 但「先不信、先判」比「先拼路径」更容易审计）。
///
/// 日志里的名字都过 `[report::log_safe]`（形状判定已经拒掉控制字符，这里是纵深防御），
/// 页面/用户可控文本不可能在 stderr 里伪造出换行。
fn load_wallpaper_from(dir: &std::path::Path, cfg: &Config) -> Option<WallpaperAsset> {
    let raw = cfg.local.login_wallpaper_file_name.as_deref()?;
    let name = raw.trim();
    if name.is_empty() {
        return None;
    }
    let Some(mime) = config::wallpaper_mime(name) else {
        eprintln!(
            "[fnos] 登录壁纸文件名不可用（已跳过，未读文件）：{}",
            report::log_safe(name)
        );
        return None;
    };
    let path = dir.join(name);
    #[cfg(test)]
    note_wallpaper_stat_attempt();
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "[fnos] 登录壁纸读取失败（已跳过）：{} — {e}",
                report::log_safe(name)
            );
            return None;
        }
    };
    match precheck_wallpaper_len(meta.len(), meta.is_file()) {
        Precheck::Readable => {}
        Precheck::NotAFile => {
            eprintln!(
                "[fnos] 登录壁纸不是一个普通文件（已跳过，未读文件）：{}",
                report::log_safe(name)
            );
            return None;
        }
        Precheck::TooLarge => {
            eprintln!(
                "[fnos] 登录壁纸超过 {} MiB 上限（已跳过，未读文件）：{} 实为 {} 字节",
                MAX_WALLPAPER_BYTES / 1024 / 1024,
                report::log_safe(name),
                meta.len()
            );
            return None;
        }
        Precheck::Empty => {
            eprintln!(
                "[fnos] 登录壁纸是空文件（已跳过，未读文件）：{}",
                report::log_safe(name)
            );
            return None;
        }
    }
    #[cfg(test)]
    note_wallpaper_read_attempt();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "[fnos] 登录壁纸读取失败（已跳过）：{} — {e}",
                report::log_safe(name)
            );
            return None;
        }
    };
    // TOCTOU：预检之后文件可能被换掉/长大（预检只管「不因为一个本来就超限的文件去分配内存」）。
    if bytes.is_empty() || bytes.len() > MAX_WALLPAPER_BYTES {
        eprintln!(
            "[fnos] 登录壁纸在预检之后变了（已跳过）：{} 实读 {} 字节",
            report::log_safe(name),
            bytes.len()
        );
        return None;
    }
    Some(WallpaperAsset {
        key: name.to_ascii_lowercase(),
        mime,
        bytes,
    })
}

/// [`load_wallpaper_from`] 的生产入口：目录就是配置文件所在目录。
fn load_wallpaper(cfg: &Config) -> Option<WallpaperAsset> {
    load_wallpaper_from(&paths::config_dir(), cfg)
}

// R34：消费方读 camelCase —— bootstrap.js:27 读 `meta.shellVersion`、shim.js:143 读 `meta.modsVersion`。
// 缺 rename_all 会序列化成 snake_case，两处都静默回落 '0.0.0'（有测试锁定）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    shell_version: &'static str,
    mods_commit: &'static str,
    mods_version: &'static str,
}

fn assets() -> Assets {
    Assets {
        files: &[
            (
                "basic_mod.css",
                include_str!("../assets/fnos-mods/basic_mod.css"),
            ),
            (
                "windows_titlebar_mod.css",
                include_str!("../assets/fnos-mods/windows_titlebar_mod.css"),
            ),
            (
                "mac_titlebar_mod.css",
                include_str!("../assets/fnos-mods/mac_titlebar_mod.css"),
            ),
            (
                "classic_launchpad_mod.css",
                include_str!("../assets/fnos-mods/classic_launchpad_mod.css"),
            ),
            (
                "spotlight_launchpad_mod.css",
                include_str!("../assets/fnos-mods/spotlight_launchpad_mod.css"),
            ),
            (
                "desktop_icon_mod.css",
                include_str!("../assets/fnos-mods/desktop_icon_mod.css"),
            ),
            (
                "lockscreen_mod.css",
                include_str!("../assets/fnos-mods/lockscreen_mod.css"),
            ),
            ("mod.js", include_str!("../assets/fnos-mods/mod.js")),
            (
                "prefect_icon/icon-map.json",
                include_str!("../assets/fnos-mods/prefect_icon/icon-map.json"),
            ),
        ],
    }
}

/// 把上游 `content-script.js` 包进「等到有 `documentElement` 再执行」的壳（**源码一个字节不改**）。
///
/// 为什么必须有这层壳（本轮探针实测）：tauri 的 `initialization_script` 走 WebView2 的
/// `AddScriptToExecuteOnDocumentCreated`，执行时刻比 Chrome 扩展的 document_start 还早 ——
/// 此刻 `document.head` 与 `document.documentElement` **都还是 null**。上游第一件事就是
/// `(document.head || document.documentElement).appendChild(link)`（cs:2618 / 2658 / 2675），
/// 于是 `startInject()` 一进门就抛 `TypeError: Cannot read properties of null (reading 'appendChild')`
/// （异步回调里抛出 ⇒ unhandled rejection），**整条注入链再也不跑**：实测
/// `#fnos-ui-mods-basic-style` / `#fnos-ui-mods-script` 在启用注入时也永远不出现，
/// 只有 `chrome.storage.onChanged` 那条支路（`updateBrandColor` → `getThemeStyleElement`）
/// 因为发生在配置推送时（此时 DOM 已就绪）才会建出 `#fnos-ui-mods-theme-style`。
///
/// 壳的语义：
/// - `documentElement` 已存在 → 立即执行（与旧行为一致）；
/// - 否则用 `MutationObserver` 观察 **`document` 本身**，解析器创建 `<html>` 时触发；
///   这里只要求「执行时 `documentElement` 存在」，**不**声称比页面自己的脚本更早 ——
///   一次 mutation 回调是微任务，完全可能被解析器已经执行过的 `<head>` 脚本排在后面；
/// - MutationObserver 不可用 / 已经过了 loading 阶段 → `DOMContentLoaded` 兜底；
/// - 上游抛错时记一笔 `window.__FNOS_UPSTREAM_ERROR__` 便于现场排查。
fn wrap_upstream(content: &str) -> String {
    format!(
        "(function () {{\n\
         var run = function () {{\n{content}\n}};\n\
         var started = false;\n\
         function start() {{\n\
           if (started) return;\n\
           started = true;\n\
           try {{ run(); }} catch (e) {{ window.__FNOS_UPSTREAM_ERROR__ = String((e && e.message) || e); }}\n\
         }}\n\
         if (document.documentElement) {{ start(); return; }}\n\
         var mo = null;\n\
         if (typeof MutationObserver === 'function') {{\n\
           try {{\n\
             mo = new MutationObserver(function () {{\n\
               if (!document.documentElement) return;\n\
               mo.disconnect();\n\
               start();\n\
             }});\n\
             mo.observe(document, {{ childList: true, subtree: true }});\n\
           }} catch (e) {{ mo = null; }}\n\
         }}\n\
         document.addEventListener('DOMContentLoaded', start);\n\
         if (document.readyState !== 'loading') start();\n\
         }})();\n"
    )
}

/// 生产入口：配置 → initialization_script。
///
/// **注入关闭时在读壁纸之前就返回**（Review fix round 1 / Minor 1）：早先的写法是
/// `build_init_script_with(cfg, load_wallpaper(cfg))` —— 参数先求值，于是哪怕
/// `inject_enabled == false`（脚本最终一定是空的），也会去配置目录读一次壁纸，
/// 配置里写了个不存在/超大/非法的名字时还会打出一行「登录壁纸读取失败（已跳过）」这种
/// 与用户实际状态无关的噪声。现在关态一次文件 IO 都不做。
pub fn build_init_script(cfg: &Config) -> String {
    if !cfg.shell.inject_enabled {
        return String::new();
    }
    build_init_script_with(cfg, load_wallpaper(cfg))
}

/// [`build_init_script`] 的**纯**内核：壁纸由调用方先读好（生产走 [`load_wallpaper`]，
/// 单测直接喂一个 [`WallpaperAsset`] 或 `None`）。这样「读文件」不会污染单测的封闭性。
///
/// ## `binaryAssets` 的两条规则（Task 13b）
///
/// 1. **按需**：只有 [`perfect_icon_enabled`] 为真时才嵌入 14 个图标；否则整个键**不出现**
///    （不是空对象）——默认载荷必须与 T13a 逐字节一致，否则每次建窗都白白多 1.1 MiB。
/// 2. **键小写**：shim 的 `assetIndex` 用小写键（`shim.js` 的 `String(k).toLowerCase()`），
///    而磁盘上的文件名有 camelCase（`panIndex.png`）。载荷里统一成小写，
///    与上游 `launchpadIconRedrawMap` 的规范写法（`prefect_icon/panindex.png`）也一致。
pub fn build_init_script_with(cfg: &Config, wallpaper: Option<WallpaperAsset>) -> String {
    if !cfg.shell.inject_enabled {
        return String::new();
    }

    // 文本资源（CSS / mod.js / icon-map.json）：`binaryAssets` 专属二进制，不进这里。
    let mut asset_map = serde_json::Map::new();
    for (name, text) in assets().files {
        asset_map.insert((*name).to_string(), json!(text));
    }

    // 二进制资源：图标按需 + 壁纸有则带上。值为**完整** data URL（shim 对 `data:` 前缀原样透传）。
    let mut binary_map = serde_json::Map::new();
    if perfect_icon_enabled(cfg) {
        for (key, bytes) in prefect_icons() {
            binary_map.insert(
                (*key).to_string(),
                json!(format!("data:image/png;base64,{}", base64::encode(bytes))),
            );
        }
    }
    if let Some(asset) = &wallpaper {
        binary_map.insert(asset.key.clone(), json!(asset.data_url()));
    }

    let mut payload = serde_json::Map::new();
    payload.insert(
        "meta".into(),
        json!(Meta {
            shell_version: SHELL_VERSION,
            mods_commit: MODS_COMMIT,
            mods_version: MODS_VERSION,
        }),
    );
    payload.insert("mods".into(), json!(&cfg.mods));
    payload.insert("local".into(), json!(&cfg.local));
    // shell 段**只发页面真正消费的键**（T14c）。`ShellConfig` 里的 homeUrl / nasUrl /
    // window 是宿主私有（页面没有理由拿到），所以这里手写一个窄对象，而不是把整个
    // ShellConfig 序列化下去。消费方：`inject/dock.js`（初始态）与 shim 转调的
    // `__FNOS_APPLY_SHELL__`（免刷新态，`commands::apply_to_page`）。
    payload.insert(
        "shell".into(),
        json!({ "dockAutoHide": cfg.shell.dock_auto_hide }),
    );
    payload.insert("assets".into(), Value::Object(asset_map));
    if !binary_map.is_empty() {
        payload.insert("binaryAssets".into(), Value::Object(binary_map));
    }
    let payload = Value::Object(payload);

    format!(
        "/* fnOS Desktop Shell init script v{ver} (mods {commit}) */\n\
         window.__FNOS_SHELL__ = {payload};\n\
         {shim}\n\
         {boot}\n\
         {dock}\n\
         {content}\n",
        ver = SHELL_VERSION,
        commit = MODS_COMMIT,
        payload = serde_json::to_string(&payload)
            .expect("payload is a serde Value and always serializes"),
        shim = SHIM_JS,
        boot = BOOTSTRAP_JS,
        dock = DOCK_JS,
        content = wrap_upstream(CONTENT_SCRIPT_JS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// 解析脚本开头的配置段：`window.__FNOS_SHELL__ = {…};` 里的那个对象。
    fn payload_json(script: &str) -> serde_json::Value {
        let start = script.find('{').expect("配置段起始 '{'");
        let end = script.find("};\n").expect("配置段结束 '};'");
        serde_json::from_str(&script[start..=end]).expect("配置段必须是合法 JSON")
    }

    /// 单测一律走**纯**内核：`build_init_script` 会去读真实配置目录里的壁纸，
    /// 那会让断言依赖开发者机器上的 `config.json`（不可重复）。
    fn script(cfg: &Config) -> String {
        build_init_script_with(cfg, None)
    }

    /// 与 `config.rs` 同一套「按 tag + pid 建独立临时目录」的做法（并行测试不互相污染）。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fnos-inj-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn binary_assets(v: &serde_json::Value) -> &serde_json::Map<String, serde_json::Value> {
        v.get("binaryAssets")
            .and_then(|b| b.as_object())
            .expect("载荷必须有 binaryAssets 对象")
    }

    #[test]
    fn script_has_five_sections_in_order() {
        let s = script(&Config::default());
        // 锚点必须带 ` = `：shim.js/bootstrap.js 里也有 `W.__FNOS_SHELL__`，只用
        // `__FNOS_SHELL__` 会让载荷整体挪到 shim 之下时本测试仍然通过。
        let i_cfg = s.find("window.__FNOS_SHELL__ = ").expect("config section");
        let i_shim = s.find("fnos-desktop-shell").expect("shim section");
        let i_boot = s.find("__FNOS_BOOTSTRAP__").expect("bootstrap section");
        // dock 段的锚点用它的 class 名：shim 对 `__FNOS_APPLY_SHELL__` 的**引用**在 shim
        // 段里就出现了，不能当 dock 段的锚点（否则 dock 挪到上游之后本测试仍绿）。
        let i_dock = s.find("fnos-shell-dock-autohide").expect("dock section");
        let i_cs = s.find("hasFnOSSignature").expect("upstream content-script");
        assert!(
            i_cfg < i_shim && i_shim < i_boot && i_boot < i_dock && i_dock < i_cs,
            "段落顺序必须是 配置→shim→bootstrap→dock→上游"
        );
    }

    #[test]
    fn shell_section_is_narrow_and_defaults_off() {
        // T14c：shell 段是**手写的窄对象**，只带页面消费的 dockAutoHide；把整个
        // ShellConfig 序列化下去会把宿主私有的 homeUrl / nasUrl / window 一并交给页面。
        let v = payload_json(&script(&Config::default()));
        let shell = v
            .get("shell")
            .and_then(|s| s.as_object())
            .expect("载荷必须有 shell 对象");
        assert_eq!(
            shell.get("dockAutoHide").and_then(|b| b.as_bool()),
            Some(false),
            "Dock 自动隐藏默认关（config.rs::ShellConfig::default）"
        );
        assert_eq!(shell.len(), 1, "shell 段只允许 dockAutoHide 一个键");
        assert!(
            v.get("homeUrl").is_none() && v.get("nasUrl").is_none() && v.get("window").is_none(),
            "宿主私有的 shell 字段不得出现在载荷顶层"
        );

        // 打开后经同一通道下发；camelCase 键名（dock.js 读 SHELL.shell.dockAutoHide）
        let mut on = Config::default();
        on.shell.dock_auto_hide = true;
        assert_eq!(
            payload_json(&script(&on))["shell"]["dockAutoHide"],
            json!(true)
        );
    }

    #[test]
    fn payload_carries_all_assets_and_mods_config() {
        let mut cfg = Config::default();
        cfg.mods.brand_color = "#123456".into();
        let s = script(&cfg);
        assert!(s.contains("\"brandColor\":\"#123456\""));

        // 必须断言**解析后**的 `assets` 对象。对整串做 `s.contains(name)` 恒真：7 个 CSS 名已由
        // bootstrap.js 的 CSS_FILES 列出（bootstrap.js:23），`mod.js` 出现在 shim.js/bootstrap.js，
        // `content-script.js` 只出现在 shim.js 的注释里（shim.js:2）。所以清空 `assets()`、或把某个
        // 键改名（`include_str!` 只校验路径，编译器从不校验键字符串）都不会让旧断言变红。
        let v = payload_json(&s);
        let assets = v
            .get("assets")
            .and_then(|a| a.as_object())
            .expect("载荷必须有 assets 对象");

        let mut got: Vec<&str> = assets.keys().map(String::as_str).collect();
        got.sort_unstable();
        let mut want = [
            "basic_mod.css",
            "windows_titlebar_mod.css",
            "mac_titlebar_mod.css",
            "classic_launchpad_mod.css",
            "spotlight_launchpad_mod.css",
            "desktop_icon_mod.css",
            "lockscreen_mod.css",
            "mod.js",
            "prefect_icon/icon-map.json",
        ];
        want.sort_unstable();
        assert_eq!(
            got.as_slice(),
            want.as_slice(),
            "assets 键集必须恰好是这 9 个文本资源"
        );
        assert_eq!(assets.len(), 9, "assets 条目数必须恰好是 9");
        assert!(
            !assets.contains_key("content-script.js"),
            "content-script.js 是末尾直接执行的上游脚本，不是 assets 条目"
        );
        for key in assets.keys() {
            assert!(
                !key.ends_with(".png"),
                "assets 只承载文本，不得含二进制键 {key}"
            );
        }
        // T13b 的门槛：默认配置（完美图标关闭、没有壁纸）时 `binaryAssets` **整个键都不出现**。
        // 这条断言是「默认载荷没有变大」的机械锁——空对象也会让每次建窗多解析一段 JSON，
        // 而 14 个图标的 base64 有 1 MiB 量级（见 payload_size_off_and_on）。
        assert!(
            v.get("binaryAssets").is_none(),
            "未配置完美图标且没有壁纸时不得输出 binaryAssets（默认载荷必须与 T13a 一致）"
        );
    }

    #[test]
    fn upstream_is_wrapped_until_document_element_exists() {
        let s = script(&Config::default());
        // 上游源码必须**逐字**保留（只包裹、不修改 vendored 代码）
        assert!(
            s.contains(CONTENT_SCRIPT_JS),
            "上游 content-script 必须逐字出现在载荷里"
        );
        // 且必须在「等到 documentElement」的壳内：否则 WebView2 的
        // AddScriptToExecuteOnDocumentCreated 会在 documentElement/head 还是 null 时执行，
        // 上游第一句 appendChild 就抛 TypeError，整条注入链静默失效。
        assert!(
            s.contains("function start() {"),
            "缺少等到 documentElement 的启动壳"
        );
        assert!(
            s.contains("mo.observe(document, { childList: true, subtree: true })"),
            "壳必须观察 document 本身（documentElement 还不存在时观察的是 document）"
        );
        assert!(
            s.contains("document.addEventListener('DOMContentLoaded', start)"),
            "MutationObserver 不可用时要有 DOMContentLoaded 兜底"
        );
        assert!(
            s.contains("window.__FNOS_UPSTREAM_ERROR__"),
            "上游抛错要留可观测痕迹"
        );
        // 壳必须在 shim/bootstrap 之后（shim 提供 chrome.*，bootstrap 先登记兜底）
        let i_boot = s.find("__FNOS_BOOTSTRAP__").expect("bootstrap section");
        let i_wrap = s.find("function start() {").expect("wrapper");
        assert!(i_boot < i_wrap, "启动壳必须排在 bootstrap 之后");
    }

    #[test]
    fn injection_disabled_yields_empty_script() {
        let mut cfg = Config::default();
        cfg.shell.inject_enabled = false;
        assert!(build_init_script(&cfg).is_empty());
        // 关掉注入时连壁纸也不该被读进来（更不该出现在脚本里）
        cfg.mods.launchpad_icon_scale_enabled = true;
        cfg.local.login_wallpaper_file_name = Some("wallpaper.png".into());
        assert!(build_init_script(&cfg).is_empty());
    }

    /// **Minor 1 的正面证据**：注入关闭时壁纸路径**一次文件 IO 都不做**。
    ///
    /// 只断言「脚本是空的」是不够的——旧实现同样返回空脚本，但它已经拿着配置里的名字去
    /// `Path::join` + `fs::metadata`/`fs::read` 过了（文件缺失时还会打一行「登录壁纸读取失败」
    /// 的噪声日志）。这里用**读文件尝试计数器**把「有没有走到 `fs::read`」变成可断言的事实：
    /// 关态必须是 0，而同一个配置把注入打开后必须是 1（否则计数器恒 0，上面的断言没有意义）。
    ///
    /// 计数器同样钉住 [`precheck_wallpaper_len`] 这道门在**读之前**：注入开着但壁纸超限时，
    /// 读尝试次数仍然是 0（见 `load_wallpaper_skips_missing_oversized_and_illegal_files`）。
    #[test]
    fn injection_off_does_not_touch_the_wallpaper_file_at_all() {
        let mut cfg = Config::default();
        cfg.local.login_wallpaper_file_name = Some("not-there-at-all.png".into());
        cfg.shell.inject_enabled = false;

        reset_wallpaper_io_counters();
        assert!(build_init_script(&cfg).is_empty());
        assert_eq!(
            wallpaper_stat_attempts(),
            0,
            "注入关闭时不得对壁纸文件做任何系统调用（连 metadata 都不做）"
        );
        assert_eq!(wallpaper_read_attempts(), 0, "更不得读文件");

        // 反证：同一份配置打开注入（文件不存在）→ 真的做了一次 metadata，读失败被跳过
        cfg.shell.inject_enabled = true;
        let script = build_init_script(&cfg);
        assert!(!script.is_empty(), "注入打开后必须产出载荷");
        assert_eq!(
            wallpaper_stat_attempts(),
            1,
            "注入打开时必须真的碰一次这个路径（否则上面那 0 是恒真的空断言）"
        );
        assert_eq!(
            wallpaper_read_attempts(),
            0,
            "文件不存在：预检就该挡住，没到 fs::read"
        );
        // 读不到的壁纸不得进载荷（配置段里当然还有那个文件名——那是配置值本身，不是资产）
        assert!(
            payload_json(&script).get("binaryAssets").is_none(),
            "读不到的壁纸不得让载荷多出 binaryAssets"
        );
    }

    #[test]
    fn payload_is_valid_json_prefix() {
        let s = script(&Config::default());
        let start = s.find('{').unwrap();
        let end = s.find("};\n").unwrap();
        let json = &s[start..=end];
        let v: serde_json::Value = serde_json::from_str(json).expect("配置段必须是合法 JSON");
        assert!(v.get("mods").is_some());
        assert!(v.get("assets").is_some());
    }

    #[test]
    fn meta_keys_are_camel_case_for_the_consumers() {
        let s = script(&Config::default());
        assert!(
            s.contains("\"shellVersion\""),
            "bootstrap.js 读 meta.shellVersion"
        );
        assert!(s.contains("\"modsVersion\""), "shim.js 读 meta.modsVersion");
        assert!(
            s.contains(&format!("\"modsVersion\":\"{MODS_VERSION}\"")),
            "载荷的 modsVersion 必须来自 MODS_VERSION 常量（R38：字面量不得重复）"
        );
        assert!(s.contains("\"modsCommit\""));
        assert!(
            !s.contains("\"shell_version\""),
            "snake_case 会让消费方静默回落 0.0.0"
        );
        assert!(!s.contains("\"mods_version\""));
    }

    // ---------- Task 13b：完美图标（按需嵌入的 PNG 资产） ----------

    /// 图标表必须与 vendored 目录**集合相等**（真的去读目录，双向比对）。
    ///
    /// 这条锁的是最容易发生、也最难发现的一类漂移：往
    /// `src-tauri/assets/fnos-mods/prefect_icon/` 里加/删一个 PNG，却忘了同步
    /// [`prefect_icons`]。少了 = 设置窗里选得到但页面永远拿不到（`getURL` 返回空串，
    /// 上游 `safeRuntimeGetURL` 于是放弃重绘）；多了 = 指向一个不存在的文件
    /// （`include_bytes!` 会编译失败，所以「多」只可能表现为「多了个空壳」）。
    #[test]
    fn icon_table_matches_the_vendored_directory() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fnos-mods/prefect_icon");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("vendored 图标目录必须存在 {}：{e}", dir.display()))
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.to_ascii_lowercase().ends_with(".png"))
            .map(|name| format!("prefect_icon/{}", name.to_ascii_lowercase()))
            .collect();
        on_disk.sort();

        let mut table: Vec<String> = prefect_icons()
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
        table.sort();
        assert_eq!(
            table, on_disk,
            "injector 的图标表必须与 vendored 目录一一对应（含大小写归一后的名字）"
        );
        assert_eq!(table.len(), 14, "完美图标资源是 14 个");
        for (key, bytes) in prefect_icons() {
            assert_eq!(*key, key.to_ascii_lowercase(), "{key} 必须是全小写键");
            assert!(
                bytes.starts_with(&[0x89, b'P', b'N', b'G']),
                "{key} 必须是 PNG（魔数 89 50 4e 47）"
            );
            assert!(bytes.len() > 1024, "{key} 看起来是个占位文件");
        }
    }

    /// 三个键**各自**都能单独把「需要图标资源」这件事打开（判据与 shim 的行为同义）。
    #[test]
    fn gating_covers_all_three_keys() {
        // 默认配置本来就是三个键全默认；再显式清一遍，证明判据不是恒真
        let mut off = Config::default();
        off.mods.launchpad_icon_scale_enabled = false;
        off.mods.launchpad_icon_redraw_map.clear();
        off.mods.launchpad_icon_scale_selected_keys.clear();
        assert!(!perfect_icon_enabled(&off));
        assert!(payload_json(&script(&off)).get("binaryAssets").is_none());

        let mut by_switch = Config::default();
        by_switch.mods.launchpad_icon_scale_enabled = true;
        assert!(perfect_icon_enabled(&by_switch));

        let mut by_map = Config::default();
        by_map
            .mods
            .launchpad_icon_redraw_map
            .insert("/a/icon_1.png".into(), "prefect_icon/emby.png".into());
        assert!(perfect_icon_enabled(&by_map));

        let mut by_keys = Config::default();
        by_keys
            .mods
            .launchpad_icon_scale_selected_keys
            .push("/a/icon_1.png".into());
        assert!(perfect_icon_enabled(&by_keys));

        // 三个键都把「有图标资源」这件事打开 → 脚本里真的出现整套 14 个图标
        for cfg in [&by_switch, &by_map, &by_keys] {
            let v = payload_json(&script(cfg));
            assert_eq!(
                binary_assets(&v).len(),
                14,
                "任一键非默认都应嵌入整套 14 个图标"
            );
        }
    }

    /// 开启后：恰好 14 个键、全小写、每个都是 `data:image/png;base64,…`，
    /// 且 base64 解码回来与 `include_bytes!` 的字节**逐字节相等**、长度与磁盘文件一致。
    #[test]
    fn icons_are_lowercase_png_data_urls_of_the_vendored_bytes() {
        let mut cfg = Config::default();
        cfg.mods.launchpad_icon_scale_enabled = true;
        let v = payload_json(&script(&cfg));
        let binary = binary_assets(&v);

        let mut keys: Vec<&str> = binary.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys.len(), 14);
        for key in &keys {
            assert_eq!(
                *key,
                key.to_ascii_lowercase(),
                "binaryAssets 的键必须全小写（shim 的查表键）"
            );
            assert!(key.starts_with("prefect_icon/"), "{key}");
            assert!(key.ends_with(".png"), "{key}");
        }
        // camelCase 的磁盘名必须已经被归一成小写键（否则 shim 查得到、但 payload 里两套写法并存）
        assert!(binary.contains_key("prefect_icon/panindex.png"));
        assert!(!binary.contains_key("prefect_icon/panIndex.png"));

        for (key, bytes) in prefect_icons() {
            let value = binary
                .get(*key)
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{key} 必须在 binaryAssets 里"));
            let b64 = value
                .strip_prefix("data:image/png;base64,")
                .unwrap_or_else(|| {
                    panic!(
                        "{key} 必须是 PNG data URL：{}",
                        &value[..40.min(value.len())]
                    )
                });
            assert_eq!(
                base64::decode(b64).expect("载荷里的 base64 必须可解码"),
                *bytes,
                "{key} 的 base64 必须还原出磁盘上的原始字节（编码器错了就是坏图）"
            );
            assert_eq!(
                b64.len(),
                (bytes.len() + 2) / 3 * 4,
                "{key} 的 base64 长度必须符合 4/3 膨胀公式"
            );
            // 文本资源那一支不受影响：assets 里不许出现同名 png
            assert!(!v["assets"].as_object().unwrap().contains_key(*key));
        }
    }

    /// 壁纸：按**小写**文件名嵌入，mime 由扩展名推导；`local` 段里的原文不被改写。
    #[test]
    fn wallpaper_is_embedded_under_its_lowercase_name() {
        for (name, mime) in [
            ("MyWall.png", "image/png"),
            ("photo.JPG", "image/jpeg"),
            ("photo.jpeg", "image/jpeg"),
            ("shot.WebP", "image/webp"),
        ] {
            let mut cfg = Config::default();
            cfg.local.login_wallpaper_file_name = Some(name.into());
            let bytes = vec![0x11u8, 0x22, 0x33, 0x44];
            let asset = WallpaperAsset {
                key: name.to_ascii_lowercase(),
                mime,
                bytes: bytes.clone(),
            };
            let v = payload_json(&build_init_script_with(&cfg, Some(asset)));
            let binary = binary_assets(&v);
            assert_eq!(binary.len(), 1, "没有完美图标时 binaryAssets 只该有壁纸");
            let value = binary
                .get(&name.to_ascii_lowercase())
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{name} 必须以小写键出现"));
            assert_eq!(
                value,
                &format!("data:{mime};base64,{}", base64::encode(&bytes)),
                "{name}"
            );
            // `local` 段仍是用户写的原文（大小写不归一：显示值 = 配置值）
            assert_eq!(v["local"]["loginWallpaperFileName"], json!(name));
        }
    }

    /// **载荷实测**：关（默认）与开（14 个图标）两种状态的字节数，以及 base64 膨胀。
    ///
    /// 打印的数字就是报告里那一节；断言锁的是「开关打开后多出来的字节恰好等于
    /// `, "binaryAssets": {…}` 这一段 JSON 的长度」——不是「大概 1.1 MiB」这种松判据。
    #[test]
    fn payload_size_off_and_on() {
        /// 配置段 JSON 的字节数（与键序无关：长度由内容决定）。
        fn payload_len(cfg: &Config) -> usize {
            serde_json::to_string(&payload_json(&script(cfg)))
                .expect("配置段必须可序列化")
                .len()
        }

        let off_cfg = Config::default();
        let on_cfg = {
            let mut c = Config::default();
            c.mods.launchpad_icon_scale_enabled = true;
            c
        };
        let off_len = payload_len(&off_cfg);
        let on_len = payload_len(&on_cfg);
        let off_script_len = script(&off_cfg).len();
        let on_script_len = script(&on_cfg).len();

        let raw: usize = prefect_icons().iter().map(|(_, b)| b.len()).sum();
        let b64_chars: usize = prefect_icons()
            .iter()
            .map(|(_, b)| base64::encode(b).len())
            .sum();
        // 期望的增量 = `,"binaryAssets":` + 那个对象本身的 JSON
        let mut expect_map = serde_json::Map::new();
        for (key, bytes) in prefect_icons() {
            expect_map.insert(
                (*key).to_string(),
                json!(format!("data:image/png;base64,{}", base64::encode(bytes))),
            );
        }
        let fragment = serde_json::to_string(&Value::Object(expect_map)).unwrap();
        let overhead = ",".len() + "\"binaryAssets\":".len();
        // 打开完美图标同时把 `launchpadIconScaleEnabled` 从 `false`(5) 写成了 `true`(4)：
        // 这一个字节也必须算进去，否则断言会因为「差一个字节」而看起来像个玄学问题。
        let switch_delta: isize = -(("false".len() - "true".len()) as isize);

        println!("T13B-PAYLOAD off_payload_json_bytes={off_len}");
        println!("T13B-PAYLOAD on_payload_json_bytes={on_len}");
        println!("T13B-PAYLOAD off_script_bytes={off_script_len}");
        println!("T13B-PAYLOAD on_script_bytes={on_script_len}");
        println!("T13B-PAYLOAD icons_raw_bytes={raw}");
        println!("T13B-PAYLOAD icons_base64_chars={b64_chars}");
        println!(
            "T13B-PAYLOAD base64_expansion_x1000={}",
            b64_chars * 1000 / raw
        );
        println!("T13B-PAYLOAD measured_delta_bytes={}", on_len - off_len);
        println!(
            "T13B-PAYLOAD expected_delta_bytes={}",
            fragment.len() as isize + overhead as isize + switch_delta
        );

        assert!(on_len > off_len, "打开完美图标后载荷必须变大");
        assert_eq!(
            on_len as isize - off_len as isize,
            fragment.len() as isize + overhead as isize + switch_delta,
            "多出来的字节必须恰好是 `,\"binaryAssets\":{{…}}` 这一段（再减去开关的 1 字节）"
        );
        // 量级锁：14 个图标的 base64 在 1 MiB 上下（报告里报的就是这两个数）
        assert!(
            (1_000_000..1_200_000).contains(&(on_len - off_len)),
            "实测增量 {} 与预期量级不符",
            on_len - off_len
        );
        // base64 的标准膨胀是 4/3（1000 倍表示下应落在 1333 附近）
        assert_eq!(b64_chars * 1000 / raw, 1333);
        // 关态就是 T13a 的原始载荷：9 个**文本**资源（7 个 CSS + mod.js + icon-map.json，
        // 实测约 288 KiB）——量级是几百 KB，不含任何 base64 大块。
        assert!(
            off_len < 400_000,
            "默认载荷不该有 MB 级内容（那是 base64 二进制的量级）：{off_len}"
        );
        assert!(
            payload_json(&script(&off_cfg))
                .get("binaryAssets")
                .is_none(),
            "关态必须连 binaryAssets 这个键都没有"
        );
    }

    // ---------- Task 13b：登录壁纸的读文件路径（唯一的 IO） ----------

    #[test]
    fn load_wallpaper_reads_and_lowercases_the_configured_file() {
        let dir = temp_dir("wallpaper-ok");
        let bytes = [0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a];
        std::fs::write(dir.join("MyWall.PNG"), bytes).unwrap();
        let mut cfg = Config::default();
        cfg.local.login_wallpaper_file_name = Some("MyWall.PNG".into());

        let asset = load_wallpaper_from(&dir, &cfg).expect("合法壁纸必须被读出来");
        assert_eq!(asset.key, "mywall.png", "键必须小写（shim 的查表键）");
        assert_eq!(asset.mime, "image/png");
        assert_eq!(asset.bytes, bytes);
        assert!(asset
            .data_url()
            .starts_with("data:image/png;base64,iVBORw0K"));

        // 首尾空白会被 trim（手改配置时常见）
        cfg.local.login_wallpaper_file_name = Some("  MyWall.PNG  ".into());
        assert_eq!(
            load_wallpaper_from(&dir, &cfg).map(|a| a.key),
            Some("mywall.png".into())
        );

        // 没有配置 → 不读、不报错
        let bare = Config::default();
        assert!(load_wallpaper_from(&dir, &bare).is_none());
        // 空白名字 → 同上
        let mut blank = Config::default();
        blank.local.login_wallpaper_file_name = Some("   ".into());
        assert!(load_wallpaper_from(&dir, &blank).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 缺失 / 过大 / 空文件 / 非法名字：一律 `None`（记一行日志），**绝不 panic、绝不截断**。
    #[test]
    fn load_wallpaper_skips_missing_oversized_and_illegal_files() {
        let dir = temp_dir("wallpaper-skip");

        // 文件不存在（配置里写了名字，但用户还没放文件进去）
        let mut missing = Config::default();
        missing.local.login_wallpaper_file_name = Some("nope.png".into());
        assert!(load_wallpaper_from(&dir, &missing).is_none());

        // 超过 8 MiB 上限：文件真的存在，但被跳过（不截断、不部分嵌入）
        std::fs::write(dir.join("huge.png"), vec![0u8; MAX_WALLPAPER_BYTES + 1]).unwrap();
        let mut huge = Config::default();
        huge.local.login_wallpaper_file_name = Some("huge.png".into());
        reset_wallpaper_io_counters();
        assert!(load_wallpaper_from(&dir, &huge).is_none());
        assert_eq!(wallpaper_stat_attempts(), 1, "预检必须看一次 metadata");
        assert_eq!(
            wallpaper_read_attempts(),
            0,
            "超限文件一个字节都不许读（旧实现是整份读进内存再比长度）"
        );
        // 正好等于上限：允许（上限是「含」上限）——这一条同时是上面那条的反证：真的读了
        std::fs::write(dir.join("exact.png"), vec![7u8; MAX_WALLPAPER_BYTES]).unwrap();
        let mut exact = Config::default();
        exact.local.login_wallpaper_file_name = Some("exact.png".into());
        reset_wallpaper_io_counters();
        assert_eq!(
            load_wallpaper_from(&dir, &exact).map(|a| a.bytes.len()),
            Some(MAX_WALLPAPER_BYTES)
        );
        assert_eq!(wallpaper_stat_attempts(), 1);
        assert_eq!(wallpaper_read_attempts(), 1, "上限之内必须真的读出来");

        // 空文件
        std::fs::write(dir.join("empty.png"), []).unwrap();
        let mut empty = Config::default();
        empty.local.login_wallpaper_file_name = Some("empty.png".into());
        reset_wallpaper_io_counters();
        assert!(load_wallpaper_from(&dir, &empty).is_none());
        assert_eq!(
            wallpaper_read_attempts(),
            0,
            "空文件同样在预检里被挡住（不必读一次才知道它是空的）"
        );

        // 不是一个普通文件（名字指向目录）：预检的 is_file() 分支，优雅跳过而不是读失败
        std::fs::create_dir_all(dir.join("adir.png")).unwrap();
        let mut adir = Config::default();
        adir.local.login_wallpaper_file_name = Some("adir.png".into());
        reset_wallpaper_io_counters();
        assert!(load_wallpaper_from(&dir, &adir).is_none());
        assert_eq!(wallpaper_read_attempts(), 0, "目录不得被 fs::read");

        // 扩展名不在允许表内（文件确实存在也不读）
        std::fs::write(dir.join("anim.gif"), b"GIF89a").unwrap();
        let mut gif = Config::default();
        gif.local.login_wallpaper_file_name = Some("anim.gif".into());
        assert!(load_wallpaper_from(&dir, &gif).is_none());

        // 穿越 / 分隔符 / 换行：形状判定直接拦住（连 join 都不做）
        let mut bads: Vec<String> = [
            "../secret.png",
            "sub/wall.png",
            "sub\\wall.png",
            "C:\\wall.png",
            "wall\npaper.png",
            "wall\tpaper.png",
            ".png",
            "wallpaper.svg",
            "wallpaper", // 没有扩展名
            "wall.png ", // 尾部空白（Windows 会吃掉，配置值与实际文件不是一回事）
            " wall.png", // 首部空白
            "wall*paper.png",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        bads.push("a".repeat(300));
        for bad in bads {
            let mut cfg = Config::default();
            cfg.local.login_wallpaper_file_name = Some(bad.clone());
            assert!(load_wallpaper_from(&dir, &cfg).is_none(), "bad={bad:?}");
        }
        // 反过来：**内部空白与非 ASCII 名字要放行**（文件名真的可以长这样；R30 的教训是
        // 「带点非 ASCII 就静默丢配置」比拒绝更难排障）。
        std::fs::write(dir.join("my wall 壁纸.PNG"), b"\x89PNG\r\n").unwrap();
        let mut cjk = Config::default();
        cjk.local.login_wallpaper_file_name = Some("my wall 壁纸.PNG".into());
        let asset = load_wallpaper_from(&dir, &cjk).expect("内部空格与非 ASCII 名字必须放行");
        assert_eq!(asset.key, "my wall 壁纸.png", "键只做 ASCII 小写化");
        assert_eq!(asset.mime, "image/png");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 读文件**之前**那道门的边界值（纯函数，直接驱动，不造 8 MiB 文件）。
    ///
    /// 上限的两个方向都要钉住：恰好 8 MiB 必须放行（「含」上限），8 MiB + 1 必须挡住；
    /// 非普通文件与空文件也在这道门里（而不是等 `fs::read` 去失败——那对目录/设备是另一套语义）。
    #[test]
    fn precheck_rejects_oversize_empty_and_non_files() {
        let cap = MAX_WALLPAPER_BYTES as u64;
        assert_eq!(precheck_wallpaper_len(cap, true), Precheck::Readable);
        assert_eq!(precheck_wallpaper_len(1, true), Precheck::Readable);
        assert_eq!(precheck_wallpaper_len(cap + 1, true), Precheck::TooLarge);
        assert_eq!(
            precheck_wallpaper_len(u64::MAX, true),
            Precheck::TooLarge,
            "离谱的大值同样只是「跳过」，不得溢出/panic"
        );
        assert_eq!(precheck_wallpaper_len(0, true), Precheck::Empty);
        assert_eq!(precheck_wallpaper_len(0, false), Precheck::NotAFile);
        assert_eq!(precheck_wallpaper_len(1, false), Precheck::NotAFile);
        // 非普通文件优先于长度判定：一个巨大的目录项也只想说「这不是普通文件」
        assert_eq!(precheck_wallpaper_len(cap + 1, false), Precheck::NotAFile);
    }
}
