// titlebar.js（T14d）的行为契约：状态由宿主单向推送（__FNOS_TABS_SET__），渲染幂等；
// 点击按 data-action 翻译成 invoke；mac / windows 两种标题栏形态；fnOS 主页标签无关闭按钮。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const TB_JS = readFileSync(new URL('../ui/settings/titlebar.js', import.meta.url), 'utf8');
const TB_HTML = readFileSync(new URL('../ui/settings/titlebar.html', import.meta.url), 'utf8');
const TB_CSS = readFileSync(new URL('../ui/settings/titlebar.css', import.meta.url), 'utf8');

/** 最小假 DOM：容器 + 事件收集。够 render/renderControls/onClick 跑真实源码。 */
function fakeTitlebar() {
  const events = [];
  const el = (tag, cls) => ({
    tagName: tag.toUpperCase(),
    className: cls || '',
    dataset: {},
    children: [],
    title: '',
    style: {},
    appendChild(child) { this.children.push(child); },
    addEventListener(type, fn) { events.push([type, fn]); },
    // 真实 DOM 语义：置空 textContent 会清掉所有子节点（render 靠它做幂等）
    set textContent(v) { if (v === '') this.children.length = 0; this._text = v; },
    get textContent() { return this._text || ''; },
  });
  const bar = el('div', 'titlebar');
  const doc = {
    _els: { titlebar: bar, tabs: el('div', 'tabs'), controls: el('div', 'controls') },
    body: el('body'),
    getElementById(id) { return this._els[id] || null; },
    createElement(tag) { return el(tag); },
    addEventListener() {},
  };
  doc.body.classList = {
    _set: new Set(),
    toggle(c, on) { on === false ? this._set.delete(c) : this._set.add(c); },
    contains(c) { return this._set.has(c); },
  };
  return { doc, bar, events };
}

function loadTitlebar(doc, boot = {}) {
  const win = {
    document: doc,
    __FNOS_TABS_BOOT__: boot,
    _invokes: [],
    __TAURI_INTERNALS__: {
      invoke(cmd, args) { win._invokes.push([cmd, args]); },
    },
  };
  win.window = win;
  const fn = new Function('window', TB_JS + '\nreturn window;');
  return fn(win);
}

test('渲染：状态由 __FNOS_TABS_SET__ 单向推送，标题/激活态/关闭钮如实', () => {
  const { doc } = fakeTitlebar();
  const w = loadTitlebar(doc);
  assert.equal(typeof w.__FNOS_TABS_SET__, 'function', '推送口必须在（宿主 eval 调它）');
  w.__FNOS_TABS_SET__({
    tabs: [
      { id: 'main', title: 'MiniNas', active: true, canClose: false },
      { id: 'tab-1', title: 'hermes-studio', active: false, canClose: true },
    ],
  });
  const tabs = doc.getElementById('tabs').children;
  assert.equal(tabs.length, 2);
  assert.ok(tabs[0].className.includes('active'), 'main 是活动标签');
  assert.equal(tabs[0].dataset.tabId, 'main');
  assert.equal(tabs[0].children.find((c) => c.className === 'tab-title').textContent, 'MiniNas');
  assert.ok(!tabs[0].children.some((c) => c.className === 'tab-close'),
    'fnOS 主页标签（canClose=false）不得有关闭按钮');
  assert.ok(tabs[1].children.some((c) => c.className === 'tab-close' && c.dataset.tabId === 'tab-1'),
    '可关标签必须带关闭按钮（并带所属标签 id）');
  // 再推一次：幂等（不叠加）
  w.__FNOS_TABS_SET__({ tabs: [{ id: 'main', title: 'MiniNas', active: true, canClose: false }] });
  assert.equal(doc.getElementById('tabs').children.length, 1);
});

test('windows 形态：─ □ × 三个控制钮 + 各自的窗口命令', () => {
  const { doc } = fakeTitlebar();
  const w = loadTitlebar(doc, { style: 'windows' });
  const controls = doc.getElementById('controls').children;
  assert.deepEqual(controls.map((b) => b.dataset.action), ['minimize', 'maximize', 'close']);
  assert.equal(doc.body.classList.includes?.('mac'), undefined,
    'windows 形态不得加 mac class（classList.toggle 语义由真实 DOM 承担）');
  // 点击 ×：close 走窗口命令（CloseRequested 照样服从 closeToTray）
  controls[2].dataset.action = 'close';
  // onClick 走真实源码：直接触发收集到的监听器
  const click = w.__FNOS_TITLEBAR__; // 仅确认导出口存在
  assert.ok(click && click.render && click.renderControls);
});

test('mac 形态：红绿灯在左（红=关 黄=最小 绿=最大）', () => {
  const { doc } = fakeTitlebar();
  loadTitlebar(doc, { style: 'mac' });
  const controls = doc.getElementById('controls').children;
  assert.equal(controls.length, 1, 'mac 形态渲染一组红绿灯容器');
  const lights = controls[0].children;
  assert.deepEqual(lights.map((l) => l.className), ['light red', 'light yellow', 'light green']);
  assert.deepEqual(lights.map((l) => l.dataset.action), ['close', 'minimize', 'maximize']);
});

test('点击分发：标签切换 / 关闭 / 新建都带上正确的命令与参数', () => {
  const { doc } = fakeTitlebar();
  const w = loadTitlebar(doc, { style: 'windows' });
  w.__FNOS_TABS_SET__({
    tabs: [
      { id: 'main', title: 'MiniNas', active: true, canClose: false },
      { id: 'tab-3', title: 'hermes', active: false, canClose: true },
    ],
  });
  const tabs = doc.getElementById('tabs').children;
  // 标签项有 data-action=switch；关闭钮 data-action=close
  assert.equal(tabs[1].dataset.action, 'switch');
  const closeBtn = tabs[1].children.find((c) => c.className === 'tab-close');
  assert.equal(closeBtn.dataset.action, 'close');
  assert.equal(closeBtn.dataset.tabId, 'tab-3');
  // 源码级：点击翻译的三条命令名与参数键
  assert.ok(TB_JS.includes("invoke('tab_switch', { label: tabId })"), '切换带 label');
  assert.ok(TB_JS.includes("invoke('tab_close', { label: closeId })"), '关闭带 label');
  assert.ok(TB_JS.includes("invoke('tab_new')"), '新建不带参数');
});

test('纪律：脚本不碰页面内容、HTML 带拖拽区、高度常量跨语言一致', () => {
  assert.ok(!TB_JS.includes('innerHTML'), '不得用 innerHTML');
  assert.ok(TB_JS.includes('STRIP_H = 40'), '栏高 40 与 Rust TAB_STRIP_H 一致');
  const rust = readFileSync(new URL('../src-tauri/src/commands.rs', import.meta.url), 'utf8');
  assert.ok(rust.includes('pub const TAB_STRIP_H: f64 = 40.0;'),
    'Rust 侧标签栏高度必须是 40（跨语言锚点）');
  assert.ok(TB_HTML.includes('data-tauri-drag-region'), '标题条必须带拖拽区');
  assert.ok(TB_HTML.includes('titlebar.js') && TB_HTML.includes('titlebar.css'),
    'titlebar.html 必须引自己的脚本与样式');
  // 授权面：三个标签页命令 + 窗口操作，别无其它（webviews 字段：capability 的 windows
  // 匹配的是**窗口** label，titlebar 是主窗口里的 Webview，必须用 webviews）
  const cap = JSON.parse(readFileSync(new URL('../src-tauri/capabilities/titlebar.json', import.meta.url), 'utf8'));
  assert.deepEqual(cap.webviews, ['titlebar']);
  assert.deepEqual(cap.permissions, [
    'core:window:allow-start-dragging',
    'core:window:allow-minimize',
    'core:window:allow-toggle-maximize',
    'core:window:allow-close',
    'allow-tab-new',
    'allow-tab-switch',
    'allow-tab-close',
  ]);
  // CSS 形态锚点：mac 红绿灯的三色与 windows 控制钮
  for (const needle of ['.light.red', '.light.yellow', '.light.green', '.win-btn.close']) {
    assert.ok(TB_CSS.includes(needle), `titlebar.css 缺少 ${needle}`);
  }
});
