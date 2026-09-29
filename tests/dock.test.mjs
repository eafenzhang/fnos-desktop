// dock.js（T14c）的行为契约：上游类名策略 + 本壳命名空间 + 免刷新切换 + 接管即藏。
//
// dock.js 在 document-start 注入，逻辑全在 DOM 上（观察器 / 指针 / class），
// 所以和 bootstrap.test.mjs 一样用**假 DOM**跑真实源码；选择器与纪律用源码级断言锁。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const DOCK = readFileSync(new URL('../src-tauri/inject/dock.js', import.meta.url), 'utf8');
const MOD = readFileSync(new URL('../src-tauri/assets/fnos-mods/mod.js', import.meta.url), 'utf8');

/** 上游 mod.js 的任务栏根/列表选择器：dock.js 必须逐字复用（上游类名策略的锚点）。 */
const UPSTREAM_ROOT = '.h-screen.fixed.left-0';
const UPSTREAM_LIST =
  '.scrollbar-hidden.absolute.inset-0.flex.flex-col.items-end.justify-start.gap-2.overflow-y-auto.pt-2';

/** 假 Dock 元素：classList 是真实的加减集合，rect/offset 由调用方给定。 */
function fakeDock(rect, opts = {}) {
  const classes = new Set();
  // 真实 DOMRect 恒有 width/height；桩补全，免得 dock.js 的零尺寸守卫把桩误判成未布局
  rect.width = rect.right - rect.left;
  rect.height = rect.bottom - rect.top;
  return {
    _classes: classes,
    _hasList: !!opts.hasList,
    classList: {
      add: (...cs) => cs.forEach((c) => classes.add(c)),
      remove: (...cs) => cs.forEach((c) => classes.delete(c)),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)),
      contains: (c) => classes.has(c),
    },
    style: {
      _props: {},
      setProperty(k, v) { this._props[k] = v; },
      removeProperty(k) { delete this._props[k]; },
    },
    offsetWidth: rect.right - rect.left,
    offsetHeight: rect.bottom - rect.top,
    getBoundingClientRect: () => rect,
    matches: (sel) => (sel === ':hover' ? !!opts.hovered : false),
    // findDock 的上游同款验证：根里能找到任务栏列表才算数
    querySelector: (sel) => (sel === UPSTREAM_LIST && opts.hasList ? {} : null),
    isConnected: true,
  };
}

function fakeDom(opts = {}) {
  const doc = {
    _listeners: {},
    _timers: [],
    _styles: [],
    _candidates: [], // querySelectorAll('.h-screen.fixed.left-0') 的返回
    adoptedStyleSheets: [],
    head: { appendChild: (n) => doc._styles.push(n) },
    documentElement: { appendChild: (n) => doc._styles.push(n) },
    createElement: () => ({ tag: 'style', textContent: '' }),
    getElementById: (id) => doc._styles.find((n) => n.id === id) || null,
    querySelectorAll: (sel) => (sel === UPSTREAM_ROOT ? doc._candidates : []),
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
    setTimeout: (fn) => { doc._timers.push(fn); return doc._timers.length; },
    clearTimeout: () => {},
  };
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

/** 排空 locate 的合并定时器（dock.js 用它把一批 SPA 变更并成一次查询）。 */
function drainTimers(doc) {
  const timers = doc._timers.splice(0);
  timers.forEach((t) => t());
}

// ---------- 源码级：上游类名策略与纪律 ----------

test('DOCK_ROOT/DOCK_LIST 选择器逐字取自上游 mod.js（上游类名策略）', () => {
  const root = MOD.match(/const TASKBAR_ROOT_SELECTOR = '([^']+)';/);
  const list = MOD.match(/const TASKBAR_LIST_SELECTOR =\n?\s*'([^']+)';/);
  assert.ok(root, '上游 mod.js 必须仍有 TASKBAR_ROOT_SELECTOR 常量');
  assert.ok(list, '上游 mod.js 必须仍有 TASKBAR_LIST_SELECTOR 常量');
  assert.equal(root[1], UPSTREAM_ROOT);
  assert.equal(list[1], UPSTREAM_LIST);
  // dock.js 里两条选择器必须与上游逐字相同——自造选择器会在上游改类名时静默失明
  assert.ok(DOCK.includes(`var DOCK_ROOT_SELECTOR = '${UPSTREAM_ROOT}';`));
  assert.ok(DOCK.includes(`'${UPSTREAM_LIST}'`), 'dock.js 必须逐字复用上游的列表选择器（根节点验证）');
});

test('纪律：dock.js 不开 IPC、不写 innerHTML、只在自己的 class 命名空间里动手', () => {
  assert.ok(!DOCK.includes('invoke('), '不得给页面开命令授权');
  assert.ok(!DOCK.includes('innerHTML'), '只加 class / 注入自己的样式，绝不 innerHTML');
  for (const cls of ['fnos-shell-dock-autohide', 'fnos-shell-dock-hidden']) {
    assert.ok(DOCK.includes(cls), `必须有本壳命名空间的 class：${cls}`);
  }
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

// ---------- 初始态：开启 ----------

test('开启：观察 document（body 可能还不存在）、接管 Dock、按贴边方向设置隐藏变换', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  assert.deepEqual(
    doc._moObserved && { target: doc._moObserved.target, subtree: doc._moObserved.options.subtree },
    { target: doc, subtree: true },
    '观察目标必须是 document（document-start 时 body 还不存在）'
  );
  assert.equal(typeof doc._listeners.pointermove, 'function');

  // Dock 出现（observer 回调 → 合并定时器 → locate）：含任务栏列表 → 被选中并接管
  const dock = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 }, { hasList: true });
  doc._candidates = [dock];
  doc._moCallback();
  drainTimers(doc);
  assert.ok(dock._classes.has('fnos-shell-dock-autohide'), '必须加本壳的常驻 class');
  assert.equal(
    dock.style._props['--fnos-dock-hide-tf'], 'translateX(-105%)',
    '贴左缘 → 向左滑出'
  );
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'), '接管即藏（自动隐藏的语义就是平时藏着）');

  // 指针在远处 → 保持藏；进左缘热区 → 回
  doc._listeners.pointermove({ clientX: 600, clientY: 400 });
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'));
  doc._listeners.pointermove({ clientX: 3, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'), '左缘热区必须唤回');
});

test('根节点验证：多个候选时只接管含任务栏列表的那个（镜像上游 resolveTaskbarNodes）', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const decoy = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 }); // 同类名、无列表
  const real = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 }, { hasList: true });
  doc._candidates = [decoy, real];
  doc._moCallback();
  drainTimers(doc);
  assert.ok(real._classes.has('fnos-shell-dock-autohide'), '必须接管有列表的真 Dock');
  assert.equal(decoy._classes.size, 0, '空容器不得被接管（接管它 = 开了没反应）');

  // 列表还没渲染：退回第一个候选（上游也容忍 list: null），列表出现后 observer 重定位
  const doc2 = fakeDom();
  load(doc2, { dockAutoHide: true });
  const only = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 });
  doc2._candidates = [only];
  doc2._moCallback();
  drainTimers(doc2);
  assert.ok(only._classes.has('fnos-shell-dock-autohide'), '无列表时退回第一个候选');
});

test('指针恰好在 Dock 上时接管不抢藏（:hover 优先）', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const dock = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 }, { hasList: true, hovered: true });
  doc._candidates = [dock];
  doc._moCallback();
  drainTimers(doc);
  assert.ok(dock._classes.has('fnos-shell-dock-autohide'));
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'), '悬停中不得立即藏（下次移开由指针驱动）');
});

test('贴右缘的 Dock 向右滑出，热区在右缘（不硬编码左缘）', () => {
  const doc = fakeDom({ vw: 1200 });
  load(doc, { dockAutoHide: true });
  const dock = fakeDock({ left: 1132, top: 0, right: 1200, bottom: 800 }, { hasList: true });
  doc._candidates = [dock];
  doc._moCallback();
  drainTimers(doc);
  assert.equal(dock.style._props['--fnos-dock-hide-tf'], 'translateX(105%)');
  doc._listeners.pointermove({ clientX: 600, clientY: 400 });
  assert.ok(dock._classes.has('fnos-shell-dock-hidden'));
  doc._listeners.pointermove({ clientX: 1196, clientY: 400 });
  assert.ok(!dock._classes.has('fnos-shell-dock-hidden'), '右缘热区必须唤回');
});

test('零尺寸 rect（SPA 未布局）不改写贴边结论', () => {
  const doc = fakeDom({ vw: 1200 });
  load(doc, { dockAutoHide: true });
  const dock = fakeDock({ left: 1132, top: 0, right: 1200, bottom: 800 }, { hasList: true });
  doc._candidates = [dock];
  doc._moCallback();
  drainTimers(doc);
  assert.equal(dock.style._props['--fnos-dock-hide-tf'], 'translateX(105%)', '先量到有效几何 = 右缘');
  // 同一元素换成全零 rect（重新布局前的瞬间）：不得被判成「贴左缘」
  dock.getBoundingClientRect = () => ({ left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 });
  doc._listeners.pointermove({ clientX: 600, clientY: 400 }); // 远处 → 藏（藏前重测）
  assert.equal(dock.style._props['--fnos-dock-hide-tf'], 'translateX(105%)', '零 rect 沿用右缘结论');
});

test('Dock 被 SPA 换掉时重新接管：旧元素清干净，新元素接管', () => {
  const doc = fakeDom();
  load(doc, { dockAutoHide: true });
  const first = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 }, { hasList: true });
  doc._candidates = [first];
  doc._moCallback();
  drainTimers(doc);
  doc._listeners.pointermove({ clientX: 600, clientY: 400 });
  assert.ok(first._classes.has('fnos-shell-dock-hidden'));

  const second = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 }, { hasList: true });
  doc._candidates = [second];
  doc._moCallback();
  drainTimers(doc);
  assert.ok(!first._classes.has('fnos-shell-dock-autohide') && !first._classes.has('fnos-shell-dock-hidden'),
    '被换掉的旧元素必须清掉本壳加的 class（不留下孤儿状态）');
  assert.equal(first.style._props['--fnos-dock-hide-tf'], undefined, '内联变量也要清干净');
  assert.ok(second._classes.has('fnos-shell-dock-autohide'));
});

// ---------- 免刷新切换（shim 转调 __FNOS_APPLY_SHELL__） ----------

test('免刷新切换：关 → 清 class / 断观察器 / 摘指针监听；再开 → 重新接管并立即藏', () => {
  const doc = fakeDom();
  const w = load(doc, { dockAutoHide: true });
  const dock = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 }, { hasList: true });
  doc._candidates = [dock];
  doc._moCallback();
  drainTimers(doc);

  w.__FNOS_APPLY_SHELL__({ dockAutoHide: false });
  assert.ok(doc._moDisconnected, '关闭必须断开观察器');
  assert.equal(doc._listeners.pointermove, undefined, '关闭必须摘掉指针监听');
  assert.equal(dock._classes.size, 0, '关闭必须清掉本壳的 class');
  assert.equal(dock.style._props['--fnos-dock-hide-tf'], undefined, '关闭不留下样式残留');

  // 再开：重新接管（observer 重新装上，Dock 出现后照常工作，且立即藏）
  w.__FNOS_APPLY_SHELL__({ dockAutoHide: true });
  assert.ok(doc._moObserved && !doc._moDisconnected, '重新开启必须重新观察');
  const again = fakeDock({ left: 0, top: 0, right: 68, bottom: 800 }, { hasList: true });
  doc._candidates = [again];
  doc._moCallback();
  drainTimers(doc);
  assert.ok(again._classes.has('fnos-shell-dock-autohide'));
  assert.ok(again._classes.has('fnos-shell-dock-hidden'), '重新开启后立即藏（不依赖第一次指针移动）');
});

test('免刷新切换的形状闸：非 boolean 不动手、同值幂等、非法形状不炸', () => {
  const doc = fakeDom();
  const w = load(doc, {});
  // 从默认关切到字符串 "true"：形状不对 → 仍然关（观察器都没装）
  w.__FNOS_APPLY_SHELL__({ dockAutoHide: 'true' });
  assert.equal(doc._moObserved, undefined);
  // 正常开 → 同值再发一次必须幂等（不能把观察器装两份）
  w.__FNOS_APPLY_SHELL__({ dockAutoHide: true });
  const observed = doc._moObserved;
  w.__FNOS_APPLY_SHELL__({ dockAutoHide: true });
  assert.equal(doc._moObserved, observed, '同值幂等：观察器不得重复安装');
  assert.ok(!doc._moDisconnected, '同值幂等：不得误断观察器');
  // null / 缺键 / 原始值：什么都不发生
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
