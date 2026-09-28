// 设置窗归一化（spec §6.5 / §8.4）。
//
// **本文件只在「用户刚输入的值 → 提交 patch」这一段使用**（`normalizeModsEntry`），
// 以及白名单的去重/小写这类纯文本整理。IPC **返回值**不得再走这里：Rust 在 `load` 与
// 每次 `set_config` 都跑过 `Config::normalize`，回包已经是权威值；再夹一次会让
// 「窗口显示的值」与「页面生效的值」差一个通道——`clampLightness` **不是不动点**
// （`#cec1b2` → `#c4b4a2` → 再夹一次 → `#c4b4a1`，而页面按 `#c4b4a2` 生效）。
// 单点收口在 `app.js::adoptConfig`（那里有完整说明与回归测试）。
//
// 语义上本文件与 Rust 侧 `config.rs::Config::normalize` 一一对应：
//
// | 键 | 规则 | Rust |
// |---|---|---|
// | brandColor | 明度夹到 30%–70%，非法 → #0066ff | `normalize_brand_color` |
// | titlebarStyle | 非 `mac` → `windows` | `Config::normalize` |
// | launchpadStyle | 非 `spotlight` → `classic` | 同上 |
// | desktopIconLayoutMode | 非 `fixed` → `adaptive` | 同上 |
// | desktopIconPerColumn | 夹到 4–16；非有限数字 / 非十进制数字字符串 → 8 | 同上 + `sanitize_u32_field` |
// | fontWeight | 非 450/normal/600 → 空串 | 同上 |
// | lockscreenDefaultUsername | 截断到 80（按**码点**，与 Rust `chars().take(80)` 一致） | 同上 |
// | enabledOrigins | trim + 小写（**只小写 ASCII**）+ 丢空 + 大小写不敏感去重 | 同上（R23） |
// | launchpadIconRedrawMap | 只留 `^prefect_icon/[a-z0-9-]+\.png$`（**大小写不敏感**，R30） | `is_valid_prefect_icon_path` |
//
// 与 Rust 有意不同的一点（**不是**遗漏）：`Config::normalize` 会把 `shell.nasUrl` 的 origin
// 并入 `mods.enabledOrigins`，但那段逻辑在 `shell` 段上，`normalizeMods(mods)` 拿不到
// `shell`。好在设置窗读到的 `mods` 就是 Rust 归一化后的内存态（`get_config` 返回
// `AppState` 里那份），该并入早已发生——因此本函数只做幂等的那部分。
//
// 类型（bool / 数组 / 对象）由 Rust 侧 `sanitize_config_value` + serde 保证，
// 这里不再做第二遍类型强转；未知键一律丢弃（`MODS_KEYS` 白名单）。

export const DEFAULT_BRAND_COLOR = '#0066ff';
const FONT_WEIGHTS = ['450', 'normal', '600'];
const MAX_USERNAME_CHARS = 80;

/** 受支持的上游 `mods` 键（= Rust `ModsConfig` 的 25 个字段，顺序同 config.rs）。 */
export const MODS_KEYS = [
  'enabledOrigins', 'autoEnableSuspectedFnOS', 'basePresetEnabled', 'windowAnimationBlurEnabled',
  'titlebarStyle', 'launchpadStyle', 'desktopIconLayoutEnabled', 'desktopIconLayoutMode',
  'desktopIconPerColumn', 'desktopIconPerColumnEnabled', 'launchpadIconScaleEnabled',
  'launchpadIconScaleSelectedKeys', 'launchpadIconMaskOnlyKeys', 'launchpadIconRedrawKeys',
  'launchpadIconRedrawMap', 'brandColor', 'fontOverrideEnabled', 'fontFamily',
  'fontMonospaceFamily', 'fontWeight', 'fontFeatureSettings', 'fontFaceName', 'fontUrl',
  'customCodeEnabled', 'lockscreenDefaultUsername'
];

const DEFAULTS = {
  enabledOrigins: [], autoEnableSuspectedFnOS: true, basePresetEnabled: true,
  windowAnimationBlurEnabled: true, titlebarStyle: 'windows', launchpadStyle: 'classic',
  desktopIconLayoutEnabled: true, desktopIconLayoutMode: 'adaptive', desktopIconPerColumn: 8,
  desktopIconPerColumnEnabled: null, launchpadIconScaleEnabled: false,
  launchpadIconScaleSelectedKeys: [], launchpadIconMaskOnlyKeys: [], launchpadIconRedrawKeys: [],
  launchpadIconRedrawMap: {}, brandColor: DEFAULT_BRAND_COLOR, fontOverrideEnabled: false,
  fontFamily: '', fontMonospaceFamily: '', fontWeight: '', fontFeatureSettings: '',
  fontFaceName: 'FnOSCustomFont', fontUrl: '', customCodeEnabled: false, lockscreenDefaultUsername: ''
};

function hexToRgb(hex) {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  if (!/^[0-9a-f]{6}$/i.test(full)) return null;
  return [parseInt(full.slice(0, 2), 16) / 255, parseInt(full.slice(2, 4), 16) / 255, parseInt(full.slice(4, 6), 16) / 255];
}

/**
 * `#rrggbb`（或 `#rgb`）→ HSL，明度夹到 30%–70%，再转回 `#rrggbb`；非法输入回落默认色。
 * 与 Rust `normalize_brand_color` 逐步对应。
 */
export function clampLightness(input) {
  const rgb = hexToRgb(String(input || '').trim());
  if (!rgb) return DEFAULT_BRAND_COLOR;
  const [r, g, b] = rgb;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  let l = (max + min) / 2;
  const d = max - min;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  let h = 0;
  if (d !== 0) {
    if (max === r) h = 60 * (((g - b) / d) % 6);
    else if (max === g) h = 60 * ((b - r) / d + 2);
    else h = 60 * ((r - g) / d + 4);
  }
  if (h < 0) h += 360;
  l = Math.min(0.70, Math.max(0.30, l));
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  let rr = 0, gg = 0, bb = 0;
  if (h < 60) [rr, gg, bb] = [c, x, 0];
  else if (h < 120) [rr, gg, bb] = [x, c, 0];
  else if (h < 180) [rr, gg, bb] = [0, c, x];
  else if (h < 240) [rr, gg, bb] = [0, x, c];
  else if (h < 300) [rr, gg, bb] = [x, 0, c];
  else [rr, gg, bb] = [c, 0, x];
  const to = (v) => Math.round(Math.min(1, Math.max(0, v + m)) * 255).toString(16).padStart(2, '0');
  return `#${to(rr)}${to(gg)}${to(bb)}`;
}

/**
 * 上游 `launchpadIconRedrawMap` 的取值约束：`^prefect_icon/[a-z0-9-]+\.png$`，**按大小写
 * 不敏感**判定——与 Rust `config.rs::is_valid_prefect_icon_path` 逐条同义（R30 的 JS 镜像；
 * fix round 1 才真正对齐，见下）。
 *
 * 为什么需要 `i`：图标资源在磁盘上是 camelCase（`src-tauri/assets/fnos-mods/prefect_icon/`
 * 下就是 `panIndex.png`），shim 建索引时把小写化后的键当唯一键（`shim.js` 的
 * `assetIndex[String(k).toLowerCase()]`，`tests/shim.test.mjs` 有一条用例锁着「大小写不
 * 敏感」），因此 `prefect_icon/Emby.png` / `PREFECT_ICON/emby.PNG` 在运行期**都能解析到同一份
 * 资源**。Rust 侧已放行这些值，而设置窗在**提交 patch 之前**跑本函数
 * （`app.js::commit` → `normalizeForSubmit` → `normalizeModsEntry`，键来自 `MODS_KEYS`），
 * 旧的全小写正则会把它们**静默丢掉**——用户看到的现象是「设置了完美图标，页面却没变化」，
 * 正是 R30 要根除的静默丢配置。
 *
 * 放宽的**只有大小写这一维**（与 Rust 完全一致）：`..` 穿越、子目录
 * （`prefect_icon/sub/a.png`）、反斜杠（`prefect_icon\a.png`：shim 的查表键用 `/`，反斜杠
 * 永远解析不到资源）、双扩展名（`a.png.png`）、空名、空格、非 ASCII 一律照旧拒绝。
 *
 * 三个容易写错的语义细节（都选了与 Rust 同义的那一种）：
 * - `/i` **不带 `u`** 时只折 ASCII（`[a-z]` 只是多匹配 `A-Z`，不会把 `K`(U+212A) 折成 `k`），
 *   与 Rust 的 `eq_ignore_ascii_case` 同一种语义；带上 `u` 反而引入 Unicode 折叠，两侧就不再
 *   同义了。
 * - `$`（不带 `m`）只匹配输入末尾，所以尾随换行**仍是拒绝**——与 Rust 的 `strip_suffix`
 *   逐字节比较一致（`prefect_icon/a.png\n` 两侧都拒）。
 * - 名称字符集不含 `.`，所以 `a.png.png` 是「主体含 `.`」而被拒（与 Rust 的判定顺序同结论）。
 *
 * 两侧的**输入表**逐行相同：Rust 侧是 `config.rs::tests::redraw_map_regex_is_case_insensitive_only`
 * （20 条断言 + `normalize()` 的 retain/幂等两条），JS 侧是 `tests/normalize.test.mjs` 的
 * `PREFECT_ICON_PATH_TABLE`；`commands.rs::tests::prefect_icon_rule_mirror_stays_in_step` 还
 * 逐字锁住本常量的正则文本（含 `i`）并逐行核对那张表。**改一侧必须同时改另一侧。**
 */
export const PREFECT_ICON_PATH = /^prefect_icon\/[a-z0-9-]+\.png$/i;

/** 上游 `launchpadIconRedrawMap` 的单值判定（= Rust `is_valid_prefect_icon_path`）。 */
export function isPrefectIconPath(v) {
  return typeof v === 'string' && PREFECT_ICON_PATH.test(v);
}

/** 白名单条目：trim + **只小写 ASCII**（R23：上游按 `location.origin` 大小写敏感比较）。
 *
 * 用 `to_ascii_lowercase` 的语义（`[A-Z]` 逐个映射），不用 `String#toLowerCase()`：
 * 后者按 Unicode 折叠（`ПРИМЕР` → `пример`），而 Rust 侧是 `to_ascii_lowercase()`，
 * 非 ASCII 大写字母**保持原样**（`config.rs:584`）。文档表格声称两侧语义一一对应，
 * 这里就必须是同一个函数语义，否则手改过的白名单条目在设置窗里显示的字符串与页面
 * 实际比对的字符串不同（§8.4 的原缺陷类别）。
 */
export function normalizeOrigin(origin) {
  return asciiLower(String(origin == null ? '' : origin).trim());
}

/** 只折 ASCII 大写字母（= Rust `str::to_ascii_lowercase`）。 */
function asciiLower(s) {
  return s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32));
}

/** 十进制数字面量（与 Rust `str::parse::<f64>()` 接受的形式对齐；**不含** `0x10` 这种十六进制）。 */
const DECIMAL = /^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$/;

/**
 * 取数值：只认「有限数字」与「十进制数字字符串」，其余（`null` / `''` / `'abc'` / bool /
 * 数组对象 / `Infinity` / 十六进制字符串）一律 `null`。
 *
 * 与 Rust `config.rs::as_f64` 逐步对应：`Value::Null` 与空串在 Rust 侧 `parse` 失败 →
 * 字段被删 → serde 默认值（8），而 JS 的 `Number(null)` / `Number('')` 会得到 0——若不
 * 区分，同一份配置在设置窗显示 `4`、在 Rust 里是 `8`。这就是「语义对齐」的具体内容。
 */
function asFiniteNumber(v) {
  if (typeof v === 'number') return Number.isFinite(v) ? v : null;
  if (typeof v === 'string') {
    const t = v.trim();
    if (!t || !DECIMAL.test(t)) return null;
    const n = Number(t);
    return Number.isFinite(n) ? n : null;
  }
  return null;
}

/**
 * 用户输入的站点地址 → 规范 origin（`scheme://host[:port]`，ASCII 小写、省略默认端口）。
 *
 * **只接受 `http://` / `https://` 开头的绝对地址**，其余一律返回空串（调用方视为校验失败）。
 * 为什么不能只靠 `new URL(value).origin` + `if (!origin)`：URL 的 scheme 允许含 `.`，
 * 于是 `nas.example.com:8000` 被当成「scheme = `nas.example.com`」、`8000` 成了 opaque path，
 * `.origin` 返回**字符串 `"null"`**；`"null"` 是 truthy，旧写法拦不住，junk 会被写进
 * `config.json` 的白名单（渲染成一个永远匹配不上的条目）。`nas:8000` / `mailto:` /
 * `data:` / `javascript:` / `ftp:` 同理。
 */
export function parseHttpOrigin(input) {
  const raw = String(input == null ? '' : input).trim();
  // 第一道闸：scheme 必须是 http(s)。正则通过不代表能解析，所以下面还要真解析一次。
  if (!/^https?:\/\//i.test(raw)) return '';
  let url;
  try {
    url = new URL(raw);
  } catch (e) {
    return '';
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return '';
  const origin = normalizeOrigin(url.origin);
  // 对 http(s) 不该出现 `"null"`；留作回归防线（这个字面量正是缺陷 B 的载体）。
  return origin === 'null' ? '' : origin;
}

/**
 * 整份 `mods` 的归一化（与 Rust `ModsConfig` 部分同义）。
 *
 * 用途只剩两个：给用户输入做提交前的整理（`normalizeModsEntry`），以及测试里直接比对
 * 语义。**不要**把它套在 IPC 回包上（`app.js::adoptConfig` 的注释解释了二次夹取的危害）。
 * 未知键一律丢弃（`MODS_KEYS` 白名单）。
 */
export function normalizeMods(input) {
  const src = input && typeof input === 'object' ? input : {};
  const out = {};
  for (const key of MODS_KEYS) {
    const value = key in src ? src[key] : DEFAULTS[key];
    out[key] = value === undefined ? DEFAULTS[key] : value;
  }

  out.brandColor = clampLightness(out.brandColor);
  if (out.titlebarStyle !== 'mac') out.titlebarStyle = 'windows';
  if (out.launchpadStyle !== 'spotlight') out.launchpadStyle = 'classic';
  if (out.desktopIconLayoutMode !== 'fixed') out.desktopIconLayoutMode = 'adaptive';

  const n = asFiniteNumber(out.desktopIconPerColumn);
  out.desktopIconPerColumn = n === null ? 8 : Math.min(16, Math.max(4, Math.round(n)));
  if (!FONT_WEIGHTS.includes(out.fontWeight)) out.fontWeight = '';

  // 按码点截断（`Array.from` ≈ Rust `chars()`），不用 `slice` 以免切断代理对
  out.lockscreenDefaultUsername = Array.from(String(out.lockscreenDefaultUsername || ''))
    .slice(0, MAX_USERNAME_CHARS)
    .join('');

  const seen = new Set();
  out.enabledOrigins = (Array.isArray(out.enabledOrigins) ? out.enabledOrigins : [])
    .map(normalizeOrigin)
    .filter((o) => {
      if (!o || seen.has(o)) return false;
      seen.add(o);
      return true;
    });

  const map = {};
  const rawMap = out.launchpadIconRedrawMap && typeof out.launchpadIconRedrawMap === 'object'
    ? out.launchpadIconRedrawMap
    : {};
  for (const [k, v] of Object.entries(rawMap)) if (isPrefectIconPath(v)) map[k] = v;
  out.launchpadIconRedrawMap = map;

  return out;
}

/**
 * 单个 `mods` 键的归一化：**用户刚输入的值**在提交 patch 之前走这里（`app.js::commit`）。
 *
 * `key` 必须是 `MODS_KEYS` 之一（其余键返回 `undefined`，调用方不要用它）。
 *
 * 为什么单独给一个入口：归一化只能对「用户输入」跑**一次**。IPC 回包已经过 Rust 的
 * `Config::normalize`，再跑一次就是缺陷 A 的「二次夹取」——`clampLightness` 不是不动点，
 * 二次夹取会让界面显示的值与页面生效的值差一个通道（`#c4b4a2` → `#c4b4a1`）。
 */
export function normalizeModsEntry(key, value) {
  return normalizeMods({ ...DEFAULTS, [key]: value })[key];
}
