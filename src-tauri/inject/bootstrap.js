/* fnOS Desktop Shell — 注入装配与降级
 * 1) 记录受管 CSS link；若 data: 外链被 CSP 拦掉，改用可构造样式表（CSP 免疫）
 * 2) 确保 mod.js 一定执行；优先让上游 <script src=data:> 执行，失败则自行执行原文
 *    §5.4：只在观察到 #fnos-ui-mods-script 出现后 ~100ms 才兜底——上游不注入就绝不执行 */
(function () {
  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document;
  var SHELL = W.__FNOS_SHELL__ || { assets: {} };
  var META = SHELL.meta || {};
  var MOD_SCRIPT_ID = 'fnos-ui-mods-script';
  var MOD_CHECK_DELAY = 100;   // §5.4 的 ~100ms 标记检查
  var CSS_RECHECK_DELAY = 600; // 上游 link 注入偏晚时的第二次自检
  var POLL_INTERVAL = 250;     // 无 MutationObserver 时的退化轮询间隔
  var POLL_MAX = 20;           // 有界：始终不出现就永不动手
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

  var MO = null;
  // window.MutationObserver 优先（真实页面 / harness 注入），退回裸全局名（typeof 安全）
  if (W && typeof W.MutationObserver === 'function') {
    MO = W.MutationObserver;
  } else if (typeof MutationObserver === 'function') {
    MO = MutationObserver;
  }

  var cssInstalled = {}; // 幂等闸门：已安装的 CSS 文件名（每份只装一次、只计一次）
  var state = {
    version: (META.shellVersion || '0.0.0'),
    cssIds: CSS_IDS.slice(),
    fallbackInstalled: 0,
    modFallbackUsed: false,
    modFallbackFailed: false
  };

  function installFallbackCss() {
    if (!D.adoptedStyleSheets || typeof CSSStyleSheet !== 'function') return;
    var sheets = [];
    var files = [];
    for (var i = 0; i < CSS_FILES.length; i++) {
      var name = CSS_FILES[i];
      if (cssInstalled[name]) continue; // 幂等：同一份不重复构造/不重复计数
      var text = SHELL.assets && SHELL.assets[name];
      if (typeof text !== 'string') continue;
      try {
        var sheet = new CSSStyleSheet();
        sheet.replaceSync(text);
        sheets.push(sheet);
        files.push(name);
      } catch (e) { /* replaceSync 不可用则放弃该文件 */ }
    }
    if (!sheets.length) return;
    try {
      D.adoptedStyleSheets = D.adoptedStyleSheets.concat(sheets);
    } catch (e) {
      return; // 安装失败不记账，下次自检仍可重试
    }
    for (var j = 0; j < files.length; j++) cssInstalled[files[j]] = true;
    state.fallbackInstalled += sheets.length;
  }

  function cssMissing() {
    for (var i = 0; i < CSS_IDS.length; i++) {
      var el = D.getElementById(CSS_IDS[i]);
      if (el && !el.sheet) return true;
    }
    // 一个 link 都没有：上游可能还没跑到，交给上游
    return false;
  }

  function cssSelfCheck() {
    if (cssMissing()) installFallbackCss();
  }

  // ---------- 本壳自己的样式覆盖（与上游 mod 无关，永远装） ----------

  /**
   * 唯一一条覆盖规则：**「绝对定位的整屏覆盖层」必须不透明**。
   *
   * 上游 mod 把 `.bg-[var(--semi-color-app-container)]` 统一改成 50% 透明的玻璃效果
   * （`basic_mod.css` 的 `background-color: color-mix(… 50%, transparent) !important`）。
   * 它在普通窗口上好看，但应用中心的**应用详情**正是用这个类的整屏覆盖层
   * （`absolute inset-0 z-10 …`，实测），半透明会让底下的应用列表穿透上来、与详情文字重叠
   * ——用户实测反馈的「应用详情没有背景底色」。
   *
   * 修法是**在同一属性上用更高特异性压回去**（三条类 0,3,0 > 一条类 0,1,0，两边都是
   * `!important` 时由特异性决出胜负）：只命中「绝对定位 + 铺满父容器」的覆盖层，
   * 普通窗口/内容区的玻璃效果原样保留。
   */
  var SHELL_CSS_ID = 'fnos-shell-overrides';
  var SHELL_CSS =
    '.bg-\\[var\\(--semi-color-app-container\\)\\].absolute.inset-0' +
    '{background-color:var(--semi-color-app-container) !important;}';
  var shellCssInstalled = false;

  function installShellCss() {
    if (shellCssInstalled) return;
    // 与上游 CSS 兜底同一条路：优先可构造样式表（CSP 免疫）
    if (D.adoptedStyleSheets && typeof CSSStyleSheet === 'function') {
      try {
        var sheet = new CSSStyleSheet();
        sheet.replaceSync(SHELL_CSS);
        D.adoptedStyleSheets = D.adoptedStyleSheets.concat([sheet]);
        shellCssInstalled = true;
        state.shellCss = 'adopted';
        return;
      } catch (e) { /* 落到 <style> */ }
    }
    if (D.getElementById(SHELL_CSS_ID)) {
      shellCssInstalled = true;
      state.shellCss = 'style';
      return;
    }
    try {
      var el = D.createElement('style');
      el.id = SHELL_CSS_ID;
      el.textContent = SHELL_CSS;
      (D.head || D.documentElement).appendChild(el);
      shellCssInstalled = true;
      state.shellCss = 'style';
    } catch (e) {
      state.shellCss = 'failed'; // 装不上不静默：状态里留痕（排障用）
    }
  }

  /* 执行 mod.js 原文。优先用 Function 显式绑定当前 window/document：
   * 初始化脚本没有真正的全局脚本作用域（沙箱/测试里 (0,eval) 拿不到 W），而 mod.js 是
   * 自足脚本、对外只经 window.* 暴露（mod.js:1/93/292/375/1286/1432），入参绑定与
   * 全局脚本等价。Function 被 CSP 拦（无 unsafe-eval）时退回作用域 eval。
   *
   * 与上游经典 <script src="data:..."> 的语义差异（已知，不构成风险）：
   * new Function 体内是函数作用域，顶层的 `function foo(){}` 声明不会像经典脚本那样
   * 变成 window.foo。已确认 content-script.js 对这些顶层名字零引用（它只经 window.*
   * 访问 mod.js 自曝的 figmaSquircleConfig/_cleanupSquircleObservers/applyFigmaSquircle/
   * initFigmaSquircles/_fnos*Initialized），故两条路径的行为一致。
   * 执行失败（两条路径都被 CSP 拒）时抛出，由调用方记录失败且不置执行标记。 */
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
    try {
      executeModJs(text);
    } catch (e) {
      // 双路径都失败：记为可观测失败，不置 __FNOS_MOD_EXECUTED__（留给后续合法路径）
      state.modFallbackFailed = true;
      W.__FNOS_MOD_FALLBACK_FAILED__ = true;
      return;
    }
    state.modFallbackUsed = true; // 只有真的执行成功才算兜底生效
    W.__FNOS_MOD_EXECUTED__ = true;
  }

  function findModScript() {
    if (!D || typeof D.getElementById !== 'function') return null;
    return D.getElementById(MOD_SCRIPT_ID);
  }

  // 元素已出现：上游的 <script src="data:...mod.js"> 已挂进 DOM（其是否被执行由 CSP
  // 决定）。先补一次 CSS 自检（上游 link 与 script 同批注入，§5.3 的“link 晚到且被拦”
  // 只能在这里补上），再等 ~100ms 看执行标记，未置位才由 bootstrap 执行原文。
  function onModScriptSeen() {
    cssSelfCheck();
    setTimeout(function () {
      cssSelfCheck();
      if (!W.__FNOS_MOD_EXECUTED__) ensureModJs();
    }, MOD_CHECK_DELAY);
  }

  // §5.4：观察 #fnos-ui-mods-script 的出现。上游不注入（未命中签名 / 未启用
  // autoEnableSuspectedFnOS）时本兜底永不执行——这是“挂到非飞牛页面”的闸门。
  function watchModScript() {
    if (findModScript()) { // 装配时已存在：立即进入检查
      onModScriptSeen();
      return;
    }
    if (typeof MO === 'function') {
      var root = D.documentElement || D;
      var seen = false;
      var observer = null;
      try {
        observer = new MO(function () {
          if (seen || !findModScript()) return;
          seen = true;
          try { observer.disconnect(); } catch (e) { /* ignore */ }
          onModScriptSeen();
        });
        observer.observe(root, { childList: true, subtree: true });
        return;
      } catch (e) { /* 构造/观察失败 → 退化轮询 */ }
    }
    pollModScript();
  }

  // 退化路径：有界轮询（250ms × 20）。语义与观察者一致：始终不出现就永不执行。
  function pollModScript() {
    var attempts = 0;
    function tick() {
      if (findModScript()) { onModScriptSeen(); return; }
      attempts++;
      if (attempts >= POLL_MAX) return; // 有界放弃，绝不无条件执行
      setTimeout(tick, POLL_INTERVAL);
    }
    tick();
  }

  W.__FNOS_BOOTSTRAP__ = {
    version: state.version,
    cssIds: state.cssIds,
    get fallbackInstalled() { return state.fallbackInstalled; },
    get modFallbackUsed() { return state.modFallbackUsed; },
    get modFallbackFailed() { return state.modFallbackFailed; },
    installFallbackCss: installFallbackCss,
    ensureModJs: ensureModJs
  };

  function afterLoad() {
    installShellCss();
    cssSelfCheck();
    watchModScript();
    setTimeout(cssSelfCheck, CSS_RECHECK_DELAY);
  }

  if (D.readyState === 'loading') {
    D.addEventListener('DOMContentLoaded', afterLoad);
  } else {
    afterLoad();
  }
})();
