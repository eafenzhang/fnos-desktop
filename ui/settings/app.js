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
// - **焦点刷新**（`refresh`）：托盘等带外改动不发事件，重新获得焦点时重取配置。
// - **关于页外链**：不在窗内导航，交给 Rust `open_url` → 系统默认浏览器。
import { SCHEMA, UPSTREAM_REPO, VENDOR_DIR } from './schema.js';
import { MODS_KEYS, normalizeModsEntry, parseHttpOrigin, DEFAULT_BRAND_COLOR } from './normalize.js';
import * as api from './bridge.js';

/** 配置的三个段（键前缀）。 */
const SECTIONS = ['mods', 'local', 'shell'];

/** 界面状态。导出供单测与 Task 11 的状态条读取。 */
export const state = { config: null, active: null, error: null };

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
    if (res.needsReload) await api.reloadMain(null);
    state.error = null;
    render();
  } catch (e) {
    state.error = `保存「${key}」失败：${message(e)}`;
    render();
  } finally {
    if (node && node.isConnected) node.classList.remove('pending');
  }
}

// ---------- 控件 ----------

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

// ---------- 关于页（spec §10：合规与品牌） ----------

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
  card.appendChild(metaRow('配置文件', meta.configPath || '未知', 'code', 'row-value path'));
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
    text: `上游许可全文与版权声明：${VENDOR_DIR}/LICENSE（另有 ${VENDOR_DIR}/NOTICE：来源仓库、锁定 commit、各文件 SHA-256、本壳的包装性改动清单）。点击上面的链接会用系统默认浏览器打开；若被系统策略拦截，可手动复制地址。`
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
 * 重取配置并就地重渲染（Review finding C）。
 *
 * 为什么需要：托盘「注入 mods」勾选项走 `commands::set_inject_enabled`，它只改
 * `AppState` + 落盘 + `sync_menus`，**不向设置窗发任何事件**；而本窗只在 `boot()` 取过
 * 一次配置，于是带外改动后它会一直显示过期值，直到关掉重开。窗口重新获得焦点时刷新是
 * 最小实现：不引入事件总线、不改 Rust 侧。
 *
 * `render()` 会保留 `state.active`（当前分组）与 `pane.scrollTop`（滚动位置），
 * 所以刷新不会把用户弹回第一组。另外**配置没变就不重渲染**：否则每次 alt-tab 回来都会
 * 重建 DOM，把用户正在输入、尚未提交的文本（例如 NAS 地址）一起丢掉。
 *
 * **Task 11 的状态条直接复用这个 `refresh()`**（它需要的正是同一个「带外变更 → 重取」
 * 钩子）：把 `boot()` 里的 `focus` 监听留在这里，别在状态条里另建一套事件/轮询机制。
 */
export async function refresh() {
  if (refreshing) return;
  refreshing = true;
  let next = null;
  try {
    next = await api.getConfig();
  } catch (e) {
    state.error = `刷新配置失败：${message(e)}`;
    refreshing = false;
    render();
    return;
  }
  refreshing = false;
  if (sameConfig(state.config, next)) return;
  adoptConfig(next);
  state.error = null;
  render();
}

/** 取一次配置并渲染。导出以便单测；无 DOM 时模块加载不会自动执行（见文件末尾）。 */
export async function boot() {
  try {
    adoptConfig(await api.getConfig());
  } catch (e) {
    state.error = `读取配置失败：${message(e)}`;
  }
  render();
  window.addEventListener('focus', () => { refresh(); });
}

// 只有真实页面才自动启动：Node 单测 `import` 本模块时没有 DOM，直接 `boot()` 会抛错，
// 于是「导入 app.js 测内部逻辑」就变得不可行（Review 打磨项）。判据用 `#pane` 而不是
// 只看 `typeof document`：只有 DOM、没有页面骨架时同样不该启动。
const hasPane = typeof document !== 'undefined'
  && typeof document.getElementById === 'function'
  && !!document.getElementById('pane');
if (hasPane) boot();
