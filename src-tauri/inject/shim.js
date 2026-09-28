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
      var handled = false;
      for (var i = 0; i < messageListeners.length; i++) {
        try {
          messageListeners[i](msg, { id: 'fnos-desktop-shell' }, function (resp) {
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
