// titlebar.js（T14d 修复轮 3）的行为契约：**标签页合并到标题栏**——一行之内左边标签、
// 右边 Windows 系统默认观感的 ─ □ ×；状态由宿主单向推送（__FNOS_TABS_SET__），渲染幂等；
// 点击按 data-action 翻译成 invoke；fnOS 主页标签无关闭按钮。
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

test('纪律：固定系统默认观感（无 mac 分支）、不碰页面内容、高度跨语言一致', () => {
  // 用户要求：最大化/最小化/关闭固定系统默认样式；上游 mod 的 mac/windows 标题栏样式
  // 只管桌面内置应用窗口，不影响这条栏
  for (const gone of ['traffic', 'light red', "style === 'mac'", '__FNOS_TABS_BOOT__']) {
    assert.ok(!TB_JS.includes(gone), `titlebar.js 不得再有样式分支：${gone}`);
  }
  assert.ok(!TB_CSS.includes('.traffic'), 'titlebar.css 不得有红绿灯样式');
  assert.ok(!TB_JS.includes('innerHTML'), '不得用 innerHTML');
  assert.ok(TB_JS.includes('STRIP_H = 40'), '栏高 40 与 Rust TAB_STRIP_H 一致');
  // 系统标题栏一致性（修复轮 4）：图标必须走系统字体栈，按钮 46px 整高
  assert.ok(TB_CSS.includes('"Segoe Fluent Icons"') && TB_CSS.includes('"Segoe MDL2 Assets"'),
    '窗口按钮必须用系统图标字体（与 Windows 标题栏同款字形）');
  assert.ok(TB_CSS.includes('width: 46px'), '按钮宽度必须与系统标题栏一致（46px）');
  const rust = readFileSync(new URL('../src-tauri/src/commands.rs', import.meta.url), 'utf8');
  assert.ok(rust.includes('pub const TAB_STRIP_H: f64 = 40.0;'),
    'Rust 侧标签栏高度必须是 40（跨语言锚点）');
  // 无边框（标签页合并到标题栏 = 一行式自绘）
  assert.ok(rust.includes('.decorations(false)'), '主窗口必须无边框（合并式标题栏）');
  assert.ok(TB_HTML.includes('data-tauri-drag-region'), '标题条必须带拖拽区');
  assert.ok(TB_HTML.includes('titlebar.js') && TB_HTML.includes('titlebar.css'),
    'titlebar.html 必须引自己的脚本与样式');
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
