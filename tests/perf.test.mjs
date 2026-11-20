// perf.js（T14g 软件渲染降级）的行为契约：
//   · **只**在确认是软件光栅（Microsoft Basic Render Driver / SwiftShader / llvmpipe）时降级；
//   · 有 GPU 加速（任何真实渲染器名）时**一个字节都不改**（不落样式、不加 class）；
//   · 拿不到 WebGL 渲染器名（被策略禁用 / 探针失败）时**不降级**——宁可保留效果；
//   · 降级只动上游那几处最贵的合成属性，别的选择器一概不碰；
//   · documentElement 还不存在时不抛错，等它出现再落（赶在桌面首帧之前）。
//
// 实测依据（`inject/perf.js` 文件头）：软件光栅下拖窗口 4.7 fps → 19.8 fps；GPU 机器零收益。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const PERF = readFileSync(new URL('../src-tauri/inject/perf.js', import.meta.url), 'utf8');

/** 假 document/window：createElement('canvas') 的 WebGL 结果可控。 */
function fakeCtx(opts = {}) {
  const added = [];
  const styleEls = [];
  const root = {
    _classes: new Set(),
    get className() { return [...this._classes].join(' '); },
    set className(v) { this._classes = new Set(String(v).split(/\s+/).filter(Boolean)); },
    classList: null,
    appendChild(el) { added.push(el); if (el && el.tagName === 'STYLE') styleEls.push(el); return el; },
  };
  root.classList = {
    add: (c) => root._classes.add(c),
    contains: (c) => root._classes.has(c),
  };
  const doc = {
    documentElement: opts.noRoot ? null : root,
    head: root,
    _byId: new Map(),
    _listeners: {},
    getElementById(id) { return this._byId.get(id) || null; },
    createElement(tag) {
      if (tag === 'style') {
        const el = { tagName: 'STYLE', id: '', textContent: '' };
        queueMicrotask(() => { if (el.id) doc._byId.set(el.id, el); });
        // 同步登记：perf.js 落样式后不会再查它，但让 getElementById 能看见更接近真实
        Object.defineProperty(el, 'id', {
          get() { return this._id || ''; },
          set(v) { this._id = v; doc._byId.set(v, el); },
        });
        return el;
      }
      if (tag === 'canvas') {
        if (opts.canvasThrows) throw new Error('canvas blocked');
        return {
          getContext(kind) {
            if (kind !== 'webgl' && kind !== 'experimental-webgl') return null;
            if (opts.noWebgl) return null;
            return {
              getExtension(name) {
                if (name === 'WEBGL_debug_renderer_info') {
                  return opts.noDebugInfo ? null : { UNMASKED_RENDERER_WEBGL: 0x9246 };
                }
                if (name === 'WEBGL_lose_context') return { loseContext() { this.lost = true; } };
                return null;
              },
              getParameter(p) {
                if (p === 0x9246) return opts.renderer ?? 'ANGLE (NVIDIA, NVIDIA Quadro 2000 Direct3D11 vs_5_0 ps_5_0, D3D11)';
                return null;
              },
            };
          },
        };
      }
      return { tagName: String(tag).toUpperCase() };
    },
    addEventListener(ev, fn) { (this._listeners[ev] ||= []).push(fn); },
  };
  const win = {
    document: doc,
    top: null,
    self: null,
    MutationObserver: opts.noMO ? undefined : class {
      constructor(cb) { this.cb = cb; doc._mo = this; }
      observe() {}
      disconnect() { this.disconnected = true; }
    },
  };
  win.top = win;
  win.self = win;
  return { doc, win, root, added, styleEls };
}

function load(ctx) {
  const fn = new Function('window', PERF + '\nreturn window;');
  return fn(ctx.win);
}

test('软件光栅（Microsoft Basic Render Driver）→ 降级：加根 class + 落样式', () => {
  const ctx = fakeCtx({ renderer: 'ANGLE (Microsoft, Microsoft Basic Render Driver (0x0000008C) Direct3D11 vs_5_0 ps_5_0, D3D11)' });
  const win = load(ctx);
  assert.equal(win.__FNOS_PERF__.softwareRender, true, '必须识别为软件光栅');
  assert.equal(win.__FNOS_PERF__.degraded, true, '必须真的落下降级');
  assert.ok(ctx.root._classes.has('fnos-software-render'), '根元素必须带降级 class');
  assert.equal(ctx.styleEls.length, 1, '必须恰好落一段样式');
  const css = ctx.styleEls[0].textContent;
  assert.equal(ctx.styleEls[0].id, 'fnos-shell-software-render-style');
  // 最贵的两项：静态背景模糊 + 强制分层
  assert.ok(css.includes('.trim-ui__app-layout--window'), '必须锚定上游的应用窗口类');
  assert.ok(css.includes('backdrop-filter: none !important;'), '必须去掉背景模糊');
  assert.ok(css.includes('will-change: auto !important;'), '必须去掉强制分层');
  // 模态遮罩的背景模糊
  assert.ok(css.includes('#root .semi-modal-mask'), '必须去掉模态遮罩的背景模糊');
  // 动画期的 filter: blur(...) 归零（与上游「窗口动画模糊」关闭时同一组变量）
  for (const v of [
    '--fnos-window-motion-backdrop-start-blur',
    '--fnos-window-enter-filter-start-blur',
    '--fnos-window-exit-close-filter-end-blur',
    '--fnos-window-exit-minimize-filter-end-blur',
    '--fnos-window-restore-filter-end-blur',
  ]) {
    assert.ok(css.includes(v + ': 0px'), '必须把 ' + v + ' 归零');
  }
  // 纪律：不碰别的选择器（只有窗口 / 模态遮罩 / 变量三组）
  assert.ok(!css.includes('body *') && !css.includes('!important; }'), '不得写通配式覆盖');
  assert.ok(css.length < 2000, '降级样式必须保持小而可审计');
});

test('软件光栅的另外两种名字（SwiftShader / llvmpipe）同样降级', () => {
  for (const renderer of ['Google SwiftShader', 'llvmpipe (LLVM 15.0.7, 256 bits)', 'Mesa softpipe']) {
    const ctx = fakeCtx({ renderer });
    const win = load(ctx);
    assert.equal(win.__FNOS_PERF__.softwareRender, true, renderer + ' 必须识别为软件光栅');
    assert.equal(win.__FNOS_PERF__.degraded, true, renderer + ' 必须降级');
  }
});

test('GPU 加速（真实渲染器名）→ 一个字节都不改', () => {
  for (const renderer of [
    'ANGLE (NVIDIA, NVIDIA Quadro 2000 (0x00000DD8) Direct3D11 vs_5_0 ps_5_0, D3D11)',
    'ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)',
    'ANGLE (AMD, AMD Radeon RX 6600 Direct3D11 vs_5_0 ps_5_0, D3D11)',
    'ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)',
  ]) {
    const ctx = fakeCtx({ renderer });
    const win = load(ctx);
    assert.equal(win.__FNOS_PERF__.softwareRender, false, renderer + ' 不得被判成软件光栅');
    assert.equal(win.__FNOS_PERF__.degraded, false);
    assert.equal(ctx.styleEls.length, 0, 'GPU 机器上不得落任何样式');
    assert.equal(ctx.root._classes.size, 0, 'GPU 机器上不得改根 class');
  }
});

test('拿不到渲染器名（WebGL 被禁 / 探针失败 / 无扩展）→ 不降级', () => {
  for (const opts of [
    { noWebgl: true },        // 策略/参数禁用 WebGL
    { canvasThrows: true },   // 连 canvas 都建不出来
    { noDebugInfo: true },    // 拿不到 UNMASKED_RENDERER_WEBGL
  ]) {
    const ctx = fakeCtx(opts);
    const win = load(ctx);
    assert.equal(win.__FNOS_PERF__.softwareRender, false, '不知道 ≠ 软件光栅');
    assert.equal(win.__FNOS_PERF__.degraded, false);
    assert.equal(ctx.styleEls.length, 0);
  }
});

test('documentElement 还不存在时不抛错，等它出现再落（观察器路径）', () => {
  const ctx = fakeCtx({
    noRoot: true,
    renderer: 'ANGLE (Microsoft, Microsoft Basic Render Driver, D3D11)',
  });
  // 没有 documentElement 时 head 也没有：先给一个可用的 root 供后续落样式
  const late = { _classes: new Set(), classList: { add(c) { this._s.add(c); } }, appendChild() {} };
  const win = load(ctx);
  assert.equal(win.__FNOS_PERF__.softwareRender, true);
  assert.equal(win.__FNOS_PERF__.degraded, false, '根元素还没出现：先不落');
  assert.ok(ctx.doc._mo, '必须挂上观察器等 documentElement');
  ctx.doc.documentElement = late;
  ctx.doc.head = late;
  ctx.doc._mo.cb([]);
  assert.equal(win.__FNOS_PERF__.degraded, true, 'documentElement 一出现就必须落');
  assert.ok(ctx.doc._mo.disconnected, '落完必须断开观察器');
});
