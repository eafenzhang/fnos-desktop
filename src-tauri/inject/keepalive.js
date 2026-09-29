/* fnOS Desktop Shell — 登录保活心跳（T14c）
 *
 * 背景（实测）：fnOS 的登录态是**会话级 cookie**（`entry-token`，域 `.ea121314.fnos.net`），
 * 服务端按访问滑动续期；桌面顶层文档**自己完全不轮询**（实测 75 秒零请求），所以闲置久了
 * 登录会失效，用户得重新登录。本文件补一个轻量心跳：登录后在**同源**上周期性请求一次
 * SPA 启动时自己就会调用的令牌接口（`/app/token`，实测 200 / ~28ms / 空体），让服务端
 * 持续看到这个会话。
 *
 * 安全边界（本文件最要紧的部分）：
 * - **不存在任何用户可控 URL**：路径是写死的常量 `/app/token`，用 `new URL(path, location.href)`
 *   拼出来后**再校验**协议必须是 http/https、且 origin 必须与当前页面**同源**；任一不满足就
 *   不发请求。因此它既满足「只请求 http/https，且发请求前校验 host」，也没有引入 SSRF 面
 *   （没有可配置地址，也没有从页面文本里取 URL 的路径）。
 * - 只带 `credentials: 'same-origin'`（cookie 只回给同源）、`cache: 'no-store'`（不吃缓存，
 *   每次都真的打到服务端，这才是「保活」的意义）。
 * - 登录页（有密码输入框）不发：那会打扰登录流程，且那时本来就没有会话要保。
 *
 * 开关与间隔：注入载荷 `__FNOS_SHELL__.shell.keepAliveMinutes`（`config.rs::ShellConfig`，
 * 默认 10 分钟，**0 或负数 = 关闭**）；免刷新改动经 `__FNOS_APPLY_SHELL__` 生效。
 * 该钩子名与 `dock.js` 共用：本文件**链式保留**已安装的钩子（见文件末尾），互不覆盖。
 *
 * 诊断：控制台 `__FNOS_KEEPALIVE_STATE__()` 读状态（页面上不画任何东西），
 * `__FNOS_KEEPALIVE_TICK__()` 立即打一拍（排障用）。
 */
(function () {
  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document;

  // 只在顶层文档工作（与 dock.js 同一条纪律；子框架里没有会话要保）
  var isTopFrame = false;
  try { isTopFrame = W.top === W.self; } catch (e) { isTopFrame = false; }
  if (!isTopFrame) return;

  var SHELL = W.__FNOS_SHELL__ || {};
  var SHELL_CFG = SHELL.shell || {};

  var DEFAULT_MINUTES = 10;      // 默认间隔（分钟）；与 config.rs 的默认值一致
  var MAX_MINUTES = 1440;        // 上限（一天）
  var TOKEN_PATH = '/app/token'; // 写死的同源路径（SPA 启动时自己也会调它）
  var FIRST_TICK_MS = 20000;     // 装好后第一拍的延迟：等登录/桌面就位（不抢启动）

  var minutes = 0;               // 当前生效间隔（分钟）；0 = 关闭
  var timer = null;
  var firstTimer = null;
  var installed = false;

  /** 状态（控制台诊断用；不向页面画任何东西）。 */
  var state = { enabled: false, minutes: 0, ticks: 0, lastAt: null, lastStatus: null, lastError: null };

  function sanitizeMinutes(raw) {
    var num = typeof raw === 'number' && isFinite(raw) ? raw : NaN;
    if (!isFinite(num)) return DEFAULT_MINUTES; // 缺省/非法类型 → 默认
    if (!(num > 0)) return 0;                   // 0 / 负数 = 关闭
    var n = Math.floor(num);
    if (n < 1) n = 1;                           // 正的小数（0.5 之类）→ 至少 1 分钟，而不是静默关掉
    return n > MAX_MINUTES ? MAX_MINUTES : n;
  }

  /** 当前是不是登录页（有密码输入框）——那里没有会话要保，也不该打扰登录。 */
  function onLoginPage() {
    try { return !!D.querySelector('input[type="password"]'); } catch (e) { return false; }
  }

  /**
   * 拼出保活 URL 并做**同源 + 协议**校验；不合规返回 null（宁可不发）。
   * 路径是常量，`location.href` 是本页面自身地址 —— 这里没有任何外部输入。
   */
  function tokenUrl() {
    var href = '';
    try { href = String(W.location && W.location.href || ''); } catch (e) { return null; }
    if (!/^https?:\/\//i.test(href)) return null;
    var url = null;
    try { url = new W.URL(TOKEN_PATH, href); } catch (e) { return null; }
    var origin = null;
    try { origin = String(W.location.origin || ''); } catch (e) { return null; }
    if (!origin) return null;
    if (url.origin !== origin) return null;           // 必须同源（跨源一律不发）
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return null;
    return url.href;
  }

  /** 打一拍：同源 GET 令牌接口。任何异常都吞掉（保活失败不该影响页面）。 */
  function tick() {
    if (!state.enabled) return Promise.resolve(false);
    if (onLoginPage()) return Promise.resolve(false); // 登录页不发
    var url = tokenUrl();
    if (!url) return Promise.resolve(false);
    var fetchFn = typeof W.fetch === 'function' ? W.fetch : null;
    if (!fetchFn) return Promise.resolve(false);
    return fetchFn.call(W, url, {
      method: 'GET',
      credentials: 'same-origin',
      cache: 'no-store',
      redirect: 'follow',
    }).then(function (res) {
      state.ticks += 1;
      state.lastAt = Date.now();
      state.lastStatus = res ? res.status : null;
      state.lastError = null;
      return true;
    }).catch(function (e) {
      state.ticks += 1;
      state.lastAt = Date.now();
      state.lastStatus = null;
      state.lastError = String((e && e.message) || e);
      return false;
    });
  }

  function clearTimers() {
    if (timer !== null) { W.clearInterval(timer); timer = null; }
    if (firstTimer !== null) { W.clearTimeout(firstTimer); firstTimer = null; }
  }

  function install() {
    clearTimers();
    state.enabled = minutes > 0;
    state.minutes = minutes;
    if (!state.enabled) return;
    var periodMs = Math.max(60000, minutes * 60000); // 下限 1 分钟，防手改配置写成极小值
    // 第一拍稍晚一点：等桌面/登录完成（不抢启动期），之后按周期走
    firstTimer = W.setTimeout(function () { firstTimer = null; tick(); }, FIRST_TICK_MS);
    timer = W.setInterval(tick, periodMs);
    installed = true;
  }

  function applyShell(patch) {
    if (!patch || typeof patch !== 'object') return;
    if (!Object.prototype.hasOwnProperty.call(patch, 'keepAliveMinutes')) return;
    var next = sanitizeMinutes(patch.keepAliveMinutes);
    if (next === minutes && installed) return; // 幂等
    minutes = next;
    install();
  }

  // 初始态来自注入载荷
  minutes = sanitizeMinutes(SHELL_CFG.keepAliveMinutes === undefined
    ? DEFAULT_MINUTES
    : SHELL_CFG.keepAliveMinutes);
  install();

  /**
   * 免刷新入口：**链式**保留已安装的钩子（`dock.js` 也用这个名字），
   * 两个功能各拿各的键、互不覆盖。
   */
  var prevApply = W.__FNOS_APPLY_SHELL__;
  W.__FNOS_APPLY_SHELL__ = function (patch) {
    if (typeof prevApply === 'function') {
      try { prevApply(patch); } catch (e) { /* 别的钩子出错不拖累本功能 */ }
    }
    applyShell(patch);
  };

  /** 诊断口（控制台可见，页面上不画任何东西）。 */
  W.__FNOS_KEEPALIVE_STATE__ = function () {
    return {
      enabled: state.enabled, minutes: state.minutes, ticks: state.ticks,
      lastAt: state.lastAt, lastStatus: state.lastStatus, lastError: state.lastError,
      onLoginPage: onLoginPage(),
    };
  };
  /** 立即打一拍（排障/测试用；正常路径不需要它）。 */
  W.__FNOS_KEEPALIVE_TICK__ = function () { return tick(); };
})();
