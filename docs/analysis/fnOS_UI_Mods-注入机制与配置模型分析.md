# FnOS UI Mods（Chrome MV3）注入机制与配置模型分析

分析对象：`D:\fnOS-desktop\.ref\fnOS_UI_Mods\`（只读，未修改任何文件）。
引用约定：`文件:行` 表示该文件的行号；带锚点的链接指向首次引用的证据位置。

## 0. 证据基线与「缺失件」声明

| 文件 | 行数 | 角色 |
| --- | --- | --- |
| [manifest.json](.ref/fnOS_UI_Mods/manifest.json) | 61 | MV3 清单：全站 `document_start` 内容脚本 + `web_accessible_resources` |
| [background.js](.ref/fnOS_UI_Mods/background.js) | 203 | Service Worker，**只做 GitHub 更新检查**，与注入无关 |
| [content-script.js](.ref/fnOS_UI_Mods/content-script.js) | 3337 | **真正的注入器 + 配置读取 + 全部动态逻辑** |
| [mod.js](.ref/fnOS_UI_Mods/mod.js) | 1705 | 页面世界独立行为脚本（squircle/动画），**不读配置** |
| [popup.html](.ref/fnOS_UI_Mods/popup.html) / [popup.js](.ref/fnOS_UI_Mods/popup.js) | 1270 / 2268 | 设置界面（读写配置 + 向 tab 发消息） |
| basic / titlebar / launchpad / lockscreen / desktop_icon `.css` | 1141 / 96 / 70 / 33 / 62 / 254 / 188 | 被注入的样式资源 |
| [README.md](.ref/fnOS_UI_Mods/README.md) / [LICENSE](.ref/fnOS_UI_Mods/LICENSE) / [package.json](.ref/fnOS_UI_Mods/package.json) | — | 文档 / 非商业许可 / 构建入口声明 |

**必须声明的缺失件**（无法证实的内容不做推测）：

- `package.json:8` 声明 `"build": "node scripts/build.js"`，`README.md:69` 也提到 `node scripts/build.js`，但参考集中**没有 `scripts/` 目录、没有 `scripts/build.js`**（实测该目录下无任何子目录）。
- `manifest.json:54` 与 `popup.js:108` 引用的 `prefect_icon/`（含 `icon-map.json` 与 `*.png`）、`manifest.json:14-19` 的 `icons/` 也都不在参考集中。

因此：**「构建脚本如何生成 content-script.js、是否把 CSS 内联进字符串」无法从代码证实**。下面第 3 节只陈述从现有代码可证实的行为（结论是：扩展路线下 CSS/JS 全部是**外链**，不内联）。

---

## 1. 注入判定：如何判断「当前页面是飞牛 WebUI」

判定函数 [content-script.js:2746-2779](.ref/fnOS_UI_Mods/content-script.js#L2746-L2779) `hasFnOSSignature()`，四条线索**或**关系：

```js
2746:  function hasFnOSSignature() {
2747:    const hostname = window.location.hostname || '';
2748:    const domainRegex = /(\.fnnas\.cn)$|(\.fnos\.net)$|(\.5ddd\.com)$|(\.fynas\.net)$/i;
2749:    const isKnownFnOSDomain = domainRegex.test(hostname);
2753:      const cookie = document.cookie || '';
2754:      hasFnOSToken =
2755:        cookie.includes('fnos-token') || cookie.includes('fnos-long-token') ||
2757:        Boolean(localStorage.getItem('fnos-token')) || ... sessionStorage ...
2765:    const hasFnOSDomMarkers = Boolean(document.querySelector('fn-app, [data-fn-id]'));
2769:    const hasAppCgiResource = performance.getEntriesByType('resource')
2771:      .some((entry) => ... entry.name.includes('/appcgi/'));
2773:    return (isKnownFnOSDomain || hasFnOSToken || hasFnOSDomMarkers || hasAppCgiResource);
```

要点：

1. **host 域名白名单正则**（2751-2749）：`.fnnas.cn / .fnos.net / .5ddd.com / .fynas.net`。
2. **Token 探测**（2753-2760）：`document.cookie` / `localStorage` / `sessionStorage` 中出现 `fnos-token` 或 `fnos-long-token`。注意这是**只读**探测，扩展从不写这些键。
3. **DOM 标记**（2765-2767）：`fn-app, [data-fn-id]`。
4. **网络痕迹**（2769-2771）：`performance` 资源列表里出现 `/appcgi/`。

等待机制：`waitForFnOSSignature(timeoutMs = 3000)` [2781-2804](.ref/fnOS_UI_Mods/content-script.js#L2781-L2804) —— 轮询靠 `MutationObserver(document.documentElement, {childList, subtree})`（2797-2800）+ `setTimeout` 超时（2802）。启动时实际只等 **1500ms**（2952、2879）。

**白名单旁路**：`enabledOrigins` 里包含当前 `ORIGIN` 时直接视为命中，连探测都不等：

```js
2947:      const isWhitelisted = Array.isArray(enabledOrigins) && enabledOrigins.includes(ORIGIN);
2949:      // Whitelisted origins should not wait for signature probing.
2950:      const matchesFnOSUi = isWhitelisted ? true : await waitForFnOSSignature(1500);
2953:      const autoEnabled = autoEnableSuspectedFnOS && matchesFnOSUi;
2970:      if (isWhitelisted || autoEnabled) { ...startInject(...) }
```

判定**失败时的行为**（2997-3034）：只把同步配置写进 `current*` 模块变量缓存，**不向页面插入任何节点**、不启动任何 observer；同时 `chrome.storage.onChanged` 的所有分支都因 `isInjectionActive === false` 走「仅缓存」路径（3043-3046、3060、3084 等）。popup 侧会显示徽标 `未发现 fnOS WebUI 页面特征`（`popup.html:948-955`，判定来自 `popup.js:1774-1778`）。

另一个前置条件：脚本只在顶层文档运行 ——

```js
2:  if (window.top !== window) return;
```

---

## 2. 配置模型（完整清单）

### 表 A：`chrome.storage.sync` —— 全局配置（唯一真源）

默认值来源：`content-script.js:2890-2917`，与 `popup.js:1785-1811` 完全一致（双份硬编码）。

| # | 键名 | 类型 | 默认值 | 取值枚举/范围 | 控制什么 | 证据 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `enabledOrigins` | `string[]`（origin 列表） | `[]` | 任意 `https://host` | 站点白名单：命中则跳过飞牛特征探测、必定注入 | cs:2892, 2947-2950；popup:1965-1977 |
| 2 | `autoEnableSuspectedFnOS` | `boolean` | `true` | — | 是否对「疑似飞牛页」自动注入 | cs:2893, 2953；popup:1979-1985 |
| 3 | `basePresetEnabled` | `boolean` | `true` | — | 总开关（标题栏 + 启动台 + 主题色） | cs:2894, 2642-2654；popup:1987-1991 |
| 4 | `windowAnimationBlurEnabled` | `boolean` | `true` | — | 窗口动画期间背景模糊；实现为 `documentElement` 上加/去 class `fnos-window-animation-blur-disabled` | cs:2895, 2656-2664；消费方 basic_mod.css:44 |
| 5 | `titlebarStyle` | `string` | `'windows'` | `windows` \| `mac` | 注入 `windows_titlebar_mod.css` 或 `mac_titlebar_mod.css` | cs:2896, 204-207, 2628-2636；popup:2002-2008 |
| 6 | `launchpadStyle` | `string` | `'classic'` | `classic` \| `spotlight` | 注入 `classic_launchpad_mod.css` 或 `spotlight_launchpad_mod.css` | cs:2897, 209-212；popup:2010-2017 |
| 7 | `desktopIconLayoutEnabled` | `boolean` | `true` | — | 是否注入 `desktop_icon_mod.css` + 生成网格行 CSS | cs:2898, 2449-2455, 1705-1717 |
| 8 | `desktopIconLayoutMode` | `string` | `'adaptive'` | `adaptive` \| `fixed` | 桌面图标行数用自适应变量还是固定列数 | cs:2899, 46, 1689-1699, 1719-1722 |
| 9 | `desktopIconPerColumn` | `number` | `8` | 4–16（`normalize` 夹取） | 固定网格每列图标数，写入 CSS 变量 `--fn-desktop-icons-per-column` | cs:2906, 47-49, 1680-1687, 1725 |
| 10 | `desktopIconPerColumnEnabled` | `boolean \| null` | `null` | — | **遗留键**，只读不写；作为 `desktopIconLayoutMode` 的旧版回退 | cs:2900, 1689-1699, 3108-3125；popup:1794（从未 set） |
| 11 | `launchpadIconScaleEnabled` | `boolean` | `false` | — | 「完美图标」：缩放异形图标 / 遮罩统一 / 重绘图标 的总开关 | cs:2901, 755-794 |
| 12 | `launchpadIconScaleSelectedKeys` | `string[]` | `[]` | 启动台图标 key（pathname 小写） | 逐图标缩放 | cs:2902, 776-778, 619-655 |
| 13 | `launchpadIconMaskOnlyKeys` | `string[]` | `[]` | 同上 | 逐图标只做遮罩、不改尺寸 | cs:2903, 773-775, 645-652 |
| 14 | `launchpadIconRedrawKeys` | `string[]` | `[]` | 同上 | 逐图标替换成自绘图标 | cs:2904, 763-772, 519-549 |
| 15 | `launchpadIconRedrawMap` | `Record<key, string>` | `{}` | 值必须匹配 `^prefect_icon/[a-z0-9-]+\.png$`（否则丢弃） | key → 替换图标相对路径 | cs:2905, 353-365, 510-517 |
| 16 | `brandColor` | `string` | `'#0066ff'` | 任意 `#rrggbb`，明度被夹到 **30%–70%** | 生成 10 阶 `--semi-brand-0..9` 调色板 | cs:2907, 43-45, 1667-1673, 1623-1665 |
| 17 | `fontOverrideEnabled` | `boolean` | `false` | — | 字体替换总开关 | cs:2908, 111-119 |
| 18 | `fontFamily` | `string` | `''` | 任意 font-family 串 | 正文字体 | cs:2909 |
| 19 | `fontMonospaceFamily` | `string` | `''` | 任意 | 等宽字体（code/pre/textarea/monaco/cm） | cs:2910, 2002-2020 |
| 20 | `fontWeight` | `string` | `''` | 如 `450` / `normal` | 字重 | cs:2911 |
| 21 | `fontFeatureSettings` | `string` | `''` | 如 `"liga" 1` | OpenType 特性 | cs:2912 |
| 22 | `fontFaceName` | `string` | `'FnOSCustomFont'` | — | `@font-face` 家族名（**popup 里对应输入框已被注释**，等于写死默认值） | cs:2913, 110, 117；popup.html:1175-1178 |
| 23 | `fontUrl` | `string` | `''` | URL | 网络字体（woff2 等） | cs:2914 |
| 24 | `customCodeEnabled` | `boolean` | `false` | — | 自定义 CSS/JS 注入开关 | cs:2915, 120-124；popup:1551-1553 |
| 25 | `lockscreenDefaultUsername` | `string` | `''` | 最长 80 字符 | 锁屏页自动填充用户名 + 隐藏输入框 + 「切换账户」按钮 | cs:2916, 60, 2333-2343；popup:1572-1576 |

### 表 B：`chrome.storage.local` —— 大对象/二进制资产

| 键名 | 类型 | 默认 | 内容 | 写入方 | 读取方 |
| --- | --- | --- | --- | --- | --- |
| `customFontDataUrl` | `string`(dataURL) | `''` | 导入的字体文件二进制 | popup:2169-2173 | cs:2548-2552 |
| `customFontFileName` | `string` | `''` | 原文件名 | popup:2171 | cs:2549 |
| `customFontFormat` | `string` | `''` | 推断格式（woff2/ttf…） | popup:2172 | cs:2551 |
| `loginWallpaperDataUrl` | `string`(dataURL) | `''` | 登录页壁纸 | popup:2227-2230 | cs:2574-2577 |
| `loginWallpaperFileName` | `string` | `''` | 原文件名 | popup:2228 | cs:2575 |
| `customCssCode` | `string` | `''` | 用户自定义 CSS 文本 | popup:1554-1557 | cs:2594-2601 |
| `customJsCode` | `string` | `''` | 用户自定义 JS 文本 | popup:1554-1557 | cs:2594-2601 |
| `updateCheckState` | `object` | — | GitHub 更新检查状态（与注入无关） | background.js:92；popup:1432 | background.js:85；popup:1512 |

（键名常量：cs:51-53、108-109；popup:76-83。）

### 表 C：页面 `localStorage` / `sessionStorage` —— **只读**，仅用于特征探测

| 键 | 用途 | 证据 |
| --- | --- | --- |
| `fnos-token` / `fnos-long-token` | 判断是否飞牛页面 | cs:2757-2760 |

### 表 D：页面 `window` 全局 —— 不是配置，是 content-script → popup 的数据通道

| 全局 | 内容 | 证据 |
| --- | --- | --- |
| `window.__fnosLaunchpadAppIconItems` | 启动台应用列表 `{key,title,iconSrc}[]` | cs:699-704, 2859-2864 |
| `window.__fnosLaunchpadAppIconTitles` | 同上标题数组 | cs:704, 2864 |
| `window.figmaSquircleConfig` | mod.js 自带硬编码配置 | mod.js:1-3 |

---

## 3. 配置 → 注入的映射

### 3.1 注入序列

`startInject(...)` [content-script.js:2694-2744](.ref/fnOS_UI_Mods/content-script.js#L2694-L2744) 是唯一入口（两个调用点：`2821` popup 手动应用、`2978` 自动注入），内部顺序：

```js
2713:    isInjectionActive = true;
2714:    startAppCenterMetaObserver();          // 应用中心元数据 DOM 补丁
2715:    startAppCenterRouteTransitionSupport();
2718:    startLockscreenStyleSync();            // 锁屏样式观察器/轮询
2721:    setBasePresetEnabled(basePresetEnabled);   // → 注入 basic + titlebar + launchpad
2723:    injectScript();                        // → <script src=mod.js>
2724:    setWindowAnimationBlurEnabled(...);    // → documentElement 加 class
2725-2729: updateDesktopIconLayout(...)        // → 网格行 CSS + desktop_icon_mod.css
2730:    updateBrandColor(brandColor);          // → theme <style> + inline 变量
2731:    updateFontSettings(...);               // → font <style>
2732:    updateCustomCodeSettings(...);         // → custom <style> + <script>
2733:    updateLockscreenDefaultUsername(...);
2734:    updateLoginWallpaper();                // → --fnos-login-wallpaper-url
2735:    updateLockscreenStyleInjection();      // → lockscreen_mod.css（条件）
2736-2742: updateLaunchpadIconScaleEnabled(...);
```

### 3.2 元素清单（全部 append 到 `(document.head || document.documentElement)`）

| 元素 | id | 模式 | 内容 | 条件 | 证据 |
| --- | --- | --- | --- | --- | --- |
| `<link rel=stylesheet>` | `fnos-ui-mods-basic-style` | **外链** | `basic_mod.css` | 恒注入 | 10, 2631 |
| `<link rel=stylesheet>` | `fnos-ui-mods-titlebar-style` | **外链** | `windows_titlebar_mod.css` \| `mac_titlebar_mod.css` | `basePresetEnabled` | 12, 204-207, 2634 |
| `<link rel=stylesheet>` | `fnos-ui-mods-launchpad-style` | **外链** | `classic_launchpad_mod.css` \| `spotlight_launchpad_mod.css` | `basePresetEnabled` | 13, 209-212, 2635 |
| `<link rel=stylesheet>` | `fnos-ui-mods-desktop-icon-mod-style` | **外链** | `desktop_icon_mod.css` | `desktopIconLayoutEnabled` | 20, 2449-2455 |
| `<link rel=stylesheet>` | `fnos-ui-mods-lockscreen-style` | **外链** | `lockscreen_mod.css` | 仅当登录页可见（动态增删） | 11, 2396-2409 |
| `<style>` | `fnos-ui-mods-theme-style` | 内联字符串 | `:root,body,#root,... { --semi-brand-N: … !important }` | `basePresetEnabled` | 15, 1623-1637, 1654-1665 |
| `<style>` | `fnos-ui-mods-font-style` | 内联字符串 | `@font-face` + `font-family` 覆盖规则 | 字体设置 | 16, 2029-2033 |
| `<style>` | `fnos-ui-mods-custom-css-style` | 内联字符串 | 用户 CSS 原文 | `customCodeEnabled` | 17, 2478-2483 |
| `<style>` | `fnos-ui-mods-desktop-icon-layout-style` | 内联字符串 | `--fn-desktop-icons-per-column` + `grid-template-rows` | `desktopIconLayoutEnabled` | 19, 1718-1730 |
| `<style>` | `fnos-ui-mods-launchpad-icon-scale-style` | 内联字符串 | 固定 CSS（blur clone / scale 0.75 / mask） | 按需 | 21, 289-333 |
| `<script src>` | `fnos-ui-mods-script` | **外链** | `chrome-extension://<id>/mod.js` | 恒注入 | 14, 2666-2676 |
| `<script>` 内联 | `fnos-ui-mods-custom-js-script` | 内联字符串 | 用户 JS 原文，`script.textContent = code` | `customCodeEnabled && js` | 18, 2457-2463, 2491-2497 |

外链 URL 生成：`safeRuntimeGetURL` [242-252](.ref/fnOS_UI_Mods/content-script.js#L242-L252) → `chrome.runtime.getURL(path)`，配合 `manifest.json:43-60` 的 `web_accessible_resources`（7 个 CSS + `mod.js` + `prefect_icon/*`）。

**所以：扩展路线下 CSS 与 JS 都不内联**，只有「参数化的动态样式/用户代码」才是内联字符串。关于 `scripts/build.js` 是否把 CSS 内联成 `content-script.js` 字符串——参考集缺该文件，**无法证实，不推测**。

### 3.3 时机

- 清单层：`manifest.json:28` `"run_at": "document_start"`。
- 执行层：脚本立即发起 3 个 `storage.local.get`（2886-2888）+ 1 个 `storage.sync.get`（2890），回调里 `await Promise.all([...])`（2945）后 `waitForFnOSSignature(1500)`（2952）→ 才注入。**即最坏情况在 document_start 之后约 1.5s 才注入。**
- 锁屏样式是持续性的：`MutationObserver(documentElement,{subtree,childList})`（2421-2427）+ `setInterval(…, 1200)`（2430-2434）+ `load/pageshow/popstate/visibilitychange` 监听（2436-2446）。
- 应用中心/启动台图标也是持续性：observer + `requestAnimationFrame` 合并（712-728、1385-1391）。
- `mod.js` 自身再等 `DOMContentLoaded`，并对「脚本注入很晚」做兜底（mod.js:1697-1705）。

### 3.4 热更新（不刷新页面即生效）

`chrome.storage.onChanged` [3039-3332](.ref/fnOS_UI_Mods/content-script.js#L3039-L3332) 按 `area === 'sync'` / `'local'` 分流，逐键增量更新；`area==='sync'` 分支末尾 `return`（3263），`local` 分支处理大对象资产。popup 改设置后还会额外发 `FNOS_APPLY` 消息（popup.js:1608-1629 → cs:2808-2851）。

---

## 4. mod.js 的角色

**结论：mod.js 是自足的页面行为脚本，不是工具库；它不读任何扩展配置，也不访问任何 `chrome.*`。**

证据：

1. **零 `chrome.*`**：对 `mod.js` 全文检索 `chrome.|browser.|localStorage|sessionStorage` → **0 命中**。唯一的外部输入是它自己写死的配置：

```js
// mod.js:1-3
window.figmaSquircleConfig = {
    ".semi-switch": { cornerRadius: 999, smoothing:1 }
};
```

2. **与 content-script 的耦合只有「CSS/属性契约」，没有函数调用**：content-script 只是把 mod.js 作为 `<script src>` 插进页面（2666-2676），从不调用 mod.js 里的函数；mod.js 也从不读 content-script 写的 `window.__fnosLaunchpad*`（对 mod.js 检索 `__fnos` → 0 命中）。契约体现为类名：
   - `mod.js:395-399` 定义 `fnos-window--enter/exit/…`，`basic_mod.css:93-114` 消费这些类；
   - `mod.js:594/670/675` 读写 `data-fnos-window-restore-pending` / `data-fnos-window-animation-bypass` / `fnos-window-animating-out`。
3. 它读取的元素级「配置」只有 data 属性（mod.js:377-378 `data-corner-radius` / `data-smoothing`）。
4. 结构（1705 行）：`applyFigmaSquirclesFromConfig`（5-104）→ figma-squircle-web 的 clip-path 实现（111-389）→ `setupAppWindowAnimations`（391-762）→ `setupTaskbarItemAnimations`（764-1282）→ `setupLoginClock`（1284-1428）→ `setupSmoothScrollContainers`（1430-1685）→ `initialize()`（1688-1705，DOMContentLoaded + readyState 兜底）。
5. 内部有 `window._fnosWindowAnimationInitialized` 之类的幂等锁（mod.js:392-393）。

**对 Tauri 的含义**：mod.js 可以原样当普通页面脚本内联/注入，不需要任何 shim；代价是它也**永远不会**响应我们的配置（除 CSS 类契约外）。

---

## 5. `chrome.*` 依赖清单与 shim 可行性

全扩展 API 面：`storage` / `runtime` / `tabs` / `action`。**没有** `i18n`、`scripting`、`declarativeNetRequest`、`webRequest`、`windows`、`cookies`（全文检索 0 命中）。`manifest.json:6-13` 只申请 `storage`、`tabs`、`unlimitedStorage` + `<all_urls>`。

### content-script.js（决定注入的 8 处，最关键）

| 行号 | 调用 | 用途 | 无 `chrome` 时后果 | shim 方案 |
| --- | --- | --- | --- | --- |
| 246 | `chrome?.runtime?.id` | 存活检测 | `getURL` 返回 `''` → **所有外链注入静默失败** | 让 shim 返回固定 id 或直接短路为 true |
| 247 | `chrome.runtime.getURL(path)` | 生成 5 个 CSS + mod.js + `prefect_icon/*` 的 URL | 同上，样式与 mod.js 全不加载 | 返回 `asset://…` / `data:`，或改成内联字符串 |
| 257 | `chrome.runtime.getManifest()?.version` | 响应网页版本询问（264-287） | 仅该功能失效 | 返回常量 |
| 2548 / 2574 / 2594 | `chrome.storage.local.get(defaults)` | 读字体/壁纸/自定义代码 | 自定义字体、壁纸、自定义代码失效（`catch` 里置空） | `localStorage` JSON 或注入的配置对象 |
| 2681 | `chrome.runtime.sendMessage` | 通知 SW「已注入」（触发更新检查） | 无关紧要（已 try/catch） | no-op |
| 2808 | `chrome.runtime.onMessage.addListener` | 收 `FNOS_APPLY` / `FNOS_GET_LAUNCHPAD_*` / `FNOS_CHECK` | **无法热更新配置**；popup 拿不到应用列表 | `window.postMessage` 或 Tauri event |
| 2890 | `chrome.storage.sync.get(defaults, cb)` | **主配置读取**（表 A 全部 25 键） | **完全不注入**（回调永不执行） | 从注入的 `window.__FNOS_CONFIG__` 构造并回调 |
| 3039 | `chrome.storage.onChanged.addListener` | 配置热更新 | 改设置需刷新页面 | Tauri event → 调用同一处理函数 |

（另有 `localStorage/sessionStorage` 的**只读**使用：2757-2760，属于页面 API，不是扩展 API。）

### background.js（8 处，可整块删除）

| 行号 | 调用 | 说明 |
| --- | --- | --- |
| 8 | `chrome.runtime.getManifest().version` | 版本比对 |
| 85 / 92 | `chrome.storage.local.get/set` | 更新状态 |
| 98 / 101 / 104 | `chrome.action.setBadgeText` / `setBadgeBackgroundColor` | 工具栏徽标 `UP` |
| 171 / 175 | `chrome.runtime.onStartup` / `onInstalled` | 触发检查 |
| 179 | `chrome.runtime.onMessage` | 处理 `FNOS_INJECTION_TRIGGERED` / `FNOS_CHECK_UPDATE_NOW` / `FNOS_SYNC_UPDATE_BADGE`（180-199） |

功能仅为「查 GitHub commits 判断有无更新」（55-82、114-169）。**与 UI 注入无关，Tauri 下直接丢弃。**

### popup.js（17 处，Tauri 用自绘设置窗全部替代）

| 行号 | 调用 | 用途 |
| --- | --- | --- |
| 62 | `chrome.runtime.getManifest().version` | 显示版本 |
| 173 | `chrome.tabs.create` | 外链打开 |
| 180 | `chrome.tabs.query({active,currentWindow})` | 取当前 tab |
| 389 / 480 | `chrome.runtime.getURL` (+`fetch`) | 读 `prefect_icon/icon-map.json`、探测 png 是否存在 |
| 787 / 1608 / 1774 | `chrome.tabs.sendMessage` | 取启动台应用列表 / `FNOS_APPLY` / `FNOS_CHECK` |
| 1117 / 1120-1121 | `chrome.action.*Badge*` | 更新徽标 |
| 1391 | `chrome.storage.sync.set` | 写全局配置 |
| 1403 / 1421 | `chrome.storage.local.set/remove` | 写/删大对象 |
| 1512 / 1813 | `chrome.storage.local.get` | 读更新状态 / 读资产 |
| 1785 | `chrome.storage.sync.get` | 读全局配置 |

### shim 规模评估

content-script 真正需要的 API 面极小：`runtime.id`、`runtime.getURL`、`runtime.getManifest`、`runtime.sendMessage`、`runtime.onMessage`、`storage.local.get`、`storage.sync.get`、`storage.onChanged`。**约 80–150 行 JS 即可覆盖**（其中 `storage.*.get(defaults, cb)` 的「默认值合并 + 回调」签名必须完全对齐，因为 content-script 用的是回调形式 2890-2944）。popup 侧不需要 shim —— 我们用 Tauri 自己写设置窗。

---

## 6. popup 设置界面（要在 Tauri 设置窗复刻的清单）

popup 是一个 1270 行的单页（`popup.html:957-1266` 是唯一 `.card`），结构 = `分组 → 项 → 配置键 → 控件 → 取值`：

| 分组（HTML） | 项 | 配置键 | 控件类型 | 取值/行为 | 证据 |
| --- | --- | --- | --- | --- | --- |
| 头部（`code.version` / `#updatePanel`） | 版本 1.0.2；检查更新按钮；最新提交链接 | 无（仅 local `updateCheckState`） | 文本 + button + a | 可整个删除 | popup.html:916-934 |
| 当前站点（`.title` + badge） | 显示 origin / 「可能为 fnOS 页面」徽标 | 只读 | 文本 + 徽标 | 来自 `FNOS_CHECK` 响应 | popup.html:936-955；popup.js:1774-1783 |
| 当前站点 | 为当前站点注入 | `enabledOrigins`（sync，数组增删 origin） | checkbox（switch） | 勾选即把 `origin` push 进数组 | popup.html:959-961；popup.js:1965-1977 |
| 当前站点 | 自动对疑似 fnOS WebUI 页面启用 | `autoEnableSuspectedFnOS` | checkbox | — | popup.html:980-982；popup.js:1979-1985 |
| 基础美化预设 | 基础美化预设（关闭后不应用主题色/标题栏/启动台） | `basePresetEnabled` | checkbox | — | popup.html:1001-1010；popup.js:1987-1991 |
| 主题色 | 主题色 | `brandColor` | `input type=color` + 重置按钮 | 任意 hex，明度夹 30%–70% | popup.html:1029-1031, 1018-1024；popup.js:2054-2070 |
| 主题色 | （备用）文本输入 | `brandColor` | `input type=text`（**HTML 里不存在此元素**） | popup.js:36/2072-2077 是死代码 | popup.js:36, 2072-2077 |
| 窗口 | 窗口动画模糊 | `windowAnimationBlurEnabled` | checkbox | — | popup.html:1036-1044；popup.js:1993-2000 |
| 标题栏样式 | Windows 标题栏样式 | `titlebarStyle='windows'` | radio（`name=titlebarStyle`, `value=windows`） | 二选一 | popup.html:1048-1054；popup.js:2002-2008 |
| 标题栏样式 | macOS 标题栏样式 | `titlebarStyle='mac'` | radio（`value=mac`） | 二选一 | popup.html:1056-1062 |
| 启动台样式 | 经典启动台样式 | `launchpadStyle='classic'` | radio（`name=launchpadStyle`） | 二选一 | popup.html:1066-1072；popup.js:2010-2017 |
| 启动台样式 | Spotlight 启动台样式 | `launchpadStyle='spotlight'` | radio | 二选一 | popup.html:1074-1080 |
| 桌面图标优化 | 桌面图标优化 | `desktopIconLayoutEnabled` | checkbox | — | popup.html:1084-1092；popup.js:2029-2033 |
| 桌面图标优化 | 桌面图标布局 | `desktopIconLayoutMode` | `select`（`adaptive` 自适应 / `fixed` 固定网格） | 2 值 | popup.html:1096-1099；popup.js:2048-2052 |
| 桌面图标优化 | 固定网格每列数量 | `desktopIconPerColumn` | `input type=number`（min=4,max=16,step=1,placeholder=8） | 4–16 | popup.html:1104；popup.js:2035-2046 |
| 完美图标 | 完美图标 | `launchpadIconScaleEnabled` | checkbox | 打开后向页面要应用列表 | popup.html:1108-1117；popup.js:2019-2027 |
| 完美图标 | 应用列表（逐项操作） | `launchpadIconScaleSelectedKeys` / `...MaskOnlyKeys` / `...RedrawKeys` + `...RedrawMap` | 动态列表（由 `FNOS_GET_LAUNCHPAD_APP_ITEMS` 渲染） | 每项三态之一 + 自绘图标路径 `prefect_icon/*.png` | popup.html:1119-1120；popup.js:787-789, 841-857 |
| 字体替换 | 字体替换总开关 | `fontOverrideEnabled` | checkbox | — | popup.html:1129-1130；popup.js:1538-1548 |
| 字体替换 | 本地字体名称 | `fontFamily` | `input type=text` | 如 `"MiSans VF", sans-serif` | popup.html:1150-1151 |
| 字体替换 | 等宽字体名称 | `fontMonospaceFamily` | `input type=text` | — | popup.html:1156-1157 |
| 字体替换 | 网络字体 URL | `fontUrl` | `input type=url` | woff2 等 | popup.html:1162 |
| 字体替换 | 字重 | `fontWeight` | `input type=text` | `450`/`normal`/`600` | popup.html:1167 |
| 字体替换 | OpenType 属性 | `fontFeatureSettings` | `input type=text` | `"liga" 1, "kern" 1` | popup.html:1172 |
| 字体替换 | 自定义字体名称 | `fontFaceName` | **注释掉的输入框** | 实际不可配 | popup.html:1175-1178 |
| 字体替换 | 导入字体文件 | `customFontDataUrl` / `customFontFileName` / `customFontFormat`（local） | **注释掉的 file 输入 + 按钮** | UI 不可用，但 popup.js:2161-2213 仍保留处理逻辑 | popup.html:1180-1188；popup.js:2161-2213 |
| 登录页 | 导入本地图片（png/jpg/webp） | `loginWallpaperDataUrl` / `loginWallpaperFileName`（local） | `input type=file` + 「恢复默认壁纸」按钮 | — | popup.html:1199-1207；popup.js:2215-2265 |
| 登录页 | 登录默认用户名 | `lockscreenDefaultUsername`（sync） | `input type=text`（maxlength=80） | 留空关闭 | popup.html:1208-1213；popup.js:1572-1576 |
| 自定义代码 | 自定义代码注入 | `customCodeEnabled`（sync） | checkbox | — | popup.html:1222-1223；popup.js:1551-1553 |
| 自定义代码 | 自定义 CSS | `customCssCode`（local） | `textarea` | 失焦/变更即存并应用 | popup.html:1244-1245；popup.js:1554-1557 |
| 自定义代码 | 自定义 JavaScript | `customJsCode`（local） | `textarea` | 同上 | popup.html:1250-1251 |

写入路径：布尔/字符串类 → `safeSyncSet`（popup.js:1389-1399）→ `chrome.storage.sync.set`；大对象 → `safeLocalSet`（1401-1417）→ `chrome.storage.local.set`；清空 → `safeLocalRemove`（1419-1429）。popup 还会做「读回后归一化并回写」（popup.js:1885-1932、1855-1857），即存在*隐式迁移*逻辑，Tauri 复刻时要保留归一化函数（`normalizeDesktopIconLayoutMode`、`clampBrandLightness`、`normalizeLockscreenDefaultUsername` 等）。

---

## 7. 样式分级与互斥关系

**基础层**：`basic_mod.css`（195 KB / 1141 行 / 138 个规则块）。`manifest.json:46` 列为 web 资源，`content-script.js:2631` **无条件注入**。README.md:27-38 列的功能（窗口背景模糊、开合动画、任务栏滚动与精简、悬停、右键菜单、大多数区域平滑圆角）都由它承载。

**可选项层**（映射表在 `content-script.js:204-212`）：

```js
204:  const TITLEBAR_STYLES = { windows: 'windows_titlebar_mod.css', mac: 'mac_titlebar_mod.css' };
209:  const LAUNCHPAD_STYLES = { classic: 'classic_launchpad_mod.css', spotlight: 'spotlight_launchpad_mod.css' };
```

| 文件 | 行数/规则块 | 定位 | 互斥 |
| --- | --- | --- | --- |
| `windows_titlebar_mod.css` | 70 / 11 | 标题栏可选项：类 Windows（标题左对齐、按钮在右） | 与 mac **互斥** |
| `mac_titlebar_mod.css` | 96 / 17 | 标题栏可选项：类 macOS（标题居中、红绿灯在左） | 与 windows **互斥** |
| `classic_launchpad_mod.css` | 33 / 1 | 启动台可选项：类 macOS 全屏启动台 | 与 spotlight **互斥** |
| `spotlight_launchpad_mod.css` | 62 / 6 | 启动台可选项：类 macOS 聚焦面板（648×512、superellipse(2)、backdrop blur） | 与 classic **互斥** |
| `desktop_icon_mod.css` | 188 / 11 | 桌面图标布局/悬停，自带 `:root` 变量与媒体查询阶梯（`--fn-desktop-icons-adaptive-target` 3…20） | 由开关控制，非互斥 |
| `lockscreen_mod.css` | 254 / 30 | 锁屏/登录页，**条件注入**（只在登录页可见时） | 非互斥 |

**互斥是结构性的，不是约定的**：两张标题栏共用同一个 `<link>` 元素 id `fnos-ui-mods-titlebar-style`（2634）+ 同一张映射表；启动台同理（2635），且 `normalizeTitlebarStyle` / `normalizeLaunchpadStyle`（223-229）把任何非法值夹回默认。所以「mac + windows 同时生效」在架构上不可能。

**几个容易被忽略的关系**：

- `basePresetEnabled === false` 时移除 titlebar + launchpad 的 `<link>` 并清掉调色板（2642-2654），**但 `basic_mod.css` 与 `mod.js` 照旧注入**（2631 / 2723 不受该开关控制）。也就是说这个开关名为「基础美化预设」，实际只关掉「主题色 + 标题栏 + 启动台」。
- `basic_mod.css:44` 消费 `documentElement.fnos-window-animation-blur-disabled`（由配置 4 驱动，2656-2664）。
- `basic_mod.css:1104-1106` 消费 `fnos-launchpad-icon-box--processed/--scaled/--mask-only`（由完美图标逻辑打标）。
- `basic_mod.css:1129-1139` 用 `--fnos-login-wallpaper-url/-position/-size` 给登录页背景（默认是 Unsplash 图），content-script 覆盖它并额外对 `.login-form` 写 `!important` inline 背景（2500-2544）。
- `desktop_icon_mod.css:4-9,129` 自带 `--fn-desktop-icons-per-column` 与 `--fn-desktop-icons-adaptive-target` 默认值，content-script 只在 `fixed` 模式覆盖（1722-1728）。
- README.md:40-53 明确「传统/反转标题栏」「传统/聚焦启动台」两组各二选一，并说明「可在插件弹出窗口进行配置」。
- README.md:27 提示 `corner-shape` 需要 **Chrome 139+**（Tauri 复用系统 WebView2/WebKit 时需核对内核版本，否则平滑圆角会退化）。

---

## 8. 与「只注入 basic_mod.css + mod.js + 喂一份配置」的差距

### 8.1 能复现的部分

- **`basic_mod.css` 全量**：全部静态视觉（圆角、模糊、动画关键帧、任务栏、悬停、右键菜单等 138 个规则块）。**但它不认任何配置**（CSS 里没有任何由 content-script 读配置后设置的开关，除了 `:root.fnos-window-animation-blur-disabled`，basic_mod.css:44）。
- **`mod.js` 全部行为**：squircle clip-path、窗口开/关/最小化/还原动画状态机、任务栏图标滚动、登录时钟、平滑滚动容器 —— 因为 mod.js **不读配置**（第 4 节），喂不喂配置对它毫无区别。
- **唯一一个「配置直接映射成 CSS 类」的项**：`windowAnimationBlurEnabled` → 给 `documentElement` 加/去 `fnos-window-animation-blur-disabled`（2656-2664）。我们自己写 3 行就能复现。

### 8.2 缺口（逐项）

| # | 缺口 | 为什么缺 | 需要补什么 | 证据 |
| --- | --- | --- | --- | --- |
| 1 | 标题栏样式（windows/mac） | 两张 CSS 是**独立文件**，不在 basic_mod.css 内 | 自建 2 个 `<link>`/`<style>` + 开关 | cs:204-207, 2628-2636 |
| 2 | 启动台样式（classic/spotlight） | 同上 | 同上 | cs:209-212 |
| 3 | 桌面图标 mod | `desktop_icon_mod.css` 未包含在 basic_mod.css | 追加一个条件注入器 | cs:2449-2455 |
| 4 | 桌面图标网格模式/每列数 | 动态生成的 CSS（变量 + `grid-template-rows`）不在任何 CSS 文件里 | 复刻 `updateDesktopIconLayout` 的 CSS 生成（含 `fixed`/`adaptive` 分支） | cs:1705-1730 |
| 5 | 锁屏样式 | `lockscreen_mod.css` 是条件注入，且需要「是否登录页」判定（元素可见性 + 排除桌面外壳） | 复刻 `isLockscreenView()` + MutationObserver + 1.2s 轮询 | cs:2380-2409, 2419-2446 |
| 6 | 主题色 → 10 阶调色板 | `basic_mod.css` 只**消费** `--semi-brand-*`（如 1047），不生成 | 复刻 `generateBrandPalette`/`buildThemeCss`/`applyBrandPaletteInline` + 明度夹取 | cs:1623-1665, 1667-1673 |
| 7 | 字体替换 | 需要动态 `@font-face` + 多选择器覆盖（含等宽组） | 复刻 `buildFontOverrideCss` / `normalizeFontSettings` | cs:1990-2033 |
| 8 | 自定义 CSS / JS 注入 | 纯运行时 | 复刻 2 个内联 `<style>` / `<script>` 通道 + 幂等重注入 | cs:2457-2498 |
| 9 | 登录页壁纸 | 需要写 CSS 变量 + 对 `.login-form` 强制 inline 背景 | 复刻 `updateLoginWallpaper` / `syncLoginWallpaperInlineStyle` | cs:2500-2544 |
| 10 | 锁屏默认用户名 / 文本头像 / 切换账户按钮 | 纯 DOM 构造（含**拼音首字母**算法，`Intl.Collator('zh-Hans-u-co-pinyin')`） | 复刻 `syncLockscreenDefaultUsername` / `syncLockscreenTextAvatar` | cs:2053-2075, 2261-2301, 2333-2378 |
| 11 | 完美图标（缩放/遮罩/重绘） | 需要扫描启动台卡片的 MutationObserver、按 key 打标、替换 `<img src>` | 复刻 595-794 + 510-549；**且 `prefect_icon/*.png` 不在参考集** | cs:510-549, 595-794 |
| 12 | 应用中心元数据/路由补丁 | 全部逻辑在 content-script（标签映射、下载数格式化、路由前进后退补丁） | 复刻 796-1270 + 1385-1391 | cs:214-221, 796-1270 |
| 13 | 注入判定与自动启用 | `hasFnOSSignature` + 白名单 + 等 1.5s | 复刻 2746-2804、2947-2953 | 同第 1 节 |
| 14 | 热更新（改设置免刷新） | 靠 `storage.onChanged` + `FNOS_APPLY` 消息 | 自建配置推送通道 | cs:2808-2851, 3039-3332 |
| 15 | 站点白名单 UI 语义 | `enabledOrigins` 由 popup 的 per-site 开关维护 | 设计 Tauri 侧的「目标地址/白名单」模型 | cs:2892, 2947-2950 |

**总体判断**：`basic_mod.css + mod.js` 覆盖的是「静态外观 + 全部 JS 动画」这一大块（项目价值的大头），**但所有「可配置项」与「条件注入」100% 在 content-script.js 里**，配置对象喂给 CSS/JS 本身不会被消费。所以方案 C 只能得到一套**固定外观**、不可配置的注入。

---

## 9. Tauri 2 复刻建议

**推荐：B（搬运 content-script.js + 轻量 `chrome` shim），并按下面几条做「B′」改造。** A 与 C 都不推荐。

### 为什么不选 A（自己写注入器 + 喂配置）

第 8 节列的 15 个缺口里，10 个以上是**必须重写**的算法级逻辑（调色板生成、拼音首字母、启动台卡片扫描与打标、应用中心路由补丁、锁屏可见性判定、字体/等宽 CSS 构造、幂等热更新）。工作量是数千行，且完全依赖对飞牛 DOM 结构的逆向结果——而这份逆向成果**已经存在于 content-script.js 的 3337 行里**。重写等于把已付费的逆向成本再付一次。

### 为什么不选 C（只注入 basic_mod.css + mod.js）

改动最小（约 30 行），但如 8.2 所述：**没有任何可配置项生效**，`basePresetEnabled`/`titlebarStyle`/`launchpadStyle`/`brandColor`/字体/锁屏/桌面图标/自定义代码全部丢失，`desktop_icon_mod.css` 与 `lockscreen_mod.css` 也不会被加载。只适合「能看就行」的演示。

### 为什么选 B（工作量差异）

`content-script.js` 对 `chrome.*` 的依赖面**极小且集中**：真正决定注入的只有 4 个调用点（`runtime.id` + `runtime.getURL` @246-247、`storage.local.get` @2548/2574/2594、`storage.sync.get` @2890、`storage.onChanged` @3039），其余 4 处是可 no-op 或可替换的旁路。**shim 约 80–150 行**即可让这 3337 行原样跑起来，保真度接近 100%。

### B′ 落地要点

1. **注入时机**：必须用 Tauri 的 *initialization script*（`WebviewWindowBuilder::initialization_script`，等价于 `document_start`）。若等页面加载完再注入，会出现「先闪原始 UI 再变样」。这正好对应 `manifest.json:28`。
2. **资源交付**：6 个 CSS + `mod.js` 建议**直接内联成字符串**塞进 initialization script（绕开自定义协议与 CSP）；若想保留 `chrome.runtime.getURL` 的形状以少改代码，就让 shim 返回 `data:` URL 或 Tauri asset 协议 URL，并把 `manifest.json:43-60` 的 `web_accessible_resources` 语义丢掉。
3. **配置交付**：Tauri 侧存 JSON（Rust 结构体 + 前端表单），在 initialization script 里注入 `window.__FNOS_CONFIG__`；shim 的 `storage.sync.get(defaults, cb)` 从该对象做「默认值合并 + 异步回调」，行为要与 `content-script.js:2890-2944` 完全一致（回调形式、`Promise.all` 等待、`waitForFnOSSignature` 之后才 `startInject`）。
4. **本地大对象**（字体 dataURL、壁纸 dataURL、自定义代码）不要用 `localStorage`（容量/同步阻塞），交给 Tauri 文件系统或 SQLite，`storage.local.get` 时按需读取。
5. **热更新**：`storage.onChanged` 用 Tauri event 触发，直接复用 `content-script.js:3039-3332` 的整套分支（它已经写好了每个键的增量更新语义）。
6. **popup 替换**：按第 6 节的表在 Tauri 设置窗重建控件；保留 popup.js 里的归一化/迁移函数（`normalizeDesktopIconLayoutMode`、`clampBrandLightness`、`normalizeLaunchpadKeyList` 等），否则旧配置会出现无法解释的降级。
7. **丢弃**：`background.js` 全部（GitHub 更新检查，与 UI 无关）；`chrome.tabs.*` / `chrome.action.*`；popup 的更新面板。
8. **保留的条件逻辑不要删**：`if (window.top !== window) return;`（cs:2）、`extensionContextInvalidated` 的 try/catch 兜底（231-262）在 Tauri 下也应改造成「配置服务不可用时的降级」。
9. **许可证**：README.md:96-101 + LICENSE 为 *FnOS UI Mods Non-Commercial License 1.0*，**禁止任何直接或间接盈利用途**；Tauri 壳若分发需先确认合规。
10. **内核版本**：`corner-shape`（basic_mod.css:11 等多处、README.md:27）需要 Chrome 139+；Windows 走 WebView2 需核对版本，Linux WebKitGTK 大概率不支持，平滑圆角会退化成普通圆角——这是复刻保真度的**首要外部风险**。

### 工作量粗略对比

| 方案 | 代码量 | 保真度 | 主要风险 |
| --- | --- | --- | --- |
| A 自写注入器 | 数千行（重写 10+ 算法） | 取决于逆向完整度，易漏 | 逆向不全、长期维护成本 |
| **B 搬运 + shim（推荐）** | **~100 行 shim + 3 处替换点 + 设置窗（按第 6 节表格）** | **接近 100%** | shim 的 `storage.get` 回调签名要对齐；WebView 内核差异 |
| C 只注入基础资源 | ~30 行 | 固定外观，配置全失效 | 用户期望落空 |
