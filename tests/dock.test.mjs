// dock.js（T14c）的行为契约：多策略定位（全锚定上游特征）+ 顶层文档守卫 + 空间回收 +
// 本壳命名空间 + 免刷新切换 + 显隐状态机。页面上**不得**出现任何本壳提示。
//
// dock.js 在 document-start 注入，逻辑全在 DOM 上（观察器 / 指针 / class / 内联样式 /
// 定时器），所以和 bootstrap.test.mjs 一样用**假 DOM**跑真实源码；选择器与纪律用源码级断言锁。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const DOCK = readFileSync(new URL('../src-tauri/inject/dock.js', import.meta.url), 'utf8');
const MOD = readFileSync(new URL('../src-tauri/assets/fnos-mods/mod.js', import.meta.url), 'utf8');

/** 上游 mod.js 的任务栏根/项/列表选择器：dock.js 必须逐字复用（上游类名策略的锚点）。 */
const UPSTREAM_ROOT = '.h-screen.fixed.left-0';
const UPSTREAM_LIST =
  '.scrollbar-hidden.absolute.inset-0.flex.flex-col.items-end.justify-start.gap-2.overflow-y-auto.pt-2';

/** 假元素：class、内联样式、几何、计算样式、父链都可控。 */
function fakeEl(opts = {}) {
  const classes = new Set();
  const inline = {}; // prop -> {value, priority}
  const w = opts.w ?? 68;
  const h = opts.h ?? 800;
  const left = opts.left ?? 0;
  const top = opts.top ?? 0;
  const el = {
    _classes: classes,
    _inline: inline,
    _computed: Object.assign({
      display: 'block', position: 'static',
      paddingLeft: '0px', marginLeft: '0px', gridTemplateColumns: 'none',
    }, opts.computed || {}),
    id: opts.id || '',
    classList: {
      add: (...cs) => cs.forEach((c) => classes.add(c)),
      remove: (...cs) => cs.forEach((c) => classes.delete(c)),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)),
      contains: (c) => classes.has(c),
    },
    style: {
      getPropertyValue: (p) => (inline[p] ? inline[p].value : ''),
      setProperty: (p, v, prio) => { inline[p] = { value: v, priority: prio || '' }; },
      removeProperty: (p) => { delete inline[p]; },
      get display() { return inline.display ? inline.display.value : ''; },
      set display(v) { if (v === '') delete inline.display; else inline.display = { value: v, priority: '' }; },
    },
    offsetWidth: w,
    offsetHeight: h,
    getBoundingClientRect: () => ({ left, top, right: left + w, bottom: top + h, width: w, height: h }),
    matches: (sel) => (sel === ':hover' ? !!opts.hovered : false),
    querySelector: (sel) => (sel === UPSTREAM_LIST && opts.hasList ? {} : null),
    querySelectorAll: (sel) => (sel === 'img,svg' ? { length: opts.icons ?? 0 } : []),
    parentElement: opts.parent || null,
    isConnected: true,
  };
  return el;
}

/** 假的任务栏图标项（S2 的爬升起点）：只需要几何与父链。 */
function fakeItem(w, h, parent) {
  return { offsetWidth: w, offsetHeight: h, parentElement: parent || null };
}

function fakeDom(opts = {}) {
  const body = opts.body || fakeEl({ w: 1200, h: 800, computed: { position: 'static' } });
  const doc = {
    _listeners: {},
    _styles: [],
    _candidates: [], // querySelectorAll('.h-screen.fixed.left-0') 的返回
    _items: [],      // querySelectorAll(上游任务栏项选择器) 的返回
    _fixed: [],      // querySelectorAll('.fixed') 的返回
    _all: [],        // querySelectorAll('body *') 的返回
    _queryAll: {},   // 其余选择器（机制②的窗口候选）由用例直接喂
    body,
    documentElement: fakeEl({ w: 1200, h: 800 }),
    adoptedStyleSheets: [],
    head: { appendChild: (n) => doc._styles.push(n) },
    createElement: (tag) => {
      const node = { tag, textContent: '', className: '', id: '', remove() {
        const i = doc._styles.indexOf(node);
        if (i >= 0) doc._styles.splice(i, 1);
      } };
      return node;
    },
    getElementById: (id) => doc._styles.find((n) => n.id === id) || null,
    querySelectorAll: (sel) => {
      if (sel === UPSTREAM_ROOT) return doc._candidates;
      if (sel === '.fixed') return doc._fixed;
      if (sel === 'body *') return doc._all;
      // 上游任务栏项选择器（含 w-[47px] 与 !border-l-[3px] 的转义，桩按特征识别）
      if (sel.indexOf('\\[47px\\]') >= 0 && sel.indexOf('border-l') >= 0) return doc._items;
      return doc._queryAll[sel] || [];
    },
    addEventListener(ev, fn) { this._listeners[ev] = fn; },
    removeEventListener(ev) { delete this._listeners[ev]; },
    readyState: 'loading',
  };
  class MOStub {
    constructor(cb) { doc._moCallback = cb; }
    observe(target, options) {
      doc._moObserved = { target, options };
      doc._moDisconnected = false; // 新实例是活的：与真实观察器同义
    }
    disconnect() { doc._moDisconnected = true; }
  }
  class CSSStyleSheetStub {
    constructor() { this.cssText = ''; this.__fnosDock = false; }
    // 真实语义：replaceSync 只写内容，**不会**自动加入 adoptedStyleSheets——
    // 加入靠调用方赋值（dock.js 的 concat）。桩推自会算两份。
    replaceSync(text) { this.cssText = text; }
  }
  const win = {
    document: doc,
    MutationObserver: MOStub,
    CSSStyleSheet: opts.noCSSOM ? undefined : CSSStyleSheetStub,
    innerWidth: opts.vw ?? 1200,
    innerHeight: opts.vh ?? 800,
    getComputedStyle: (el) => (el && el._computed) || null,
    _winListeners: {},
    _resized: 0,
    Event: function (type) { this.type = type; },
    dispatchEvent(e) { if (e && e.type === 'resize') this._resized += 1; return true; },
    addEventListener(ev, fn) { this._winListeners[ev] = fn; },
    removeEventListener(ev) { delete this._winListeners[ev]; },
    _timers: new Map(),
    setTimeout(fn) { const id = Math.random(); this._timers.set(id, fn); return id; },
    clearTimeout(id) { this._timers.delete(id); },
    // 排空全部待触发定时器（display:none 延迟 + idle 兜底都会用它；守卫内部自查状态）
    drainTimers() { const fns = [...this._timers.values()]; this._timers.clear(); fns.forEach((fn) => fn()); },
  };
  win.top = win;   // 默认当成顶层文档（与真实浏览器一致）
  win.self = win;
  doc._win = win;
  return doc;
}

/** 跑真实 dock.js；shell 形如注入载荷的 `shell` 段。 */
function load(doc, shell) {
  const win = doc._win;
  win.__FNOS_SHELL__ = { shell: shell || {} };
  const fn = new Function('window', DOCK + '\nreturn window;');
  return fn(win);
}

/** 让 observer 回调走完「合并窗口 → locate」全流程。 */
function settle(doc) {
  doc._moCallback();
  doc._win.drainTimers();
}

// ---------- 源码级：上游类名策略与纪律 ----------

test('DOCK_ROOT/ITEM/LIST 选择器逐字取自上游 mod.js（上游类名策略）', () => {
  const root = MOD.match(/const TASKBAR_ROOT_SELECTOR = '([^']+)';/);
  const item = MOD.match(/const TASKBAR_ITEM_SELECTOR =\s*'([^']+)';/);
  const list = MOD.match(/const TASKBAR_LIST_SELECTOR =\n?\s*'([^']+)';/);
  assert.ok(root, '上游 mod.js 必须仍有 TASKBAR_ROOT_SELECTOR 常量');
  assert.ok(item, '上游 mod.js 必须仍有 TASKBAR_ITEM_SELECTOR 常量');
  assert.ok(list, '上游 mod.js 必须仍有 TASKBAR_LIST_SELECTOR 常量');
  assert.ok(DOCK.includes(`var DOCK_ROOT_SELECTOR = '${root[1]}';`));
  assert.ok(DOCK.includes(`'${item[1]}'`), 'dock.js 必须逐字复用上游的任务栏项选择器（S2 爬升的锚点）');
  assert.ok(DOCK.includes(`'${list[1]}'`), 'dock.js 必须逐字复用上游的列表选择器（根节点验证）');
});

test('纪律：不开 IPC、不写 innerHTML、只在自己的 class 命名空间里动手，且页面上没有任何提示', () => {
  assert.ok(!DOCK.includes('invoke('), '不得给页面开命令授权');
  assert.ok(!DOCK.includes('innerHTML'), '只加 class / 注入自己的样式，绝不 innerHTML');
  for (const cls of ['fnos-shell-dock-autohide', 'fnos-shell-dock-hidden']) {
    assert.ok(DOCK.includes(cls), `必须有本壳命名空间的 class：${cls}`);
  }
  // T14c 修复轮 7：页面上不得再出现任何本壳提示/角标（用户要求）
  for (const gone of ['fnos-shell-dock-badge', 'function badge(', 'createElement(\'div\')', 'scheduleDiag', 'DIAG_DELAY_MS']) {
    assert.ok(!DOCK.includes(gone), `dock.js 不得再有页面提示机制：${gone}`);
  }
  // 诊断只剩控制台口（不向页面画东西）
  assert.ok(DOCK.includes('__FNOS_DOCK_STATE__'), '必须保留控制台诊断口');
});

// ---------- 顶层文档守卫 ----------

test('只在顶层文档工作：子框架（应用窗口 iframe）里什么都不做', () => {
  const doc = fakeDom();
  const win = doc._win;
  win.top = {}; // 子框架：top !== self
  load(doc, { dockAutoHide: true });
  assert.equal(doc._moObserved, undefined, '子框架不得观察 DOM');
  assert.equal(doc._listeners.pointermove, undefined, '子框架不得监听指针');
  assert.equal(doc.adoptedStyleSheets.length, 0, '子框架不得注入样式');
  assert.equal(typeof win.__FNOS_APPLY_SHELL__, 'undefined', '子框架不得安装 shell 钩子');
});

// ---------- 初始态：默认关 ----------

test('默认关：不装观察器、不监听指针、不注入样式，但 shell 钩子仍然在', () => {
  for (const shell of [undefined, {}, { dockAutoHide: false }, { dockAutoHide: 'yes' }]) {
    const doc = fakeDom();
    const w = load(doc, shell);
    assert.equal(doc._moObserved, undefined, '默认关不得观察 DOM');
    assert.equal(doc._listeners.pointermove, undefined, '默认关不得监听指针');
    assert.equal(doc.adoptedStyleSheets.length, 0, '默认关不得注入样式');
    assert.equal(typeof w.__FNOS_APPLY_SHELL__, 'function', '免刷新钩子必须始终在');
  }
});

// ---------- 定位策略 ----------

test('S1：上游根选择器命中（含列表优先、退化取第一个）', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const dock = fakeEl({ w: 68, h: 800, left: 0, hasList: true });
  doc._candidates = [dock];
  settle(doc);
  assert.ok(dock._classes.has('fnos-shell-dock-autohide'), 'S1 命中即接管');
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'), '接管即藏');
});

test('S2：上游根选择器落空时，从任务栏图标项向上爬到最外层 Dock 形状容器', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const pill = fakeEl({ w: 53, h: 218, left: 15, top: 300, icons: 5 });
  const item = fakeItem(47, 40, pill);
  doc._items = [item];
  settle(doc);
  assert.ok(pill._classes.has('fnos-shell-dock-autohide'), '必须接管 Dock 容器（不是单个图标项）');
  assert.ok(pill._classes.has('fnos-shell-dock-hidden'), '接管即藏');
});

test('S3：S1/S2 落空时，贴边 + Dock 形状 + 图标丰富的固定容器兜底（宽扁悬浮窗不算）', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const pill = fakeEl({ w: 53, h: 218, left: 15, top: 300, icons: 5 });
  doc._fixed = [pill];
  settle(doc);
  assert.ok(pill._classes.has('fnos-shell-dock-autohide'), '贴边固定容器必须被接管');

  const doc2 = fakeDom();
  load(doc2, { dockAutoHide: true });
  const widget = fakeEl({ w: 250, h: 55, left: 920, icons: 0 }); // 右上角资源监控
  doc2._fixed = [widget];
  settle(doc2);
  assert.equal(widget._classes.size, 0, '宽扁无图标的悬浮窗不是 Dock');
});

// ---------- 空间回收（T14c 修复轮 7） ----------

test('机制①：同级内容区的 pl-[66px]（fnOS 实测形态）→ 中和为 0', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  // 桌面根：flex 行，两个子节点 —— Dock（fixed，不占位）与内容区（pl-[66px] 预留）
  const root = fakeEl({ w: 1200, h: 800, left: 0 });
  const dock = fakeEl({ w: 66, h: 800, left: 0, top: 0, parent: root, hasList: true, icons: 5 });
  const content = fakeEl({
    w: 1200, h: 800, left: 0, parent: root,
    computed: { paddingLeft: '66px' },
  });
  root.children = [dock, content];
  doc._candidates = [dock];
  settle(doc);
  assert.equal(content.style.getPropertyValue('padding-left'), '0px',
    '内容区的预留内边距必须归零（桌面图标与流内内容都在它里面）');
  assert.equal(content._inline['padding-left'].priority, 'important', '必须带 !important（压过 Tailwind 类）');
  assert.equal(dock._classes.has('fnos-shell-dock-autohide'), true, 'Dock 本体照常接管');
});

test('机制①：预留写在祖先链上时同样中和；子节点深处也能找到', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const desktop = fakeEl({
    w: 1200, h: 800, left: 0,
    computed: { display: 'grid', gridTemplateColumns: '66px 1134px', paddingLeft: '66px' },
  });
  const dock = fakeEl({ w: 66, h: 800, left: 0, top: 0, parent: desktop, hasList: true, icons: 5 });
  doc._candidates = [dock];
  settle(doc);
  assert.equal(desktop.style.getPropertyValue('padding-left'), '0px', '祖先的 padding-left 必须归零');
  assert.equal(desktop.style.getPropertyValue('grid-template-columns'), '0px 1134px',
    'grid 第一轨必须归零（其余轨保持原样）');
});

test('机制①的闸门：非满尺寸容器、过小/过大的预留一律不动手', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const root = fakeEl({ w: 1200, h: 800, left: 0 });
  const dock = fakeEl({ w: 66, h: 800, left: 0, top: 0, parent: root, hasList: true, icons: 5 });
  const small = fakeEl({ w: 400, h: 300, left: 0, parent: root, computed: { paddingLeft: '66px' } });
  const tiny = fakeEl({ w: 1200, h: 800, left: 0, parent: root, computed: { paddingLeft: '8px' } });
  const huge = fakeEl({ w: 1200, h: 800, left: 0, parent: root, computed: { paddingLeft: '320px' } });
  root.children = [dock, small, tiny, huge];
  doc._candidates = [dock];
  settle(doc);
  assert.equal(small.style.getPropertyValue('padding-left'), '', '非满尺寸容器不是内容区，不得动');
  assert.equal(tiny.style.getPropertyValue('padding-left'), '', '过小的内边距不是 Dock 预留');
  assert.equal(huge.style.getPropertyValue('padding-left'), '', '过大的内边距不是 Dock 预留');
});

test('机制②：窗口的 !left-[66px] / !right-0 / !w-[calc(100%-66px)] → left/width 被拉回满宽', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const dock = fakeEl({ w: 66, h: 800, left: 0, top: 0, hasList: true, icons: 5 });
  doc._candidates = [dock];
  const win = fakeEl({ w: 1134, h: 800, left: 66, top: 0 });
  win.className = 'trim-ui__app-layout--window !inset-y-0 !left-[66px] !right-0 !w-[calc(100%-66px)]';
  doc._queryAll['[class*="app-layout--window"]'] = [win];
  settle(doc);
  assert.equal(win.style.getPropertyValue('left'), '0px', '窗口左偏移必须归零');
  assert.equal(win.style.getPropertyValue('width'), '100%', '窗口宽度必须补回满宽');
  assert.equal(win._inline.left.priority, 'important', '必须带 !important（压过 Tailwind 类）');
});

test('机制②：签名不完整/数字不一致时不动手；签名消失后覆盖被撤回（用户改回自由尺寸）', () => {
  const doc = fakeDom();
  const w = load(doc, { dockAutoHide: true });
  const dock = fakeEl({ w: 66, h: 800, left: 0, top: 0, hasList: true, icons: 5 });
  doc._candidates = [dock];
  // ① 缺 !right-0 → 不是最大化签名
  const partial = fakeEl({ w: 1134, h: 800, left: 66 });
  partial.className = 'trim-ui__app-layout--window !left-[66px] !w-[calc(100%-66px)]';
  // ② 三处数字不一致 → 不动手
  const mixed = fakeEl({ w: 1134, h: 800, left: 66 });
  mixed.className = 'trim-ui__app-layout--window !left-[66px] !right-0 !w-[calc(100%-80px)]';
  // ③ 正常签名 → 覆盖
  const win = fakeEl({ w: 1134, h: 800, left: 66 });
  win.className = 'trim-ui__app-layout--window !left-[66px] !right-0 !w-[calc(100%-66px)]';
  doc._queryAll['[class*="app-layout--window"]'] = [partial, mixed, win];
  settle(doc);
  assert.equal(partial.style.getPropertyValue('left'), '', '缺 !right-0 不得动手');
  assert.equal(mixed.style.getPropertyValue('width'), '', '三处数字不一致不得动手');
  assert.equal(win.style.getPropertyValue('left'), '0px', '完整签名必须覆盖');

  // 用户把窗口改回自由尺寸：类消失 → 下一轮把定位权还给窗口管理器
  win.className = 'trim-ui__app-layout--window';
  win._inline.left = { value: '240px', priority: '' }; // 窗口管理器随后写入的自由位置
  doc._moCallback();
  doc._win.drainTimers();
  assert.ok(!win.style.getPropertyValue('left') || win.style.getPropertyValue('left') === '240px',
    '签名消失后本壳必须撤回覆盖（定位权还给窗口管理器）');
  assert.equal(win.style.getPropertyValue('width'), '', 'width 覆盖同样撤回');
  assert.equal(w.__FNOS_DOCK_STATE__().windowsFixed, 0, '诊断口必须如实');
});

test('免刷新关闭：接管、内边距中和与窗口覆盖全部还原（含还原原本就存在的内联值）', () => {
  const doc = fakeDom();
  const w = load(doc, { dockAutoHide: true });
  const root = fakeEl({ w: 1200, h: 800, left: 0 });
  const dock = fakeEl({ w: 66, h: 800, left: 0, top: 0, parent: root, hasList: true, icons: 5 });
  const content = fakeEl({ w: 1200, h: 800, left: 0, parent: root, computed: { paddingLeft: '66px' } });
  content._inline['padding-left'] = { value: '40px', priority: '' }; // 上游原本就有内联值
  const win = fakeEl({ w: 1134, h: 800, left: 66 });
  win.className = 'trim-ui__app-layout--window !left-[66px] !right-0 !w-[calc(100%-66px)]';
  root.children = [dock, content];
  doc._candidates = [dock];
  doc._queryAll['[class*="app-layout--window"]'] = [win];
  settle(doc);
  assert.equal(content.style.getPropertyValue('padding-left'), '0px', '先中和');
  assert.equal(win.style.getPropertyValue('left'), '0px', '先覆盖');

  w.__FNOS_APPLY_SHELL__({ dockAutoHide: false });
  assert.equal(content.style.getPropertyValue('padding-left'), '40px', '必须还原上游原本的内联值');
  assert.equal(win.style.getPropertyValue('left'), '', '窗口覆盖必须撤回');
  assert.equal(win.style.getPropertyValue('width'), '', '窗口宽覆盖必须撤回');
  assert.equal(dock._classes.size, 0, 'Dock 的 class 必须清干净');
  assert.equal(dock.style.display, '', 'display 必须还原');
  assert.ok(doc._moDisconnected, '观察器必须断开');
  assert.deepEqual(w.__FNOS_DOCK_STATE__(), {
    enabled: false, found: false, targets: 0, axis: 'x', edgeMin: true, shown: false,
    reclaimed: [], windowsFixed: 0, lastMiss: '',
  }, '诊断口必须如实');
});

// ---------- 显隐状态机 ----------

test('开启：观察 document、接管即藏且到点真正离场（display:none）+ 派发 resize', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  assert.deepEqual(
    doc._moObserved && { target: doc._moObserved.target, subtree: doc._moObserved.options.subtree },
    { target: doc, subtree: true },
    '观察目标必须是 document（document-start 时 body 还不存在）'
  );
  assert.equal(typeof doc._listeners.pointermove, 'function');

  const dock = fakeEl({ w: 68, h: 800, left: 0, top: 0, hasList: true, icons: 5 });
  doc._candidates = [dock];
  doc._moCallback();
  doc._win.drainTimers(); // 第一轮排空：locate 执行（接管即藏）
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'), '接管即藏（自动隐藏的语义就是平时藏着）');
  assert.equal(dock.style.getPropertyValue('--fnos-dock-hide-tf'), 'translateX(-105%)', '贴左缘 → 向左滑出');
  assert.equal(dock.style.display, '', '滑出动画期间还在布局里');
  doc._win.drainTimers(); // 第二轮排空：display:none 的延迟到点
  assert.equal(dock.style.display, 'none', '动画结束后真正 display:none');
  assert.ok(doc._win._resized >= 1, 'Dock 离场必须派发 resize（窗口管理器借它重排）');

  doc._listeners.pointermove({ clientX: 3, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'), '边缘热区必须唤回');
  assert.equal(dock.style.display, '', '唤回必须先恢复布局');
});

test('迟滞唤出：藏在 Dock 脚印下的应用按钮可以直接点（只有顶到边缘才唤出）', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const dock = fakeEl({ w: 68, h: 800, left: 0, top: 0, hasList: true, icons: 5 });
  doc._candidates = [dock];
  settle(doc);
  doc._win.drainTimers(); // 已 display:none

  doc._listeners.pointermove({ clientX: 30, clientY: 400 });
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'), '脚印内不唤出');

  doc._listeners.pointermove({ clientX: 3, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'), '边缘热区唤出');
  doc._listeners.pointermove({ clientX: 40, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'), '脚印内保持显示');
  doc._listeners.pointermove({ clientX: 600, clientY: 400 });
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'), '离开脚印即藏');
});

test('无操作兜底：3 秒没有任何指针事件就藏（iframe 吞事件时只有它能救）', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const dock = fakeEl({ w: 68, h: 800, left: 0, top: 0, hasList: true, icons: 5 });
  doc._candidates = [dock];
  settle(doc);

  doc._listeners.pointermove({ clientX: 3, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'));
  doc._win.drainTimers();
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'), '无操作必须自动隐藏');
  doc._win.drainTimers();
  assert.equal(dock.style.display, 'none');

  // 连续指针事件重置 idle（不得累积多个兜底定时器）
  doc._listeners.pointermove({ clientX: 3, clientY: 400 });
  const pendingBefore = doc._win._timers.size;
  doc._listeners.pointermove({ clientX: 4, clientY: 400 });
  assert.ok(doc._win._timers.size <= pendingBefore, '未离开引脚时不得新增兜底定时器');
});

test('指针离开窗口 / 窗口失焦：立即藏（auto-hide 的通用语义）', () => {
  const doc = fakeDom();
  const win = doc._win;
  load(doc, { dockAutoHide: true });
  const dock = fakeEl({ w: 68, h: 800, left: 0, top: 0, hasList: true, icons: 5 });
  doc._candidates = [dock];
  settle(doc);
  doc._listeners.pointermove({ clientX: 3, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'));

  doc._listeners.pointerleave();
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'), 'pointerleave 必须藏');
  doc._listeners.pointermove({ clientX: 3, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'));
  win._winListeners.blur();
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'), '失焦必须藏');
});

test('贴右缘的 Dock 向右滑出，热区在右缘（不硬编码左缘，也不做空间回收）', () => {
  const doc = fakeDom({ vw: 1200 });
  load(doc, { dockAutoHide: true });
  const holder = fakeEl({ w: 1200, h: 800, left: 0, computed: { paddingLeft: '68px' } });
  const dock = fakeEl({ w: 68, h: 800, left: 1132, top: 300, parent: holder, hasList: true, icons: 5 });
  doc._candidates = [dock];
  settle(doc);
  assert.equal(dock.style.getPropertyValue('--fnos-dock-hide-tf'), 'translateX(105%)');
  assert.equal(holder.style.getPropertyValue('padding-left'), '', '贴右缘时不得改左内边距');
  doc._listeners.pointermove({ clientX: 600, clientY: 400 });
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'));
  doc._listeners.pointermove({ clientX: 1196, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'), '右缘热区必须唤回');
});

test('零尺寸 rect（SPA 未布局）不改写贴边结论', () => {
  const doc = fakeDom({ vw: 1200 });
  load(doc, { dockAutoHide: true });
  const dock = fakeEl({ w: 68, h: 800, left: 1132, top: 300, hasList: true, icons: 5 });
  doc._candidates = [dock];
  settle(doc);
  assert.equal(dock.style.getPropertyValue('--fnos-dock-hide-tf'), 'translateX(105%)', '先量到有效几何 = 右缘');
  dock.getBoundingClientRect = () => ({ left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 });
  doc._listeners.pointermove({ clientX: 600, clientY: 400 }); // 远处 → 藏（藏前重测）
  assert.equal(dock.style.getPropertyValue('--fnos-dock-hide-tf'), 'translateX(105%)', '零 rect 沿用右缘结论');
});

test('Dock 被 SPA 换掉时重新接管：旧元素与旧回收完全还原，新元素接管', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const holder = fakeEl({ w: 1200, h: 800, left: 0, computed: { paddingLeft: '68px' } });
  const first = fakeEl({ w: 68, h: 800, left: 0, top: 0, parent: holder, hasList: true, icons: 5 });
  doc._candidates = [first];
  settle(doc);
  assert.equal(holder.style.getPropertyValue('padding-left'), '0px');

  const second = fakeEl({ w: 68, h: 800, left: 0, top: 0, hasList: true, icons: 5 });
  doc._candidates = [second];
  settle(doc);
  assert.ok(!first._classes.has('fnos-shell-dock-autohide') && !first._classes.has('fnos-shell-dock-hidden'),
    '被换掉的旧元素必须清掉本壳加的 class（不留下孤儿状态）');
  assert.equal(first.style.display, '', '被换掉的旧元素不得残留 display:none');
  assert.equal(holder.style.getPropertyValue('padding-left'), '', '旧的一轮回收必须还原（避免叠加）');
  assert.ok(second._classes.has('fnos-shell-dock-autohide'));
  assert.ok(second._classes.has('fnos-shell-dock-hidden'), '新元素接管即藏');
});

// ---------- 免刷新切换的形状闸 ----------

test('免刷新切换的形状闸：非 boolean 不动手、同值幂等、非法形状不炸', () => {
  const doc = fakeDom();
  const w = load(doc, {});
  w.__FNOS_APPLY_SHELL__({ dockAutoHide: 'true' });
  assert.equal(doc._moObserved, undefined, '非 boolean 不得开启');
  w.__FNOS_APPLY_SHELL__({ dockAutoHide: true });
  const observed = doc._moObserved;
  w.__FNOS_APPLY_SHELL__({ dockAutoHide: true });
  assert.equal(doc._moObserved, observed, '同值幂等：观察器不得重复安装');
  assert.ok(!doc._moDisconnected, '同值幂等：不得误断观察器');
  w.__FNOS_APPLY_SHELL__(null);
  w.__FNOS_APPLY_SHELL__({});
  w.__FNOS_APPLY_SHELL__(42);
  assert.ok(doc._moObserved, '非法形状不得拆除已开启的功能');
});

// ---------- 样式注入的 CSP 兜底 ----------

test('样式注入：优先可构造样式表，不可用时落到 <style>（两路内容一致）', () => {
  const withCSSOM = fakeDom();
  load(withCSSOM, { dockAutoHide: true });
  assert.equal(withCSSOM.adoptedStyleSheets.length, 1, '有 CSSOM 时必须用它（CSP 免疫）');
  assert.match(withCSSOM.adoptedStyleSheets[0].cssText, /fnos-shell-dock-hidden/);

  const without = fakeDom({ noCSSOM: true });
  load(without, { dockAutoHide: true });
  assert.equal(without._styles.length, 1, '没有 CSSOM 时必须落到 <style>');
  assert.match(without._styles[0].textContent, /fnos-shell-dock-hidden/);
  assert.equal(without._styles[0].id, 'fnos-shell-dock-style');
});
