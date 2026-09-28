// 设置窗主逻辑：schema 驱动渲染 + IPC 提交（spec §8）。
//
// 三条硬约束：
// 1. **显示的值 = 归一化后的值**（§8.4）：`state.config.mods` 一律先过 `normalizeMods`，
//    任何渲染都只读 `state.config`，绝不把 IPC 原始值或用户刚输入的原文画到界面上。
// 2. **提交后就地重渲染**：`set_config` 返回的是 Rust 归一化后的权威配置，
//    以它为准刷新界面（因此「取色器选了 #ffffff，右侧显示 #b3b3b3」是同一份数据的两个视图）。
// 3. `needsReload` 为真（只有 `shell.injectEnabled` / `shell.homeUrl` 会）时随后调
//    `reload_main`——Rust 侧销毁并按新载荷重建主窗口（§6.6 勘误）。
import { SCHEMA, UPSTREAM_REPO, VENDOR_DIR } from './schema.js';
import { normalizeMods, normalizeOrigin, DEFAULT_BRAND_COLOR } from './normalize.js';
import * as api from './bridge.js';

/** 配置的三个段（键前缀）。 */
const SECTIONS = ['mods', 'local', 'shell'];

const state = { config: null, active: null, error: null };

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

/** 把 IPC 返回的配置收进 state：`mods` 必须先归一化（§8.4）。 */
function adoptConfig(raw) {
  if (!raw || typeof raw !== 'object') return;
  state.config = { ...raw, mods: normalizeMods(raw.mods) };
}

// ---------- 提交 ----------

/** 提交一个设置项：`set_config` → 需要时 `reload_main` → 用返回的权威配置重渲染。 */
async function commit(key, value, node) {
  if (node) node.classList.add('pending');
  try {
    const res = await api.setConfig(setPath({}, resolvePath(key), value));
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
      const doAdd = () => {
        let origin = '';
        try {
          // 与 Rust `origin_of` 同义：只取 scheme://host[:port]，并小写化
          origin = normalizeOrigin(new URL(addInput.value.trim()).origin);
        } catch (e) {
          origin = '';
        }
        if (!origin) {
          addInput.classList.add('error');
          addInput.focus();
          return;
        }
        addInput.classList.remove('error');
        addInput.value = '';
        const next = list.concat([origin]).filter((o, i, a) => a.indexOf(o) === i);
        commit(key, next, wrap);
      };
      add.addEventListener('click', doAdd);
      addInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') doAdd(); });
      addRow.append(addInput, add);
      box.appendChild(addRow);
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
  // 「关于页永远显示未知」。本轮真机验证正是这样抓到它的（详见 task-9-report.md）。
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
  // `target=_blank` 是**必需的**，不是装饰：wry 在 `new_window_handler` 为 None 时
  // 直接 `args.SetHandled(true)`（wry-0.57.0/src/webview2/mod.rs 的 NewWindowRequested
  // 分支），新窗口请求被吞掉 = 点击不动；若不加 target，则会在**设置窗自身**里导航到
  // GitHub——UI 被顶掉、capability 又只授权本地来源，设置窗就废了。
  // 这里刻意不 preventDefault：将来 Rust 侧补上 on_new_window（系统浏览器打开）后链接自动可用。
  link.target = '_blank';
  link.rel = 'noopener noreferrer';
  link.id = 'upstreamLink';
  linkLine.appendChild(link);
  legal.appendChild(linkLine);
  legal.appendChild(el('p', {
    className: 'legal-line dim',
    text: `上游许可全文与版权声明：${VENDOR_DIR}/LICENSE（另有 ${VENDOR_DIR}/NOTICE：来源仓库、锁定 commit、各文件 SHA-256、本壳的包装性改动清单）。链接仅作展示，当前窗口未注册外部打开处理器，点击不会跳转，需手动复制到浏览器。`
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

async function boot() {
  try {
    adoptConfig(await api.getConfig());
  } catch (e) {
    state.error = `读取配置失败：${message(e)}`;
  }
  render();
}

boot();
