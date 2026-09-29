// 设置窗（Task 14b 起）：**托管上游 popup UI** + 本壳自己的状态条 / 外壳开关 / 关于页。
//
// 结构（`settings.html`）：
//
//   #status      顶部状态条（Task 11 / spec §12.3），判据全在 `status.js`（本文件只负责画）
//   #upstreamHost 上游 UI：`popup.html` 装在一个 372×522 的 iframe 里（上游自己写死的尺寸）
//   #shellFields  本壳的开关（`shell.injectEnabled` / `shell.nasUrl`）——上游 UI 里没有这两项
//   #about        关于/合规（spec §10）
//
// 本文件**不再**有任何 schema 驱动的分组渲染（Task 14b 退休）：面板、控件、逐项完美图标、
// 壁纸与自定义代码的界面全部由上游 popup 提供。因此这里只剩下三件事：
//
// 1. **宿主桥**（`installHostBridge`）：iframe 里的 `chrome-shim.js` 不直接发 IPC，
//    它把命令 postMessage 过来，由本文件用既有 IPC 通路执行。**命令白名单**在这里，
//    iframe 拿不到任意命令；纯读的 `get_page_report` / `get_local_store` 也在表里。
// 2. **两个组合应答**：`page_check`（上游问「这一页是不是 fnOS WebUI」）与 `app_items`
//    （上游要「启动台应用项列表」）。后者的数据源是**页面上报的槽位**（Task 13b），
//    槽位是空的时候才请宿主去让页面再汇报一次，并在有界窗口内轮询（见 `answerAppItems`）。
// 3. 本壳自己的三块 UI：状态条、外壳开关、关于页。
//
// 纪律（与前几轮一致）：
// - **显示的值 = 生效的值**：`state.config` 一律原样采纳 `get_config` / `set_config` 的返回，
//   不在 JS 侧二次归一化（`normalize.js` 仍是「用户刚输入的值」在提交前的唯一入口，
//   而这里唯一的手工输入是 NAS 地址，提交时由 Rust 的 `Config::normalize` 收口）。
// - **显示与否都要有据**：状态条只读真实回包；应用项列表只读 Rust 分好槽的上报。
// - 页面可控文本一律 `textContent`，绝不 `innerHTML`。
import { addCurrentOriginToWhitelist, cornerShapeHint, statusBar, statusFor, reportVerdict } from './status.js';
import * as api from './bridge.js';

/** 上游项目主页 + vendored 资源位置（spec §10：关于页必须给出这两件事）。 */
export const UPSTREAM_REPO = 'https://github.com/aurysian-yan/fnOS_UI_Mods';
export const VENDOR_DIR = 'src-tauri/assets/fnos-mods';

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
 * 界面状态。导出供单测与状态条读取。
 *
 * `page` / `report` / `appItemsReport` 与 Task 11/13a/13b 同义：`page` 是 `get_page_state`
 * 的最近一次回包，`report` 与 `appItemsReport` 是 `get_page_report` 信封的两个槽位
 * （状态条证据 / 应用项列表）。三者必须在同一个快照里取（见 `fetchPageSnapshot`）。
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

/** 有 DOM 才动 DOM（`node --test` 会在无 DOM 的 Node 里 import 本模块）。 */
function domReady() {
  return typeof document !== 'undefined' && typeof document.getElementById === 'function';
}

// ---------- 配置 ----------

/**
 * 把 IPC 返回的配置收进 state **与** iframe 的快照。
 *
 * **`mods` 原样采纳，不做任何归一化**（Review finding A / §8.4）：`clampLightness` 不是不动点
 * （`#cec1b2` → Rust `#c4b4a2` → JS 再夹一次 `#c4b4a1`），而页面按 Rust 那份生效。
 * 归一化只剩一个合法入口：用户刚输入的值在提交 patch 之前（本文件只剩 NAS 地址一处，
 * 且它由 Rust 的 `Config::normalize` 收口）。回归测试见 `tests/settings.test.mjs`。
 */
export function adoptConfig(raw) {
  if (!raw || typeof raw !== 'object') return;
  const mods = raw.mods && typeof raw.mods === 'object' ? raw.mods : {};
  state.config = { ...raw, mods };
  snapshot.config = state.config;
}

/** 提交一个 `shell.*` 键；需要时跟随 `reload_main`（与 T13b 的设置窗行为一致）。 */
async function commitShell(path, value) {
  try {
    const patch = path === 'injectEnabled'
      ? { shell: { injectEnabled: !!value } }
      : { shell: { nasUrl: String(value == null ? '' : value) } };
    const res = await api.setConfig(patch);
    adoptConfig(res.config);
    state.error = null;
    if (res.needsReload) {
      await api.reloadMain(null);
      await fetchPageSnapshot();
    }
  } catch (e) {
    state.error = `保存「${path}」失败：${message(e)}`;
  }
  renderShell();
  renderStatus();
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
 * 上游问「这一页是不是 fnOS WebUI」。
 *
 * 判据只用宿主观测：**识别**（白名单 / *.fnos.net / nasUrl 的 origin，`config.rs::is_recognized`）
 * **或**页面自己的注入链回报（`status.js::reportVerdict`，与状态条的强态同源）。
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
    return null; // 「取不到」与「取到了、确实没识别」是两件事（见 status.js）
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
        // 写完配置/重建主窗口之后：状态条与外壳区必须跟着换，否则会停在旧判定上
        if (data.cmd === 'set_config' || data.cmd === 'reload_main' || data.cmd === 'apply') {
          renderStatus();
          renderShell();
        }
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
 */
export function mountUpstreamFrame() {
  if (!domReady()) return null;
  const host = document.getElementById('upstreamHost');
  if (!host || host.dataset.state === 'ready') return null;
  const frame = document.createElement('iframe');
  frame.id = 'upstreamFrame';
  frame.title = '上游设置界面（fnOS UI Mods popup）';
  frame.setAttribute('src', UPSTREAM_PAGE);
  frame.addEventListener('load', () => { reportFrameVerdict(); });
  host.appendChild(frame);
  host.dataset.state = 'ready';
  return frame;
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
  return {
    mounted: true,
    sameOrigin: true,
    ready: !!siteToggle && !!version,
    version: version ? String(version.textContent).trim() : null,
    siteToggle: !!siteToggle,
    appListEmptyText: appList ? String(appList.textContent).trim().slice(0, 80) : null,
    appListItemCount: appList ? appList.querySelectorAll('.launchpad-app-item').length : 0,
    chromeReady: !!(chromeObj && chromeObj.storage && chromeObj.storage.sync && chromeObj.runtime),
    retiredGroupUiPresent: upstreamGroups.length > 0,
    notice: notice ? String(notice.textContent).replace(/\s+/g, ' ').slice(0, 500) : null,
  };
}

/** 把帧内自检的结论写进占位说明（只在**没渲染出来**时才说话）。 */
function reportFrameVerdict() {
  const host = domReady() ? document.getElementById('upstreamHost') : null;
  if (!host) return;
  const verdict = probeUpstreamFrame();
  if (verdict.ready) return;
  const note = document.getElementById('upstreamNote');
  if (note) note.textContent = `上游设置界面没能渲染：${verdict.reason || '未知原因'}`;
}

// ---------- 状态条（spec §12.3） ----------

/** 判据是否还停在「加载失败 / 加载中」（重试后的轮询用于决定何时停）。 */
function stillFailed(page) {
  return !!page && (page.loadFailed === true || page.loading === true);
}

/**
 * 「重试」：走既有 `reload_main`（Rust 侧销毁主窗口并按配置重建）。
 *
 * 重建是异步的，紧接着取一次 `get_page_state` 大概率还是旧的失败态，所以轮询一小会儿：
 * 状态条于是从「加载失败」走到真实结果，而不是卡在旧快照上。错误页本身没有任何 IPC
 * 授权，重试入口只能在设置窗与托盘——这正是本函数存在的理由。
 */
async function retryMain() {
  state.error = null;
  renderStatus(true);
  try {
    await api.reloadMain(null);
  } catch (e) {
    state.error = `重试失败：${message(e)}`;
    await fetchPageSnapshot();
    renderStatus();
    renderShell();
    return;
  }
  const deadline = Date.now() + 8000;
  do {
    await sleep(600);
    await fetchPageSnapshot();
    renderStatus(true);
  } while (Date.now() < deadline && stillFailed(state.page));
  renderStatus();
  renderShell();
}

/**
 * 「把当前页加入白名单」：走 `set_config {mods:{enabledOrigins}}`。
 *
 * origin 来自 `get_page_state`（Rust 用 `config::origin_of` 解析出来的），不是页面上抓来的
 * 字符串；去重/小写由 `status.js::addCurrentOriginToWhitelist` 与 Rust 的 `Config::normalize`
 * 双保险。写完显式重建主窗口：注入载荷是建窗时注册的（活窗口换不掉），不重建就要等下一次导航。
 */
async function whitelistCurrentOrigin() {
  const origin = state.page && state.page.origin;
  if (!origin) return;
  const next = addCurrentOriginToWhitelist(origin, (state.config && state.config.mods || {}).enabledOrigins);
  try {
    const res = await api.setConfig({ mods: { enabledOrigins: next } });
    adoptConfig(res.config);
    state.error = null;
    await api.reloadMain(null);
  } catch (e) {
    state.error = `加入白名单失败：${message(e)}`;
  }
  await fetchPageSnapshot();
  renderStatus();
  renderShell();
}

/** 把状态条重新画一遍（`status.js::statusFor` 是唯一的判据来源）。 */
function renderStatus(busy) {
  if (!domReady()) return;
  const model = statusFor(state.config, state.page, state.report);
  const bar = statusBar(model.text, model.kind);
  if (!bar) return;
  for (const action of model.actions) {
    if (action === 'retry') {
      const b = button('重试');
      b.id = 'statusRetry';
      b.disabled = !!busy;
      b.addEventListener('click', () => { retryMain(); });
      bar.appendChild(b);
    } else if (action === 'whitelist') {
      const b = button('把当前页加入白名单');
      b.id = 'statusWhitelist';
      b.disabled = !!busy;
      b.addEventListener('click', () => { whitelistCurrentOrigin(); });
      bar.appendChild(b);
    }
  }
}

// ---------- 外壳开关（上游 UI 里没有的两项） ----------

/**
 * `shell.injectEnabled`（注入总开关）与 `shell.nasUrl`。
 *
 * 为什么本壳还要留这两项（T14b 的设计取舍，不是漏删）：上游 popup 是**扩展**的设置界面，
 * 它假设「扩展总是被注入」，所以没有注入总开关，也没有「NAS 地址」这个概念（上游用
 * `enabledOrigins` 白名单表达同一件事的另一半）。而本壳这两个键有真实语义：
 * `injectEnabled=false` 会让宿主**不注册**任何 mods 初始化脚本（T7+8 的 gap (a)）；
 * `nasUrl` 在保存时会把它的 origin 并入白名单，是「一键把 NAS 加进来」的入口。
 * 托盘精简（T14a）之后，注入总开关只剩设置窗这一个入口——把它一起删掉就等于删功能。
 * 因此它们放在**本壳自己的区域**里（与上游界面并列），而不是塞进上游 UI。
 */
function renderShell() {
  if (!domReady()) return;
  const host = document.getElementById('shellFields');
  if (!host) return;
  host.textContent = '';
  const shell = (state.config && state.config.shell) || {};

  const injectField = el('div', { className: 'field' });
  const injectId = 'f_shell_injectEnabled';
  const injectLabel = el('label', { text: '注入 mods（总开关）' });
  injectLabel.htmlFor = injectId;
  const inject = el('input', { id: injectId });
  inject.type = 'checkbox';
  inject.checked = shell.injectEnabled !== false;
  inject.addEventListener('change', () => { commitShell('injectEnabled', inject.checked); });
  injectField.append(injectLabel, inject, el('p', {
    className: 'hint',
    text: '关掉之后宿主不再为任何窗口注册 mods 载荷（改这一项会重建主窗口）。',
  }));
  host.appendChild(injectField);

  const nasField = el('div', { className: 'field' });
  const nasId = 'f_shell_nasUrl';
  const nasLabel = el('label', { text: 'NAS WebUI 地址' });
  nasLabel.htmlFor = nasId;
  const nas = el('input', { id: nasId });
  nas.type = 'text';
  nas.placeholder = 'http://192.168.1.10:8000';
  nas.value = typeof shell.nasUrl === 'string' ? shell.nasUrl : '';
  const nasSave = button('保存');
  nasSave.id = 'f_shell_nasUrl_save';
  const commitNas = () => { commitShell('nasUrl', nas.value); };
  nasSave.addEventListener('click', commitNas);
  nas.addEventListener('keydown', (e) => { if (e.key === 'Enter') commitNas(); });
  nasField.append(nasLabel, nas, nasSave, el('p', {
    className: 'hint',
    text: '保存后宿主会把它的 origin 并入 mods.enabledOrigins（白名单）。',
  }));
  host.appendChild(nasField);

  const actions = el('div', { className: 'field' });
  const openDir = button('打开配置目录');
  openDir.id = 'shellOpenDir';
  openDir.addEventListener('click', async () => {
    try {
      await api.openConfigDir();
      state.error = null;
    } catch (e) {
      state.error = `打开配置目录失败：${message(e)}`;
    }
    renderShell();
  });
  actions.appendChild(openDir);
  host.appendChild(actions);

  if (state.error) host.insertBefore(el('p', { className: 'error-banner', text: state.error }), host.firstChild);
}

// ---------- 关于页（spec §10：合规与品牌） ----------

/**
 * 关于页要展示的两个合规件路径（spec §10 / Ruling R54）。
 *
 * 优先用宿主解析出的**随包真实路径**（`meta.licensePath` / `meta.noticePath`），老宿主
 * （或只喂半个 meta 的单测）没有它们时才回落到源码树里的 vendored 位置，而不是画 `undefined`。
 */
export function compliancePaths(meta) {
  const m = meta || {};
  return {
    license: m.licensePath || `${VENDOR_DIR}/LICENSE`,
    notice: m.noticePath || `${VENDOR_DIR}/NOTICE`,
  };
}

function metaRow(label, value, tag, className) {
  const row = el('div', { className: 'row' });
  row.appendChild(el('span', { className: 'row-label', text: label }));
  row.appendChild(el(tag || 'b', { className: className || 'row-value', text: value }));
  return row;
}

function renderAbout() {
  if (!domReady()) return;
  const about = document.getElementById('about');
  if (!about) return;
  about.textContent = '';
  const meta = (state.config && state.config.meta) || {};

  const card = el('div', { className: 'card' });
  card.appendChild(el('h2', { className: 'pane-title', text: '关于' }));
  card.appendChild(metaRow('应用版本', meta.shellVersion || '未知'));
  card.appendChild(metaRow('mods commit', meta.modsCommit || '未知'));
  card.appendChild(metaRow('mods 版本', meta.modsVersion || '未知'));
  // Rust 侧字段是 `webview_version` + `#[serde(rename_all = "camelCase")]`，serde 只把 `_v`
  // 变成 `V`，因此真实 JSON 键是 **`webviewVersion`**（不是 `webViewVersion`）。两个都读：
  // 契约写法差异不该表现为「关于页永远显示未知」。
  const webviewVersion = meta.webViewVersion || meta.webviewVersion;
  card.appendChild(metaRow('WebView2 版本', webviewVersion || '未知（未取到运行时版本）'));
  // spec §12.3 / §14：`corner-shape` 需要 Chromium/WebView2 139+；取不到版本时什么都不说。
  const shapeHint = cornerShapeHint(webviewVersion);
  if (shapeHint) {
    card.appendChild(el('p', { id: 'cornerShapeHint', className: 'hint warn', text: shapeHint, attrs: { role: 'note' } }));
  }
  card.appendChild(metaRow('配置文件', meta.configPath || '未知', 'code', 'row-value path'));
  // spec §10 / R54：必须指出**随包**许可全文与 NOTICE 的真实位置。
  const legalPaths = compliancePaths(meta);
  card.appendChild(metaRow('上游许可全文', legalPaths.license, 'code', 'row-value path'));
  card.appendChild(metaRow('来源与改动声明', legalPaths.notice, 'code', 'row-value path'));
  about.appendChild(card);

  const actions = el('div', { className: 'card' });
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
    renderShell();
    renderStatus();
  });
  actions.appendChild(reset);
  about.appendChild(actions);

  // 许可与免责（spec §10）：非官方 + 非商业 + 上游出处 + vendored 许可全文位置。
  const legal = el('div', { className: 'card legal' });
  legal.appendChild(el('p', {
    className: 'legal-line',
    text: '本应用是第三方桌面壳，非飞牛（fnOS）官方产品，与飞牛官方无任何关联，也未获其授权或认可。',
  }));
  legal.appendChild(el('p', {
    className: 'legal-line',
    text: '随应用注入的界面修改资源（CSS/JS）来自上游开源项目 fnOS UI Mods，遵循其 Non-Commercial License 1.0，仅供非商业个人使用；本应用及这些资源均不得用于任何商业用途。上游资源按原样保留、未作修改，本壳仅做注入与包装性改动（清单见 NOTICE）。',
  }));
  legal.appendChild(el('p', {
    className: 'legal-line',
    text: '本窗的主界面就是上游自己的设置界面（popup.html + popup.js，逐字节原样 + 一行 chrome.* 兼容层标签），'
      + '本壳只额外提供状态条、上面这两项外壳开关与这一页关于/合规信息。',
  }));
  const linkLine = el('p', { className: 'legal-line' });
  linkLine.appendChild(document.createTextNode('上游项目：'));
  const link = el('a', { text: UPSTREAM_REPO });
  link.href = UPSTREAM_REPO;
  link.rel = 'noopener noreferrer';
  link.id = 'upstreamLink';
  // 外链**不在窗内导航**，交给 Rust `open_url` 用系统默认浏览器打开（Review finding D）。
  // 不加 preventDefault 的话 Chromium 会在设置窗自身里导航到 GitHub——UI 被顶掉，而
  // capability 只授权本地来源；`target=_blank` 也救不了（wry 默认不注册 new_window_handler，
  // 新窗口请求被静默吞掉）。`href` 仍保留：URL 可见、可复制、在无障碍树里仍是 Hyperlink。
  link.addEventListener('click', async (e) => {
    e.preventDefault();
    try {
      await api.openUrl(UPSTREAM_REPO);
      state.error = null;
    } catch (err) {
      state.error = `打开上游链接失败：${message(err)}`;
      renderShell();
    }
  });
  linkLine.appendChild(link);
  legal.appendChild(linkLine);
  legal.appendChild(el('p', {
    className: 'legal-line dim',
    text: `上游许可全文：${legalPaths.license}；来源与改动声明：${legalPaths.notice}（后者含来源仓库、锁定 commit、各文件 SHA-256、本壳的包装性改动清单）。两者都随安装包分发；上方的链接会用系统默认浏览器打开上游仓库，若被系统策略拦截，可手动复制上面的路径。`,
  }));
  about.appendChild(legal);
}

// ---------- 生命周期 ----------

let refreshing = false;

function sameConfig(a, b) {
  return !!a && !!b && JSON.stringify(a) === JSON.stringify(b);
}

/**
 * 重取配置 + 主窗口快照并就地重画。
 *
 * 为什么需要：配置可以被本窗**之外**的入口改动（手工编辑 `config.json`、`set_config` 的
 * 带外调用），而那些入口不向本窗发任何事件；主窗口更是**可以不改配置**就导航（用户点链接、
 * 错误页自动重试）。窗口重新获得焦点时刷新是最小实现。
 *
 * **不重挂上游帧**：上游 UI 有自己的输入状态与滚动位置，重新加载 iframe 会把它清掉；
 * 上游自己通过 `storage.sync.set` 写配置，所以它的界面与配置天然同步。本窗只重画
 * 状态条、外壳开关与关于页（它们才是本壳的视图）。
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
  await fetchPageSnapshot();
  refreshing = false;
  if (failure) {
    state.error = `刷新配置失败：${message(failure)}`;
    renderStatus();
    renderShell();
    return;
  }
  if (!sameConfig(state.config, next)) {
    adoptConfig(next);
    state.error = null;
    renderAbout();
  }
  renderStatus();
  renderShell();
}

/**
 * 启动：取配置 → 写快照 → 挂宿主桥 → **然后**才建上游帧 → 画状态条/外壳/关于。
 *
 * 次序是硬约束（见 `mountUpstreamFrame` 的注释）：上游在脚本开头同步读版本号。
 * 导出以便单测；无 DOM 时模块加载不会自动执行（见文件末尾）。
 */
export async function boot() {
  try {
    adoptConfig(await api.getConfig());
  } catch (e) {
    state.error = `读取配置失败：${message(e)}`;
  }
  await fetchPageSnapshot();
  renderStatus();
  renderShell();
  renderAbout();
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
