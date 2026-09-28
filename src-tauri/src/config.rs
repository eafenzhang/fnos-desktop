use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use url::Url;

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

/// 主窗口尺寸的合法区间（Item 2）。
///
/// 归一化是窗口几何的**唯一校验入口**（`WindowGeom::clamp_to_usable`，由 `Config::normalize`
/// 调用）：`0x0` / 负数 / 超大值都不允许渗给消费方。round 1 只在 `tray::apply_window_geom`
/// 加了消费侧防御，而 `commands::build_main_window` 的 `.inner_size(w, h)` 是**更早**的第一次
/// 消费，于是配置里历史遗留的 `0x0` 仍然会建出一张 0 尺寸的窗口。
pub const MIN_WINDOW_W: f64 = 480.0;
pub const MIN_WINDOW_H: f64 = 360.0;
pub const MAX_WINDOW_W: f64 = 16384.0;
pub const MAX_WINDOW_H: f64 = 16384.0;

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

/// 单维夹取：非有限（`NaN` / `inf`）或 `<= 0` → 默认值；否则夹到 `[min, max]`。
///
/// `<= 0` 一律当「没有值」而不是「夹到下限」：`0x0` 是历史缺陷写坏配置的指纹
///（最小化窗口的 `inner_size` 就是 0），负数更是纯粹的坏值，两者都该回到默认几何。
fn usable_window_dim(value: f64, default: f64, min: f64, max: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        default
    } else {
        value.clamp(min, max)
    }
}

impl WindowGeom {
    /// 把 `w` / `h` 夹成**任何消费方都能直接用**的几何（`0x0` / 负数 → 默认 1200×820，
    /// 越界 → `[MIN_WINDOW_*, MAX_WINDOW_*]`）；`x` / `y` 不在这里处理——它们的防御在
    /// `tray::apply_window_geom`（`-32000` 是 Windows 给最小化窗口的坐标哨兵值）。
    pub fn clamp_to_usable(&mut self) {
        let default = WindowGeom::default();
        self.w = usable_window_dim(self.w, default.w, MIN_WINDOW_W, MAX_WINDOW_W);
        self.h = usable_window_dim(self.h, default.h, MIN_WINDOW_H, MAX_WINDOW_H);
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

/// 是否为**可直接交给 WebView 的绝对 http(s) URL**（判定前先 trim）。
///
/// Finding 1：`homeUrl` / `nasUrl` 是用户可控文本（`open_config_dir` 明示了配置文件路径，
/// 手改是预期用法），而 round 0 只检查「非空」，于是 `"not a url"` 能一路走到
/// `main.rs` 的 `cfg.shell.home_url.parse().expect("home url")` —— 建窗即 panic；
/// release 下 R9 隐藏了控制台，表现就是「双击无反应」。这里集中做真正的校验：
///
/// - `javascript:alert(1)` / `file:///…` / `data:…` → scheme 不是 http(s) → `None`
/// - `"not a url"` / `"http://"` / `"http:"` → 解析失败或没有 host → `None`
///
/// 返回的 `Url` 与 `tauri::Url` 是同一个类型（tauri 直接 re-export `url::Url`）。
pub fn parse_web_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str().map_or(true, str::is_empty) {
        return None;
    }
    Some(url)
}

impl ShellConfig {
    /// 主页面地址：`normalize` 后必定合法；这里仍然再校验一次并回落 `DEFAULT_HOME_URL`，
    /// 因此**任何消费方都可以无 panic 地取用**（Finding 1：不许再对用户文本 `expect`）。
    pub fn home_url_or_default(&self) -> &str {
        let trimmed = self.home_url.trim();
        if parse_web_url(trimmed).is_some() {
            trimmed
        } else {
            DEFAULT_HOME_URL
        }
    }

    /// NAS WebUI 地址的解析结果：`None` = 未配置**或填错了**。
    pub fn nas_url_parsed(&self) -> Option<Url> {
        parse_web_url(&self.nas_url)
    }

    /// NAS WebUI 地址（trim 后的原文）：`None` = 未配置**或填错**。
    ///
    /// Finding 1 选定的语义是「**保留原文但一律禁用**」：`normalize` 不删用户写错的值
    /// （用户能在配置文件里看到自己的错字并改回来），但凡是要*使用*它的地方——托盘的
    /// 「打开 NAS」置灰（`tray::sync_menus`）、`commands::open_nas` 的跳转——都只看这个
    /// 取值器。于是「非法 nasUrl」在所有入口都一致地表现为「没有可用地址」，
    /// 不会再出现 round 0 那种「菜单可点、点了静默什么都不做」。
    pub fn nas_target(&self) -> Option<&str> {
        self.nas_url_parsed()?;
        Some(self.nas_url.trim())
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

/// 品牌色明度的合法区间（HSL 的 L，取值 0.0–1.0）。
const BRAND_LIGHTNESS_MIN: f64 = 0.30;
const BRAND_LIGHTNESS_MAX: f64 = 0.70;

/// 「已经在区间内」的判定必须带上的 8 位量化容差。
///
/// 夹取把 `l` 钉在边界上，再由 `to()` 把三个通道各自四舍五入到 8 位：每个通道最多偏
/// 0.5/255，于是**夹取结果重新算出来的 `l` 最多偏离边界 0.5/255**。若判据写成严格的
/// `[0.30, 0.70]`，夹取结果就不是不动点：
///
/// - `#ffffff` → `#b3b3b3`（`l = 179/255 = 0.70196 > 0.70`）——它「看起来稳定」只是因为
///   灰度重算后仍落在 179；
/// - `#cec1b2` → `#c4b4a2`（同一个 `0.70196`）——再夹一次就变成 `#c4b4a1`，于是
///   `Config::save`（写盘前再 `normalize()` 一次）落盘的值与内存/生效值差一个通道。
///
/// 1/255 是 0.5/255 的两倍，留出浮点误差余量。区间因此实际判定为
/// `[0.30 - 1/255, 0.70 + 1/255]`：这是「不许改动夹取结果」的**必要条件**——
/// 要让 `#ffffff → #b3b3b3` 保持不变，`0.70196` 就必须被算作「已在区间内」。
const BRAND_LIGHTNESS_EPSILON: f64 = 1.0 / 255.0;

/// `#rrggbb`（或 `#rgb`）→ HSL：明度不在 30%–70% 时夹到边界再转回 `#rrggbb`，
/// **已经在区间内（含 [`BRAND_LIGHTNESS_EPSILON`] 的量化容差）时原样返回**规范化后的输入。
/// 非法输入回落默认色。输出永远是小写 6 位。
///
/// **幂等（不动点）**：`f(f(x)) == f(x)` 对任意输入成立，由构造保证——区间内直接返回
/// `x` 的规范写法；区间外的返回值其明度最多偏离边界 0.5/255 < `BRAND_LIGHTNESS_EPSILON`，
/// 故第二次调用必然走「原样返回」这一支。
///
/// 这条不变式是 `Config::save` 敢在写盘前再 `normalize()` 一次的前提，也是 §8.4
///「显示即生效」的前提：内存、注入载荷、磁盘必须是同一个字符串。
///
/// - `#cec1b2`（L=75.3%）→ 夹取 → `#c4b4a2`，再作用一次仍是 `#c4b4a2`
///   （修前是 `#c4b4a1`：磁盘与生效值分叉）
/// - `#ffffff` → `#b3b3b3`、`#000000` → `#4d4d4d`、`bogus` → `#0066ff`（越界/非法行为不变）
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
    let (ru, gu, bu) = match (
        u8::from_str_radix(&expanded[0..2], 16),
        u8::from_str_radix(&expanded[2..4], 16),
        u8::from_str_radix(&expanded[4..6], 16),
    ) {
        (Ok(r), Ok(g), Ok(b)) => (r, g, b),
        _ => return DEFAULT_BRAND_COLOR.into(),
    };
    // 输入的规范写法（小写 6 位，`#06f` → `#0066ff`）。故意用解析出的字节重新格式化，
    // 而不是给输入做小写化：`u8::from_str_radix` 接受前导 `+`（`"#+f0000"` 能解析成
    // `#0f0000`），直接小写输入会把 `+` 原样带进结果。
    let canonical = format!("#{ru:02x}{gu:02x}{bu:02x}");
    let (r, g, b) = (ru as f64 / 255.0, gu as f64 / 255.0, bu as f64 / 255.0);

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    // 幂等的关键一步：区间内（含量化容差）原样返回，夹取结果因此必是不动点。
    if (BRAND_LIGHTNESS_MIN - BRAND_LIGHTNESS_EPSILON
        ..=BRAND_LIGHTNESS_MAX + BRAND_LIGHTNESS_EPSILON)
        .contains(&l)
    {
        return canonical;
    }

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
    let l = l.clamp(BRAND_LIGHTNESS_MIN, BRAND_LIGHTNESS_MAX);

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

/// 从 URL 得到 origin（`scheme://host[:port]`，scheme/host 小写、省略默认端口）。
///
/// 与浏览器的 `location.origin` 对齐（Finding 1 顺带修正）：`Origin::ascii_serialization`
/// 会剥掉 userinfo、丢掉 `:80` / `:443` 这类默认端口——旧的手写实现会把
/// `http://user:pass@host:80/` 变成 `http://user:pass@host:80` 这种永远匹配不上
/// `location.origin` 的条目。非法或非 http(s) URL → `None`。
pub fn origin_of(url: &str) -> Option<String> {
    Some(parse_web_url(url)?.origin().ascii_serialization())
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
        let nas_origin = self.shell.nas_target().and_then(origin_of);
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
        // Finding 1：`homeUrl` 必须是可导航的绝对 http(s) URL，否则回落默认常量。
        // round 0 只判空，于是 `"not a url"` / `"javascript:alert(1)"` 会一路走到
        // `main.rs` 的 `.parse().expect("home url")` → release 下静默启动失败。
        // （本段在 `m` 的可变借用结束之后，故可安全改 `self.shell`。）
        match parse_web_url(&self.shell.home_url) {
            Some(_) => self.shell.home_url = self.shell.home_url.trim().to_string(),
            None => self.shell.home_url = DEFAULT_HOME_URL.into(),
        }
        // Item 2：窗口几何在这里夹取（`load` 与 `save` 都必经 `normalize`，
        // `set_config` / `reset_config` / `set_inject_enabled` 也各自显式调用），因此
        // `commands::build_main_window` 的 `inner_size(cfg.shell.window.w, h)` 拿到的
        // 一定是可用几何——历史遗留的 `0x0` 不会再建出 0 尺寸窗口。
        self.shell.window.clamp_to_usable();
        // 保存 NAS WebUI 地址时自动把其 origin 并入注入白名单（跳过 1.5s 探测），幂等。
        // `nas_target()` 先做完整校验：非法的 `nasUrl`（如 `javascript:…`）不得混进白名单，
        // 否则会往非飞牛页面注入。
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

    /// 配置文件绝对路径（`paths::config_dir()/config.json`）。Task 7/8 接线后由
    /// `commands.rs` 的读写路径统一消费——顺带让 `#[allow(dead_code)]` 可以摘掉（R18）。
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

    /// 写盘（先写 `.tmp` 再 rename，避免半截文件）。
    ///
    /// Finding 1「归一化/落盘处校验」：写之前再 `normalize()` 一次（幂等，clone 一份改，
    /// 不动调用方）。所有写路径的必经之处就是这里，因此**磁盘上的 URL 永远是合法的**——
    /// 内存里被谁塞了非法值也不可能落盘成「下次启动即 panic」的配置。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut cfg = self.clone();
        cfg.normalize();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&cfg).unwrap())?;
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

    /// 缺陷（fix round 2）：`normalize_brand_color` 必须是不动点。
    ///
    /// 修前 `#c4b4a2`（L = 179/255 = 0.70196，只比上界高 0.5/255）会被**再夹一次**成
    /// `#c4b4a1`，而 `Config::save` 写盘前正好会再 `normalize()` 一次——落盘值与内存/
    /// 注入载荷/页面生效值就此差一个通道（§8.4「显示即生效」被破坏）。
    ///
    /// 任务书里写的 `normalize_brand_color("#cec1b2") == "#cec1b2"` 在算术上不成立：
    /// `#cec1b2` 的 HSL 明度是 **75.3%**，本就在 `[0.30, 0.70]` 之外，必须被夹一次成
    /// `#c4b4a2`（否则「明度夹到 30%–70%」的既有契约失效，也与 `commands.rs` 的
    /// finding-A 断言冲突）。真正要锁的不变式是「**已经归一化**的值再归一化不变」，
    /// 即下面的不动点断言 + `brand_color_clamp_is_a_fixed_point_over_representative_colors`。
    #[test]
    fn brand_color_clamp_is_idempotent() {
        // 越界输入的单次夹取行为逐条不变（含任务书点名的四个）
        assert_eq!(normalize_brand_color("#cec1b2"), "#c4b4a2"); // L=75.3% → 70% → 单次夹取
        assert_eq!(normalize_brand_color("#ffffff"), "#b3b3b3");
        assert_eq!(normalize_brand_color("#000000"), "#4d4d4d");
        assert_eq!(normalize_brand_color("bogus"), "#0066ff");
        assert_eq!(normalize_brand_color("#06f"), "#0066ff");
        // 夹取结果必须是不动点（本轮修的就是这三行里的前两行）
        assert_eq!(normalize_brand_color("#c4b4a2"), "#c4b4a2");
        assert_eq!(normalize_brand_color("#b3b3b3"), "#b3b3b3");
        assert_eq!(normalize_brand_color("#4d4d4d"), "#4d4d4d");
        // 区间内的输入原样返回，只做「小写 + 补成 6 位」的规范化
        assert_eq!(normalize_brand_color("#0066ff"), "#0066ff");
        assert_eq!(normalize_brand_color("#06F"), "#0066ff");
        assert_eq!(normalize_brand_color("  #06f  "), "#0066ff");
        assert_eq!(normalize_brand_color("0066FF"), "#0066ff");
        assert_eq!(normalize_brand_color("#3366CC"), "#3366cc");
    }

    /// 不动点性质 `f(f(x)) == f(x)`（以及输出形状恒为小写 6 位十六进制）在代表性颜色集上
    /// 成立：全部 256 档灰度（夹取边界正好落在灰度带上，最容易破坏不动点）、通道极值组合、
    /// 缺陷色本身，以及一个确定性 LCG 抽出的 15 万个随机色。
    ///
    /// 旧的（非幂等）实现每几千个越界随机色就有反例（fix round 1 的评审在 198,829 个随机色
    /// 里量到 43 个），这个规模足以覆盖；LCG 无外部依赖，失败可复现。
    #[test]
    fn brand_color_clamp_is_a_fixed_point_over_representative_colors() {
        fn check(raw: &str) {
            let once = normalize_brand_color(raw);
            let twice = normalize_brand_color(&once);
            assert_eq!(twice, once, "夹取不是不动点：{raw} -> {once} -> {twice}");
            assert_eq!(once.len(), 7, "输出必须是 6 位：{raw} -> {once}");
            assert!(once.starts_with('#'), "输出必须带 #：{raw} -> {once}");
            assert_eq!(
                once,
                once.to_ascii_lowercase(),
                "输出必须小写：{raw} -> {once}"
            );
            assert!(
                once[1..].bytes().all(|b| b.is_ascii_hexdigit()),
                "输出必须是十六进制：{raw} -> {once}"
            );
        }

        let mut cases = 0usize;
        for v in 0u16..=255 {
            check(&format!("#{v:02x}{v:02x}{v:02x}"));
            cases += 1;
        }
        for (r, g, b) in [
            (0, 0, 0),
            (255, 255, 255),
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
            (255, 255, 0),
            (0, 255, 255),
            (255, 0, 255),
            (1, 2, 3),
            (254, 253, 252),
            (0xce, 0xc1, 0xb2), // 缺陷色
            (0xc4, 0xb4, 0xa2), // 单次夹取的结果
            (0xc4, 0xb4, 0xa1), // 修前被二次夹取出来的值
        ] {
            check(&format!("#{r:02x}{g:02x}{b:02x}"));
            cases += 1;
        }
        // 确定性 LCG（Numerical Recipes 常数）：无外部依赖，失败可复现
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..150_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            check(&format!(
                "#{:02x}{:02x}{:02x}",
                (state >> 40) as u8,
                (state >> 24) as u8,
                (state >> 8) as u8
            ));
            cases += 1;
        }
        assert_eq!(cases, 256 + 13 + 150_000);
    }

    /// 穷举**全部** 16,777,216 个 `#rrggbb`（16 核机上实测 75 秒），默认不跑：不动点性质
    /// 在整个色彩空间上成立，而不只是代表性抽样。需要完整证据时单独跑：
    /// `cargo test -- --ignored brand_color_clamp_is_idempotent_exhaustively`
    #[test]
    #[ignore = "穷举 16.7M 色约 75 秒；作为代表性抽样的补充证据按需运行"]
    fn brand_color_clamp_is_idempotent_exhaustively() {
        let mut mismatches: Vec<String> = Vec::new();
        for v in 0u32..=0x00ff_ffff {
            let raw = format!("#{v:06x}");
            let once = normalize_brand_color(&raw);
            let twice = normalize_brand_color(&once);
            if twice != once {
                mismatches.push(format!("{raw} -> {once} -> {twice}"));
                if mismatches.len() >= 8 {
                    break;
                }
            }
        }
        assert!(mismatches.is_empty(), "不动点反例：{mismatches:?}");
    }

    /// fix round 2 的验收不变式：`save()` 写盘前会 `normalize()` 一份 clone，只要
    /// `normalize_brand_color` 是不动点，「内存 == 磁盘 == 再 load 回来」就由构造成立。
    /// 修前 `#c4b4a2` 会在写盘这一步变成 `#c4b4a1`（磁盘与生效值分叉，重启后再 load 甚至
    /// 会让生效值也跟着掉一个通道）。
    #[test]
    fn save_round_trips_brand_color_byte_identically() {
        let dir = temp_dir("brand-roundtrip");
        let p = dir.join("config.json");
        let mut c = Config::default();
        c.mods.brand_color = "#cec1b2".into(); // 手工 / 历史遗留的越界值
        c.normalize(); // = `Config::load` 的必经之路
        let applied = c.mods.brand_color.clone();
        assert_eq!(applied, "#c4b4a2", "单次夹取的结果");

        c.save(&p).unwrap();
        assert_eq!(c.mods.brand_color, applied, "save() 不得改动内存里的生效值");

        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("\"brandColor\": \"#c4b4a2\""),
            "磁盘上的值必须与生效值逐字节相同：{text}"
        );
        assert!(!text.contains("#c4b4a1"), "二次夹取的值不得落盘：{text}");
        assert_eq!(Config::load(&p).mods.brand_color, applied);

        // 反复 save → load 不再漂移（修前第二次读回来就是 #c4b4a1）
        for _ in 0..3 {
            Config::load(&p).save(&p).unwrap();
        }
        assert_eq!(Config::load(&p).mods.brand_color, applied);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 任务书要求的手写 `config.json` 场景：`#cec1b2` 是越界值（L=75.3%），第一次 `load`
    /// 夹成 `#c4b4a2`，此后 load → save → load 全程保持 `#c4b4a2`（修前 `save` 会写成
    /// `#c4b4a1`）。顺带钉住「区间内的手写值原样落盘、只做小写规范化」。
    #[test]
    fn hand_written_brand_color_config_does_not_drift() {
        let dir = temp_dir("brand-handwritten");
        let p = dir.join("config.json");
        std::fs::write(
            &p,
            r##"{"schemaVersion":1,"mods":{"brandColor":"#cec1b2"}}"##,
        )
        .unwrap();

        let first = Config::load(&p);
        assert_eq!(first.mods.brand_color, "#c4b4a2");
        first.save(&p).unwrap();
        let disk = std::fs::read_to_string(&p).unwrap();
        assert!(disk.contains("\"brandColor\": \"#c4b4a2\""), "{disk}");
        assert!(!disk.contains("#c4b4a1"), "{disk}");

        let second = Config::load(&p);
        assert_eq!(second.mods.brand_color, "#c4b4a2");
        second.save(&p).unwrap();
        assert_eq!(Config::load(&p).mods.brand_color, "#c4b4a2");

        // 已经归一化的手写值（区间内）也必须原样落盘
        std::fs::write(
            &p,
            r##"{"schemaVersion":1,"mods":{"brandColor":"#3366CC"}}"##,
        )
        .unwrap();
        let upper = Config::load(&p);
        assert_eq!(upper.mods.brand_color, "#3366cc");
        upper.save(&p).unwrap();
        assert!(std::fs::read_to_string(&p)
            .unwrap()
            .contains("\"brandColor\": \"#3366cc\""));
        std::fs::remove_dir_all(&dir).ok();
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
        // Finding 1：非 http(s) 的 scheme 不再当成 origin（`javascript:` 永远不该进白名单）
        assert_eq!(origin_of("javascript:alert(1)"), None);
        assert_eq!(origin_of("file:///C:/x"), None);
        // 与 `location.origin` 对齐：默认端口要省略（旧实现会留下 `:80`，永远匹配不上）
        assert_eq!(origin_of("http://host:80/x"), Some("http://host".into()));
        assert_eq!(origin_of("https://host:443/x"), Some("https://host".into()));
        assert_eq!(
            origin_of("http://host:8080/x"),
            Some("http://host:8080".into())
        );

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

    /// 非法 URL 合集：scheme 不对（`javascript:` / `file:` / `data:`）、解析不了
    /// （`"not a url"`）、没有 host（`"http://"`）、空白。Finding 1 的判定基准。
    const MALFORMED_URLS: [&str; 8] = [
        "not a url",
        "javascript:alert(1)",
        "file:///C:/Windows/System32/calc.exe",
        "data:text/html,<script>alert(1)</script>",
        "http://",
        "http:",
        "   ",
        "",
    ];

    /// Finding 1（Important）：手改的 `homeUrl` 不得让应用启动即 panic —— 非法值一律回落
    /// 默认常量，且 `main.rs` 用的取值器本身也永不出错。
    #[test]
    fn malformed_home_url_falls_back_to_default() {
        for raw in MALFORMED_URLS {
            let mut c = Config::default();
            c.shell.home_url = raw.into();
            c.normalize();
            assert_eq!(c.shell.home_url, DEFAULT_HOME_URL, "raw={raw:?}");
            assert_eq!(
                c.shell.home_url_or_default(),
                DEFAULT_HOME_URL,
                "raw={raw:?}"
            );
            // 建窗路径（`commands::resolve_main_url`）依赖这条：取出来的串一定可解析
            assert!(parse_web_url(c.shell.home_url_or_default()).is_some());
        }

        // 合法值（含首尾空白）必须原样通过 → 证明上面的回落不是「一律重置」
        for raw in [
            " https://fnos.net/ ",
            "http://192.168.1.10:5666/webui/",
            "https://abc.fnos.net/app",
        ] {
            let mut c = Config::default();
            c.shell.home_url = raw.into();
            c.normalize();
            assert_eq!(c.shell.home_url, raw.trim(), "raw={raw:?}");
            assert_eq!(c.shell.home_url_or_default(), raw.trim(), "raw={raw:?}");
        }
    }

    /// 非法 `homeUrl` 从文件读到内存的整条链路都要回落（`load` 里是 sanitize → normalize）。
    #[test]
    fn malformed_home_url_in_file_falls_back_on_load() {
        let dir = temp_dir("badhome");
        let p = dir.join("config.json");
        std::fs::write(
            &p,
            br##"{"shell":{"homeUrl":"javascript:alert(1)","injectEnabled":true}}"##,
        )
        .unwrap();
        let c = Config::load(&p);
        assert_eq!(c.shell.home_url, DEFAULT_HOME_URL);
        // 同文件里的其它键不受牵连
        assert!(c.shell.inject_enabled);

        // 非字符串类型（对象/数组）走 `sanitize_string_field` 删除 → 默认值，同样不是 panic
        std::fs::write(&p, br#"{"shell":{"homeUrl":{"a":1}}}"#).unwrap();
        assert_eq!(Config::load(&p).shell.home_url, DEFAULT_HOME_URL);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Finding 1：非法 `nasUrl` 一律视为「未配置」——托盘的「打开 NAS」据此置灰
    /// （`tray::sync_menus` 的唯一判据就是这个 `nas_target()`），`commands::open_nas`
    /// 也走同一取值器，因此不会再出现「菜单可点、点了静默无事发生」。
    #[test]
    fn malformed_nas_url_disables_tray_item() {
        for raw in MALFORMED_URLS {
            let mut c = Config::default();
            c.shell.nas_url = raw.into();
            c.normalize();
            // 这就是托盘「打开 NAS」置灰的判据本身：`tray::sync_menus` 里写的是
            // `item.set_enabled(cfg.shell.nas_target().is_some())`
            assert_eq!(c.shell.nas_target(), None, "raw={raw:?}");
            // 「保留原文但禁用」：用户输入不被静默删除（与 `homeUrl` 的回落策略不同，
            // 原因见 `ShellConfig::nas_target` 的文档）
            assert_eq!(c.shell.nas_url, raw, "raw={raw:?}");
            // 非法 nasUrl 不得混进注入白名单
            assert!(c.mods.enabled_origins.is_empty(), "raw={raw:?}");
        }

        // 合法值：托盘可点 + origin 并入白名单
        let mut c = Config::default();
        c.shell.nas_url = " http://192.168.1.10:5666/webui/ ".into();
        c.normalize();
        assert_eq!(
            c.shell.nas_target(),
            Some("http://192.168.1.10:5666/webui/")
        );
        assert!(c.shell.nas_url_parsed().is_some());
        assert_eq!(c.mods.enabled_origins, vec!["http://192.168.1.10:5666"]);
    }

    /// Item 2：窗口几何的夹取规则本身（`normalize` 是唯一入口）。
    /// `0x0` / 负数 → 默认几何；越界 → 上下限；合法值（含边界值）原样通过。
    #[test]
    fn window_geometry_is_clamped_to_usable_range() {
        for (w, h, want_w, want_h) in [
            (0.0, 0.0, 1200.0, 820.0),              // 历史坏配置的指纹 → 默认几何
            (-1.0, -9999.0, 1200.0, 820.0),         // 负数 → 默认几何
            (1.0, 1.0, MIN_WINDOW_W, MIN_WINDOW_H), // 过小的正数 → 下限（不是默认值）
            (100000.0, 100000.0, MAX_WINDOW_W, MAX_WINDOW_H), // 荒谬的大值 → 上限
            (1200.0, 820.0, 1200.0, 820.0),         // 合法值原样通过
            (480.0, 360.0, 480.0, 360.0),           // 正好在下限上 → 原样通过
            (16384.0, 16384.0, MAX_WINDOW_W, MAX_WINDOW_H), // 正好在上限上 → 原样通过
        ] {
            let mut c = Config::default();
            c.shell.window.w = w;
            c.shell.window.h = h;
            c.normalize();
            assert_eq!(c.shell.window.w, want_w, "w={w}, h={h}");
            assert_eq!(c.shell.window.h, want_h, "w={w}, h={h}");
        }

        // 两维独立判定：只有 h 坏时不该牵连合法的 w
        let mut c = Config::default();
        c.shell.window.w = 1300.0;
        c.shell.window.h = 0.0;
        c.normalize();
        assert_eq!(c.shell.window.w, 1300.0);
        assert_eq!(c.shell.window.h, 820.0);
    }

    /// Item 2：`Config::load`（sanitize → `from_value` → `normalize`）与 `Config::save`
    /// 两条链路都必须产出可用几何——`commands::build_main_window` 的 `inner_size` 消费的
    /// 正是这份值，`save_window_geom` 则是把运行期尺寸写回 state / 磁盘的那条路径。
    #[test]
    fn window_geometry_from_file_is_clamped() {
        let dir = temp_dir("geom");
        let p = dir.join("config.json");

        // 手改出来的 0x0（round 1 报告里的实测残留值）
        std::fs::write(
            &p,
            br#"{"shell":{"window":{"w":0,"h":0,"x":null,"y":null}}}"#,
        )
        .unwrap();
        let c = Config::load(&p);
        assert_eq!((c.shell.window.w, c.shell.window.h), (1200.0, 820.0));
        // 同文件里的其它键不受牵连
        assert_eq!(c.shell.home_url, DEFAULT_HOME_URL);

        // 负数 → 默认；超大 → 上限；x/y 语法未变（仍可为 null）
        std::fs::write(
            &p,
            br#"{"shell":{"window":{"w":-5,"h":100000,"x":1,"y":2}}}"#,
        )
        .unwrap();
        let c = Config::load(&p);
        assert_eq!(c.shell.window.w, 1200.0);
        assert_eq!(c.shell.window.h, MAX_WINDOW_H);
        assert_eq!((c.shell.window.x, c.shell.window.y), (Some(1.0), Some(2.0)));

        // 落盘是最后一道防线：内存里被塞了 0 尺寸也不能写进磁盘
        let mut mem = Config::default();
        mem.shell.window.w = 0.0;
        mem.shell.window.h = 0.0;
        mem.save(&p).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["shell"]["window"]["w"], 1200.0);
        assert_eq!(v["shell"]["window"]["h"], 820.0);
        assert_eq!(Config::load(&p).shell.window.h, 820.0);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `commands::resolve_main_url` 的兜底分支断言 `DEFAULT_HOME_URL` 一定可解析
    /// （写到 `unreachable!`），这条单测把该假设钉住。
    #[test]
    fn default_home_url_is_a_valid_web_url() {
        assert!(parse_web_url(DEFAULT_HOME_URL).is_some());
        assert_eq!(
            parse_web_url(DEFAULT_HOME_URL).map(|u| u.to_string()),
            Some("https://fnos.net/".into())
        );
    }

    /// Finding 1「落盘即合法」：即便内存里被塞了非法 URL，`save` 写出的文件也必须合法
    /// （下次启动读到它不会 panic）。`nasUrl` 的「保留原文」语义同样保持不变。
    #[test]
    fn save_normalizes_urls_before_writing() {
        let dir = temp_dir("saveurl");
        let p = dir.join("config.json");
        let mut c = Config::default();
        c.shell.home_url = "javascript:alert(1)".into();
        c.shell.nas_url = "not a url".into();
        c.save(&p).unwrap();

        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("javascript:alert(1)"), "非法 homeUrl 落盘了");
        let back = Config::load(&p);
        assert_eq!(back.shell.home_url, DEFAULT_HOME_URL);
        assert_eq!(back.shell.nas_target(), None);
        // 「保留原文但禁用」：非法 nasUrl 仍在文件里（用户可见可改），只是不被采用
        assert_eq!(back.shell.nas_url, "not a url");
        std::fs::remove_dir_all(&dir).ok();
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
        // （`homeUrl` 这里不能再用 `"keep"` 当哨兵：Finding 1 之后「非 URL 文本」会被
        // 归一化成默认值，见 `malformed_home_url_falls_back_to_default`。）
        for (raw, want) in [("8.5", 9u32), ("\"12\"", 12), ("null", 8)] {
            std::fs::write(
                &p,
                format!(
                    r#"{{"mods":{{"desktopIconPerColumn":{raw}}},"shell":{{"homeUrl":"http://keep.local/"}}}}"#
                ),
            )
            .unwrap();
            let c = Config::load(&p);
            assert_eq!(c.mods.desktop_icon_per_column, want, "raw={raw}");
            // 单个坏类型键不得丢掉同文件里的其它键
            assert_eq!(c.shell.home_url, "http://keep.local/", "raw={raw}");
        }

        // 段本身不是对象（如 `"mods": 5`）：该段全部走默认值，其它段照旧读入
        std::fs::write(
            &p,
            br#"{"mods":5,"shell":{"homeUrl":"http://keep.local/"}}"#,
        )
        .unwrap();
        let c = Config::load(&p);
        assert_eq!(c.mods.desktop_icon_per_column, 8);
        assert_eq!(c.mods.font_face_name, "FnOSCustomFont");
        assert_eq!(c.shell.home_url, "http://keep.local/");
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
