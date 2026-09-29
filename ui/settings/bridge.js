// 设置窗 → Rust IPC 桥（spec §8.3）。
//
// `withGlobalTauri` **未启用**（`tauri.conf.json` 没有该键），所以设置窗里没有
// `window.__TAURI__`；但 tauri 2 始终注入内部对象 `window.__TAURI_INTERNALS__`
// （`tauri-2.12.0/scripts/core.js:81` 用 `Object.defineProperty` 定义 `invoke`，
// 与 `withGlobalTauri` 无关）。两条路径都试，取先可用的那条。
//
// 命令授权不在这里：`capabilities/default.json` 只把 8 个 `allow-*` 授予
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
/** 用系统默认浏览器打开外部链接（关于页的上游仓库）。Rust 侧只放行 http(s)。 */
export function openUrl(url) { return invoke('open_url', { url }); }
/**
 * 主窗口的观测状态（Task 11）：`get_page_state` 是**只读**命令，`capabilities/default.json`
 * 只把它授予 label 为 `settings` 的窗口——主窗口（含内置错误页）拿不到任何命令授权，
 * 所以页面永远无法自己声称「已注入」或改写这个判定。
 */
export function getPageState() { return invoke('get_page_state'); }
/**
 * 页面上报（Task 13a 的状态证据 + Task 13b 的应用项列表）：`get_page_report` 同样是**只读**、
 * 同样只授设置窗。
 *
 * 上报本身**不走 IPC**：shim 写 `document.title`（`FNOSREPORT:` 单条 / `FNOSCHUNK:` 分片），
 * Rust 的 `on_document_title_changed` 校验后存内存。所以本命令是「读取口」，不是「上报口」——
 * 页面上没有任何命令可调，capability 集合里也没有任何 `remote` 块。
 *
 * 返回体是**信封**（Task 13b 起）：
 *
 * ```json
 * { "report": <最近一条状态相关上报 | null>, "appItems": <最近一条应用项列表上报 | null> }
 * ```
 *
 * 两个槽位各自独立地受文档身份门约束（origin + 文档 URL 都对得上才返回，否则清掉并给
 * `null`）——「还没上报 / 被拒 / 换页面后作废」都表现为对应的 `null`。拆两个槽位的理由
 * （避免 T13a 的状态条强态被 T13b 主动拉取的列表上报冲掉）见
 * `src-tauri/src/report.rs::is_app_items_report`；设置窗侧的解读见
 * `ui/settings/app.js::reportSlots`。
 */
export function getPageReport() { return invoke('get_page_report'); }

/**
 * 导入登录壁纸（Task 13b）：设置窗自己写不了文件，由宿主校验并写进配置目录。
 *
 * 参数形状：tauri 的命令参数默认按 **camelCase** 从 JS 取值
 * （`tauri-macros-2.7.0/src/command/wrapper.rs` 的 `ArgumentCase::Camel`，是默认值），
 * 所以 Rust 侧的 `data_base64` 在这里必须写成 `dataBase64`。
 *
 * 返回的是宿主**落盘用的文件名**（净化 + 内容指纹），调用方再走 `set_config` 写
 * `local.loginWallpaperFileName`——导入与配置是两步，但都由设置窗发起（见 app.js）。
 */
export function importWallpaper(name, dataBase64) {
  return invoke('import_wallpaper', { name, dataBase64 });
}
