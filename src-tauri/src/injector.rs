//! 纯函数：把配置与 vendored 资源拼成一段 initialization_script。
//! 不读文件、不碰 Tauri API —— 便于单测与快照锁定。

use crate::config::Config;
use serde::Serialize;
use serde_json::json;

pub const SHELL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MODS_COMMIT: &str = "483c3e2e217faebc1be45b4e824865854a61e3dd";

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

pub fn build_init_script(cfg: &Config) -> String {
    if !cfg.shell.inject_enabled {
        return String::new();
    }

    let mut asset_map = serde_json::Map::new();
    for (name, text) in assets().files {
        // content-script.js 由本函数末尾直接执行，不需要经 getURL 暴露
        asset_map.insert((*name).to_string(), json!(text));
    }
    // `prefect_icon/*.png` 是二进制，不走这里的 `assets`：Task 13 按配置选择后用单独的
    // `binaryAssets` 键承载。此处刻意完全不输出该键 —— shim.js 把它默认成 `{}`（shim.js:9）。

    let payload = json!({
        "meta": Meta {
            shell_version: SHELL_VERSION,
            mods_commit: MODS_COMMIT,
            mods_version: "1.0.2",
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
        payload = serde_json::to_string(&payload).unwrap(),
        shim = SHIM_JS,
        boot = BOOTSTRAP_JS,
        content = CONTENT_SCRIPT_JS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn script_has_four_sections_in_order() {
        let s = build_init_script(&Config::default());
        let i_cfg = s.find("__FNOS_SHELL__").expect("config section");
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
        for name in [
            "basic_mod.css",
            "mod.js",
            "content-script.js",
            "windows_titlebar_mod.css",
            "mac_titlebar_mod.css",
            "classic_launchpad_mod.css",
            "spotlight_launchpad_mod.css",
            "desktop_icon_mod.css",
            "lockscreen_mod.css",
        ] {
            assert!(s.contains(name), "载荷缺少资源键 {name}");
        }
        assert!(s.contains("\"brandColor\":\"#123456\""));
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
        assert!(s.contains("\"modsCommit\""));
        assert!(
            !s.contains("\"shell_version\""),
            "snake_case 会让消费方静默回落 0.0.0"
        );
        assert!(!s.contains("\"mods_version\""));
    }
}
