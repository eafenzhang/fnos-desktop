use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_HOME_URL: &str = "https://fnos.net/";
pub const DEFAULT_BRAND_COLOR: &str = "#0066ff";
pub const FONT_WEIGHTS: [&str; 3] = ["450", "normal", "600"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModsConfig {
    pub enabled_origins: Vec<String>,
    // 上游键名含大写缩写 `OS`（cs:2893），serde 的 camelCase 只会给出 `...Fnos`，必须显式改写
    #[serde(rename = "autoEnableSuspectedFnOS")]
    pub auto_enable_suspected_fnos: bool,
    pub base_preset_enabled: bool,
    pub window_animation_blur_enabled: bool,
    pub titlebar_style: String,
    pub launchpad_style: String,
    pub desktop_icon_layout_enabled: bool,
    pub desktop_icon_layout_mode: String,
    pub desktop_icon_per_column: u32,
    pub desktop_icon_per_column_enabled: Option<bool>,
    pub launchpad_icon_scale_enabled: bool,
    pub launchpad_icon_scale_selected_keys: Vec<String>,
    pub launchpad_icon_mask_only_keys: Vec<String>,
    pub launchpad_icon_redraw_keys: Vec<String>,
    pub launchpad_icon_redraw_map: std::collections::BTreeMap<String, String>,
    pub brand_color: String,
    pub font_override_enabled: bool,
    pub font_family: String,
    pub font_monospace_family: String,
    pub font_weight: String,
    pub font_feature_settings: String,
    pub font_face_name: String,
    pub font_url: String,
    pub custom_code_enabled: bool,
    pub lockscreen_default_username: String,
}

impl Default for ModsConfig {
    fn default() -> Self {
        Self {
            enabled_origins: Vec::new(),
            auto_enable_suspected_fnos: true,
            base_preset_enabled: true,
            window_animation_blur_enabled: true,
            titlebar_style: "windows".into(),
            launchpad_style: "classic".into(),
            desktop_icon_layout_enabled: true,
            desktop_icon_layout_mode: "adaptive".into(),
            desktop_icon_per_column: 8,
            desktop_icon_per_column_enabled: None,
            launchpad_icon_scale_enabled: false,
            launchpad_icon_scale_selected_keys: Vec::new(),
            launchpad_icon_mask_only_keys: Vec::new(),
            launchpad_icon_redraw_keys: Vec::new(),
            launchpad_icon_redraw_map: Default::default(),
            brand_color: DEFAULT_BRAND_COLOR.into(),
            font_override_enabled: false,
            font_family: String::new(),
            font_monospace_family: String::new(),
            font_weight: String::new(),
            font_feature_settings: String::new(),
            font_face_name: "FnOSCustomFont".into(),
            font_url: String::new(),
            custom_code_enabled: false,
            lockscreen_default_username: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct LocalConfig {
    pub custom_css_code: String,
    pub custom_js_code: String,
    pub login_wallpaper_file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowGeom {
    pub w: f64,
    pub h: f64,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

impl Default for WindowGeom {
    fn default() -> Self {
        Self { w: 1200.0, h: 820.0, x: None, y: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShellConfig {
    pub home_url: String,
    pub nas_url: String,
    pub inject_enabled: bool,
    pub close_to_tray: bool,
    pub window: WindowGeom,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            home_url: DEFAULT_HOME_URL.into(),
            nas_url: String::new(),
            inject_enabled: true,
            close_to_tray: true,
            window: WindowGeom::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub schema_version: u32,
    pub mods: ModsConfig,
    pub local: LocalConfig,
    pub shell: ShellConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            mods: ModsConfig::default(),
            local: LocalConfig::default(),
            shell: ShellConfig::default(),
        }
    }
}

/// #rrggbb（或 #rgb）→ HSL，把明度夹到 30%–70%，再转回 #rrggbb。
/// 非法输入回落默认色。
pub fn normalize_brand_color(input: &str) -> String {
    let raw = input.trim();
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    // 非 ASCII 直接回落：后续按字节切片，多字节字符会落在字符边界之外而 panic。
    if !hex.is_ascii() {
        return DEFAULT_BRAND_COLOR.into();
    }
    let expanded = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => hex.to_string(),
        _ => return DEFAULT_BRAND_COLOR.into(),
    };
    let (r, g, b) = match (
        u8::from_str_radix(&expanded[0..2], 16),
        u8::from_str_radix(&expanded[2..4], 16),
        u8::from_str_radix(&expanded[4..6], 16),
    ) {
        (Ok(r), Ok(g), Ok(b)) => (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0),
        _ => return DEFAULT_BRAND_COLOR.into(),
    };

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let l = l.clamp(0.30, 0.70);

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", to(r1), to(g1), to(b1))
}

/// 从 URL 的 scheme://host[:port] 得到 origin（全部小写）。无 scheme 或空 host 返回 None。
pub fn origin_of(url: &str) -> Option<String> {
    let s = url.trim();
    let (scheme, rest) = s.split_once("://")?;
    if scheme.is_empty() { return None; }
    let host_port = rest.split(['/', '?', '#']).next()?;
    if host_port.is_empty() { return None; }
    Some(format!("{}://{}", scheme.to_ascii_lowercase(), host_port.to_ascii_lowercase()))
}

/// 上游 `launchpadIconRedrawMap` 的取值约束：`^prefect_icon/[a-z0-9-]+\.png$`
pub fn is_valid_prefect_icon_path(v: &str) -> bool {
    let Some(rest) = v.strip_prefix("prefect_icon/") else { return false };
    let Some(name) = rest.strip_suffix(".png") else { return false };
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl Config {
    pub fn normalize(&mut self) {
        self.schema_version = SCHEMA_VERSION;
        // §6.4：先算出 origin（借用在此结束），最后再并入白名单
        let nas_origin = origin_of(&self.shell.nas_url);
        let m = &mut self.mods;
        m.brand_color = normalize_brand_color(&m.brand_color);
        if m.titlebar_style != "mac" { m.titlebar_style = "windows".into(); }
        if m.launchpad_style != "spotlight" { m.launchpad_style = "classic".into(); }
        if m.desktop_icon_layout_mode != "fixed" { m.desktop_icon_layout_mode = "adaptive".into(); }
        m.desktop_icon_per_column = m.desktop_icon_per_column.clamp(4, 16);
        if !FONT_WEIGHTS.contains(&m.font_weight.as_str()) { m.font_weight = String::new(); }
        m.lockscreen_default_username = m.lockscreen_default_username.chars().take(80).collect();
        m.enabled_origins.retain(|o| !o.trim().is_empty());
        m.launchpad_icon_redraw_map
            .retain(|_, v| is_valid_prefect_icon_path(v));
        if self.shell.home_url.trim().is_empty() {
            self.shell.home_url = DEFAULT_HOME_URL.into();
        }
        // 保存 NAS WebUI 地址时自动把其 origin 并入注入白名单（跳过 1.5s 探测），幂等
        if let Some(origin) = nas_origin {
            if !self.mods.enabled_origins.iter().any(|o| o.eq_ignore_ascii_case(&origin)) {
                self.mods.enabled_origins.push(origin);
            }
        }
    }

    /// 配置文件绝对路径。消费方在 Task 7/8 接线前暂未使用。
    #[allow(dead_code)]
    pub fn config_path() -> PathBuf {
        crate::paths::config_dir().join("config.json")
    }

    pub fn load(path: &Path) -> Config {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Config::default(),
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(mut c) => { c.normalize(); c }
            Err(_) => {
                let _ = std::fs::write(path.with_extension("json.bak"), &text);
                Config::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_upstream() {
        let c = Config::default();
        assert_eq!(c.mods.brand_color, "#0066ff");
        assert_eq!(c.mods.titlebar_style, "windows");
        assert_eq!(c.mods.launchpad_style, "classic");
        assert_eq!(c.mods.desktop_icon_layout_mode, "adaptive");
        assert_eq!(c.mods.desktop_icon_per_column, 8);
        assert_eq!(c.mods.font_face_name, "FnOSCustomFont");
        assert!(c.mods.auto_enable_suspected_fnos);
        assert!(c.mods.base_preset_enabled);
        assert!(c.mods.desktop_icon_layout_enabled);
        assert!(!c.mods.font_override_enabled);
        assert!(!c.mods.custom_code_enabled);
        assert!(c.mods.enabled_origins.is_empty());
        assert_eq!(c.shell.home_url, DEFAULT_HOME_URL);
        assert!(c.shell.inject_enabled);
        assert!(c.shell.close_to_tray);
    }

    #[test]
    fn brand_color_lightness_is_clamped() {
        // 纯白明度 100% → 夹到 70%；纯黑 0% → 夹到 30%
        assert_eq!(normalize_brand_color("#ffffff"), "#b3b3b3"); // L=70%
        assert_eq!(normalize_brand_color("#000000"), "#4d4d4d"); // L=30%
        assert_eq!(normalize_brand_color("#0066ff"), "#0066ff"); // 合法值保持不变
        assert_eq!(normalize_brand_color("nope"), "#0066ff");    // 非法值回落默认
        assert_eq!(normalize_brand_color("#06f"), "#0066ff");    // 展开短写法
    }

    #[test]
    fn enums_and_numbers_fall_back() {
        let mut c = Config::default();
        c.mods.titlebar_style = "linux".into();
        c.mods.launchpad_style = "weird".into();
        c.mods.desktop_icon_layout_mode = "fixedd".into();
        c.mods.desktop_icon_per_column = 99;
        c.mods.font_weight = "bold".into();
        c.mods.lockscreen_default_username = "x".repeat(120);
        c.normalize();
        assert_eq!(c.mods.titlebar_style, "windows");
        assert_eq!(c.mods.launchpad_style, "classic");
        assert_eq!(c.mods.desktop_icon_layout_mode, "adaptive");
        assert_eq!(c.mods.desktop_icon_per_column, 16);
        assert_eq!(c.mods.font_weight, "");
        assert_eq!(c.mods.lockscreen_default_username.chars().count(), 80);
    }

    #[test]
    fn roundtrip_and_migration() {
        let dir = std::env::temp_dir().join(format!("fnos-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");

        let mut c = Config::default();
        c.mods.brand_color = "#336699".into();
        c.save(&p).unwrap();
        let back = Config::load(&p);
        assert_eq!(back.mods.brand_color, "#336699");
        assert_eq!(back.schema_version, SCHEMA_VERSION);

        // 旧版本（无 schemaVersion / 缺字段）应能补齐。
        // 注意：load 必经 §6.5 归一化，深色 #010203（L≈0.8%）会被明度夹到 30% → #264d73，
        // 故此处断言「文件里的值确被读入并归一化」（≠ 默认色），而非原样保留。
        std::fs::write(&p, br##"{"mods":{"brandColor":"#010203"}}"##).unwrap();
        let migrated = Config::load(&p);
        assert_eq!(migrated.mods.brand_color, "#264d73");
        assert_ne!(migrated.mods.brand_color, DEFAULT_BRAND_COLOR);
        assert_eq!(migrated.schema_version, SCHEMA_VERSION);
        assert_eq!(migrated.mods.titlebar_style, "windows");

        // 损坏 JSON：回退默认 + 生成 .bak
        std::fs::write(&p, b"{not json").unwrap();
        let broken = Config::load(&p);
        assert_eq!(broken.mods.brand_color, "#0066ff");
        assert!(dir.join("config.json.bak").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 「旧版本 / 缺字段」必须补齐为**上游默认值**，而不是类型零值：
    /// 若退化成零值，partial config 会把 `autoEnableSuspectedFnOS` 等默认 true 的键静默变 false。
    #[test]
    fn partial_config_fills_upstream_defaults() {
        let c: Config = serde_json::from_str(r##"{"mods":{"brandColor":"#010203"}}"##).unwrap();
        assert!(c.mods.auto_enable_suspected_fnos);
        assert!(c.mods.base_preset_enabled);
        assert!(c.mods.window_animation_blur_enabled);
        assert!(c.mods.desktop_icon_layout_enabled);
        assert_eq!(c.mods.desktop_icon_per_column, 8);
        assert_eq!(c.mods.titlebar_style, "windows");
        assert_eq!(c.mods.desktop_icon_per_column_enabled, None);
        assert_eq!(c.shell.home_url, DEFAULT_HOME_URL);
        assert!(c.shell.inject_enabled);
        assert!(c.shell.close_to_tray);
        assert_eq!(c.shell.window.w, 1200.0);
        assert_eq!(c.schema_version, SCHEMA_VERSION);

        // 整个 mods / shell 段缺失时同样补齐
        let d: Config = serde_json::from_str("{}").unwrap();
        assert!(d.mods.auto_enable_suspected_fnos);
        assert_eq!(d.mods.desktop_icon_per_column, 8);
        assert_eq!(d.shell.home_url, DEFAULT_HOME_URL);
        assert_eq!(d.schema_version, SCHEMA_VERSION);
    }

    /// spec §6.1/§6.2：`mods` 段键名必须与上游 `chrome.storage.sync` 逐字一致
    /// （`content-script.js:2890-2917`），否则用户无法直接粘贴浏览器扩展里已有的配置。
    #[test]
    fn mods_keys_match_upstream_storage_names() {
        let v = serde_json::to_value(Config::default()).unwrap();
        let mods = v["mods"].as_object().unwrap();
        let mut got: Vec<&str> = mods.keys().map(String::as_str).collect();
        got.sort_unstable();

        let mut want = vec![
            "enabledOrigins",
            "autoEnableSuspectedFnOS",
            "basePresetEnabled",
            "windowAnimationBlurEnabled",
            "titlebarStyle",
            "launchpadStyle",
            "desktopIconLayoutEnabled",
            "desktopIconLayoutMode",
            "desktopIconPerColumn",
            "desktopIconPerColumnEnabled",
            "launchpadIconScaleEnabled",
            "launchpadIconScaleSelectedKeys",
            "launchpadIconMaskOnlyKeys",
            "launchpadIconRedrawKeys",
            "launchpadIconRedrawMap",
            "brandColor",
            "fontOverrideEnabled",
            "fontFamily",
            "fontMonospaceFamily",
            "fontWeight",
            "fontFeatureSettings",
            "fontFaceName",
            "fontUrl",
            "customCodeEnabled",
            "lockscreenDefaultUsername",
        ];
        want.sort_unstable();
        assert_eq!(want.len(), 25);
        assert_eq!(got, want);

        // 读回：上游写法（大写 OS）必须被识别，而不是当成未知键被忽略
        let c: Config = serde_json::from_str(r#"{"mods":{"autoEnableSuspectedFnOS":false}}"#).unwrap();
        assert!(!c.mods.auto_enable_suspected_fnos);
        // local 段键名同样对齐上游（cs:108-109、cs:55）
        let lv = serde_json::to_value(LocalConfig::default()).unwrap();
        let mut lgot: Vec<&str> = lv.as_object().unwrap().keys().map(String::as_str).collect();
        lgot.sort_unstable();
        assert_eq!(lgot, vec!["customCssCode", "customJsCode", "loginWallpaperFileName"]);
    }

    #[test]
    fn brand_color_non_ascii_falls_back_without_panic() {
        // 非 ASCII 会让「按字节长度分支 + 按字节切片」落在字符边界外（panic）：
        // "日" 是 3 字节 → 走短写法展开分支；"#日abc" 是 6 字节 → 走 6 位切片分支。
        assert_eq!(normalize_brand_color("日"), DEFAULT_BRAND_COLOR);
        assert_eq!(normalize_brand_color("#日abc"), DEFAULT_BRAND_COLOR);
        assert_eq!(normalize_brand_color("#日"), DEFAULT_BRAND_COLOR);
    }

    #[test]
    fn nas_url_origin_joins_whitelist() {        assert_eq!(origin_of("http://192.168.1.10:5666/webui/"), Some("http://192.168.1.10:5666".into()));
        assert_eq!(origin_of("https://abc.fnos.net"), Some("https://abc.fnos.net".into()));
        assert_eq!(origin_of("not a url"), None);

        let mut c = Config::default();
        c.shell.nas_url = "HTTP://Nas.Local:8000/".into();
        c.normalize();
        assert!(c.mods.enabled_origins.iter().any(|o| o == "http://nas.local:8000"));

        // 幂等：再次归一化不得重复添加
        c.normalize();
        assert_eq!(c.mods.enabled_origins.iter().filter(|o| o.contains("nas.local")).count(), 1);
    }
}
