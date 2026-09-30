/* fnOS Desktop Shell — 桌面内窗口默认居中 / 尺寸变化后保持居中（T14c 修复轮 15～18）
 *
 * 做什么：fnOS 自己的「应用窗口」是**桌面文档里的 DOM 元素**（`.trim-ui__app-layout--window`，
 * 绝对定位 + 内联 left/top），它的窗口管理器默认按**级联**摆位（每开一个往右下偏一点）。
 * 实测用户机器上：内容区 1200×820 里，1100×640 的文件管理窗口落在 (75,113)，而居中应是 (50,90)。
 * 本文件负责四件事：
 *   ① 新窗口默认居中（用户要求）；
 *   ② 外壳程序窗口在 Windows 上最大化/还原（= 内容区尺寸变化）后**保持居中**（用户要求）；
 *   ③ 桌面内窗口「最大化 ⇄ 窗口」往返后仍然居中（用户要求）；
 *   ④ 只做这些，绝不干预用户拖动/缩放（除非这几种形态切换本身）。
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
 * 内容区尺寸变化（修复轮 18：用户要求「程序最大化/还原时，程序内打开的应用窗口保持居中」）：
 *   实测：外壳窗口最大化（1200×820 → 1920×1009）时，fnOS 自己会改写窗口位置（50,90 → 165,202，
 *   而且不是居中），还原后也停在偏心位置。所以本文件订阅 `resize`：按每个窗口**相对内容区中心
 *   的偏移比例**重算位置 —— 偏移比例 0（居中）的窗口在新尺寸下仍是正中（这正是用户要的），
 *   被用户拖到一边的窗口则保持同样的相对位置（不会被硬拉回中间）。
 *   比例是**记在状态里的**（不是从当前位置现算）：fnOS 可能先于/后于我们写位置，从当前位置现算
 *   会把它的值当成人家的意图。落位时记 0，用户在窗口形态下拖动时（内容区尺寸没变、位置变了）
 *   才更新比例。落位后还有一次 [`REFLOW_SETTLE_MS`] 补正，兜住「fnOS 晚于我们写位置」。
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
 * - 普通内联写入（不做 `!important`、也不抢样式优先级）：谁后写谁生效，窗口管理器与用户的
 *   操作仍然优先；
 * - 跳过铺满内容区的窗口（最大化形态）；
 * - 顶层文档才工作。
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
  var SETTLE_MS = 160;        // 落位后的补正窗口（覆盖「窗口管理器稍后改写」的版本差异）
  var REFLOW_SETTLE_MS = 160; // 内容区尺寸变化后的补正窗口（fnOS 可能晚于我们写位置）

  var placed = new WeakSet(); // 已处理过的窗口（只居中一次，之后交给窗口管理器与用户）
  var held = [];              // 尺寸还没量到、暂时按住的窗口（摆正即放开）
  var tracked = [];           // 已接管形态观察 / 比例跟随的窗口状态
  var reflowScheduled = false;
  var reflowTimer = null;

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

  /** 内容区与窗口的实测几何（量不到时返回 null）。 */
  function measure(el) {
    var area = el && el.parentElement;
    if (!area) return null;
    var availW = area.clientWidth || 0;
    var availH = area.clientHeight || 0;
    var w = el.offsetWidth || 0;
    var h = el.offsetHeight || 0;
    if (!(availW > 0 && availH > 0 && w > 0 && h > 0)) return null;
    return { availW: availW, availH: availH, w: w, h: h };
  }

  /** 比例夹取：0 = 正中，±1 = 贴到内容区边缘（再往外没有意义）。 */
  function clampRatio(v) {
    if (!isFinite(v)) return 0;
    if (v > 1) return 1;
    if (v < -1) return -1;
    return v;
  }

  /**
   * 写位置并记账（记下我们写的字面值：属性观察里要能分辨「这是我们自己写的」）。
   * 不夹取到 0 以上会写负数 left —— 那不是合法布局值，按 0 处理更符合「贴边」的直觉。
   */
  function writePos(st, el, left, top) {
    var l = Math.max(0, Math.round(left)) + 'px';
    var t = Math.max(0, Math.round(top)) + 'px';
    el.style.left = l;
    el.style.top = t;
    if (st) {
      st.wroteLeft = l;
      st.wroteTop = t;
    }
  }

  /**
   * 把一个窗口摆到内容区正中，并把它的「中心偏移比例」归零（= 用户要的居中语义）。
   * 比例记录在窗口状态上，供内容区尺寸变化时按比例跟随。
   */
  function centerWindow(el) {
    if (!el || !el.isConnected) return false;
    var m = measure(el);
    if (!m) return false;
    // 铺满内容区 = 最大化形态：不动它（那条路径由本壳的样式覆盖处理）
    if (m.w >= m.availW - 4 && m.h >= m.availH - 4) return true;
    var st = stateOf(el);
    writePos(st, el, (m.availW - m.w) / 2, (m.availH - m.h) / 2);
    if (st) {
      st.ratioX = 0;
      st.ratioY = 0;
      st.areaW = m.availW;
      st.areaH = m.availH;
    }
    return true;
  }

  /**
   * 按记录的比例把窗口摆到**新尺寸**的内容区里：比例 0 → 正中，±1 → 贴边。
   * 公式 `新中心 = 新区半宽 + 比例 × 新区半宽`（等价于 `新区半宽 × (1 + 比例)`），
   * 再减半个窗口尺寸得到 left —— 居中窗口（比例 0）在新尺寸下仍是正中。
   */
  function applyRatio(st, el, m) {
    writePos(st, el,
      m.availW / 2 + st.ratioX * (m.availW / 2) - m.w / 2,
      m.availH / 2 + st.ratioY * (m.availH / 2) - m.h / 2);
    st.areaW = m.availW;
    st.areaH = m.availH;
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
    trackWindow(el); // 形态观察 + 比例跟随（见文件头）
    if (typeof W.requestAnimationFrame === 'function') {
      W.requestAnimationFrame(function () { attempt(el); });
    }
    W.setTimeout(function () { attempt(el); }, SETTLE_MS);
  }

  // ---------- 逐窗口状态：形态（最大化往返）+ 中心偏移比例（尺寸变化跟随） ----------

  function stateOf(el) {
    return (el && el.__fnosPosState) || null;
  }

  function stopTrack(st) {
    var i = tracked.indexOf(st);
    if (i >= 0) tracked.splice(i, 1);
    try { if (st.obs) st.obs.disconnect(); } catch (e) { /* 忽略 */ }
    st.obs = null;
  }

  /** 给一个窗口挂状态与属性观察（只观察它自己，且只关心 class/style）。 */
  function trackWindow(el) {
    if (!el || stateOf(el)) return;
    var m = measure(el);
    var st = {
      el: el,
      obs: null,
      wasMax: isMaximized(el),
      ratioX: 0,          // 默认按居中记账（place 已同步落位）
      ratioY: 0,
      areaW: m ? m.availW : 0,
      areaH: m ? m.availH : 0,
      wroteLeft: el.style.left || '',
      wroteTop: el.style.top || '',
    };
    try { el.__fnosPosState = st; } catch (e) { return; }
    tracked.push(st);
    if (typeof W.MutationObserver !== 'function') return; // 老内核：退化为只在创建时居中
    try {
      st.obs = new W.MutationObserver(function () { onWindowAttrs(st); });
      st.obs.observe(el, { attributes: true, attributeFilter: ['class', 'style'] });
    } catch (e) { st.obs = null; }
  }

  /**
   * 窗口自己的 class/style 变了。两种情况：
   *   ① 形态切换（铺满 ⇄ 窗口）→ 进最大化记一笔，回到窗口形态**重新居中**；
   *   ② 内容区尺寸没变、位置变了 → 那是用户在拖窗口，把「中心偏移比例」记下来，
   *      好让之后内容区尺寸变化时它还在同一个相对位置。
   * 内容区尺寸变了（例如外壳窗口正在最大化）时**不**更新比例：那可能是 fnOS 自己写的，
   * 不是用户的意图。
   */
  function onWindowAttrs(st) {
    var el = st.el;
    if (!el || !el.isConnected) { stopTrack(st); return; }
    if (isMaximized(el)) { st.wasMax = true; return; }
    if (st.wasMax) { st.wasMax = false; centerWindow(el); return; }
    // 我们自己写的那一次不构成「用户的拖动」（否则会反复把自己写的值当意图记账）
    if (el.style.left === st.wroteLeft && el.style.top === st.wroteTop) return;
    var m = measure(el);
    if (!m) return;
    if (st.areaW > 0 && st.areaH > 0 && (st.areaW !== m.availW || st.areaH !== m.availH)) return;
    var left = parseFloat(el.style.left);
    var top = parseFloat(el.style.top);
    if (isFinite(left) && isFinite(top)) {
      st.ratioX = clampRatio((left + m.w / 2 - m.availW / 2) / (m.availW / 2));
      st.ratioY = clampRatio((top + m.h / 2 - m.availH / 2) / (m.availH / 2));
    }
  }

  // ---------- 内容区尺寸变化（外壳窗口最大化/还原、拖边缘改大小…） ----------

  /** 立即按比例把受管窗口摆到当前内容区里（最大化形态的窗口不动）。 */
  function reflow() {
    reflowScheduled = false;
    for (var i = tracked.length - 1; i >= 0; i--) {
      var st = tracked[i];
      var el = st.el;
      if (!el || !el.isConnected) { stopTrack(st); continue; }
      var m = measure(el);
      if (!m) continue;
      if (m.w >= m.availW - 4 && m.h >= m.availH - 4) continue; // 最大化形态：由样式铺满
      applyRatio(st, el, m);
    }
  }

  /**
   * 内容区尺寸变化：先在本帧按比例摆好，再在 [`REFLOW_SETTLE_MS`] 后补正一次。
   * 为什么要有补正：fnOS 自己也会在尺寸变化时改写窗口位置（实测外壳最大化时它把
   * (50,90) 改成 (165,202)），谁先谁后不确定；补正那一次用的是**记下来的比例**，
   * 不读当前位置，所以重复执行结果恒定（既不会漂移，也不会把 fnOS 的值当意图）。
   */
  function onAreaResize() {
    if (reflowScheduled) return;
    reflowScheduled = true;
    if (typeof W.requestAnimationFrame === 'function') W.requestAnimationFrame(reflow);
    else W.setTimeout(reflow, 16);
    if (reflowTimer) W.clearTimeout(reflowTimer);
    reflowTimer = W.setTimeout(function () {
      reflowTimer = null;
      reflow();
    }, REFLOW_SETTLE_MS);
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
  W.addEventListener('resize', onAreaResize); // 外壳窗口最大化/还原、拖边缘改大小
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
        var st = stateOf(el);
        return {
          left: el.style.left || '', top: el.style.top || '',
          w: el.offsetWidth, h: el.offsetHeight,
          visibility: (el.style && el.style.visibility) || '',
          maximized: isMaximized(el),
          ratio: st ? [Number(st.ratioX.toFixed(3)), Number(st.ratioY.toFixed(3))] : null,
          area: st ? [st.areaW, st.areaH] : null,
        };
      }),
    };
  };
})();
