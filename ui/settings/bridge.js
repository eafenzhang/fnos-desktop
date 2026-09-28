// 设置窗 → Rust IPC 桥（spec §8.3）。
//
// `withGlobalTauri` **未启用**（`tauri.conf.json` 没有该键），所以设置窗里没有
// `window.__TAURI__`；但 tauri 2 始终注入内部对象 `window.__TAURI_INTERNALS__`
// （`tauri-2.12.0/scripts/core.js:81` 用 `Object.defineProperty` 定义 `invoke`，
// 与 `withGlobalTauri` 无关）。两条路径都试，取先可用的那条。
//
// 命令授权不在这里：`capabilities/default.json` 只把 5 个 `allow-*` 授予
// label 为 `settings` 的窗口，其它窗口（含远程页面）调同一命令会被 ACL 拒绝。

/** 当前可用的 invoke 实现；都不可用时抛错（由调用方 catch 并显示到界面上）。 */
function resolveInvoke() {
  const global = typeof window !== 'undefined' ? window.__TAURI__ : undefined;
  if (global && global.core && typeof global.core.invoke === 'function') {
    return global.core.invoke;
  }
  const internals = typeof window !== 'undefined' ? window.__TAURI_INTERNALS__ : undefined;
  if (internals && typeof internals.invoke === 'function') {
    return (cmd, args) => internals.invoke(cmd, args);
  }
  return null;
}

/**
 * 直通 invoke。刻意**不**在模块加载时抛错：那会让整个 `app.js` 挂掉、设置窗变白屏；
 * 改为返回 rejected Promise，由 `app.js` 的 boot/commit 捕获后显示可读的错误。
 */
export function invoke(cmd, args) {
  const fn = resolveInvoke();
  if (!fn) {
    return Promise.reject(new Error(
      `IPC 桥不可用：既没有 window.__TAURI__.core.invoke，也没有 window.__TAURI_INTERNALS__.invoke（命令 ${cmd}）`
    ));
  }
  return Promise.resolve(fn(cmd, args));
}

export function getConfig() { return invoke('get_config'); }
/** patch 形如 `{ mods: { brandColor } }` / `{ shell: { injectEnabled } }`。 */
export function setConfig(patch) { return invoke('set_config', { patch }); }
/** `needsReload` 为真时调用：Rust 侧销毁并按新载荷**重建**主窗口（§6.6 勘误）。 */
export function reloadMain(url) { return invoke('reload_main', { url: url ?? null }); }
export function openConfigDir() { return invoke('open_config_dir'); }
export function resetConfig(scope) { return invoke('reset_config', { scope }); }
