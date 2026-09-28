/* fnOS Desktop Shell — chrome.* 兼容层
 * 上游 content-script.js 只依赖下面这些成员；缺 chrome.runtime.id 会导致它完全不注入。 */
(function () {
  var W = typeof window !== 'undefined' ? window : globalThis;
  var SHELL = W.__FNOS_SHELL__ || { mods: {}, local: {}, assets: {} };
  var syncStore = Object.assign({}, SHELL.mods || {});
  var localStore = Object.assign({}, SHELL.local || {});
  var assets = SHELL.assets || {};
  var binaryAssets = SHELL.binaryAssets || {};
  var assetIndex = Object.create(null);
  (function buildAssetIndex() {
    for (var k in assets) {
      assetIndex[String(k).toLowerCase()] = { text: assets[k] };
    }
    for (var kb in binaryAssets) {
      assetIndex[String(kb).toLowerCase()] = { b64: binaryAssets[kb] };
    }
  })();
  var changeListeners = [];
  var messageListeners = [];
  var dataUrlCache = Object.create(null);

  /* ---------- 页面 → 宿主上报通道（Task 13a） ----------
   *
   * 只有一条既有通路可用：`document.title`。宿主（Rust）在 `on_document_title_changed`
   * 里认 `FNOSREPORT:` 前缀（src-tauri/src/report.rs），与 Task 11 的自检探针 `FNOSPROBE:`
   * 走同一条路。**不经 IPC、不需要任何 capability**：远程页面依旧一个命令都调不动
   * （capabilities/ 下没有任何 remote 块，两轮 ACL 探针见 docs/acceptance/M1-M2-验收记录.md）。
   *
   * 上报体 = 一层薄信封 + 上游**原文**（payload 逐字来自上游，本层不改一个字段）：
   *   { type: <上游协议里的 type>, dir: 'out' | 'response', payload: <上游对象> }
   * `dir:'out'` 是上游自己 sendMessage 出去的消息（content-script.js:2681 的
   * FNOS_INJECTION_TRIGGERED 是其中唯一一条，也是「注入链真的跑到最后一步」最硬的信号）；
   * `dir:'response'` 是上游 sendResponse 的应答原文（content-script.js:2865 的 {items,titles}）。
   *
   * 宿主会拒：>32KiB、非 JSON、非对象、type 不在允许表内。本层只做两件事：
   *   1) 只在上游协议内真实存在的 type 上发送（免得把页面自己的杂项消息写进标题）；
   *   2) **任何异常都不许冒泡到页面**（上游 notifyInjectionTriggered 自己就包了 try/catch，
   *      本层再包一层：写标题失败只当没发生）。
   *
   * 宿主存在性：载荷（window.__FNOS_SHELL__）只有本壳的 injector 会写，且必带
   * meta.shellVersion（injector.rs 的 Meta）。页面自己伪造它没有意义——它本来就写得动
   * document.title——这只是「没有宿主就别白改一次标题」。
   */
  var REPORT_TYPES = {
    FNOS_INJECTION_TRIGGERED: 1,
    FNOS_APPLY: 1,
    FNOS_GET_LAUNCHPAD_APP_ITEMS: 1,
    FNOS_GET_LAUNCHPAD_APP_TITLES: 1,
    FNOS_CHECK: 1
  };
  var REPORT_TITLE_PREFIX = 'FNOSREPORT:';
  // 与宿主 32KiB 上限同量级（这里按字符数早退，宿主按字节终审）。
  // 注意：WebView2 把送达宿主的 document.title 截到 **4096 字节**（Task 13a 的长度阶梯实测），
  // 所以超过 ~4085 字节的上报实际上到不了宿主——那个上限是纵深防御，不是可用额度。
  var REPORT_MAX_CHARS = 32768;
  var HOST = !!(SHELL && SHELL.meta && typeof SHELL.meta.shellVersion === 'string');
  var reportBase = null;   // 上报前页面自己的标题（上报后要还回去）
  var reportTimer = null;

  function sendReport(type, dir, payload) {
    if (!HOST || !REPORT_TYPES[type]) return;
    var body;
    try { body = JSON.stringify({ type: type, dir: dir, payload: payload }); } catch (e) { return; }
    if (typeof body !== 'string' || body.length === 0 || body.length > REPORT_MAX_CHARS) return;
    try {
      var d = W.document;
      if (!d) return;
      if (reportBase === null) reportBase = String(d.title || '');
      d.title = REPORT_TITLE_PREFIX + body;
      if (reportTimer === null) {
        // 把标题还给页面：控制标题不是页面的标题（宿主也不镜像它，见 commands.rs::handle_title）。
        // 定时器只为「还回去」而存在，一次上报只挂一个。
        reportTimer = setTimeout(function () {
          reportTimer = null;
          try {
            if (String(d.title).indexOf(REPORT_TITLE_PREFIX) === 0) {
              d.title = reportBase === null ? '' : reportBase;
            }
          } catch (e) { /* ignore */ }
          reportBase = null;
        }, 400);
      }
    } catch (e) { /* 宿主不在 / document 不可用：静默，绝不打扰页面 */ }
  }

  function toDataUrl(mime, text) {
    var key = mime + '\u0000' + text;
    if (dataUrlCache[key]) return dataUrlCache[key];
    var bytes = new TextEncoder().encode(text);
    var bin = '';
    for (var i = 0; i < bytes.length; i += 0x8000) {
      bin += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    }
    var url = 'data:' + mime + ';base64,' + btoa(bin);
    dataUrlCache[key] = url;
    return url;
  }

  function mimeFor(path) {
    if (/\.css$/i.test(path)) return 'text/css';
    if (/\.js$/i.test(path)) return 'text/javascript';
    if (/\.png$/i.test(path)) return 'image/png';
    if (/\.json$/i.test(path)) return 'application/json';
    return 'text/plain';
  }

  function pick(store, keys) {
    var out = {};
    if (keys === null || keys === undefined) return Object.assign({}, store);
    if (typeof keys === 'string') {
      if (Object.prototype.hasOwnProperty.call(store, keys)) out[keys] = store[keys];
      return out;
    }
    if (Array.isArray(keys)) {
      for (var i = 0; i < keys.length; i++) {
        if (Object.prototype.hasOwnProperty.call(store, keys[i])) out[keys[i]] = store[keys[i]];
      }
      return out;
    }
    // 对象形式：值作为默认值
    var defaults = keys;
    for (var k in defaults) {
      out[k] = Object.prototype.hasOwnProperty.call(store, k) ? store[k] : defaults[k];
    }
    return out;
  }

  function makeArea(store, areaName) {
    return {
      get: function (keys, cb) {
        var result = pick(store, keys);
        if (typeof cb === 'function') {
          queueMicrotask(function () { cb(result); });
          return undefined;
        }
        return Promise.resolve(result);
      },
      set: function (items, cb) {
        var changes = {};
        for (var k in items) {
          changes[k] = { oldValue: store[k], newValue: items[k] };
          store[k] = items[k];
        }
        dispatch(areaName, changes);
        if (typeof cb === 'function') queueMicrotask(cb);
        return Promise.resolve();
      },
      remove: function (keys, cb) {
        var list = Array.isArray(keys) ? keys : [keys];
        var changes = {};
        for (var i = 0; i < list.length; i++) {
          changes[list[i]] = { oldValue: store[list[i]], newValue: undefined };
          delete store[list[i]];
        }
        dispatch(areaName, changes);
        if (typeof cb === 'function') queueMicrotask(cb);
        return Promise.resolve();
      }
    };
  }

  function dispatch(area, changes) {
    if (!Object.keys(changes).length) return;
    for (var i = 0; i < changeListeners.length; i++) {
      try { changeListeners[i](changes, area); } catch (e) { /* 上游单个分支异常不影响其余 */ }
    }
  }

  // 宿主（Rust）通过 webview.eval 调用：__FNOS_APPLY_CONFIG__({mods:{...}, local:{...}})
  W.__FNOS_APPLY_CONFIG__ = function (patch) {
    patch = patch || {};
    var pairs = [['sync', syncStore, patch.mods], ['local', localStore, patch.local]];
    for (var p = 0; p < pairs.length; p++) {
      var area = pairs[p][0], store = pairs[p][1], next = pairs[p][2];
      if (!next) continue;
      var changes = {};
      for (var k in next) {
        var oldValue = store[k];
        var newValue = next[k];
        if (JSON.stringify(oldValue) === JSON.stringify(newValue)) continue;
        changes[k] = { oldValue: oldValue, newValue: newValue };
        store[k] = newValue;
      }
      dispatch(area, changes);
    }
  };

  W.chrome = W.chrome || {};
  W.chrome.runtime = {
    id: 'fnos-desktop-shell',
    getURL: function (path) {
      if (typeof path !== 'string' || !path) return '';
      var entry = assetIndex[path.toLowerCase()];
      if (!entry) return '';
      if (typeof entry.b64 === 'string') {
        return 'data:' + mimeFor(path) + ';base64,' + entry.b64;
      }
      var text = entry.text;
      if (typeof text !== 'string') return ''; // 上游 cs:2622 的注入闸门依赖空串
      if (path.toLowerCase() === 'mod.js') {
        text = text + '\n;window.__FNOS_MOD_EXECUTED__=true;\n';
      }
      return toDataUrl(mimeFor(path), text);
    },
    getManifest: function () {
      return { version: (SHELL.meta && SHELL.meta.modsVersion) || '0.0.0' };
    },
    sendMessage: function (msg, cb) {
      // Task 13a：先把上游这条消息原文回传给宿主（只在上报协议内、且宿主存在时）。
      var reportType = msg && typeof msg === 'object' ? msg.type : null;
      sendReport(reportType, 'out', msg);
      var handled = false;
      for (var i = 0; i < messageListeners.length; i++) {
        try {
          messageListeners[i](msg, { id: 'fnos-desktop-shell' }, function (resp) {
            // 应答同样只回传**第一次**应答的原文（多个监听者时后到的不是上游的语义结果）。
            if (!handled) sendReport(reportType, 'response', resp);
            handled = true;
            if (typeof cb === 'function') cb(resp);
          });
        } catch (e) { /* ignore */ }
      }
      if (!handled && typeof cb === 'function') queueMicrotask(function () { cb(undefined); });
      return Promise.resolve(undefined);
    },
    onMessage: {
      addListener: function (fn) { messageListeners.push(fn); },
      removeListener: function (fn) {
        var i = messageListeners.indexOf(fn);
        if (i >= 0) messageListeners.splice(i, 1);
      }
    },
    lastError: undefined
  };
  W.chrome.storage = {
    sync: makeArea(syncStore, 'sync'),
    local: makeArea(localStore, 'local'),
    onChanged: {
      addListener: function (fn) { changeListeners.push(fn); },
      removeListener: function (fn) {
        var i = changeListeners.indexOf(fn);
        if (i >= 0) changeListeners.splice(i, 1);
      }
    }
  };
})();
