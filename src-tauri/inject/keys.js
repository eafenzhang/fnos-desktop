/* fnOS Desktop Shell — 标签页键盘快捷键（P0-3）
 *
 * 背景：T14d 起外部应用开成主窗口里的标签页（浏览器语义），但**一条快捷键都没绑**——
 * 全仓搜不到 `keydown`。用户按 Ctrl+T / Ctrl+W / Ctrl+Tab 时，按键落进页面、什么都不发生。
 * 既然标签条长得像浏览器，这几个组合键就该能用。
 *
 * ## 为什么必须走「页面 → 宿主」的标题通道，而不是别的
 *
 * 键盘焦点在 **WebView2 子 Webview**（`main` / `tab-N`）里，不在 tauri 的 Window 上，
 * 所以 `WindowEvent::KeyboardInput` 根本收不到这些键；而给 WebView2 装
 * `AcceleratorKeyPressed` 需要直接拿 COM 接口（`with_webview` + 裸 unsafe），对一个自用壳
 * 来说是不必要的风险面。本壳已经有一条**不占任何权限**的页面→宿主通路：写
 * `document.title`（`report.rs` / `commands.rs::handle_title`）。这里沿用同一条通路：
 *   `FNOSKEY:<name>` → 宿主执行、**不**镜像到窗口标题。
 *
 * ## 安全边界
 *
 * - 只认白名单里的四个动作（`next` / `prev` / `close` / `new`），别名一律不认；
 * - 任何页面都能写标题，所以**这不构成额外授权**：它能做的只是「切/关/开标签」，
 *   与用户亲手点标签条完全等价，命令本身仍由宿主侧的既有函数执行；
 * - 宿主侧还有一道 120ms 冷却（见 `commands.rs::KEY_COOLDOWN_MS`），连发/刷屏被折成一次；
 * - 本文件**不读页面内容、不改 DOM**，只 `preventDefault` 掉它自己要消费的那几个组合键。
 *
 * ## 组合键（与 Windows 上的浏览器一致）
 *
 *   Ctrl+T            新标签页（开 fnOS 主页，与标签条「+」同一入口）
 *   Ctrl+W            关闭当前标签页（fnOS 主页标签不可关，宿主会拒绝）
 *   Ctrl+Tab          下一个标签页
 *   Ctrl+Shift+Tab    上一个标签页
 *
 * 带 Alt / Meta 的一律不碰（那属于系统级快捷键）；只认**物理** Ctrl。
 */
(function () {
  'use strict';

  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document;
  if (!D || typeof D.addEventListener !== 'function') return;

  // 只在顶层文档工作（与 dock.js / keepalive.js 同一条纪律）
  var isTopFrame = false;
  try { isTopFrame = W.top === W.self; } catch (e) { isTopFrame = false; }
  if (!isTopFrame) return;

  var KEY_TITLE_PREFIX = 'FNOSKEY:';
  var RESTORE_DELAY_MS = 60; // 让 WebView2 至少上报一次改后的标题（同帧两次改动会被合并）

  var reportBase = null;
  var restoreTimer = null;

  /** 把控制标题还给页面（只在自己写过的那条还以控制前缀开头时才动）。 */
  function restoreTitle() {
    restoreTimer = null;
    try {
      if (String(D.title || '').indexOf(KEY_TITLE_PREFIX) !== 0) return;
      D.title = reportBase === null ? '' : reportBase;
    } catch (e) { /* 写标题失败：页面标题本来就写得动，这里只当没发生 */ }
    reportBase = null;
  }

  /** 写一条快捷键命令到标题通道。 */
  function emit(action) {
    try {
      if (reportBase === null) reportBase = String(D.title || '');
      D.title = KEY_TITLE_PREFIX + action;
    } catch (e) {
      reportBase = null;
      return;
    }
    if (restoreTimer !== null) W.clearTimeout(restoreTimer);
    restoreTimer = W.setTimeout(restoreTitle, RESTORE_DELAY_MS);
  }

  /** 事件目标是不是「用户正在输入」的地方——不在这里抢键（保留页面的编辑语义）。 */
  function inEditable(el) {
    if (!el) return false;
    var tag = String(el.tagName || '').toUpperCase();
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
    try { if (el.isContentEditable === true) return true; } catch (e) { /* 忽略 */ }
    return false;
  }

  /**
   * 把按键映射成动作名。只认精确组合，不做「宽松匹配」：
   * Ctrl+T / Ctrl+W / Ctrl+Tab / Ctrl+Shift+Tab。
   *
   * **编辑区的例外只给「会关掉东西」的那两个**：`next` / `prev` 是纯导航，在搜索框里也
   * 应该能用；而 `new` / `close` 在输入框里会让人瞬间丢掉正在填的表单（fnOS 有重命名、
   * 搜索、密码框这类输入），所以那两个只在非编辑区生效。
   */
  function actionFor(e) {
    if (!e || e.repeat) return null;            // 长按不连发（浏览器的切标签也不是连发）
    if (!e.ctrlKey || e.altKey || e.metaKey) return null; // 只认物理 Ctrl；Alt/Meta 留给系统
    var key = String(e.key || '');
    var lower = key.toLowerCase();
    if (key === 'Tab') {
      return e.shiftKey ? 'prev' : 'next';      // 导航：编辑区里也放行
    }
    if (e.shiftKey) return null;                // Ctrl+Shift+字母 不是我们要的
    if (lower !== 't' && lower !== 'w') return null;
    if (inEditable(e.target)) return null;      // 会关掉东西的动作不在编辑区里抢键
    return lower === 't' ? 'new' : 'close';
  }

  D.addEventListener('keydown', function (e) {
    var action = actionFor(e);
    if (!action) return;
    // 只有真的要消费时才 preventDefault：绝不能把页面的其它组合键吃掉。
    try { e.preventDefault(); } catch (err) { /* 不可取消就照发（宿主侧仍会执行） */ }
    try { e.stopPropagation(); } catch (err) { /* 忽略 */ }
    emit(action);
  }, true);

  /** 诊断口（控制台可见，页面上不画任何东西）。 */
  W.__FNOS_KEYS_STATE__ = function () {
    return { prefix: KEY_TITLE_PREFIX, pending: restoreTimer !== null };
  };
})();
