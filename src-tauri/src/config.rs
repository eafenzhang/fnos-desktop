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
    /// T14c：Dock（fnOS WebUI 的任务栏）自动隐藏，**默认关**。
    ///
    /// 归属 `shell` 而不是 `mods`：它不是上游 mod 的某个设置项，而是本壳自己的页面修改
    /// （`inject/dock.js`，用上游 mod.js 的任务栏选择器找到 Dock 后加本壳自己的 class）。
    /// 默认关的理由：滑出屏幕是显眼的外观改变，宁可让用户显式打开，也不默认替用户改页面。
    ///
    /// 生效通道：注入总开关**开**时由建窗载荷带下去（`injector::build_init_script_with`
    /// 的 `shell` 段，只发页面消费的键）；改动经 `commands::apply_to_page` 免刷新生效
    /// （不在 `needs_reload` 判据里——它不是 initialization_script 的静态内容，
    /// 活窗口上就能换）。
    pub dock_auto_hide: bool,
    /// T14c：登录保活心跳的间隔（**分钟**；0 = 关闭，默认 10）。
    ///
    /// 为什么需要：fnOS 的登录态是会话级 cookie（`entry-token`），服务端按访问滑动续期，
    /// 而桌面顶层文档自己完全不轮询（实测 75 秒零请求）——闲置久了登录失效。注入层的
    /// `inject/keepalive.js` 按这个间隔在同源上打一拍（`GET /app/token`）。
    ///
    /// 上限 1440（一天）：手改配置写成天文数字没有意义，`normalize` 会夹住。
    /// 与 `dock_auto_hide` 同一条通道下发（载荷 + 免刷新推送），不需要重建窗口。
    pub keep_alive_minutes: u32,
    /// T14d：检查更新的**下载加速镜像前缀**（默认空 = 直连 GitHub）。
    ///
    /// 为什么需要：Release 资产的下载域名（github.com → objects.githubusercontent.com）
    /// 在部分网络环境直连超时（实测 os error 10060），而 API 域名可达。配置形如
    /// `https://your-mirror.example.com/` 的「前缀 + 原始 URL」加速服务后，安装包改为
    /// 从 `前缀/https://github.com/…` 下载（`updater.rs` 下载前会校验前缀 https 且
    /// host 非本机/内网/保留地址；加速服务本身由用户选择并自担信任）。
    /// 只影响**安装包下载**；版本比对 API 仍直连 api.github.com。
    pub update_mirror_prefix: String,
    pub window: WindowGeom,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            home_url: DEFAULT_HOME_URL.into(),
            nas_url: String::new(),
            inject_enabled: true,
            close_to_tray: true,
            dock_auto_hide: false,
            keep_alive_minutes: 10,
            update_mirror_prefix: String::new(),
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
    /// （用户能在配置文件里看到自己的错字并改回来），但凡是要*使用*它的地方都只看这个
    /// 取值器。于是「非法 nasUrl」在所有入口都一致地表现为「没有可用地址」，
    /// 不会再出现 round 0 那种「菜单可点、点了静默什么都不做」。
    ///
    /// 注（T13b fix round 1 订正注释）：托盘的「打开 NAS」菜单项、`tray::sync_menus` 与
    /// `commands::open_nas` 都已随托盘精简删除；这条取值器现在唯一的消费方是
    /// `normalize`（把合法 `nasUrl` 的 origin 并入注入白名单，见 `enabled_origins`），
    /// 以及 T14e 起的 `commands::resolve_main_url`（配了 nasUrl 启动直达 NAS 桌面）。
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

/// 上游 `launchpadIconRedrawMap` 的取值约束：`^prefect_icon/[a-z0-9-]+\.png$`，
/// 但**按大小写不敏感**判定（R30：Windows 路径）。
///
/// 为什么放宽（R30）：图标资源在磁盘上是 camelCase（`src-tauri/assets/fnos-mods/prefect_icon/`
/// 下就是 `panIndex.png` 这类名字），而 shim 的 `getURL` 建索引时把小写化后的键当唯一键
/// （shim.js 的 `assetIndex[String(k).toLowerCase()]`，`tests/shim.test.mjs` 有一条用例锁着
/// 「大小写不敏感」）——也就是说 `emby.PNG` / `Prefect_Icon/Emby.png` 在运行期**都能解析到同一份
/// 资源**，而旧实现只认全小写，会把它们当非法值在 `normalize` 里 `retain` 掉。用户看到的现象是
/// 「设置了完美图标，页面却没变化」，且没有任何提示——**静默丢配置**比拒绝更难排障。
///
/// 放宽的**只有大小写这一维**：
/// - 仍然拒绝穿越（`..`）、子目录（`sub/dir.png`）、反斜杠（`prefect_icon\emby.png`：
///   shim 的查表键用 `/`，反斜杠永远解析不到资源，放行只会造出一个查不到的值）、
///   双扩展名（`a.png.png`）、空名，以及 ASCII 字母数字连字符之外的字符。
/// - 判定顺序不变（先去前缀、再去后缀、再验主体字符集），所以 `prefect_icon/a.png.png`
///   仍然是「主体含 `.`」而被拒。
pub fn is_valid_prefect_icon_path(v: &str) -> bool {
    let Some(rest) = strip_prefix_ascii_ci(v, "prefect_icon/") else {
        return false;
    };
    let Some(name) = strip_suffix_ascii_ci(rest, ".png") else {
        return false;
    };
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 去掉 `prefix`（**只折 ASCII 大小写**），前缀不匹配时 `None`。
///
/// 用 `str::get(..n)` 而不是切片索引：非 ASCII 字节边界上取 `None` 而不是 panic
///（长度前缀永远按字节算，而 `&str` 不能切在字符中间）。
fn strip_prefix_ascii_ci<'a>(v: &'a str, prefix: &str) -> Option<&'a str> {
    let head = v.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }
    v.get(prefix.len()..)
}

/// 去掉 `suffix`（**只折 ASCII 大小写**），后缀不匹配（或比整个串还长）时 `None`。
fn strip_suffix_ascii_ci<'a>(v: &'a str, suffix: &str) -> Option<&'a str> {
    let start = v.len().checked_sub(suffix.len())?;
    let tail = v.get(start..)?;
    if !tail.eq_ignore_ascii_case(suffix) {
        return None;
    }
    v.get(..start)
}

// ---------- 登录壁纸（Task 13b）：`local.loginWallpaperFileName` 的取值约束 ----------
//
// 与 `is_valid_prefect_icon_path` 同一个立场：**配置文件里的值是用户文本**（`open_config_dir`
// 明示了路径，手改是预期用法），所以任何要*使用*它的地方——`injector` 读文件、
// `commands::import_wallpaper` 写文件——都必须先过下面这几个纯函数。
//
// 这与 `shell.nasUrl` 的处理方式一致：**不**在 `Config::normalize` 里删掉不合法的值
// （用户要能在 config.json 里看到自己的错字），而是让所有消费方一致地表现为「没有可用壁纸」，
// 并在 stderr 留一行说明。

/// 登录壁纸的文件名长度上限（放宽后的形状判定用）。本壳自己生成的名字最长约 56 字符。
const MAX_WALLPAPER_NAME_LEN: usize = 256;

/// 登录壁纸的**大小上限**（8 MiB）。
///
/// 为什么有上限：壁纸要 base64 后塞进初始化脚本（`binaryAssets`），WebView2 每次建窗都要
/// 解析这段文本。10 MiB 的图 → ~13.4 MiB 的脚本，建窗会明显变慢；8 MiB 足够覆盖 4K JPEG。
/// 超限时 `injector` 打一行日志并**跳过**这一项（不 panic、不截断）。
pub const MAX_WALLPAPER_BYTES: usize = 8 * 1024 * 1024;

/// 登录壁纸允许的扩展名（小写规范形式）：`png` / `jpg` / `jpeg` / `webp`。
///
/// 与设置窗的壁纸文件输入一致（T14b 起是**上游** popup.html 的
/// `#loginWallpaperFile` 的 `accept`，以及 `chrome-shim.js` 的 `WALLPAPER_EXTS` 早退表）：
/// 上游只把壁纸当 CSS `background-image` 用，这三种格式是 WebView2 一定能解码的。
pub fn wallpaper_ext(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    for ext in ["png", "jpg", "jpeg", "webp"] {
        if lower.len() > ext.len() + 1 && lower.ends_with(&format!(".{ext}")) {
            return Some(match ext {
                "png" => "png",
                "jpg" => "jpg",
                "jpeg" => "jpeg",
                _ => "webp",
            });
        }
    }
    None
}

/// 文件名是否是一个**可用的登录壁纸名**。
///
/// 两个入口共用这一份判定：① `import_wallpaper` 收到的用户文件名；②
/// `local.loginWallpaperFileName` 这个**配置值**（`injector::load_wallpaper_from` 要拿它
/// `Path::join` 去读文件）。因此规则是「形状安全 + 扩展名在允许表内」，而**不**要求纯 ASCII：
/// 用户把文件叫「登录壁纸.png」是再正常不过的事，而 R30 的教训正是「带点非 ASCII 就把用户的
/// 配置静默丢掉，比拒绝更难排障」。
///
/// 明确拒绝（每一条都有理由，不是洁癖）：
/// - 空 / 超过 256 字节 / 首尾有空白（Windows 会静默吃掉路径末尾的空格与点 → 配置值与实际
///   打开的文件不是一个东西）；
/// - 路径分隔符 `/` `\`、盘符 `:`、Windows 保留字符 `*?"<>|`、`..`、以 `.` 开头 → 穿越与
///   意外路径（`Path::join` 遇到分隔符会真的换目录）；
/// - 任何控制字符（含 `\n`：日志是本项目的评审证据，绝不允许页面/用户可控文本换行）；
/// - **Windows 保留设备名**（`CON` / `PRN` / `AUX` / `NUL` / `COM1`–`COM9` / `LPT1`–`LPT9`，
///   大小写不敏感、**带不带扩展名都算**）——见 [`is_windows_device_name`]；
/// - 没有扩展名或扩展名不在 `png/jpg/jpeg/webp` 内（mime 由扩展名推导，见 [`wallpaper_mime`]）。
pub fn is_wallpaper_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_WALLPAPER_NAME_LEN {
        return false;
    }
    if name.trim() != name || name.starts_with('.') || name.contains("..") {
        return false;
    }
    if name.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\u{0}'
            )
    }) {
        return false;
    }
    if is_windows_device_name(name) {
        return false;
    }
    wallpaper_ext(name).is_some()
}

/// 这个文件名是不是 Windows 的**保留设备名**。
///
/// 判据取「第一个 `.` 之前的那一段」（Windows 认名字的方式：`NUL.txt`、`CON.log.png` 都是设备，
/// 扩展名不改变结论），大小写不敏感；`CON` / `PRN` / `AUX` / `NUL` 是完整名单，
/// `COM1`–`COM9` / `LPT1`–`LPT9` 是「三字母 + 一位数字」的形状。
///
/// ## 为什么读路径也必须拒（Review fix round 1 / Minor 2）
///
/// 写路径本来就免疫：导入落盘的名字一律是 `stored_wallpaper_name()` 生成的
/// `<stem>-<fnv64 指纹>.<ext>`，**永远带一串十六进制后缀**，所以 `CON.png` 这类名字根本落不了盘
/// （而且 `stored_wallpaper_name` 会把非法字符剥掉）。但读路径不是这样：
/// `local.loginWallpaperFileName` 是**用户可编辑**的配置值，
/// `injector::load_wallpaper_from` 会拿它 `Path::join(配置目录, name)` 去 `metadata`/`read`。
/// 在 Windows 上 `...\config\CON.png` 解析到的不是你放在配置目录里的文件，而是**控制台设备**——
/// 也就是说用户的配置会悄悄指向一个与配置目录无关的东西（成败取决于设备语义，而不是我们的校验）。
/// 在写路径拒绝它、读路径却放行，两边就不一致了；统一在**名字形状**这一层拒掉最省事也最可审计。
pub fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let bytes = upper.as_bytes();
    // `COM1`–`COM9` / `LPT1`–`LPT9`：恰好三字母 + 一位数字，不是子串匹配
    //（`CONSOLE` / `mycon` / `com10` 都应该放行）。
    bytes.len() == 4
        && (bytes.starts_with(b"COM") || bytes.starts_with(b"LPT"))
        && (b'1'..=b'9').contains(&bytes[3])
}

/// 登录壁纸的 mime（由扩展名推导）；名字形状不合法或扩展名不在允许表内 → `None`。
///
/// 与 shim 的 `mimeFor(path)` 同源（那边也要认 `jpg`/`jpeg`/`webp`，否则 data URL 会被
/// 标成 `text/plain`），两处都从这份允许表出发。
pub fn wallpaper_mime(name: &str) -> Option<&'static str> {
    if !is_wallpaper_name(name) {
        return None;
    }
    match wallpaper_ext(name)? {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        _ => Some("image/webp"),
    }
}

/// FNV-1a 64 位散列（内容指纹）。
///
/// 只为「同一份文件导入两次得到同一个名字」这一件事服务：导入的落盘名 = `<stem>-<指纹>.png`
/// （见 [`stored_wallpaper_name`]），于是**内容变了名字才变**，`set_config` 的
/// `needsReload` 判据（比较文件名）与「载荷里嵌的是哪张图」天然一致——不会出现
/// 「重新导入同名文件但页面还是旧图」。不是密码学散列，也不当校验和用。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 把用户选中的文件名净化成 `<stem>-<16 位指纹>.<ext>`（小写）。
///
/// 为什么不直接用用户给的名字：① 用户的文件名可能含中文/空格/路径片段，直接落盘既可能穿越、
/// 也可能与配置目录里的既有文件（含 `config.json`）撞名；② 覆盖同名文件会让「文件内容变了但
/// 配置里的名字没变」——主窗口就不会重建，页面继续用旧图。指纹让它**由内容决定**。
///
/// `original` 的扩展名必须先过 [`wallpaper_ext`]（调用方已经判过），这里只负责 stem。
pub fn stored_wallpaper_name(original: &str, bytes: &[u8]) -> String {
    let stem_raw = original
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(original);
    let mut stem = String::with_capacity(stem_raw.len());
    for c in stem_raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            stem.push(c);
        } else if !stem.ends_with('-') {
            stem.push('-');
        }
        if stem.len() >= 32 {
            break;
        }
    }
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "wallpaper" } else { stem };
    let ext = wallpaper_ext(original).unwrap_or("png");
    format!("{stem}-{:016x}.{ext}", fnv1a64(bytes))
}

/// `<path>.bak`（追加式命名：对 `config.json` 得 `config.json.bak`）。
fn bak_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// [`Config::load_with_report`] 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoadReport {
    /// 本次加载是否因**文件内容不可用**而回落默认值，且损坏原件已留成 `<path>.bak`
    /// （`.bak` 的写入结果参与判定：写不成功就不声称「已保留」）。
    pub recovered_from_backup: bool,
}

/// 把损坏原文写成 `.bak`，并按「是否真的写成功」给出报告（Task 11 的状态条据此回显）。
fn write_backup(path: &Path, text: &str) -> LoadReport {
    LoadReport {
        recovered_from_backup: std::fs::write(bak_path(path), text).is_ok(),
    }
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
    for key in ["injectEnabled", "closeToTray", "dockAutoHide"] {
        sanitize_bool_field(&mut shell, key);
    }
    sanitize_u32_field(&mut shell, "keepAliveMinutes");
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
        // `set_config` / `reset_config` 也各自显式调用；T13b fix round 1 订正：托盘精简后
        // `commands::set_inject_enabled` 已删除，开关改动现在同样走 `set_config`），因此
        // `commands::build_main_window` 的 `inner_size(cfg.shell.window.w, h)` 拿到的
        // 一定是可用几何——历史遗留的 `0x0` 不会再建出 0 尺寸窗口。
        self.shell.window.clamp_to_usable();
        // 保活心跳间隔的上限（T14c）：手改配置写成天文数字没有意义，夹到一天。
        if self.shell.keep_alive_minutes > 1440 {
            self.shell.keep_alive_minutes = 1440;
        }
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
    ///
    /// `recovered_from_backup` 的判据刻意收得很紧：**文件内容不可用 → 回落默认值，且损坏
    /// 原件确实被留成了 `<path>.bak`** 才算。于是：
    ///
    /// - 文件不存在（全新安装）→ `false`（没有任何东西可恢复）；
    /// - 逐字段类型损坏但整体可解析（走 `sanitize_config_value`）→ `false`（配置没丢）；
    /// - `.bak` 没写成功（磁盘满 / 无权限）→ `false`：宁可不提示，也不谎称「已保留 .bak」。
    pub fn load_with_report(path: &Path) -> (Config, LoadReport) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => {
                // 读失败（含文件不存在）：尽力拷原件留证，再回落默认
                let copied = std::fs::copy(path, bak_path(path)).is_ok();
                return (
                    Config::default(),
                    LoadReport {
                        recovered_from_backup: copied,
                    },
                );
            }
        };
        let mut text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                // 非 UTF-8（UTF-16 / 二进制）：拷原件留证，否则损坏证据会被丢掉
                let copied = std::fs::copy(path, bak_path(path)).is_ok();
                return (
                    Config::default(),
                    LoadReport {
                        recovered_from_backup: copied,
                    },
                );
            }
        };
        // Notepad 默认会写 UTF-8 BOM；不剥掉的话 serde_json 直接报错 → 整份配置被丢弃
        if text.starts_with('\u{feff}') {
            text.remove(0);
        }

        let value: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return (Config::default(), write_backup(path, &text)),
        };
        if !value.is_object() {
            // 顶层不是对象（如 `[1,2]` / `"x"`）：没有可净化的字段，按损坏留证
            return (Config::default(), write_backup(path, &text));
        }

        match serde_json::from_value::<Config>(sanitize_config_value(value)) {
            Ok(mut c) => {
                c.normalize();
                (c, LoadReport::default())
            }
            Err(_) => (Config::default(), write_backup(path, &text)),
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

    /// 测试内的简写：绝大多数断言只关心**配置本身**，不关心回退报告；报告另有专门用例
    ///（`load_reports_recovery_from_backup`）。生产代码走 `Config::load_with_report`。
    fn load(path: &Path) -> Config {
        Config::load_with_report(path).0
    }

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
        assert!(!c.shell.dock_auto_hide);
        assert_eq!(c.shell.keep_alive_minutes, 10, "保活心跳默认 10 分钟");
    }

    /// T14c：保活心跳间隔的夹取（手改配置写成天文数字没有意义）。
    #[test]
    fn keep_alive_minutes_is_clamped_to_a_day() {
        let mut c = Config::default();
        c.shell.keep_alive_minutes = 5000;
        c.normalize();
        assert_eq!(c.shell.keep_alive_minutes, 1440, "上限夹到一天");
        // 0（关闭）与正常值原样保留
        c.shell.keep_alive_minutes = 0;
        c.normalize();
        assert_eq!(c.shell.keep_alive_minutes, 0, "0 = 关闭，必须保留");
        c.shell.keep_alive_minutes = 45;
        c.normalize();
        assert_eq!(c.shell.keep_alive_minutes, 45);
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
        assert_eq!(load(&p).mods.brand_color, applied);

        // 反复 save → load 不再漂移（修前第二次读回来就是 #c4b4a1）
        for _ in 0..3 {
            load(&p).save(&p).unwrap();
        }
        assert_eq!(load(&p).mods.brand_color, applied);
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

        let first = load(&p);
        assert_eq!(first.mods.brand_color, "#c4b4a2");
        first.save(&p).unwrap();
        let disk = std::fs::read_to_string(&p).unwrap();
        assert!(disk.contains("\"brandColor\": \"#c4b4a2\""), "{disk}");
        assert!(!disk.contains("#c4b4a1"), "{disk}");

        let second = load(&p);
        assert_eq!(second.mods.brand_color, "#c4b4a2");
        second.save(&p).unwrap();
        assert_eq!(load(&p).mods.brand_color, "#c4b4a2");

        // 已经归一化的手写值（区间内）也必须原样落盘
        std::fs::write(
            &p,
            r##"{"schemaVersion":1,"mods":{"brandColor":"#3366CC"}}"##,
        )
        .unwrap();
        let upper = load(&p);
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
        c.shell.dock_auto_hide = true; // T14c：shell 新键随同一份往返
        c.shell.keep_alive_minutes = 30;
        c.save(&p).unwrap();
        let back = load(&p);
        assert_eq!(back.mods.brand_color, "#336699");
        assert!(back.shell.dock_auto_hide);
        assert_eq!(back.shell.keep_alive_minutes, 30);
        assert_eq!(back.schema_version, SCHEMA_VERSION);

        // 旧版本（无 schemaVersion / 缺字段）应能补齐。
        // 注意：load 必经 §6.5 归一化，深色 #010203（L≈0.8%）会被明度夹到 30% → #264d73，
        // 故此处断言「文件里的值确被读入并归一化」（≠ 默认色），而非原样保留。
        std::fs::write(&p, br##"{"mods":{"brandColor":"#010203"}}"##).unwrap();
        let migrated = load(&p);
        assert_eq!(migrated.mods.brand_color, "#264d73");
        assert_ne!(migrated.mods.brand_color, DEFAULT_BRAND_COLOR);
        assert_eq!(migrated.schema_version, SCHEMA_VERSION);
        assert_eq!(migrated.mods.titlebar_style, "windows");

        // 损坏 JSON：回退默认 + 生成 .bak（内容必须等于写入的损坏文本）
        let corrupt: &[u8] = b"{not json";
        std::fs::write(&p, corrupt).unwrap();
        let broken = load(&p);
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
        assert!(!c.shell.dock_auto_hide);
        assert_eq!(c.shell.keep_alive_minutes, 10, "保活心跳默认 10 分钟");
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

    /// Task 11（spec §12.3「config.json 损坏 → 回退默认 + 保留 .bak + 设置窗提示」）：
    /// 「回退过」这件事必须能**被设置窗读到**，而不是只在磁盘上留个 `.bak` 让人自己发现。
    ///
    /// 判据刻意收得很紧：只有「文件内容不可用 → 回落默认值，且原件确实被留成了 `.bak`」
    /// 才算恢复。文件不存在（全新安装）、逐字段类型损坏但整体可解析，都**不算**——
    /// 状态条不能虚报一次「你的配置损坏了」。
    #[test]
    fn load_reports_recovery_from_backup() {
        let dir = temp_dir("recovery-report");
        let p = dir.join("config.json");
        let bak = dir.join("config.json.bak");

        // 1) 全新目录：文件不存在 → 默认值，但这不是「从备份恢复」
        assert!(
            !Config::load_with_report(&p).1.recovered_from_backup,
            "文件不存在（全新安装）不得报告为已恢复"
        );

        // 2) 合法配置 → 不报告
        let mut ok = Config::default();
        ok.mods.brand_color = "#336699".into();
        ok.save(&p).unwrap();
        assert!(!Config::load_with_report(&p).1.recovered_from_backup);

        // 3) 非法 JSON → 默认值 + 报告 + `.bak` 里是损坏原文
        let broken: &[u8] = b"{not json";
        std::fs::write(&p, broken).unwrap();
        let _ = std::fs::remove_file(&bak);
        let (cfg, report) = Config::load_with_report(&p);
        assert_eq!(cfg.mods.brand_color, DEFAULT_BRAND_COLOR);
        assert!(report.recovered_from_backup, "非法 JSON 必须报告已回退");
        assert_eq!(std::fs::read(&bak).unwrap(), broken, "损坏原文必须留证");

        // 4) 顶层不是对象（`[1,2]`）同样算损坏
        std::fs::write(&p, b"[1,2]").unwrap();
        assert!(Config::load_with_report(&p).1.recovered_from_backup);

        // 5) 非 UTF-8（UTF-16 / 二进制）：拷原件留证 + 报告
        std::fs::write(&p, [0xff, 0xfe, 0x41, 0x00]).unwrap();
        assert!(Config::load_with_report(&p).1.recovered_from_backup);

        // 6) 逐字段类型损坏但整体可解析 → 走 sanitize，不算整份回退（不虚报）
        std::fs::write(&p, br#"{"mods":{"brandColor":42}}"#).unwrap();
        assert!(
            !Config::load_with_report(&p).1.recovered_from_backup,
            "逐字段净化成功时不得报告为已回退"
        );

        // 7) `load()` 的旧签名仍然可用（报告被丢弃），两侧必须给出同一份配置
        assert_eq!(load(&p), Config::load_with_report(&p).0);

        std::fs::remove_dir_all(&dir).ok();
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
        let c = load(&p);
        assert_eq!(c.shell.home_url, DEFAULT_HOME_URL);
        // 同文件里的其它键不受牵连
        assert!(c.shell.inject_enabled);

        // 非字符串类型（对象/数组）走 `sanitize_string_field` 删除 → 默认值，同样不是 panic
        std::fs::write(&p, br#"{"shell":{"homeUrl":{"a":1}}}"#).unwrap();
        assert_eq!(load(&p).shell.home_url, DEFAULT_HOME_URL);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Finding 1：非法 `nasUrl` 一律视为「未配置」——`nas_target()` 是唯一取值器，
    /// 所有消费方（现在只剩 `normalize` 的白名单并入）都据此表现为「没有可用地址」，
    /// 因此不会再出现「菜单可点、点了静默无事发生」。
    ///
    /// 名字里的 `tray` 是历史遗留（T13b fix round 1 订正注释）：托盘「打开 NAS」菜单项、
    /// `tray::sync_menus` 与 `commands::open_nas` 都已随托盘精简删除；这条用例守的语义
    /// （非法值解析不出来、合法值能把 origin 并入白名单）没有变，也就没有改测试名。
    #[test]
    fn malformed_nas_url_disables_tray_item() {
        for raw in MALFORMED_URLS {
            let mut c = Config::default();
            c.shell.nas_url = raw.into();
            c.normalize();
            // 「未配置」的判据本身：所有消费方都是 `nas_target()` 这一个取值器
            assert_eq!(c.shell.nas_target(), None, "raw={raw:?}");
            // 「保留原文但禁用」：用户输入不被静默删除（与 `homeUrl` 的回落策略不同，
            // 原因见 `ShellConfig::nas_target` 的文档）
            assert_eq!(c.shell.nas_url, raw, "raw={raw:?}");
            // 非法 nasUrl 不得混进注入白名单
            assert!(c.mods.enabled_origins.is_empty(), "raw={raw:?}");
        }

        // 合法值：可解析 + origin 并入白名单
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
        let c = load(&p);
        assert_eq!((c.shell.window.w, c.shell.window.h), (1200.0, 820.0));
        // 同文件里的其它键不受牵连
        assert_eq!(c.shell.home_url, DEFAULT_HOME_URL);

        // 负数 → 默认；超大 → 上限；x/y 语法未变（仍可为 null）
        std::fs::write(
            &p,
            br#"{"shell":{"window":{"w":-5,"h":100000,"x":1,"y":2}}}"#,
        )
        .unwrap();
        let c = load(&p);
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
        assert_eq!(load(&p).shell.window.h, 820.0);

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
        let back = load(&p);
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
                "dockAutoHide": "true",
                "keepAliveMinutes": "25",
                "window": { "w": "1200", "h": 900, "x": null, "y": "30" }
              }
            }"##,
        )
        .unwrap();
        let c = load(&p);

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
        // T14c："true" → true（默认关，故 true 只能来自强制转换，不是回落默认）
        assert!(c.shell.dock_auto_hide);
        // T14c：数字字符串 → 数字（默认 10，故 25 只能来自强制转换）
        assert_eq!(c.shell.keep_alive_minutes, 25);
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
            let c = load(&p);
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
        let c = load(&p);
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

        let c = load(&p);
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

        let c = load(&p);
        assert_eq!(c.mods.brand_color, DEFAULT_BRAND_COLOR); // 回落默认
        assert_eq!(c.mods.desktop_icon_per_column, 8);
        let bak = dir.join("config.json.bak");
        assert!(bak.exists(), "读错误未留证");
        assert_eq!(std::fs::read(&bak).unwrap(), corrupt); // 原件字节完整保留

        // 文件不存在：回落默认，且无可留证内容 → 不产生 .bak
        let missing = dir.join("nope.json");
        let d = load(&missing);
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
            let c = load(&p);
            assert_eq!(c.mods.desktop_icon_per_column, want, "raw={raw}");
            assert_eq!(c.mods.titlebar_style, "mac");
            assert_eq!(c.mods.launchpad_style, "spotlight");
            assert_eq!(c.mods.desktop_icon_layout_mode, "fixed");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// R30：`launchpadIconRedrawMap` 的值**按大小写不敏感**判定（Windows 路径），
    /// 但**只放宽大小写这一维**——穿越/子目录/反斜杠/双扩展名/空名一律仍然剔除。
    #[test]
    fn redraw_map_regex_is_case_insensitive_only() {
        // 仍然合法：全小写、数字、连字符
        assert!(is_valid_prefect_icon_path("prefect_icon/emby.png"));
        assert!(is_valid_prefect_icon_path(
            "prefect_icon/home-assistant.png"
        ));
        assert!(is_valid_prefect_icon_path("prefect_icon/a1-b2.png"));

        // R30 放宽的三处大小写（每一处都单独钉住，免得将来只放宽其中之一还全绿）
        assert!(
            is_valid_prefect_icon_path("prefect_icon/Emby.png"),
            "资源名本身是 camelCase（磁盘上就是 panIndex.png 这类名字）"
        );
        assert!(
            is_valid_prefect_icon_path("prefect_icon/emby.PNG"),
            "混合大小写扩展名"
        );
        assert!(
            is_valid_prefect_icon_path("PREFECT_ICON/emby.PnG"),
            "目录段 + 扩展名同时混合大小写（分隔符两侧都算）"
        );
        assert!(is_valid_prefect_icon_path(
            "Prefect_Icon/Home-Assistant.PNG"
        ));

        // 只放宽大小写：其余约束一个字都没松
        assert!(!is_valid_prefect_icon_path("../x"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/a.png.png"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/sub/dir.png"));
        assert!(!is_valid_prefect_icon_path("other/emby.png"));
        assert!(
            !is_valid_prefect_icon_path("prefect_icon\\emby.png"),
            "反斜杠：shim 的查表键用 `/`（assetIndex 全小写、按原分隔符），放行只会造出查不到的值"
        );
        assert!(!is_valid_prefect_icon_path("prefect_icon/em by.png"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/.png"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/emby.png "));
        assert!(!is_valid_prefect_icon_path(" prefect_icon/emby.png"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/图标.png"));
        // 非 ASCII 不得 panic（前缀长度按字节切，可能落在字符中间）
        assert!(!is_valid_prefect_icon_path("图标"));
        assert!(!is_valid_prefect_icon_path("prefect_icon/图的.png"));

        // normalize 的 retain 必须与上面的判定同源：混合大小写的值要被**保留**下来
        let mut c = Config::default();
        for (k, v) in [
            ("ok", "prefect_icon/emby.png"),
            ("upper", "prefect_icon/Emby.png"),
            ("ext", "prefect_icon/emby.PNG"),
            ("dir", "PREFECT_ICON/emby.png"),
            ("dots", "prefect_icon/a.png.png"),
            ("esc", "../x"),
            ("sub", "prefect_icon/sub/dir.png"),
            ("bslash", "prefect_icon\\emby.png"),
        ] {
            c.mods.launchpad_icon_redraw_map.insert(k.into(), v.into());
        }
        c.normalize();
        let keys: Vec<String> = c.mods.launchpad_icon_redraw_map.keys().cloned().collect();
        assert_eq!(
            keys,
            vec!["dir", "ext", "ok", "upper"],
            "混合大小写的合法值必须原样保留（retain 与判定同源）"
        );
        assert_eq!(
            c.mods.launchpad_icon_redraw_map["upper"], "prefect_icon/Emby.png",
            "保留的是用户写的原文，不做改写"
        );
        // 幂等：再归一化一次不改变任何东西（否则每次 save/load 都会漂移）
        c.normalize();
        let again: Vec<String> = c.mods.launchpad_icon_redraw_map.keys().cloned().collect();
        assert_eq!(again, keys);
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
        assert_eq!(load(&p).mods.font_family, "second");
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

    // ---------- 登录壁纸（Task 13b）：`local.loginWallpaperFileName` 的取值约束 ----------

    /// 允许表：`png` / `jpg` / `jpeg` / `webp`（大小写不敏感）；其余一律拒绝。
    ///
    /// 「拒绝」是**形状**层面的，所以 `svg` / `gif` / `bmp` / `ico` / 没有扩展名 / 只有点
    /// 都在这里被挡掉——它们永远不会走到读文件或写文件那一步。
    #[test]
    fn wallpaper_extension_allow_list_is_exact() {
        for (name, mime) in [
            ("wallpaper.png", "image/png"),
            ("WALLPAPER.PNG", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.jpeg", "image/jpeg"),
            ("a.JPEG", "image/jpeg"),
            ("a.webp", "image/webp"),
            ("my-photo_2024.09.jpg", "image/jpeg"),
            ("wallpaper-0a1b2c3d4e5f6071.png", "image/png"),
            // 中文文件名与**内部空格**都要放行：用户把文件叫「登录壁纸.png」很常见，
            // 而落盘名由 `stored_wallpaper_name` 重新生成（与这里的名字无关）。
            ("登录壁纸.png", "image/png"),
            ("my wallpaper.png", "image/png"),
            ("壁纸 1.jpeg", "image/jpeg"),
        ] {
            assert_eq!(wallpaper_mime(name), Some(mime), "{name}");
            assert!(is_wallpaper_name(name), "{name}");
        }
        for name in [
            "",
            "wallpaper",     // 没有扩展名
            "wallpaper.svg", // 设置窗的 accept 之外
            "wallpaper.gif",
            "wallpaper.bmp",
            "wallpaper.ico",
            "wallpaper.avif",
            "wallpaper.", // 只有点
            ".png",       // 隐藏文件 / 没有 stem
            ".hidden.png",
            "中文", // 没有扩展名
            "a/b.png",
            "a\\b.png",
            "..png",
            "../x.png",
            "C:x.png",   // 盘符（冒号被拒）
            "a\nb.png",  // 换行（日志注入的载体）
            "a\tb.png",  // 制表符
            " lead.png", // 首尾空白（Windows 会吃掉路径末尾的空白/点）
            "trail.png ",
            "a*b.png", // Windows 保留字符
            "a?b.png",
            "a\"b.png",
            "a<b.png",
            "a>b.png",
            "a|b.png",
            // Windows 保留设备名（Minor 2）：读路径会 `Path::join` 这个配置值，
            // `CON.png` 之类的名字在 Windows 上指向设备而不是配置目录里的文件。
            // 带不带扩展名、大小写、以及 `NUL.txt` 这种多扩展名形态都算。
            "CON.png",
            "con.PNG",
            "Con.png",
            "PRN.png",
            "aux.jpg",
            "nul.webp",
            "COM1.png",
            "com9.jpeg",
            "LPT1.png",
            "lpt9.PNG",
            "NUL.txt.png",
            "COM1.jpg",
        ] {
            assert_eq!(wallpaper_mime(name), None, "{name:?} 必须被拒");
            assert!(!is_wallpaper_name(name), "{name:?} 必须被拒");
        }
        // 设备名判定是**精确**的：含 `con`/`com` 子串的普通名字、以及 `COM10` 这种
        // 不在文档名单里的形状都必须照常放行（过宽的拒绝会把用户的正常文件名挡在门外）。
        for ok in [
            "console.png",
            "mycon.png",
            "con-1.png",
            "com.png",
            "com0.png",
            "com10.png",
            "lpt.png",
            "lpt0.png",
            "lpt10.png",
            "auxiliary.jpg",
            "null.webp",
        ] {
            assert!(is_wallpaper_name(ok), "{ok:?} 不得被设备名规则误伤");
            assert!(wallpaper_mime(ok).is_some(), "{ok:?}");
            assert!(!is_windows_device_name(ok), "{ok:?}");
        }
        for dev in ["CON", "prn", "Aux", "nul", "COM1", "lpt9", "NUL.txt"] {
            assert!(is_windows_device_name(dev), "{dev:?}");
        }
        // `wallpaper.png.png` 的具体结论：字符集允许、扩展名判定看**最后**一段，于是它是
        // 「合法形状」——这不是漏洞（名字里没有分隔符就不能穿越，落盘名还会被重新生成），
        // 这里显式钉住真实语义，免得将来有人照着错误的期望去改实现。
        assert!(is_wallpaper_name("wallpaper.png.png"));
        assert_eq!(wallpaper_mime("wallpaper.png.png"), Some("image/png"));

        // 超长名字（>256 字节）拒绝
        assert!(!is_wallpaper_name(&format!("{}.png", "a".repeat(300))));
        // 正好在长度上限内则放行（上限是「含」上限）
        let ok = format!("{}.png", "a".repeat(MAX_WALLPAPER_NAME_LEN - 4));
        assert_eq!(ok.len(), MAX_WALLPAPER_NAME_LEN);
        assert!(is_wallpaper_name(&ok));
    }

    /// 扩展名推导本身（`import_wallpaper` 用它做「扩展名与内容一致」的那一条检查）。
    #[test]
    fn wallpaper_ext_is_ascii_case_insensitive() {
        assert_eq!(wallpaper_ext("a.PNG"), Some("png"));
        assert_eq!(wallpaper_ext("a.JpEg"), Some("jpeg"));
        assert_eq!(wallpaper_ext("a.WebP"), Some("webp"));
        assert_eq!(wallpaper_ext("登录壁纸.PNG"), Some("png"));
        assert_eq!(wallpaper_ext("png"), None);
        assert_eq!(wallpaper_ext(".png"), None);
        assert_eq!(wallpaper_ext("a.pngs"), None);
    }

    /// 落盘名由「净化后的 stem + 内容指纹」组成：
    /// ① 用户文件的路径/空格/中文不会带进来；② 同一份内容永远得到同一个名字；
    /// ③ 内容变了名字就变（这是 `set_config` 的 `needsReload` 判据能发现换图的前提）。
    #[test]
    fn stored_wallpaper_name_is_content_addressed_and_sanitized() {
        let png = [0x89u8, 0x50, 0x4e, 0x47];
        let jpeg = [0xffu8, 0xd8, 0xff, 0xe0];

        let a = stored_wallpaper_name("my photo.png", &png);
        assert_eq!(a, stored_wallpaper_name("my photo.png", &png), "同内容同名");
        assert_eq!(a, stored_wallpaper_name("MY PHOTO.PNG", &png), "大小写归一");
        assert!(a.starts_with("my-photo-"), "{a}");
        assert!(a.ends_with(".png"), "{a}");
        assert_eq!(a.len(), "my-photo-".len() + 16 + 4, "{a}");
        assert!(is_wallpaper_name(&a), "{a}");
        assert_eq!(wallpaper_mime(&a), Some("image/png"));

        // 内容不同 → 名字不同（换图必然触发一次主窗口重建）
        assert_ne!(a, stored_wallpaper_name("my photo.png", &jpeg));

        // 危险/奇怪的用户输入不会渗进落盘名
        for (raw, ext) in [
            (r"C:\Users\x\壁纸 1.png", "png"),
            ("../../etc/passwd.png", "png"),
            ("\n[fnos] 注入.png", "png"),
            ("...png", "png"),
            ("中文名字.jpeg", "jpeg"),
        ] {
            let n = stored_wallpaper_name(raw, &png);
            assert!(is_wallpaper_name(&n), "raw={raw:?} -> {n}");
            assert!(n.ends_with(&format!(".{ext}")), "raw={raw:?} -> {n}");
            assert!(
                !n.contains('/') && !n.contains('\\') && !n.contains('\n') && !n.contains(':'),
                "raw={raw:?} -> {n}"
            );
        }
        // stem 全是非法字符 → 回落固定词，且不是空名字
        let fallback = stored_wallpaper_name("///.png", &png);
        assert!(fallback.starts_with("wallpaper-"), "{fallback}");
        // 超长 stem 被截到 32 字符以内
        let long = stored_wallpaper_name(&format!("{}.png", "z".repeat(200)), &png);
        assert!(long.len() <= 32 + 1 + 16 + 4, "{long}");
        // 扩展名按原样保留（jpg / jpeg 各自成一种）
        assert!(stored_wallpaper_name("x.jpg", &png).ends_with(".jpg"));
        assert!(stored_wallpaper_name("x.jpeg", &png).ends_with(".jpeg"));
        assert!(stored_wallpaper_name("x.webp", &png).ends_with(".webp"));
    }

    /// `local.loginWallpaperFileName` **不进** `normalize` 的过滤：用户手改的错名字要留在
    /// 配置文件里可见可改（与 `nasUrl` 的「保留原文但禁用」同一条纪律），
    /// 只是消费方（`injector` / `import_wallpaper`）一律不采用它。
    #[test]
    fn wallpapers_name_is_kept_verbatim_in_config() {
        let dir = temp_dir("wallpaper-keep");
        let p = dir.join("config.json");
        std::fs::write(
            &p,
            br#"{"local":{"loginWallpaperFileName":"../../etc/passwd.png"}}"#,
        )
        .unwrap();
        let c = load(&p);
        assert_eq!(
            c.local.login_wallpaper_file_name.as_deref(),
            Some("../../etc/passwd.png"),
            "不合法的名字必须留在配置里（用户要能看到自己的错字）"
        );
        assert_eq!(wallpaper_mime("../../etc/passwd.png"), None);
        c.save(&p).unwrap();
        assert!(std::fs::read_to_string(&p)
            .unwrap()
            .contains("../../etc/passwd.png"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 大小上限是公开常量：注入侧与导入侧必须用同一个数（8 MiB）。
    #[test]
    fn wallpaper_size_cap_is_eight_mib() {
        assert_eq!(MAX_WALLPAPER_BYTES, 8 * 1024 * 1024);
    }
}
