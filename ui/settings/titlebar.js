/* fnOS Desktop — 一行式标题栏（标签 + 窗口控制）的渲染与指令分发（T14d 修复轮 4）。
 *
 * 这个脚本跑在 `titlebar` Webview 里（主窗口顶部 TAB_STRIP_H 高的一条）。状态由宿主
 * （Rust `commands::push_tabs` / `relayout_main_window`）单向推送：
 *   `__FNOS_TABS_SET__`   —— 标签列表（渲染最近一次推送，本脚本不维护状态）；
 *   `__FNOS_WIN_MAX_SET__` —— 窗口是否最大化（切换 □ / 还原 的按钮字形）。
 *
 * 用户要求（修复轮 2 → 4）：最大化 / 最小化 / 关闭用**系统默认样式**——右对齐按钮、
 * **系统图标字体**（Segoe Fluent Icons / Segoe MDL2 Assets，与 Windows 标题栏同款字形
 * 与尺寸），最大化 ↔ 还原字形随状态切换。桌面内置应用窗口的样式仍由上游 mod 的
 * 「标题栏样式」设置接管，与本栏无关。
 *
 * 命令授权（capabilities/titlebar.json）：tab_new / tab_switch / tab_close + 窗口的
 * minimize / toggle_maximize / close + 拖拽（data-tauri-drag-region）。
 */
(function () {
  'use strict';

  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document || (typeof document !== 'undefined' ? document : null);
  var STRIP_H = 32; // 与 Rust 侧 TAB_STRIP_H 一致（有跨语言断言；= Windows 11 caption 高度）

  // 系统标题栏同款字形：Segoe Fluent Icons（Win11）与 Segoe MDL2 Assets（Win10）共用
  // 这些码点，字体栈在 CSS 里逐级回落。字符就是私用区码点本身（U+E921/E922/E923/E8BB，
  // 字节已核对），文件以 UTF-8 保存。
  var GLYPH_MINIMIZE = ''; // minimize ─
  var GLYPH_MAXIMIZE = ''; // maximize □
  var GLYPH_RESTORE = ''; // restore（最大化状态的按钮字形）
  var GLYPH_CLOSE = ''; // close ×
  // 全屏按钮用用户参考图的「四角括号」图标：inline SVG，不依赖 Segoe 字形在不同
  // Windows 版本上的形状差异。四段 L 形路径，中间留空；进入 = 外角，退出 = 内角。
  var FS_PATHS_ENTER = ['M2 6V2H6', 'M10 2H14V6', 'M2 10V14H6', 'M10 14H14V10'];
  var FS_PATHS_EXIT = ['M6 2V6H2', 'M10 2V6H14', 'M6 14V10H2', 'M10 14V10H14'];

  var invoke = function (cmd, args) {
    var internals = W.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') return;
    try {
      internals.invoke(cmd, args);
    } catch (_e) {
      /* 命令失败不致命：栏还在，用户可以再点 */
    }
  };

  /** 四角括号 SVG（16×16 viewBox；颜色/线宽由 CSS 控制）。 */
  function fullscreenIcon(paths) {
    if (!D) return null;
    var make = typeof D.createElementNS === 'function'
      ? function (tag) { return D.createElementNS('http://www.w3.org/2000/svg', tag); }
      : function (tag) { return D.createElement(tag); };
    var svg = make('svg');
    svg.setAttribute('viewBox', '0 0 16 16');
    svg.setAttribute('aria-hidden', 'true');
    paths.forEach(function (d) {
      var path = make('path');
      path.setAttribute('d', d);
      svg.appendChild(path);
    });
    return svg;
  }

  /** fnOS 页面标题的固定后缀（「MiniNas - 飞牛 fnOS」→ NAS 名称「MiniNas」）。 */
  var TITLE_SUFFIX = /[-–]\s*飞牛\s*fnOS\s*$/;
  var HOME_FALLBACK = 'fnOS';

  /** 从 main 标签标题里剥出 NAS 名称（剥不出后缀就原样用；空了回退 fnOS）。 */
  function nasName(title) {
    var name = String(title == null ? '' : title).replace(TITLE_SUFFIX, '').trim();
    return name || HOME_FALLBACK;
  }

  /** 渲染一次标签条。state = { tabs: [{id, title, active, canClose}] }。
   *
   * `main`（fnOS 桌面）**不渲染成普通标签**，而是合并成最左的「桌面」元素：
   * 品牌图标 + NAS 名称（修复轮 6，用户要求），点击回到桌面；外部应用标签随后。
   *
   * 用户要求（标签样式改造）：**首页标签永远固定在最左**，与宿主推送顺序无关；
   * 重复的 `main` 条目只渲染一次，且永远不渲染关闭钮（main 不可关闭）。
   */
  var renderedSig = '';
  var renderedIds = [];
  var renderedNodes = Object.create(null);

  function childByClass(el, cls) {
    if (!el || !el.children) return null;
    for (var i = 0; i < el.children.length; i++) {
      if (String(el.children[i].className || '').split(/\s+/).indexOf(cls) >= 0) {
        return el.children[i];
      }
    }
    return null;
  }

  function scrollActive(host) {
    var active = null;
    for (var i = 0; i < host.children.length; i++) {
      var cls = String(host.children[i].className || '');
      if (cls.split(/\s+/).indexOf('active') >= 0) { active = host.children[i]; break; }
    }
    if (active && typeof active.scrollIntoView === 'function') {
      try { active.scrollIntoView({ inline: 'nearest', block: 'nearest' }); } catch (_e) { /* 老内核 */ }
    }
  }

  function updateHome(node, tab) {
    var name = nasName(tab.title);
    node.className = 'home' + (tab.active ? ' active' : '');
    node.title = name;
    node.setAttribute('aria-selected', tab.active ? 'true' : 'false');
    var label = childByClass(node, 'home-name');
    if (label) label.textContent = name;
  }

  function updateTab(node, tab) {
    var titleText = String(tab.title == null ? tab.id : tab.title);
    node.className = 'tab' + (tab.active ? ' active' : '');
    node.title = titleText;
    node.setAttribute('aria-selected', tab.active ? 'true' : 'false');
    var label = childByClass(node, 'tab-title');
    if (label) label.textContent = titleText;
    var close = childByClass(node, 'tab-close');
    if (close) close.setAttribute('aria-label', '关闭标签页：' + titleText);
  }

  function render(state) {
    var tabs = state && Array.isArray(state.tabs) ? state.tabs : [];
    var host = D.getElementById('tabs');
    if (!host) return;
    host.setAttribute('role', 'tablist');
    var home = null;
    var rest = [];
    tabs.forEach(function (tab) {
      if (tab.id === 'main') {
        if (!home) home = tab;
        return;
      }
      rest.push(tab);
    });
    var desiredIds = (home ? ['main'] : []).concat(rest.map(function (t) { return String(t.id); }));
    var sig = JSON.stringify({
      home: home ? { title: nasName(home.title), active: !!home.active } : null,
      rest: rest.map(function (t) {
        return {
          id: String(t.id),
          title: String(t.title == null ? t.id : t.title),
          active: !!t.active,
          canClose: !!t.canClose,
        };
      }),
    });
    if (sig === renderedSig) return;
    // 同一组标签（顺序不变、关闭钮结构不变）→ 原地更新文本/激活态。
    // 关键是不要替换节点：用户 mousedown 之后的后台标题更新若触发重建，mouseup
    // 会落在新节点上，点击就丢了。
    var sameIds = desiredIds.length === renderedIds.length &&
      desiredIds.every(function (id, i) { return id === renderedIds[i]; });
    var canReuse = sameIds && !!renderedNodes.main &&
      rest.every(function (t) { return !!renderedNodes[String(t.id)] === !!t.canClose; });
    if (canReuse) {
      updateHome(renderedNodes.main, home);
      rest.forEach(function (t) { updateTab(renderedNodes[String(t.id)], t); });
      renderedSig = sig;
      scrollActive(host);
      return;
    }
    // 标签集合/顺序/关闭钮结构变了：重建（这类变化是用户动作，不是标题抖动）。
    host.textContent = '';
    renderedNodes = Object.create(null);
    if (home) renderedNodes.main = renderHome(host, home);
    rest.forEach(function (tab) { renderedNodes[String(tab.id)] = renderTab(host, tab); });
    renderedIds = desiredIds;
    renderedSig = sig;
    scrollActive(host);
  }

  /** 「桌面」元素：fnOS 品牌图标 + NAS 名称，固定最左、不可关闭。 */
  function renderHome(host, tab) {
    var name = nasName(tab.title);
    var home = D.createElement('div');
    home.className = 'home' + (tab.active ? ' active' : '');
    home.dataset.tabId = 'main';
    home.dataset.action = 'switch';
    home.title = name;
    home.setAttribute('role', 'tab');
    home.setAttribute('aria-selected', tab.active ? 'true' : 'false');
    home.setAttribute('tabindex', '0');
    var img = D.createElement('img');
    img.src = 'icons/icon16.png';
    img.alt = 'fnOS';
    img.setAttribute('draggable', 'false');
    home.appendChild(img);
    var label = D.createElement('span');
    label.className = 'home-name';
    label.textContent = name;
    home.appendChild(label);
    host.appendChild(home);
    return home;
  }

  /** 一个外部应用标签（固定 144px；关闭钮由 canClose 决定）。 */
  function renderTab(host, tab) {
    var item = D.createElement('div');
    var titleText = String(tab.title == null ? tab.id : tab.title);
    item.className = 'tab' + (tab.active ? ' active' : '');
    item.dataset.tabId = String(tab.id);
    item.dataset.action = 'switch';
    item.title = titleText;
    item.setAttribute('role', 'tab');
    item.setAttribute('aria-selected', tab.active ? 'true' : 'false');
    item.setAttribute('tabindex', '0');
    var title = D.createElement('span');
    title.className = 'tab-title';
    title.textContent = titleText;
    item.appendChild(title);
    if (tab.canClose) {
      var close = D.createElement('button');
      close.className = 'tab-close';
      close.dataset.action = 'close';
      close.dataset.tabId = String(tab.id);
      close.textContent = GLYPH_CLOSE;
      close.title = '关闭标签页';
      close.setAttribute('aria-label', '关闭标签页：' + titleText);
      item.appendChild(close);
    }
    host.appendChild(item);
    return item;
  }

  /** minimize / maximize 按钮引用（供状态推送切换字形与禁用态）。 */
  var minBtn = null;
  var maxBtn = null;
  /** 全屏按钮引用与当前状态（宿主 __FNOS_WIN_FULLSCREEN_SET__ 推送）。 */
  var fullscreenBtn = null;
  var fullscreenOn = false;

  /**
   * 窗口控制区（**系统默认观感**，固定 windows 形态）：右对齐三个系统图标按钮。
   * 关闭走主窗口的 CloseRequested：照样服从 closeToTray（收起而不是退出）。
   */
  function renderControls() {
    var host = D.getElementById('controls');
    if (!host) return;
    host.textContent = '';
    minBtn = null;
    maxBtn = null;
    fullscreenBtn = null;
    var button = function (cls, glyph, action, title) {
      var b = D.createElement('button');
      b.className = cls;
      b.dataset.action = action;
      b.textContent = glyph;
      b.title = title;
      b.setAttribute('aria-label', title);
      host.appendChild(b);
      return b;
    };
    // 用户要求：全屏 icon 放在最小化左边；图标用四角括号 SVG（见 fullscreenIcon）。
    fullscreenBtn = button('win-btn fullscreen', '', 'fullscreen', '全屏');
    minBtn = button('win-btn', GLYPH_MINIMIZE, 'minimize', '最小化');
    maxBtn = button('win-btn maximize', GLYPH_MAXIMIZE, 'maximize', '最大化');
    button('win-btn close', GLYPH_CLOSE, 'close', '关闭');
    setFullscreenState(fullscreenOn);
  }

  /** 宿主推送的窗口最大化状态 → 切换按钮字形（系统标题栏语义）。 */
  function setMaxState(max) {
    if (!maxBtn) return;
    maxBtn.textContent = max ? GLYPH_RESTORE : GLYPH_MAXIMIZE;
    maxBtn.title = max ? '还原' : '最大化';
    maxBtn.setAttribute('aria-label', maxBtn.title);
  }

  /** 宿主推送的全屏状态 → 切换四角括号图标 / 提示，并处理窗口按钮的禁用态。 */
  function setFullscreenState(on) {
    fullscreenOn = !!on;
    if (fullscreenBtn) {
      fullscreenBtn.textContent = '';
      var icon = fullscreenIcon(fullscreenOn ? FS_PATHS_EXIT : FS_PATHS_ENTER);
      if (icon) fullscreenBtn.appendChild(icon);
      fullscreenBtn.dataset.fsState = fullscreenOn ? 'exit' : 'enter';
      fullscreenBtn.title = fullscreenOn ? '退出全屏（Esc）' : '全屏';
      fullscreenBtn.setAttribute('aria-label', fullscreenBtn.title);
    }
    // 全屏下任务栏已被藏起来：最小化会把用户困在一个没有任务栏的桌面；最大化对
    // fullscreen 窗口没有意义，还可能触发 toggle_maximize 把窗口退出全屏。两者禁用。
    if (minBtn) minBtn.disabled = fullscreenOn;
    if (maxBtn) maxBtn.disabled = fullscreenOn;
  }

  /** 点击分发：`data-action` → invoke。close/switch 带上所在标签的 id。 */
  function onClick(ev) {
    var el = ev.target;
    while (el && el !== D.body && !(el.dataset && el.dataset.action)) {
      el = el.parentElement;
    }
    if (!el || !el.dataset || !el.dataset.action) return;
    var action = el.dataset.action;
    if (action === 'home') {
      // 品牌图标：回到桌面标签（main），不新开、不重载
      invoke('tab_switch', { label: 'main' });
      return;
    }
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
    if (action === 'fullscreen') {
      // 状态由宿主 __FNOS_WIN_FULLSCREEN_SET__ 回推；这里只翻转当前已知状态。
      invoke('set_fullscreen', { value: !fullscreenOn });
      return;
    }
    if (action === 'minimize') {
      if (fullscreenOn) return; // 全屏下禁用（见 setFullscreenState）
      invoke('plugin:window|minimize');
      return;
    }
    if (action === 'maximize') {
      if (fullscreenOn) return; // 全屏下禁用（见 setFullscreenState）
      invoke('plugin:window|toggle_maximize');
      return;
    }
    if (action === 'close') {
      // 窗口 ×：走主窗口的 CloseRequested，照样服从 closeToTray
      invoke('plugin:window|close');
    }
  }

  // 宿主推送口（Rust eval；单向、必须在脚本顶层就位——首条推送可能先于 boot 到达）。
  function activeTabState() {
    var tabs = latestTabs && Array.isArray(latestTabs.tabs) ? latestTabs.tabs : [];
    for (var i = 0; i < tabs.length; i++) {
      if (tabs[i].active) return { tabs: tabs, index: i, tab: tabs[i] };
    }
    return { tabs: tabs, index: -1, tab: null };
  }

  function titlebarSwitch(delta) {
    var cur = activeTabState();
    if (!cur.tabs.length) return;
    var idx = cur.index >= 0 ? cur.index : 0;
    var next = (idx + delta + cur.tabs.length) % cur.tabs.length;
    var tab = cur.tabs[next];
    if (tab && tab.id) invoke('tab_switch', { label: tab.id });
  }

  /** 标题栏有焦点时的快捷键：与 keys.js 的语义一致（只认物理 Ctrl）。 */
  function onTitlebarKey(ev) {
    if (!ev) return;
    if (ev.key === 'Escape' && fullscreenOn) {
      try { ev.preventDefault(); } catch (_e) { /* 不可取消也照发 */ }
      invoke('set_fullscreen', { value: false });
      return;
    }
    if (!ev.ctrlKey && !ev.altKey && !ev.metaKey && !ev.shiftKey
      && (ev.key === 'Enter' || ev.key === ' ')) {
      var target = ev.target;
      if (target && target.dataset && target.dataset.action) {
        try { ev.preventDefault(); } catch (_e) { /* 忽略 */ }
        onClick(ev);
        return;
      }
    }
    if (!ev.ctrlKey || ev.altKey || ev.metaKey || ev.repeat) return;
    var key = String(ev.key || '');
    if (key === 'Tab') {
      try { ev.preventDefault(); } catch (_e) {}
      titlebarSwitch(ev.shiftKey ? -1 : 1);
      return;
    }
    if (ev.shiftKey) return;
    var lower = key.toLowerCase();
    if (lower === 't') {
      try { ev.preventDefault(); } catch (_e) {}
      invoke('tab_new');
      return;
    }
    if (lower === 'w') {
      var cur = activeTabState();
      if (cur.tab && cur.tab.id && cur.tab.id !== 'main') {
        try { ev.preventDefault(); } catch (_e) {}
        invoke('tab_close', { label: cur.tab.id });
      }
    }
  }

  /** 合并高频标签状态推送：可见时每帧最多渲染一次；隐藏（全屏悬浮栏）时同步渲染，
   *  因为隐藏页面的 rAF 可能被节流/暂停，不能让标签条带着旧状态被唤出。 */
  var pendingTabs = null;
  var tabsScheduled = false;
  var latestTabs = { tabs: [] };
  function scheduleTabs(state) {
    pendingTabs = state;
    latestTabs = state || { tabs: [] };
    if (typeof W.requestAnimationFrame !== 'function' || (D && D.hidden)) {
      render(state);
      return;
    }
    if (tabsScheduled) return;
    tabsScheduled = true;
    W.requestAnimationFrame(function () {
      tabsScheduled = false;
      render(pendingTabs);
    });
  }

  window.__FNOS_TABS_SET__ = scheduleTabs;
  window.__FNOS_WIN_MAX_SET__ = setMaxState;
  window.__FNOS_WIN_FULLSCREEN_SET__ = setFullscreenState;

  function boot() {
    var bar = D.getElementById('titlebar');
    if (!bar) return; // 没有宿主结构就什么都不做（单测环境）
    bar.style.height = STRIP_H + 'px';
    renderControls();
    D.addEventListener('click', onClick);
    // 焦点在标题栏时也要有 Esc / Ctrl+T/W/Tab（内容区由 keys.js 处理）。
    D.addEventListener('keydown', onTitlebarKey);
    // 标签溢出：滚轮映射到标签区的横向滚动
    var tabsEl = D.getElementById('tabs');
    if (tabsEl && typeof tabsEl.addEventListener === 'function') {
      tabsEl.addEventListener('wheel', function (ev) {
        if (Math.abs(ev.deltaY) <= Math.abs(ev.deltaX)) return;
        tabsEl.scrollLeft += ev.deltaY;
        try { ev.preventDefault(); } catch (_e) { /* 不可取消就算了 */ }
      }, { passive: false });
    }
    // 双击标签条空白区 = 最大化 / 还原（系统标题栏的标准语义）
    bar.addEventListener('dblclick', function (ev) {
      if (fullscreenOn) return; // 全屏下双击不得把窗口 toggle_maximize 出去
      if (ev.target.dataset && ev.target.dataset.action) return;
      if (ev.target.closest && ev.target.closest('.tabs')) return;
      invoke('plugin:window|toggle_maximize');
    });
  }

  // 真实页面自动启动；单测只导入 render/renderControls/setMaxState（无 DOM 断言隔离）
  if (D && D.getElementById('titlebar')) {
    boot();
  }

  window.__FNOS_TITLEBAR__ = {
    render: render,
    renderControls: renderControls,
    setMaxState: setMaxState,
    setFullscreenState: setFullscreenState,
    nasName: nasName,
  };
})();
