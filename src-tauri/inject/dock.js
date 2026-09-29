/* fnOS Desktop Shell — Dock 自动隐藏（T14c，本壳自己的页面功能，不是上游 mod）
 *
 * 做什么：把 fnOS WebUI 的 Dock（任务栏）平时滑出屏幕边缘，鼠标移近它所在的边缘时滑回。
 * 策略（上游类名策略）：Dock 根元素的定位选择器**逐字取自上游 mod.js** 的
 * `TASKBAR_ROOT_SELECTOR`（mod.js::setupTaskbarItemAnimations）——上游已经替我们确认过
 * 「这个类名组合就是 Dock」，本壳不自己发明选择器、也不改上游节点的任何内容，
 * 只在自己的命名空间里加两个 class（`fnos-shell-dock-autohide` / `fnos-shell-dock-hidden`）。
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
  var HOST_CLASS = 'fnos-shell-dock-autohide';   // 常驻：宣告「本壳在管这个元素」
  var HIDDEN_CLASS = 'fnos-shell-dock-hidden';   // 状态：平时滑出，指针靠近边缘时移除
  var STYLE_ID = 'fnos-shell-dock-style';
  var EDGE_PX = 6;         // 屏幕边缘热区宽度（比 Dock 厚度窄得多，不挡正常操作）
  var SLACK_PX = 12;       // 「还在 Dock 上」判定的余量（PointerEvent 抖动 + 边框）
  var LOCATE_DELAY_MS = 150; // SPA 重渲染的合并窗口：一批变更只查一次选择器

  var enabled = SHELL_CFG.dockAutoHide === true;
  var dock = null;       // 当前接管的 Dock 元素（可能被 SPA 换掉，observer 会重新找）
  var observer = null;   // MutationObserver：Dock 出现 / 被替换时重新接管
  var locateScheduled = false;
  var axis = 'x';        // Dock 贴边方向：'x'（左右缘）或 'y'（上下缘）
  var edgeMin = true;    // true = 贴在坐标最小的一侧（左/上）；false = 右/下

  var CSS =
    '.' + HOST_CLASS + '{transition:transform .28s cubic-bezier(.4,0,.2,1);will-change:transform;}' +
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

  /** Dock 贴在哪条边上？按接管时的真实几何判断，量不出来按左缘（上游的实际形态）处理。 */
  function measure(el) {
    var r = el.getBoundingClientRect();
    var vw = W.innerWidth || 0;
    var vh = W.innerHeight || 0;
    if (r.left <= 1) { axis = 'x'; edgeMin = true; return 'translateX(-105%)'; }
    if (vw && r.right >= vw - 1) { axis = 'x'; edgeMin = false; return 'translateX(105%)'; }
    if (r.top <= 1) { axis = 'y'; edgeMin = true; return 'translateY(-105%)'; }
    if (vh && r.bottom >= vh - 1) { axis = 'y'; edgeMin = false; return 'translateY(105%)'; }
    axis = 'x'; edgeMin = true;
    return 'translateX(-105%)';
  }

  /** 释放一个曾接管的元素：清掉本壳加的两个 class 和内联的隐藏变换变量（完全还原）。 */
  function release(el) {
    if (!el) return;
    el.classList.remove(HOST_CLASS, HIDDEN_CLASS);
    try { el.style.removeProperty('--fnos-dock-hide-tf'); } catch (e) { /* 不支持就算了 */ }
  }

  /** 重新找 Dock 并接管。已接管的元素不变；旧元素被换掉时把自己加的东西清干净。 */
  function locate() {
    locateScheduled = false;
    var found = null;
    try { found = D.querySelector(DOCK_ROOT_SELECTOR); } catch (e) { return; }
    if (!found) return;
    if (found === dock) return;
    release(dock);
    dock = found;
    dock.classList.add(HOST_CLASS);
    dock.style.setProperty('--fnos-dock-hide-tf', measure(dock));
  }

  /** observer 回调的合并入口：SPA 一批变更只安排一次重找。 */
  function scheduleLocate() {
    if (locateScheduled) return;
    locateScheduled = true;
    W.setTimeout(locate, LOCATE_DELAY_MS);
  }

  function setHidden(hidden) {
    if (dock) dock.classList.toggle(HIDDEN_CLASS, hidden);
  }

  /**
   * 指针驱动显隐。`over` 用 **元素尺寸**（translate 不改变 width/height）而不是实时 rect：
   * Dock 藏起来之后 rect 会滑出屏幕，按实时 rect 判「还在 Dock 上吗」对贴右/下缘的形态
   * 会恒真（rect.right 反而更大）。热区（near）只认屏幕边缘那几像素。
   */
  function onPointerMove(e) {
    if (!enabled || !dock) return;
    var vw = W.innerWidth || 0;
    var vh = W.innerHeight || 0;
    var size = axis === 'y' ? dock.offsetHeight : dock.offsetWidth;
    var coord = axis === 'y' ? e.clientY : e.clientX;
    var span = axis === 'y' ? vh : vw;
    var near = edgeMin ? coord <= EDGE_PX : (span ? coord >= span - EDGE_PX : false);
    var over = edgeMin ? coord <= size + SLACK_PX : (span ? coord >= span - size - SLACK_PX : false);
    setHidden(!(near || over));
  }

  function install() {
    ensureStyle();
    if (!observer && typeof W.MutationObserver === 'function') {
      observer = new W.MutationObserver(scheduleLocate);
      // 观察 document：本脚本在 document-start 注入，body 可能还不存在
      observer.observe(D, { childList: true, subtree: true });
    }
    D.addEventListener('pointermove', onPointerMove, { passive: true });
    locate();
  }

  function teardown() {
    if (observer) { observer.disconnect(); observer = null; }
    D.removeEventListener('pointermove', onPointerMove);
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
