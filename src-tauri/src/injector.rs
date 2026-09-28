//! 纯函数：把配置与 vendored 资源拼成一段 initialization_script。
//! 不读文件、不碰 Tauri API —— 便于单测与快照锁定。

use crate::config::Config;
use serde::Serialize;
use serde_json::json;

pub const SHELL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MODS_COMMIT: &str = "483c3e2e217faebc1be45b4e824865854a61e3dd";
/// 上游 fnOS_UI_Mods 的版本号（R38）：`injector` 的载荷与 `commands::ConfigView.meta`
/// 共用同一个常量，避免字面量在两处漂移。
pub const MODS_VERSION: &str = "1.0.2";

const SHIM_JS: &str = include_str!("../inject/shim.js");
const BOOTSTRAP_JS: &str = include_str!("../inject/bootstrap.js");
const CONTENT_SCRIPT_JS: &str = include_str!("../assets/fnos-mods/content-script.js");

struct Assets {
    files: &'static [(&'static str, &'static str)],
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
/// - 否则用 `MutationObserver` 观察 **`document` 本身**，解析器创建 `<html>` 时立刻触发
///   （微任务，仍早于页面自己的脚本）；
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

pub fn build_init_script(cfg: &Config) -> String {
    if !cfg.shell.inject_enabled {
        return String::new();
    }

    // `prefect_icon/*.png` 是二进制，不走这里的 `assets`：Task 13 按配置选择后用单独的
    // `binaryAssets` 键承载。此处刻意完全不输出该键 —— shim.js 把它默认成 `{}`（shim.js:9）。
    // 同理，`content-script.js` 由本函数末尾直接执行，不需要经 getURL 暴露。
    let mut asset_map = serde_json::Map::new();
    for (name, text) in assets().files {
        asset_map.insert((*name).to_string(), json!(text));
    }

    let payload = json!({
        "meta": Meta {
            shell_version: SHELL_VERSION,
            mods_commit: MODS_COMMIT,
            mods_version: MODS_VERSION,
        },
        "mods": &cfg.mods,
        "local": &cfg.local,
        "assets": asset_map,
    });

    format!(
        "/* fnOS Desktop Shell init script v{ver} (mods {commit}) */\n\
         window.__FNOS_SHELL__ = {payload};\n\
         {shim}\n\
         {boot}\n\
         {content}\n",
        ver = SHELL_VERSION,
        commit = MODS_COMMIT,
        payload = serde_json::to_string(&payload)
            .expect("payload is a serde Value and always serializes"),
        shim = SHIM_JS,
        boot = BOOTSTRAP_JS,
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

    #[test]
    fn script_has_four_sections_in_order() {
        let s = build_init_script(&Config::default());
        // 锚点必须带 ` = `：shim.js/bootstrap.js 里也有 `W.__FNOS_SHELL__`，只用
        // `__FNOS_SHELL__` 会让载荷整体挪到 shim 之下时本测试仍然通过。
        let i_cfg = s.find("window.__FNOS_SHELL__ = ").expect("config section");
        let i_shim = s.find("fnos-desktop-shell").expect("shim section");
        let i_boot = s.find("__FNOS_BOOTSTRAP__").expect("bootstrap section");
        let i_cs = s.find("hasFnOSSignature").expect("upstream content-script");
        assert!(
            i_cfg < i_shim && i_shim < i_boot && i_boot < i_cs,
            "段落顺序必须是 配置→shim→bootstrap→上游"
        );
    }

    #[test]
    fn payload_carries_all_assets_and_mods_config() {
        let mut cfg = Config::default();
        cfg.mods.brand_color = "#123456".into();
        let s = build_init_script(&cfg);
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
        assert!(
            v.get("binaryAssets").is_none(),
            "本阶段刻意不输出 binaryAssets（Task 13 才按配置承载 .png）"
        );
    }

    #[test]
    fn upstream_is_wrapped_until_document_element_exists() {
        let s = build_init_script(&Config::default());
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
    }

    #[test]
    fn payload_is_valid_json_prefix() {
        let s = build_init_script(&Config::default());
        let start = s.find('{').unwrap();
        let end = s.find("};\n").unwrap();
        let json = &s[start..=end];
        let v: serde_json::Value = serde_json::from_str(json).expect("配置段必须是合法 JSON");
        assert!(v.get("mods").is_some());
        assert!(v.get("assets").is_some());
    }

    #[test]
    fn meta_keys_are_camel_case_for_the_consumers() {
        let s = build_init_script(&Config::default());
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
}
