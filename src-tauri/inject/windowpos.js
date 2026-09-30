/* fnOS Desktop Shell — 桌面内窗口默认居中（T14c 修复轮 16）
 *
 * 做什么：fnOS 自己的「应用窗口」是**桌面文档里的 DOM 元素**（`.trim-ui__app-layout--window`，
 * 绝对定位 + 内联 left/top），它的窗口管理器默认按**级联**摆位（每开一个往右下偏一点）。
 * 实测用户机器上：桌面 1200×820 里，1100×640 的文件管理窗口落在 (75,113)，而居中应是 (50,90)。
 * 用户要求「打开应用窗口默认应用内居中显示」——本文件就是这件事。
 *
 * 时机（修复轮 16：用户反馈「会跳动到居中，而不是一开始就居中」）：
 *   窗口插入 DOM 时**内联 left/top 就已经是级联值**，并且立刻开始约 400ms 的入场动画
 *   （实测 opacity 0→1、scale 0.96→1、translateY 18px→0）。早先版本等 120ms 合并窗 + 一帧
 *   rAF 才落位，那会儿窗口已经淡入到 opacity 0.24～0.87 —— 用户看到的就是「先出现在偏右下的
 *   位置、再跳一下」。所以现在：
 *     · MutationObserver 回调里**同步**落位。回调是插入那一个任务里的微任务，DOM 已进树、
 *       样式/布局尚未绘制，写下的 left/top 就是用户看到的第一眼（不存在中间帧）；
 *     · 尺寸一时量不到（父容器还没布局）就先 `visibility:hidden` 按住，等量到了再摆正并放它
 *       出来 —— 宁可晚一帧出现，也不要先出现在错的位置；
 *     · 之后再补两次幂等重试（下一帧 + [`SETTLE_MS`]），兜住「窗口管理器稍后改写尺寸/位置」
 *       的版本差异；值没变时是空写，用户看不见。
 *
 * 最大化往返（修复轮 17：用户要求「窗口→最大化→窗口 依然保持居中」）：
 *   fnOS 还原时会把「最大化前的位置」原样还回来，而那个位置未必居中——用户拖过/缩放过窗口
 *   之后就不居中了（缩放只改尺寸不改 left/top，原本居中的窗口一缩放就偏了）。所以给每个
 *   已接管的窗口挂一个只盯它自己的属性观察：进了「铺满内容区」形态就记一笔（wasMax），
 *   之后第一次回到窗口形态**重新居中一次**。用户在窗口形态下的拖动/缩放不构成形态切换，
 *   因此不会被拉回来（只居中一次的语义仍然成立）。
 *
 * 边界与纪律：
 * - **只认 fnOS 自己的窗口类**，不碰别的元素；
 * - 只对**刚创建**的窗口动手：写一次内联 left/top（居中值），随后**不再干预**——用户拖动
 *   窗口之后我们绝不去拉回来（两次重试都在 [`SETTLE_MS`] = 160ms 内，人来不及拖）；
 * - 跳过铺满内容区的窗口（最大化形态）；
 * - 不做 `!important`、也不抢样式优先级：普通内联写入即可，谁后写谁生效——
 *   这样窗口管理器与用户的操作都仍然优先。
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
  var SETTLE_MS = 160; // 落位后的补正窗口（覆盖「窗口管理器稍后改写」的版本差异）

  var placed = new WeakSet(); // 已处理过的窗口（只居中一次，之后交给窗口管理器与用户）
  var held = [];              // 尺寸还没量到、暂时按住的窗口（摆正即放开）
  var tracked = [];           // 已接管「最大化 ⇄ 窗口」往返的窗口（{el, obs, wasMax}）

  /** 窗口此刻是不是「铺满内容区」= 最大化形态（内容区 = 包含块，也就是它的父容器）。 */
  function isMaximized(el) {
    var area = el && el.parentElement;
    if (!area) return false;
    var availW = area.clientWidth || 0;
    var availH = area.clientHeight || 0;
    var w = el.offsetWidth || 0;
    var h = el.offsetHeight || 0;
    if (!(availW > 0 && availH > 0 && w > 0 && h > 0)) return false;
    return w >= availW - 4 && h >= availH - 4;
  }

  /** 把一个窗口摆到内容区正中（内容区 = 窗口的父容器，也就是它的包含块）。 */
  function centerWindow(el) {
    if (!el || !el.isConnected) return false;
    var area = el.parentElement;
    if (!area) return false;
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

  /**
   * 从最大化**还原**成窗口时重新居中（用户要求：窗口→最大化→窗口 往返后仍然居中）。
   *
   * 为什么需要它：fnOS 会把「最大化前的位置」原样还回来，而那个位置未必居中——
   * 用户拖过/缩放过窗口之后就不居中了（缩放只改尺寸不改 left/top，于是原本居中的窗口
   * 一缩放就偏了），此后「最大化→还原」只会把这个偏心位置再还回来。
   *
   * 边界：只认**形态切换**这一件事——最大化形态（铺满内容区）记一笔，之后第一次回到
   * 窗口形态就居中一次。用户在窗口形态下的拖动/缩放只改 class/style，不构成切换，
   * 因此**不会**被本函数拉回来（那由 `placed` 的一次性语义保证）。
   */
  function onWindowAttrs(state) {
    var el = state.el;
    if (!el || !el.isConnected) { stopTrack(state); return; }
    if (isMaximized(el)) { state.wasMax = true; return; }
    if (!state.wasMax) return; // 没经历过最大化形态：窗口形态下的一切改动都不干预
    state.wasMax = false;
    centerWindow(el);
  }

  function stopTrack(state) {
    var i = tracked.indexOf(state);
    if (i >= 0) tracked.splice(i, 1);
    try { if (state.obs) state.obs.disconnect(); } catch (e) { /* 忽略 */ }
    state.obs = null;
  }

  /** 给一个窗口挂上「class/style 变更」观察（只观察它自己，属性名限定 class/style）。 */
  function trackWindow(el) {
    if (!el || el.__fnosPosTracked) return;
    try { el.__fnosPosTracked = true; } catch (e) { return; }
    var state = { el: el, obs: null, wasMax: isMaximized(el) };
    tracked.push(state);
    if (typeof W.MutationObserver !== 'function') return; // 老内核：退化为只在创建时居中
    try {
      state.obs = new W.MutationObserver(function () { onWindowAttrs(state); });
      state.obs.observe(el, { attributes: true, attributeFilter: ['class', 'style'] });
    } catch (e) { state.obs = null; }
  }

  /** 按住（尺寸未知时先用 visibility 挡住，免得它以级联位置先露一脸）。 */
  function hold(el) {
    if (held.indexOf(el) >= 0) return;
    held.push(el);
    try { el.style.visibility = 'hidden'; } catch (e) { /* 不支持就算了 */ }
  }

  /** 放开被按住的窗口（只有按住过的才动，不覆盖页面自己的 visibility）。 */
  function release(el) {
    var i = held.indexOf(el);
    if (i < 0) return;
    held.splice(i, 1);
    try { el.style.removeProperty('visibility'); } catch (e) { /* 忽略 */ }
  }

  /** 一次尝试：量得到就摆正 + 放开；量不到就按住等下一次。 */
  function attempt(el) {
    if (!el || !el.isConnected) { release(el); return; }
    if (centerWindow(el)) release(el);
    else hold(el);
  }

  /** 给一个刚出现的窗口排三次尝试：**同步**（插入与首帧之间）→ 下一帧 → [`SETTLE_MS`] 后。 */
  function place(el) {
    if (placed.has(el)) return;
    placed.add(el);
    attempt(el);
    // 挂上「最大化 ⇄ 窗口」往返的观察（见 onWindowAttrs）：还原时再居中一次
    trackWindow(el);
    if (typeof W.requestAnimationFrame === 'function') {
      W.requestAnimationFrame(function () { attempt(el); });
    }
    W.setTimeout(function () { attempt(el); }, SETTLE_MS);
  }

  function pick(root) {
    var out = [];
    try {
      if (root.matches && root.matches(WINDOW_SELECTOR)) out.push(root);
    } catch (e) { /* matches 不可用就算了 */ }
    try {
      var inner = root.querySelectorAll ? root.querySelectorAll(WINDOW_SELECTOR) : [];
      for (var i = 0; i < inner.length; i++) out.push(inner[i]);
    } catch (e) { /* 忽略 */ }
    return out;
  }

  /**
   * 观察回调：**只处理本轮新增的节点**，且**同步**落位（见文件头的时机说明）。
   * 不做「延迟合并扫描」——那正是「先出现在级联位置再跳一下」的来源。
   */
  function onMutations(records) {
    for (var i = 0; i < records.length; i++) {
      var added = records[i].addedNodes || [];
      for (var j = 0; j < added.length; j++) {
        var node = added[j];
        if (!node || node.nodeType !== 1) continue; // 3 = 文本节点
        var hits = pick(node);
        for (var k = 0; k < hits.length; k++) place(hits[k]);
      }
    }
  }

  /** 页面加载时就已存在的窗口（上次会话留下的）也一并居中。 */
  function scan() {
    var nodes = [];
    try { nodes = D.querySelectorAll(WINDOW_SELECTOR); } catch (e) { return; }
    for (var i = 0; i < nodes.length; i++) place(nodes[i]);
  }

  if (typeof W.MutationObserver === 'function') {
    try {
      new W.MutationObserver(onMutations).observe(D, { childList: true, subtree: true });
    } catch (e) { /* 观察装不上就算了：下面的首次扫描仍然有效 */ }
  }
  if (D.readyState === 'loading') D.addEventListener('DOMContentLoaded', scan);
  else scan();

  /** 诊断口（控制台可见，页面上不画任何东西）。 */
  W.__FNOS_WINDOWPOS_STATE__ = function () {
    var nodes = [];
    try { nodes = D.querySelectorAll(WINDOW_SELECTOR); } catch (e) { /* 忽略 */ }
    return {
      windows: nodes.length,
      held: held.length,
      tracked: tracked.length,
      positions: [].slice.call(nodes).map(function (el) {
        return {
          left: el.style.left || '', top: el.style.top || '',
          w: el.offsetWidth, h: el.offsetHeight,
          visibility: (el.style && el.style.visibility) || '',
          maximized: isMaximized(el),
        };
      }),
    };
  };
})();
