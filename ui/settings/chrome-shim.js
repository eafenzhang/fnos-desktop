// chrome.* 兼容层（Task 14b）——让**上游原样的** popup.js 跑在本壳的设置窗里。
//
// 设置窗的界面自 T14b 起就是上游的 `popup.html` + `popup.js`（`ui/settings/popup.html`
// 与 vendored 逐字节相同，只多插入一行 `<script src="./chrome-shim.js"></script>`）：
//
//   settings.html（本壳）┌ 状态条 + 外壳说明 + 关于/合规（本壳自己的区域）
//                        └ <iframe src="popup.html">（上游 UI，本文件就在这个 frame 里）
//
// 本文件是**经典脚本**（不是 module）：它必须在 popup.js 之前同步定义好
// `window.chrome`，因为 popup.js 在脚本开头就同步调用 `chrome.runtime.getManifest()`。
//
// 所有 IPC 都不在这个 frame 里直接发：本 frame 把请求 postMessage 给父 frame
// （`ui/settings/app.js` 的桥），父 frame 用 `window.__TAURI_INTERNALS__.invoke` 调
// tauri 命令并把结果回传。两条理由：
//   1. 父 frame 是设置窗的顶层文档，它的 IPC 通路是 T11/T13a/T13b 已经在真机上验证过的
//      那条；iframe 里 `__TAURI_INTERNALS__` 是否被注入没有任何需要赌的地方；
//   2. 映射逻辑（键 → `{mods|local}` patch、键 → 本地存储、键 → 拒绝）集中一处，
//      父 frame 只暴露一张白名单命令表，iframe 不可能凭自己的意思调任意命令。
//
// 唯一同步的接口是 `runtime.getManifest()`：它读父 frame 在**创建 iframe 之前**就写好的
// 快照对象 `window.__FNOS_SETTINGS_SNAPSHOT__`（同源直接属性访问，同步可见）。
//
// ---------------------------------------------------------------------------
// 从 popup.js 逐行读出来的 `chrome.*` 面与**键的分区**（不许凭印象，行号即 popup.js）
// ---------------------------------------------------------------------------
// `chrome.storage.sync.get({...})`（popup.js:1785-1811）的默认值对象**就是**本壳
// `ModsConfig` 的 25 个字段，一个不多一个不少（顺序也一致），所以 sync 段的全部键
// （读写都是）→ `set_config {mods:{…}}`；`get` 回包按 popup 请求的键逐个从 config.mods 取，
// 缺什么就用 popup 自己给的默认值（本壳不发明默认值）。
//
// `chrome.storage.local`（popup.js:1813-1821 的 get、1389-1429 的 set/remove）一共 8 个键，
// 分四类，**没有一类是「静默丢掉」**：
//   1. `customCssCode` / `customJsCode`（popup.js:82-83）——本壳 `local` 段真有这两个字段
//      （`LocalConfig`），逐项注入也真的用它们，所以走 `set_config {local:{…}}`；
//   2. `loginWallpaperDataUrl` / `loginWallpaperFileName`（popup.js:79-80）——本壳**有**登录
//      壁纸功能，但它要求文件落在配置目录里（`injector::load_wallpaper` 只从那里按
//      `local.loginWallpaperFileName` 读）。所以 data URL 交给宿主命令 `import_wallpaper`
//      校验并落盘，再把落盘名写进 `local.loginWallpaperFileName`；
//   3. `updateCheckState`（popup.js:81）——纯粹的扩展本地状态，本壳配置模型里没有对应字段，
//      它进**新的设置窗本地存储** `local-store.json`（`get_local_store` / `set_local_store`，
//      只授设置窗、只存字符串、有大小与键名上限）；
//   4. `customFontDataUrl` / `customFontFileName` / `customFontFormat`（popup.js:76-78）——
//      **拒绝**（用户决策 D4：不随包字体、也不做字体文件导入）。写这些键会立刻：
//      ① 把一句明确的说明挂到可见的「外壳说明」框里；② 抛错，让上游自己的
//      `safeLocalSet` 走进失败分支（`console.warn` + 状态文案），绝不假装成功。
//      读这些键返回 popup 请求的默认值（空串）——「没有导入过字体」是事实，不是谎。
//      删除这些键按**空操作成功**处理：本壳从来没有存过字体数据，删完之后「没有导入的字体」
//      依然成立，返回成功是如实的（详见 `localRemove` 的注释）。
//
// 其余面：`tabs.query` ← `get_page_state`（主窗口那一页，不是设置窗自己）；
// `tabs.sendMessage` 只有三个 type（popup.js:787 / 1608 / 1774）；`tabs.create` ← `open_url`；
// `action.setBadge*` ← 空操作（本壳没有扩展图标徽标这种东西）；
// `runtime.getURL` ← 由设置窗资产根（`ui/settings/`）回答的**同步**真实 URL。
//
// **联网**：上游的更新检查（popup.js:1153-1179 的 `fetch(GITHUB_COMMITS_API_URL)`）被本层
// 接管：不发任何网络请求，直接返回「最新提交 = 本外壳内置的 vendored commit」这一份合成应答。
// 于是上游自己算出 `hasUpdate=false`（首次是 `first`、之后是 `same`），状态文案是
// 「已记录当前最新提交 / 暂无更新」——这是如实的：本壳确实把上游代码锁在 vendored commit 上，
// 由本仓发版决定升级，而不是让用户去比对 GitHub 的 HEAD。

(() => {
  'use strict';

  /** 父 frame 在桥就绪时置位（iframe 由它创建，所以本标志必然早于本脚本执行）。 */
  const BRIDGE_FLAG = '__FNOS_SETTINGS_BRIDGE__';
  /** 同步快照对象（父 frame 就地更新，本 frame 直接读属性）。 */
  const SNAPSHOT_KEY = '__FNOS_SETTINGS_SNAPSHOT__';
  /** postMessage 协议（请求 / 应答）。 */
  const REQ = 'FNOS_SHIM_REQUEST';
  const REP = 'FNOS_SHIM_REPLY';

  // ---------- 键的分区表（与 popup.js 逐行对应，见文件头） ----------

  /** popup.js:1785-1811 的 `chrome.storage.sync.get` 默认值对象的键，顺序原样。 */
  const SYNC_MODS_KEYS = [
    'enabledOrigins', 'autoEnableSuspectedFnOS', 'basePresetEnabled',
    'windowAnimationBlurEnabled', 'titlebarStyle', 'launchpadStyle',
    'desktopIconLayoutEnabled', 'desktopIconLayoutMode', 'desktopIconPerColumnEnabled',
    'launchpadIconScaleEnabled', 'launchpadIconScaleSelectedKeys',
    'launchpadIconMaskOnlyKeys', 'launchpadIconRedrawKeys', 'launchpadIconRedrawMap',
    'desktopIconPerColumn', 'brandColor', 'fontOverrideEnabled', 'fontFamily',
    'fontMonospaceFamily', 'fontWeight', 'fontFeatureSettings', 'fontFaceName', 'fontUrl',
    'customCodeEnabled', 'lockscreenDefaultUsername',
  ];

  /** popup.js:82-83 → 本壳 `LocalConfig` 的字段（逐项注入真的用它们）。 */
  const LOCAL_CONFIG_KEYS = ['customCssCode', 'customJsCode'];
  /** popup.js:79-80 → 宿主 `import_wallpaper` + `local.loginWallpaperFileName`。 */
  const LOCAL_WALLPAPER_DATA_KEY = 'loginWallpaperDataUrl';
  const LOCAL_WALLPAPER_NAME_KEY = 'loginWallpaperFileName';
  /** popup.js:81 → 设置窗本地存储（`local-store.json`）。 */
  const LOCAL_STORE_KEYS = ['updateCheckState'];
  /** popup.js:76-78 → D4：本壳不提供字体文件导入，写入一律拒绝。 */
  const LOCAL_FONT_KEYS = ['customFontDataUrl', 'customFontFileName', 'customFontFormat'];

  /** 壁纸大小上限（与 `config::MAX_WALLPAPER_BYTES` / 宿主命令同一个数，这里早退一次省一次 IPC）。 */
  const MAX_WALLPAPER_BYTES = 8 * 1024 * 1024;
  /** 壁纸允许的扩展名（与 `config::wallpaper_ext` / popup 的 `<input accept>` 同一张表）。 */
  const WALLPAPER_EXTS = ['png', 'jpg', 'jpeg', 'webp'];

  /** 上游更新检查唯一会请求的地址（popup.js:89-90）。 */
  const GITHUB_COMMITS_API_URL =
    'https://api.github.com/repos/aurysian-yan/FnOS_UI_Mods/commits?per_page=1';
  const GITHUB_COMMITS_PAGE_URL =
    'https://github.com/aurysian-yan/FnOS_UI_Mods/commits';

  /** `chrome.runtime.getURL` 允许的形状：只能指到设置窗资产根下的相对路径。 */
  const ASSET_PATH = /^[A-Za-z0-9][A-Za-z0-9._/-]*$/;

  // ---------- 与父 frame 的桥 ----------

  let nextRequestId = 1;
  const pendingRequests = new Map();

  /** 每个命令的超时（毫秒）。壁纸导入要过 base64 解码 + 落盘，给得宽一点。 */
  const COMMAND_TIMEOUT_MS = {
    set_config: 20000,
    import_wallpaper: 60000,
    app_items: 15000,
    page_check: 10000,
    apply: 20000,
  };
  const DEFAULT_TIMEOUT_MS = 8000;

  function parentWindow() {
    try {
      return window.parent && window.parent !== window ? window.parent : null;
    } catch (_error) {
      return null;
    }
  }

  /** 父 frame 的桥是否就绪（同步、无副作用：只是读一个同源属性）。 */
  function bridgeReady() {
    const parent = parentWindow();
    if (!parent) return false;
    try {
      return parent[BRIDGE_FLAG] === true;
    } catch (_error) {
      return false;
    }
  }

  /** 同步快照（父 frame 在创建 iframe 之前写入；读不到时返回空对象）。 */
  function snapshot() {
    const parent = parentWindow();
    if (!parent) return {};
    try {
      const snap = parent[SNAPSHOT_KEY];
      return snap && typeof snap === 'object' ? snap : {};
    } catch (_error) {
      return {};
    }
  }

  /**
   * 调用宿主的**白名单**命令。父 frame 只认这张表里的命令名，iframe 拿不到任意命令。
   * 失败一律 reject 一个 `Error`（上游的 `safeSyncSet` / `safeLocalSet` 靠 `error.message`）。
   */
  function callHost(cmd, args) {
    if (!bridgeReady()) {
      return Promise.reject(new Error(
        `设置窗外壳桥不可用（命令 ${cmd}）：本页面不是由设置窗宿主创建，或父窗口桥未就绪`
      ));
    }
    const target = parentWindow();
    const id = nextRequestId++;
    const timeoutMs = COMMAND_TIMEOUT_MS[cmd] || DEFAULT_TIMEOUT_MS;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (pendingRequests.delete(id)) reject(new Error(`外壳命令超时（${timeoutMs}ms）：${cmd}`));
      }, timeoutMs);
      pendingRequests.set(id, {
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (error) => { clearTimeout(timer); reject(error); },
      });
      try {
        // targetOrigin 用 '*'：消息只发给**父 frame 那一个 window**（不是广播），
        // 而我们发送的内容只有命令名与参数；应答侧再按 `event.source` 严格对账。
        target.postMessage({ [REQ]: true, id, cmd, args: args || {} }, '*');
      } catch (error) {
        pendingRequests.delete(id);
        clearTimeout(timer);
        reject(new Error(`向设置窗宿主发送命令失败：${String((error && error.message) || error)}`));
      }
    });
  }

  window.addEventListener('message', (event) => {
    if (event.source !== parentWindow()) return;
    const data = event.data;
    if (!data || typeof data !== 'object' || data[REP] !== true) return;
    const entry = pendingRequests.get(data.id);
    if (!entry) return;
    pendingRequests.delete(data.id);
    if (data.ok) entry.resolve(data.value);
    else entry.reject(new Error(String(data.error || '宿主命令失败')));
  });

  // ---------- 可见的「外壳说明」（本层自己的 DOM，不动上游 UI 的一个字节） ----------

  const NOTICE_ID = 'fnosShellNotice';
  let noticeBody = null;

  /**
   * 把一句说明挂进可见的说明框（追加在 `<body>` 末尾，**不覆盖**上游任何节点）。
   *
   * 为什么要有它：上游 popup.js 对本地存储写入失败只有两句固定文案
   * （popup.js:2176-2178 的「存储空间不足 / 本地存储写入失败」），它说不出**本壳为什么**
   * 拒绝。诚实要求「失败要看得见且说得出原因」，所以原因由本层直接画在界面上。
   */
  function notice(text) {
    if (!ensureNoticeBox()) return;
    if (text) appendNotice(text);
  }

  /** 建好说明框（幂等）。返回框里的正文节点是否可用。 */
  function ensureNoticeBox() {
    try {
      if (noticeBody) return true;
      if (typeof document === 'undefined' || !document.body) return false;
      const box = document.createElement('div');
      box.id = NOTICE_ID;
      box.dataset.fnosShell = '1';
      // 内联样式，且只用与主题无关的 rgba(currentColor)：上游 popup 自己带一整套
      // 明暗两套变量，本层不去猜它的类名，也不改它的样式表。
      box.setAttribute('style',
        'margin:8px;padding:8px 10px;border:1px dashed currentColor;border-radius:8px;' +
        'font:12px/1.6 system-ui,"Segoe UI",sans-serif;opacity:.85;white-space:pre-wrap;');
      const title = document.createElement('div');
      title.textContent = '外壳说明（本应用，非上游界面）';
      title.setAttribute('style', 'font-weight:600;margin-bottom:4px;');
      box.appendChild(title);
      noticeBody = document.createElement('div');
      box.appendChild(noticeBody);
      document.body.appendChild(box);
      // 固定的两条：本壳对上游 UI 的两处**有意差异**（D4 与离线更新检查）。
      appendNotice('字体文件导入：本外壳不提供（用户决策 D4）。请改用「网络字体 URL」或本机已安装的字体名。');
      appendNotice('更新检查：本外壳离线运行（已内置 vendored commit），不会发起任何网络请求。');
      return true;
    } catch (_error) {
      // 连说明框都画不出来（无 DOM）时也不能让上游的逻辑挂掉：错误仍会照常抛出。
      return false;
    }
  }

  function appendNotice(text) {
    if (!noticeBody) return;
    const line = '· ' + String(text == null ? '' : text);
    // 去重：上游会在初始化与每次刷新时都问一次列表，同一条拒绝说明只该出现一次——
    // 一墙重复的告警只会让人以为出了很多事。
    for (const child of noticeBody.children) {
      if (child.textContent === line) return;
    }
    const node = document.createElement('div');
    // 页面可控文本一律 textContent（本层没有任何 innerHTML 拼接）
    node.textContent = line;
    node.setAttribute('role', 'alert');
    noticeBody.appendChild(node);
  }

  // ---------- 配置缓存（sync.get 的回包 + 每次写回都刷新） ----------

  let cachedConfig = null;

  async function loadConfig() {
    const view = await callHost('get_config', {});
    if (view && typeof view === 'object') cachedConfig = view;
    return cachedConfig || {};
  }

  function configNow() {
    const snap = snapshot();
    if (snap.config && typeof snap.config === 'object') return snap.config;
    return cachedConfig || {};
  }

  // ---------- 键 → patch 的纯映射 ----------

  function owns(object, key) {
    return !!object && Object.prototype.hasOwnProperty.call(object, key);
  }

  /** `sync.get(defaults)` 的回包：逐键取 config.mods，缺键用 popup 自己给的默认值。 */
  function pickSync(defaults, config) {
    const mods = (config && config.mods && typeof config.mods === 'object') ? config.mods : {};
    const out = {};
    const requested = requestedKeys(defaults, SYNC_MODS_KEYS);
    for (const key of requested) {
      if (!SYNC_MODS_KEYS.includes(key)) continue; // 非本壳 mods 键：不发明、不返回
      out[key] = owns(mods, key) ? mods[key] : defaultFor(defaults, key);
    }
    return out;
  }

  /** `sync.set(data)` → `{mods:{…}}`；出现未知键就抛（不静默丢）。 */
  function syncPatch(data) {
    const patch = {};
    const unknown = [];
    for (const [key, value] of Object.entries(data && typeof data === 'object' ? data : {})) {
      if (SYNC_MODS_KEYS.includes(key)) patch[key] = value;
      else unknown.push(key);
    }
    if (unknown.length) {
      throw new Error(`本外壳没有映射这些同步存储键（未写入任何内容）：${unknown.join('、')}`);
    }
    return { mods: patch };
  }

  /** `local.get(defaults)`：四类键各自的来源（见文件头的分区）。 */
  function pickLocal(defaults, config, store) {
    const out = {};
    const local = (config && config.local && typeof config.local === 'object') ? config.local : {};
    const requested = requestedKeys(defaults, null);
    for (const key of requested) {
      if (LOCAL_CONFIG_KEYS.includes(key)) {
        out[key] = typeof local[key] === 'string' ? local[key] : defaultFor(defaults, key);
      } else if (key === LOCAL_WALLPAPER_NAME_KEY) {
        out[key] = typeof local.loginWallpaperFileName === 'string' && local.loginWallpaperFileName
          ? local.loginWallpaperFileName
          : defaultFor(defaults, key);
      } else if (key === LOCAL_WALLPAPER_DATA_KEY) {
        // 本壳不把 data URL 存两遍（字节在配置目录的 PNG 里），但上游用
        // `uploadedLoginWallpaperDataUrl && uploadedLoginWallpaperFileName` 两件事都非空
        // 才说「已导入」（popup.js:1303）。所以这里给一个明确的**宿主侧标记**，让
        // 「已导入: <落盘名> / PNG」这句话在**真的**导入过时出现，没导入时是空串。
        out[key] = typeof local.loginWallpaperFileName === 'string' && local.loginWallpaperFileName
          ? 'shell:imported'
          : defaultFor(defaults, key);
      } else if (LOCAL_FONT_KEYS.includes(key)) {
        // D4：从来没有、也不会有字体数据 → 返回 popup 请求的默认值（空串）是事实。
        out[key] = defaultFor(defaults, key);
      } else if (LOCAL_STORE_KEYS.includes(key)) {
        out[key] = owns(store, key) ? decodeStoreValue(store[key], defaultFor(defaults, key))
          : defaultFor(defaults, key);
      } else {
        // 未知键：读操作按「没有存过」回答（写操作另见 localSet / localRemove 的拒绝）。
        out[key] = defaultFor(defaults, key);
      }
    }
    return out;
  }

  /** defaults 支持对象 / 数组 / 省略三种形状（与 chrome.storage 的语义对齐）。 */
  function requestedKeys(defaults, fallback) {
    if (Array.isArray(defaults)) return defaults.map(String);
    if (defaults && typeof defaults === 'object') return Object.keys(defaults);
    if (typeof defaults === 'string') return [defaults];
    return fallback ? fallback.slice() : [];
  }

  function defaultFor(defaults, key) {
    if (defaults && typeof defaults === 'object' && !Array.isArray(defaults) && owns(defaults, key)) {
      return defaults[key];
    }
    return undefined;
  }

  function decodeStoreValue(raw, fallback) {
    if (typeof raw !== 'string') return fallback;
    try {
      return JSON.parse(raw);
    } catch (_error) {
      return fallback;
    }
  }

  // ---------- 写盘路径（会顺带重建主窗口的键，与 T13b 的设置窗行为一致） ----------

  /**
   * `set_config` 之后：`needsReload` 为真时调 `reload_main`。
   *
   * 谁会是 true（`commands::set_config` 的判据）：`shell.injectEnabled` / `shell.homeUrl`
   * 变更、完美图标**开关状态**变更（14 张 PNG 要不要进初始化载荷）、登录壁纸**文件名**变更
   * ——这三类都是建窗时注册的 `initialization_script` 的一部分，活窗口上换不掉。
   * 上游 UI 恰好能改到前两类中的完美图标那一类（`launchpadIconScaleEnabled` 等）与壁纸，
   * 所以这条通路必须保留，否则「勾上完美图标但页面上没有图标」。
   */
  async function afterConfigWrite(result) {
    if (result && result.config) cachedConfig = result.config;
    if (result && result.needsReload) {
      await callHost('reload_main', { url: null });
    }
    return result;
  }

  async function syncSet(data) {
    const patch = syncPatch(data);
    if (!Object.keys(patch.mods).length) return;
    await afterConfigWrite(await callHost('set_config', { patch }));
  }

  /**
   * `local.set`：字体键拒绝、壁纸键走宿主导入、CSS/JS 走 `local` 段、其余进本地存储。
   *
   * 上游**每次**调用只涉及同一类键（popup.js:2169-2173 字体三键、2227-2230 壁纸两键、
   * 1554-1557 两个代码键、1432-1434 更新状态一键），但这里仍按类分组处理：
   * 出现字体键就整次拒绝（在最前面，先拒绝再谈别的）。
   */
  async function localSet(data) {
    const entries = Object.entries(data && typeof data === 'object' ? data : {});
    const fontKeys = entries.map(([k]) => k).filter((k) => LOCAL_FONT_KEYS.includes(k));
    if (fontKeys.length) {
      const reason = '本外壳不提供本地字体文件导入（决策 D4）：请改用「网络字体 URL」'
        + '或本机已安装的字体名。字体数据没有写入任何地方。';
      notice(`拒绝了字体数据写入（${fontKeys.join('、')}）：${reason}`);
      throw new Error(reason);
    }

    const localPatch = {};
    const storePatch = {};
    const unknown = [];
    let wallpaper = null;

    for (const [key, value] of entries) {
      if (LOCAL_CONFIG_KEYS.includes(key)) {
        localPatch[key] = typeof value === 'string' ? value : String(value == null ? '' : value);
      } else if (key === LOCAL_WALLPAPER_DATA_KEY || key === LOCAL_WALLPAPER_NAME_KEY) {
        wallpaper = wallpaper || {};
        wallpaper[key] = value;
      } else if (LOCAL_STORE_KEYS.includes(key)) {
        storePatch[key] = JSON.stringify(value === undefined ? null : value);
      } else {
        unknown.push(key);
      }
    }

    if (unknown.length) {
      const reason = `本外壳没有映射这些本地存储键（未写入任何内容）：${unknown.join('、')}`;
      notice(reason);
      throw new Error(reason);
    }

    if (Object.keys(localPatch).length) {
      await afterConfigWrite(await callHost('set_config', { patch: { local: localPatch } }));
    }

    if (Object.keys(storePatch).length) {
      await callHost('set_local_store', { patch: storePatch });
    }

    if (wallpaper) await importWallpaperPair(wallpaper);
  }

  /**
   * 壁纸两键 → 宿主落盘 + `local.loginWallpaperFileName`。
   *
   * 只给 name（没给 data URL）不是上游的形状，如实报错；data URL 不是
   * `data:<mime>;base64,<载荷>` 也如实报错（不回显用户数据本身）。
   */
  async function importWallpaperPair(pair) {
    const dataUrl = pair[LOCAL_WALLPAPER_DATA_KEY];
    const name = pair[LOCAL_WALLPAPER_NAME_KEY];
    if (typeof name !== 'string' || !name) {
      throw new Error('壁纸导入失败：本外壳需要 png / jpg / jpeg / webp 的文件名');
    }
    const comma = typeof dataUrl === 'string' ? dataUrl.indexOf(',') : -1;
    if (comma < 0) {
      throw new Error('壁纸导入失败：只接受 data:<mime>;base64,<载荷> 形状的图片数据');
    }
    const head = dataUrl.slice(0, comma);
    if (head.indexOf(';base64') < 0) {
      throw new Error('壁纸导入失败：图片数据不是 base64 data URL');
    }
    const payload = dataUrl.slice(comma + 1);
    const ext = (name.split('.').pop() || '').toLowerCase();
    if (WALLPAPER_EXTS.indexOf(ext) < 0) {
      throw new Error('壁纸导入失败：只支持 png / jpg / jpeg / webp');
    }
    // 早退一次：base64 的长度上限 = 字节上限向上取整到 4 的倍数（与宿主同一算法）
    if (payload.length > Math.ceil(MAX_WALLPAPER_BYTES / 3) * 4) {
      throw new Error(`壁纸导入失败：图片超过 ${MAX_WALLPAPER_BYTES / 1024 / 1024} MiB 上限`);
    }
    const stored = await callHost('import_wallpaper', { name, dataBase64: payload });
    if (typeof stored !== 'string' || !stored) {
      throw new Error('壁纸导入失败：宿主没有返回落盘文件名');
    }
    await afterConfigWrite(await callHost('set_config', {
      patch: { local: { loginWallpaperFileName: stored } },
    }));
    notice(`登录壁纸已由宿主导入并落盘：${stored}（主窗口会按新载荷重建一次）`);
  }

  /** `local.remove`：壁纸键 → 清 `local.loginWallpaperFileName`；字体键 → 空操作成功。 */
  async function localRemove(keys) {
    const list = (Array.isArray(keys) ? keys : [keys]).map(String);
    const storePatch = {};
    const localPatch = {};
    const unknown = [];

    for (const key of list) {
      if (LOCAL_FONT_KEYS.includes(key)) {
        // D4：本壳从来没有存过字体数据，「删掉导入的字体」这件事**已经成立**，
        // 所以返回成功是如实的（真正的拒绝发生在写入那一步，而且带可见说明）。
        continue;
      }
      if (key === LOCAL_WALLPAPER_DATA_KEY || key === LOCAL_WALLPAPER_NAME_KEY) {
        localPatch.loginWallpaperFileName = null; // null = 删除该键（config.rs::merge 的语义）
      } else if (LOCAL_CONFIG_KEYS.includes(key)) {
        localPatch[key] = null;
      } else if (LOCAL_STORE_KEYS.includes(key)) {
        storePatch[key] = null;
      } else {
        unknown.push(key);
      }
    }

    if (unknown.length) {
      const reason = `本外壳没有映射这些本地存储键（未删除任何内容）：${unknown.join('、')}`;
      notice(reason);
      throw new Error(reason);
    }
    if (Object.keys(localPatch).length) {
      await afterConfigWrite(await callHost('set_config', { patch: { local: localPatch } }));
      if (owns(localPatch, 'loginWallpaperFileName')) {
        notice('登录壁纸已恢复默认（配置里的文件名已清除；旧图片文件保留在配置目录里，可手工删除）');
      }
    }
    if (Object.keys(storePatch).length) {
      await callHost('set_local_store', { patch: storePatch });
    }
  }

  // ---------- tabs.* ----------

  /** `get_page_state` → chrome.tabs.Tab 形状（active/currentWindow 恒真：主窗口就是那一页）。 */
  function tabFromPageState(page) {
    const url = page && typeof page.url === 'string' ? page.url : '';
    return {
      id: 1,
      active: true,
      currentWindow: true,
      url,
      title: url,
      // 上游只读 `.url` 与 `.id`（popup.js:180-181 / 787 / 1608 / 1774），其余字段是形状填充。
      index: 0,
      highlighted: true,
      pinned: false,
      incognito: false,
    };
  }

  /**
   * `FNOS_APPLY` 透传给父 frame：由它映射成 `set_config`（页面由宿主的
   * `apply_to_page` 就地收到 `__FNOS_APPLY_CONFIG__`）。这一条**不新增任何配置语义**——
   * 上游在发这条消息之前已经逐键 `storage.sync.set` 过了，配置就是权威，消息里的派生值
   * （夹取后的品牌色、规范化后的每列数量）由宿主 `Config::normalize` 再收一次口。
   */
  async function sendToPage(message) {
    const type = message && typeof message === 'object' ? message.type : null;
    if (type === 'FNOS_CHECK') {
      const answer = await callHost('page_check', {});
      return { isFnOSWebUi: !!(answer && answer.isFnOSWebUi) };
    }
    if (type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS' || type === 'FNOS_GET_LAUNCHPAD_APP_TITLES') {
      const answer = await callHost('app_items', {});
      const tooLarge = !!(answer && answer.tooLarge === true);
      const count = Number(answer && answer.itemCount);
      const itemCount = Number.isFinite(count) && count > 0 ? count : 0;
      if (tooLarge) {
        const howMany = itemCount > 0 ? `${itemCount} 个` : '过多';
        notice(`应用项列表过大（${howMany}），超过分片上报预算（8 片 × 3000 字节），无法上报：`
          + '逐项列表因此拿不到。请减少启动台里的应用数量，或直接编辑配置目录里的'
          + ' launchpadIconRedrawMap / launchpadIconRedrawKeys。');
      }
      // `tooLarge` / `pending` / `itemCount` 一并回给上游：上游只读 `items` / `titles`
      // （popup.js:790-797），多出来的字段是给**排障与测试**留的痕迹，不改变上游任何分支。
      return {
        items: Array.isArray(answer && answer.items) ? answer.items : [],
        titles: Array.isArray(answer && answer.titles) ? answer.titles : [],
        itemCount,
        tooLarge,
        pending: !!(answer && answer.pending === true),
      };
    }
    if (type === 'FNOS_APPLY') {
      await callHost('apply', { message });
      return {};
    }
    throw new Error(`本外壳没有映射这条页面消息（未发送任何内容）：${String(type)}`);
  }

  // ---------- 联网：只接管上游的更新检查 ----------

  /**
   * 上游更新检查的**离线**应答（popup.js:1153-1179 的期望形状：一个数组，第一项有 `sha`）。
   *
   * 返回的 sha 就是本壳内置的 vendored commit（`meta.modsCommit`）：于是
   * ① 首次检查 → `lastResult='first'` →「已记录当前最新提交」；
   * ② 之后每次 → `baseSha === sha` → `same` →「暂无更新」。两句都是事实。
   */
  function offlineCommitResponse() {
    const meta = (snapshot().config && snapshot().config.meta) || {};
    const sha = typeof meta.modsCommit === 'string' ? meta.modsCommit : '';
    const body = JSON.stringify([{
      sha,
      html_url: GITHUB_COMMITS_PAGE_URL,
      commit: {
        message: `本外壳内置的 vendored commit（离线，不请求网络）${sha ? `：${sha.slice(0, 7)}` : ''}`,
        committer: { date: '' },
      },
    }]);
    return new Response(body, {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  }

  function installFetchGuard() {
    const nativeFetch = typeof window.fetch === 'function' ? window.fetch.bind(window) : null;
    window.fetch = function (input, init) {
      const url = typeof input === 'string'
        ? input
        : (input && typeof input.url === 'string' ? input.url : '');
      if (url.indexOf(GITHUB_COMMITS_API_URL) === 0
        || url.indexOf('https://api.github.com/repos/aurysian-yan/FnOS_UI_Mods/') === 0) {
        return Promise.resolve(offlineCommitResponse());
      }
      if (nativeFetch) return nativeFetch(input, init);
      return Promise.reject(new Error('本外壳的设置窗里 fetch 不可用'));
    };
  }

  // ---------- chrome.* 本体 ----------

  const chromeApi = {
    runtime: {
      id: 'fnos-desktop-shell',
      /**
       * **同步**返回一个真实可取的 URL（上游把它交给 `fetch` 与 `<img src>`）。
       *
       * 只放行「设置窗资产根下的相对路径」：本壳的资产协议把 `ui/settings/` 作为根，
       * 所以 `prefect_icon/emby.png` 解析成 `http://tauri.localhost/prefect_icon/emby.png`。
       * 绝对 URL / 协议相对 URL / 带 `..` 的路径一律返回空串——上游对空串的语义正是
       * 「这个资源不存在」（`checkPrefectIconResourceExists`）。
       */
      getURL(path) {
        const raw = typeof path === 'string' ? path.trim() : '';
        if (!raw || raw.indexOf('..') >= 0 || raw.indexOf('//') === 0) return '';
        if (!ASSET_PATH.test(raw)) return '';
        try {
          return new URL(raw, document.baseURI).href;
        } catch (_error) {
          return '';
        }
      },
      /**
       * 版本显示。上游在脚本开头同步调用它，所以它读**父 frame 事先写好的快照**
       * （`config.meta.modsVersion`，与注入载荷同源，R38）；读不到时说「未知」而不是编一个版本号。
       */
      getManifest() {
        const meta = (snapshot().config && snapshot().config.meta) || {};
        return { version: typeof meta.modsVersion === 'string' && meta.modsVersion
          ? meta.modsVersion
          : '未知' };
      },
    },

    storage: {
      sync: {
        async get(defaults) {
          const config = await loadConfig();
          return pickSync(defaults, config);
        },
        async set(data) {
          await syncSet(data);
        },
      },
      local: {
        async get(defaults) {
          const config = await loadConfig();
          const store = await callHost('get_local_store', {});
          return pickLocal(defaults, config, store && typeof store === 'object' ? store : {});
        },
        async set(data) {
          await localSet(data);
        },
        async remove(keys) {
          await localRemove(keys);
        },
      },
    },

    tabs: {
      /** 上游只关心「当前窗口的活动标签页」= 主窗口那一页（不是设置窗自己）。 */
      async query() {
        const page = await callHost('get_page_state', {});
        return [tabFromPageState(page)];
      },
      async sendMessage(_tabId, message) {
        return sendToPage(message);
      },
      /** 上游的外链（popup.js:173）→ 宿主 `open_url` → 系统默认浏览器。 */
      async create(options) {
        const url = options && typeof options.url === 'string' ? options.url : '';
        await callHost('open_url', { url });
        return { id: 1 };
      },
    },

    action: {
      /** 本壳没有扩展图标徽标这个概念：空操作（上游 `syncActionBadge` 只当它是「不支持」）。 */
      async setBadgeText() {},
      async setBadgeBackgroundColor() {},
    },
  };

  // 安装：浏览器自己可能已经在 window 上放了 chrome（普通页面里它是 {app,csi,loadTimes}）。
  // 优先**填进**那个对象（它可能不可写/不可配置，但通常可扩展），失败才整体替换。
  function installChrome(api) {
    const existing = window.chrome;
    if (existing && typeof existing === 'object') {
      try {
        existing.runtime = api.runtime;
        existing.storage = api.storage;
        existing.tabs = api.tabs;
        existing.action = api.action;
      } catch (_error) {
        // 落到下面的整体替换
      }
    }
    const ok = !!window.chrome && !!window.chrome.storage && !!window.chrome.storage.sync;
    if (ok) return true;
    try {
      Object.defineProperty(window, 'chrome', {
        value: api, writable: true, configurable: true,
      });
    } catch (_error) {
      try {
        window.chrome = api;
      } catch (_error2) {
        return false;
      }
    }
    return !!window.chrome && !!window.chrome.storage && !!window.chrome.storage.sync;
  }

  installFetchGuard();
  const installed = installChrome(chromeApi);

  // 安装失败也**不静默**：说明框里写清楚，界面自己会呈现（上游随后会因为
  // `chrome.storage.sync` 缺失而抛错，用户看到的是白屏之外的明确原因）。
  if (!installed) {
    notice('chrome.* 兼容层安装失败：window.chrome 既不可写也不可扩展，上游设置界面无法工作。');
  } else if (!bridgeReady()) {
    notice('设置窗宿主桥未就绪：界面会渲染，但读写配置、读取主窗口状态都会明确失败。');
  }

  // 说明框要在 popup.js 之前就准备好（它是 append 到 body 末尾的，不挡任何控件）。
  notice('');
})();
