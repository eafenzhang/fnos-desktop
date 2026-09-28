/* fnOS Desktop Shell — 注入装配与降级
 * 1) 记录受管 CSS link；若 data: 外链被 CSP 拦掉，改用可构造样式表（CSP 免疫）
 * 2) 确保 mod.js 一定执行；优先让上游 <script src=data:> 执行，失败则自行执行原文 */
(function () {
  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document;
  var SHELL = W.__FNOS_SHELL__ || { assets: {} };
  var META = SHELL.meta || {};
  var CSS_IDS = [
    'fnos-ui-mods-basic-style',
    'fnos-ui-mods-titlebar-style',
    'fnos-ui-mods-launchpad-style',
    'fnos-ui-mods-desktop-icon-mod-style',
    'fnos-ui-mods-lockscreen-style'
  ];
  var CSS_FILES = [
    'basic_mod.css',
    'windows_titlebar_mod.css',
    'mac_titlebar_mod.css',
    'classic_launchpad_mod.css',
    'spotlight_launchpad_mod.css',
    'desktop_icon_mod.css',
    'lockscreen_mod.css'
  ];

  var state = {
    version: (META.shellVersion || '0.0.0'),
    cssIds: CSS_IDS.slice(),
    fallbackInstalled: 0,
    modFallbackUsed: false
  };

  function installFallbackCss() {
    if (!D.adoptedStyleSheets || typeof CSSStyleSheet !== 'function') return;
    var sheets = [];
    for (var i = 0; i < CSS_FILES.length; i++) {
      var text = SHELL.assets && SHELL.assets[CSS_FILES[i]];
      if (typeof text !== 'string') continue;
      try {
        var sheet = new CSSStyleSheet();
        sheet.replaceSync(text);
        sheets.push(sheet);
        state.fallbackInstalled++;
      } catch (e) { /* replaceSync 不可用则放弃该文件 */ }
    }
    if (!sheets.length) return;
    try {
      D.adoptedStyleSheets = D.adoptedStyleSheets.concat(sheets);
    } catch (e) { /* ignore */ }
  }

  function cssMissing() {
    for (var i = 0; i < CSS_IDS.length; i++) {
      var el = D.getElementById(CSS_IDS[i]);
      if (el && !el.sheet) return true;
    }
    // 一个 link 都没有：上游可能还没跑到，交给上游
    return false;
  }

  // 执行 mod.js 原文。优先用 Function 显式绑定当前 window/document：
  // 初始化脚本没有真正的全局脚本作用域（沙箱/测试里 (0,eval) 拿不到 W），而 mod.js 是
  // 自足脚本、对外只经 window.* 暴露（mod.js:1/93/292/375/1263/1286/1432），入参绑定与
  // 全局脚本等价。Function 被 CSP 拦（无 unsafe-eval）时退回作用域 eval。
  function executeModJs(text) {
    var code = text + '\n;window.__FNOS_MOD_EXECUTED__=true;';
    var runner = null;
    try {
      runner = new Function('window', 'document', code);
    } catch (e) {
      runner = null;
    }
    if (runner) {
      runner(W, D);
      return;
    }
    (0, eval)(code);
  }

  function ensureModJs() {
    if (W.__FNOS_MOD_EXECUTED__) return; // 上游 data: <script> 已执行 → no-op
    var text = SHELL.assets && SHELL.assets['mod.js'];
    if (typeof text !== 'string') return;
    state.modFallbackUsed = true;
    try {
      executeModJs(text);
    } catch (e) { /* 执行失败也必须置位，避免上游反复兜底 */ }
    W.__FNOS_MOD_EXECUTED__ = true;
  }

  W.__FNOS_BOOTSTRAP__ = {
    version: state.version,
    cssIds: state.cssIds,
    get fallbackInstalled() { return state.fallbackInstalled; },
    get modFallbackUsed() { return state.modFallbackUsed; },
    installFallbackCss: installFallbackCss,
    ensureModJs: ensureModJs
  };

  function afterLoad() {
    if (cssMissing()) installFallbackCss();
    setTimeout(ensureModJs, 120);
    setTimeout(function () {
      if (cssMissing()) installFallbackCss();
    }, 600);
  }

  if (D.readyState === 'loading') {
    D.addEventListener('DOMContentLoaded', afterLoad);
  } else {
    afterLoad();
  }
})();
