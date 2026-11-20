// 设置页的「完美图标 · 逐项」数据层（T14g 修复轮 3：上游设置界面退役，逐项 UI 移植过来）。
//
// 上游原来的逐项 UI 在 popup.js 里（应用列表来自页面上报、图标路径靠一串候选猜名字）。
// 本壳的版本把「猜」换成**显式选择**：应用列表照旧来自页面（`request_app_items` 请页面
// 重新汇报 + `get_page_report` 读回包），每个应用给三项：
//   · 统一形状（`launchpadIconScaleSelectedKeys`）
//   · 仅遮罩（`launchpadIconMaskOnlyKeys`）
//   · 重绘图标（`launchpadIconRedrawKeys` + `launchpadIconRedrawMap[key] = prefect_icon/<name>.png`）
// 三条数组的互斥关系与上游一致：**重绘优先**——一个 key 进了红绘集合，就从另外两条里去掉。
//
// 纯函数（可单测、不碰 DOM）：报告槽位解码、应用项形状校验、选择合并、路径取值约束。

/** 与 `src-tauri/assets/fnos-mods/prefect_icon/` 的 vendored 目录集合相等（测试锁定）。 */
export const PREFECT_ICONS = [
  'alist', 'emby', 'home-assistant', 'icloud', 'it-tools', 'kodi', 'one-panel',
  'oray-hsk', 'panindex', 'qbittorrent', 'quarkpan', 'syncthing', 'transmission', 'xunlei',
];

/** `launchpadIconRedrawMap` 值的形状——与 Rust `config::is_valid_prefect_icon_path` 同一约束。 */
export const PREFECT_ICON_PREFIX = 'prefect_icon/';
// 与 Rust `is_valid_prefect_icon_path` 同义：前缀/后缀**只折 ASCII 大小写**，名字是
// 字母/数字/连字符。JS 用 `i` 覆盖同一语义。
const PREFECT_ICON_PATH_RE = /^prefect_icon\/[a-z0-9-]+\.png$/i;

/** 图标名 → 配置里的相对路径；名字不在清单里返回 `null`（不猜、不拼）。 */
export function prefectIconPath(name) {
  const n = String(name || '').trim().toLowerCase();
  if (!PREFECT_ICONS.includes(n)) return null;
  return `${PREFECT_ICON_PREFIX}${n}.png`;
}

/** 路径是否合法（与 Rust 侧同一个正则；`prefectIconPath` 的产物必然通过）。 */
export function isPrefectIconPath(path) {
  return typeof path === 'string' && PREFECT_ICON_PATH_RE.test(path);
}

/**
 * `get_page_report` 的回包信封 → 两个槽位。
 * 宿主永远返回对象；这里对任何非对象输入回落成两个 `null`（升级中途的旧宿主也不炸）。
 */
export function pickReportSlots(envelope) {
  const e = envelope && typeof envelope === 'object' ? envelope : {};
  const pick = (v) => (v && typeof v === 'object' ? v : null);
  return { report: pick(e.report), appItems: pick(e.appItems) };
}

/** 应用项上报被接受的两个 type（与页面侧 shim 的同一个分支）。 */
export const APP_ITEM_REPORT_TYPES = ['FNOS_GET_LAUNCHPAD_APP_ITEMS', 'FNOS_GET_LAUNCHPAD_APP_TITLES'];

/** 「列表大到装不下」的诊断（`{tooLarge:true,itemCount:N}`）；严格比较，页面可控的字符串不算。 */
export function isTooLargeReport(report) {
  if (!report || typeof report !== 'object') return false;
  if (APP_ITEM_REPORT_TYPES.indexOf(report.type) < 0) return false;
  const payload = report.payload;
  return !!payload && typeof payload === 'object' && payload.tooLarge === true;
}

/**
 * 从页面上报里取应用项；取不到（没上报 / 形状不对）返回 `null`。
 * 每一项必须有非空字符串 `key`；`title` / `iconSrc` 只作为**显示用**的可选字段。
 * 页面可控文本只经 `textContent` 渲染（调用方负责）。
 */
export function appItemsFromReport(report) {
  if (!report || typeof report !== 'object') return null;
  if (APP_ITEM_REPORT_TYPES.indexOf(report.type) < 0) return null;
  const payload = report.payload;
  if (!payload || typeof payload !== 'object' || !Array.isArray(payload.items)) return null;
  const raw = payload.items;
  const list = raw
    .filter((item) => item && typeof item === 'object' && typeof item.key === 'string' && item.key.length > 0)
    .map((item) => ({
      key: item.key,
      title: typeof item.title === 'string' && item.title.trim() ? item.title.trim() : item.key,
      iconSrc: typeof item.iconSrc === 'string' ? item.iconSrc : '',
    }));
  // 有数据但**一条都认不出** = 形状不对（与「确实 0 个应用」的空数组不是一回事）
  if (raw.length > 0 && list.length === 0) return null;
  return list;
}

function toggleKey(list, key, on) {
  const set = new Set(Array.isArray(list) ? list : []);
  if (on) set.add(key);
  else set.delete(key);
  return [...set];
}

/**
 * 合并一次逐项选择变更。`current` 形如
 * `{scale: [], mask: [], redraw: {[key]: 'prefect_icon/x.png'}}`；
 * `change` 是 `{scale?: boolean, mask?: boolean, redraw?: '' | 'prefect_icon/x.png'}`。
 *
 * 返回新的 `{scale, mask, redraw}`（不修改入参）。互斥规则与上游一致：**重绘优先**。
 */
export function applyIconSelection(current, key, change) {
  const cur = current && typeof current === 'object' ? current : {};
  const next = {
    scale: Array.isArray(cur.scale) ? [...cur.scale] : [],
    mask: Array.isArray(cur.mask) ? [...cur.mask] : [],
    redraw: cur.redraw && typeof cur.redraw === 'object' ? { ...cur.redraw } : {},
  };
  if (typeof change.scale === 'boolean') next.scale = toggleKey(next.scale, key, change.scale);
  if (typeof change.mask === 'boolean') next.mask = toggleKey(next.mask, key, change.mask);
  if (change.redraw === '') delete next.redraw[key];
  else if (isPrefectIconPath(change.redraw)) next.redraw[key] = change.redraw;
  if (next.redraw[key]) {
    next.scale = next.scale.filter((k) => k !== key);
    next.mask = next.mask.filter((k) => k !== key);
  }
  return next;
}

/** 从配置的 mods 段读出现有选择（形状兜底；不认识的键直接忽略）。 */
export function selectionFromConfig(mods) {
  const m = mods && typeof mods === 'object' ? mods : {};
  const redraw = {};
  const raw = m.launchpadIconRedrawMap && typeof m.launchpadIconRedrawMap === 'object'
    ? m.launchpadIconRedrawMap : {};
  for (const [key, path] of Object.entries(raw)) {
    if (typeof key === 'string' && key && isPrefectIconPath(path)) redraw[key] = path;
  }
  return {
    scale: Array.isArray(m.launchpadIconScaleSelectedKeys)
      ? m.launchpadIconScaleSelectedKeys.filter((k) => typeof k === 'string' && k) : [],
    mask: Array.isArray(m.launchpadIconMaskOnlyKeys)
      ? m.launchpadIconMaskOnlyKeys.filter((k) => typeof k === 'string' && k) : [],
    redraw,
  };
}

/** 选择 → `set_config` 的 mods 补丁（四条键一起写，保证互斥关系落盘）。 */
export function selectionPatch(selection) {
  const s = selection || {};
  const redraw = s.redraw && typeof s.redraw === 'object' ? s.redraw : {};
  const keys = Object.keys(redraw).filter((k) => isPrefectIconPath(redraw[k]));
  return {
    mods: {
      launchpadIconScaleSelectedKeys: Array.isArray(s.scale) ? s.scale.filter((k) => !keys.includes(k)) : [],
      launchpadIconMaskOnlyKeys: Array.isArray(s.mask) ? s.mask.filter((k) => !keys.includes(k)) : [],
      launchpadIconRedrawKeys: keys,
      launchpadIconRedrawMap: Object.fromEntries(keys.map((k) => [k, redraw[k]])),
    },
  };
}

/** 图标路径 → 图标名（`prefect_icon/emby.png` → `emby`；不合法给空串）。 */
export function iconNameFromPath(path) {
  if (!isPrefectIconPath(path)) return '';
  return path.slice(PREFECT_ICON_PREFIX.length, -'.png'.length);
}
