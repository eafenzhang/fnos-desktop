// 设置窗（T14c 修复轮 3 起）：**只承载上游 popup UI**，本壳不再画任何自有界面。
//
// 结构（`settings.html`）：
//
//   #upstreamHost 上游 UI：`popup.html` 装在一个 372×522 的 iframe 里（上游自己写死的尺寸）。
//                「自动隐藏 Dock」由 chrome-shim 以上游自己的行样式注入其中。
//                状态条、外壳开关（注入总开关 / nasUrl）与关于/合规页已按用户要求全部
//                删除——这两个 shell 键仅存于 `config.json`（Rust 侧照常归一化与生效），
//                合规声明由随包的 NOTICE / LICENSE 与 README 承担。
//
// 本文件剩下的职责只有三件：
//
// 1. **宿主桥**（`installHostBridge`）：iframe 里的 `chrome-shim.js` 不直接发 IPC，
//    它把命令 postMessage 过来，由本文件用既有 IPC 通路执行。**命令白名单**在这里，
//    iframe 拿不到任意命令；纯读的 `get_page_report` / `get_local_store` 也在表里。
// 2. **两个组合应答**：`page_check`（上游问「这一页是不是 fnOS WebUI」）与 `app_items`
//    （上游要「启动台应用项列表」）。后者的数据源是**页面上报的槽位**（Task 13b），
//    槽位是空的时候才请宿主去让页面再汇报一次，并在有界窗口内轮询（见 `answerAppItems`）。
// 3. **上游帧的挂载与自检**：配置快照就位后才建帧（时序硬约束，见 `mountUpstreamFrame`）；
//    帧内没渲染出上游节点时如实显示失败说明（绝不静默留白）。
//
// 纪律（与前几轮一致）：
// - **显示的值 = 生效的值**：`state.config` 一律原样采纳 `get_config` / `set_config` 的返回，
//   不在 JS 侧二次归一化（tests/settings.test.mjs 的 A 组锁着「app.js 不得引用 normalize.js」）。
// - 页面可控文本（`triggerReason`）进判读前先过 `clipPageText` 整形；一律 `textContent`，
//   绝不 `innerHTML`。
import * as api from './bridge.js';

/** 上游 popup 的页面（`ui/settings/popup.html`，与 vendored 副本逐字节相同 + 一行 shim 标签）。 */
export const UPSTREAM_PAGE = 'popup.html';

/** 宿主桥的 postMessage 协议（与 `chrome-shim.js` 逐字对应）。 */
const BRIDGE_FLAG = '__FNOS_SETTINGS_BRIDGE__';
const SNAPSHOT_KEY = '__FNOS_SETTINGS_SNAPSHOT__';
const REQ = 'FNOS_SHIM_REQUEST';
const REP = 'FNOS_SHIM_REPLY';

/** 应用项槽位为空时：请页面重汇报之后，最多等多久 / 多久看一眼（有界，不是无限等）。 */
export const APP_ITEMS_WAIT_MS = 6000;
export const APP_ITEMS_POLL_MS = 400;

/**
 * 界面状态。导出供单测与运行期证据口读取。
 *
 * `page` / `report` / `appItemsReport` 与 Task 11/13a/13b 同义：`page` 是 `get_page_state`
 * 的最近一次回包，`report` 与 `appItemsReport` 是 `get_page_report` 信封的两个槽位
 * （注入证据 / 应用项列表）。三者必须在同一个快照里取（见 `fetchPageSnapshot`）。
 */
export const state = {
  config: null, error: null, page: null, report: null, appItemsReport: null,
};

/**
 * 给 iframe 里的 `chrome-shim.js` **同步**读的快照（同一对象引用，就地更新）。
 *
 * 为什么需要：上游 popup.js 在脚本开头就同步调用 `chrome.runtime.getManifest().version`，
 * 而 IPC 是异步的。父 frame 在**创建 iframe 之前**先把 `get_config` 拿到手、写进这个对象，
 * 于是 shim 的同步读一定命中；此后每次配置变化都就地更新同一个对象。
 */
export const snapshot = { config: null };

/**
 * `get_page_report` 的回包信封 → 两个槽位（两个字段各自可能缺失/不是对象 → `null`）。
 *
 * 宿主**永远**返回对象（`{report, appItems}`），但设置窗可能正跑在一个更老的宿主上
 * （升级中途），所以这里对非对象输入一律回落成两个 `null`。
 */
export function reportSlots(envelope) {
  const e = envelope && typeof envelope === 'object' ? envelope : {};
  const pick = (v) => (v && typeof v === 'object' ? v : null);
  return { report: pick(e.report), appItems: pick(e.appItems) };
}

function message(e) {
  return String((e && e.message) || e || '未知错误');
}

/** 有 DOM 才动 DOM（`node --test` 会在无 DOM 的 Node 里 import 本模块）。 */
function domReady() {
  return typeof document !== 'undefined' && typeof document.getElementById === 'function';
}

// ---------- 配置 ----------

/**
 * 把 IPC 返回的配置收进 state **与** iframe 的快照。
 *
 * **`mods` 原样采纳，不做任何归一化**（Review finding A / §8.4）：Rust 归一化过的值
 * 再过一遍 JS 镜像就会漂移（`#cec1b2` → Rust `#c4b4a2` → JS 再夹一次 `#c4b4a1`），
 * 而页面按 Rust 那份生效。T14b fix round 1 起镜像函数已删除、设置窗没有任何 mods
 * 归一化。回归测试见 `tests/settings.test.mjs` 的 A 组（含「不得引用 normalize.js」的
 * 源码级断言）。
 */
export function adoptConfig(raw) {
  if (!raw || typeof raw !== 'object') return;
  const mods = raw.mods && typeof raw.mods === 'object' ? raw.mods : {};
  state.config = { ...raw, mods };
  snapshot.config = state.config;
}

// ---------- 完美图标：应用项列表（数据来自页面上报的槽位） ----------

/** 应用项上报被接受的两个 type（上游 cs:2853-2855 的同一个分支）。 */
const APP_ITEM_REPORT_TYPES = ['FNOS_GET_LAUNCHPAD_APP_ITEMS', 'FNOS_GET_LAUNCHPAD_APP_TITLES'];

/**
 * 这条上报是不是 shim 在「列表大到装不下」时回的**诊断**（T13b fix round 1 / Minor 5）。
 *
 * 形状由 `src-tauri/inject/shim.js::noteAppItemsSend` 定义：`{items:[],titles:[],tooLarge:true,
 * itemCount:N}`。它走的是**同一条**应用项通道（宿主的分流判据只看 type），所以宿主侧不需要
 * 任何新槽位，设置窗只是多认一个字段。`tooLarge === true` 用严格比较：页面可控的 `"true"`
 * 字符串不算。
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
 * 只认**形状**：`payload.items` 必须是数组，元素必须有非空字符串 `key`。返回 `[]` 是
 * 「上游确实回报了 0 个应用」——与 `null`（没有可用数据）在下游是两种不同的事实。
 * 页面可控文本（`title` / `key`）只经 `textContent` 渲染，绝不做 HTML 拼接。
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

/**
 * 应用项槽位 → 给上游 `chrome.tabs.sendMessage` 的应答（Task 14b 的**纯判据**，可单测）。
 *
 * 三种结果，各自对应一个**如实**的下一步：
 * - 槽位里是「列表过大」的诊断 → `{items:[], titles:[], tooLarge:true, itemCount}`：
 *   再问多少次都一样（装不下就是装不下），所以直接回空列表，让上游走它的兜底文案，
 *   而**为什么**由 `chrome-shim.js` 的可见说明框说清楚；
 * - 槽位里有非空的可用列表 → `{items, titles, itemCount}`（含 `iconSrc` 已被页面侧剥掉，
 *   上游对缺失的 iconSrc 有自己的兜底：`if (iconSrc)` + `img.onerror` 隐藏）；
 * - 其余（没上报 / 形状不对 / 空列表）→ `null` =「还没有可用数据」，调用方据此去
 *   请页面重汇报一次并在有界窗口内轮询。
 */
export function appItemsAnswer(report) {
  if (isTooLargeReport(report)) {
    const n = Number(report.payload.itemCount);
    return {
      items: [], titles: [],
      itemCount: Number.isFinite(n) && n > 0 ? n : 0,
      tooLarge: true,
    };
  }
  const items = appItemsFromReport(report);
  if (!items || items.length === 0) return null;
  const payload = report.payload && typeof report.payload === 'object' ? report.payload : {};
  const titles = Array.isArray(payload.titles)
    ? payload.titles.filter((t) => typeof t === 'string')
    : [];
  return { items, titles, itemCount: items.length };
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * 上游要应用项列表时的完整应答（纯判据 + 有界的「再问一次」）。
 *
 * 为什么不能只读槽位：那份列表只能由**页面侧**主动向上游请求（T13b 的
 * `inject/shim.js::requestAppItems`，页面加载时跑一轮有限重试），设置窗开得晚就错过了。
 * 所以槽位为空时请宿主 [`api.requestAppItems`] 去让页面**当场**再问一次，然后最多等
 * [`APP_ITEMS_WAIT_MS`]（每 [`APP_ITEMS_POLL_MS`] 取一次快照）。超时就如实返回一个空应答
 * ——上游会走它自己的兜底文案（「先打开启动台再试」），而不是我们编一份假列表。
 */
export async function answerAppItems() {
  await fetchPageSnapshot();
  const first = appItemsAnswer(state.appItemsReport);
  if (first) return first;

  try {
    await api.requestAppItems();
  } catch (e) {
    // 主窗口没开 / 页面不在了：如实回空（上游的兜底文案说的是同一件事）
    return { items: [], titles: [], itemCount: 0, pending: true, reason: message(e) };
  }
  const deadline = Date.now() + APP_ITEMS_WAIT_MS;
  do {
    await sleep(APP_ITEMS_POLL_MS);
    await fetchPageSnapshot();
    const next = appItemsAnswer(state.appItemsReport);
    if (next) return next;
  } while (Date.now() < deadline);
  return { items: [], titles: [], itemCount: 0, pending: true };
}

/**
 * 页面可控文本（`triggerReason`）进判读前的唯一一道整形：控制字符（含换行/制表）与行
 * 分隔符换成空格、连续空白折叠、按**码点**截断到 [`MAX_TRIGGER_REASON_CHARS`]。
 * 原属 status.js（T14c 修复轮 3 随状态条 UI 退役迁入）——`pageCheckAnswer` 判读上报时
 * 仍然需要它：`triggerReason` 是页面可写的，长度与控制字符必须先收口。
 */
export const MAX_TRIGGER_REASON_CHARS = 60;

function clipPageText(s) {
  const cleaned = String(s)
    .replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  const cps = Array.from(cleaned);
  if (cps.length <= MAX_TRIGGER_REASON_CHARS) return cleaned;
  return cps.slice(0, MAX_TRIGGER_REASON_CHARS).join('') + '…';
}

/**
 * 最近一次页面上报（`get_page_report` 回包）的判读结果：`{ injected, reason }`。
 *
 * **唯一能认定「注入链已触发」的证据**：上游 `content-script.js` 的
 * `notifyInjectionTriggered()` 在 `startInject()` 末尾 `chrome.runtime.sendMessage(...)`；
 * 本壳的 shim（`inject/shim.js` 的 `sendMessage`）把这条消息**原文**放进信封的 `payload`，
 * 宿主校验 type 白名单后存内存。它比「脚本已注册」强一档：说明**上游自己的注入链**
 * 确实跑到了最后一步。
 *
 * 不升级的形状（全部按「没有上报」处理，宁可退回弱判定）：
 * - 不是对象 / `null`（没上报、被拒、换页面后作废）；
 * - `type` 不是 `FNOS_INJECTION_TRIGGERED`（例如应用项列表应答）；
 * - `dir !== 'out'`：`'response'` 是**别人问它、它作答**，不代表这次加载触发过注入；
 * - `payload` 缺失 → 仍然算注入已触发（triggerReason 只是附加信息，缺了不影响结论）。
 *
 * `reason` 会过一遍 [`clipPageText`]（页面公开可写的文本，进判读/日志前先整形）。
 */
function reportVerdict(report) {
  if (!report || typeof report !== 'object') return { injected: false, reason: null };
  if (report.type !== 'FNOS_INJECTION_TRIGGERED') return { injected: false, reason: null };
  if (report.dir !== 'out') return { injected: false, reason: null };
  const payload = report.payload && typeof report.payload === 'object' ? report.payload : null;
  const raw = payload && typeof payload.triggerReason === 'string' ? clipPageText(payload.triggerReason) : '';
  return { injected: true, reason: raw || null };
}

/**
 * 上游问「这一页是不是 fnOS WebUI」。
 *
 * 判据只用宿主观测：**识别**（白名单 / *.fnos.net / nasUrl 的 origin，`config.rs::is_recognized`）
 * **或**页面自己的注入链回报（`reportVerdict`，本文件；原 status.js 已随状态条退役迁入）。
 * 错误页 / 加载失败一律 false：上游据此决定要不要 `FNOS_APPLY`，对着一张错误页说 true 只会
 * 让它白干活。
 */
export function pageCheckAnswer(page, report) {
  if (!page || typeof page !== 'object') return { isFnOSWebUi: false };
  if (page.errorPage === true || page.loadFailed === true) return { isFnOSWebUi: false };
  return { isFnOSWebUi: page.recognized === true || reportVerdict(report).injected };
}

/**
 * 上游 `FNOS_APPLY` 的转发：消息里的派生值 → `set_config` 的 patch。
 *
 * 上游在发这条消息之前已经逐键 `storage.sync.set` 过了，所以**配置就是权威**；这条消息里
 * 的价值是「不必等下一次导航」：把它落到 `set_config`，宿主的 `apply_to_page` 会当场
 * `eval` 一次 `__FNOS_APPLY_CONFIG__`，活页面立刻收到。字段映射与 `mods` 段一一对应。
 *
 * 只认**列出了的键**（白名单），其余一概不进 patch：上游 UI 里的 `refreshFontAsset` /
 * `refreshCustomCode` / `refreshLoginWallpaper` 是「资源要不要重算」的提示，本壳的对应资源
 * 都由宿主按配置派生（字体不导入、代码/壁纸走载荷重建），没有需要照着做的动作。
 */
export function applyPatchFromPopup(msg) {
  const m = msg && typeof msg === 'object' ? msg : {};
  const mods = {};
  const local = {};
  const modsKeys = [
    'basePresetEnabled', 'windowAnimationBlurEnabled', 'titlebarStyle', 'launchpadStyle',
    'desktopIconLayoutEnabled', 'desktopIconLayoutMode', 'desktopIconPerColumn',
    'launchpadIconScaleEnabled', 'launchpadIconScaleSelectedKeys',
    'launchpadIconMaskOnlyKeys', 'launchpadIconRedrawKeys', 'launchpadIconRedrawMap',
    'brandColor', 'lockscreenDefaultUsername', 'customCodeEnabled',
  ];
  for (const key of modsKeys) if (key in m) mods[key] = m[key];

  const font = m.fontSettings && typeof m.fontSettings === 'object' ? m.fontSettings : null;
  if (font) {
    if ('enabled' in font) mods.fontOverrideEnabled = !!font.enabled;
    if ('family' in font) mods.fontFamily = String(font.family == null ? '' : font.family);
    if ('monospaceFamily' in font) {
      mods.fontMonospaceFamily = String(font.monospaceFamily == null ? '' : font.monospaceFamily);
    }
    if ('weight' in font) mods.fontWeight = String(font.weight == null ? '' : font.weight);
    if ('featureSettings' in font) {
      mods.fontFeatureSettings = String(font.featureSettings == null ? '' : font.featureSettings);
    }
    if ('faceName' in font) mods.fontFaceName = String(font.faceName == null ? '' : font.faceName);
    if ('url' in font) mods.fontUrl = String(font.url == null ? '' : font.url);
  }

  const code = m.customCodeSettings && typeof m.customCodeSettings === 'object' ? m.customCodeSettings : null;
  if (code) {
    if ('css' in code) local.customCssCode = String(code.css == null ? '' : code.css);
    if ('js' in code) local.customJsCode = String(code.js == null ? '' : code.js);
  }

  const patch = {};
  if (Object.keys(mods).length) patch.mods = mods;
  if (Object.keys(local).length) patch.local = local;
  return patch;
}

// ---------- 主窗口观测 ----------

async function fetchPageState() {
  try {
    const page = await api.getPageState();
    return page && typeof page === 'object' ? page : null;
  } catch (e) {
    return null; // 「取不到」与「取到了、确实没识别」是两件事
  }
}

async function fetchPageReport() {
  try {
    const report = await api.getPageReport();
    return report && typeof report === 'object' ? report : null;
  } catch (e) {
    return null;
  }
}

/**
 * 一次性刷新「主窗口观测 + 两个上报槽位」。
 *
 * 三者属于**同一时刻的主窗口**：上报是页面文档的属性，主窗口导航/重建后旧上报会被 Rust
 * 侧作废（`commands.rs::get_page_report` 按文档身份判定），所以取 `page` 的地方必须同时
 * 重取两个槽位，否则会出现「page 说是新页面、report 还是上一页的注入信号」这种自相矛盾。
 */
async function fetchPageSnapshot() {
  const [page, envelope] = await Promise.all([fetchPageState(), fetchPageReport()]);
  const slots = reportSlots(envelope);
  state.page = page;
  state.report = slots.report;
  state.appItemsReport = slots.appItems;
}

// ---------- 宿主桥（iframe 里的 chrome-shim → 本文件 → IPC） ----------

/**
 * iframe 允许调用的**命令白名单**。表外的一切命令名都会被拒绝（回 `ok:false` 的错误）——
 * iframe 是我们自己的 shim，但「它能调什么」不该由它自己决定。
 */
const HOST_COMMANDS = {
  get_config: () => api.getConfig(),
  set_config: (a) => api.setConfig(a.patch),
  reload_main: (a) => api.reloadMain(a && a.url ? a.url : null),
  get_page_state: () => api.getPageState(),
  get_page_report: () => api.getPageReport(),
  get_local_store: () => api.getLocalStore(),
  set_local_store: (a) => api.setLocalStore(a.patch),
  import_wallpaper: (a) => api.importWallpaper(a.name, a.dataBase64),
  open_url: (a) => api.openUrl(a.url),
  request_app_items: () => api.requestAppItems(),
  // 组合命令：不是 tauri 命令，只在本窗内部成立（见各自的实现）
  page_check: async () => {
    await fetchPageSnapshot();
    return pageCheckAnswer(state.page, state.report);
  },
  app_items: () => answerAppItems(),
  apply: async (a) => {
    const patch = applyPatchFromPopup(a && a.message);
    if (!Object.keys(patch).length) return {};
    const res = await api.setConfig(patch);
    adoptConfig(res.config);
    if (res.needsReload) {
      await api.reloadMain(null);
      await fetchPageSnapshot();
    }
    return {};
  },
};

/** 安装宿主桥：把 iframe 的命令请求派发到白名单，并把结果回传。 */
export function installHostBridge() {
  if (!domReady() || typeof window.addEventListener !== 'function') return;
  window[BRIDGE_FLAG] = true;
  window[SNAPSHOT_KEY] = snapshot;
  window.addEventListener('message', (event) => {
    const frame = document.getElementById('upstreamFrame');
    if (!frame || event.source !== frame.contentWindow) return;
    const data = event.data;
    if (!data || typeof data !== 'object' || data[REQ] !== true) return;
    const reply = (ok, value, error) => {
      try {
        frame.contentWindow.postMessage({ [REP]: true, id: data.id, ok, value, error }, '*');
      } catch (e) { /* 帧已经没了：这次调用按失败处理即可 */ }
    };
    const handler = Object.prototype.hasOwnProperty.call(HOST_COMMANDS, data.cmd)
      ? HOST_COMMANDS[data.cmd]
      : null;
    if (!handler) {
      reply(false, undefined, `设置窗没有映射这条命令：${String(data.cmd)}`);
      return;
    }
    Promise.resolve()
      .then(() => handler(data.args || {}))
      .then((value) => {
        reply(true, value === undefined ? null : value);
      })
      .catch((e) => reply(false, undefined, message(e)));
  });
}

// ---------- 上游帧 ----------

/**
 * 挂载上游 popup（**只在配置快照就位之后**）。
 *
 * 时序是硬约束：上游 popup.js 第一行就同步读 `chrome.runtime.getManifest().version`，
 * 而 shim 的 `getManifest` 读的是父 frame 的快照。先建帧再取配置会让版本显示落在兜底值上。
 *
 * **帧内的 `data-state` 不在挂载时就写 ready**（fix round 1 / Minor 2）：只有帧内真的出现
 * 上游自己的节点（`#siteToggle` + `code.version`）才算渲染成功。旧实现在这里立刻写
 * `ready`，而 CSS 又把占位说明在 ready 态隐藏，于是「上游界面没渲染出来」的兜底
 * （`applyFrameVerdict`）永远不可能被用户看见——正是「绝不静默留白」要防的那种情形。
 */
export function mountUpstreamFrame() {
  if (!domReady()) return null;
  const host = document.getElementById('upstreamHost');
  if (!host || host.dataset.state === 'ready' || host.dataset.state === 'mounted') return null;
  const frame = document.createElement('iframe');
  frame.id = 'upstreamFrame';
  frame.title = '设置界面（fnOS Desktop）';
  frame.setAttribute('src', UPSTREAM_PAGE);
  frame.addEventListener('load', () => { applyFrameVerdict(true); });
  host.appendChild(frame);
  // `mounted`：帧已经挂上，占位说明先让位给帧本身；真正的判定交给 applyFrameVerdict。
  host.dataset.state = 'mounted';
  applyFrameVerdict();
  scheduleFrameVerdict();
  return frame;
}

/** 帧内自检的有界重试：上游 popup.js 是异步启动的，给它几拍再判死。 */
export const FRAME_VERDICT_TRIES = 8;
export const FRAME_VERDICT_INTERVAL_MS = 400;
let frameVerdictTimer = null;

/**
 * 有界轮询帧内自检（最多 [`FRAME_VERDICT_TRIES`] × [`FRAME_VERDICT_INTERVAL_MS`]）。
 *
 * 只为了「不冤判」：上游 UI 的静态节点在文档就绪时就存在，但帧的 `load` 事件与
 * `chrome.storage.sync.get` 的回包之间有真实的空档；轮询窗口结束仍未就绪就如实
 * 显示失败说明（不无限等，也不假装 ready）。
 */
export function scheduleFrameVerdict() {
  if (frameVerdictTimer !== null) return;
  let tries = 0;
  const tick = () => {
    frameVerdictTimer = null;
    tries += 1;
    if (applyFrameVerdict(false)) return;
    if (tries >= FRAME_VERDICT_TRIES) {
      // 有界窗口用尽仍未就绪 → 如实显示失败说明（**不再**停在会隐藏说明的态上）。
      applyFrameVerdict(true);
      return;
    }
    frameVerdictTimer = setTimeout(tick, FRAME_VERDICT_INTERVAL_MS);
  };
  frameVerdictTimer = setTimeout(tick, FRAME_VERDICT_INTERVAL_MS);
}

/**
 * 帧内自检：上游 UI 到底渲染出来没有（运行期证据也读它）。
 *
 * 判据只用**上游自己的** id/class（`#siteToggle` 是站点开关、`code.version` 是版本行），
 * 所以「本壳的旧分组 UI 还在不在」这件事在这里是可判定的：旧 UI 没有这些元素。
 * 读不到（跨源/还没就绪）时如实说明，不猜。
 */
export function probeUpstreamFrame() {
  if (!domReady()) return { mounted: false, reason: '无 DOM' };
  const frame = document.getElementById('upstreamFrame');
  if (!frame) return { mounted: false, reason: '还没挂载上游界面' };
  let doc = null;
  let win = null;
  try {
    doc = frame.contentDocument;
    win = frame.contentWindow;
  } catch (e) {
    return { mounted: true, sameOrigin: false, reason: '跨源读不到帧内文档' };
  }
  if (!doc || !doc.getElementById) return { mounted: true, sameOrigin: true, ready: false, reason: '帧内文档还没就绪' };
  const version = doc.querySelector('code.version');
  const siteToggle = doc.getElementById('siteToggle');
  const appList = doc.getElementById('launchpadAppList');
  const notice = doc.getElementById('fnosShellNotice');
  const chromeObj = win && win.chrome;
  const upstreamGroups = doc.querySelectorAll('#nav, #pane, .nav-item');
  const ready = !!siteToggle && !!version;
  return {
    mounted: true,
    sameOrigin: true,
    ready,
    // 未就绪时给出**可读的原因**（旧实现这里没有 reason，占位说明只能显示「未知原因」）。
    reason: ready ? null : frameFailureReason(doc, chromeObj),
    version: version ? String(version.textContent).trim() : null,
    siteToggle: !!siteToggle,
    appListEmptyText: appList ? String(appList.textContent).trim().slice(0, 80) : null,
    appListItemCount: appList ? appList.querySelectorAll('.launchpad-app-item').length : 0,
    chromeReady: !!(chromeObj && chromeObj.storage && chromeObj.storage.sync && chromeObj.runtime),
    retiredGroupUiPresent: upstreamGroups.length > 0,
    notice: notice ? String(notice.textContent).replace(/\s+/g, ' ').slice(0, 500) : null,
  };
}

/** 帧内为什么没渲染出来：按**能观测到的**差别给一句可读的原因（不猜、不编）。 */
function frameFailureReason(doc, chromeObj) {
  const chromeReady = !!(chromeObj && chromeObj.storage && chromeObj.storage.sync && chromeObj.runtime);
  if (!chromeReady) {
    return '帧内的 chrome.* 兼容层没有就位（chrome-shim.js 没跑起来，popup.js 会在启动时抛错）';
  }
  const body = doc && doc.body ? String(doc.body.textContent || '').trim() : '';
  if (!body) return '帧内文档是空的（popup.html 没有加载出来，或被 CSP/协议拒绝）';
  return '上游界面的关键节点不存在（#siteToggle / code.version 都没有出现，popup.html 或 popup.js 没有跑起来）';
}

/**
 * 把帧内自检的结论写进占位说明，并**只在真的就绪时**把 host 标成 ready。
 *
 * 返回是否就绪。未就绪时分两种态：还在有界重试窗口内 → `mounted`（不显示说明，避免抖动），
 * 重试用尽或帧的 `load` 已经把结论带来 → `failed`（说明可见）。CSS 只在 ready 态隐藏说明，
 * 所以 `failed` 的说明**一定看得见**（旧实现永远停在 ready，说明被 CSS 隐藏）。
 */
function applyFrameVerdict(final = false) {
  const host = domReady() ? document.getElementById('upstreamHost') : null;
  if (!host) return false;
  const verdict = probeUpstreamFrame();
  if (verdict.ready) {
    host.dataset.state = 'ready';
    return true;
  }
  if (final) {
    host.dataset.state = 'failed';
    const note = document.getElementById('upstreamNote');
    if (note) {
      note.textContent = `上游设置界面没能渲染：${verdict.reason || '未知原因'}`
        + '（本壳不会假装它渲染成功；这句话只有在帧内确实没有上游节点时才会出现。）';
    }
  }
  return false;
}

// ---------- 生命周期 ----------

let refreshing = false;

function sameConfig(a, b) {
  return !!a && !!b && JSON.stringify(a) === JSON.stringify(b);
}

/**
 * 重取配置 + 主窗口快照并就地更新 state。
 *
 * 为什么需要：配置可以被本窗**之外**的入口改动（手工编辑 `config.json`、`set_config` 的
 * 带外调用），而那些入口不向本窗发任何事件；主窗口更是**可以不改配置**就导航（用户点链接、
 * 错误页自动重试）。窗口重新获得焦点时刷新是最小实现。
 *
 * **不重挂上游帧**：上游 UI 有自己的输入状态与滚动位置，重新加载 iframe 会把它清掉；
 * 上游自己通过 `storage.sync.set` 写配置，所以它的界面与配置天然同步。
 */
export async function refresh() {
  if (refreshing) return;
  refreshing = true;
  try {
    const next = await api.getConfig();
    if (!sameConfig(state.config, next)) adoptConfig(next);
  } catch (e) {
    // 取不到就不动现有快照：本窗已经没有自有视图需要因此降级
  }
  await fetchPageSnapshot();
  refreshing = false;
}

/**
 * 启动：取配置 → 写快照 → 挂宿主桥 → **然后**才建上游帧。
 *
 * 次序是硬约束（见 `mountUpstreamFrame` 的注释）：上游在脚本开头同步读版本号。
 * 导出以便单测；无 DOM 时模块加载不会自动执行（见文件末尾）。
 */
export async function boot() {
  try {
    adoptConfig(await api.getConfig());
  } catch (e) {
    // 读不到配置也要把桥和帧装起来：chrome-shim 对空快照有明确的兜底与失败说明
  }
  await fetchPageSnapshot();
  installHostBridge();
  mountUpstreamFrame();
  // 运行期证据口（UIA / WebView2 CDP 都读它）：帧内自检 + 当前快照，用于证明
  // 「渲染出来的是上游 UI，不是本壳退休掉的分组 UI」。
  window.__FNOS_UPSTREAM_PROBE__ = () => ({
    ...probeUpstreamFrame(),
    pageCheck: pageCheckAnswer(state.page, state.report),
    configPath: (state.config && state.config.meta && state.config.meta.configPath) || null,
    modsVersion: (state.config && state.config.meta && state.config.meta.modsVersion) || null,
  });
  window.addEventListener('focus', () => { refresh(); });
}

// 只有真实页面才自动启动：Node 单测 `import` 本模块时没有 DOM（`#upstreamHost`），
// 直接 `boot()` 会抛错，于是「导入 app.js 测内部逻辑」就变得不可行。
const hasShell = domReady() && !!document.getElementById('upstreamHost');
if (hasShell) boot();
