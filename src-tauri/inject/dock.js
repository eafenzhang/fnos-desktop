/* fnOS Desktop Shell — Dock 自动隐藏（T14c，本壳自己的页面功能，不是上游 mod）
 *
 * 做什么：把 fnOS WebUI 的 Dock（任务栏）平时滑出屏幕边缘，鼠标顶到它所在的边缘时滑回，
 * 并把 Dock 让出来的那条宽度**还给桌面与窗口**。
 *
 * 定位（多策略，全部锚定上游已知特征）：
 *   S1 上游类名策略   —— 逐字复用上游 mod.js 的 TASKBAR_ROOT_SELECTOR（+ TASKBAR_LIST_SELECTOR
 *                        验证），命中即用。
 *   S2 上游任务栏项策略 —— 用上游的 TASKBAR_ITEM_SELECTOR 找到任务栏图标项，从它的父层沿
 *                        祖先向上爬到最外层「Dock 形状」的容器（窄长条）。
 *   S3 贴边容器策略   —— Tailwind `.fixed` 元素里，贴屏幕边缘 + Dock 形状 + 含 ≥3 个图标。
 *   S4 全量扫描策略   —— S3 失败后的兜底：扫 `body *` 的计算样式 position:fixed（贵，
 *                        只在需要时做一次；locate 本就有 150ms 合并窗）。
 *   找不到            —— 静默（不向页面画任何提示；状态可在控制台用 `__FNOS_DOCK_STATE__()`
 *                        读，那里也带着各策略的候选计数）。
 *
 * 只在**顶层文档**工作：注入脚本会跑进每一个 iframe（fnOS 的应用窗口就是 iframe），
 * 子框架里既没有 Dock，也可能把应用自己的侧栏误判成 Dock 藏掉（T14c 修复轮 7）。
 *
 * 显隐规则：
 *   唤出   —— 指针顶到 Dock 所在的屏幕边缘热区（EDGE_PX）。**不是**「进入 Dock 的脚印」：
 *            藏在 Dock 底下的应用按钮就在那条带里，一靠近就唤出会把要点的按钮盖住。
 *   保持   —— 唤出后指针留在 Dock 脚印（自身尺寸 + 余量）内就一直显示，离开即藏（迟滞）。
 *   藏起   —— 指针离开脚印 / 离开窗口 / 窗口失焦 / **无操作超过 IDLE_MS** / 接管时。
 *            指针进入 iframe（应用窗口）之后顶层文档收不到 pointermove——纯事件驱动的
 *            显隐会卡在「显示」，所以必须有 idle 兜底。
 *   离场   —— 滑出动画结束后 `display:none`：transform 藏得住视觉，藏不住按 rect 做的
 *            工作区计算。
 *
 * 空间回收（T14c 修复轮 7～8）：只把 Dock 藏起来**不等于**把左侧那条预留宽度还回来——实测
 * 「桌面与应用全屏左侧仍有空白」。在用户的 fnOS 上实测到**两条**互不相同的机制，本层分别中和
 * （都只在识别到「预留宽度」时动手，teardown 时逐条还原）：
 *   ① 布局容器用**内边距**预留：桌面根 `<div class="relative flex …">` 里，内容区
 *      `<div class="… pl-[66px]">` 把 Dock 宽写成左内边距 → 中和该内边距（含 margin / grid
 *      首轨两种同族变体）。它同时修正了桌面图标区与一切流内内容。
 *   ② 应用窗口用**工具类**预留：fnOS 的窗口管理器给「最大化到工作区」的窗口加
 *      `!left-[66px]` / `!right-0` / `!w-[calc(100%-66px)]`（均 `!important`，66 = Dock 宽，
 *      三处同数）。绝对定位元素不吃父级 padding，所以 ① 对窗口无效，必须按同一签名覆盖
 *      `left`/`width`（窗口回到自由尺寸时类消失，覆盖随即撤回）。
 *   两种机制的宽度都以**类名/计算样式里读到的实际预留值**为准，不写死 66。
 *
 * 开关链路：
 *   初始态   —— 注入载荷 `__FNOS_SHELL__.shell.dockAutoHide`（injector.rs 只发页面消费的键）
 *   免刷新态 —— shim 的 `__FNOS_APPLY_CONFIG__` 收到 `patch.shell` 时转调
 *               `__FNOS_APPLY_SHELL__(patch.shell)`（本文件定义；shim 不解释 shell 键语义）
 * 配置侧默认关（`config.rs::ShellConfig::default`），且注入总开关关闭时本文件根本不会被注入。
 */
(function () {
  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document;

  // 只在顶层文档工作（见文件头）。跨源访问 top 会抛异常 —— 那就是子框架。
  var isTopFrame = false;
  try { isTopFrame = W.top === W.self; } catch (e) { isTopFrame = false; }
  if (!isTopFrame) return;

  var SHELL = W.__FNOS_SHELL__ || {};
  var SHELL_CFG = SHELL.shell || {};

  // 以下三条选择器**逐字取自上游 mod.js**（TASKBAR_ROOT_SELECTOR / TASKBAR_ITEM_SELECTOR /
  // TASKBAR_LIST_SELECTOR）。改这里必须连上游那份一起核对，tests/dock.test.mjs 会把两边
  // 源码逐字比对。
  var DOCK_ROOT_SELECTOR = '.h-screen.fixed.left-0';
  var DOCK_ITEM_SELECTOR =
    '.flex.h-10.w-\\[47px\\].items-center.justify-center.gap-x-2.border-0.\\!border-l-\\[3px\\].border-solid.border-transparent.hover\\:bg-white-10';
  var DOCK_LIST_SELECTOR =
    '.scrollbar-hidden.absolute.inset-0.flex.flex-col.items-end.justify-start.gap-2.overflow-y-auto.pt-2';

  var HOST_CLASS = 'fnos-shell-dock-autohide';   // 常驻：宣告「本壳在管这个元素」
  var HIDDEN_CLASS = 'fnos-shell-dock-hidden';   // 状态：滑出屏幕（随后 display:none 离场）
  var STYLE_ID = 'fnos-shell-dock-style';
  var EDGE_PX = 6;           // 屏幕边缘热区宽度（唤出条件；比 Dock 厚度窄得多）
  var SLACK_PX = 12;         // 「还在 Dock 脚印内」判定的余量（指针抖动 + 边框）
  var LOCATE_DELAY_MS = 150; // SPA 重渲染的合并窗口：一批变更只查一次选择器
  var TRANSITION_MS = 320;   // 与 CSS 的 .28s 过渡对齐（略留余量）；到点后真正 display:none
  var IDLE_MS = 3000;        // 「无操作」判定：这么久没有任何指针事件就藏
  var DOCK_SIDE_MAX = 140;   // 「Dock 形状」的窄边上限 / 长边下限（像素）
  var STRIP_MIN = 24;        // 预留宽度识别区间：窄于此不值得动（不是 Dock 栏宽度）
  var STRIP_MAX = 200;       // 宽于此不是 Dock 栏（可能是内容区），一律不动手
  var LAYOUT_PASS_MIN_MS = 1500; // 布局预留扫描的节流间隔（覆盖一旦写上就一直在）

  var enabled = SHELL_CFG.dockAutoHide === true;
  var dock = null;       // 当前接管的 Dock 本体（可能被 SPA 换掉，observer 会重新找）
  var targets = [];      // 接管元素集合：第 0 项是 Dock 本体，之后是承载它的窄列
  var reclaimed = [];    // 被中和的预留（{el, prop, prev}，prev = 原内联值，'' = 原本没有）
  var observer = null;   // MutationObserver：Dock 出现 / 被替换时重新接管
  var locateScheduled = false;
  var axis = 'x';        // Dock 贴边方向：'x'（左右缘）或 'y'（上下缘）
  var edgeMin = true;    // true = 贴在坐标最小的一侧（左/上）；false = 右/下
  var shown = false;     // 显隐意图（与 HIDDEN_CLASS 互斥）；接管默认 false = 平时是藏着的
  var idleTimer = null;  // 无操作兜底定时器（armIdle）
  var displayTimer = null; // 滑出动画结束后真正 display:none 的定时器
  var lastMiss = '';     // 最近一次「全部策略落空」的计数快照（控制台诊断用）
  var lastPaddingPass = 0; // 上一次布局预留扫描的时间戳（节流）

  var CSS =
    '.' + HOST_CLASS + '{transition:transform .28s cubic-bezier(.4,0,.2,1);}' +
    '.' + HOST_CLASS + '.' + HIDDEN_CLASS + '{transform:var(--fnos-dock-hide-tf,translateX(-105%));}' +
    '@media (prefers-reduced-motion: reduce){.' + HOST_CLASS + '{transition:none;}}';

  /** 样式只装一次；优先可构造样式表（CSP 对 inline <style> 收紧时仍然可用），否则 <style>。 */
  function ensureStyle() {
    if (W.CSSStyleSheet && D.adoptedStyleSheets) {
      for (var i = 0; i < D.adoptedStyleSheets.length; i++) {
        if (D.adoptedStyleSheets[i].__fnosDock) return;
      }
      try {
        var sheet = new W.CSSStyleSheet();
        sheet.replaceSync(CSS);
        sheet.__fnosDock = true;
        D.adoptedStyleSheets = D.adoptedStyleSheets.concat([sheet]);
        return;
      } catch (e) { /* 构造失败（老内核）→ 落到 <style> */ }
    }
    if (D.getElementById(STYLE_ID)) return;
    var el = D.createElement('style');
    el.id = STYLE_ID;
    el.textContent = CSS;
    (D.head || D.documentElement).appendChild(el);
  }

  /** 元素像不像 Dock 容器：窄长条（竖条：宽 ≤140 且高 ≥140；横条：高 ≤140 且宽 ≥140）。 */
  function dockShaped(el) {
    var w = el.offsetWidth || 0;
    var h = el.offsetHeight || 0;
    if (!(w > 0 && h > 0)) return false;
    return (w <= DOCK_SIDE_MAX && h >= DOCK_SIDE_MAX) || (h <= DOCK_SIDE_MAX && w >= DOCK_SIDE_MAX);
  }

  /** 容器里图标（img/svg）够多才算 Dock：≥3 个。系统悬浮窗（如资源监控）没有这么多。 */
  function iconRich(el) {
    try { return el.querySelectorAll('img,svg').length >= 3; } catch (e) { return false; }
  }

  /** 本壳自己的元素绝不能被当成 Dock。 */
  function isOwnEl(el) {
    return !!(el && el.id && String(el.id).indexOf('fnos-shell') === 0);
  }

  // ---------- 多策略定位（S1 → S2 → S3 → S4；全部落空时把计数写进 lastMiss） ----------

  /** S1：上游根选择器。含任务栏列表的候选优先（resolveTaskbarNodes 的镜像），退化取第一个。 */
  function s1UpstreamRoot() {
    var candidates;
    try { candidates = D.querySelectorAll(DOCK_ROOT_SELECTOR); } catch (e) { return null; }
    if (!candidates || !candidates.length) return null;
    var fallback = null;
    for (var i = 0; i < candidates.length; i++) {
      if (!fallback) fallback = candidates[i];
      try {
        if (candidates[i].querySelector(DOCK_LIST_SELECTOR)) return candidates[i];
      } catch (e) { /* 选择器异常按无列表处理 */ }
    }
    return fallback;
  }

  /** S2：上游任务栏项 → 从父层沿祖先向上爬到最外层「Dock 形状」的容器。 */
  function s2FromItems() {
    var items;
    try { items = D.querySelectorAll(DOCK_ITEM_SELECTOR); } catch (e) { return null; }
    if (!items || !items.length) return null;
    for (var i = 0; i < items.length; i++) {
      var cur = items[i].parentElement;
      var best = null;
      for (var depth = 0; cur && depth < 10; depth++) {
        if (!dockShaped(cur)) break;
        best = cur;
        cur = cur.parentElement;
      }
      if (best) return best;
    }
    return null;
  }

  /** S3/S4：贴屏幕边缘 + Dock 形状 + 图标丰富的固定定位容器。`deep` = 全量计算样式扫描。 */
  function s3FixedNearEdge(deep) {
    var nodes;
    try { nodes = deep ? D.querySelectorAll('body *') : D.querySelectorAll('.fixed'); } catch (e) { return null; }
    if (!nodes || !nodes.length) return null;
    var vw = W.innerWidth || 0;
    var vh = W.innerHeight || 0;
    var limit = deep ? 2000 : nodes.length; // 全量扫描设上限，避免超大页面卡顿
    for (var i = 0; i < nodes.length && i < limit; i++) {
      var el = nodes[i];
      if (isOwnEl(el)) continue;
      if (deep) {
        var cs = null;
        try { cs = W.getComputedStyle ? W.getComputedStyle(el) : null; } catch (e) { /* 忽略 */ }
        if (!cs || cs.position !== 'fixed') continue;
      }
      var r;
      try { r = el.getBoundingClientRect(); } catch (e) { continue; }
      if (!(r.width > 0 && r.height > 0)) continue;
      var vertical = r.width <= DOCK_SIDE_MAX && r.height >= DOCK_SIDE_MAX;
      var horizontal = r.height <= DOCK_SIDE_MAX && r.width >= DOCK_SIDE_MAX;
      if (!vertical && !horizontal) continue;
      var nearEdge = (vertical && (r.left <= 80 || (vw && r.right >= vw - 80)))
        || (horizontal && (r.top <= 80 || (vh && r.bottom >= vh - 80)));
      if (!nearEdge) continue;
      if (!iconRich(el)) continue;
      return el;
    }
    return null;
  }

  var STRATEGIES = [
    ['S1 上游根选择器', function () { return s1UpstreamRoot(); }],
    ['S2 任务栏项爬升', function () { return s2FromItems(); }],
    ['S3 贴边固定容器', function () { return s3FixedNearEdge(false); }],
    ['S4 全量扫描', function () { return s3FixedNearEdge(true); }]
  ];

  /** 依序尝试所有策略；返回找到的元素，全部落空时把各策略计数写进 `lastMiss`。 */
  function findDock() {
    var counts = [];
    for (var i = 0; i < STRATEGIES.length; i++) {
      var el = null;
      try { el = STRATEGIES[i][1](); } catch (e) { el = null; }
      if (el && !isOwnEl(el)) {
        lastMiss = ''; // 找到了：旧的落空计数不再有诊断意义
        return el;
      }
      var n = 0;
      try {
        if (i === 0) n = D.querySelectorAll(DOCK_ROOT_SELECTOR).length;
        else if (i === 1) n = D.querySelectorAll(DOCK_ITEM_SELECTOR).length;
        else if (i === 2) n = D.querySelectorAll('.fixed').length;
        else n = -1;
      } catch (e) { /* 保持 0 */ }
      counts.push(STRATEGIES[i][0] + ' ' + (i === 3 ? '未命中' : n));
    }
    lastMiss = counts.join('；');
    return null;
  }

  /**
   * Dock 贴在哪条边上？按传入的**当前**几何判断。零尺寸（SPA 还没布局好 / 已 display:none，
   * rect 全 0 会把任意元素判成「贴左上」）不改写结论——沿用上一次的贴边方向，
   * 按上游实际形态 = 左缘起步。
   */
  function measure(rect) {
    var vw = W.innerWidth || 0;
    var vh = W.innerHeight || 0;
    if (!(rect.width > 0 && rect.height > 0)) {
      return edgeMin
        ? (axis === 'x' ? 'translateX(-105%)' : 'translateY(-105%)')
        : (axis === 'x' ? 'translateX(105%)' : 'translateY(105%)');
    }
    if (rect.left <= 1) { axis = 'x'; edgeMin = true; return 'translateX(-105%)'; }
    if (vw && rect.right >= vw - 1) { axis = 'x'; edgeMin = false; return 'translateX(105%)'; }
    if (rect.top <= 1) { axis = 'y'; edgeMin = true; return 'translateY(-105%)'; }
    if (vh && rect.bottom >= vh - 1) { axis = 'y'; edgeMin = false; return 'translateY(105%)'; }
    axis = 'x'; edgeMin = true;
    return 'translateX(-105%)';
  }

  /** 在**要藏的那一刻**重测贴边方向并写下变换变量（那时 Dock 一定可见，rect 可信）。 */
  function syncTransform() {
    if (!dock) return;
    var rect;
    try { rect = dock.getBoundingClientRect(); } catch (e) { return; }
    dock.style.setProperty('--fnos-dock-hide-tf', measure(rect));
  }

  // ---------- 空间回收：把「谁预留了左侧那条宽度」找出来中和掉 ----------

  /** 把某个元素的「预留属性」置为 !important 的新值，并记账以便还原（同一属性只记一次）。 */
  function override(el, prop, value) {
    for (var i = 0; i < reclaimed.length; i++) {
      if (reclaimed[i].el === el && reclaimed[i].prop === prop) return;
    }
    var prev = '';
    try { prev = el.style.getPropertyValue(prop) || ''; } catch (e) { prev = ''; }
    reclaimed.push({ el: el, prop: prop, prev: prev });
    try { el.style.setProperty(prop, value, 'important'); } catch (e) { /* 不支持就算了 */ }
  }

  /** grid 第一轨的数值（`gridTemplateColumns` 可能是 "66px 1134px" 或含 minmax(...)）。 */
  function firstTrack(template) {
    var s = String(template == null ? '' : template).trim();
    if (!s) return NaN;
    // 按「括号外的空白」切分，避免把 minmax(0px, 1fr) 切开
    var parts = s.split(/\s+(?![^()]*\))/);
    var m = /^(-?\d*\.?\d+)px$/.exec(parts[0] || '');
    return m ? parseFloat(m[1]) : NaN;
  }

  /**
   * 机制①：某个容器用**内边距/外边距/grid 首轨**预留了左侧宽度 → 中和掉。
   * 判据是「满尺寸容器 + 预留值落在合理区间（[`STRIP_MIN`, `STRIP_MAX`]）」，不写死具体数值
   * （用户机上实测是 Tailwind 的 `pl-[66px]`，66 = Dock 宽）。
   */
  function neutralizeSpacer(el, vw, vh) {
    var rect;
    try { rect = el.getBoundingClientRect(); } catch (e) { return; }
    if (!(rect.width >= vw - 4 && rect.height >= vh * 0.8)) return; // 只有满尺寸容器才是内容区
    var cs = null;
    try { cs = W.getComputedStyle ? W.getComputedStyle(el) : null; } catch (e) { cs = null; }
    if (!cs) return;
    var pl = parseFloat(cs.paddingLeft);
    if (isFinite(pl) && pl >= STRIP_MIN && pl <= STRIP_MAX) override(el, 'padding-left', '0px');
    var ml = parseFloat(cs.marginLeft);
    if (isFinite(ml) && ml >= STRIP_MIN && ml <= STRIP_MAX) override(el, 'margin-left', '0px');
    if (cs.display === 'grid') {
      var template = String(cs.gridTemplateColumns || '');
      var track = firstTrack(template);
      if (isFinite(track) && track >= STRIP_MIN && track <= STRIP_MAX) {
        var parts = template.trim().split(/\s+(?![^()]*\))/);
        parts[0] = '0px';
        override(el, 'grid-template-columns', parts.join(' '));
      }
    }
  }

  /** 在 root 及其后代里**有界**地找预留容器（深度 ≤ 4、每层 ≤ 40、总数 ≤ 400）。 */
  function neutralizeIn(root, vw, vh, depth, budget) {
    if (!root || depth > 4 || budget.n <= 0) return;
    budget.n -= 1;
    neutralizeSpacer(root, vw, vh);
    var kids = root.children;
    if (!kids) return;
    for (var i = 0; i < kids.length && i < 40; i++) {
      neutralizeIn(kids[i], vw, vh, depth + 1, budget);
    }
  }

  /** 已记账的回收是否仍然有效：元素还在文档里、且覆盖还在（否则应当**立刻**重扫）。 */
  function reclaimStillValid() {
    if (!reclaimed.length) return true; // 还没扫过：交给节流（首轮 lastPaddingPass = 0 必跑）
    for (var i = 0; i < reclaimed.length; i++) {
      var item = reclaimed[i];
      if (!item.el || item.el.isConnected === false) return false;
      try {
        if (!item.el.style.getPropertyValue(item.prop)) return false;
      } catch (e) { return false; }
    }
    return true;
  }

  /**
   * 机制①的入口。fnOS 实测结构：
   *   <div class="relative flex …">            ← 桌面根（flex 行）
   *     <div class="fixed inset-y-0 left-0">    ← Dock（本壳接管；fixed 不占位）
   *     <div class="… pl-[66px]">               ← 内容区：桌面图标与窗口都在里面
   *   </div>
   * 所以预留写在 Dock 的**同级内容区**上（也从 Dock 的祖先链兜一层），本函数两处都扫。
   *
   * 有界 + 节流：一次最多 400 个节点，且两次之间至少 [`LAYOUT_PASS_MIN_MS`]；但只要
   * [`reclaimStillValid`] 发现覆盖丢了（容器被 SPA 换掉），节流窗口不作数——**立刻**补扫，
   * 免得左侧空白闪一下再收回（修复轮 9）。
   */
  function reclaimLayoutPadding() {
    if (!(axis === 'x' && edgeMin)) return; // 只处理贴左缘的形态
    var now = Date.now();
    if (now - lastPaddingPass < LAYOUT_PASS_MIN_MS && reclaimStillValid()) return;
    lastPaddingPass = now;
    var vw = W.innerWidth || 0;
    var vh = W.innerHeight || 0;
    var budget = { n: 400 };
    var host = dock && dock.parentElement;
    if (host && host.children) {
      for (var i = 0; i < host.children.length; i++) {
        if (host.children[i] === dock) continue;
        neutralizeIn(host.children[i], vw, vh, 1, budget);
      }
    }
    for (var el = dock, depth = 0; el && depth < 10; el = el.parentElement, depth++) {
      var parent = el.parentElement;
      if (!parent || parent === D.documentElement) break;
      neutralizeSpacer(parent, vw, vh);
    }
  }

  // ---------- 机制②：应用窗口的工作区偏移（fnOS 把 Dock 宽写死在工具类里） ----------

  /**
   * fnOS 的窗口管理器用 Tailwind 工具类把「工作区 = 视口 − Dock 宽」写死，三处**同数**且都带
   * `!important`：`!left-[66px]` / `!right-0` / `!w-[calc(100%-66px)]`。绝对定位元素不吃父级
   * padding，所以机制①动不了窗口——必须按同一签名覆盖 `left`/`width`。签名里三处的数字必须
   * 一致，否则不动手（宁可不动，也不误伤别的定位）。
   */
  var MAXIMIZED_LEFT = /(?:^|\s)!left-\[(\d+)px\](?:\s|$)/;
  var MAXIMIZED_WIDTH = /(?:^|\s)!w-\[calc\(100%-(\d+)px\)\](?:\s|$)/;

  /** 返回预留宽度（px）——只有「三处同数 + `!right-0`」齐备时才算「最大化到工作区」的窗口。 */
  function maximizedSignature(el) {
    var cls = '';
    try { cls = String(el.className || ''); } catch (e) { return null; }
    if (cls.indexOf('!right-0') < 0) return null;
    var left = MAXIMIZED_LEFT.exec(cls);
    var width = MAXIMIZED_WIDTH.exec(cls);
    if (!left || !width || left[1] !== width[1]) return null;
    var n = parseInt(left[1], 10);
    return isFinite(n) && n >= STRIP_MIN && n <= STRIP_MAX ? n : null;
  }

  var windowFixes = [];   // 被覆盖过定位的窗口（签名消失时逐条撤回）

  /** 撤回签名已消失的窗口覆盖（用户把窗口改回自由尺寸时，定位权还给窗口管理器）。 */
  function revertWindowFixes() {
    for (var i = windowFixes.length - 1; i >= 0; i--) {
      var el = windowFixes[i];
      if (maximizedSignature(el)) continue; // 仍是最大化形态：保持覆盖
      try {
        el.style.removeProperty('left');
        el.style.removeProperty('width');
      } catch (e) { /* 元素没了就算了 */ }
      windowFixes.splice(i, 1);
    }
  }

  /** 把「最大化到工作区」的窗口拉回满宽（Dock 已让位；覆盖只到签名消失为止）。 */
  function reclaimWorkspaceWindows() {
    revertWindowFixes();
    if (!(axis === 'x' && edgeMin)) return;
    var nodes;
    // 候选用 fnOS 自己的窗口类（语义明确、数量少）；签名仍是最终判据
    try { nodes = D.querySelectorAll('[class*="app-layout--window"]'); } catch (e) { return; }
    for (var i = 0; nodes && i < nodes.length; i++) {
      var el = nodes[i];
      if (!maximizedSignature(el)) continue;
      try {
        el.style.setProperty('left', '0px', 'important');
        el.style.setProperty('width', '100%', 'important');
      } catch (e) { /* 不支持就算了 */ }
      if (windowFixes.indexOf(el) < 0) windowFixes.push(el);
    }
  }

  /** teardown 用：撤回全部窗口覆盖（无条件）。 */
  function releaseWindowFixes() {
    for (var i = 0; i < windowFixes.length; i++) {
      try {
        windowFixes[i].style.removeProperty('left');
        windowFixes[i].style.removeProperty('width');
      } catch (e) { /* 元素没了就算了 */ }
    }
    windowFixes = [];
  }

  function restoreReclaimed() {
    for (var i = 0; i < reclaimed.length; i++) {
      var item = reclaimed[i];
      try {
        if (item.prev) item.el.style.setProperty(item.prop, item.prev);
        else item.el.style.removeProperty(item.prop);
      } catch (e) { /* 元素没了就算了 */ }
    }
    reclaimed = [];
  }

  // ---------- 接管与还原 ----------

  /** 释放一个曾接管的元素：class、内联变量、display 全部还原（不留下孤儿状态）。 */
  function releaseOne(el) {
    if (!el) return;
    el.classList.remove(HOST_CLASS, HIDDEN_CLASS);
    try {
      el.style.removeProperty('--fnos-dock-hide-tf');
      el.style.display = '';
    } catch (e) { /* 不支持就算了 */ }
  }

  /**
   * 只释放接管元素（class / 内联变量 / display），**不动空间回收**。
   *
   * 为什么必须分开（修复轮 9）：fnOS 在悬浮等时机**会重建 Dock 元素**，而「换元素」走的正是
   * 这条路。早先这里连着 `restoreReclaimed()` 一起还原，于是每次悬浮都先把左侧预留还回去
   * （空白闪回来）、再等下一轮扫描才中和 —— 用户看到的就是「悬浮时左侧空白闪烁」。
   * 容器没变，回收就该一直有效：还原只在真正关闭功能（`teardown`）时做。
   */
  function releaseTargets() {
    for (var i = 0; i < targets.length; i++) releaseOne(targets[i]);
    targets = [];
    dock = null;
  }

  /** 完全还原：接管元素 + 空间回收 + 窗口覆盖（**只在功能关闭时**调用）。 */
  function releaseAll() {
    releaseTargets();
    releaseWindowFixes();
    restoreReclaimed();
  }

  /**
   * Dock 离场时派发一次 resize：工作区变大，让窗口管理器重排已铺开的窗口（修复轮 5）。
   *
   * **只在离场时派发，回归时不再派发**（修复轮 9）：回归（悬浮唤出）时 Dock 是**浮在内容
   * 之上**的，布局本该一点不动；派发 resize 只会让窗口管理器重算，把 66px 偏移又写回窗口与
   * 内容区，与我们的中和互相拉锯 —— 用户看到的就是「悬浮时左侧空白闪烁」。
   */
  function notifyResize() {
    try { W.dispatchEvent(new W.Event('resize')); } catch (e) { /* 老内核缺 Event 构造器就算了 */ }
  }

  /** 把 `shown` 的意图落到接管元素上（幂等；接管后无条件调一次以防换元素后状态漂移）。 */
  function renderState() {
    if (!dock) return;
    if (displayTimer) { W.clearTimeout(displayTimer); displayTimer = null; }
    if (shown) {
      for (var i = 0; i < targets.length; i++) {
        try { targets[i].style.display = ''; } catch (e) { /* 已被上游藏起来就算了 */ }
      }
      try { void dock.offsetWidth; } catch (e) { /* 强制重排：display:none 刚恢复时要有动画起点 */ }
      dock.classList.remove(HIDDEN_CLASS);
      return; // 唤出是「浮层」：不改布局、不派发 resize（见 notifyResize 的注释）
    }
    syncTransform();
    dock.classList.add(HIDDEN_CLASS);
    // 滑出动画结束后**真正离场**（transform 挡不住按 rect 的工作区计算，修复轮 2）。
    displayTimer = W.setTimeout(function () {
      displayTimer = null;
      if (!dock || shown) return;
      for (var i = 0; i < targets.length; i++) {
        try { targets[i].style.display = 'none'; } catch (e) { /* 不支持就算了 */ }
      }
      notifyResize(); // Dock 离场：工作区变大，已开的窗口借 resize 重排（修复轮 5）
    }, TRANSITION_MS);
  }

  function setShown(next) {
    if (next === shown) return;
    shown = next;
    renderState();
  }

  /** 重新找 Dock 并接管。已接管的元素不变；旧元素被换掉时把自己加的东西清干净。 */
  function locate() {
    locateScheduled = false;
    var found = findDock();
    if (found && found !== dock) {
      // fnOS 会重建 Dock 元素（悬浮等时机）：这里只释放旧元素的接管标记，
      // **空间回收保持不变**（容器没变；还原会让左侧空白闪回来，见 releaseTargets）。
      releaseTargets();
      dock = found;
      targets = [found];
      found.classList.add(HOST_CLASS);
      // 元素被换掉时容器也可能被换掉：清掉节流，让本轮立刻重扫一次
      lastPaddingPass = 0;
      // 先定贴边方向：空间回收（只处理贴左缘）与滑出方向都依赖它，而它只能从**当前可见**
      // 的 Dock 几何量出来——晚于这一步量就会拿到默认值（修复轮 7 的实现次序）。
      syncTransform();
      // 接管即按当前意图落态（默认 = 藏）：不等第一次 pointermove，否则用户在设置里
      // 打开开关、切回主窗口，鼠标不动就永远看不到效果（修复轮 2）。
      // 指针恰好在 Dock 上时先不藏（:hover 读的是当前真实悬停链）。
      var hovered = false;
      try { hovered = dock.matches(':hover'); } catch (e) { /* 老内核不支持就照藏 */ }
      if (hovered) shown = true;
      renderState();
    }
    if (!dock) return;
    // 空间回收每一批变更都重跑（窗口会动态开关/最大化）：机制①（内边距）幂等，
    // 机制②（窗口工具类）自带「签名消失即撤回」。
    reclaimLayoutPadding();
    reclaimWorkspaceWindows();
  }

  /** observer 回调的合并入口：SPA 一批变更只安排一次重找。 */
  function scheduleLocate() {
    if (locateScheduled) return;
    locateScheduled = true;
    W.setTimeout(locate, LOCATE_DELAY_MS);
  }

  /** 无操作兜底：IDLE_MS 内没有任何指针事件就藏（iframe 吞事件的场景只有它能救）。 */
  function armIdle() {
    if (idleTimer) W.clearTimeout(idleTimer);
    idleTimer = W.setTimeout(function () {
      idleTimer = null;
      if (enabled && dock) setShown(false);
    }, IDLE_MS);
  }

  /**
   * 指针驱动显隐（迟滞）。`over` 用 **元素尺寸**（translate/display 都不改变 width/height）
   * 而不是实时 rect：Dock 藏起来之后实时 rect 会滑出屏幕甚至归零，按它判「还在 Dock 上吗」
   * 对贴右/下缘的形态会恒真或恒假。热区（near）只认屏幕边缘那几像素。
   */
  function onPointerMove(e) {
    if (!enabled || !dock) return;
    armIdle();
    var vw = W.innerWidth || 0;
    var vh = W.innerHeight || 0;
    var size = axis === 'y' ? dock.offsetHeight : dock.offsetWidth;
    var coord = axis === 'y' ? e.clientY : e.clientX;
    var span = axis === 'y' ? vh : vw;
    var near = edgeMin ? coord <= EDGE_PX : (span ? coord >= span - EDGE_PX : false);
    var over = edgeMin ? coord <= size + SLACK_PX : (span ? coord >= span - size - SLACK_PX : false);
    if (near) setShown(true);
    else if (shown && !over) setShown(false);
  }

  /** 指针离开窗口 / 窗口失焦：auto-hide 的通用语义，直接藏。 */
  function onHideSignal() {
    if (enabled) setShown(false);
  }

  function install() {
    ensureStyle();
    if (!observer && typeof W.MutationObserver === 'function') {
      observer = new W.MutationObserver(scheduleLocate);
      // 观察 document：本脚本在 document-start 注入，body 可能还不存在
      observer.observe(D, { childList: true, subtree: true });
    }
    D.addEventListener('pointermove', onPointerMove, { passive: true });
    D.addEventListener('pointerleave', onHideSignal);
    W.addEventListener('blur', onHideSignal);
    armIdle();
    locate();
  }

  function teardown() {
    if (observer) { observer.disconnect(); observer = null; }
    if (idleTimer) { W.clearTimeout(idleTimer); idleTimer = null; }
    if (displayTimer) { W.clearTimeout(displayTimer); displayTimer = null; }
    D.removeEventListener('pointermove', onPointerMove);
    D.removeEventListener('pointerleave', onHideSignal);
    W.removeEventListener('blur', onHideSignal);
    releaseAll();
    notifyResize(); // 空间还原了，也让窗口管理器重排一次
  }

  /** set_config 的免刷新入口（shim 的 `__FNOS_APPLY_CONFIG__` 转调；幂等）。 */
  W.__FNOS_APPLY_SHELL__ = function (patch) {
    if (!patch || typeof patch !== 'object') return;
    if (typeof patch.dockAutoHide !== 'boolean') return; // 不认识的形状不动手
    var next = patch.dockAutoHide;
    if (next === enabled) return;
    enabled = next;
    if (enabled) install();
    else teardown();
  };

  /**
   * 诊断口（控制台可见，**不向页面画任何东西**；T14c 修复轮 7 起页面上不再有提示）：
   * `__FNOS_DOCK_STATE__()` 返回接管状态、空间回收记账与各策略的候选计数。
   */
  W.__FNOS_DOCK_STATE__ = function () {
    var props = [];
    for (var i = 0; i < reclaimed.length; i++) {
      if (props.indexOf(reclaimed[i].prop) < 0) props.push(reclaimed[i].prop);
    }
    return {
      enabled: enabled, found: !!dock, targets: targets.length, axis: axis, edgeMin: edgeMin,
      shown: shown, reclaimed: props, windowsFixed: windowFixes.length, lastMiss: lastMiss,
    };
  };

  if (enabled) install();
})();
