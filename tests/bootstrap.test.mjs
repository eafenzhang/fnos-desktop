import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const SHIM = readFileSync(new URL('../src-tauri/inject/shim.js', import.meta.url), 'utf8');
const BOOT = readFileSync(new URL('../src-tauri/inject/bootstrap.js', import.meta.url), 'utf8');

const MOD_SCRIPT_ID = 'fnos-ui-mods-script';
const MANAGED_LINK_ID = 'fnos-ui-mods-basic-style';

// fakeDom(opts.readyState = 'complete') 才能让 afterLoad() 在 load() 期间立即执行——
// 观察者/定时器路径全部挂在 afterLoad 上，'loading' 会让 §5.4 的观察逻辑根本装不上。
function fakeDom(opts = {}) {
  const nodes = [];
  const doc = {
    nodes,
    head: { appendChild: (n) => nodes.push(n) },
    getElementById: (id) => nodes.find((n) => n.id === id) || null,
    createElement: (tag) => ({ tag, setAttribute() {}, style: {} }),
    documentElement: { appendChild: () => {} },
    addEventListener(ev, fn) { (this._listeners ||= {})[ev] = fn; },
    adoptedStyleSheets: [],
    readyState: opts.readyState || 'loading'
  };
  return doc;
}

// 确定性排空 _timers：按入队顺序执行，带深度上限（轮询路径会自我续期）。
function drainTimers(doc, maxDepth = 200) {
  for (let i = 0; i < maxDepth; i++) {
    const timers = doc._timers;
    if (!timers || !timers.length) return i;
    timers.shift()();
  }
  throw new Error('drainTimers: 超过深度上限（疑似自续期定时器死循环）');
}

function load(shell, doc, opts = {}) {
  const installed = [];
  class CSSStyleSheetStub {
    constructor() { this.cssText = ''; }
    replaceSync(text) { this.cssText = text; installed.push(this); }
  }
  // MutationObserver 替身：与 CSSStyleSheet/setTimeout 同样的注入方式。
  // 记录 callback（doc._observerCallback）供测试手动触发，并记录 observe/disconnect。
  class MutationObserverStub {
    constructor(cb) {
      this.callback = cb;
      doc._observer = this;
      doc._observerCallback = cb;
    }
    observe(target, options) {
      this.target = target;
      this.options = options;
      doc._observed = { target, options };
    }
    disconnect() {
      this.disconnected = true;
      doc._observerDisconnected = true;
    }
  }
  const cssStub = { supports: () => true };
  const setTimeoutStub = (fn) => { (doc._timers ||= []).push(fn); return (doc._timers.length); };
  const MO = opts.noMutationObserver ? undefined : MutationObserverStub;
  const win = {
    __FNOS_SHELL__: shell,
    document: doc,
    CSS: cssStub,
    CSSStyleSheet: CSSStyleSheetStub,
    MutationObserver: MO
  };
  win.window = win;
  const fn = new Function(
    'window', 'document', 'TextEncoder', 'queueMicrotask', 'btoa', 'CSS', 'CSSStyleSheet', 'setTimeout', 'MutationObserver',
    SHIM + '\n' + BOOT + '\nreturn window;'
  );
  const w = fn(win, doc, TextEncoder, queueMicrotask, (s) => Buffer.from(s, 'binary').toString('base64'),
               cssStub, CSSStyleSheetStub, setTimeoutStub, MO);
  w.__installedSheets = installed;
  return w;
}

const SHELL = {
  mods: {}, local: {},
  assets: { 'basic_mod.css': 'body{color:red}', 'mod.js': 'window.__MOD_RAN__=(window.__MOD_RAN__||0)+1;' }
};

test('bootstrap 暴露版本与受管 CSS id 列表', () => {
  const doc = fakeDom();
  const w = load({ ...SHELL, meta: { shellVersion: '9.9.9' } }, doc);
  assert.equal(typeof w.__FNOS_BOOTSTRAP__.version, 'string');
  // 真实值必须来自 __FNOS_SHELL__.meta.shellVersion，而不是 '0.0.0' 回落
  assert.equal(w.__FNOS_BOOTSTRAP__.version, '9.9.9');
  assert.ok(w.__FNOS_BOOTSTRAP__.cssIds.includes(MANAGED_LINK_ID));
});

test('本壳样式覆盖：绝对定位的整屏覆盖层恢复不透明（应用详情不再是透明的）', () => {
  const doc = fakeDom({ readyState: 'complete' });
  const w = load(SHELL, doc);
  // 覆盖表必须装（与上游 CSS 是否被拦无关）
  const css = (w.__installedSheets || []).map((s) => s.cssText).join(String.fromCharCode(10));
  const marker = 'bg-\\[var\\(--semi-color-app-container\\)\\].absolute.inset-0';
  assert.ok(css.indexOf(marker) >= 0, '必须有针对「绝对定位整屏覆盖层」的通用规则');
  assert.ok(css.indexOf(marker + '{background-color:var(--semi-color-app-container) !important;}') >= 0,
    '用主题自己的容器底色恢复不透明，并带 !important');
  // 应用中心专用一条：上游 basic_mod.css:424 把 .fnos-app-center-route-detail 设成
  // transparent !important（约 0,4,1），通用三条类（0,3,0）压不过它 —— 必须同形态 + 更高特异性。
  assert.ok(css.indexOf('.fnos-app-center-route-detail.fnos-app-center-route-active') >= 0,
    '必须有针对应用中心详情（激活态）的规则');
  assert.ok(css.indexOf('.fnos-app-center-route-detail.absolute.inset-0') >= 0,
    '离场动画态同样要覆盖（上游那条也是 transparent !important）');
  assert.ok(css.indexOf('img[alt="应用中心"]') >= 0, '必须锚定上游自己的应用中心窗口选择器');
  const appCenterRule = css.slice(css.indexOf('fnos-app-center-route-detail'));
  assert.ok(appCenterRule.indexOf('background-color:var(--semi-color-app-container) !important;') >= 0,
    '应用中心那条也要用主题容器底色恢复不透明');
  // 过渡期透明：上游给该面板挂了 `background-color 0s linear 320ms`（背景延迟切换），
  // 必须把 background-color 从过渡属性里去掉，否则「列表→详情」会露出一瞬透明。
  assert.ok(appCenterRule.indexOf('transition-property:transform,opacity !important;') >= 0,
    '必须去掉 background-color 的过渡延迟');
  // 桌面壁纸顶行的亮蓝线：裁掉壁纸顶部 1px
  assert.ok(css.indexOf('#root .absolute.inset-0.z-0.object-contain{clip-path:inset(1px 0 0 0) !important;}') >= 0,
    '必须有「裁掉壁纸顶部 1px」的规则');
});

// Dock 预留清除（T14c 修复轮 14～15）：与上面那张表**分开**，按自动隐藏开关装卸。
// 语义：藏 Dock 时才撤掉那条预留；Dock 常驻时预留必须留着（否则内容钻到 Dock 底下）。
test('Dock 预留清除表：开关开着才装，三处预留都在样式层（元素一出现就是 0）', () => {
  const doc = fakeDom({ readyState: 'complete' });
  const w = load({ ...SHELL, shell: { dockAutoHide: true } }, doc);
  const css = doc.adoptedStyleSheets.map((s) => s.cssText).join(String.fromCharCode(10));
  // 注：CSS 文本里类名带转义（`.pl-\[66px\]{…}`），断言用 `\\]` 匹配那一个反斜杠。
  assert.ok(css.includes('66px\\]{padding-left:0 !important;}'),
    '必须有「预留类置 0」的样式层规则');
  assert.ok(css.includes('#root .desktop > [class*="pl-["]{padding-left:0 !important;}'),
    '必须有与取值无关的兜底规则（作用域到 .desktop 的直接子元素）');
  assert.ok(css.includes('[class*="pl-[280px]"]{padding-left:0 !important;}'),
    '经典启动台的预留（pl-[280px]）也必须在样式层置 0');
  assert.ok(css.includes('.box-border.flex.h-full.flex-col.items-center.justify-start.py-\\[64px\\]{padding-left:0 !important;}'),
    '启动台容器还要一条与取值无关的（按结构锚定，别的 fnOS 版本换数值也命中）');
  assert.equal(w.__FNOS_SHELL_CSS__.dockReclaim, true);
});

test('Dock 预留清除表：开关关着（含未给 shell 段）不装，基础表里也没有预留规则', () => {
  for (const shell of [{}, { shell: {} }, { shell: { dockAutoHide: false } }]) {
    const doc = fakeDom({ readyState: 'complete' });
    const w = load({ ...SHELL, ...shell }, doc);
    const css = doc.adoptedStyleSheets.map((s) => s.cssText).join(String.fromCharCode(10));
    assert.ok(!css.includes('padding-left:0 !important;}'),
      '关着时不装预留清除（Dock 常驻，内容该让出宽度）');
    // 基础表仍然要在（不透明覆盖层 / 蓝线那些与开关无关）
    assert.ok(css.includes('--semi-color-app-container'), '基础覆盖表必须照装');
    assert.equal(w.__FNOS_SHELL_CSS__.dockReclaim, false);
  }
});

test('Dock 预留清除表：免刷新装卸（装可幂等、卸后列表里真的没有）', () => {
  const doc = fakeDom({ readyState: 'complete' });
  const w = load({ ...SHELL, shell: { dockAutoHide: false } }, doc);
  w.__FNOS_SHELL_CSS__.setDockReclaim(true);
  assert.equal(w.__FNOS_SHELL_CSS__.dockReclaim, true);
  const texts = doc.adoptedStyleSheets.map((s) => s.cssText);
  assert.ok(texts.some((t) => t.includes('[class*="pl-[280px]"]')), '装上后启动台规则必须在');
  w.__FNOS_SHELL_CSS__.setDockReclaim(true); // 幂等：不得重复装
  assert.equal(doc.adoptedStyleSheets.filter((s) => s.cssText.includes('pl-[280px]')).length, 1);
  w.__FNOS_SHELL_CSS__.setDockReclaim(false);
  assert.equal(w.__FNOS_SHELL_CSS__.dockReclaim, false);
  assert.ok(!doc.adoptedStyleSheets.some((s) => s.cssText.includes('pl-[280px]')),
    '卸下后不得残留（否则 Dock 常驻时内容会钻到底下）');
  // 基础表不受影响（它不在装卸范围内）
  assert.ok(doc.adoptedStyleSheets.some((s) => s.cssText.includes('--semi-color-app-container')));
});

test('link 未生效时用 adoptedStyleSheets 补装 CSS', () => {
  const doc = fakeDom({ readyState: 'complete' });
  // 受管 link 已注入但 sheet === null（被 CSP/协议拦掉的真实形态）→ cssMissing() 必须为真
  doc.nodes.push({ id: MANAGED_LINK_ID, sheet: null });
  const w = load(SHELL, doc);
  assert.ok(w.__FNOS_BOOTSTRAP__.fallbackInstalled >= 1);
  assert.ok(doc.adoptedStyleSheets.length >= 1);
  // 按**内容**判定（不依赖安装顺序）：上游那份兜底 CSS 必须真的在列表里
  const texts = doc.adoptedStyleSheets.map((x) => x.cssText);
  assert.ok(texts.indexOf('body{color:red}') >= 0, '上游 CSS 兜底必须装上');
  assert.equal(doc.adoptedStyleSheets.indexOf(w.__installedSheets[0]) >= 0, true);
  // 本壳的覆盖表也在（无论上游 CSS 是否被拦，它都要装）
  assert.ok(texts.some((t) => t.indexOf('--semi-color-app-container') >= 0), '本壳覆盖表必须装上');
});

test('link 生效时不触发 CSS 兜底（cssMissing 对照组）', () => {
  const doc = fakeDom({ readyState: 'complete' });
  doc.nodes.push({ id: MANAGED_LINK_ID, sheet: {} });
  const w = load(SHELL, doc);
  assert.equal(w.__FNOS_BOOTSTRAP__.fallbackInstalled, 0);
  // 上游 CSS 兜底不得触发；列表里只允许有本壳自己的覆盖表（它与应用 CSS 无关，永远装）
  const texts = doc.adoptedStyleSheets.map((x) => x.cssText);
  assert.equal(texts.indexOf('body{color:red}'), -1, '不得补装上游 CSS');
  assert.ok(texts.every((t) => t.indexOf('--semi-color-app-container') >= 0), '只应有本壳覆盖表');
});

test('mod.js 未执行时兜底执行且只执行一次', () => {
  const doc = fakeDom();
  const w = load(SHELL, doc);
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  assert.equal(w.__MOD_RAN__, 1);
  assert.equal(w.__FNOS_MOD_EXECUTED__, true);
});

test('标记已置位时不兜底（ensureModJs 是 no-op，不重复执行、不标记兜底）', () => {
  const doc = fakeDom();
  const w = load(SHELL, doc);
  // 模拟 shim 的 data: <script> 路径已经跑过：标记已置位
  w.__FNOS_MOD_EXECUTED__ = true;
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  assert.equal(w.__MOD_RAN__, undefined);
  assert.equal(w.__FNOS_BOOTSTRAP__.modFallbackUsed, false);
});

test('7 个受管 CSS 文件都可经 adoptedStyleSheets 补装（R29）', () => {
  const doc = fakeDom();
  const files = [
    'basic_mod.css', 'windows_titlebar_mod.css', 'mac_titlebar_mod.css',
    'classic_launchpad_mod.css', 'spotlight_launchpad_mod.css',
    'desktop_icon_mod.css', 'lockscreen_mod.css'
  ];
  const assets = {};
  for (const f of files) assets[f] = '.' + f + '{}';
  const w = load({ mods: {}, local: {}, assets }, doc);
  w.__FNOS_BOOTSTRAP__.installFallbackCss();
  assert.equal(w.__FNOS_BOOTSTRAP__.fallbackInstalled, 7);
  assert.equal(doc.adoptedStyleSheets.length, 7);
  assert.deepEqual(w.__installedSheets.map((s) => s.cssText), files.map((f) => '.' + f + '{}'));
});

test('缺 __FNOS_SHELL__ / assets 时不抛异常', () => {
  const doc = fakeDom();
  const w = load(undefined, doc);
  assert.equal(typeof w.__FNOS_BOOTSTRAP__.version, 'string');
  assert.equal(w.__FNOS_BOOTSTRAP__.version, '0.0.0'); // meta 缺失时的回落值（对照组）
  w.__FNOS_BOOTSTRAP__.installFallbackCss();
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  const w2 = load({ mods: {}, local: {} }, fakeDom());
  w2.__FNOS_BOOTSTRAP__.installFallbackCss();
  w2.__FNOS_BOOTSTRAP__.ensureModJs();
  assert.equal(w2.__FNOS_BOOTSTRAP__.modFallbackUsed, false);
});

test('installFallbackCss 幂等：重复自检不重复安装、不重复计数', () => {
  const doc = fakeDom({ readyState: 'complete' });
  doc.nodes.push({ id: MANAGED_LINK_ID, sheet: null });
  const w = load(SHELL, doc);
  drainTimers(doc); // 跑掉 +600ms 的第二次自检
  w.__FNOS_BOOTSTRAP__.installFallbackCss(); // 再显式调一次
  assert.equal(w.__FNOS_BOOTSTRAP__.fallbackInstalled, 1);
  // 按**内容**判定安装次数（不走顺序/总数：本壳覆盖表也在同一个列表里）
  assert.equal(doc.adoptedStyleSheets.filter((x) => x.cssText === 'body{color:red}').length, 1,
    '上游 CSS 只补装一次');
  assert.equal(doc.adoptedStyleSheets.filter((x) => x.cssText.indexOf('--semi-color-app-container') >= 0).length, 1,
    '本壳覆盖表也只装一次（幂等）');
});

test('script 元素出现约 100ms 后才兜底执行 mod.js', () => {
  const doc = fakeDom({ readyState: 'complete' });
  const w = load(SHELL, doc);
  // 装配时不观察即执行是禁止的：观察者必须已挂上，且此刻什么都没跑
  assert.equal(typeof doc._observerCallback, 'function');
  assert.deepEqual(doc._observed.options, { childList: true, subtree: true });
  assert.equal(w.__MOD_RAN__, undefined);

  // 模拟上游插入 <script id="fnos-ui-mods-script">（data: 被 CSP 拦 → 标记不会置位）
  doc.nodes.push({ id: MOD_SCRIPT_ID, tag: 'script' });
  doc._observerCallback([{ type: 'childList' }]);

  assert.equal(doc._observerDisconnected, true); // 首次出现即停止观察
  assert.equal(w.__MOD_RAN__, undefined);        // 约 100ms 检查还没到期
  drainTimers(doc);
  assert.equal(w.__MOD_RAN__, 1);
  assert.equal(w.__FNOS_MOD_EXECUTED__, true);
  assert.equal(w.__FNOS_BOOTSTRAP__.modFallbackUsed, true);
});

test('上游始终不注入时不会执行 mod.js', () => {
  const doc = fakeDom({ readyState: 'complete' });
  const w = load(SHELL, doc);
  // 从不插入 #fnos-ui-mods-script；即使上游压根没注入（autoEnableSuspectedFnOS=false
  // 或签名探测超时），也必须排空所有定时器后仍然没跑过 mod.js
  drainTimers(doc);
  assert.equal(w.__MOD_RAN__, undefined);
  assert.equal(w.__FNOS_MOD_EXECUTED__, undefined);
  assert.equal(w.__FNOS_MOD_FALLBACK_FAILED__, undefined);
  assert.equal(w.__FNOS_BOOTSTRAP__.modFallbackUsed, false);
});

test('元素在装配时已存在则立即进入检查（不再观察）', () => {
  const doc = fakeDom({ readyState: 'complete' });
  doc.nodes.push({ id: MOD_SCRIPT_ID, tag: 'script' });
  const w = load(SHELL, doc);
  assert.equal(doc._observer, undefined); // 已存在 → 无需观察
  assert.equal(w.__MOD_RAN__, undefined); // 仍等 ~100ms
  drainTimers(doc);
  assert.equal(w.__MOD_RAN__, 1);
});

test('new Function 与 (0,eval) 双双被拒时失败可观测，不置执行标记', () => {
  const doc = fakeDom();
  // 语法非法的 mod.js 原文：new Function 与间接 eval 都会抛 SyntaxError → 双路径失败
  const w = load({ mods: {}, local: {}, assets: { 'mod.js': 'function {' } }, doc);
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  assert.equal(w.__FNOS_MOD_FALLBACK_FAILED__, true);
  assert.equal(w.__FNOS_BOOTSTRAP__.modFallbackFailed, true);
  assert.equal(w.__FNOS_BOOTSTRAP__.modFallbackUsed, false); // 只有真的执行成功才算兜底成功
  assert.equal(w.__MOD_RAN__, undefined);
  assert.equal(w.__FNOS_MOD_EXECUTED__, undefined);          // 留给后续合法路径
});

test('MutationObserver 不可用时退化有界轮询：元素不出现则不执行 mod.js', () => {
  const doc = fakeDom({ readyState: 'complete' });
  const w = load(SHELL, doc, { noMutationObserver: true });
  assert.equal(doc._observer, undefined);
  const ran = drainTimers(doc);
  assert.ok(ran <= 40, `轮询必须是有界的（实际排空 ${ran} 个定时器）`);
  assert.equal(w.__MOD_RAN__, undefined);
  assert.equal(w.__FNOS_MOD_EXECUTED__, undefined);
});

test('MutationObserver 不可用时退化有界轮询：元素出现后兜底执行一次', () => {
  const doc = fakeDom({ readyState: 'complete' });
  const w = load(SHELL, doc, { noMutationObserver: true });
  doc.nodes.push({ id: MOD_SCRIPT_ID, tag: 'script' });
  drainTimers(doc);
  assert.equal(w.__MOD_RAN__, 1);
  assert.equal(w.__FNOS_MOD_EXECUTED__, true);
});
