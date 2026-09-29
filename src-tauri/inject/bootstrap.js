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
   * 覆盖规则：**「整屏覆盖层」必须不透明**（应用详情看不到背景底色的实测修复）。
   *
   * 上游 mod 里有两处会把覆盖层弄成透明：
   * 1. `basic_mod.css` 的 `.bg-[var(--semi-color-app-container)]`（一条类，0,1,0）——
   *    统一改成 50% 玻璃效果：`color-mix(… 50%, transparent) !important`；
   * 2. `basic_mod.css:424` 的应用中心**专用**规则（约 0,4,1，同样 `!important`）——
   *    把 `.fnos-app-center-route-detail`（= 详情覆盖层本体）直接设成
   *    `background-color: transparent !important`，而「激活态」那条只改 opacity/transform、
   *    **没有**把背景改回来。于是详情透出底下的应用列表，与详情文字重叠。
   *
   * 所以这里给两条规则，且**特异性都压过上游**（两边都是 `!important` 时由特异性决出胜负）：
   * - 通用一条（0,3,0）盖住第 1 类（任何「绝对定位整屏覆盖层」都恢复不透明）；
   * - 应用中心一条（0,4,1 起）盖住第 2 类——同时带上 `.fnos-app-center-route-active` 与
   *   `.absolute.inset-0` 两个变体，前者保证「激活态」赢、后者连「离场动画态」也一并压过。
   * 普通窗口/内容区的玻璃效果不受影响（那两处选择器都不命中它们）。
   */
  var SHELL_CSS_ID = 'fnos-shell-overrides';
  var SHELL_CSS =
    '.bg-\\[var\\(--semi-color-app-container\\)\\].absolute.inset-0' +
    '{background-color:var(--semi-color-app-container) !important;}' +
    '.trim-ui__app-layout--window:has(.trim-ui__app-layout--header-title img[alt="应用中心"]) ' +
    '.fnos-app-center-route-detail.fnos-app-center-route-active,' +
    '.trim-ui__app-layout--window:has(.trim-ui__app-layout--header-title img[alt="应用中心"]) ' +
    '.fnos-app-center-route-detail.absolute.inset-0' +
    '{background-color:var(--semi-color-app-container) !important;' +
    // 上游给这个面板挂了 `transition: …, background-color 0s linear 320ms`（背景延迟 320ms 才切换）。
    // 背景已经是底色的前提下这条延迟只会让「列表→详情」的过渡期露出一瞬透明，所以把
    // `background-color` 从过渡属性里去掉（保留 transform/opacity 的滑动与淡入）。
    'transition-property:transform,opacity !important;}' +
    // 桌面壁纸（fnOS 的 live 壁纸 webp，object-fit: cover）**顶行本身就是一条亮蓝线**（实测
    // rgb(38,77,143)，只占 1px）。裁掉壁纸顶部 1px，那条线就没有了；下方由页面底色承接，
    // 视觉无缝。
    '#root .absolute.inset-0.z-0.object-contain' +
    '{clip-path:inset(1px 0 0 0) !important;}' +
    // Dock 宽度预留（T14c 修复轮 14）：fnOS 用 Tailwind 自定义值类 `pl-[66px]` 给内容区
    // 留出 Dock 的宽度。此前只在 JS 里事后中和——fnOS 一旦（悬浮唤出 Dock、经典启动台
    // 重渲染等时机）重新生成这个容器，就会有一帧带着预留，表现为「左侧空白闪一下」。
    // 样式层没有这个中间帧：元素一出现就是 0。
    // 只钉死这个**具体值**的类（66px 就是本机 Dock 宽，见实测），其余取值仍由
    // `inject/dock.js` 的 `reclaimLayoutPadding` 兜（它按「满尺寸容器 + 预留值落在
    // 24–200px」判定，覆盖别的 fnOS 版本）。
    '.pl-\\[66px\\]' +
    '{padding-left:0 !important;}' +
    // 同一条预留的**与取值无关**的兜底：内容区容器是 `.desktop` 的**直接子元素**
    // （实测 `#root > div > .desktop > div.relative.box-border.h-full.pl-[66px]`）。
    // 万一别的 fnOS 版本/别的启动台样式用的是另一个数值（`pl-[72px]`…），这条一样命中；
    // 作用域只到 `.desktop` 的直接子元素，**不会**波及应用窗口内部那些有意的
    // Tailwind 内边距（它们在更深的层级）。
    '#root .desktop > [class*="pl-["]' +
    '{padding-left:0 !important;}';
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
