// 设置窗归一化（spec §6.5 / §8.4）。
//
// 为什么必须有一份 JS 侧归一化：设置窗拿到 `mods` 后要**显示当前值**；若直接显示原始值，
// 就会出现「设置窗显示 A、页面按 B 生效」（§6.5 的原话）。本文件与 Rust 侧
// `config.rs::ModsConfig` 的归一化语义一一对应：
//
// | 键 | 规则 | Rust |
// |---|---|---|
// | brandColor | 明度夹到 30%–70%，非法 → #0066ff | `normalize_brand_color` |
// | titlebarStyle | 非 `mac` → `windows` | `Config::normalize` |
// | launchpadStyle | 非 `spotlight` → `classic` | 同上 |
// | desktopIconLayoutMode | 非 `fixed` → `adaptive` | 同上 |
// | desktopIconPerColumn | 夹到 4–16，非数字 → 8 | 同上 + `sanitize_u32_field` |
// | fontWeight | 非 450/normal/600 → 空串 | 同上 |
// | lockscreenDefaultUsername | 截断到 80（按**码点**，与 Rust `chars().take(80)` 一致） | 同上 |
// | enabledOrigins | trim + 小写 + 丢空 + 大小写不敏感去重 | 同上（R23） |
// | launchpadIconRedrawMap | 只留 `^prefect_icon/[a-z0-9-]+\.png$` | `is_valid_prefect_icon_path` |
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

/** 上游 `launchpadIconRedrawMap` 的取值约束（= Rust `is_valid_prefect_icon_path`）。 */
export function isPrefectIconPath(v) {
  return typeof v === 'string' && /^prefect_icon\/[a-z0-9-]+\.png$/.test(v);
}

/** 白名单条目：trim + 小写（R23：上游按 `location.origin` 大小写敏感比较）。 */
export function normalizeOrigin(origin) {
  return String(origin == null ? '' : origin).trim().toLowerCase();
}

/** 单个 `mods` 键的归一化值（供设置窗显示与落盘前的最后一次对齐）。 */
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

  const n = Number(out.desktopIconPerColumn);
  out.desktopIconPerColumn = Number.isFinite(n) ? Math.min(16, Math.max(4, Math.round(n))) : 8;
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
