// 设置页（T14g，用户要求：参考图重做设置页 + 检查更新移进设置页 + 实时下载进度）。
//
// 结构：
// - 左侧分页导航（通用 / 外观 / 桌面 / 更新 / 高级 / 关于），右侧是设置卡片；
// - 每个控件带 `data-patch="<段>.<键>"`，由本文件统一「读配置 → 渲染」与「改控件 → set_config」；
// - 上游扩展的设置项**全部移植到本页**（T14g 修复轮 3：上游设置窗口退役）：外观/字体、
//   桌面/完美图标逐项、高级/自定义代码与登录壁纸都在这里，不再有第二个设置窗口。
// - 「更新」分页轮询 `get_update_state` 渲染实时下载进度（没有事件总线，也没有新增 IPC 面）。
//
// 纪律：显示的值 = 生效的值（一律采纳 `set_config` 的返回）；页面可控文本只用 textContent，
// 绝不 innerHTML。
import * as api from './bridge.js';
import {
  PREFECT_ICONS,
  applyIconSelection,
  appItemsFromReport,
  isTooLargeReport,
  pickReportSlots,
  prefectIconPath,
  selectionFromConfig,
  selectionPatch,
} from './page-items.js';

/** 分页顺序（侧边栏与面板的 `data-section` / `data-panel` 必须与它一致）。 */
export const SECTIONS = ['general', 'appearance', 'desktop', 'update', 'advanced', 'about'];

/**
 * 更新阶段的字符串——与 Rust `updater::phase` **逐字一致**（跨语言锁：
 * `tests/settings-page.test.mjs` 与 `src-tauri/src/updater.rs` 的测试读同一组字面量）。
 */
export const UPDATE_PHASES = [
  'idle', 'checking', 'upToDate', 'downloading', 'ready', 'installing', 'error',
];

/** 进度轮询间隔（只在「更新」分页可见时轮询）。 */
export const UPDATE_POLL_MS = 350;

/** 主题色预设（与参考图的圆点一致：默认蓝 + 常用强调色）。 */
export const SWATCHES = [
  '#0066ff', '#4d6bfe', '#3a3d45', '#e5484d', '#e05c9a', '#f5a524', '#22c55e',
];

/** 上游扩展仓库（与 `updater::APP_REPO_SLUG` 同一组织下的界面美化资源）。 */
export const UPSTREAM_REPO_URL = 'https://github.com/aurysian-yan/fnOS_UI_Mods';

/** 应用项读取的有界等待（与上游 UI 时期的同一组预算）。 */
export const APP_ITEMS_WAIT_MS = 6000;
export const APP_ITEMS_POLL_MS = 400;

export const state = {
  config: null,
  section: 'general',
  update: null,
  error: null,
  /** 主窗口当前文档的 origin（「为当前站点注入」用）；拿不到为 null。 */
  siteOrigin: null,
  /** 完美图标逐项：页面报告的应用列表（`null` = 还没读取/读取失败）。 */
  iconItems: null,
  /** 完美图标逐项：当前选择 `{scale, mask, redraw}`。 */
  iconSelection: null,
};

// ---------- 纯函数（可单测，不碰 DOM） ----------

/** 按 `a.b` 取配置值（取不到给 `undefined`）。 */
export function readPath(obj, path) {
  const parts = String(path || '').split('.');
  let cur = obj;
  for (const p of parts) {
    if (cur == null || typeof cur !== 'object') return undefined;
    cur = cur[p];
  }
  return cur;
}

/** `a.b` + 值 → `set_config` 的 patch（只带这一条，绝不整段覆盖）。 */
export function patchFor(path, value) {
  const parts = String(path || '').split('.');
  if (parts.length !== 2 || !parts[0] || !parts[1]) {
    throw new Error(`配置路径必须是「段.键」：${path}`);
  }
  return { [parts[0]]: { [parts[1]]: value } };
}

/** 字节数 → 人类可读（1.0 MB / 512 KB）。 */
export function formatBytes(bytes) {
  const n = Number(bytes);
  if (!Number.isFinite(n) || n < 0) return '—';
  if (n < 1024) return `${Math.round(n)} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/** 速度（字节/秒）→ 可读文本；未知/为 0 时给空串（不显示「0 B/s」这种噪声）。 */
export function formatSpeed(bytesPerSec) {
  const n = Number(bytesPerSec);
  if (!Number.isFinite(n) || n <= 0) return '';
  return `${formatBytes(n)}/s`;
}

/**
 * 进度百分比：0–100 的整数；总大小未知时返回 `null`（调用方显示「已下载 X」而不是假进度）。
 * 已下载达到总大小时封顶 100（服务端多报几个字节也不显示 101%）。
 */
export function progressPercent(status) {
  const total = Number(status && status.total);
  const done = Number(status && status.downloaded);
  if (!Number.isFinite(total) || total <= 0 || !Number.isFinite(done) || done < 0) return null;
  return Math.max(0, Math.min(100, Math.floor((done / total) * 100)));
}

/** 「更新」分页主文案（阶段 → 一句话）。 */
export function updateHeadline(status) {
  const s = status && typeof status === 'object' ? status : {};
  const latest = typeof s.latest === 'string' ? s.latest : '';
  switch (s.phase) {
    case 'checking': return '正在检查更新…';
    case 'downloading': return latest ? `正在下载 ${latest}…` : '正在下载更新…';
    case 'ready': return typeof s.message === 'string' && s.message ? s.message : '新版本已下载。';
    case 'installing': return '正在安装并重启…';
    case 'error': return typeof s.message === 'string' && s.message ? s.message : '更新失败。';
    case 'upToDate': return typeof s.message === 'string' && s.message ? s.message : '已是最新版本。';
    default: return '从 GitHub Releases 检查新版本；有新版本会自动下载。';
  }
}

/** 按钮状态：检查按钮在跑的时候置灰；「安装并重启」只在就绪时出现。 */
export function updateButtons(status) {
  const phase = status && status.phase;
  return {
    checkDisabled: phase === 'checking' || phase === 'downloading' || phase === 'installing',
    installVisible: phase === 'ready',
  };
}

/** 进度条下方左侧文本（百分比；总量未知时退回「已下载」）。 */
export function progressText(status) {
  const pct = progressPercent(status);
  const done = Number(status && status.downloaded) || 0;
  const total = Number(status && status.total);
  if (pct === null) return `已下载 ${formatBytes(done)}`;
  return `${pct}% · ${formatBytes(done)} / ${formatBytes(total)}`;
}

/** 进度条下方右侧文本（速度；未知时给空串）。 */
export function progressSpeed(status) {
  return formatSpeed(status && status.bytesPerSec);
}

// ---------- DOM ----------

function dom() {
  return typeof document !== 'undefined' ? document : null;
}

let toastTimer = null;
function notice(text, isError = false) {
  const el = dom() && dom().getElementById('toast');
  if (!el) return;
  el.textContent = String(text || '');
  el.classList.toggle('error', !!isError);
  if (toastTimer !== null) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toastTimer = null;
    el.textContent = '';
    el.classList.remove('error');
  }, 3200);
}

/** 采纳一份新配置并整页重绘（显示的值 = 生效的值）。 */
export function adoptConfig(view) {
  if (!view || typeof view !== 'object') return;
  state.config = view;
  renderConfig(view);
}

/** 把配置值写进所有控件。 */
export function renderConfig(view) {
  const doc = dom();
  if (!doc || !view) return;
  for (const el of doc.querySelectorAll('[data-patch]')) {
    const path = el.getAttribute('data-patch');
    const value = readPath(view, path);
    const kind = el.getAttribute('data-kind');
    if (kind === 'switch') {
      el.setAttribute('aria-checked', value === true ? 'true' : 'false');
    } else if (kind === 'segmented') {
      for (const b of el.querySelectorAll('button[data-value]')) {
        b.classList.toggle('on', String(value) === b.getAttribute('data-value'));
      }
    } else if (kind === 'color') {
      const v = typeof value === 'string' ? value.toLowerCase() : '';
      for (const b of el.querySelectorAll('.swatch')) {
        b.classList.toggle('on', b.getAttribute('data-value').toLowerCase() === v);
      }
      const custom = el.querySelector('.swatch-custom');
      if (custom && /^#[0-9a-f]{6}$/i.test(v)) custom.value = v;
    } else if (kind === 'number' || kind === 'text' || kind === 'textarea') {
      const next = value == null ? '' : String(value);
      if (el.value !== next) el.value = next;
    }
  }
  // 登录壁纸的当前文件名（local.loginWallpaperFileName；null = 未设置）
  const wallpaper = readPath(view, 'local.loginWallpaperFileName');
  const wallpaperEl = doc.getElementById('wallpaperName');
  if (wallpaperEl) {
    wallpaperEl.textContent = typeof wallpaper === 'string' && wallpaper
      ? wallpaper
      : '未设置';
  }
  renderSiteToggle();
  // meta 文本（版本 / 路径 / 上游版本）
  const meta = view.meta && typeof view.meta === 'object' ? view.meta : {};
  const text = (id, value) => {
    const el = doc.getElementById(id);
    if (el) el.textContent = value == null ? '—' : String(value);
  };
  text('aboutVersion', meta.shellVersion ? `v${meta.shellVersion}` : '—');
  text('updateCurrent', meta.shellVersion ? `v${meta.shellVersion}` : '—');
  text('aboutMods', meta.modsVersion
    ? `fnOS_UI_Mods v${meta.modsVersion}（${String(meta.modsCommit || '').slice(0, 7)}）`
    : '—');
  text('configPath', meta.configPath);
  text('aboutLicense', meta.licensePath);
  text('aboutNotice', meta.noticePath);
}

/** 渲染更新状态（进度条 / 文案 / 按钮）。 */
export function renderUpdate(status) {
  state.update = status && typeof status === 'object' ? status : null;
  const doc = dom();
  if (!doc) return;
  const s = state.update || {};
  const message = doc.getElementById('updateMessage');
  if (message) message.textContent = updateHeadline(s);

  const progress = doc.getElementById('updateProgress');
  const fill = doc.getElementById('updateProgressFill');
  const textEl = doc.getElementById('updateProgressText');
  const speedEl = doc.getElementById('updateProgressSpeed');
  const downloading = s.phase === 'downloading';
  if (progress) progress.hidden = !downloading;
  if (downloading) {
    const pct = progressPercent(s);
    if (fill) fill.style.width = `${pct === null ? 100 : pct}%`;
    if (textEl) textEl.textContent = progressText(s);
    if (speedEl) speedEl.textContent = progressSpeed(s);
  }

  const { checkDisabled, installVisible } = updateButtons(s);
  const check = doc.getElementById('updateCheck');
  const install = doc.getElementById('updateInstall');
  if (check) {
    check.disabled = checkDisabled;
    check.textContent = s.phase === 'installing' ? '安装中…' : '检查更新';
  }
  if (install) install.hidden = !installVisible;

  // 没有 digest 的老 release：安装按钮旁必须如实说明（不能只在日志里）
  if (installVisible && s.hasDigest === false && message) {
    message.textContent = `${updateHeadline(s)}（⚠ 该 release 没有 sha256 digest，安装包只做了体积校验）`;
  }

  // 「当前版本」那一行的说明：阶段 → 一句状态（不重复检查按钮那一行）
  const currentDesc = doc.getElementById('updateCurrentDesc');
  if (currentDesc) {
    if (s.phase === 'upToDate') currentDesc.textContent = s.message || '已是最新版本。';
    else if (s.latest && (s.phase === 'downloading' || s.phase === 'ready')) currentDesc.textContent = `发现新版本 ${s.latest}`;
    else if (s.phase === 'checking') currentDesc.textContent = '正在检查…';
    else if (s.phase === 'installing') currentDesc.textContent = '安装完成会自动重启。';
    else if (s.phase === 'error') currentDesc.textContent = '上次检查失败。';
    else currentDesc.textContent = '尚未检查更新。';
  }
}

let pollTimer = null;
async function refreshUpdate() {
  try {
    renderUpdate(await api.getUpdateState());
  } catch (e) {
    state.error = String((e && e.message) || e);
  }
}

function setPolling(on) {
  if (on && pollTimer === null) {
    pollTimer = setInterval(refreshUpdate, UPDATE_POLL_MS);
  } else if (!on && pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

/** 切换分页（导出供单测驱动；DOM 不存在时只更新 state）。 */
export function selectSection(id) {
  const next = SECTIONS.includes(id) ? id : 'general';
  state.section = next;
  const doc = dom();
  if (!doc) return;
  for (const item of doc.querySelectorAll('.nav-item')) {
    const on = item.getAttribute('data-section') === next;
    item.classList.toggle('active', on);
    if (on) item.setAttribute('aria-current', 'page');
    else item.removeAttribute('aria-current');
  }
  for (const panel of doc.querySelectorAll('.panel')) {
    panel.classList.toggle('active', panel.getAttribute('data-panel') === next);
  }
  if (next === 'update') {
    refreshUpdate();
    setPolling(true);
  } else {
    setPolling(false);
  }
  if (next === 'advanced') {
    // 上游设置现在是**独立窗口**（`upstream.html`）：离开本页时它可能改过配置，
    // 回到本页时重取一次即可（其余分页不受影响）。
    refreshConfig();
  }
}

async function refreshConfig() {
  try {
    adoptConfig(await api.getConfig());
  } catch (e) {
    notice(`读取配置失败：${(e && e.message) || e}`, true);
  }
}

async function commit(path, value) {
  return commitPatch(patchFor(path, value));
}

/** 多键补丁的提交入口（逐项图标选择、站点白名单要一次写多条键）。 */
async function commitPatch(patch) {
  try {
    const res = await api.setConfig(patch);
    if (res && res.config) adoptConfig(res.config);
    if (res && res.needsReload) await api.reloadMain(null);
    notice('已保存');
    return true;
  } catch (e) {
    notice(`保存失败：${(e && e.message) || e}`, true);
    await refreshConfig(); // 回滚到真实值（显示的值 = 生效的值）
    return false;
  }
}

// ---------- 通用：「为当前站点注入」（读主窗口当前 origin + enabledOrigins） ----------

/** 当前主窗口文档的 origin（拿不到返回 null）。 */
export async function fetchSiteOrigin() {
  try {
    const page = await api.getPageState();
    const origin = page && typeof page.origin === 'string' ? page.origin : '';
    return origin || null;
  } catch (e) {
    return null;
  }
}

async function refreshSiteState() {
  state.siteOrigin = await fetchSiteOrigin();
  renderSiteToggle();
}

/** 白名单里是否已有这个 origin（大小写不敏感；Rust 落在配置里的是小写）。 */
export function originsInclude(origins, origin) {
  const o = String(origin || '').toLowerCase();
  if (!o) return false;
  return Array.isArray(origins) && origins.some((x) => String(x).toLowerCase() === o);
}

/** 切换后的新白名单（去掉大小写重复项；空 origin 原样返回）。 */
export function toggleOrigin(origins, origin, on) {
  const list = Array.isArray(origins) ? origins.filter((x) => typeof x === 'string' && x) : [];
  const o = String(origin || '').toLowerCase();
  if (!o) return list;
  const without = list.filter((x) => x.toLowerCase() !== o);
  return on ? [...without, o] : without;
}

/** `https://x/` → `https://x`（解析失败给空串；与 Rust `Url::origin()` 同一形状）。 */
export function originOf(raw) {
  try {
    const u = new URL(String(raw || '').trim());
    if (u.protocol !== 'http:' && u.protocol !== 'https:') return '';
    return u.origin;
  } catch (e) {
    return '';
  }
}

/**
 * 「桌面地址 / 官网入口」自带的放行：这两个 origin 不需要白名单（闸门本来就会放行，
 * `normalize` 还会把 nasUrl 的 origin 自动并进白名单）。对它们来说开关是**恒真且不可关**的，
 * 如实显示为禁用而不是让用户点了没反应。
 */
export function configForcedOrigins(config) {
  const shell = (config && config.shell) || {};
  return [originOf(shell.nasUrl), originOf(shell.homeUrl)].filter(Boolean);
}

function renderSiteToggle() {
  const doc = dom();
  if (!doc) return;
  const toggle = doc.getElementById('siteToggle');
  const desc = doc.getElementById('siteOriginDesc');
  const origin = state.siteOrigin;
  const origins = (state.config && state.config.mods && state.config.mods.enabledOrigins) || [];
  const forced = origin
    ? configForcedOrigins(state.config).some((o) => o.toLowerCase() === origin.toLowerCase())
    : false;
  if (desc) {
    if (!origin) desc.textContent = '当前站点：—（主窗口还没有可识别的地址）';
    else if (forced) desc.textContent = `当前站点：${origin}（由「桌面地址 / 官网入口」自带放行，无需开关）`;
    else desc.textContent = `当前站点：${origin}`;
  }
  if (toggle) {
    toggle.disabled = !origin || forced;
    const on = forced || originsInclude(origins, origin);
    toggle.setAttribute('aria-checked', on ? 'true' : 'false');
  }
}

async function toggleSiteInjection() {
  const doc = dom();
  const toggle = doc && doc.getElementById('siteToggle');
  if (!toggle || !state.siteOrigin) return;
  if (!state.siteOrigin) return;
  const origins = (state.config && state.config.mods && state.config.mods.enabledOrigins) || [];
  const next = toggleOrigin(origins, state.siteOrigin, toggle.getAttribute('aria-checked') !== 'true');
  toggle.setAttribute('aria-checked', originsInclude(next, state.siteOrigin) ? 'true' : 'false');
  await commitPatch({ mods: { enabledOrigins: next } });
}

// ---------- 桌面：完美图标逐项 ----------

function readIconSelection() {
  if (!state.iconSelection) {
    state.iconSelection = selectionFromConfig(state.config && state.config.mods);
  }
  return state.iconSelection;
}

/** 把应用列表画进 #iconItemsList（页面可控文本只用 textContent）。 */
export function renderIconItems() {
  const doc = dom();
  if (!doc) return;
  const list = doc.getElementById('iconItemsList');
  if (!list) return;
  if (!state.iconItems) return; // 还没读取：保留提示语
  const sel = readIconSelection();
  list.textContent = '';
  if (state.iconItems.length === 0) {
    const empty = doc.createElement('div');
    empty.className = 'items-empty';
    empty.textContent = '没有读到应用。先打开一次启动台，再点「读取应用列表」。';
    list.appendChild(empty);
    return;
  }
  for (const item of state.iconItems) {
    const row = doc.createElement('div');
    row.className = 'item-row';

    const main = doc.createElement('div');
    main.className = 'item-title';
    main.textContent = item.title || item.key;
    main.title = item.key;
    if (item.title && item.title !== item.key) {
      const keyEl = doc.createElement('div');
      keyEl.className = 'item-key';
      keyEl.textContent = item.key;
      main.appendChild(keyEl);
    }
    row.appendChild(main);

    const controls = doc.createElement('div');
    controls.className = 'item-controls';

    const scale = doc.createElement('label');
    scale.className = 'check';
    const scaleInput = doc.createElement('input');
    scaleInput.type = 'checkbox';
    scaleInput.checked = sel.scale.includes(item.key);
    scaleInput.addEventListener('change', () => {
      state.iconSelection = applyIconSelection(readIconSelection(), item.key, { scale: scaleInput.checked });
      renderIconItems();
      commitPatch(selectionPatch(state.iconSelection));
    });
    scale.appendChild(scaleInput);
    scale.appendChild(doc.createTextNode('统一形状'));
    controls.appendChild(scale);

    const mask = doc.createElement('label');
    mask.className = 'check';
    const maskInput = doc.createElement('input');
    maskInput.type = 'checkbox';
    maskInput.checked = sel.mask.includes(item.key);
    maskInput.addEventListener('change', () => {
      state.iconSelection = applyIconSelection(readIconSelection(), item.key, { mask: maskInput.checked });
      renderIconItems();
      commitPatch(selectionPatch(state.iconSelection));
    });
    mask.appendChild(maskInput);
    mask.appendChild(doc.createTextNode('仅遮罩'));
    controls.appendChild(mask);

    const select = doc.createElement('select');
    select.className = 'select-sm';
    select.setAttribute('aria-label', `重绘图标：${item.title || item.key}`);
    const none = doc.createElement('option');
    none.value = '';
    none.textContent = '不重绘';
    select.appendChild(none);
    for (const icon of PREFECT_ICONS) {
      const opt = doc.createElement('option');
      opt.value = prefectIconPath(icon);
      opt.textContent = icon;
      select.appendChild(opt);
    }
    select.value = sel.redraw[item.key] || '';
    select.addEventListener('change', () => {
      state.iconSelection = applyIconSelection(readIconSelection(), item.key, { redraw: select.value });
      renderIconItems();
      commitPatch(selectionPatch(state.iconSelection));
    });
    controls.appendChild(select);

    row.appendChild(controls);
    list.appendChild(row);
  }
}

/**
 * 从主窗口页面读启动台应用列表：请页面重新汇报（`request_app_items`），再在有界窗口里
 * 轮询 `get_page_report` 的信封。拿不到就如实说明（不编假列表）。
 */
export async function loadIconItems(deps = {}) {
  const doc = dom();
  const hint = doc && doc.getElementById('iconItemsHint');
  const sleep = deps.sleep || ((ms) => new Promise((r) => setTimeout(r, ms)));
  const now = deps.now || (() => Date.now());
  const getReport = deps.getReport || api.getPageReport;
  const request = deps.request || api.requestAppItems;
  const waitMs = deps.waitMs || APP_ITEMS_WAIT_MS;
  const pollMs = deps.pollMs || APP_ITEMS_POLL_MS;

  if (hint) hint.textContent = '正在读取应用列表…';
  try {
    await request();
  } catch (e) {
    if (hint) hint.textContent = `读取失败：${(e && e.message) || e}（主窗口没打开？）`;
    return null;
  }
  const deadline = now() + waitMs;
  do {
    await sleep(pollMs);
    let envelope = null;
    try {
      envelope = await getReport();
    } catch (e) {
      envelope = null;
    }
    const slots = pickReportSlots(envelope);
    if (isTooLargeReport(slots.appItems)) {
      const payload = slots.appItems.payload || {};
      const count = Number(payload.itemCount) || 0;
      state.iconItems = [];
      if (hint) hint.textContent = `应用太多（${count} 个），逐项列表超出上报预算；请减少启动台应用，或直接手改 config.json 的 launchpadIconRedrawMap。`;
      renderIconItems();
      return state.iconItems;
    }
    const items = appItemsFromReport(slots.appItems);
    if (items && items.length >= 0) {
      state.iconItems = items;
      state.iconSelection = selectionFromConfig(state.config && state.config.mods);
      if (hint) {
        hint.textContent = items.length
          ? `共 ${items.length} 个应用；勾选后立即生效。`
          : '页面回报了 0 个应用（启动台为空？）。';
      }
      renderIconItems();
      return state.iconItems;
    }
  } while (now() < deadline);
  if (hint) hint.textContent = '没有等到应用列表。先打开一次启动台，再点「读取应用列表」。';
  return null;
}

// ---------- 高级：登录壁纸 ----------

/** data URL → 纯 base64（`import_wallpaper` 只收 base64）。 */
export function stripDataUrl(dataUrl) {
  const s = String(dataUrl || '');
  const comma = s.indexOf(',');
  if (comma < 0 || !s.slice(0, comma).includes(';base64')) return null;
  return s.slice(comma + 1);
}

async function importWallpaperFile(file) {
  if (!file) return;
  try {
    const dataUrl = await new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result || ''));
      reader.onerror = () => reject(reader.error || new Error('读取文件失败'));
      reader.readAsDataURL(file);
    });
    const base64 = stripDataUrl(dataUrl);
    if (!base64) {
      notice('只支持 png / jpg / jpeg / webp 图片', true);
      return;
    }
    const stored = await api.importWallpaper(file.name, base64);
    const res = await api.setConfig({ local: { loginWallpaperFileName: stored } });
    if (res && res.config) adoptConfig(res.config);
    if (res && res.needsReload) await api.reloadMain(null);
    notice('登录壁纸已导入');
  } catch (e) {
    notice(`导入壁纸失败：${(e && e.message) || e}`, true);
  }
}

async function clearWallpaper() {
  try {
    const res = await api.setConfig({ local: { loginWallpaperFileName: null } });
    if (res && res.config) adoptConfig(res.config);
    if (res && res.needsReload) await api.reloadMain(null);
    notice('已清除登录壁纸');
  } catch (e) {
    notice(`清除失败：${(e && e.message) || e}`, true);
  }
}

function bindControls() {
  const doc = dom();
  if (!doc) return;
  for (const el of doc.querySelectorAll('[data-patch]')) {
    const path = el.getAttribute('data-patch');
    const kind = el.getAttribute('data-kind');
    if (kind === 'switch') {
      el.addEventListener('click', () => {
        const next = el.getAttribute('aria-checked') !== 'true';
        el.setAttribute('aria-checked', next ? 'true' : 'false');
        commit(path, next);
      });
    } else if (kind === 'segmented') {
      el.addEventListener('click', (ev) => {
        const btn = ev.target.closest('button[data-value]');
        if (!btn) return;
        for (const b of el.querySelectorAll('button[data-value]')) b.classList.remove('on');
        btn.classList.add('on');
        commit(path, btn.getAttribute('data-value'));
      });
    } else if (kind === 'color') {
      el.addEventListener('click', (ev) => {
        const btn = ev.target.closest('.swatch');
        if (!btn) return;
        commit(path, btn.getAttribute('data-value'));
      });
      const custom = el.querySelector('.swatch-custom');
      if (custom) custom.addEventListener('change', () => commit(path, custom.value));
    } else if (kind === 'number' || kind === 'text' || kind === 'textarea') {
      el.addEventListener('change', () => {
        let value = el.value;
        if (kind === 'number') {
          const n = Number(value);
          if (!Number.isFinite(n)) {
            renderConfig(state.config); // 非法输入 → 回显真实值
            return;
          }
          const min = Number(el.getAttribute('min'));
          const max = Number(el.getAttribute('max'));
          let v = Math.round(n);
          if (Number.isFinite(min)) v = Math.max(min, v);
          if (Number.isFinite(max)) v = Math.min(max, v);
          value = v;
        }
        commit(path, value);
      });
    }
  }
}

function bindActions() {
  const doc = dom();
  if (!doc) return;
  for (const el of doc.querySelectorAll('[data-action]')) {
    el.addEventListener('click', async () => {
      const action = el.getAttribute('data-action');
      try {
        if (action === 'open-config-dir') await api.openConfigDir();
        else if (action === 'open-repo') await api.openUrl(UPSTREAM_REPO_URL);
        else if (action === 'open-upstream-settings') await api.openUpstreamSettings();
      } catch (e) {
        notice(`操作失败：${(e && e.message) || e}`, true);
      }
    });
  }

  const check = doc.getElementById('updateCheck');
  if (check) {
    check.addEventListener('click', async () => {
      check.disabled = true;
      try {
        renderUpdate(await api.startUpdateCheck());
        setPolling(true);
      } catch (e) {
        notice(`检查更新失败：${(e && e.message) || e}`, true);
        check.disabled = false;
      }
    });
  }
  const install = doc.getElementById('updateInstall');
  if (install) {
    install.addEventListener('click', async () => {
      install.disabled = true;
      try {
        await api.installUpdate();
        renderUpdate({ ...(state.update || {}), phase: 'installing' });
        setPolling(true);
      } catch (e) {
        install.disabled = false;
        notice(`安装失败：${(e && e.message) || e}`, true);
      }
    });
  }

  // 恢复默认：两次点击确认（不依赖原生 confirm——WebView2 里它会阻塞整条 UI 线程）
  const reset = doc.getElementById('resetAll');
  if (reset) {
    let armed = false;
    let timer = null;
    const disarm = () => {
      armed = false;
      reset.classList.remove('armed');
      reset.textContent = '恢复默认';
      if (timer !== null) clearTimeout(timer);
      timer = null;
    };
    reset.addEventListener('click', async () => {
      if (!armed) {
        armed = true;
        reset.classList.add('armed');
        reset.textContent = '再次点击确认';
        timer = setTimeout(disarm, 4000);
        return;
      }
      disarm();
      try {
        // `reset_config` 自己会在载荷变化时重建主窗口（与 `set_config` 的
        // `needsReload` 语义不同），这里不再额外调 `reload_main`。
        adoptConfig(await api.resetConfig('all'));
        notice('已恢复默认设置');
      } catch (e) {
        notice(`恢复默认失败：${(e && e.message) || e}`, true);
      }
    });
  }

  // 「为当前站点注入」：白名单开关（origin 来自主窗口当前文档）
  const siteToggle = doc.getElementById('siteToggle');
  if (siteToggle) siteToggle.addEventListener('click', () => { toggleSiteInjection(); });

  // 完美图标逐项：读取应用列表
  const iconRefresh = doc.getElementById('iconItemsRefresh');
  if (iconRefresh) {
    iconRefresh.addEventListener('click', async () => {
      iconRefresh.disabled = true;
      try {
        await loadIconItems();
      } finally {
        iconRefresh.disabled = false;
      }
    });
  }

  // 登录壁纸：选择 / 清除
  const pick = doc.getElementById('wallpaperPick');
  const fileInput = doc.getElementById('wallpaperFile');
  if (pick && fileInput) {
    pick.addEventListener('click', () => fileInput.click());
    fileInput.addEventListener('change', async () => {
      const file = fileInput.files && fileInput.files[0];
      fileInput.value = '';
      if (file) await importWallpaperFile(file);
    });
  }
  const clear = doc.getElementById('wallpaperClear');
  if (clear) clear.addEventListener('click', () => { clearWallpaper(); });
}

/** 启动：取配置 → 渲染 → 绑事件 → 挂分页切换；「更新」分页只在可见时轮询。 */
export async function boot() {
  const doc = dom();
  if (!doc) return;
  try {
    adoptConfig(await api.getConfig());
  } catch (e) {
    notice(`读取配置失败：${(e && e.message) || e}`, true);
  }
  bindControls();
  bindActions();
  for (const item of doc.querySelectorAll('.nav-item')) {
    item.addEventListener('click', () => selectSection(item.getAttribute('data-section')));
  }
  await refreshSiteState();
  await refreshUpdate();
  // 配置也可能被手工编辑改动：窗口重新获得焦点时刷新一次（站点 origin 也会变）
  window.addEventListener('focus', () => {
    refreshConfig();
    refreshSiteState();
    if (state.section === 'update') refreshUpdate();
  });
}

// 只有真实页面才自动启动：Node 单测 import 本模块时没有这些节点。
const hasPage = typeof document !== 'undefined' && !!document.getElementById('nav');
if (hasPage) boot();
