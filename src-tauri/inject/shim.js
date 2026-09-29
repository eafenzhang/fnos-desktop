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

  /** 上游 `content-script.js:55` 读的本地存储键：壁纸文件名（配置值，宿主原样下发）。 */
  var LOGIN_WALLPAPER_NAME_KEY = 'loginWallpaperFileName';
  /** 上游 `content-script.js:54` 读的本地存储键：壁纸**数据**（本壳由 binaryAssets 解析出来）。 */
  var LOGIN_WALLPAPER_DATA_KEY = 'loginWallpaperDataUrl';

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
  // T13b：单条上报的**实测**额度（**字节**，含前缀）。4096 是通道上限，留出余量取 4000；
  // 中文按 UTF-8 每字 3 字节，所以判定必须按字节而不是字符数。
  var REPORT_SINGLE_MAX_BYTES = 4000;
  // 分片：前缀与两个上限必须与宿主 `report.rs` 的常量一致（`commands.rs` 的
  // `chunk_prefix_and_shim_agree` 逐字锁着它们）。
  var REPORT_CHUNK_PREFIX = 'FNOSCHUNK:';
  var REPORT_CHUNK_BODY_MAX_BYTES = 3000;
  var REPORT_CHUNK_MAX = 8;
  var HOST = !!(SHELL && SHELL.meta && typeof SHELL.meta.shellVersion === 'string');
  var reportBase = null;   // 上报前页面自己的标题（上报后要还回去）
  var reportTimer = null;

  function titleTarget() {
    try { return W.document || null; } catch (e) { return null; }
  }

  /**
   * `setTimeout`，但在 Node 下让定时器**不阻止进程退出**（`unref`）。
   *
   * 为什么需要：本层是纯 JS，整个 `tests/shim.test.mjs` 在 Node 里加载它。分片发送与
   * 「应用项列表」的有限重试都会挂定时器，若这些定时器压着事件循环，测试进程会白等几十秒
   * （甚至在 `node --test` 下看起来像挂住）。浏览器里 `setTimeout` 返回的是数字，`unref`
   * 不存在——所以这段只在 Node 下生效，页面行为一个字节都没变。
   */
  function later(fn, ms) {
    var id = setTimeout(fn, ms);
    try {
      if (id && typeof id.unref === 'function') id.unref();
    } catch (e) { /* 浏览器：id 是数字，没有 unref */ }
    return id;
  }

  /** 写一次控制标题；记下页面原本的标题（只在第一次上报时记）。 */
  function writeTitle(text) {
    var d = titleTarget();
    if (!d) return false;
    if (reportBase === null) reportBase = String(d.title || '');
    try { d.title = text; return true; } catch (e) { return false; }
  }

  /**
   * 把标题还给页面：控制标题不是页面的标题（宿主也不镜像它，见 `commands.rs::handle_title`）。
   *
   * 定时器只为「还回去」而存在，一次上报（单条或一整串分片）只挂一个。400ms 是 Task 13a
   * 实测够用的窗口；这里额外要求「当前标题仍以某个控制前缀开头」才还原——页面自己又改了标题
   * 就不动它。
   */
  function armTitleRestore() {
    if (reportTimer !== null) return;
    reportTimer = later(function () {
      reportTimer = null;
      var d = titleTarget();
      try {
        var t = d ? String(d.title) : '';
        if (t.indexOf(REPORT_TITLE_PREFIX) === 0 || t.indexOf(REPORT_CHUNK_PREFIX) === 0) {
          d.title = reportBase === null ? '' : reportBase;
        }
      } catch (e) { /* ignore */ }
      reportBase = null;
    }, 400);
  }

  /** UTF-8 字节长度（没有 TextEncoder 时按 UTF-8 规则手算：只影响切分粒度，不影响安全）。 */
  function utf8Length(s) {
    if (typeof TextEncoder === 'function') {
      try { return new TextEncoder().encode(s).length; } catch (e) { /* 走手算 */ }
    }
    var n = 0;
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      if (c < 0x80) n += 1;
      else if (c < 0x800) n += 2;
      else if (c >= 0xd800 && c <= 0xdbff) { n += 4; i += 1; }
      else n += 3;
    }
    return n;
  }

  /**
   * 按**字节**把正文切成若干片，切点永远落在码点边界上（不能把中文/emoji 切成半个）。
   *
   * 返回 `null` 表示「需要的片数超过 `maxChunks`」——那种情况下宁可不发：宿主侧有同样的
   * 硬上限，多发只会被拒。
   */
  function utf8Chunks(s, maxBytes, maxChunks) {
    var out = [];
    var cur = '';
    var curBytes = 0;
    for (var i = 0; i < s.length;) {
      var cp = s.codePointAt(i);
      var unit = cp > 0xffff ? 2 : 1;
      var n = cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
      if (curBytes + n > maxBytes && curBytes > 0) {
        out.push(cur);
        cur = '';
        curBytes = 0;
      }
      cur += unit === 2 ? s.substr(i, 2) : s.charAt(i);
      curBytes += n;
      i += unit;
    }
    if (curBytes > 0) out.push(cur);
    if (!out.length || out.length > maxChunks) return null;
    return out;
  }

  /**
   * 分片发送：每片一个独立 task。
   *
   * 为什么不能同步连写：同一帧内两次标题改动会被 WebView2 合并成最后一次（Task 11 的探针
   * 注释记录过同一现象）——那样中间的分片会永远到不了宿主。`setTimeout(…, 0)` 让每片落在
   * 各自的 task 里（宿主侧还有 5 秒窗口兜住「页面卡住」的情形）。
   */
  function sendChunks(chunks) {
    var i = 0;
    function step() {
      if (i >= chunks.length) { armTitleRestore(); return; }
      if (!writeTitle(REPORT_CHUNK_PREFIX + i + ',' + chunks.length + ',' + chunks[i])) return;
      i += 1;
      later(step, 0);
    }
    step();
  }

  /**
   * 这条应答值不值得上报。
   *
   * 只有一处收窄（Task 13b）：**空的应用项列表不回传**。启动台还没渲染时上游只能给出空列表
   * （见 `requestAppItems` 的注释），把它写进上报会覆盖掉之前那份真实列表 —— 设置窗的逐项 UI
   * 就会从「有 12 个应用」变成「0 个应用」。状态条依赖的 `FNOS_INJECTION_TRIGGERED`
   * 完全不经过这里（它是 `dir:'out'` 的上游消息，不是应答）。
   */
  function reportableResponse(type, resp) {
    if (type !== 'FNOS_GET_LAUNCHPAD_APP_ITEMS' && type !== 'FNOS_GET_LAUNCHPAD_APP_TITLES') return true;
    var ok = !!(resp && typeof resp === 'object' && Array.isArray(resp.items) && resp.items.length > 0);
    // 拿到非空列表就不再重试（见 `requestAppItems`）：一次成功之后仍每 6 秒问上游一遍，
    // 只会让页面白做 DOM 扫描。
    if (ok) appItemsSeen = true;
    return ok;
  }

  /**
   * 上报给宿主的应用项列表副本：**只保留 `key` / `title`，剥掉 `iconSrc`**。
   *
   * ## 为什么必须剥（Task 13b 运行期实测的缺陷）
   *
   * 上游 `collectLaunchpadAppItems()`（content-script.js:595-610）把每一项的 `iconSrc` 填成
   * `<img>` 的 **`currentSrc`**。而本壳的「完美图标」重绘做的就是把这个 `currentSrc` 换成
   * `binaryAssets` 里那张内置 PNG 的 **data URL**（`applyLaunchpadRedrawIcon` → `img.src =
   * safeRuntimeGetURL(...)`，实测单张 91,142 字符）。于是**只要用户开了完美图标并配了重绘，
   * 上游的应答本身就带上了内联图片**：一个应用 ≈ 91 KB，60 个应用 ≈ 5.4 MB。
   *
   * 那条应答会在 `sendReport` 的第一道闸门（`REPORT_MAX_CHARS` 32 Ki）被**静默丢掉**——
   * 设置窗于是永远收不到应用项列表，逐项 UI 恒显示「尚未收到」，功能等于不存在。
   * 实测证据：`_t13b-runtime.log` 的 `send:early-len:91345`。
   *
   * 剥掉之后的量级：`key`(~55 字符) + `title`(~10-20 字符) ≈ 90 字节/项，200 个应用 ≈ 18 KB，
   * 落在 8 片 × 3000 字节（24 KB）的预算内——这正是分片通道存在的意义。
   *
   * ## 为什么删字段而不是截断 data URL
   *
   * 设置窗的逐项 UI **只需要 `key`（写进 `launchpadIconRedrawMap` 等四个键）与 `title`
   * （显示）**：`ui/settings/app.js::appItemsFromReport` 只认这两个。截断成
   * `data:image/png;base64,iVBORw0…` 仍然是几十 KB 的死重量，还会让人误以为宿主拿到了可用预览。
   *
   * ## 谁不受影响
   *
   * 只改**上报的那一份副本**：`sendMessage` 的 `cb(resp)` 拿到的仍是上游原文（含 `iconSrc`），
   * 上游 popup 的预览语义一个字节都没变。
   */
  function appItemsForReport(resp) {
    var src = resp && Array.isArray(resp.items) ? resp.items : [];
    var items = [];
    for (var i = 0; i < src.length; i += 1) {
      var it = src[i];
      if (!it || typeof it !== 'object') continue;
      items.push({
        key: typeof it.key === 'string' ? it.key : '',
        title: typeof it.title === 'string' ? it.title : ''
      });
    }
    var titles = [];
    for (var j = 0; j < items.length; j += 1) titles.push(items[j].title);
    return { items: items, titles: titles };
  }

  /** 上报给宿主的应答正文（目前只有应用项列表需要**瘦身**，见 [`appItemsForReport`]）。 */
  function responseForReport(type, resp) {
    if (type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS' || type === 'FNOS_GET_LAUNCHPAD_APP_TITLES') {
      return appItemsForReport(resp);
    }
    return resp;
  }

  function sendReport(type, dir, payload) {
    if (!HOST || !REPORT_TYPES[type]) return;
    var body;
    try { body = JSON.stringify({ type: type, dir: dir, payload: payload }); } catch (e) { return; }
    if (typeof body !== 'string' || body.length === 0 || body.length > REPORT_MAX_CHARS) return;
    // 小载荷走**既有**的单条路径（语义逐字未变）：状态条依赖的 FNOS_INJECTION_TRIGGERED
    // 就在这里。1000 字符以内即使全是汉字也只有 3000 字节 < 4000，无需测量。
    if (body.length <= 1000 || utf8Length(body) <= REPORT_SINGLE_MAX_BYTES) {
      if (!writeTitle(REPORT_TITLE_PREFIX + body)) return;
      armTitleRestore();
      return;
    }
    var chunks = utf8Chunks(body, REPORT_CHUNK_BODY_MAX_BYTES, REPORT_CHUNK_MAX);
    if (!chunks) return;
    sendChunks(chunks);
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
    // T13b：登录壁纸的 mime 由扩展名推导（`config::wallpaper_mime` 是同一张表）。
    // 少了这三行，裸 base64 形态的壁纸 data URL 会被标成 `text/plain`（图片不显示）。
    if (/\.jpe?g$/i.test(path)) return 'image/jpeg';
    if (/\.webp$/i.test(path)) return 'image/webp';
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
    // 登录壁纸的名字变了 → 重新解析一次 data URL（配置是免刷新推送的，资源键却跟着文件名走）
    if (patch.local && Object.prototype.hasOwnProperty.call(patch.local, LOGIN_WALLPAPER_NAME_KEY)) {
      syncLoginWallpaperDataUrl();
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
        // T13b：宿主（`injector.rs`）对二进制资源下发**完整** data URL（mime 由扩展名推导）。
        // 裸 base64 仍然兼容（本壳更早的形态与单测用它），按扩展名补前缀。
        if (/^data:/i.test(entry.b64)) return entry.b64;
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
            // `responseForReport` 只对**上报的那一份**做瘦身（应用项列表剥掉 `iconSrc`，见那里的
            // 注释）；`cb(resp)` 拿到的仍是上游原文。
            if (!handled && reportableResponse(reportType, resp)) sendReport(reportType, 'response', responseForReport(reportType, resp));
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

  /**
   * 登录壁纸：把配置里的**文件名**解析成上游要的 **data URL**（Task 13b）。
   *
   * 上游读的是 `chrome.storage.local` 的 `loginWallpaperDataUrl`（content-script.js:54），
   * **不是** `getURL` —— 所以只把文件放进 `binaryAssets` 是不够的，必须把它接到 local store 上，
   * 否则 `loadLoginWallpaperFromStorage` 永远拿到空串（用户在设置窗里选了图，页面上却没有壁纸）。
   *
   * 宿主在载荷里给的是：`local.loginWallpaperFileName`（配置值原文）+ `binaryAssets` 里以
   * **小写文件名**为键的完整 data URL。两者对不上（文件被删/太大被跳过/名字非法）时删掉这个键
   * ——上游看到的与「没配壁纸」完全同义（而不是一个坏的 `data:,`）。
   */
  function syncLoginWallpaperDataUrl() {
    try {
      var name = localStore[LOGIN_WALLPAPER_NAME_KEY];
      if (typeof name !== 'string' || !name.replace(/\s+/g, '')) {
        delete localStore[LOGIN_WALLPAPER_DATA_KEY];
        return;
      }
      var url = W.chrome.runtime.getURL(String(name).replace(/^\s+|\s+$/g, ''));
      if (typeof url === 'string' && url) localStore[LOGIN_WALLPAPER_DATA_KEY] = url;
      else delete localStore[LOGIN_WALLPAPER_DATA_KEY];
    } catch (e) {
      delete localStore[LOGIN_WALLPAPER_DATA_KEY];
    }
  }

  /**
   * 完美图标：主动向上游要一次应用项列表（Task 13b）。
   *
   * 上游只在收到 `FNOS_GET_LAUNCHPAD_APP_ITEMS` 时才收集应用项（content-script.js:2853-2870），
   * 而它的收集完全依赖 DOM（`collectLaunchpadIconCards` 扫 `div.cursor-pointer`）——启动台没
   * 渲染出来时只能得到空列表。本壳没有「把请求送进页面」的命令（页面上一个命令都调不动，R70），
   * 所以由本层在配置**真的启用**完美图标时自己发，并按固定间隔重试有限次：用户打开一次启动台
   * 之后，列表就会经既有的上报通道（大载荷自动分片）回到宿主。
   *
   * 只发请求、不改页面：应答由上游自己的监听器给出，本层只转发原文。
   */
  var APP_ITEMS_TRIES = 10;
  var APP_ITEMS_INTERVAL_MS = 6000;
  /** 已经拿到过一份非空的列表（`reportableResponse` 置位）→ 不再重试。 */
  var appItemsSeen = false;

  function perfectIconConfigured() {
    var mods = SHELL.mods || {};
    if (mods.launchpadIconScaleEnabled) return true;
    var map = mods.launchpadIconRedrawMap;
    if (map && typeof map === 'object' && Object.keys(map).length > 0) return true;
    var keys = mods.launchpadIconScaleSelectedKeys;
    return !!(Array.isArray(keys) && keys.length > 0);
  }

  function requestAppItems() {
    if (!HOST || !perfectIconConfigured()) return;
    var tries = 0;
    function ask() {
      if (appItemsSeen) return;
      tries += 1;
      try {
        W.chrome.runtime.sendMessage({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' });
      } catch (e) { return; }
      if (tries < APP_ITEMS_TRIES) later(ask, APP_ITEMS_INTERVAL_MS);
    }
    later(ask, 2500);
  }

  requestAppItems();
  syncLoginWallpaperDataUrl();
})();
