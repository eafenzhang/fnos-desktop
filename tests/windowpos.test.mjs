// windowpos.js（T14c 修复轮 16～17）的行为契约：新窗口**同步**落位到内容区正中（不出现
// 「先以级联位置露一脸、再跳一下」）、最大化往返后重新居中、窗口形态下的拖动/缩放不被干扰、
// 尺寸未就绪先按住、只动一次不再干预。
//
// 关键回归点（用户实测反馈）：
// · 落位必须发生在**观察回调返回之前**（= 插入与首帧之间），所以断言一律在回调之后
//   **不推进任何定时器**的情况下检查；
// · 「窗口 → 最大化 → 窗口」往返后必须仍然居中（fnOS 只会把最大化前的偏心位置还回来）。
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
    closest: (sel) => (sel === `.${WINDOW_CLS}` && !!opts.isWindow ? el : null),
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
    _listeners: {},   // ev -> [fn]（pointerdown/up 等会注册多个）
    _windows: opts.windows || [],
    _mos: [],   // 全部观察器实例（document 级的 + 逐窗口的）
    addEventListener(ev, fn) { (this._listeners[ev] ||= []).push(fn); },
    querySelectorAll: (sel) => (sel === `.${WINDOW_CLS}` ? doc._windows : []),
  };
  class MOStub {
    constructor(cb) { this.cb = cb; doc._mos.push(this); }
    observe(target, options) {
      this.target = target;
      this.options = options;
      doc._observed = { target, options };
    }
    disconnect() { this.disconnected = true; }
  }
  const win = {
    document: doc,
    MutationObserver: MOStub,
    top: null,
    self: null,
    innerWidth: opts.vw ?? 1200,
    innerHeight: opts.vh ?? 820,
    _timers: [],
    _resizeListeners: [],
    addEventListener(ev, fn) { if (ev === 'resize') this._resizeListeners.push(fn); },
    requestAnimationFrame(fn) { this._timers.push(fn); return this._timers.length; },
    setTimeout(fn) { this._timers.push(fn); return this._timers.length; },
    clearTimeout() { /* 桩：定时器一并在 drain 时执行 */ },
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
  return fakeEl({ isWindow: true, parent: opts.parent || ctx.area, ...opts });
}

/** document 级观察器（插入节点的入口）。 */
function fireDoc(ctx, records) {
  const mo = ctx.doc._mos.find((m) => m.target === ctx.doc);
  assert.ok(mo, '必须有 document 级观察器');
  mo.cb(records);
}

/** 逐窗口的属性观察器（class/style 变更 = 最大化形态切换）。 */
function fireEl(ctx, el) {
  const mos = ctx.doc._mos.filter((m) => m.target === el);
  assert.ok(mos.length > 0, '窗口必须被挂上属性观察器（最大化往返要靠它）');
  mos.forEach((m) => m.cb([{ type: 'attributes', target: el }]));
}

/** 切到最大化形态：铺满内容区。 */
function maximize(ctx, el) {
  el.offsetWidth = ctx.area.clientWidth;
  el.offsetHeight = ctx.area.clientHeight;
  fireEl(ctx, el);
}

/** 从最大化还原成给定的窗口尺寸。 */
function unmaximize(ctx, el, w = 1100, h = 640) {
  el.offsetWidth = w;
  el.offsetHeight = h;
  fireEl(ctx, el);
}

/** 按下 / 松开指针（模拟用户拖窗口：windowpos 靠它区分「用户拖动」与「fnOS 重排」）。 */
function pointerDown(ctx, el) {
  (ctx.doc._listeners.pointerdown || []).forEach((fn) => fn({ target: el }));
}
function pointerUp(ctx) {
  for (const ev of ['pointerup', 'pointercancel']) {
    (ctx.doc._listeners[ev] || []).forEach((fn) => fn());
  }
}

/** 改内容区尺寸 = 外壳程序窗口在 Windows 上最大化/还原（视口尺寸一起变）。 */
function setArea(ctx, w, h) {
  ctx.area.clientWidth = w;
  ctx.area.clientHeight = h;
  ctx.win.innerWidth = w;
  ctx.win.innerHeight = h;
  (ctx.doc._win._resizeListeners || []).forEach((fn) => fn());
  ctx.win.drain(); // rAF 那一趟 + 补正那一趟
}

test('落位是**同步**的：观察回调返回时窗口已经在正中（不推进任何定时器）', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  assert.equal(win._inline.left, '50px', '同步落位：插入与首帧之间就写好了（否则用户先看到级联位置）');
  assert.equal(win._inline.top, '90px', '垂直方向同理');
  assert.equal(win._inline.visibility, undefined, '尺寸量得到就不需要按住');
});

test('递归命中：整棵子树一次插入时，子树里的窗口也要同步落位', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  const host = fakeEl({ isWindow: false, windows: [win], parent: ctx.area });
  fireDoc(ctx, [{ addedNodes: [host] }]);
  assert.equal(win._inline.left, '50px');
  assert.equal(win._inline.top, '90px');
});

test('最大化形态（铺满内容区）不动它', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx, { w: 1200, h: 820 });
  fireDoc(ctx, [{ addedNodes: [win] }]);
  assert.equal(win._inline.left, undefined, '铺满内容区的窗口不得被改动');
  assert.equal(win._inline.top, undefined);
});

test('尺寸还没量到：先按住不给看（visibility:hidden），量到后摆正并放开', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx, { w: 0, h: 0 });
  fireDoc(ctx, [{ addedNodes: [win] }]);
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

test('用户拖动（指针按在窗口上）写下的位置被记为意图，之后不被拉回', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  ctx.win.drain(); // 两次幂等重试跑完
  pointerDown(ctx, win);
  win._inline.left = '300px'; // 用户按住标题栏拖动
  win._inline.top = '220px';
  fireEl(ctx, win);
  pointerUp(ctx);
  assert.equal(win._inline.left, '300px', '拖动中的写入必须保留');
  fireEl(ctx, win); // 指针松开后的重复通知（值没变）也不得拉回
  assert.equal(win._inline.left, '300px', '用户摆的位置优先：绝不再拉回居中');
  fireDoc(ctx, [{ addedNodes: [fakeEl({ isWindow: false })] }]); // 页面上别的变更
  assert.equal(win._inline.top, '220px');
});

test('fnOS 自己重排（非用户写入）：当场按比例纠正，居中的窗口回正中', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  ctx.win.drain();
  win._inline.left = '165px'; // 实测：fnOS 重排把 (50,90) 改写成 (165,202)
  win._inline.top = '202px';
  fireEl(ctx, win);
  assert.equal(win._inline.left, '50px', '非用户写入必须被纠正（微任务里完成，画不出中间帧）');
  assert.equal(win._inline.top, '90px');
});

test('窗口 → 最大化 → 窗口：还原时重新居中（用户要求的往返语义）', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  // 用户先把窗口拖到偏心位置（比如为了看后面的内容）
  win._inline.left = '300px';
  win._inline.top = '220px';
  maximize(ctx, win);
  assert.equal(win._inline.left, '300px', '最大化形态下不得动位置（该形态由样式覆盖处理）');
  unmaximize(ctx, win);
  assert.equal(win._inline.left, '50px', '还原后必须回到居中');
  assert.equal(win._inline.top, '90px');
});

test('窗口形态下缩放（不构成形态切换）：按已记的相对位置重摆，不回正中也不漂移', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  // 用户先拖到 300,220（按住指针 → 记为意图）
  pointerDown(ctx, win);
  win._inline.left = '300px';
  win._inline.top = '220px';
  fireEl(ctx, win);
  pointerUp(ctx);
  // 再缩放：只改尺寸不改位置（无指针的样式变更）
  win.offsetWidth = 900;
  win.offsetHeight = 500;
  fireEl(ctx, win);
  // 比例 ≈ 0.4167/0.3171 → 新尺寸下 left = 600+0.4167×600-450 = 400, top = 410+130-250 = 290
  assert.equal(win._inline.left, '400px', '按已记的相对位置重摆（不回正中、也不停在缩放锚点上）');
  assert.equal(win._inline.top, '290px');
});

test('最大化往返后再缩放一次，仍然只在还原那次居中（幂等，不来回拉锯）', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  maximize(ctx, win);
  unmaximize(ctx, win, 900, 500); // 还原并顺手改了尺寸
  assert.equal(win._inline.left, '150px', '按还原后的实测尺寸居中：((1200-900)/2)');
  assert.equal(win._inline.top, '160px', '((820-500)/2)');
  pointerDown(ctx, win);
  win._inline.left = '40px';      // 用户再拖走（按住指针 = 意图）
  fireEl(ctx, win);
  pointerUp(ctx);
  assert.equal(win._inline.left, '40px', '不得反复居中');
});

test('不做全量重扫：不相关的节点插入不会去动已有窗口', () => {
  const ctx = fakeCtx();
  const existing = newWindow(ctx);
  ctx.doc._windows = [existing]; // 页面加载时就存在的窗口（首次扫描会居中它）
  load(ctx);
  assert.equal(existing._inline.left, '50px', '首次扫描要居中已存在的窗口');
  existing._inline.left = '400px'; // 之后用户挪了它
  fireDoc(ctx, [{ addedNodes: [fakeEl({ isWindow: false })] }]);
  assert.equal(existing._inline.left, '400px', '观察回调只处理新增节点，不得触发全量重扫');
});

// ---------- 内容区尺寸变化（外壳窗口最大化/还原）也要保持居中 ----------

test('外壳最大化（内容区变大）后，居中的窗口仍然居中', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  assert.equal(win._inline.left, '50px');
  setArea(ctx, 1920, 1009); // 实测：本机外壳最大化后的视口
  assert.equal(win._inline.left, '410px', '((1920-1100)/2) 居中');
  assert.equal(win._inline.top, '185px', '((1009-640)/2) 四舍五入');
  setArea(ctx, 1200, 820); // 还原
  assert.equal(win._inline.left, '50px', '还原后回到原居中位置');
  assert.equal(win._inline.top, '90px');
});

test('用户拖到一边的窗口：尺寸变化时保持**相对位置**，不被硬拉回中间', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  ctx.win.drain();            // 落位后的两次幂等重试先跑完（真实里用户也不会在 160ms 内拖）
  pointerDown(ctx, win);
  win._inline.left = '600px'; // 用户拖到右侧：中心偏移比例 ≈ (600+550-600)/600 = 0.917
  win._inline.top = '90px';
  fireEl(ctx, win);           // 指针按着 = 用户意图 → 记下比例
  pointerUp(ctx);
  setArea(ctx, 1920, 1009);
  // 新尺寸下：中心 = 960 + 0.917×960 ≈ 1840 → left ≈ 1290（保持贴右的相对位置）
  assert.equal(win._inline.left, '1290px', '按比例跟随，不回到中间');
  assert.equal(win._inline.top, '185px', '竖直方向原本就是居中，仍居中');
});

test('外壳最大化时 fnOS 抢先写了级联位置：非用户写入按新尺寸纠正，重复执行稳定', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  ctx.win.drain();
  // 模拟外壳正在最大化：内容区/视口一起变大，fnOS 按它自己的算法改写位置（实测行为）
  ctx.area.clientWidth = 1920;
  ctx.area.clientHeight = 1009;
  ctx.win.innerWidth = 1920;
  ctx.win.innerHeight = 1009;
  win._inline.left = '165px';
  win._inline.top = '202px';
  fireEl(ctx, win);
  assert.equal(win._inline.left, '410px', '纠正必须按比例（0 = 居中）在新尺寸下落位');
  const first = win._inline.left;
  (ctx.doc._win._resizeListeners || []).forEach((fn) => fn());
  ctx.win.drain();
  assert.equal(win._inline.left, first, '重复执行结果恒定（不会漂移）');
});

test('视口尺寸没变的 resize（Dock 隐藏的合成事件那种）：不得触发重排', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  ctx.win.drain();
  // 首次 resize（建立基准）→ 重排照常
  (ctx.doc._win._resizeListeners || []).forEach((fn) => fn());
  ctx.win.drain();
  assert.equal(win._inline.left, '50px');
  win._inline.left = '400px'; // 伪造一个「当前值」（不触发属性观察）
  (ctx.doc._win._resizeListeners || []).forEach((fn) => fn()); // 同尺寸再来一次
  ctx.win.drain();
  assert.equal(win._inline.left, '400px', '同尺寸的 resize 不得再触发重排（修复轮 19）');
});

test('最大化形态的窗口在尺寸变化时不被写位置', () => {
  const ctx = fakeCtx();
  load(ctx);
  const win = newWindow(ctx);
  fireDoc(ctx, [{ addedNodes: [win] }]);
  ctx.win.drain();
  maximize(ctx, win); // 铺满内容区 → 记成最大化形态
  // 外壳随后也最大化：内容区与「已铺满的窗口」一起变大（窗口管理器会同步铺满）
  ctx.area.clientWidth = 1920;
  ctx.area.clientHeight = 1009;
  ctx.win.innerWidth = 1920;
  ctx.win.innerHeight = 1009;
  win.offsetWidth = 1920;
  win.offsetHeight = 1009;
  win._inline.left = '';
  win._inline.top = '';
  fireEl(ctx, win);
  (ctx.doc._win._resizeListeners || []).forEach((fn) => fn());
  ctx.win.drain();
  assert.equal(win._inline.left, '', '铺满内容区的窗口由样式铺满，不得被写死位置');
  assert.equal(win._inline.top, '');
});

test('纪律：非窗口元素零改动、不写页面内容、只在顶层文档工作', () => {
  const ctx = fakeCtx();
  load(ctx);
  const plain = fakeEl({ isWindow: false });
  fireDoc(ctx, [{ addedNodes: [plain] }, { addedNodes: [null] }, { addedNodes: [] }]);
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
