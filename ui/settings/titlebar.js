/* fnOS Desktop — 一行式标题栏（标签 + 窗口控制）的渲染与指令分发（T14d 修复轮 3）。
 *
 * 这个脚本跑在 `titlebar` Webview 里（主窗口顶部 TAB_STRIP_H 高的一条）。状态由宿主
 * （Rust `commands::push_tabs`）通过 `__FNOS_TABS_SET__` 推送——它是**单向**的：本脚本
 * 不维护任何状态，只渲染最近一次推送，并把用户点击翻译成 invoke 命令。
 *
 * 用户要求（修复轮 2 + 3）：最大化 / 最小化 / 关闭用**系统默认样式**——右对齐的
 * ─ □ ×（Windows 11 原生观感：悬停浅灰、关闭悬停红），固定不变、不跟随上游 mod 的
 * 标题栏样式；桌面内置应用窗口的样式仍由上游 mod 的「标题栏样式」设置接管。
 *
 * 命令授权（capabilities/titlebar.json）：tab_new / tab_switch / tab_close + 窗口的
 * minimize / toggle_maximize / close + 拖拽（data-tauri-drag-region）。
 * 除这些之外没有任何能力——这里连配置都读不到。
 */
(function () {
  'use strict';

  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document || (typeof document !== 'undefined' ? document : null);
  var STRIP_H = 40; // 与 Rust 侧 TAB_STRIP_H 一致（有跨语言断言）

  var invoke = function (cmd, args) {
    var internals = W.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') return;
    try {
      internals.invoke(cmd, args);
    } catch (_e) {
      /* 命令失败不致命：栏还在，用户可以再点 */
    }
  };

  /** 渲染一次标签条。state = { tabs: [{id, title, active, canClose}] }。 */
  function render(state) {
    var tabs = state && Array.isArray(state.tabs) ? state.tabs : [];
    var host = D.getElementById('tabs');
    if (!host) return;
    host.textContent = '';
    tabs.forEach(function (tab) {
      var item = D.createElement('div');
      item.className = 'tab' + (tab.active ? ' active' : '');
      item.dataset.tabId = String(tab.id);
      item.dataset.action = 'switch';
      item.title = String(tab.title == null ? tab.id : tab.title);
      var title = D.createElement('span');
      title.className = 'tab-title';
      title.textContent = String(tab.title == null ? tab.id : tab.title);
      item.appendChild(title);
      if (tab.canClose) {
        var close = D.createElement('button');
        close.className = 'tab-close';
        close.dataset.action = 'close';
        close.dataset.tabId = String(tab.id);
        close.textContent = '×';
        close.title = '关闭标签页';
        item.appendChild(close);
      }
      host.appendChild(item);
    });
  }

  /**
   * 窗口控制区（**系统默认观感**，固定 windows 形态）：右对齐 ─ □ ×。
   * 关闭走主窗口的 CloseRequested：照样服从 closeToTray（收起而不是退出）。
   */
  function renderControls() {
    var host = D.getElementById('controls');
    if (!host) return;
    host.textContent = '';
    var button = function (cls, glyph, action, title) {
      var b = D.createElement('button');
      b.className = cls;
      b.dataset.action = action;
      b.textContent = glyph;
      b.title = title;
      host.appendChild(b);
    };
    button('win-btn', '─', 'minimize', '最小化');
    button('win-btn', '□', 'maximize', '最大化 / 还原');
    button('win-btn close', '×', 'close', '关闭');
  }

  /** 点击分发：`data-action` → invoke。close/switch 带上所在标签的 id。 */
  function onClick(ev) {
    var el = ev.target;
    while (el && el !== D.body && !(el.dataset && el.dataset.action)) {
      el = el.parentElement;
    }
    if (!el || !el.dataset || !el.dataset.action) return;
    var action = el.dataset.action;
    if (action === 'tab-new') {
      invoke('tab_new');
      return;
    }
    if (action === 'switch') {
      var tabId = el.dataset.tabId;
      if (tabId) invoke('tab_switch', { label: tabId });
      return;
    }
    if (action === 'close' && el.dataset.tabId) {
      // 标签页上的 ×：关的是那个标签
      invoke('tab_close', { label: el.dataset.tabId });
      return;
    }
    if (action === 'minimize') {
      invoke('plugin:window|minimize');
      return;
    }
    if (action === 'maximize') {
      invoke('plugin:window|toggle_maximize');
      return;
    }
    if (action === 'close') {
      // 窗口 ×：走主窗口的 CloseRequested，照样服从 closeToTray
      invoke('plugin:window|close');
    }
  }

  // 宿主推送标签条状态（Rust `push_tabs`；eval 是单向的，所以用全局函数回话）。
  // 必须在脚本顶层就位：宿主的第一条推送可能先于 boot 的事件绑定到达。
  window.__FNOS_TABS_SET__ = render;

  function boot() {
    var bar = D.getElementById('titlebar');
    if (!bar) return; // 没有宿主结构就什么都不做（单测环境）
    bar.style.height = STRIP_H + 'px';
    renderControls();
    D.addEventListener('click', onClick);
    // 双击标签条空白区 = 最大化 / 还原（系统标题栏的标准语义）
    bar.addEventListener('dblclick', function (ev) {
      if (ev.target.dataset && ev.target.dataset.action) return;
      if (ev.target.closest && ev.target.closest('.tabs')) return;
      invoke('plugin:window|toggle_maximize');
    });
  }

  // 真实页面自动启动；单测只导入 render/renderControls（无 DOM 断言隔离）
  if (D && D.getElementById('titlebar')) {
    boot();
  }

  window.__FNOS_TITLEBAR__ = { render: render, renderControls: renderControls };
})();
