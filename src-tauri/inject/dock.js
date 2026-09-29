/* fnOS Desktop Shell — Dock 自动隐藏（T14c，本壳自己的页面功能，不是上游 mod）
 *
 * 做什么：把 fnOS WebUI 的 Dock（任务栏）平时滑出屏幕边缘，鼠标顶到它所在的边缘时滑回。
 *
 * 定位（多策略，全部锚定上游已知特征；T14c 修复轮 4 起策略化——上游的根选择器在部分
 * fnOS 版本上匹配 0 个元素，实测反馈见诊断角标）：
 *   S1 上游类名策略   —— 逐字复用上游 mod.js 的 TASKBAR_ROOT_SELECTOR（+ TASKBAR_LIST_SELECTOR
 *                        验证），命中即用；这是与上游行为完全一致的首选路径。
 *   S2 上游任务栏项策略 —— 用上游的 TASKBAR_ITEM_SELECTOR 找到任务栏图标项，沿祖先向上爬到
 *                        最外层「Dock 形状」的容器（窄长条）。图标项与上游 isTaskbarAppItem
 *                        同源，所以爬出来的就是上游认定的任务栏所在。
 *   S3 贴边容器策略   —— Tailwind `.fixed` 元素里，贴屏幕边缘 + Dock 形状（窄长条）+
 *                        含 ≥3 个图标（img/svg）的容器。
 *   S4 全量扫描策略   —— S3 失败后的兜底：扫描 `body *` 的计算样式 position:fixed
 *                        （贵，只在需要时做一次；locate 本就有 150ms 合并窗）。
 *   全部失败          —— 左下角诊断角标报告每种策略的计数（绝不静默失效）；登录页
 *                        （存在密码输入框）不弹诊断——那里本来就没有 Dock。
 *
 * 显隐规则（T14c 修复轮 3 定稿）：
 *   唤出   —— 指针顶到 Dock 所在的屏幕边缘热区（EDGE_PX）。**不是**「进入 Dock 的脚印」：
 *            藏在 Dock 底下的应用按钮就在那条带里，一靠近就唤出会把要点的按钮盖住。
 *   保持   —— 唤出后指针留在 Dock 脚印（自身尺寸 + 余量）内就一直显示，离开即藏（迟滞）。
 *   藏起   —— 指针离开脚印 / 离开窗口 / 窗口失焦 / **无操作超过 IDLE_MS** / 接管时。
 *            指针进入 iframe（fnOS 的应用窗口）之后顶层文档收不到 pointermove——纯事件
 *            驱动的显隐会卡在「显示」，所以必须有 idle 兜底。
 *   离场   —— 滑出动画结束后 `display:none`：transform 藏得住视觉，藏不住按 rect 做的
 *            工作区计算（全屏应用给 Dock 让出的那条宽度）。
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
  var DIAG_DELAY_MS = 5000;  // 开启后这么久还没找到 Dock → 页面角标给出诊断（不静默失效）
  var DOCK_SIDE_MAX = 140;   // 「Dock 形状」的窄边上限 / 长边下限（像素）

  var enabled = SHELL_CFG.dockAutoHide === true;
  var dock = null;       // 当前接管的 Dock 元素（可能被 SPA 换掉，observer 会重新找）
  var observer = null;   // MutationObserver：Dock 出现 / 被替换时重新接管
  var locateScheduled = false;
  var axis = 'x';        // Dock 贴边方向：'x'（左右缘）或 'y'（上下缘）
  var edgeMin = true;    // true = 贴在坐标最小的一侧（左/上）；false = 右/下
  var shown = false;     // 显隐意图（与 HIDDEN_CLASS 互斥）；接管默认 false = 平时是藏着的
  var idleTimer = null;  // 无操作兜底定时器（armIdle）
  var displayTimer = null; // 滑出动画结束后真正 display:none 的定时器
  var diagTimer = null;  // 「还没找到 Dock」诊断角标的定时器
  var lastMiss = '';     // 最近一次「全部策略落空」的计数快照（诊断角标用）

  var CSS =
    '.' + HOST_CLASS + '{transition:transform .28s cubic-bezier(.4,0,.2,1);}' +
    '.' + HOST_CLASS + '.' + HIDDEN_CLASS + '{transform:var(--fnos-dock-hide-tf,translateX(-105%));}' +
    '@media (prefers-reduced-motion: reduce){.' + HOST_CLASS + '{transition:none;}}' +
    // 诊断/提示角标（本壳命名空间；pointer-events:none 不挡任何点击）
    '.fnos-shell-dock-badge{position:fixed;left:10px;bottom:10px;z-index:2147483000;max-width:460px;' +
    'padding:7px 11px;border-radius:8px;background:rgba(20,20,22,.82);color:#d8dadd;' +
    'font:12px/1.6 system-ui,"Segoe UI","Microsoft YaHei",sans-serif;pointer-events:none;' +
    'box-shadow:0 2px 10px rgba(0,0,0,.35);white-space:pre-wrap;}' +
    '.fnos-shell-dock-badge.warn{color:#ffd479;}';

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

  /** 本壳自己的元素（诊断角标等）绝不能被当成 Dock。 */
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

  /** S2：上游任务栏项 → 沿祖先向上爬到最外层「Dock 形状」的容器。 */
  function s2FromItems() {
    var items;
    try { items = D.querySelectorAll(DOCK_ITEM_SELECTOR); } catch (e) { return null; }
    if (!items || !items.length) return null;
    for (var i = 0; i < items.length; i++) {
      // 从图标项的**父层**开始爬（图标项本身 47×40，不是「Dock 形状」）
      var cur = items[i].parentElement;
      var best = null;
      for (var depth = 0; cur && depth < 10; depth++) {
        if (!dockShaped(cur)) break;
        best = cur;
        cur = cur.parentElement;
      }
      // 爬到了至少一层 shaped 容器才算「找到了 Dock 所在」
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
      // 记录该策略的候选规模（诊断用）：前两条是选择器匹配数，后两条是扫描节点数
      var n = 0;
      try {
        if (i === 0) n = D.querySelectorAll(DOCK_ROOT_SELECTOR).length;
        else if (i === 1) n = D.querySelectorAll(DOCK_ITEM_SELECTOR).length;
        else if (i === 2) n = D.querySelectorAll('.fixed').length;
        else n = -1;
      } catch (e) { /* 保持 0 */ }
      counts.push(STRATEGIES[i][0] + ' ' + (i === 3 ? '未执行/未命中' : n));
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

  /** 释放一个曾接管的元素：class、内联变量、display 全部还原（不留下孤儿状态）。 */
  function release(el) {
    if (!el) return;
    el.classList.remove(HOST_CLASS, HIDDEN_CLASS);
    try {
      el.style.removeProperty('--fnos-dock-hide-tf');
      el.style.display = '';
    } catch (e) { /* 不支持就算了 */ }
  }

  /**
   * Dock 离场/回归都会改变「可用工作区」，而 fnOS 的窗口管理器通常只在 resize 时重排
   * 窗口位置（修复轮 5：已打开的窗口不会跟着回收 Dock 让出的那条宽度）。Dock 真正
   * 离场（display:none）或恢复布局的当下派发一次 resize，让最大化/已铺开的窗口按
   * 新工作区重排。
   */
  function notifyResize() {
    try { W.dispatchEvent(new W.Event('resize')); } catch (e) { /* 老内核缺 Event 构造器就算了 */ }
  }

  /** 把 `shown` 的意图落到当前 Dock 上（幂等；接管后无条件调一次以防换元素后状态漂移）。 */
  function renderState() {
    if (!dock) return;
    if (displayTimer) { W.clearTimeout(displayTimer); displayTimer = null; }
    if (shown) {
      try { dock.style.display = ''; } catch (e) { /* 已被上游藏起来就算了 */ }
      void dock.offsetWidth; // 强制重排：display:none 刚恢复时要有动画起点，否则直接跳到位
      dock.classList.remove(HIDDEN_CLASS);
      notifyResize(); // Dock 回到布局：工作区变小，窗口管理器按旧几何收回去
      return;
    }
    syncTransform();
    dock.classList.add(HIDDEN_CLASS);
    // 滑出动画结束后**真正离场**（transform 挡不住按 rect 的全屏工作区计算，修复轮 2）。
    displayTimer = W.setTimeout(function () {
      displayTimer = null;
      if (dock && !shown) {
        try { dock.style.display = 'none'; } catch (e) { /* 不支持就算了 */ }
        notifyResize(); // Dock 离场：工作区变大，已开的窗口借 resize 重排（修复轮 5）
      }
    }, TRANSITION_MS);
  }

  function setShown(next) {
    if (next === shown) return;
    shown = next;
    renderState();
  }

  // ---------- 页面角标：只在「找不到 Dock」时出现的诊断（不静默失效；成功不弹提示） ----------

  /** 设置或清除角标；`text` 为空 = 清除。样式走 adoptedStyleSheet（CSP 免疫），不挡点击。 */
  function badge(text, warn) {
    var el = D.getElementById('fnos-shell-dock-badge');
    if (!text) {
      if (el) el.remove();
      return;
    }
    if (!el) {
      el = D.createElement('div');
      el.id = 'fnos-shell-dock-badge';
      el.className = 'fnos-shell-dock-badge';
      (D.body || D.documentElement).appendChild(el);
    }
    el.className = 'fnos-shell-dock-badge' + (warn ? ' warn' : '');
    el.textContent = text;
  }

  /**
   * 开启后一段时间还没找到 Dock → 诊断角标（选择器失明时绝不悄悄躺平）。
   * 登录页本来就没有 Dock（用户实测反馈）：页面里有密码输入框时静默顺延，不误报。
   * （T14c 修复轮 6：接管成功的左下角提示已按用户要求删除——找到即静默接管，
   * 只清掉可能挂着的失败诊断角标。）
   */
  function scheduleDiag() {
    if (diagTimer) W.clearTimeout(diagTimer);
    diagTimer = W.setTimeout(function () {
      diagTimer = null;
      if (!enabled || dock) return; // 已接管 / 已关闭：不需要诊断
      var onLogin = false;
      try { onLogin = !!D.querySelector('input[type="password"]'); } catch (e) { /* 按「不在登录页」处理 */ }
      if (onLogin) { scheduleDiag(); return; } // 登录页没有 Dock：静默等登录后再判
      badge('fnOS壳·Dock自动隐藏：仍未找到 Dock 元素。各策略计数——' + lastMiss
        + '。找到后本提示自动消失；若登录后仍持续显示，请截图反馈。', true);
    }, DIAG_DELAY_MS);
  }

  /** 重新找 Dock 并接管。已接管的元素不变；旧元素被换掉时把自己加的东西清干净。 */
  function locate() {
    locateScheduled = false;
    var found = findDock();
    if (!found) return;
    if (found !== dock) {
      release(dock);
      dock = found;
      dock.classList.add(HOST_CLASS);
      // 接管即按当前意图落态（默认 = 藏）：不等第一次 pointermove，否则用户在设置里
      // 打开开关、切回主窗口，鼠标不动就永远看不到效果（修复轮 2）。
      // 指针恰好在 Dock 上时先不藏（:hover 读的是当前真实悬停链）。
      var hovered = false;
      try { hovered = dock.matches(':hover'); } catch (e) { /* 老内核不支持就照藏 */ }
      if (hovered) shown = true;
      renderState();
      badge(null); // 找到了：清掉可能挂着的「找不到 Dock」诊断角标（不再弹任何成功提示）
    }
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
    scheduleDiag();
    locate();
  }

  function teardown() {
    if (observer) { observer.disconnect(); observer = null; }
    if (idleTimer) { W.clearTimeout(idleTimer); idleTimer = null; }
    if (displayTimer) { W.clearTimeout(displayTimer); displayTimer = null; }
    if (diagTimer) { W.clearTimeout(diagTimer); diagTimer = null; }
    D.removeEventListener('pointermove', onPointerMove);
    D.removeEventListener('pointerleave', onHideSignal);
    W.removeEventListener('blur', onHideSignal);
    release(dock);
    dock = null;
    badge(null); // 功能关了，角标也不该留着
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

  /** 诊断口：控制台里 `__FNOS_DOCK_STATE__()` 即可看到接管状态（不携带任何页面数据）。 */
  W.__FNOS_DOCK_STATE__ = function () {
    return {
      enabled: enabled, found: !!dock, axis: axis, edgeMin: edgeMin, shown: shown,
      lastMiss: lastMiss,
    };
  };

  if (enabled) install();
})();
