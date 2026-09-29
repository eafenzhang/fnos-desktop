/* fnOS Desktop Shell — Dock 自动隐藏（T14c，本壳自己的页面功能，不是上游 mod）
 *
 * 做什么：把 fnOS WebUI 的 Dock（任务栏）平时滑出屏幕边缘，鼠标顶到它所在的边缘时滑回。
 * 策略（上游类名策略）：Dock 根元素与列表的定位选择器**逐字取自上游 mod.js**
 * （`TASKBAR_ROOT_SELECTOR` / `TASKBAR_LIST_SELECTOR`，见 `setupTaskbarItemAnimations` 与
 * `resolveTaskbarNodes`）——上游已经替我们确认过「这个类名组合就是 Dock」，本壳不自己
 * 发明选择器、也不改上游节点的任何内容，只在自己的命名空间里加两个 class
 * （`fnos-shell-dock-autohide` / `fnos-shell-dock-hidden`）。
 *
 * 显隐规则（T14c 修复轮 3 定稿）：
 *   唤出   —— 指针顶到 Dock 所在的屏幕边缘热区（EDGE_PX）。**不是**「进入 Dock 的脚印」：
 *            藏在 Dock 底下的应用按钮就在那条带里，一靠近就唤出会把要点的按钮盖住。
 *   保持   —— 唤出后指针留在 Dock 脚印（自身尺寸 + 余量）内就一直显示，离开即藏（迟滞）。
 *   藏起   —— 指针离开脚印 / 离开窗口 / 窗口失焦 / **无操作超过 IDLE_MS** / 接管时。
 *            「无操作也藏」是硬需求：指针进入 iframe（fnOS 的应用窗口）之后，顶层文档
 *            收不到 pointermove——纯事件驱动的显隐会卡在「显示」，表现为「放着不动它不藏」。
 *   离场   —— 滑出动画结束后 `display:none`：transform 藏得住视觉，藏不住按 rect 做的
 *            工作区计算（全屏应用给 Dock 让出的那条宽度）与命中测试的旧读数。
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

  // 逐字 = 上游 mod.js 的 TASKBAR_ROOT_SELECTOR。改这里必须连上游那份一起核对，
  // tests/dock.test.mjs 会把两边源码里的这条选择器逐字比对。
  var DOCK_ROOT_SELECTOR = '.h-screen.fixed.left-0';
  // 逐字 = 上游 mod.js 的 TASKBAR_LIST_SELECTOR。上游的 resolveTaskbarNodes（mod.js:1222-1229）
  // 用「根节点里能找到这条列表」确认找对了元素——本壳镜像同一条验证：页面上可能有多个
  // `.h-screen.fixed.left-0` 容器，querySelector 取第一个可能拿错，接管一个空容器的表现
  // 恰好就是「开了开关什么都没发生」（T14c 修复轮 2）。
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

  var enabled = SHELL_CFG.dockAutoHide === true;
  var dock = null;       // 当前接管的 Dock 元素（可能被 SPA 换掉，observer 会重新找）
  var observer = null;   // MutationObserver：Dock 出现 / 被替换时重新接管
  var locateScheduled = false;
  var axis = 'x';        // Dock 贴边方向：'x'（左右缘）或 'y'（上下缘）
  var edgeMin = true;    // true = 贴在坐标最小的一侧（左/上）；false = 右/下
  var shown = false;     // 显隐意图（与 HIDDEN_CLASS 互斥）；接管默认 false = 平时是藏着的
  var idleTimer = null;  // 无操作兜底定时器（armIdle）
  var displayTimer = null; // 滑出动画结束后真正 display:none 的定时器

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

  /**
   * 上游同款验证：候选根里能找到任务栏列表的才算数（`resolveTaskbarNodes` 的镜像）。
   * 列表还没渲染出来时退回第一个候选——上游同样容忍 `list: null`，observer 会在列表
   * 出现后重新定位。
   */
  function findDock() {
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

  /** 把 `shown` 的意图落到当前 Dock 上（幂等；接管后无条件调一次以防换元素后状态漂移）。 */
  function renderState() {
    if (!dock) return;
    if (displayTimer) { W.clearTimeout(displayTimer); displayTimer = null; }
    if (shown) {
      try { dock.style.display = ''; } catch (e) { /* 已被上游藏起来就算了 */ }
      void dock.offsetWidth; // 强制重排：display:none 刚恢复时要有动画起点，否则直接跳到位
      dock.classList.remove(HIDDEN_CLASS);
      return;
    }
    syncTransform();
    dock.classList.add(HIDDEN_CLASS);
    // 滑出动画结束后**真正离场**（transform 挡不住按 rect 的全屏工作区计算，修复轮 2）。
    displayTimer = W.setTimeout(function () {
      displayTimer = null;
      if (dock && !shown) {
        try { dock.style.display = 'none'; } catch (e) { /* 不支持就算了 */ }
      }
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
    locate();
  }

  function teardown() {
    if (observer) { observer.disconnect(); observer = null; }
    if (idleTimer) { W.clearTimeout(idleTimer); idleTimer = null; }
    if (displayTimer) { W.clearTimeout(displayTimer); displayTimer = null; }
    D.removeEventListener('pointermove', onPointerMove);
    D.removeEventListener('pointerleave', onHideSignal);
    W.removeEventListener('blur', onHideSignal);
    release(dock);
    dock = null;
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

  if (enabled) install();
})();
