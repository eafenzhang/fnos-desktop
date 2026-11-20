/* fnOS Desktop Shell — 软件渲染降级（T14g，性能）
 *
 * 做什么：探测 WebView2 的**渲染后端**，只有确认是软件光栅（Microsoft Basic Render
 * Driver / SwiftShader / llvmpipe）时，才把上游那套最贵的合成效果降下来：
 *   · 应用窗口的 `backdrop-filter: saturate(2) blur(32px)`（`basic_mod.css` 的
 *     `.trim-ui__app-layout--window`，静态 32px 背景模糊）
 *   · 应用窗口的 `will-change: transform, opacity, filter, backdrop-filter`（强制分层）
 *   · 打开/关闭/最小化/还原动画里的 `filter: blur(…)`
 *   · 模态遮罩的 `backdrop-filter: blur(6px)`
 *
 * 为什么只降**软件渲染**（实测，1200×788 内容区里拖一个 1100×640 的应用窗口）：
 *   · 软件光栅（`--disable-gpu` → Microsoft Basic Render Driver）：4.7 fps；
 *     去掉背景模糊 + will-change 后 19.8 fps（约 4 倍），再加上不透明背景 21.3 fps；
 *   · 有 GPU 加速的机器上同样改动**零收益**（Quadro 2000：59.4 vs 58 fps，都贴着 vsync）。
 * 所以绝不能无条件降级——那只会白白丢掉上游的视觉效果。
 *
 * 判据只认**明确的软件渲染器名字**。拿不到 WebGL（被企业策略/启动参数禁用等）时
 * **不降级**：宁可保留效果，也不要把「WebGL 被关掉」误判成「这台机器跑不动」。
 *
 * 诊断口：`window.__FNOS_PERF__` = { renderer, softwareRender, degraded }。
 */
(function () {
  'use strict';

  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document;
  if (!D) return;

  // 只对顶层文档生效（与 windowpos.js 同一纪律：iframe 里没有应用窗口）。
  var isTopFrame = false;
  try { isTopFrame = W.top === W.self; } catch (e) { isTopFrame = false; }
  if (!isTopFrame) return;

  /** 软件光栅的渲染器名（Chromium/ANGLE 的取值）。只认这些**明确的**名字。 */
  var SOFTWARE_RENDERER_RE = /swiftshader|basic render|llvmpipe|softpipe/i;

  /** 降级样式：只动上游那几处最贵的合成属性，别的一概不碰。 */
  var DEGRADED_CSS_ID = 'fnos-shell-software-render-style';
  var ROOT_CLASS = 'fnos-software-render';
  var DEGRADED_CSS =
    /* 动画里的 filter: blur(...) 与动画期背景模糊：按上游自己的
     * 「窗口动画模糊」关掉时的同一组变量归零（语义一致，不引入新观感）。 */
    ':root.' + ROOT_CLASS + ' {' +
    '--fnos-window-motion-backdrop-start-blur: 0px;' +
    '--fnos-window-motion-backdrop-end-blur: 0px;' +
    '--fnos-window-enter-filter-start-blur: 0px;' +
    '--fnos-window-enter-filter-end-blur: 0px;' +
    '--fnos-window-exit-close-filter-start-blur: 0px;' +
    '--fnos-window-exit-close-filter-end-blur: 0px;' +
    '--fnos-window-exit-minimize-filter-start-blur: 0px;' +
    '--fnos-window-exit-minimize-filter-end-blur: 0px;' +
    '--fnos-window-restore-filter-start-blur: 0px;' +
    '--fnos-window-restore-filter-end-blur: 0px;' +
    '}' +
    /* 静态 32px 背景模糊：软件光栅下这是每帧最贵的一笔（拖动窗口时整块重算）。 */
    ':root.' + ROOT_CLASS + ' .trim-ui__app-layout--window {' +
    'backdrop-filter: none !important;' +
    'will-change: auto !important;' +
    '}' +
    /* 模态遮罩的 6px 背景模糊：全屏面积，软件光栅下同样昂贵。 */
    ':root.' + ROOT_CLASS + ' #root .semi-modal-mask {' +
    'backdrop-filter: none !important;' +
    '}';

  /**
   * 读 WebGL 的渲染器名。任何一步失败都返回空串（= 不知道，不降级）。
   * 探完立刻 `loseContext()`：这个 canvas 只用于问一句话，不该留下一个常驻上下文。
   */
  function probeRenderer() {
    try {
      if (typeof D.createElement !== 'function') return '';
      var canvas = D.createElement('canvas');
      if (!canvas || typeof canvas.getContext !== 'function') return '';
      var gl = canvas.getContext('webgl') || canvas.getContext('experimental-webgl');
      if (!gl || typeof gl.getParameter !== 'function') return '';
      var renderer = '';
      try {
        var info = typeof gl.getExtension === 'function'
          ? gl.getExtension('WEBGL_debug_renderer_info')
          : null;
        if (info) renderer = String(gl.getParameter(info.UNMASKED_RENDERER_WEBGL) || '');
      } catch (e) { /* 扩展被拦：拿不到名字就按「不知道」处理 */ }
      try {
        var lose = typeof gl.getExtension === 'function' ? gl.getExtension('WEBGL_lose_context') : null;
        if (lose && typeof lose.loseContext === 'function') lose.loseContext();
      } catch (e) { /* 释放失败不影响结论 */ }
      return renderer;
    } catch (e) {
      return '';
    }
  }

  var renderer = probeRenderer();
  var softwareRender = SOFTWARE_RENDERER_RE.test(renderer);
  var state = {
    renderer: renderer,
    softwareRender: softwareRender,
    degraded: false,
  };
  try { W.__FNOS_PERF__ = state; } catch (e) { /* 诊断口装不上不影响功能 */ }

  if (!softwareRender) return;

  /** 落样式与根 class（documentElement 一出现就做，赶在桌面首帧之前）。 */
  function applyDegraded() {
    var root = D.documentElement;
    if (!root) return false;
    if (state.degraded) return true;
    try {
      if (typeof root.classList === 'object' && root.classList) {
        root.classList.add(ROOT_CLASS);
      } else if (typeof root.className === 'string') {
        root.className = (root.className ? root.className + ' ' : '') + ROOT_CLASS;
      }
    } catch (e) { /* class 装不上：样式里还有 :root 选择器兜底 */ }
    var existing = null;
    try { existing = D.getElementById(DEGRADED_CSS_ID); } catch (e) { existing = null; }
    if (!existing) {
      try {
        var style = D.createElement('style');
        if (!style) return false;
        style.id = DEGRADED_CSS_ID;
        style.textContent = DEGRADED_CSS;
        (D.head || root).appendChild(style);
      } catch (e) {
        return false;
      }
    }
    state.degraded = true;
    return true;
  }

  if (applyDegraded()) return;

  // documentElement 还不存在（与上游包装壳同一时机问题）：观察 document 本身。
  if (typeof W.MutationObserver === 'function') {
    try {
      var mo = new W.MutationObserver(function () {
        if (!D.documentElement) return;
        if (mo) { try { mo.disconnect(); } catch (e) { /* 忽略 */ } }
        applyDegraded();
      });
      mo.observe(D, { childList: true, subtree: true });
    } catch (e) { /* 观察器装不上 → 下面的 DOMContentLoaded 兜底 */ }
  }
  try {
    D.addEventListener('DOMContentLoaded', applyDegraded);
  } catch (e) { /* 连兜底都装不上就放弃：宁可保留效果也不抛错 */ }
})();
