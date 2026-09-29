/* fnOS Desktop Shell — 桌面内窗口默认居中（T14c 修复轮 15）
 *
 * 做什么：fnOS 自己的「应用窗口」是**桌面文档里的 DOM 元素**（`.trim-ui__app-layout--window`，
 * 绝对定位 + 内联 left/top），它的窗口管理器默认按**级联**摆位（每开一个往右下偏一点）。
 * 实测用户机器上：桌面 1200×853 里，1100×640 的文件管理窗口落在 (77,138)，而居中应是 (50,106)。
 * 用户要求「打开应用窗口默认应用内居中显示」——本文件就是这件事。
 *
 * 边界与纪律：
 * - **只认 fnOS 自己的窗口类**（`.trim-ui__app-layout--window`），不碰别的元素；
 * - 只对**刚创建**的窗口动手：写一次内联 left/top（居中值），随后**不再干预**——用户拖动
 *   窗口（窗口管理器改写内联 left/top）之后我们绝不去拉回来；
 * - 跳过铺满内容区的窗口（最大化形态，宽 ≈ 内容区宽）与还没布局完的窗口（尺寸为 0）；
 * - 不做 `!important`：普通内联写入即可，谁后写谁生效——这样窗口管理器与用户的操作都仍然优先。
 */
(function () {
  'use strict';

  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document;

  var isTopFrame = false;
  try { isTopFrame = W.top === W.self; } catch (e) { isTopFrame = false; }
  if (!isTopFrame) return;

  // fnOS 应用窗口的类名（实测：`w-[1100px] h-[640px] absolute flex flex-col … trim-ui__app-layout--window`）。
  // 不写死尺寸类：窗口大小可变（用户可拖边缘），居中按**实测尺寸**算。
  var WINDOW_SELECTOR = '.trim-ui__app-layout--window';
  var SETTLE_MS = 160;   // 首次居中后的一次补正（窗口管理器可能在同一帧后改写内联 left/top）
  var SCAN_DELAY_MS = 120; // 观察回调的合并窗口

  var placed = new WeakSet();  // 已处理过的窗口（只居中一次）
  var scanScheduled = false;

  /** 把一个窗口摆到内容区正中（内容区 = 窗口的父容器，也就是它的包含块）。 */
  function centerWindow(el) {
    if (!el || !el.isConnected) return;
    var area = el.parentElement;
    if (!area) return;
    var availW = area.clientWidth || 0;
    var availH = area.clientHeight || 0;
    var w = el.offsetWidth || 0;
    var h = el.offsetHeight || 0;
    if (!(availW > 0 && availH > 0 && w > 0 && h > 0)) return false;
    // 铺满内容区 = 最大化形态：不动它（那条路径由本壳的样式覆盖处理）
    if (w >= availW - 4 && h >= availH - 4) return true;
    el.style.left = Math.max(0, Math.round((availW - w) / 2)) + 'px';
    el.style.top = Math.max(0, Math.round((availH - h) / 2)) + 'px';
    return true;
  }

  /** 给一个刚出现的窗口排两次居中：下一帧一次（布局已就绪），[`SETTLE_MS`] 后再补一次。 */
  function place(el) {
    if (placed.has(el)) return;
    placed.add(el);
    var attempt = function () { centerWindow(el); };
    if (typeof W.requestAnimationFrame === 'function') W.requestAnimationFrame(attempt);
    else attempt();
    W.setTimeout(attempt, SETTLE_MS);
  }

  function scan() {
    scanScheduled = false;
    var nodes;
    try { nodes = D.querySelectorAll(WINDOW_SELECTOR); } catch (e) { return; }
    for (var i = 0; i < nodes.length; i++) place(nodes[i]);
  }

  function scheduleScan() {
    if (scanScheduled) return;
    scanScheduled = true;
    W.setTimeout(scan, SCAN_DELAY_MS);
  }

  // 新窗口出现（SPA 挂载）→ 合并扫描一次
  if (typeof W.MutationObserver === 'function') {
    try {
      new W.MutationObserver(scheduleScan).observe(D, { childList: true, subtree: true });
    } catch (e) { /* 观察装不上就算了：下面的首次扫描仍然有效 */ }
  }
  // 首次扫描：页面加载时已经存在的窗口（例如上次会话留下的）也一并居中
  if (D.readyState === 'loading') D.addEventListener('DOMContentLoaded', scheduleScan);
  else scheduleScan();

  /** 诊断口（控制台可见，页面上不画任何东西）。 */
  W.__FNOS_WINDOWPOS_STATE__ = function () {
    var nodes = [];
    try { nodes = D.querySelectorAll(WINDOW_SELECTOR); } catch (e) { /* 忽略 */ }
    return {
      windows: nodes.length,
      positions: [].slice.call(nodes).map(function (el) {
        return { left: el.style.left || '', top: el.style.top || '', w: el.offsetWidth, h: el.offsetHeight };
      }),
    };
  };
})();
