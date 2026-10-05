// titlebar.js（T14d 修复轮 5）的行为契约：**标签页合并到标题栏**——一行之内最左品牌图标
// （点击回到桌面标签）、然后标签、右边 Windows 系统默认观感的 ─ □ ×；状态由宿主单向
// 推送（__FNOS_TABS_SET__ / __FNOS_WIN_MAX_SET__），渲染幂等；点击按 data-action 翻译
// 成 invoke；fnOS 主页标签无关闭按钮。
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
    attributes: {},
    appendChild(child) { this.children.push(child); },
    setAttribute(k, v) { this.attributes[k] = v; },
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

function loadTitlebar(doc) {
  const win = {
    document: doc,
    _invokes: [],
    __TAURI_INTERNALS__: {
      invoke(cmd, args) { win._invokes.push([cmd, args]); },
    },
  };
  win.window = win;
  const fn = new Function('window', TB_JS + '\nreturn window;');
  return fn(win);
}

test('渲染：main 合并为最左「桌面」元素（品牌图标 + NAS 名称），外部标签随后', () => {
  const { doc } = fakeTitlebar();
  const w = loadTitlebar(doc);
  assert.equal(typeof w.__FNOS_TABS_SET__, 'function', '推送口必须在（宿主 eval 调它）');
  w.__FNOS_TABS_SET__({
    tabs: [
      { id: 'main', title: 'MiniNas - 飞牛 fnOS', active: true, canClose: false },
      { id: 'tab-1', title: 'hermes-studio', active: false, canClose: true },
    ],
  });
  const kids = doc.getElementById('tabs').children;
  assert.equal(kids.length, 2, 'main 合并成桌面元素 + 1 个外部标签');
  const home = kids[0];
  assert.ok(home.className.includes('home') && home.className.includes('active'), '桌面元素在桌面标签激活时高亮');
  assert.equal(home.dataset.tabId, 'main');
  assert.equal(home.dataset.action, 'switch', '点击桌面元素 = 切回桌面标签');
  const img = home.children.find((c) => c.tagName === 'IMG');
  assert.ok(img && img.src.includes('icons/icon16.png'), '桌面元素带 fnOS 品牌图标');
  const name = home.children.find((c) => c.className === 'home-name');
  assert.equal(name.textContent, 'MiniNas', 'NAS 名称 = 页面标题剥掉「- 飞牛 fnOS」后缀');
  assert.ok(!home.children.some((c) => c.className === 'tab-close'), '桌面元素不可关闭');
  // 外部标签照旧渲染在桌面元素之后
  const tab = kids[1];
  assert.equal(tab.dataset.tabId, 'tab-1');
  assert.ok(tab.className.includes('tab') && !tab.className.includes('active'));
  assert.ok(tab.children.some((c) => c.className === 'tab-close'));
  // 再推一次：幂等（不叠加）
  w.__FNOS_TABS_SET__({
    tabs: [
      { id: 'main', title: 'MiniNas - 飞牛 fnOS', active: true, canClose: false },
      { id: 'tab-1', title: 'hermes-studio', active: false, canClose: true },
    ],
  });
  assert.equal(doc.getElementById('tabs').children.length, 2);
});

test('nasName：剥「- 飞牛 fnOS」后缀；剥不出原样；空回退 fnOS', () => {
  const { doc } = fakeTitlebar();
  const w = loadTitlebar(doc);
  const nasName = w.__FNOS_TITLEBAR__.nasName;
  assert.equal(nasName('MiniNas - 飞牛 fnOS'), 'MiniNas');
  assert.equal(nasName('FN Connect 远程访问 - 飞牛 fnOS'), 'FN Connect 远程访问');
  assert.equal(nasName('MiniNas'), 'MiniNas', '没有后缀就原样用');
  assert.equal(nasName(''), 'fnOS', '空标题回退 fnOS');
});

test('品牌图标：点击回桌面标签（main），不新开也不重载', () => {
  // HTML 里不再有独立 brand 按钮——桌面元素由 render 从 main 标签合并生成
  assert.ok(!TB_HTML.includes('id="brand"'), '不得再有独立品牌按钮（已与 main 合并）');
  assert.ok(TB_JS.includes("icons/icon16.png"), '品牌图标用 frontendDist 里的 16px fnOS 图标');
  assert.ok(TB_JS.includes("action === 'home'") && TB_JS.includes("invoke('tab_switch', { label: 'main' })"),
    'home 动作必须切回 main 标签（保留通用动作分支）');
});

test('窗口控制：系统图标字体的 ─ □ ×（46px 整高），点击翻译成窗口命令', () => {
  const { doc } = fakeTitlebar();
  const w = loadTitlebar(doc);
  w.__FNOS_TITLEBAR__.renderControls();
  const controls = doc.getElementById('controls').children;
  assert.deepEqual(controls.map((b) => b.dataset.action), ['minimize', 'maximize', 'close']);
  assert.equal(controls[0].className, 'win-btn');
  assert.equal(controls[1].className, 'win-btn maximize');
  assert.equal(controls[2].className, 'win-btn close', '关闭钮必须带 close class（悬停红）');
  // 系统标题栏同款字形：Segoe 图标字体的私用区码点（不是文本字符 ─□×）
  assert.equal(controls[0].textContent.codePointAt(0), 0xe921, '最小化 = U+E921');
  assert.equal(controls[1].textContent.codePointAt(0), 0xe922, '最大化 = U+E922');
  assert.equal(controls[2].textContent.codePointAt(0), 0xe8bb, '关闭 = U+E8BB');
  // 最大化 ↔ 还原字形切换（宿主 __FNOS_WIN_MAX_SET__ 推送状态）
  w.__FNOS_TITLEBAR__.setMaxState(true);
  assert.equal(controls[1].textContent.codePointAt(0), 0xe923, '最大化状态下按钮 = U+E923（还原）');
  assert.equal(controls[1].title, '还原');
  w.__FNOS_WIN_MAX_SET__(false);
  assert.equal(controls[1].textContent.codePointAt(0), 0xe922, '还原后回到 U+E922');
  // 源码级：三条窗口命令
  for (const cmd of ['plugin:window|minimize', 'plugin:window|toggle_maximize', 'plugin:window|close']) {
    assert.ok(TB_JS.includes(`invoke('${cmd}')`), `缺少窗口命令：${cmd}`);
  }
});

test('点击分发：标签切换 / 关闭 / 新建都带上正确的命令与参数', () => {
  const { doc } = fakeTitlebar();
  const w = loadTitlebar(doc);
  w.__FNOS_TABS_SET__({
    tabs: [
      { id: 'main', title: 'MiniNas', active: true, canClose: false },
      { id: 'tab-3', title: 'hermes', active: false, canClose: true },
    ],
  });
  const tabs = doc.getElementById('tabs').children;
  assert.equal(tabs[1].dataset.action, 'switch');
  const closeBtn = tabs[1].children.find((c) => c.className === 'tab-close');
  assert.equal(closeBtn.dataset.action, 'close');
  assert.equal(closeBtn.dataset.tabId, 'tab-3');
  // 源码级：点击翻译的命令名与参数键
  assert.ok(TB_JS.includes("invoke('tab_switch', { label: tabId })"), '切换带 label');
  assert.ok(TB_JS.includes("invoke('tab_close', { label: el.dataset.tabId })"),
    '关闭带 label（关的是那个标签）');
  assert.ok(TB_JS.includes("invoke('tab_new')"), '新建不带参数');
});

test('纪律：不碰页面内容、高度 32 跨语言一致、授权面精确', () => {
  assert.ok(!TB_JS.includes('innerHTML'), '不得用 innerHTML');
  assert.ok(TB_JS.includes('STRIP_H = 32'), '栏高 32 与 Rust TAB_STRIP_H 一致');
  // 系统标题栏一致性（修复轮 4）：图标必须走系统字体栈，按钮 46px 整高
  assert.ok(TB_CSS.includes('"Segoe Fluent Icons"') && TB_CSS.includes('"Segoe MDL2 Assets"'),
    '窗口按钮必须用系统图标字体（与 Windows 标题栏同款字形）');
  assert.ok(TB_CSS.includes('width: 46px'), '按钮宽度必须与系统标题栏一致（46px）');
  // 授权面：三个标签页命令 + 窗口控制 + 拖拽，别无其它
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
  assert.ok(!cap.permissions.some((p) => p.includes('get-config') || p.includes('open-url')),
    '标签页栏不得得到任何应用命令授权');
});
