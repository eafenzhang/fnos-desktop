use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
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
        Self {
            w: 1200.0,
            h: 820.0,
            x: None,
            y: None,
        }
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
    let s = if d == 0.0 {
        0.0
    } else {
        d / (1.0 - (2.0 * l - 1.0).abs())
    };
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
    if scheme.is_empty() {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    // 剥掉 userinfo（`user:pass@`）：取最后一个 '@' 之后的段落（密码本身可能含 '@'）。
    // 否则白名单里会留下 `http://user:pass@host` 这种永远匹配不上 `location.origin` 的条目。
    let host_port = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    if host_port.is_empty() {
        return None;
    }
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host_port.to_ascii_lowercase()
    ))
}

/// 上游 `launchpadIconRedrawMap` 的取值约束：`^prefect_icon/[a-z0-9-]+\.png$`
pub fn is_valid_prefect_icon_path(v: &str) -> bool {
    let Some(rest) = v.strip_prefix("prefect_icon/") else {
        return false;
    };
    let Some(name) = rest.strip_suffix(".png") else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `<path>.bak`（追加式命名：对 `config.json` 得 `config.json.bak`）。
fn bak_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// bool 强转：`true`/`false` 原样；字符串 `"true"`/`"1"`（忽略大小写/空白）→ true，
/// `"false"`/`"0"` → false；其余（含 `null`）→ None（调用方删除该键）。
fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("true") || t == "1" {
                Some(true)
            } else if t.eq_ignore_ascii_case("false") || t == "0" {
                Some(false)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 数值：JSON 数字原样取用，数字字符串按十进制解析，浮点四舍五入到整数（半值远离零）；
/// 非有限值（`"inf"`/`"NaN"`）与其它类型 → None。
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|f| f.is_finite())
}

fn as_u32(v: &Value) -> Option<u32> {
    let n = as_f64(v)?;
    Some(n.round().clamp(0.0, u32::MAX as f64) as u32)
}

/// 取出对象字段；缺失或不是对象 → 空对象（该段全部由 `#[serde(default)]` 补齐），不牵连其它段。
fn take_object(root: &mut Map<String, Value>, key: &str) -> Map<String, Value> {
    match root.remove(key) {
        Some(Value::Object(o)) => o,
        _ => Map::new(),
    }
}

fn sanitize_bool_field(obj: &mut Map<String, Value>, key: &str) {
    match obj.get(key).and_then(as_bool) {
        Some(b) => {
            obj.insert(key.into(), Value::Bool(b));
        }
        None => {
            obj.remove(key);
        }
    }
}

/// `Option<bool>`（§6.2 遗留键）：只有 JSON bool 保留；`null`/其它删除 → 默认 `None`。
fn sanitize_opt_bool_field(obj: &mut Map<String, Value>, key: &str) {
    if !matches!(obj.get(key), Some(Value::Bool(_))) {
        obj.remove(key);
    }
}

fn sanitize_string_field(obj: &mut Map<String, Value>, key: &str) {
    if !matches!(obj.get(key), Some(Value::String(_))) {
        obj.remove(key);
    }
}

/// `Option<String>`：字符串与 `null` 都保留（显式 None），其余删除。
fn sanitize_opt_string_field(obj: &mut Map<String, Value>, key: &str) {
    if !matches!(obj.get(key), Some(Value::String(_)) | Some(Value::Null)) {
        obj.remove(key);
    }
}

fn sanitize_u32_field(obj: &mut Map<String, Value>, key: &str) {
    match obj.get(key).and_then(as_u32) {
        Some(n) => {
            obj.insert(key.into(), Value::from(n));
        }
        None => {
            obj.remove(key);
        }
    }
}

/// `string[]`：只保留字符串元素，元素 trim 后为空的丢弃；非数组 → 删除。
fn sanitize_string_list_field(obj: &mut Map<String, Value>, key: &str) {
    let list = match obj.get(key) {
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(|item| {
                    let Value::String(s) = item else { return None };
                    let t = s.trim();
                    if t.is_empty() {
                        None
                    } else {
                        Some(Value::String(t.to_string()))
                    }
                })
                .collect::<Vec<Value>>(),
        ),
        _ => None,
    };
    match list {
        Some(l) => {
            obj.insert(key.into(), Value::Array(l));
        }
        None => {
            obj.remove(key);
        }
    }
}

/// `Record<string,string>`：只保留 `string → string` 的条目；非对象 → 删除。
/// 值本身是否合法（`prefect_icon/*.png`）由 `Config::normalize` 的正则兜底。
fn sanitize_string_map_field(obj: &mut Map<String, Value>, key: &str) {
    let kept = match obj.get(key) {
        Some(Value::Object(m)) => Some(
            m.iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k.clone(), Value::String(s.clone()))),
                    _ => None,
                })
                .collect::<Map<String, Value>>(),
        ),
        _ => None,
    };
    match kept {
        Some(m) => {
            obj.insert(key.into(), Value::Object(m));
        }
        None => {
            obj.remove(key);
        }
    }
}

/// `WindowGeom`：`w`/`h` 按数值强转；`x`/`y` 数值或 `null`（两者都是 `Option<f64>`）；非对象 → 删除。
fn sanitize_window_field(obj: &mut Map<String, Value>, key: &str) {
    let win = match obj.get(key) {
        Some(Value::Object(w)) => Some(w.clone()),
        _ => None,
    };
    let Some(mut win) = win else {
        obj.remove(key);
        return;
    };
    sanitize_u32_field(&mut win, "w");
    sanitize_u32_field(&mut win, "h");
    for k in ["x", "y"] {
        match win.get(k) {
            Some(Value::Null) => {}
            Some(v) => match as_f64(v) {
                Some(n) => {
                    win.insert(k.into(), Value::from(n));
                }
                None => {
                    win.remove(k);
                }
            },
            None => {}
        }
    }
    obj.insert(key.into(), Value::Object(win));
}

/// 逐字段净化：类型不符的已知键**删除**，由 `#[serde(default)]` 用上游默认值补齐；
/// 能安全强转的键（bool 字符串、数字字符串、浮点）就地转换。未列出的键原样保留（serde 忽略未知键）。
fn sanitize_config_value(raw: Value) -> Value {
    let Value::Object(mut root) = raw else {
        return Value::Object(Map::new());
    };
    sanitize_u32_field(&mut root, "schemaVersion");

    let mut mods = take_object(&mut root, "mods");
    for key in [
        "autoEnableSuspectedFnOS",
        "basePresetEnabled",
        "windowAnimationBlurEnabled",
        "desktopIconLayoutEnabled",
        "launchpadIconScaleEnabled",
        "fontOverrideEnabled",
        "customCodeEnabled",
    ] {
        sanitize_bool_field(&mut mods, key);
    }
    sanitize_opt_bool_field(&mut mods, "desktopIconPerColumnEnabled");
    for key in [
        "titlebarStyle",
        "launchpadStyle",
        "desktopIconLayoutMode",
        "brandColor",
        "fontFamily",
        "fontMonospaceFamily",
        "fontWeight",
        "fontFeatureSettings",
        "fontFaceName",
        "fontUrl",
        "lockscreenDefaultUsername",
    ] {
        sanitize_string_field(&mut mods, key);
    }
    for key in [
        "enabledOrigins",
        "launchpadIconScaleSelectedKeys",
        "launchpadIconMaskOnlyKeys",
        "launchpadIconRedrawKeys",
    ] {
        sanitize_string_list_field(&mut mods, key);
    }
    sanitize_string_map_field(&mut mods, "launchpadIconRedrawMap");
    sanitize_u32_field(&mut mods, "desktopIconPerColumn");
    root.insert("mods".into(), Value::Object(mods));

    let mut local = take_object(&mut root, "local");
    for key in ["customCssCode", "customJsCode"] {
        sanitize_string_field(&mut local, key);
    }
    sanitize_opt_string_field(&mut local, "loginWallpaperFileName");
    root.insert("local".into(), Value::Object(local));

    let mut shell = take_object(&mut root, "shell");
    for key in ["homeUrl", "nasUrl"] {
        sanitize_string_field(&mut shell, key);
    }
    for key in ["injectEnabled", "closeToTray"] {
        sanitize_bool_field(&mut shell, key);
    }
    sanitize_window_field(&mut shell, "window");
    root.insert("shell".into(), Value::Object(shell));

    Value::Object(root)
}

impl Config {
    pub fn normalize(&mut self) {
        self.schema_version = SCHEMA_VERSION;
        // §6.4：先算出 origin（借用在此结束），最后再并入白名单
        let nas_origin = origin_of(&self.shell.nas_url);
        let m = &mut self.mods;
        m.brand_color = normalize_brand_color(&m.brand_color);
        if m.titlebar_style != "mac" {
            m.titlebar_style = "windows".into();
        }
        if m.launchpad_style != "spotlight" {
            m.launchpad_style = "classic".into();
        }
        if m.desktop_icon_layout_mode != "fixed" {
            m.desktop_icon_layout_mode = "adaptive".into();
        }
        m.desktop_icon_per_column = m.desktop_icon_per_column.clamp(4, 16);
        if !FONT_WEIGHTS.contains(&m.font_weight.as_str()) {
            m.font_weight = String::new();
        }
        m.lockscreen_default_username = m.lockscreen_default_username.chars().take(80).collect();
        // R23：上游按 `location.origin` **大小写敏感**比较（cs:2948），故必须 trim + 小写，
        // 并做大小写不敏感去重——否则 `" HTTP://NAS.LOCAL:8000"` 之类的条目静默失效。
        let mut seen = std::collections::HashSet::new();
        let origins = std::mem::take(&mut m.enabled_origins);
        m.enabled_origins = origins
            .into_iter()
            .filter_map(|o| {
                let o = o.trim().to_ascii_lowercase();
                if o.is_empty() || !seen.insert(o.clone()) {
                    None
                } else {
                    Some(o)
                }
            })
            .collect();
        m.launchpad_icon_redraw_map
            .retain(|_, v| is_valid_prefect_icon_path(v));
        if self.shell.home_url.trim().is_empty() {
            self.shell.home_url = DEFAULT_HOME_URL.into();
        }
        // 保存 NAS WebUI 地址时自动把其 origin 并入注入白名单（跳过 1.5s 探测），幂等
        if let Some(origin) = nas_origin {
            if !self
                .mods
                .enabled_origins
                .iter()
                .any(|o| o.eq_ignore_ascii_case(&origin))
            {
                self.mods.enabled_origins.push(origin);
            }
        }
    }

    /// 配置文件绝对路径。消费方在 Task 7/8 接线前暂未使用。
    #[allow(dead_code)]
    pub fn config_path() -> PathBuf {
        crate::paths::config_dir().join("config.json")
    }

    /// 宽松加载：**逐字段**净化，单个坏类型键只回落它自己，绝不丢掉整份配置（§6.5 / §6.1）。
    ///
    /// 顺序：读字节 → UTF-8 解码（剥 BOM）→ 解析 `Value` → 按字段净化 → `from_value` → `normalize`。
    /// 读/解码错误把原件拷成 `<path>.bak`；语法损坏把文本写进 `<path>.bak`，两者都回落默认值。
    pub fn load(path: &Path) -> Config {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => {
                // 读失败（含文件不存在）：尽力拷原件留证，再回落默认
                let _ = std::fs::copy(path, bak_path(path));
                return Config::default();
            }
        };
        let mut text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                // 非 UTF-8（UTF-16 / 二进制）：拷原件留证，否则损坏证据会被丢掉
                let _ = std::fs::copy(path, bak_path(path));
                return Config::default();
            }
        };
        // Notepad 默认会写 UTF-8 BOM；不剥掉的话 serde_json 直接报错 → 整份配置被丢弃
        if text.starts_with('\u{feff}') {
            text.remove(0);
        }

        let value: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                let _ = std::fs::write(bak_path(path), &text);
                return Config::default();
            }
        };
        if !value.is_object() {
            // 顶层不是对象（如 `[1,2]` / `"x"`）：没有可净化的字段，按损坏留证
            let _ = std::fs::write(bak_path(path), &text);
            return Config::default();
        }

        match serde_json::from_value::<Config>(sanitize_config_value(value)) {
            Ok(mut c) => {
                c.normalize();
                c
            }
            Err(_) => {
                let _ = std::fs::write(bak_path(path), &text);
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
        assert_eq!(normalize_brand_color("nope"), "#0066ff"); // 非法值回落默认
        assert_eq!(normalize_brand_color("#06f"), "#0066ff"); // 展开短写法
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

    /// 每个测试一个独立临时目录（按 tag + pid 命名），避免并行测试共用路径互相污染。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fnos-cfg-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roundtrip_and_migration() {
        let dir = std::env::temp_dir().join(format!("fnos-cfg-{}", std::process::id()));
        // 先清空固定目录：残留的 .bak 会让下面「损坏必留证」的断言假通过
        let _ = std::fs::remove_dir_all(&dir);
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

        // 损坏 JSON：回退默认 + 生成 .bak（内容必须等于写入的损坏文本）
        let corrupt: &[u8] = b"{not json";
        std::fs::write(&p, corrupt).unwrap();
        let broken = Config::load(&p);
        assert_eq!(broken.mods.brand_color, "#0066ff");
        let bak = dir.join("config.json.bak");
        assert!(bak.exists());
        assert_eq!(std::fs::read(&bak).unwrap(), corrupt);

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
        let c: Config =
            serde_json::from_str(r#"{"mods":{"autoEnableSuspectedFnOS":false}}"#).unwrap();
        assert!(!c.mods.auto_enable_suspected_fnos);
        // local 段键名同样对齐上游（cs:108-109、cs:55）
        let lv = serde_json::to_value(LocalConfig::default()).unwrap();
        let mut lgot: Vec<&str> = lv.as_object().unwrap().keys().map(String::as_str).collect();
        lgot.sort_unstable();
        assert_eq!(
            lgot,
            vec!["customCssCode", "customJsCode", "loginWallpaperFileName"]
        );
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
    fn nas_url_origin_joins_whitelist() {
        assert_eq!(
            origin_of("http://192.168.1.10:5666/webui/"),
            Some("http://192.168.1.10:5666".into())
        );
        assert_eq!(
            origin_of("https://abc.fnos.net"),
            Some("https://abc.fnos.net".into())
        );
        assert_eq!(origin_of("not a url"), None);

        let mut c = Config::default();
        c.shell.nas_url = "HTTP://Nas.Local:8000/".into();
        c.normalize();
        assert!(c
            .mods
            .enabled_origins
            .iter()
            .any(|o| o == "http://nas.local:8000"));

        // 幂等：再次归一化不得重复添加
        c.normalize();
        assert_eq!(
            c.mods
                .enabled_origins
                .iter()
                .filter(|o| o.contains("nas.local"))
                .count(),
            1
        );
    }

    /// §6.5 要求**逐字段**回退：单个键类型不对不得丢掉整份配置。
    /// 上游 `chrome.storage.sync` 的值常见 `null` / 字符串（§6.1 允许直接粘贴浏览器扩展配置）。
    #[test]
    fn lenient_load_coerces_bad_types() {
        let dir = temp_dir("lenient");
        let p = dir.join("config.json");
        std::fs::write(
            &p,
            br##"{
              "schemaVersion": "1",
              "mods": {
                "brandColor": "#123456",
                "desktopIconPerColumn": "8",
                "autoEnableSuspectedFnOS": "false",
                "titlebarStyle": null,
                "launchpadStyle": "spotlight",
                "fontFamily": "My Font",
                "enabledOrigins": [1, "http://a", " "],
                "desktopIconPerColumnEnabled": null,
                "launchpadIconRedrawMap": {"a": "prefect_icon/emby.png", "b": "../x", "c": 1}
              },
              "local": { "customCssCode": "body{}", "loginWallpaperFileName": null },
              "shell": {
                "homeUrl": "http://nas.local/",
                "injectEnabled": "true",
                "closeToTray": "false",
                "window": { "w": "1200", "h": 900, "x": null, "y": "30" }
              }
            }"##,
        )
        .unwrap();
        let c = Config::load(&p);

        // 整份配置必须仍在（这些值都不是默认值）——单个坏类型键不得触发 wholesale reset
        assert_eq!(c.mods.font_family, "My Font");
        assert_eq!(c.mods.launchpad_style, "spotlight");
        assert_eq!(c.shell.home_url, "http://nas.local/");
        assert_eq!(c.local.custom_css_code, "body{}");
        // 字符串 → 保留；再按 §6.5 夹明度：#123456 的 L≈20.4% → 30% → #1a4d7f（≠ 默认色 ⇒ 未被删除）
        assert_eq!(c.mods.brand_color, "#1a4d7f");
        assert_ne!(c.mods.brand_color, DEFAULT_BRAND_COLOR);
        // 数字/数字字符串 → 数字；null → 删除 → #[serde(default)] = 8
        assert_eq!(c.mods.desktop_icon_per_column, 8);
        // 字符串 bool → bool：默认是 true，故 "false" 能区分「被强制转换」与「被删除后回落默认」
        assert!(!c.mods.auto_enable_suspected_fnos);
        // null 字符串 → 删除 → 默认 windows
        assert_eq!(c.mods.titlebar_style, "windows");
        // string[]：只保留 trim 后非空的字符串元素
        assert_eq!(c.mods.enabled_origins, vec!["http://a".to_string()]);
        // 遗留三态键：null → 删除 → None
        assert_eq!(c.mods.desktop_icon_per_column_enabled, None);
        // map：只保留 string→string；"../x" 再由 §6.5 的正则剔除
        assert_eq!(c.mods.launchpad_icon_redraw_map.len(), 1);
        assert_eq!(
            c.mods
                .launchpad_icon_redraw_map
                .get("a")
                .map(String::as_str),
            Some("prefect_icon/emby.png")
        );
        // shell bool："true" → true（默认值相同，仅证明不丢键）；"false" → false（可区分）
        assert!(c.shell.inject_enabled);
        assert!(!c.shell.close_to_tray);
        // window：数字字符串 / 数字 / null / 数字字符串
        assert_eq!(c.shell.window.w, 1200.0);
        assert_eq!(c.shell.window.h, 900.0);
        assert_eq!(c.shell.window.x, None);
        assert_eq!(c.shell.window.y, Some(30.0));
        // local：null 保留为 None；字符串保留
        assert_eq!(c.local.login_wallpaper_file_name, None);
        // "1" 字符串 → 数字 → normalize 固定为 SCHEMA_VERSION
        assert_eq!(c.schema_version, SCHEMA_VERSION);
        // 宽松加载成功 ⇒ 不该产生 .bak
        assert!(!dir.join("config.json.bak").exists());

        // 数字分支的 variant：证明浮点/数字字符串真的被转换，而不是删除后回落默认 8。
        // 8.5 → 9（f64::round 半值远离零）；「非数字 → 8」按 §6.5 由 null 一例覆盖。
        for (raw, want) in [("8.5", 9u32), ("\"12\"", 12), ("null", 8)] {
            std::fs::write(
                &p,
                format!(
                    r#"{{"mods":{{"desktopIconPerColumn":{raw}}},"shell":{{"homeUrl":"keep"}}}}"#
                ),
            )
            .unwrap();
            let c = Config::load(&p);
            assert_eq!(c.mods.desktop_icon_per_column, want, "raw={raw}");
            // 单个坏类型键不得丢掉同文件里的其它键
            assert_eq!(c.shell.home_url, "keep", "raw={raw}");
        }

        // 段本身不是对象（如 `"mods": 5`）：该段全部走默认值，其它段照旧读入
        std::fs::write(&p, br#"{"mods":5,"shell":{"homeUrl":"keep"}}"#).unwrap();
        let c = Config::load(&p);
        assert_eq!(c.mods.desktop_icon_per_column, 8);
        assert_eq!(c.mods.font_face_name, "FnOSCustomFont");
        assert_eq!(c.shell.home_url, "keep");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Notepad 默认保存的 UTF-8 带 BOM（EF BB BF）不得被当成损坏文件。
    #[test]
    fn bom_prefixed_file_loads() {
        let dir = temp_dir("bom");
        let p = dir.join("config.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            r##"{"mods":{"brandColor":"#336699","fontFamily":"BOM 字体"}}"##.as_bytes(),
        );
        std::fs::write(&p, &bytes).unwrap();

        let c = Config::load(&p);
        assert_eq!(c.mods.font_family, "BOM 字体");
        assert_eq!(c.mods.brand_color, "#336699");
        assert_eq!(c.schema_version, SCHEMA_VERSION);
        // 剥 BOM 后解析成功 ⇒ 不算损坏，不写 .bak
        assert!(!dir.join("config.json.bak").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 读/解码失败（非 UTF-8、UTF-16、文件被占用）同样必须留证：拷原件到 `<path>.bak`。
    #[test]
    fn read_error_writes_bak() {
        let dir = temp_dir("readerr");
        let p = dir.join("config.json");
        let corrupt: &[u8] = &[0xFF, 0xFE, 0x41, 0x00]; // UTF-16LE BOM + 'A'：非法 UTF-8
        std::fs::write(&p, corrupt).unwrap();

        let c = Config::load(&p);
        assert_eq!(c.mods.brand_color, DEFAULT_BRAND_COLOR); // 回落默认
        assert_eq!(c.mods.desktop_icon_per_column, 8);
        let bak = dir.join("config.json.bak");
        assert!(bak.exists(), "读错误未留证");
        assert_eq!(std::fs::read(&bak).unwrap(), corrupt); // 原件字节完整保留

        // 文件不存在：回落默认，且无可留证内容 → 不产生 .bak
        let missing = dir.join("nope.json");
        let d = Config::load(&missing);
        assert_eq!(d.mods.brand_color, DEFAULT_BRAND_COLOR);
        assert!(!dir.join("nope.json.bak").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 合法枚举值、以及 4/16 边界必须原样通过；越界才夹取。
    #[test]
    fn enum_valid_values_pass_through() {
        let dir = temp_dir("enums");
        let p = dir.join("config.json");
        for (raw, want) in [("4", 4u32), ("16", 16), ("3", 4), ("17", 16)] {
            std::fs::write(
                &p,
                format!(
                    r#"{{"mods":{{"desktopIconPerColumn":{raw},"titlebarStyle":"mac","launchpadStyle":"spotlight","desktopIconLayoutMode":"fixed"}}}}"#
                ),
            )
            .unwrap();
            let c = Config::load(&p);
            assert_eq!(c.mods.desktop_icon_per_column, want, "raw={raw}");
            assert_eq!(c.mods.titlebar_style, "mac");
            assert_eq!(c.mods.launchpad_style, "spotlight");
            assert_eq!(c.mods.desktop_icon_layout_mode, "fixed");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 上游 `^prefect_icon/[a-z0-9-]+\.png$`（cs:361/515）：穿越、双扩展名、大写都必须被剔除。
    #[test]
    fn redraw_map_regex_is_enforced() {
        assert!(is_valid_prefect_icon_path("prefect_icon/emby.png"));
        assert!(is_valid_prefect_icon_path(
            "prefect_icon/home-assistant.png"
        ));
        assert!(!is_valid_prefect_icon_path("../x"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/a.png.png"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/Emby.png"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/emby.PNG"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/sub/dir.png"));
        assert!(!is_valid_prefect_icon_path("other/emby.png"));

        let mut c = Config::default();
        for (k, v) in [
            ("ok", "prefect_icon/emby.png"),
            ("up", "prefect_icon/Emby.png"),
            ("dots", "prefect_icon/a.png.png"),
            ("esc", "../x"),
            ("sub", "prefect_icon/sub/dir.png"),
        ] {
            c.mods.launchpad_icon_redraw_map.insert(k.into(), v.into());
        }
        c.normalize();
        let keys: Vec<&str> = c
            .mods
            .launchpad_icon_redraw_map
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["ok"]);
        assert_eq!(
            c.mods.launchpad_icon_redraw_map["ok"],
            "prefect_icon/emby.png"
        );
    }

    /// 同一路径写两次：后写胜出、文件仍是合法 JSON、不留 `.tmp`。
    #[test]
    fn save_overwrites_existing_file() {
        let dir = temp_dir("save");
        let p = dir.join("config.json");
        let mut c = Config::default();
        c.mods.font_family = "first".into();
        c.save(&p).unwrap();
        c.mods.font_family = "second".into();
        c.save(&p).unwrap();

        let text = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["mods"]["fontFamily"], "second");
        assert_eq!(Config::load(&p).mods.font_family, "second");
        assert!(!dir.join("config.json.tmp").exists());
        assert!(!dir.join("config.json.bak").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 契约是 `scheme://host[:port]`：authority 里的 userinfo 必须剥掉，
    /// 否则白名单会留下 `http://user:pass@host` 这种永远匹配不上 `location.origin` 的条目。
    #[test]
    fn origin_of_strips_userinfo() {
        assert_eq!(
            origin_of("http://user:pass@host/x"),
            Some("http://host".into())
        );
        assert_eq!(
            origin_of("HTTP://User:Pass@Nas.Local:8000/webui/"),
            Some("http://nas.local:8000".into())
        );
        assert_eq!(
            origin_of("https://user@abc.fnos.net"),
            Some("https://abc.fnos.net".into())
        );
        // 密码里含 '@' → 取最后一个 '@' 之后的段落
        assert_eq!(
            origin_of("http://a@b@host:1/"),
            Some("http://host:1".into())
        );
        // userinfo 之后没有 host
        assert_eq!(origin_of("http://user@"), None);
        // 回归：无 userinfo 的老行为不变
        assert_eq!(
            origin_of("http://192.168.1.10:5666/webui/"),
            Some("http://192.168.1.10:5666".into())
        );
        assert_eq!(origin_of("not a url"), None);

        let o = origin_of("http://user:pass@host/x").unwrap();
        assert!(!o.contains("user") && !o.contains('@'));
    }

    /// 上游按 `location.origin` **大小写敏感**比较（cs:2948），
    /// 故白名单项必须 trim + 小写，且大小写不敏感去重（R4 的 NAS 并入仍须幂等）。
    #[test]
    fn enabled_origins_are_trimmed_and_lowercased() {
        let mut c = Config::default();
        c.mods.enabled_origins = vec![
            " HTTP://NAS.LOCAL:8000 ".into(),
            "http://nas.local:8000".into(), // 与上一条重复（忽略大小写/空白）
            "   ".into(),                   // 空白项丢弃
            "https://FnOS.net".into(),
            "\thttps://fnos.net\n".into(), // 与上一条重复
        ];
        c.normalize();
        assert_eq!(
            c.mods.enabled_origins,
            vec!["http://nas.local:8000", "https://fnos.net"]
        );

        // R4 仍成立：nasUrl 的 origin 并入白名单；与已有的大写条目视为同一条 → 不重复、幂等
        let mut c = Config::default();
        c.mods.enabled_origins = vec!["HTTP://NAS.LOCAL:8000".into()];
        c.shell.nas_url = "http://nas.local:8000/webui/".into();
        c.normalize();
        c.normalize();
        assert_eq!(c.mods.enabled_origins, vec!["http://nas.local:8000"]);
    }
}
