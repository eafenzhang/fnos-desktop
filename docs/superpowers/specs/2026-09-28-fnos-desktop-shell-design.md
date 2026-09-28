# fnOS 桌面壳（Tauri 2）设计文档

- 状态：**已批准**（2026-09-28；复核结论见 §15）
- 日期：2026-09-28
- 分类：架构级（空工作区新建项目）
- 相关：`docs/analysis/fnOS_UI_Mods-注入机制与配置模型分析.md`、`方案C-探针报告.md`、`_spike_tauri/`（throwaway 探针）

---

## 1. 背景与问题定义

用户请求：把 `https://fnos.net/` 打包成 Windows 桌面应用，自动注入 `fnOS_UI_Mods` 修改样式，并在托盘右键菜单里提供「系统设置」。

**对象不匹配（已在头脑风暴阶段澄清）**：`fnos.net` 是飞牛官网 / FN Connect 登录入口（SPA），而 `fnOS_UI_Mods` 的注入目标是 **fnOS NAS 的 WebUI 桌面**。两者必须分开处理：壳负责「打开入口 + 承载 WebUI」，mods 负责「只对 WebUI 生效的注入」。

**技术可行性已由探针实证**（非静态推断）：Tauri 2 的 `initialization_script` 等价于扩展的 `document_start` 且不受页面 CSP 约束；外部 URL 窗口、原生托盘菜单（含勾选项）、本地设置窗与外部窗并存，均已跑通并在 `方案C-探针报告.md` 中留有运行时证据。

---

## 2. 已确认的决策（输入，不可在实现中擅自更改）

| # | 决策 | 值 | 来源 |
|---|---|---|---|
| D1 | 目标页面 | 默认打开 fnos.net 官网；可配置 NAS WebUI 地址；注入只对 WebUI 生效、对官网自动跳过 | 用户确认 |
| D2 | 托盘形态 | 右键原生菜单含「系统设置」→ 独立设置窗，承载 mods 全部配置项 | 用户确认 |
| D3 | 技术栈 | 方案 C：Tauri 2 | 用户确认 |
| D4 | 字体 | 不打包 Sarasa（约 27MB）；**不做字体文件导入** | 用户确认 |
| D5 | 关主窗口行为 | 隐藏到托盘，不退出；退出只能走托盘菜单 | 用户确认 |
| D6 | 分发产物 | **只做 NSIS 安装包**，不做便携版 | 用户确认 |
| D7 | 端到端验收输入 | FN Connect ID `ea121314` | 用户确认 |
| D8 | 分发范围 | **自用**（不对外分发） | 用户确认 |
| D9 | 应用命名 | `productName = "fnOS"` | 用户确认 |

### D4 的精确解释（已确认）

- 安装包**不含**任何字体文件
- **不实现字体文件导入**（上游 popup 里该 UI 本身就被注释掉了；且 13MB 字体转 data URL 会让每次导航的注入载荷增加约 17MB）——该能力**不做**，不是 P2
- 字体定制仅通过上游**实际启用**的三条路径：`fontFamily`（本地已安装字体名）、`fontMonospaceFamily`、`fontUrl`（网络字体 URL）

---

### 优先级标记约定（全文通用）

- **P0** = 必须，进 M1–M3 验收
- **P1** = 可后置，最晚 M2 末补齐（不影响 M3 出包）
- **P2** = 本期不做，除非另行明确要求

---

## 3. 非目标（YAGNI，明确不做）

1. 便携版 / 免安装包（D6 已否决）
2. Windows 以外平台
3. 应用自动更新（上游 `background.js` 的 GitHub 更新检查与徽标整体丢弃）
4. 浏览器扩展的 popup / service worker 等价物（用设置窗替代）
5. 修改上游 JS/CSS 原文（只做包装与配置注入，见 §10 合规）
6. `tabs` / `action` / `i18n` 等扩展专有 API 的等价能力
7. 字体文件导入（D4 已明确不做）

---

## 4. 架构与模块边界

```
D:\fnOS-desktop\
  src-tauri\                     Rust 侧：唯一有状态的层
    src\
      main.rs                    组装：Builder / 窗口 / 生命周期 / 单实例
      config.rs                  配置模型 + 原子持久化 + schemaVersion 迁移 + 归一化
      injector.rs                纯函数：配置 + 资源 → initialization_script 字符串
      tray.rs                    托盘图标、菜单、事件分发
      commands.rs                设置窗 IPC 命令
      paths.rs                   配置/缓存/日志路径解析
    inject\
      shim.js                    chrome.* 兼容层（约 100 行，独立文件以便单测）
      bootstrap.js               装配与兜底（外链失败降级、mod.js 兜底执行）
    assets\fnos-mods\            vendored 上游资源（锁定 commit 483c3e2）
      LICENSE  NOTICE            §10 合规件
      content-script.js  mod.js
      basic_mod.css  windows_titlebar_mod.css  mac_titlebar_mod.css
      classic_launchpad_mod.css  spotlight_launchpad_mod.css
      desktop_icon_mod.css  lockscreen_mod.css
      prefect_icon\              完美图标素材（含 icon-map.json）
    capabilities\default.json    §9 权限
    icons\                       icon.ico / icon.png
    tauri.conf.json
  ui\settings\                   设置窗前端：原生 HTML/CSS/JS，零框架
    index.html  settings.css  settings.js  normalize.js
  docs\
    superpowers\specs\           设计文档（本文件）
    analysis\                    上游代码分析报告
  tests\                         Node 侧契约测试
```

**依赖方向（单向，无环）**

```
config (纯数据) ──► injector (纯函数) ──► main/tray/commands (命令式外壳)
                                            │
                                            └─ IPC 契约 ─► ui/settings (前端)
```

**单元职责与接口**

| 单元 | 做什么 | 怎么用 | 依赖 |
|---|---|---|---|
| `config` | 读写 `config.json`，归一化，迁移 | `Config::load() -> Result<Config>`、`config.save()` | 仅文件系统 |
| `injector` | 把 `Config` + vendored 资源拼成一段 JS | `build_init_script(&Config, &Assets) -> String`（**纯函数，可快照测试**） | `config` 只读 |
| `tray` | 建托盘、发菜单事件 | `tray::install(app, &Config)` | `config`、`commands` |
| `commands` | 设置窗 IPC | `invoke("get_config")` 等，见 §8.3 | `config`、`injector` |
| `ui/settings` | 呈现配置、发起 IPC | 只通过 §8.3 契约与 Rust 通信 | 无（浏览器环境） |
| `inject/shim.js` | 伪装 `chrome.*` 子集 | 被 `injector` 串进注入载荷；也可被 Node 测试直接加载 | 无 |

**关键边界原则**：`injector` 不读文件、不碰 Tauri API、不做 IO —— 它只接受配置与资源字符串，返回脚本字符串。这样注入行为可被单测与快照锁定，也让「上游升级」只影响 `assets/`。

---

## 5. 注入桥（核心技术决策）

### 5.1 上游机制的确切事实（已核实，非推测）

- 上游 `content-script.js`（3337 行）在 `document_start` 运行，先等 `storage` 与签名判定，再注入。
- **7 个 CSS 文件走外链**：`<link id="fnos-ui-mods-*-style" rel=stylesheet href=chrome.runtime.getURL('xxx.css')>`（`cs:2612-2635`、`2449-2455`、`2396-2409`）。勘误（2026-09-28，由 T4 实测）：正文原写「6 个」漏计 `lockscreen_mod.css`，权威集合是 basic / windows_titlebar / mac_titlebar / classic_launchpad / spotlight_launchpad / desktop_icon / lockscreen 共 7 个（`cs:2402` 锁定）。
- **`mod.js` 走外链** `<script src=getURL('mod.js')>`（`cs:2666-2676`）；`mod.js` **零 `chrome.*`**、**不读配置**（自足脚本）。
- **参数化内容才内联**：主题色 10 阶调色板、字体 `@font-face`、自定义 CSS/JS、桌面图标网格变量。
- `safeRuntimeGetURL` 在 `chrome?.runtime?.id` 缺失时返回 `''`，而 `injectStyle` 里 `if (!nextHref) return;` → **缺 shim 就完全注入失败**（`cs:242-252`、`2621-2622`）。
- 判定由上游自己完成：`hasFnOSSignature()` 覆盖 `.fnnas.cn / .fnos.net / .5ddd.com / .fynas.net` 四类域名 + `fnos-token` cookie + `fn-app`/`[data-fn-id]` DOM + `/appcgi/` 资源（`cs:2746-2779`）；`window.top !== window` 直接 return（`cs:2`）。
- 配置白名单 `enabledOrigins` 命中时**跳过 1.5s 签名探测**（`cs:2947-2950`）。

**由此确定路线：搬运 `content-script.js` + 提供 `chrome.*` shim**（即分析报告推荐的 B′）。自写注入器需要重写 15 类算法级逻辑（调色板生成、拼音首字母、启动台卡片扫描、路由补丁、锁屏判定等），不采纳。

### 5.2 shim 规格（必须逐项覆盖，缺一即整体失效）

`inject/shim.js` 注入下面这个 `chrome` 对象：

| 成员 | 语义 | 证据 |
|---|---|---|
| `chrome.runtime.id` | 非空字符串常量（如 `"fnos-desktop-shell"`） | `cs:246` |
| `chrome.runtime.getURL(path)` | 返回**惰性构造的 `data:` URL**；`mod.js` 返回「真实内容 + 执行标记」的 data URL（见 5.4） | `cs:247` |
| `chrome.runtime.getManifest()` | `{ version: <mods 版本> }` | `cs:257` |
| `chrome.storage.sync.get(defaults, cb)` | `cb(合并(defaults, shell.mods))`；**必须支持回调签名**，同时兼容 Promise 形式 | `cs:2890-2944` |
| `chrome.storage.local.get(keys, cb)` | 从 `shell.local` 取子集；支持 `null`（全量）与字符串/数组/对象三种 keys 形式 | `cs:2548,2574,2594` |
| `chrome.storage.onChanged.addListener(fn)` | 由宿主（Rust → `webview.eval`）派发 `{key: {newValue, oldValue}}` | `cs:3039-3332` |
| `chrome.runtime.sendMessage` / `onMessage.addListener` | 走 `window.postMessage` 桥；M2 用于设置窗→页面即时应用 | `cs:2681,2808` |

**契约测试**：`tests/shim.test.mjs` 直接加载 `shim.js`（构造最小 `window`），断言 ① `sync.get` 回调在同步/微任务内必被调用；② 默认值合并语义与 `cs:2890-2944` 一致（含 `enabledOrigins` 等数组键的引用隔离）；③ `local.get(null)` 返回全量；④ 缺键时 `getURL` 不抛异常。

### 5.3 资源外链可达性（三级兜底）

远程页面上，`chrome-extension://` 不可用，`tauri://` / `http://asset.localhost` 会撞混合内容或 CSP。方案：

1. **主路径**：`getURL(path)` 返回 `data:text/css;base64,…`（内容取自注入的原始文本，惰性 base64）
2. **兜底 A**：`bootstrap.js` 在 `DOMContentLoaded` 后自检受管 link 的 `link.sheet === null`（被 CSP/协议拦截）。勘误（2026-09-28，T5 评审）：受管 link 是 **5 个 id**（`CSS_IDS`：basic / titlebar / launchpad / desktop-icon-mod / lockscreen，对应 7 个 CSS 文件），原文误写「6 个」
3. **兜底 B（CSP 免疫）**：改用 `document.adoptedStyleSheets` + `CSSStyleSheet.replaceSync()` 安装同一份 CSS —— 可构造样式表**不受 `style-src` 约束**

**绝不使用**：本地 HTTP 服务（https 页面下的混合内容）、`tauri://` 直链（跨源 + CSP）。

### 5.4 `mod.js` 的时机与 CSP 兜底

上游只在签名命中后才注入 `mod.js`，因此**不能**无条件提前执行（否则官网页面会挂上窗口动画）。

设计：`getURL('mod.js')` 返回的 data URL 内容 = **`mod.js` 原文 + `window.__FNOS_MOD_EXECUTED__ = true;`**。`bootstrap.js` 用 `MutationObserver` 观察 `script#fnos-ui-mods-script` 的出现；若 ~100ms 后标记仍未置位（说明 data: 脚本被 CSP 拦），则由 bootstrap 直接执行 `mod.js` 原文——此时刻与上游语义一致。**元素始终不出现时不得执行**（上游未确认签名就不会注入，此时跑 `mod.js` 会把窗口动画挂到非飞牛页面）。

勘误（2026-09-28，T5 评审）：原文称该兜底「不受 CSP 约束」**不准确**。CSS 兜底走 `adoptedStyleSheets`，确实不受 `style-src` 约束；但 `mod.js` 兜底用的是 `new Function` / 间接 `eval`，**同属 eval 家族、受同一条 `script-src`（缺 `unsafe-eval`）约束**。页面 CSP 严格时两条执行路径会同时失效，此时唯一路径是上游自己的 `data:` `<script src>`（需 `script-src` 允许 `data:`）。bootstrap 因此在双重失败时置 `window.__FNOS_MOD_FALLBACK_FAILED__ = true` 且不置执行标记，使该情形可观测；真实 WebUI 上的实测列入 Task 10 验收。

### 5.5 注入时序

```
Tauri initialization_script（每次顶层文档导航、HTML 解析前）
  ① window.__FNOS_SHELL__ = { meta: { shellVersion, modsCommit, modsVersion }, mods:{...25键}, local:{...}, assets:{ "basic_mod.css": "<原文>", ... }, binaryAssets:{ "prefect_icon/x.png": "<base64>" } }
     （勘误 2026-09-28：原文误写为顶层 version/modsCommit、且漏了 binaryAssets；`meta` 的键名是 camelCase，
      由 Task 6 的 `#[serde(rename_all = "camelCase")]` 保证——否则 shim/bootstrap 会静默回落 '0.0.0'）
  ② shim.js                      安装 chrome.* 兼容层
  ③ bootstrap.js                 惰性 getURL、链接自检与 adoptedStyleSheets 兜底、mod.js 兜底执行
  ④ content-script.js            上游原样代码（自带签名判定与全部注入逻辑）
     **勘误（2026-09-28，T7+8 实测）**：不能裸拼。WebView2 的 document-start 阶段 `document.head` 与
     `document.documentElement` **都还是 null**，上游首次 `appendChild` 会抛错且被静默吞掉 → 整条注入链
     从不执行。`injector.rs` 因此把上游执行**包在**一个「等 `documentElement` 出现（MutationObserver +
     DOMContentLoaded 兜底）再跑」的壳里 —— 上游文件本身仍未修改，但这是**新增的第 5 项包装改动**，
     必须同步写进 `assets/fnos-mods/NOTICE`。
```

- 配置变更：Rust 落盘 → `webview.eval` 派发 `storage.onChanged` → 上游增量分支生效（**不刷新页面**）
- 需要整页重载时（如切换主页地址）：`webview.navigate(url)`

### 5.6 载荷体积（M1 需实测）

约 400 KB/次导航（`content-script.js` 113KB + 7 CSS 约 218KB + `mod.js` 63KB + 配置；勘误：原写「6 CSS」，权威数量见 §5.1）。探针实测 195KB CSS 在 `initAt≈200ms` 完成注入，属可接受范围；M1 记录真实首个内容渲染时间，若退化明显再改为「按需注入 CSS」。

---

## 6. 配置模型

### 6.1 存储

- 路径：`%APPDATA%\com.fnos.desktop\config.json`（Tauri `app_config_dir`）
- 结构（下面是**完整骨架**，只有 `mods` 段按 §6.2 展开 25 个键；为节省篇幅此处用 `"<25 键见 §6.2>"` 表示该段的全部键）：

```json
{
  "schemaVersion": 1,
  "mods":  { "<25 键见 §6.2>": "<各自默认值>" },
  "local": { "customCssCode": "", "customJsCode": "", "loginWallpaperFileName": null },
  "shell": {
    "homeUrl": "https://fnos.net/",
    "nasUrl": "",
    "injectEnabled": true,
    "closeToTray": true,
    "window": { "w": 1200, "h": 820, "x": null, "y": null }
  }
}
```

- 写入：临时文件 + 原子替换；损坏时回退默认并保留 `.bak`
- **`mods` 段与上游 `chrome.storage.sync` 一一对应**，因此用户可以把浏览器扩展里已有的配置直接粘进来（互通性，非目标但零成本获得）

### 6.2 `mods` 段（上游 25 键，默认值必须与 `cs:2890-2917` / `popup.js:1785-1811` 一致）

| 键 | 类型 | 默认 | 取值/约束 |
|---|---|---|---|
| `enabledOrigins` | string[] | `[]` | origin 列表；命中即注入且跳过探测 |
| `autoEnableSuspectedFnOS` | bool | `true` | |
| `basePresetEnabled` | bool | `true` | 关掉**只**摘标题栏+启动台，`basic_mod.css` 与 `mod.js` 照旧 |
| `windowAnimationBlurEnabled` | bool | `true` | |
| `titlebarStyle` | string | `'windows'` | `windows`\|`mac` |
| `launchpadStyle` | string | `'classic'` | `classic`\|`spotlight` |
| `desktopIconLayoutEnabled` | bool | `true` | |
| `desktopIconLayoutMode` | string | `'adaptive'` | `adaptive`\|`fixed` |
| `desktopIconPerColumn` | number | `8` | 4–16 夹取 |
| `desktopIconPerColumnEnabled` | bool\|null | `null` | 遗留键，只读回退 |
| `launchpadIconScaleEnabled` | bool | `false` | |
| `launchpadIconScaleSelectedKeys` | string[] | `[]` | |
| `launchpadIconMaskOnlyKeys` | string[] | `[]` | |
| `launchpadIconRedrawKeys` | string[] | `[]` | |
| `launchpadIconRedrawMap` | Record | `{}` | 值须匹配 `^prefect_icon/[a-z0-9-]+\.png$` |
| `brandColor` | string | `'#0066ff'` | `#rrggbb`，**明度夹 30%–70%** |
| `fontOverrideEnabled` | bool | `false` | |
| `fontFamily` | string | `''` | |
| `fontMonospaceFamily` | string | `''` | |
| `fontWeight` | string | `''` | `450`\|`normal`\|`600` |
| `fontFeatureSettings` | string | `''` | |
| `fontFaceName` | string | `'FnOSCustomFont'` | 上游输入框已注释 |
| `fontUrl` | string | `''` | |
| `customCodeEnabled` | bool | `false` | |
| `lockscreenDefaultUsername` | string | `''` | ≤80 字符 |

### 6.3 `local` 段（上游大对象）

`customCssCode`、`customJsCode`、`loginWallpaperFileName`（登录壁纸走 shell 自有文件存储，见 §8.2）。`customFontDataUrl` / `CustomFontFileName` / `customFontFormat` **不实现**（D4 已明确不做字体文件导入）。

### 6.4 `shell` 段（本应用自有）

| 键 | 含义 | 备注 |
|---|---|---|
| `homeUrl` | 主窗口启动地址 | 默认 `https://fnos.net/` |
| `nasUrl` | NAS WebUI 地址 | 保存时**自动把其 origin 并入 `mods.enabledOrigins`**，从而跳过 1.5s 探测 |
| `injectEnabled` | 总开关（托盘勾选项） | 关闭时不注入 `content-script.js`，仅保留空壳 |
| `closeToTray` | 关窗隐藏 | D5 默认 `true` |
| `window` | 窗口几何 | 关闭时保存、启动时恢复 |

### 6.5 必须复刻的归一化语义

上游 popup 读回配置后会**归一化并回写**（`popup.js:1855-1932`）。若不复刻，会出现「设置窗显示 A、页面实际按 B 生效」：

- `brandColor`：非法值 → `#0066ff`；**明度夹到 30%–70%**
- `desktopIconPerColumn`：夹到 4–16，非数字 → 8
- `desktopIconLayoutMode`：非 `fixed` → `adaptive`
- `titlebarStyle` / `launchpadStyle`：非法值回落默认（`cs:223-229`）
- `fontWeight`：非 `450/normal/600` → 空
- `lockscreenDefaultUsername`：截断到 80 字符

归一化函数放 `ui/settings/normalize.js`，并由 Rust 侧 `config.rs` 再校验一次（双层，防手改 config.json）。

### 6.6 热更新路径

1. 设置窗 `invoke("set_config", {patch})`
2. Rust 归一化 → 落盘 → `webview.eval` 派发 `storage.onChanged` 事件
3. 上游增量分支（`cs:3039-3332`）就地生效。**勘误（2026-09-28，T7+8 实测）**：原文「`injectEnabled` 翻转或 `homeUrl` 变更时才整页重载」**不成立** —— 已注册的 `initialization_script` 在窗口存活期间无法替换，重载只会重跑旧载荷。正确做法是**销毁并按新载荷重建 main 窗口**（置 `recreating` 标志绕过 `CloseRequested` 的 `prevent_close()`+hide，待 `Destroyed` 事件中重建，因 tauri 只在此刻释放窗口 label 注册），随后 `sync_menus`。

---

## 7. 窗口与托盘

- **主窗口**：label `main`，`WebviewUrl::External(homeUrl)`；标题跟随页面；关闭 → 隐藏（D5）
- **设置窗**：label `settings`，`WebviewUrl::App("settings.html")`；单例（已存在则 `show + set_focus`）；关闭 → 真正销毁
- **托盘菜单**：

```
✓ 注入 mods                      → 翻转 shell.injectEnabled，即时重注入
  打开 NAS                       → 有 nasUrl 则主窗口导航过去；未配置则置灰
  显示 / 隐藏主窗口
  系统设置                       → 打开设置窗
  ──────────────
  退出
```

- 托盘图标：内存生成的 32×32 RGBA（探针已验证 `Image::new_owned`），正式版换成 `icons/` 里的设计图标
- 单实例：重复启动只唤起已有窗口

---

## 8. 设置窗

### 8.1 结构

左侧分组导航 + 右侧面板，纯原生 HTML/CSS/JS（零框架，避免引入构建链）。风格对齐飞牛系统设置（深色、圆角、分组卡片）。

### 8.2 分组与项

| 分组 | 项 | 绑定 | 控件 |
|---|---|---|---|
| **站点** | 为当前站点注入（白名单增删） | `mods.enabledOrigins` | 列表 + 添加/删除 |
| | NAS WebUI 地址 | `shell.nasUrl`（联动白名单） | 输入框 + 「用当前页填充」 |
| | 自动对疑似飞牛站点启用 | `mods.autoEnableSuspectedFnOS` | 开关 |
| **基础** | 基础美化预设 | `mods.basePresetEnabled` | 开关（附注：关掉只摘标题栏+启动台） |
| | 窗口动画模糊 | `mods.windowAnimationBlurEnabled` | 开关 |
| **主题** | 主题色 | `mods.brandColor` | 取色器 + 重置（显示夹取后的值） |
| **标题栏** | Windows / macOS | `mods.titlebarStyle` | 单选 |
| **启动台** | 经典 / Spotlight | `mods.launchpadStyle` | 单选 |
| **桌面图标** | 优化开关 | `mods.desktopIconLayoutEnabled` | 开关 |
| | 布局 | `mods.desktopIconLayoutMode` | 下拉 |
| | 每列数量 | `mods.desktopIconPerColumn` | 数字 4–16 |
| **完美图标** | 总开关 | `mods.launchpadIconScaleEnabled` | 开关 |
| | 应用逐项三态 | `launchpadIconScaleSelectedKeys` / `…MaskOnlyKeys` / `…RedrawKeys` / `…RedrawMap` | 动态列表（**P1，可后置到 M2 末**） |
| **字体** | 开关 / 正文字体 / 等宽字体 / 网络字体 URL / 字重 / OpenType | `mods.font*` | 开关 + 输入框 |
| **登录页** | 默认用户名 | `mods.lockscreenDefaultUsername` | 输入框（≤80） |
| | 登录壁纸 | shell 文件存储 | 选择文件 + 恢复默认（**P1**） |
| **自定义代码** | 开关 / CSS / JS | `mods.customCodeEnabled` + `local.customCssCode`/`customJsCode` | 开关 + 两个 textarea |
| **关于** | 版本 / mods commit / WebView2 版本 / 配置路径 | 只读 | 文本 + 打开目录按钮 + 上游许可与免责声明 |

### 8.3 IPC 契约

| 命令 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `get_config` | — | `{mods, local, shell, meta}` | `meta` 含版本、mods commit、WebView2 版本、配置路径 |
| `set_config` | `{patch: Partial<Config>}` | `{config, needsReload}` | 归一化 + 落盘 + 派发 `onChanged`；`needsReload=true` 仅在 `injectEnabled` 或 `homeUrl` 变更时出现，此时调用方随后调用 `reload_main`（该命令执行**窗口重建**，见 §6.6 勘误）。勘误：原文返回体还含 `applied`，无任何消费者，已删除 |
| `reload_main` | `{url?}` | `()` | 重载/导航主窗口 |
| `open_config_dir` | — | `()` | 打开配置目录 |
| `reset_config` | `{scope}` | `{config}` | 重置为默认（含确认） |

### 8.4 视觉要求

- 与飞牛系统设置一致的深色卡面、分组卡片、开关与单选样式
- 所有项的当前值必须显示**归一化之后**的值（避免 §6.5 的「显示不一致」）
- 未连接/未注入时在顶部给出状态条（复用上游判定结果，见 §12.3）

---

## 9. 安全

已用 tauri 2.12 源码核实（非推断）：

- 外部页面里存在 `__TAURI_INTERNALS__`（探针实测），但**命令授权由 capability 决定**：`RuntimeAuthority::resolve_access`（`tauri-2.12.0/src/ipc/authority.rs:462`）只放行 capability 中列出的命令；应用命令会被 `tauri-build` 自动生成 `allow-<cmd>` / `deny-<cmd>` 权限（`tauri-utils-2.10.0/src/acl/build.rs:289-317`）。

`capabilities/default.json`：

```json
{
  "identifier": "settings-only",
  "description": "只授权设置窗调用应用命令",
  "windows": ["settings"],
  "permissions": [
    "core:default",
    "allow-get-config", "allow-set-config", "allow-reload-main",
    "allow-open-config-dir", "allow-reset-config"
  ]
}
```

- **不配置 `remote.urls`** → 远程页面即使有 IPC 桥也调不到任何命令
- **纵深防御**：增加一个 `remote: { urls: ["*"] }` 的 capability，只列 `deny-*`
- 注入 JS 与页面同上下文（无法隔离）→ 上游代码视为受信依赖，**锁定 commit + 记录全部 vendored 文件 SHA-256**（`NOTICE` 中列出）
- 验收项：在外部页面控制台调用 `window.__TAURI_INTERNALS__.invoke('set_config', …)` 必须被拒

---

## 10. 合规与品牌

上游许可为 `FnOS UI Mods Non-Commercial License 1.0`（`LICENSE:1-29`）：

- **禁止商业用途**（售卖、打包进付费产品/服务、基于它的付费托管/集成/定制/咨询/支持等）
- 分发**必须保留版权声明与许可全文**；**修改版必须明确说明做过改动**
- `README:91` 声明该项目为个人学习作品、**非飞牛官方内容、未获官方授权或认可**

因此本设计落实：

1. `src-tauri/assets/fnos-mods/` 内保留上游 `LICENSE` 与新增 `NOTICE`（来源仓库 + commit `483c3e2` + 各文件 SHA-256 + **我们的包装性改动清单**：chrome shim、getURL 改写为 data URL、mod.js 兜底执行；上游 JS/CSS 原文未改）
2. 设置窗「关于」页放置：上游许可全文入口、非官方与非商业声明
3. **应用命名（D9）**：`productName = "fnOS"`。`fnOS` 是飞牛的产品名，而上游声明「未获飞牛官方授权或认可」（`README:91`）。因 **D8 = 自用、不对外分发**，此命名的现实风险很低，按用户决定执行；「关于」页仍保留非官方声明。**若将来要对外分享，需重新评估命名**
4. **D8 = 自用**（不对外分发）。因此 M3 只需产出可用于本机的 NSIS 安装包，不涉及对外分发合规流程；上游许可的商业禁止条款在本项目下不构成阻碍

---

## 11. 打包

- 产物：**仅 NSIS 安装包**（D6），`bundle.targets = ["nsis"]`
- 目标体积 ~10MB（不含字体）
- 必须项：`icons/icon.ico`（缺失会让 `tauri-build` 直接失败）、`tauri.conf.json` 的 `identifier`/`productName`/`version`
- **构建环境前置（探针踩过的真实坑，写进 `README.md`）**：
  - 本机默认工具链 `x86_64-pc-windows-gnu` 且 rustup 自包含目录缺 `as.exe` → 必须 `$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH`，否则 `dlltool` 报 `CreateProcess`
  - 无 MSVC `link.exe` / Windows SDK
- 可选优化：`--release` + LTO，`webview_install_mode` 保持默认（在线引导安装 WebView2 运行时）

---

## 12. 测试与验收

### 12.1 自动化测试

| 层 | 内容 |
|---|---|
| Rust 单测 | 配置序列化/反序列化往返；`schemaVersion` 迁移；归一化边界（明度夹取、列数夹取、枚举回落、用户名截断） |
| Rust 快照 | `injector::build_init_script` 对固定配置的输出快照（防止载荷结构被无意改动） |
| Node 契约 | `tests/shim.test.mjs` 验证 §5.2 的 shim 语义（回调必被调用、默认值合并、`local.get(null)`、异常安全） |
| 静态检查 | `cargo clippy`、`node --check` 校验 `ui/settings/*.js` 与 `inject/*.js` |

### 12.2 端到端验收清单（M1–M3 完成时逐条执行，需 D7 的 FN ID `ea121314`）

1. 启动 → 主窗口打开 `https://fnos.net/`，**页面无任何 mods 注入痕迹**（官网被正确跳过）
2. 托盘右键菜单四项存在，勾选项渲染为 ✓
3. 设置窗填 NAS WebUI 地址（或走 FN ID 登录后取当前页）→ 保存后 `enabledOrigins` 含该 origin
4. 在 NAS WebUI 页面：`basic_mod.css` 生效（外观变化）、`mod.js` 行为生效（窗口动画/squircle）
5. 切换 `titlebarStyle` / `launchpadStyle` / 主题色 → **不刷新页面即时生效**
6. 关闭主窗口 → 进程仍在托盘；`显示/隐藏` 可恢复
7. 退出 → 进程结束，配置持久化
8. 安全项：外部页面 console 调用 `invoke('set_config')` 被拒
9. 安装包：NSIS 安装 → 桌面/开始菜单入口 → 卸载干净

### 12.3 失败模式与恢复

| 现象 | 处理 |
|---|---|
| 页面未通过签名判定（上游不注入） | 设置窗顶部状态条提示「未检测到 fnOS WebUI」，并提供「把当前页加入白名单」一键操作。**阶段性方案**：M1–M2 由 Rust 侧 `on_page_load` 判定并在设置窗打开时查询；双向上报通道在 P1（完美图标/登录壁纸）一并落地 |
| 外链 CSS 被 CSP 拦截 | §5.3 兜底 B 自动接管（`adoptedStyleSheets`） |
| `mod.js` data URL 被拦 | §5.4 兜底执行 |
| 主窗口加载失败/离线 | `on_page_load` 失败时显示内置错误页 + 重试按钮 |
| WebView2 运行时缺失 | 启动时检测，缺失则提示并给微软运行时下载入口（不静默失败） |
| `config.json` 损坏 | 回退默认 + 保留 `.bak` + 设置窗提示 |

---

## 13. 里程碑

| 阶段 | 交付 | 验收 |
|---|---|---|
| **M1 骨架与注入桥** | 项目骨架、`config`、`tray`、双窗、`injector` + shim + bootstrap | §12.2 的 1–4、8 |
| **M2 设置窗** | §8.2 全部项（完美图标/登录壁纸为 P1）、§8.3 IPC、归一化 | §12.2 的 5、6、7 |
| **M3 打包** | NSIS + 图标 + 版本信息 + 合规件 | §12.2 的 9 |
| **M4 打磨** | §12.3 全部失败模式、载荷耗时测量与优化、WebView2 版本检测 | 全部 |

---

## 14. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 上游 `content-script.js` 与真实 WebUI DOM 强耦合，固件升级后可能失效 | 注入失效 | 锁定 commit；失效时设置窗状态条可见；上游升级只需替换 `assets/` |
| data: URL 在严格 CSP 页面被拦 | 样式/行为不全 | §5.3/§5.4 双兜底 |
| 每次导航注入 ~400KB | 首屏变慢 | M1 实测；必要时 CSS 按需注入 |
| 上游非商业许可 | 不能商业分发 | §10；D8 假设已限定 |
| rustup gnu 工具链缺 `as.exe` | 构建失败 | §11 PATH 前置写入 README |
| `corner-shape` 需 Chromium 139+ | 形状效果退化 | 启动检测 WebView2 版本并在「关于」页显示；低版本走普通圆角 |

---

## 15. 复核结论（2026-09-28，已全部确认）

| # | 事项 | 结论 |
|---|---|---|
| 1 | D8 分发范围 | **自用**（不对外分发）→ M3 只产出本机可用的 NSIS 安装包 |
| 2 | D4 字体 | **不做字体文件导入**；只用 `fontFamily` / `fontMonospaceFamily` / `fontUrl` |
| 3 | D9 应用命名 | **`fnOS`**（非官方声明仍保留在「关于」页；将来若分享需重评命名） |
| 4 | P1 项 | **同意后置**：完美图标逐项配置、登录壁纸导入后置到 M2 末 |

**spec 已冻结。** 后续实现以本文件为准；如需变更，先改本文件再改代码。
