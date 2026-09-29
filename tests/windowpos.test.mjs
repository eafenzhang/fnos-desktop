// windowpos.js（T14c 修复轮 16）的行为契约：新窗口**同步**落位到内容区正中（不出现
// 「先以级联位置露一脸、再跳一下」）、最大化窗口不动、尺寸未就绪先按住、只动一次不再干预。
//
// 关键回归点（用户实测反馈）：落位必须发生在**观察回调返回之前**（= 插入与首帧之间），
// 所以断言一律在回调之后**不推进任何定时器**的情况下检查。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const POS = readFileSync(new URL('../src-tauri/inject/windowpos.js', import.meta.url), 'utf8');

const WINDOW_CLS = 'trim-ui__app-layout--window';

/** 假元素：几何、内联样式、选择器命中都可控。 */
function fakeEl(opts = {}) {
  const inline = {};
  const el = {
    nodeType: 1,
    _inline: inline,
    offsetWidth: opts.w ?? 1100,
    offsetHeight: opts.h ?? 640,
    isConnected: opts.connected !== false,
    parentElement: opts.parent || null,
    style: {
      get left() { return inline.left || ''; },
      set left(v) { inline.left = v; },
      get top() { return inline.top || ''; },
      set top(v) { inline.top = v; },
      get visibility() { return inline.visibility || ''; },
      set visibility(v) { inline.visibility = v; },
      removeProperty: (p) => { delete inline[p]; },
    },
    matches: (sel) => sel === `.${WINDOW_CLS}` && !!opts.isWindow,
    querySelectorAll: (sel) => (sel === `.${WINDOW_CLS}` ? (opts.windows || []) : []),
    _windows: opts.windows || [],
  };
  return el;
}

function fakeCtx(opts = {}) {
  const area = fakeEl({ isWindow: false, w: 0, h: 0 });
  area.clientWidth = opts.areaW ?? 1200;
  area.clientHeight = opts.areaH ?? 820;
  const doc = {
    readyState: opts.readyState || 'complete',
    _listeners: {},
    _windows: opts.windows || [],
    addEventListener(ev, fn) { this._listeners[ev] = fn; },
    querySelectorAll: (sel) => (sel === `.${WINDOW_CLS}` ? doc._windows : []),
  };
  class MOStub {
    constructor(cb) { doc._mo = cb; }
    observe() { doc._observed = true; }
  }
  const win = {
    document: doc,
    MutationObserver: MOStub,
    top: null,
    self: null,
    _timers: [],
    requestAnimationFrame(fn) { this._timers.push(fn); return this._timers.length; },
    setTimeout(fn) { this._timers.push(fn); return this._timers.length; },
    drain() { const fns = this._timers.slice(); this._timers.length = 0; fns.forEach((fn) => fn()); },
  };
  win.top = win;
  win.self = win;
  doc._win = win;
  return { doc, win, area };
}

function load(ctx) {
  const fn = new Function('window', POS + '\nreturn window;');
  return fn(ctx.win);
}

/** 造一个「刚插进内容区」的窗口元素。 */
function newWindow(ctx, opts = {}) {
  const el = fakeEl({ isWindow: true, parent: opts.parent || ctx.area, ...opts });
  return el;
}

test('落位是**同步**的：观察回调返回时窗口已经在正中（不推进任何定时器）', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  ctx.doc._mo([{ addedNodes: [win] }]);
  assert.equal(win._inline.left, '50px', '同步落位：插入与首帧之间就写好了（否则用户先看到级联位置）');
  assert.equal(win._inline.top, '90px', '垂直方向同理');
  assert.equal(win._inline.visibility, undefined, '尺寸量得到就不需要按住');
});

test('递归命中：整棵子树一次插入时，子树里的窗口也要同步落位', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  const host = fakeEl({ isWindow: false, windows: [win], parent: ctx.area });
  ctx.doc._mo([{ addedNodes: [host] }]);
  assert.equal(win._inline.left, '50px');
  assert.equal(win._inline.top, '90px');
});

test('最大化形态（铺满内容区）不动它', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx, { w: 1200, h: 820 });
  ctx.doc._mo([{ addedNodes: [win] }]);
  assert.equal(win._inline.left, undefined, '铺满内容区的窗口不得被改动');
  assert.equal(win._inline.top, undefined);
});

test('尺寸还没量到：先按住不给看（visibility:hidden），量到后摆正并放开', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx, { w: 0, h: 0 });
  ctx.doc._mo([{ addedNodes: [win] }]);
  assert.equal(win._inline.visibility, 'hidden', '量不到尺寸时必须先挡住（否则会以级联位置先露一脸）');
  assert.equal(win._inline.left, undefined, '还没摆正，不得写位置');
  assert.equal(ctx.win._timers.length, 2, '留了两次重试（下一帧 + 补正窗）');

  win.offsetWidth = 1100;
  win.offsetHeight = 640;
  ctx.win.drain();
  assert.equal(win._inline.left, '50px', '量到尺寸后必须摆正');
  assert.equal(win._inline.top, '90px');
  assert.equal(win._inline.visibility, undefined, '摆正后必须放开');
});

test('只动一次：用户拖动之后，后续 DOM 变更不得把窗口拉回来', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  ctx.doc._mo([{ addedNodes: [win] }]);
  ctx.win.drain(); // 两次幂等重试也跑完
  win._inline.left = '300px'; // 用户拖动
  win._inline.top = '220px';
  ctx.doc._mo([{ addedNodes: [fakeEl({ isWindow: false })] }]); // 页面上别的变更
  assert.equal(win._inline.left, '300px', '用户拖动优先：绝不再拉回居中');
  assert.equal(win._inline.top, '220px');
});

test('不做全量重扫：不相关的节点插入不会去动已有窗口', () => {
  const ctx = fakeCtx();
  const existing = newWindow(ctx);
  ctx.doc._windows = [existing]; // 页面加载时就存在的窗口（首次扫描会居中它）
  load(ctx);
  assert.equal(existing._inline.left, '50px', '首次扫描要居中已存在的窗口');
  existing._inline.left = '400px'; // 之后用户挪了它
  ctx.doc._mo([{ addedNodes: [fakeEl({ isWindow: false })] }]);
  assert.equal(existing._inline.left, '400px', '观察回调只处理新增节点，不得触发全量重扫');
});

test('纪律：非窗口元素零改动、不写页面内容、只在顶层文档工作', () => {
  const ctx = fakeCtx();
  load(ctx);
  const plain = fakeEl({ isWindow: false });
  ctx.doc._mo([{ addedNodes: [plain] }, { addedNodes: [null] }, { addedNodes: [] }]);
  assert.deepEqual(Object.keys(plain._inline), [], '非窗口元素不得被写样式');

  assert.ok(POS.includes('W.top === W.self'), '只在顶层文档工作');
  assert.ok(!POS.includes('innerHTML'), '不写页面内容');
  assert.ok(!POS.includes("createElement('div')"), '不往页面里掺本壳元素');
  // 子框架：什么都不做
  const sub = fakeCtx();
  sub.win.top = {}; // top !== self
  load(sub);
  assert.equal(sub.doc._observed, undefined, '子框架不得观察 DOM');
});
