// 设置窗主逻辑：schema 驱动渲染 + IPC 提交（spec §8）。
//
// 三条硬约束：
// 1. **显示的值 = 生效的值**（§8.4）：`state.config.mods` 一律原样采纳 `get_config` /
//    `set_config` 的返回（Rust 已经归一化过，它就是权威值），**不再二次归一化**——
//    `clampLightness` 不是不动点，二次夹取会让界面显示 `#c4b4a1` 而页面按 `#c4b4a2`
//    生效。归一化只保留给「用户刚输入的值」，在提交 patch 之前跑一次（见 `commit`）。
//    任何渲染都只读 `state.config`，绝不把 IPC 原始值或用户刚输入的原文画到界面上。
// 2. **提交后就地重渲染**：`set_config` 返回的是 Rust 归一化后的权威配置，
//    以它为准刷新界面（因此「取色器选了 #ffffff，右侧显示 #b3b3b3」是同一份数据的两个视图）。
// 3. `needsReload` 为真（只有 `shell.injectEnabled` / `shell.homeUrl` 会）时随后调
//    `reload_main`——Rust 侧销毁并按新载荷重建主窗口（§6.6 勘误）。
//
// 另外两条窗口级行为：
// - **焦点刷新**（`refresh`）：带外改动（手工编辑 `config.json`、将来任何新增的写入方）
//   不发事件，重新获得焦点时重取配置。托盘精简后托盘侧只剩「退出前保存窗口几何」
//   （`tray.rs` → `commands::save_window_geom`），不再有配置开关；见 `refresh` 的注释。
// - **关于页外链**：不在窗内导航，交给 Rust `open_url` → 系统默认浏览器。
import { SCHEMA, UPSTREAM_REPO, VENDOR_DIR, PREFECT_ICONS, prefectIconPath } from './schema.js';
import { MODS_KEYS, normalizeModsEntry, parseHttpOrigin, DEFAULT_BRAND_COLOR } from './normalize.js';
import { addCurrentOriginToWhitelist, cornerShapeHint, statusBar, statusFor } from './status.js';
import * as api from './bridge.js';

/** 配置的三个段（键前缀）。 */
const SECTIONS = ['mods', 'local', 'shell'];

/**
 * 界面状态。导出供单测与状态条读取。
 *
 * `page` 是 Rust 侧 `get_page_state` 的**最近一次**回包（主窗口 URL / 是否命中白名单 /
 * 上次加载是否失败）；`report` / `appItemsReport` 是 `get_page_report` 回包信封里的**两个
 * 槽位**（见 [`reportSlots`]）：前者是状态条证据（Task 13a 的上游注入链信号），后者是
 * 「完美图标」逐项 UI 的数据源（Task 13b 的应用项列表）。
 *
 * 为什么要拆成两个槽位：Task 13b 的 shim 会在完美图标启用时主动向上游拉取应用项列表，那条
 * 上报**晚于**上游的 `FNOS_INJECTION_TRIGGERED`。若是同一个「最近一次」槽位，状态条的强态
 * 判据就会被自己拉的数据冲掉（T13b 审计发现的回归）。宿主侧的分流见
 * `src-tauri/src/report.rs::is_app_items_report`。
 *
 * 三者相互独立：主窗口可以只导航不改配置，而页面上报又只在页面自己跑完注入链（或应答了
 * 列表请求）后才出现，所以 `refresh()` 三条都要取（见那里的注释）。
 */
export const state = {
  config: null, active: null, error: null, page: null, report: null, appItemsReport: null,
};

/**
 * `get_page_report` 的回包信封 → 两个槽位（两个字段各自可能缺失/不是对象 → `null`）。
 *
 * 宿主**永远**返回对象（`{report, appItems}`），但设置窗可能正跑在一个更老的宿主上
 * （升级中途），所以这里对非对象输入一律回落成两个 `null`，绝不把 `undefined` 画进界面。
 */
export function reportSlots(envelope) {
  const e = envelope && typeof envelope === 'object' ? envelope : {};
  const pick = (v) => (v && typeof v === 'object' ? v : null);
  return { report: pick(e.report), appItems: pick(e.appItems) };
}

// ---------- DOM 小工具 ----------

function el(tag, opts = {}) {
  const node = document.createElement(tag);
  if (opts.id) node.id = opts.id;
  if (opts.className) node.className = opts.className;
  if (opts.text != null) node.textContent = opts.text;
  if (opts.attrs) for (const [k, v] of Object.entries(opts.attrs)) node.setAttribute(k, String(v));
  return node;
}

function button(text, className) {
  const b = el('button', { text, className });
  b.type = 'button'; // 显式：避免将来包进 <form> 时变成提交按钮
  return b;
}

function message(e) {
  return String((e && e.message) || e || '未知错误');
}

// ---------- 配置路径解析 ----------

/** `mods.x` / `shell.y` / `local.z`；无前缀按 `mods.x`（与上游 popup 的写法兼容）。 */
export function resolvePath(key) {
  const head = String(key).split('.')[0];
  return SECTIONS.includes(head) ? String(key) : `mods.${key}`;
}

/** 设置项的 DOM id：`mods.basePresetEnabled` → `f_mods_basePresetEnabled`。 */
export function fieldId(key) {
  return `f_${resolvePath(key).replace(/\./g, '_')}`;
}

function readValue(config, key) {
  let cur = config;
  for (const part of resolvePath(key).split('.')) {
    if (cur == null) return undefined;
    cur = cur[part];
  }
  return cur;
}

function setPath(obj, path, value) {
  const parts = path.split('.');
  const last = parts.pop();
  let cur = obj;
  for (const p of parts) {
    if (cur[p] == null || typeof cur[p] !== 'object') cur[p] = {};
    cur = cur[p];
  }
  cur[last] = value;
  return obj;
}

/**
 * 把 IPC 返回的配置收进 state。
 *
 * **`mods` 原样采纳，不做任何归一化**（Review finding A / §8.4）。理由：
 * Rust 的 `Config::normalize` 在 `load` 与**每次** `set_config` 都跑过，`get_config` /
 * `set_config` 回包里的 `mods` 就是「页面实际生效的那份值」。JS 侧再夹一次会引入
 * 一个单通道偏差，因为 `clampLightness` **不是不动点**：
 *
 *   `#cec1b2` --Rust--> `#c4b4a2` --JS 再夹一次--> `#c4b4a1`
 *
 * 于是设置窗显示 `#c4b4a1`（取色器与 `f_mods_brandColor_value` 都是它），而页面按
 * `#c4b4a2` 生效——正是 §8.4 要根除的「显示 A、生效 B」。实测 20 万随机色里约 40 个
 * 落在这种「再夹一次就变」的带上，所以手写/历史遗留颜色很容易踩到。
 *
 * 归一化只剩一个合法入口：用户刚输入的值，在提交 patch 之前（`commit` →
 * `normalizeModsEntry`）。回归测试见 `tests/settings.test.mjs` 的 `#cec1b2` 案例。
 */
export function adoptConfig(raw) {
  if (!raw || typeof raw !== 'object') return;
  // 只做「形状」兜底（缺 `mods` 时给空对象，避免渲染期到处判空），不改任何值。
  const mods = raw.mods && typeof raw.mods === 'object' ? raw.mods : {};
  state.config = { ...raw, mods };
}

// ---------- 提交 ----------

/**
 * 提交前的最后一次归一化：**只对 `mods.*` 白名单键**做（与 Rust 同义）。
 *
 * 这是归一化的唯一入口——它作用在「用户刚输入的值」上，绝不作用在 IPC 回包上
 * （`adoptConfig` 的注释说明了二次夹取的危害）。`shell.*` / `local.*` 原样提交，
 * 由 Rust 侧按各自规则处理。
 */
function normalizeForSubmit(path, value) {
  const [section, ...rest] = path.split('.');
  const sub = rest.join('.');
  return section === 'mods' && MODS_KEYS.includes(sub) ? normalizeModsEntry(sub, value) : value;
}

/** 提交一个设置项：`set_config` → 需要时 `reload_main` → 用返回的权威配置重渲染。 */
async function commit(key, value, node) {
  if (node) node.classList.add('pending');
  const path = resolvePath(key);
  try {
    const res = await api.setConfig(setPath({}, path, normalizeForSubmit(path, value)));
    adoptConfig(res.config);
    // gap (a)：只有 injectEnabled / homeUrl 变更才会是 true，此时必须重建主窗口
    if (res.needsReload) {
      await api.reloadMain(null);
      // 重建会换掉主窗口那一整次加载：状态条必须跟着换，否则会一直显示旧页面的判定
      await fetchPageSnapshot();
    }
    state.error = null;
    render();
    renderStatus();
  } catch (e) {
    state.error = `保存「${key}」失败：${message(e)}`;
    render();
  } finally {
    if (node && node.isConnected) node.classList.remove('pending');
  }
}

// ---------- 完美图标：逐项语义（Task 13b） ----------
//
// **上游的语义（照抄，不发明形状）**：`content-script.js:755-794`
//   - 光有 `launchpadIconRedrawMap[key]` 不够：`redrawKeys` 也必须包含 key，
//     上游用 `normalizeLaunchpadKeyList(redrawKeys).filter(k => typeof map[k] === 'string')`
//     重建 `currentLaunchpadIconRedrawMap`（cs:764-772）；
//   - **重绘优先于另外两种**：`maskOnlyKeys` 与 `scaleSelectedKeys` 都会被剔除掉已经在
//     `redrawSet` 里的 key（cs:773-778）；
//   - 三种处置**全部**受 `launchpadIconScaleEnabled` 总开关约束
//     （cs:644-648 的 `enabled && shouldXxx(...)`）。
// 所以逐项 UI 每个应用只有一个「处置」：不处理 / 缩放 / 仅遮罩 / 重绘为某个内置图标。
//
// **订正（fix round 1 / Minor 6）**：上一条曾写成「三种处置互斥，重绘优先」——**这是错的**。
// cs:773-778 只做了「把已在 `redrawSet` 里的 key 从另外两个 list 里删掉」这一件事；上游**没有**
// 在 `maskOnlyKeys` 与 `scaleSelectedKeys` 之间做任何互斥（cs:633-652 里两个判定各自独立，
// 一个 key 可以同时 `shouldScale` 与 `shouldMaskOnly`，两个 class 都会被 toggle 上）。
// 真正成立的性质只有两条：① 重绘会盖过另外两种；② 三种都受总开关约束。
//
// 那么「一个下拉、四种取值」的 UI 模型还站得住吗？**站得住，但理由是 UI 自己的选择**：
// 一个下拉天然只能表达一个值（这正是「每项一个处置」的交互模型），而它写出去的四个键在
// 上游那边**各自独立生效**（`applyIconSelection` 每次都先把这一项的三种成员身份全清掉、
// 只加回选中的那一种，见那里的注释），所以「下拉选了 A 就不会再留着 B」是**本壳写入时的
// 规范化**，不是上游强制的互斥。用户若手工在配置里同时写上两个 list，页面会两个都应用——
// 这与上面两条真性质都不冲突。

/** 应用项上报被接受的两个 type（上游 cs:2853-2855 的同一个分支）。 */
const APP_ITEM_REPORT_TYPES = ['FNOS_GET_LAUNCHPAD_APP_ITEMS', 'FNOS_GET_LAUNCHPAD_APP_TITLES'];

/**
 * 这条上报是不是 shim 在「列表大到装不下」时回的**诊断**（fix round 1 / Minor 5）。
 *
 * 形状由 `src-tauri/inject/shim.js::noteAppItemsSend` 定义：`{items:[],titles:[],tooLarge:true,
 * itemCount:N}`。它走的是**同一条**应用项通道（宿主的分流判据只看 type），所以宿主侧不需要
 * 任何新命令/新槽位，设置窗只是多认一个字段。
 *
 * 为什么需要它：清单大到超过分片预算时，shim 不会发那份列表（越界即拒），而重发同一份列表
 * 永远不会成功、重试循环因此停下。没有这条诊断的话，UI 只会说「还没收到可用的应答」——
 * 一个永远等不到结果的谎。`tooLarge === true` 用严格比较：页面可控的 `"true"` 字符串不算。
 */
export function isTooLargeReport(report) {
  if (!report || typeof report !== 'object') return false;
  if (APP_ITEM_REPORT_TYPES.indexOf(report.type) < 0) return false;
  const payload = report.payload;
  return !!payload && typeof payload === 'object' && payload.tooLarge === true;
}

/**
 * 从最近一次页面上报里取出应用项列表；取不到（没上报 / 形状不对）返回 `null`。
 *
 * 只认**形状**，不做任何猜测：`payload.items` 必须是数组，元素必须有非空字符串 `key`
 * （`iconSrc` / `title` 缺失时由渲染侧兜底）。返回 `[]` 是「上游确实回报了 0 个应用」——
 * 与 `null`（没有可用数据）在下游是两种不同的文案，不能混。
 *
 * 页面可控文本（`title` / `key` / `iconSrc`）**只经 textContent 渲染**，绝不做 HTML 拼接。
 */
export function appItemsFromReport(report) {
  if (!report || typeof report !== 'object') return null;
  if (APP_ITEM_REPORT_TYPES.indexOf(report.type) < 0) return null;
  const payload = report.payload;
  if (!payload || typeof payload !== 'object' || !Array.isArray(payload.items)) return null;
  return payload.items.filter((item) => (
    item && typeof item === 'object' && typeof item.key === 'string' && item.key.length > 0
  ));
}

/** 某个应用项当前的处置：`'off'` / `'scale'` / `'mask'` / `'redraw:<path>'`。 */
export function iconSelectionFor(mods, key) {
  const m = mods && typeof mods === 'object' ? mods : {};
  const map = m.launchpadIconRedrawMap && typeof m.launchpadIconRedrawMap === 'object'
    ? m.launchpadIconRedrawMap
    : {};
  const path = typeof map[key] === 'string' ? map[key] : '';
  const list = (v) => (Array.isArray(v) ? v : []);
  if (path && list(m.launchpadIconRedrawKeys).indexOf(key) >= 0) return `redraw:${path}`;
  if (list(m.launchpadIconMaskOnlyKeys).indexOf(key) >= 0) return 'mask';
  if (list(m.launchpadIconScaleSelectedKeys).indexOf(key) >= 0) return 'scale';
  return 'off';
}

/**
 * 把一个应用项的处置换算成四个键的**下一个值**（四条键的语义与「重绘优先」见上面的文件级注释）。
 *
 * 返回的对象用配置键名做字段（可以直接 `set_config`）；`config` 只读不改。
 */
export function applyIconSelection(mods, key, choice) {
  const m = mods && typeof mods === 'object' ? mods : {};
  const scale = new Set(Array.isArray(m.launchpadIconScaleSelectedKeys) ? m.launchpadIconScaleSelectedKeys : []);
  const mask = new Set(Array.isArray(m.launchpadIconMaskOnlyKeys) ? m.launchpadIconMaskOnlyKeys : []);
  const redraw = new Set(Array.isArray(m.launchpadIconRedrawKeys) ? m.launchpadIconRedrawKeys : []);
  const map = m.launchpadIconRedrawMap && typeof m.launchpadIconRedrawMap === 'object'
    ? { ...m.launchpadIconRedrawMap }
    : {};

  // 先把这一项的三种成员身份全清掉，再按需要加回唯一的一种。这是**本壳写入时的规范化**：
  // 一个下拉只能表达一个值，而四个键在上游各自独立生效（上游只保证「重绘盖过另外两种」，
  // 见文件级注释的订正）。
  scale.delete(key);
  mask.delete(key);
  redraw.delete(key);
  delete map[key];

  if (choice === 'scale') {
    scale.add(key);
  } else if (choice === 'mask') {
    mask.add(key);
  } else if (typeof choice === 'string' && choice.indexOf('redraw:') === 0) {
    const path = choice.slice('redraw:'.length);
    if (PREFECT_ICONS.some((name) => prefectIconPath(name) === path)) {
      redraw.add(key);
      map[key] = path;
    }
  }

  return {
    launchpadIconScaleSelectedKeys: Array.from(scale),
    launchpadIconMaskOnlyKeys: Array.from(mask),
    launchpadIconRedrawKeys: Array.from(redraw),
    launchpadIconRedrawMap: map,
  };
}

/** 「重绘」下拉里的一项：[值, 显示文本]（显示文本只用内置名，不含任何页面可控文本）。 */
function redrawOption(name) {
  return [`redraw:${prefectIconPath(name)}`, `重绘：${name}`];
}

/** 逐项处置的下拉选项（顺序：不处理 → 缩放 → 仅遮罩 → 14 个重绘目标）。 */
export function iconChoiceOptions() {
  return [['off', '不处理'], ['scale', '缩放'], ['mask', '仅遮罩']]
    .concat(PREFECT_ICONS.map(redrawOption));
}

/** 壁纸大小上限（与 `config::MAX_WALLPAPER_BYTES` / 宿主命令同一个数，这里早退一次省一次 IPC）。 */
export const MAX_WALLPAPER_BYTES = 8 * 1024 * 1024;

/**
 * 「没有可用应用项列表」时的**如实**说明。
 *
 * 五种情形必须说五种话（`null` 与 `[]` 是不同的事实）：
 * - 上游的列表**大到装不下**（shim 回的诊断）→ 说清「列表过大，无法上报」并给出项数；
 * - 没有任何应用项上报 → 「还没收到」，并说明它什么时候才会有（启动台渲染出应用图标时）；
 * - 只是 shim 自己发出的**请求**（`dir === 'out'`）→ 「已经问过，但还没有可用的应答」；
 * - 有应答但形状不对 → 「已按拒绝处理」，不画任何项；
 * - 别种 type → 把 type 说出来（只回显形如 `FNOS_XXX` 的串，页面可控文本不原样进 UI）。
 */
export function appListEmptyText(report, items) {
  // 列表过大这条**必须排在 `items.length === 0` 之前**：shim 的诊断上报里 items 就是空数组
  // （它只能是空的——装得下就不叫过大了），排在后面会把它误说成「上游回报了 0 个应用项」。
  // `itemCount` 是宿主算出来的数字（Number.isFinite 收口），页面可控文本一个字符都不进文案。
  if (isTooLargeReport(report)) {
    const n = Number(report.payload.itemCount);
    const howMany = Number.isFinite(n) && n > 0 ? `${n} 个` : '过多';
    return `应用项列表过大（${howMany}），超过分片上报的预算（8 片 × 3000 字节），无法上报：`
      + '设置窗因此拿不到逐项列表。请减少启动台中的应用数量，或点下面的「打开配置目录」手工编辑'
      + '（逐项映射写在 `launchpadIconRedrawMap` / `launchpadIconRedrawKeys`）。';
  }
  if (items && items.length === 0) {
    return '上游回报了 0 个应用项：启动台的图标还没渲染出来时只能收集到空列表。'
      + '请回到主窗口打开一次启动台，再切回本窗口（会自动刷新）。';
  }
  const isObject = !!report && typeof report === 'object';
  const type = isObject ? report.type : null;
  // 只回显形如 `FNOS_XXX` 的串：页面上报的 type 已经过宿主白名单，这里再收一次口，
  // 免得任何奇怪形状的字符串被画进界面（它只用于**说明**，不进任何判据）。
  const known = typeof type === 'string' && /^FNOS_[A-Z_]{2,40}$/.test(type) ? type : '';
  if (!isObject) {
    return '尚未收到应用项列表：它由主窗口页面经上报通道回报，而上游只能在启动台渲染出应用图标时'
      + '收集到（content-script.js:595-610）。请回到主窗口打开一次启动台，再切回本窗口（会自动刷新）；'
      + '也可以点下面的「打开配置目录」手工编辑。';
  }
  if (APP_ITEM_REPORT_TYPES.indexOf(type) >= 0 && report.dir === 'out') {
    // 这一条是 shim **自己发出的请求**（宿主原样存下来了）：说明请求已经送到页面，但上游还没有
    // 答出一份非空的列表——空列表的应答由 shim 主动丢弃（否则会把上一次的真实列表覆盖成 0 项）。
    return '已经向页面请求过应用项列表，但还没收到可用的应答：上游只在启动台渲染出应用图标时'
      + '才能收集到（content-script.js:595-610）。请回到主窗口打开一次启动台，再切回本窗口'
      + '（会自动刷新）；也可以点下面的「打开配置目录」手工编辑。';
  }
  if (APP_ITEM_REPORT_TYPES.indexOf(type) >= 0) {
    // 声称是应用项列表，却没有可用的 items（形状不对 / 上报被组装后被拒）
    return '最近一次上报声称是应用项列表，但形状不可用，已按拒绝处理（不画任何项）。'
      + '请回到主窗口打开一次启动台让它重新回报；也可以点下面的「打开配置目录」手工编辑。';
  }
  if (known) {
    return `最近一次上报是 ${known}，不含应用项列表（可能是主窗口刚加载完，或启动台还没打开）。`
      + '请回到主窗口打开一次启动台，再切回本窗口；也可以点下面的「打开配置目录」手工编辑。';
  }
  return '最近一次上报不含应用项列表。请回到主窗口打开一次启动台，再切回本窗口；'
    + '也可以点下面的「打开配置目录」手工编辑。';
}

/**
 * 一组「打开配置目录」的入口（应用项列表取不到时的手工编辑指引）。
 *
 * 用的还是既有的 `open_config_dir` 命令（关于页也有同一个按钮）——**不新增任何命令**，
 * 因此设置窗的授权面在这一轮只多了 `import_wallpaper` 一条。
 */
function configDirRow(idPrefix) {
  const row = el('div', { className: 'app-actions' });
  row.appendChild(el('span', {
    className: 'hint',
    text: '手工编辑入口（mods.launchpadIconRedrawMap / launchpadIconRedrawKeys）：'
  }));
  const open = button('打开配置目录');
  open.id = `${idPrefix}_open`;
  open.addEventListener('click', async () => {
    try {
      await api.openConfigDir();
      state.error = null;
    } catch (e) {
      state.error = `打开配置目录失败：${message(e)}`;
      render();
    }
  });
  row.appendChild(open);
  return row;
}

/** 把选中的文件读成 base64（`data:` 头去掉，只把载荷交给 IPC）。 */
function readFileBase64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error('FileReader 读取失败'));
    reader.onload = () => {
      const text = String(reader.result || '');
      const comma = text.indexOf(',');
      resolve(comma >= 0 ? text.slice(comma + 1) : text);
    };
    reader.readAsDataURL(file);
  });
}

// ---------- 控件 ----------

/**
 * 一次提交**多个** `mods.*` 键（完美图标的逐项处置要同时改四个键，不能分四次提交）。
 *
 * 与 `commit` 同一套纪律：值先在 JS 侧过一遍 `normalizeModsEntry`（归一化的唯一入口，
 * 作用在「用户刚输入的值」上），回包再原样采纳（`adoptConfig`）；`needsReload` 时重建主窗口
 * ——逐项键本身经 `apply_to_page` 即时生效，但「第一次把某个键从默认值改成非默认」会改变
 * `injector::perfect_icon_enabled`（要不要嵌图标），那时必须重建。
 */
async function commitMods(patch, node) {
  if (node) node.classList.add('pending');
  try {
    const normalized = {};
    for (const [k, v] of Object.entries(patch)) normalized[k] = normalizeModsEntry(k, v);
    const res = await api.setConfig({ mods: normalized });
    adoptConfig(res.config);
    if (res.needsReload) {
      await api.reloadMain(null);
      await fetchPageSnapshot();
    }
    state.error = null;
  } catch (e) {
    state.error = `保存逐项设置失败：${message(e)}`;
  } finally {
    if (node && node.isConnected) node.classList.remove('pending');
  }
  render();
  renderStatus();
}

function appendHint(wrap, item) {
  if (item.hint) wrap.appendChild(el('p', { className: 'hint', text: item.hint }));
  return wrap;
}

function fieldEl(item) {
  const key = resolvePath(item.key);
  const id = fieldId(key);
  const value = readValue(state.config, key);
  const wrap = el('div', { className: 'field' });
  wrap.dataset.key = key;

  const label = el('label', { id: `${id}_label`, text: item.label });
  label.htmlFor = id;
  wrap.appendChild(label);

  switch (item.type) {
    case 'bool': {
      const input = el('input', { id });
      input.type = 'checkbox';
      input.checked = !!value;
      input.addEventListener('change', () => commit(key, input.checked, wrap));
      wrap.appendChild(input);
      break;
    }
    case 'color': {
      const box = el('div', { className: 'color-box' });
      const input = el('input', { id });
      input.type = 'color';
      input.value = value || DEFAULT_BRAND_COLOR;
      input.addEventListener('change', () => commit(key, input.value, wrap));
      // §8.4 的正面落实：把**归一化后实际生效**的值用文字显示出来，
      // 与取色器里用户刚点的原始颜色是两个不同的东西（明度被夹时会不同）。
      const shown = el('code', { id: `${id}_value`, className: 'normalized', text: value || DEFAULT_BRAND_COLOR });
      shown.title = '归一化后实际生效的值';
      const reset = button('重置', 'reset');
      reset.id = `${id}_reset`;
      reset.addEventListener('click', () => commit(key, DEFAULT_BRAND_COLOR, wrap));
      box.append(input, shown, reset);
      wrap.appendChild(box);
      break;
    }
    case 'radio': {
      const box = el('div', { className: 'radios', attrs: { role: 'radiogroup' } });
      for (const [v, text] of item.options) {
        const l = el('label', { className: 'radio' });
        const r = el('input', { id: `${id}_${v}` });
        r.type = 'radio';
        r.name = key;
        r.value = v;
        r.checked = value === v;
        r.addEventListener('change', () => commit(key, v, wrap));
        l.htmlFor = r.id;
        l.append(r, document.createTextNode(text));
        box.appendChild(l);
      }
      wrap.appendChild(box);
      break;
    }
    case 'select': {
      const input = el('select', { id });
      for (const [v, text] of item.options) {
        const o = el('option', { text });
        o.value = v;
        o.selected = v === value;
        input.appendChild(o);
      }
      input.addEventListener('change', () => commit(key, input.value, wrap));
      wrap.appendChild(input);
      break;
    }
    case 'number': {
      const input = el('input', { id });
      input.type = 'number';
      input.min = String(item.min);
      input.max = String(item.max);
      input.step = '1';
      input.value = String(value);
      input.addEventListener('change', () => commit(key, Number(input.value), wrap));
      wrap.appendChild(input);
      break;
    }
    case 'originList': {
      const box = el('div', { className: 'origins' });
      const list = Array.isArray(value) ? value : [];
      for (const origin of list) {
        const row = el('div', { className: 'origin-row' });
        row.appendChild(el('code', { className: 'origin', text: origin }));
        const del = button('删除', 'del');
        del.id = `${id}_del_${list.indexOf(origin)}`;
        del.addEventListener('click', () => commit(key, list.filter((x) => x !== origin), wrap));
        row.appendChild(del);
        box.appendChild(row);
      }
      if (!list.length) {
        box.appendChild(el('p', { className: 'origin-empty', text: '（空：未命中白名单的站点会先走约 1.5s 的探测）' }));
      }
      const addRow = el('div', { className: 'origin-row add' });
      const addInput = el('input', { id: `${id}_add` });
      addInput.type = 'text';
      addInput.placeholder = 'https://nas.example.com:8000';
      const add = button('添加');
      add.id = `${id}_addbtn`;
      // 可见的错误提示（不是只把边框标红）：`role=alert` 让读屏与 UIA 都能拿到它。
      const addErr = el('p', { id: `${id}_adderr`, className: 'origin-error', attrs: { role: 'alert' } });
      const doAdd = () => {
        // 只接受 http(s) 绝对地址（Review finding B）。旧写法 `new URL(v).origin` 对
        // `nas.example.com:8000` / `nas:8000` / `mailto:` / `data:` / `javascript:` 返回
        // **字符串 `"null"`**，而 `"null"` 是 truthy，`if (!origin)` 拦不住 → junk 落盘成
        // 一个永远匹配不上的白名单条目。
        const origin = parseHttpOrigin(addInput.value);
        if (!origin) {
          addInput.classList.add('error');
          addErr.textContent = '只接受 http:// 或 https:// 开头的完整地址，例如 http://nas.local:5666（未写入任何内容）';
          addInput.focus();
          return;
        }
        addInput.classList.remove('error');
        addErr.textContent = '';
        addInput.value = '';
        const next = list.concat([origin]).filter((o, i, a) => a.indexOf(o) === i);
        commit(key, next, wrap);
      };
      add.addEventListener('click', doAdd);
      addInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') doAdd(); });
      addRow.append(addInput, add);
      box.append(addRow, addErr);
      wrap.appendChild(box);
      break;
    }
    case 'code': {
      const input = el('textarea', { id });
      input.rows = 8;
      input.spellcheck = false;
      input.value = value == null ? '' : String(value);
      input.addEventListener('change', () => commit(key, input.value, wrap));
      wrap.appendChild(input);
      break;
    }
    case 'appList': {
      // 数据来源：主窗口页面经标题通道回报的应用项列表（`get_page_report` 信封的 `appItems`
      // 槽位 → `state.appItemsReport`）。**不读** `state.report`——那是状态条的证据槽位，
      // 里面永远是「最近一条非应用项上报」。没有可用数据时**不画空列表、不造项**，而是如实
      // 说明 + 指向「打开配置目录」手工编辑（这一项的数据不在宿主侧，宿主无法代用户补出来）。
      const box = el('div', { className: 'app-list' });
      const items = appItemsFromReport(state.appItemsReport);
      const mods = (state.config && state.config.mods) || {};

      if (items === null || items.length === 0) {
        box.appendChild(el('p', {
          id: `${id}_empty`,
          className: items === null ? 'hint warn' : 'hint',
          text: appListEmptyText(state.appItemsReport, items),
          attrs: { role: 'note' }
        }));
        box.appendChild(configDirRow(`${id}_dir`));
        wrap.appendChild(box);
        break;
      }

      // 逐项处置只在总开关打开时被上游采纳（cs:644-648），这一点必须写在界面上
      if (!mods.launchpadIconScaleEnabled) {
        box.appendChild(el('p', {
          id: `${id}_gated`,
          className: 'hint warn',
          text: '「完美图标」总开关当前是关闭的：下面这些逐项处置已经写进配置，但页面上不会生效（上游只在总开关打开时处理逐项键）。',
          attrs: { role: 'note' }
        }));
      }
      const options = iconChoiceOptions();
      items.forEach((item, index) => {
        const row = el('div', { className: 'app-item' });
        row.dataset.appKey = item.key;
        const title = typeof item.title === 'string' && item.title ? item.title : item.key;
        row.appendChild(el('span', { className: 'app-title', text: title }));
        row.appendChild(el('code', { className: 'app-key', text: item.key }));
        const select = el('select', { id: `${id}_sel_${index}` });
        select.dataset.appKey = item.key;
        const current = iconSelectionFor(mods, item.key);
        for (const [v, text] of options) {
          const o = el('option', { text });
          o.value = v;
          o.selected = v === current;
          select.appendChild(o);
        }
        select.addEventListener('change', () => {
          commitMods(applyIconSelection(state.config.mods, item.key, select.value), wrap);
        });
        row.appendChild(select);
        box.appendChild(row);
      });
      box.appendChild(el('p', {
        className: 'hint',
        text: `共 ${items.length} 个应用项（来自页面回报；主窗口重新加载后需要再回报一次）`
      }));
      box.appendChild(configDirRow(`${id}_dir`));
      wrap.appendChild(box);
      break;
    }
    case 'imageFile': {
      const box = el('div', { className: 'file-box' });
      const input = el('input', { id });
      input.type = 'file';
      input.accept = 'image/png,image/jpeg,image/webp';
      const stored = value == null ? '' : String(value);
      const status = el('p', { id: `${id}_status`, className: 'hint' });
      status.textContent = stored ? `当前：${stored}` : '当前：未设置';

      const applyStoredName = async (name) => {
        wrap.classList.add('pending');
        try {
          const res = await api.setConfig({ local: { loginWallpaperFileName: name } });
          adoptConfig(res.config);
          state.error = null;
          // 壁纸是**载荷内容**（`injector` 建窗时把它嵌进 binaryAssets），所以 Rust 侧会把
          // 它判成 needsReload；这里照既有流程重建主窗口，下一次加载就带上新图。
          if (res.needsReload) {
            await api.reloadMain(null);
            await fetchPageSnapshot();
          }
        } catch (e) {
          state.error = `保存「${key}」失败：${message(e)}`;
        } finally {
          if (wrap.isConnected) wrap.classList.remove('pending');
        }
        render();
        renderStatus();
      };

      input.addEventListener('change', async () => {
        const file = input.files && input.files[0];
        if (!file) return;
        if (file.size > MAX_WALLPAPER_BYTES) {
          state.error = `图片 ${file.size} 字节，超过 ${MAX_WALLPAPER_BYTES / 1024 / 1024} MiB 上限，未导入`;
          render();
          return;
        }
        wrap.classList.add('pending');
        let dataBase64;
        try {
          dataBase64 = await readFileBase64(file);
        } catch (e) {
          wrap.classList.remove('pending');
          state.error = `读取文件失败：${message(e)}`;
          render();
          return;
        }
        try {
          // 第一步：宿主校验 + 落盘到配置目录，返回它实际用的文件名
          const imported = await api.importWallpaper(file.name, dataBase64);
          state.error = null;
          wrap.classList.remove('pending');
          // 第二步：走既有的 set_config 路径写入配置（Rust 侧会判 needsReload 并重建主窗口）
          await applyStoredName(imported);
        } catch (e) {
          wrap.classList.remove('pending');
          state.error = `导入壁纸失败：${message(e)}`;
          render();
        }
      });

      const clear = button('清除', 'reset');
      clear.id = `${id}_clear`;
      clear.disabled = !stored;
      clear.addEventListener('click', () => { applyStoredName(null); });
      box.append(input, clear, status);
      wrap.appendChild(box);
      break;
    }
    default: {
      const input = el('input', { id });
      input.type = 'text';
      input.value = value == null ? '' : String(value);
      if (item.maxlength) input.maxLength = item.maxlength;
      input.addEventListener('change', () => commit(key, input.value, wrap));
      wrap.appendChild(input);
    }
  }

  return appendHint(wrap, item);
}

// ---------- 顶部状态条（Task 11 / spec §12.3） ----------

/**
 * 取一次主窗口的观测状态。
 *
 * 失败时**返回 null**（而不是抛）：状态条于是显示「主窗口状态未知」，这与「取到了，
 * 确实没检测到 WebUI」是两件事——不许把一次 IPC 失败说成「未检测到 fnOS WebUI」。
 */
async function fetchPageState() {
  try {
    const page = await api.getPageState();
    return page && typeof page === 'object' ? page : null;
  } catch (e) {
    return null;
  }
}

/**
 * 取一次页面上报（Task 13a）。
 *
 * 与 `fetchPageState` 同一套失败语义：取不到就 `null` → 状态条退回「注入脚本已注册」
 * 这一弱文案。**不许**把「IPC 失败」当成「已上报」或「未上报」之外的任何结论——
 * 这里没有第三个状态可说。
 */
async function fetchPageReport() {
  try {
    const report = await api.getPageReport();
    return report && typeof report === 'object' ? report : null;
  } catch (e) {
    return null;
  }
}

/**
 * 一次性刷新「主窗口观测 + 页面上报」两份快照。
 *
 * 两者都属于**同一时刻的主窗口**：上报是页面文档的属性，主窗口导航/重建后旧上报会被
 * Rust 侧作废（`commands.rs::get_page_report` 按文档身份判定），所以任何取 `page` 的地方
 * 都必须**同时**重取两个上报槽位，否则会出现「page 说是新页面、report 还是上一页的注入信号」
 * 这种自相矛盾的状态条。
 *
 * Task 13b：回包是信封（`{report, appItems}`），两个槽位分别进 `state.report` 与
 * `state.appItemsReport`——状态条只读前者，逐项 UI 只读后者。
 */
async function fetchPageSnapshot() {
  const [page, envelope] = await Promise.all([fetchPageState(), fetchPageReport()]);
  const slots = reportSlots(envelope);
  state.page = page;
  state.report = slots.report;
  state.appItemsReport = slots.appItems;
}

/** 判据是否还停在「加载失败」这一态（重试后的轮询用于决定何时停）。 */
function stillFailed(page) {
  return !!page && (page.loadFailed === true || page.loading === true);
}

/**
 * 「重试」：走既有 `reload_main`（Rust 侧销毁主窗口并按配置的 homeUrl + 新载荷重建）。
 *
 * 重建是异步的，紧接着取一次 `get_page_state` 大概率还是旧的失败态（窗口还没建回来），
 * 所以这里轮询一小会儿：状态条于是从「加载失败」走到真实结果，而不是卡在旧快照上。
 * `errorPage` 是内置错误页，**没有**任何 IPC 授权（capability 只给 settings 窗），
 * 所以重试入口只能在设置窗与托盘——这正是本函数存在的理由。
 */
async function retryMain() {
  state.error = null;
  renderStatus(true);
  try {
    await api.reloadMain(null);
  } catch (e) {
    state.error = `重试失败：${message(e)}`;
    await fetchPageSnapshot();
    render();
    renderStatus();
    return;
  }
  const deadline = Date.now() + 8000;
  do {
    await new Promise((r) => setTimeout(r, 600));
    await fetchPageSnapshot();
    renderStatus(true);
  } while (Date.now() < deadline && stillFailed(state.page));
  render();
  renderStatus();
}

/**
 * 「把当前页加入白名单」：走既有 `set_config` patch 路径写入 `mods.enabledOrigins`。
 *
 * origin 的合法性由 `status.js::addCurrentOriginToWhitelist` + Rust 的
 * `Config::normalize` 双保险（后者会把非 `scheme://host[:port]` 的东西挡在语义之外，
 * 且 `enabledOrigins` 只做 trim/小写/去重），这里拿到的 origin 来自
 * `get_page_state`（Rust 用 `config::origin_of` 解析出来的），不是页面上抓来的字符串。
 *
 * 写完**显式重建主窗口**：注入载荷是建窗时注册到 WebView2 的（`initialization_script`
 * 无法在活窗口上替换），而 `needsReload` 只覆盖 `injectEnabled`/`homeUrl`——不重建的话
 * 这次加入要等下一次导航才生效。重建后「下一次加载即注入」当场成立。
 */
async function whitelistCurrentOrigin() {
  const origin = state.page && state.page.origin;
  if (!origin) return;
  const next = addCurrentOriginToWhitelist(origin, (state.config.mods || {}).enabledOrigins);
  try {
    const res = await api.setConfig({ mods: { enabledOrigins: next } });
    adoptConfig(res.config);
    state.error = null;
    await api.reloadMain(null);
  } catch (e) {
    state.error = `加入白名单失败：${message(e)}`;
  }
  await fetchPageSnapshot();
  render();
  renderStatus();
}

/**
 * 把状态条重新画一遍（`status.js::statusFor` 是唯一的判据来源）。
 *
 * `busy` 只影响按钮可用性：重试期间的按钮置灰，避免连点堆出多次重建。
 */
function renderStatus(busy) {
  const model = statusFor(state.config, state.page, state.report);
  const el = statusBar(model.text, model.kind);
  if (!el) return;
  for (const action of model.actions) {
    if (action === 'retry') {
      const b = button('重试');
      b.id = 'statusRetry';
      b.disabled = !!busy;
      b.addEventListener('click', () => { retryMain(); });
      el.appendChild(b);
    } else if (action === 'whitelist') {
      const b = button('把当前页加入白名单');
      b.id = 'statusWhitelist';
      b.disabled = !!busy;
      b.addEventListener('click', () => { whitelistCurrentOrigin(); });
      el.appendChild(b);
    }
  }
}

// ---------- 关于页（spec §10：合规与品牌） ----------

/**
 * 关于页要展示的两个合规件路径（spec §10「分发必须保留许可全文」/ Ruling R54）。
 *
 * 优先用宿主解析出的**随包真实路径**（`meta.licensePath` / `meta.noticePath`：安装版落在
 * 安装目录的 `fnos-mods/`，开发版落在 `target/(debug|release)/fnos-mods/`，两处都由
 * `tauri-build` 的 `copy_resources` 保证文件确实在）。这两个字段是 T12 才加上的，
 * 老宿主（或只喂半个 meta 的单测）没有它们——那时才回落到源码树里的 vendored 位置，
 * 而不是把 `undefined` 画到界面上。
 */
export function compliancePaths(meta) {
  const m = meta || {};
  return {
    license: m.licensePath || `${VENDOR_DIR}/LICENSE`,
    notice: m.noticePath || `${VENDOR_DIR}/NOTICE`
  };
}

function metaRow(label, value, tag, className) {
  const row = el('div', { className: 'row' });
  row.appendChild(el('span', { className: 'row-label', text: label }));
  row.appendChild(el(tag || 'b', { className: className || 'row-value', text: value }));
  return row;
}

function renderAbout(pane) {
  const meta = state.config.meta || {};
  // Rust 侧字段是 `webview_version` + `#[serde(rename_all = "camelCase")]`，serde 只把
  // `_v` 变成 `V`，因此真实 JSON 键是 **`webviewVersion`**（不是 `webViewVersion`）。
  // 任务书/spec §8.3 的写法是 `webViewVersion`，这里两个都读：契约写法差异不该表现为
  // 「关于页永远显示未知」（本轮真机 UIA 断言正是靠这一点区分出 `undefined` 的）。
  const webviewVersion = meta.webViewVersion || meta.webviewVersion;
  const card = el('div', { className: 'card' });
  card.appendChild(metaRow('应用版本', meta.shellVersion || '未知'));
  card.appendChild(metaRow('mods commit', meta.modsCommit || '未知'));
  card.appendChild(metaRow('mods 版本', meta.modsVersion || '未知'));
  card.appendChild(metaRow('WebView2 版本', webviewVersion || '未知（未取到运行时版本）'));
  // spec §12.3 / §14：`corner-shape`（squircle 圆角）需要 Chromium/WebView2 139+；
  // 低于门槛时只说「会退化为普通圆角」，取不到版本时什么都不说（见 status.js 的注释）。
  const shapeHint = cornerShapeHint(webviewVersion);
  if (shapeHint) {
    card.appendChild(el('p', {
      id: 'cornerShapeHint',
      className: 'hint warn',
      text: shapeHint,
      attrs: { role: 'note' },
    }));
  }
  card.appendChild(metaRow('配置文件', meta.configPath || '未知', 'code', 'row-value path'));
  // spec §10 / R54：关于页必须指出**随包**许可全文与 NOTICE 的真实位置。
  // 这两行是 T12 新增的（此前只显示源码树路径，安装后在磁盘上并不存在）。
  const legalPaths = compliancePaths(meta);
  card.appendChild(metaRow('上游许可全文', legalPaths.license, 'code', 'row-value path'));
  card.appendChild(metaRow('来源与改动声明', legalPaths.notice, 'code', 'row-value path'));
  pane.appendChild(card);

  const actions = el('div', { className: 'card' });
  const openDir = button('打开配置目录');
  openDir.id = 'openDir';
  openDir.addEventListener('click', async () => {
    try {
      await api.openConfigDir();
      state.error = null;
    } catch (e) {
      state.error = `打开配置目录失败：${message(e)}`;
      render();
    }
  });
  const reset = button('恢复默认设置', 'danger');
  reset.id = 'resetAll';
  reset.addEventListener('click', async () => {
    if (!window.confirm('确定恢复全部默认设置？注入开关与地址也会回到默认值。')) return;
    try {
      adoptConfig(await api.resetConfig('all'));
      state.error = null;
    } catch (e) {
      state.error = `恢复默认失败：${message(e)}`;
    }
    render();
  });
  actions.append(openDir, reset);
  pane.appendChild(actions);

  // 许可与免责（spec §10）：非官方 + 非商业 + 上游出处 + vendored 许可全文位置。
  const legal = el('div', { className: 'card legal' });
  legal.appendChild(el('p', {
    className: 'legal-line',
    text: '本应用是第三方桌面壳，非飞牛（fnOS）官方产品，与飞牛官方无任何关联，也未获其授权或认可。'
  }));
  legal.appendChild(el('p', {
    className: 'legal-line',
    text: '随应用注入的界面修改资源（CSS/JS）来自上游开源项目 fnOS UI Mods，遵循其 Non-Commercial License 1.0，仅供非商业个人使用；本应用及这些资源均不得用于任何商业用途。上游资源按原样保留、未作修改，本壳仅做注入与包装性改动。'
  }));
  const linkLine = el('p', { className: 'legal-line' });
  linkLine.appendChild(document.createTextNode('上游项目：'));
  const link = el('a', { text: UPSTREAM_REPO });
  link.href = UPSTREAM_REPO;
  link.rel = 'noopener noreferrer';
  link.id = 'upstreamLink';
  // 外链**不在窗内导航**，交给 Rust `open_url` 用系统默认浏览器打开（Review finding D）。
  //
  // 为什么必须 preventDefault：不加的话 Chromium 会在**设置窗自身**里导航到 GitHub——
  // UI 被顶掉，而 `capabilities/default.json` 只授权本地来源，加载后的远程页面调不动
  // 任何命令，设置窗等于废掉。`target=_blank` 也救不了：wry 在 `new_window_handler`
  // 为 None 时直接 `args.SetHandled(true)`（wry-0.57.0/src/webview2/mod.rs 的
  // NewWindowRequested 分支，tauri 默认不注册），新窗口请求被静默吞掉 = 点了不跳转。
  // `href` 仍然保留：URL 可见、可复制、在无障碍树里仍是 Hyperlink。
  link.addEventListener('click', async (e) => {
    e.preventDefault();
    try {
      await api.openUrl(UPSTREAM_REPO);
      state.error = null;
    } catch (err) {
      state.error = `打开上游链接失败：${message(err)}`;
      render();
    }
  });
  linkLine.appendChild(link);
  legal.appendChild(linkLine);
  legal.appendChild(el('p', {
    className: 'legal-line dim',
    text: `上游许可全文：${legalPaths.license}；来源与改动声明：${legalPaths.notice}（后者含来源仓库、锁定 commit、各文件 SHA-256、本壳的包装性改动清单）。两者都随安装包分发；上方的链接会用系统默认浏览器打开上游仓库，若被系统策略拦截，可手动复制上面的路径。`
  }));
  pane.appendChild(legal);
}

// ---------- 渲染 ----------

function renderGroup(pane, group) {
  const card = el('div', { className: 'card' });
  for (const item of group.items) card.appendChild(fieldEl(item));
  pane.appendChild(card);
}

function render() {
  const nav = document.getElementById('nav');
  const pane = document.getElementById('pane');
  const scroll = pane.scrollTop;
  nav.textContent = '';
  pane.textContent = '';

  if (!state.config) {
    pane.appendChild(el('p', { className: 'fatal', text: state.error || '配置尚未加载' }));
    return;
  }

  const active = SCHEMA.find((g) => g.id === state.active) || SCHEMA[0];
  state.active = active.id;

  for (const group of SCHEMA) {
    const b = button(group.title, `nav-item${group.id === active.id ? ' on' : ''}`);
    b.id = `nav_${group.id}`;
    b.dataset.group = group.id;
    b.setAttribute('aria-current', group.id === active.id ? 'true' : 'false');
    b.addEventListener('click', () => { state.active = group.id; render(); });
    nav.appendChild(b);
  }

  pane.appendChild(el('h2', { className: 'pane-title', text: active.title }));
  if (state.error) pane.appendChild(el('p', { className: 'error-banner', text: state.error }));

  if (active.id === 'about') renderAbout(pane);
  else renderGroup(pane, active);

  pane.scrollTop = scroll;
}

/** 正在刷新（防止焦点事件与 boot / commit 的两次 getConfig 互相穿插）。 */
let refreshing = false;

/** 粗粒度比较：IPC 配置是纯 JSON（mods/local/shell/meta），序列化结果一致即视为没变。 */
function sameConfig(a, b) {
  return !!a && !!b && JSON.stringify(a) === JSON.stringify(b);
}

/**
 * 重取配置**与主窗口状态**并就地重渲染（Review finding C + Task 11）。
 *
 * 为什么需要：配置可以被本窗**之外**的入口改动（`set_config` 的带外调用、手工编辑
 * `config.json` 后重启、以及将来任何新增的写入方），而那些入口**不向设置窗发任何事件**；
 * 本窗只在 `boot()` 取过一次配置，于是带外改动后它会一直显示过期值，直到关掉重开。
 * 窗口重新获得焦点时刷新是最小实现：不引入事件总线、不改 Rust 侧。
 *
 * 注（T13b fix round 1 订正注释）：托盘曾有一个「注入 mods」勾选项，走
 * `commands::set_inject_enabled` + `tray::sync_menus`；托盘精简后勾选项与 `sync_menus`
 * 都已删除，开关只剩设置窗这一个入口（`schema.js` 的 `shell.injectEnabled`）。
 * 这条焦点刷新对**任何**带外改动仍然必要，所以保留。
 *
 * Task 11 的状态条复用同一个钩子，但它要的数据比配置多一份：主窗口**可以在不改任何
 * 配置**的情况下导航（用户在页面里点链接、错误页自动重试……），
 * 所以 `get_page_state` 必须每次刷新都取——不能挂在「配置变了」这个条件上。反过来，
 * 配置没变时依旧**不重渲染 #pane**（否则每次 alt-tab 回来都会重建 DOM、丢掉用户正在
 * 输入却尚未提交的文本）。
 *
 * Task 13a 再加一份：`get_page_report`（页面经标题通道回报的注入链信号）与 `get_page_state`
 * 是**同一时刻主窗口**的两个视图，必须在同一个快照里取（见 `fetchPageSnapshot`）。焦点刷新
 * 正好是「页面刚跑完注入链、用户切回设置窗」的时刻——状态条据实化主要靠这个钩子。
 *
 * Task 13b：**应用项列表**（`state.appItemsReport`）和配置一样是「主窗口那边的事，本窗只读
 * 快照」。用户的实际动线就是「切到主窗口打开启动台 → 切回设置窗」，所以焦点刷新必须让
 * **正在显示它的那一组**重画一次，否则界面会一直停在「尚未收到应用项列表」——而那句文案
 * 又写着「会自动刷新」，就成了一句假话。
 *
 * 重画的条件刻意收得很窄：**只有**当前分组是「完美图标」（该组只有总开关与应用项下拉，
 * 没有用户可能正在输入却尚未提交的文本框）**且**应用项槽位的内容真的变了。其余情形一律
 * 沿用「配置没变就不重渲染 #pane」的既有纪律（见上一段）。
 *
 * `render()` 会保留 `state.active`（当前分组）与 `pane.scrollTop`（滚动位置），
 * 所以刷新不会把用户弹回第一组。
 */
export async function refresh() {
  if (refreshing) return;
  refreshing = true;
  let next = null;
  let failure = null;
  try {
    next = await api.getConfig();
  } catch (e) {
    failure = e;
  }
  const beforeAppItems = JSON.stringify(state.appItemsReport);
  await fetchPageSnapshot();
  const appItemsChanged = JSON.stringify(state.appItemsReport) !== beforeAppItems;
  refreshing = false;
  if (failure) {
    state.error = `刷新配置失败：${message(failure)}`;
    renderStatus();
    render();
    return;
  }
  const changed = !sameConfig(state.config, next);
  if (changed) {
    adoptConfig(next);
    state.error = null;
  }
  renderStatus();
  if (changed || (appItemsChanged && state.active === 'perfectIcon')) render();
}

/** 取一次配置并渲染。导出以便单测；无 DOM 时模块加载不会自动执行（见文件末尾）。 */
export async function boot() {
  try {
    adoptConfig(await api.getConfig());
  } catch (e) {
    state.error = `读取配置失败：${message(e)}`;
  }
  await fetchPageSnapshot();
  render();
  renderStatus();
  window.addEventListener('focus', () => { refresh(); });
}

// 只有真实页面才自动启动：Node 单测 `import` 本模块时没有 DOM，直接 `boot()` 会抛错，
// 于是「导入 app.js 测内部逻辑」就变得不可行（Review 打磨项）。判据用 `#pane` 而不是
// 只看 `typeof document`：只有 DOM、没有页面骨架时同样不该启动。
const hasPane = typeof document !== 'undefined'
  && typeof document.getElementById === 'function'
  && !!document.getElementById('pane');
if (hasPane) boot();
