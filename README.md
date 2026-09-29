# fnOS 桌面壳（fnOS Desktop Shell）

把 fnOS NAS 的 WebUI 装进一个 Windows 桌面窗口，常驻托盘，并向 WebUI 页面注入
[fnOS_UI_Mods](https://github.com/aurysian-yan/fnOS_UI_Mods) 的 CSS/JS（上游资源**原样 vendored、未作修改**）。

> **非官方**：本项目是第三方桌面壳，**不是飞牛（fnOS）官方产品**，与飞牛官方无任何关联，也未获其授权或认可。
> 随附的界面修改资源遵循上游 **FnOS UI Mods Non-Commercial License 1.0**，**仅供非商业个人使用**，禁止任何商业用途。
> 本项目按「自用、不对外分发」定位（设计文档 D8）；若要对外分享，命名（`fnOS`）与合规流程需重新评估。

## 它是什么 / 不是什么

- **是**：一个 Tauri 2 的 Windows 桌面壳。主窗口打开 `https://fnos.net/`（或你配置的 NAS 地址）；命中 fnOS WebUI 的页面才会被注入；托盘常驻；设置窗（界面就是上游自己的 popup UI，见下）改配置免刷新生效。
- **不是**：官方客户端；不是 fnOS 系统的一部分；不含任何 NAS 服务端代码；**不含字体文件**（见「已知限制」）。

## 环境前置（Windows）

| 前置 | 说明 |
|---|---|
| Rust | 1.90+（实测 1.96.0），默认工具链 **`x86_64-pc-windows-gnu`** |
| MSYS2 mingw64 | **必须**放在 `PATH` 最前，见下 |
| Node.js | 18+（实测 26.1.0），只用于前端契约测试 |
| cargo-tauri | 2.x（实测 2.12.0） |
| `src-tauri/icons/icon.ico` | 必须存在，缺失会让 `tauri-build` 直接失败 |
| WebView2 运行时 | Win11 一般自带。安装包默认 `webviewInstallMode`（见下）会在安装时联网引导安装 |

**MSYS2 的坑（本项目真踩过）**：rustup 的 gnu 自包含目录里缺 `as.exe`，不把 mingw64 放进 `PATH`
就会在链接阶段报 `dlltool ... CreateProcess`。**每一条 cargo 命令都先执行**：

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
```

## 构建

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd src-tauri
cargo build                 # debug：保留控制台，[fnos] 运行日志直接可见
.\target\debug\fnos-desktop.exe
```

Release（无控制台窗口，见「已知限制」）：

```powershell
cargo build --release       # 产物 src-tauri\target\release\fnos-desktop.exe
```

**改了 `ui/` 之后必须强制重编译该 crate。** `frontendDist`（`../ui/settings`）的资产由
`generate_context!` 在**编译期**嵌进二进制，而 `tauri-codegen` 不为 dist 目录发
`rerun-if-changed`；只改前端时 `cargo build` 可能直接报 `Finished` 而跑的还是旧界面。任选一种：

```powershell
cargo clean -p fnos-desktop ; cargo build
# 或
(Get-Item src\main.rs).LastWriteTime = Get-Date ; cargo build
```

## 测试

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd src-tauri
cargo fmt --check
cargo test                  # Rust 单测（配置往返/迁移、归一化边界、注入载荷快照、IPC 契约）
cargo clippy --all-targets  # 可选；当前会报 2 条**既有**风格告警（与本轮打包改动无关，见报告）
cd ..
node --test "tests/**/*.mjs"     # shim / bootstrap / 设置窗 / 状态条 / 错误页契约测试
```

> `node --test tests/`（目录位置参数）在 **Node 26** 下不可用（报 `Cannot find module ...\tests`）。
> 权威命令是上面的 **glob 形式**，等价于 `npm test`。

## 打包（NSIS 安装包）

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd src-tauri
cargo tauri build --bundles nsis
```

产物：`src-tauri\target\release\bundle\nsis\fnOS_0.1.0_x64-setup.exe`。

- `bundle.targets = ["nsis"]`（只要安装包，不出 MSI）。
- **`webviewInstallMode` 用的是默认值 `downloadBootstrapper`（silent）**：安装时如果机器上没有
  WebView2 运行时，安装包会**联网下载并静默安装**微软的引导程序。含义：**离线机器上装完可能仍缺运行时**，
  应用启动会失败；需要离线分发就改成 `embedBootstrapper` 或 `offlineInstaller`
  （体积分别 +~1.8MB / +~127MB）。
- `LICENSE` / `NOTICE` 通过 `bundle.resources` 随包分发，安装后落在安装目录的 `fnos-mods\` 下（见「合规」）。

## 安装 / 卸载

- **安装**：双击安装包（或 `"fnOS_0.1.0_x64-setup.exe" /S` 静默）。`installMode = currentUser`，
  **per-user、不需要管理员**（NSIS 里是 `RequestExecutionLevel user`），装到 `%LOCALAPPDATA%\fnOS`，
  并创建开始菜单与桌面「fnOS」快捷方式。装出来的目录（实测）：

  ```
  %LOCALAPPDATA%\fnOS\
  ├─ fnos-desktop.exe      # 主程序（用的是 cargo 的二进制名，不是 productName）
  ├─ uninstall.exe
  ├─ WebView2Loader.dll
  └─ fnos-mods\
     ├─ LICENSE            # 上游许可全文（随包分发，见「合规」）
     └─ NOTICE
  ```

- **卸载**：Windows「设置 → 应用」，或静默
  `"%LOCALAPPDATA%\fnOS\uninstall.exe" /S`。**必定**删除：程序目录（含 `fnos-mods\` 两个合规件）、
  开始菜单与桌面快捷方式、卸载注册项（`HKCU\...\Uninstall\fnOS`）。
- **卸载不会删掉的东西**（实测，`installer.nsi:869-887`）：NSIS 模板把「应用数据」清理放在
  **卸载向导的「删除应用数据」勾选框**后面，而 `/S` 静默卸载时那个勾选状态恒为 0，所以以下三项
  在静默卸载后**仍在**——
  - `%APPDATA%\com.fnos.desktop\`（你的配置，本来就不该被卸载删掉）
  - `%LOCALAPPDATA%\com.fnos.desktop\EBWebView\`（WebView2 缓存）
  - `HKCU\Software\fnOS\fnOS`（安装时写下的安装位置，GUI 卸载不勾选「删除应用数据」时同样会留下）

  要清干净就手动删：`Remove-Item "$env:LOCALAPPDATA\com.fnos.desktop" -Recurse -Force` 与
  `Remove-Item "HKCU:\Software\fnOS" -Recurse -Force`。

## 配置与数据

| 项 | 位置 |
|---|---|
| 配置 | `%APPDATA%\com.fnos.desktop\config.json` |
| 配置损坏时的备份 | `%APPDATA%\com.fnos.desktop\config.json.bak` |
| 设置窗本地状态（T14b，上游 `chrome.storage.local` 里本壳配置模型没有对应字段的那部分） | `%APPDATA%\com.fnos.desktop\local-store.json`（损坏时改名为 `local-store.json.bak`） |
| 登录壁纸落盘文件（T13b；名字带内容指纹） | `%APPDATA%\com.fnos.desktop\<stem>-<指纹>.<ext>` |
| 配置目录覆盖 | 环境变量 `FNOS_DESKTOP_CONFIG_DIR`（用于隔离测试/便携） |
| WebView2 用户数据（缓存；见「安装 / 卸载」：静默卸载不删） | `%LOCALAPPDATA%\com.fnos.desktop\EBWebView` |

配置路径由 `src-tauri/src/paths.rs` 的 `config_dir()` 决定（`%APPDATA%\<identifier>`），
`config.rs::config_path()` 在其下取 `config.json`；设置窗「关于」页显示的 `配置文件` 就是它。

## 设置窗：托管上游自己的设置界面（Task 14b）

设置窗的**主界面就是上游的 `popup.html` + `popup.js`**（不是本壳仿写的一套分组表单）：

- 两个文件经 `tools/vendor-mods.ps1` 逐字节 vendor 进 `src-tauri/assets/fnos-mods/`（SHA-256 记在
  NOTICE），再逐字节复制到 `ui/settings/`（设置窗资产根 = `tauri.conf.json` 的 `frontendDist`）。
  **上游 `popup.js` 一个字节都没改**；`ui/settings/popup.html` 与 vendored 副本的唯一差别是插入
  一行 `<script src="./chrome-shim.js"></script>`（必须在 popup.js 之前定义 `window.chrome`）。
  这一处适配在 NOTICE 的包装性改动第 7 条里写明。`popup.ts`/`popup.html` 需要的 PNG
  （`prefect_icon/*.png` 14 张 + `icons/*.png` 4 张）与 `icon-map.json` 也在同一目录下，
  因为 `chrome.runtime.getURL()` 必须**同步**返回一个真实可取的 URL（上游拿它喂 `fetch` 与 `<img src>`）。
- `ui/settings/chrome-shim.js` 是**本壳自己的** `chrome.*` 兼容层（上行 `chrome.*` 面已逐条枚举，
  没有一条落进「静默忽略」）：

  | 上游调用 | 本壳实现 |
  |---|---|
  | `storage.sync.get/set` | `get_config` / `set_config {mods}`（回包是唯一权威值；`needsReload` 为真时跟随 `reload_main`） |
  | `storage.local.get/set/remove` | 四类键四种归宿：`customCssCode`/`customJsCode` → `local` 段；`loginWallpaperDataUrl`/`…FileName` → 宿主 `import_wallpaper` 落盘 + `local.loginWallpaperFileName`；`updateCheckState` → 上面的 `local-store.json`；`customFont*` → **如实拒绝**（见「已知限制」的字体一条） |
  | `tabs.query` | `get_page_state`（主窗口那一页，不是设置窗自己） |
  | `tabs.sendMessage` | 三个 type 逐一映射：`FNOS_CHECK` → `get_page_state`/上报的判据；`FNOS_GET_LAUNCHPAD_APP_ITEMS`（含上游别称 `…TITLES`）→ 应用项槽位，槽位空时请宿主 `request_app_items` 让页面当场再问一次并在 6s 内有界轮询；`FNOS_APPLY` → `set_config`（宿主当场 `eval` 给活页面） |
  | `runtime.getURL` | **同步**返回设置窗资产根下的真实 URL（只放行相对路径；`..`/绝对 URL 一律空串，上游对空串的语义正是「资源不存在」） |
  | `runtime.getManifest` | 同步读父窗口事先写好的配置快照（`meta.modsVersion`，与注入载荷同源） |
  | `tabs.create` / `action.*` | `open_url`（系统默认浏览器）/ 空操作 |
  | 更新检查（`fetch` GitHub API） | **不发任何网络请求**：返回一份合成应答，sha 就是本壳内置的 vendored commit，于是上游自己算出「已记录当前最新提交 / 暂无更新」 |

- 本壳只额外提供三块自己的区域（**不覆盖上游任何一个节点**）：顶部的**状态条**（Task 11/13a 的
  诚实判定）、**外壳开关**（`shell.injectEnabled` 注入总开关与 `shell.nasUrl`——上游是「扩展」，
  它假设注入总开永远是开、也没有 NAS 地址这个概念，而这两个键在本壳里有真实语义；托盘精简后
  注入总开关只剩这一个入口）、以及**关于/合规**页（版本 / mods commit / mods 版本 / WebView2
  版本 / 配置路径 / 随包 LICENSE+NOTICE 路径 / 非官方与非商业声明 / 上游链接）。
- 上游 UI 装在一个 372×522 的 iframe 里（那正是上游 popup 自己写死的 body 尺寸），因此它的样式
  与本壳的区域互不影响。帧在**配置快照就位之后**才创建——上游在脚本开头就同步读版本号。
- 上游 UI 里没有、也不该有的东西，本壳不会假装有：字体文件导入被如实拒绝（见「已知限制」），
  更新检查不会联网。

## 托盘菜单（4 个动作项 + 1 条分隔线）

1. **显示窗口**（显示 + 前置主窗口；最小化的先还原。**不是**显示/隐藏开关）
2. **重新加载**（按当前配置重建主窗口，从错误页恢复的正路之一）
3. **系统设置**（打开设置窗）
4. —— 分隔线 ——
5. **退出**（保存窗口几何后结束进程）

> **T14a 变更**：移除了「注入 mods」勾选项（该总开关只在设置窗，配置项
> `shell.injectEnabled` 与改动后重建主窗口的行为完全不变）与「打开 NAS」项。
> 菜单不再有勾选态或置灰态，因此宿主侧「把配置推给菜单」的同步通路
> （原 `tray::sync_menus`）已整体删除——不存在「菜单状态与 `config.json` 不一致」这一类缺陷。
>
> Windows 11 默认把托盘图标收进「显示隐藏的图标」的折叠区里；验收脚本走的就是
> UIAutomation 展开折叠区 → 键盘导航（`↓`×N + `Enter`，每步回读 `accFocus`）这条通路。

## 应用与托盘图标

托盘图标与 `bundle.icon` 用的都是上游 fnOS 品牌图标（`.ref/fnOS_UI_Mods/icons/icon*.png`，
渐变圆角方块 + 白色牛头），**不是**手绘占位图：

| 用途 | 文件 | 来源 |
|---|---|---|
| 托盘通知区域图标（32×32） | `src-tauri/assets/fnos-mods/icons/icon32.png` | `include_bytes!` 进 `tray.rs` |
| Windows 可执行文件图标 | `src-tauri/icons/icon.ico`（内含 16/24/32/48/64/256 六个尺寸） | `cargo tauri icon` 由 128 px 源生成 |
| 安装包 / 其它平台兜底 | `src-tauri/icons/icon.png`（512×512） | 同上 |

四枚 PNG 全部经 `tools/vendor-mods.ps1` 逐字节 vendor 进 `src-tauri/assets/fnos-mods/icons/`，
SHA-256 记在 `assets/fnos-mods/NOTICE`（连同包装改动第 6 条）。重新生成：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\vendor-mods.ps1
cargo tauri icon .ref\fnOS_UI_Mods\icons\icon128.png
```

`cargo tauri icon` 会额外产出 Android / iOS / Windows Store 资产与本项目用不到的尺寸
（`icon.icns`、`Square*Logo.png` 等）。本项目只打 NSIS、只引用 `icon.ico` 与 `icon.png`，
所以那批产物在生成后即删除（另见 `tools/vendor-mods.ps1` 的 NOTICE 第 6 条）。


## 失败模式与恢复（实际行为）

| 现象 | 行为 |
|---|---|
| 页面不是 fnOS WebUI | 上游签名判定不注入；设置窗状态条提示「未检测到 fnOS WebUI」并提供**一键加入白名单** |
| 打开的是 `https://fnos.net/` 官网根域 | 按上游正则（要求前导点）**永不注入**，状态条如实说明是「fnOS 官网」 |
| 外链 CSS 被 CSP 拦截 | 兜底 B：`adoptedStyleSheets` 接管 |
| `mod.js` 的 data URL 被拦 | 兜底执行未修改的原始脚本 |
| 主窗口加载失败 / 离线 | 切到内置错误页 `ui/settings/error.html`（显示失败地址、原因、退避计划）；**错误页不注册任何 mods 初始化脚本**。重试有三个入口：设置窗状态条的「重试」、托盘「重新加载」、错误页自动退避重试（**15s → 30s → 60s → 120s**，第 5 次失败后停止自动重试） |
| 能从网络连上但页面永不完成 | 20 秒看门狗兜底判失败 |
| WebView2 版本 < 139 | 「关于」页提示 `corner-shape`（squircle 圆角）效果退化为普通圆角；取不到版本时**不提示**、也不谎报 |
| `config.json` 损坏 | 回退默认值 + 原名改名保留为 `config.json.bak`，设置窗状态条**回显**这次回退 |
| WebView2 运行时缺失 | 由安装包的 `webviewInstallMode`（默认联网引导安装）负责补上；运行起来后「关于」页显示实际运行时版本，取不到时显示「未知（未取到运行时版本）」 |

## 合规（spec §10）

- 上游许可是 **FnOS UI Mods Non-Commercial License 1.0**：禁止商业用途；分发**必须保留版权声明与许可全文**；
  **修改版必须明确说明做过改动**。
- 因此：`src-tauri/assets/fnos-mods/` 内保留上游 `LICENSE` 原文，并新增 `NOTICE`
  （来源仓库 + 锁定 commit `483c3e2` + 各文件 SHA-256 + **本壳的包装性改动清单**：chrome shim、
  `getURL` 改写为 data URL、`mod.js` 兜底执行、配置改由宿主提供、`content-script.js` 的包装执行、
  **T14a 起把上游品牌图标 `icons/icon{16,32,48,128}.png` 用作本应用自身图标**（`cargo tauri icon`
  从 128 px 源生成的多尺寸 `.ico`/`.png` 里，**大于 128 px 的尺寸都是那一份 128 px 源的放大**，
  上游只提供到 128 px）、**T14b 起设置窗托管上游 `popup.html` + `popup.js`**（前者只多一行
  chrome.* 兼容层标签，后者逐字节原样；`ui/settings/` 下另有一份逐字节相同的资产副本）。
- 这两个文件通过 `bundle.resources` **随安装包分发**到安装目录的 `fnos-mods\`
  （`LICENSE` → `<安装目录>\fnos-mods\LICENSE`，`NOTICE` → `<安装目录>\fnos-mods\NOTICE`，卸载时一并删除）。
  普通 `cargo build` 也会在 `src-tauri\target\<profile>\fnos-mods\` 放一份（`tauri-build` 的 `copy_resources`），
  所以开发版的「关于」页会显示 `target\debug|release\...` 下的路径，那是**对的**，文件确实在那儿。
- 设置窗「关于」页显示这两个文件的**安装后真实路径**（由 `get_config` 的 `meta.licensePath` /
  `meta.noticePath` 从资源目录解析而来，不是源码树路径；`resource_dir()` 在 Windows 上给的是
  `current_exe().canonicalize()` 的 `\\?\C:\...` 逐字路径，页面展示前会剥掉那个前缀，只对盘符形式生效），
  并给出非官方声明与上游仓库链接（链接交给系统默认浏览器打开，不在窗内导航）。

## 已知限制

- **字体不随包，也不做字体文件导入**（设计文档 D4）：仓库和安装包里都**没有**任何字体文件。
  设置窗主界面是上游 popup，它确实带一个「导入字体文件」控件——但那个控件在**上游自己的
  `popup.html` 里就是注释掉的**（`popup.html:1180-1188`），所以界面上根本没有它；而 `popup.js`
  里那条代码路径依然存在（`popup.js:2161-2196`，`#fontFile` 为 null 时整段跳过）。本壳的
  `chrome-shim.js` 仍然把写入 `customFontDataUrl` / `customFontFileName` / `customFontFormat`
  的请求**如实拒绝**（可见的「外壳说明」框里写明原因，并抛错让上游走进它自己的失败分支），
  而不是假装成功：将来上游把那一段注释解开，界面也不会说谎。可用的字段只有
  `fontOverrideEnabled` / `fontFamily` / `fontMonospaceFamily` / `fontUrl` / `fontWeight` /
  `fontFeatureSettings` / `fontFaceName`——字体得是你本机已装的，或由 `fontUrl` 指向网络字体。
- **更新检查不联网**：上游 popup 的「检查」按钮在本壳里不会发起任何请求（`fetch` 被接管），
  它报告的是「本壳内置的 vendored commit」。升级由本仓发版决定，不是让用户去比对 GitHub HEAD。
- **载荷约 423KB / 每次导航**（7 个 CSS + `mod.js` + `content-script.js`），且每次导航都会重新注入解析；
  实测受管元素与 CSS 变量在 DOMContentLoaded 之前生效（约 180ms）。数字与口径见验收记录 §4。
- **设置窗资产重复一份**：上游的 `popup.html` / `popup.js` / 19 张 PNG + `icon-map.json`
  既在 `src-tauri/assets/fnos-mods/`（provenance + NOTICE 的 SHA 表）又在 `ui/settings/`
  （资产协议只能服务 `frontendDist`，而 `chrome.runtime.getURL` 必须同步给出真实 URL）。
  代价是 `frontendDist` 大约 +1.0 MB（`generate_context!` 会把它嵌进二进制）。
- **失败判定只覆盖首次导航**：页面**内**跳转失败不会切错误页（状态条仍会给出「把当前页加入白名单」）。
- **release 版没有控制台**（`windows_subsystem = "windows"`），`[fnos]` 日志不可见；排障请用 debug 版。
- **安装包未签名**：仓库里没有任何签名证书配置，`Get-AuthenticodeSignature` 对安装包与主程序实测都是
  `NotSigned`，所以**双击**运行时预计会被 SmartScreen 的「Windows 已保护你的电脑」拦一下（「更多信息 → 仍要运行」）。
  本次 T12 验收用的是静默安装（`/S`），没有触发该提示，因此这条是**推断**而非实测。
- **不要把 `fnos-desktop.exe` 拷到别处运行**：本机实测，复制出来的副本会**静默退出**（无报错、无窗口）。
  始终从它真实所在目录运行——开发版跑 `src-tauri\target\<profile>\fnos-desktop.exe`，安装版从开始菜单启动。
- 「关于」页显示的许可件路径依赖 `bundle.resources`；若手工删掉了安装目录里的 `fnos-mods\`，路径会指向不存在的文件。
- **导入新登录壁纸不会删掉旧文件**：落盘名带内容指纹（`<stem>-<fnv64>.<ext>`），换一张就是新文件，
  被替换掉的旧壁纸会如实留在配置目录里（无害、可手工删除）——本壳不做自动清理，免得误删用户自己放进去的东西。

## 需要人工确认的部分

自动化能覆盖的部分（构建、测试、安装/卸载往返、UIAutomation 读界面文案）都在
[`docs/acceptance/M1-M2-验收记录.md`](docs/acceptance/M1-M2-验收记录.md) 与本次 T12 报告里；
**§3「需要人工确认（本次未执行）」** 列出了必须由你本人拿着真实 NAS（FN ID `ea121314`）做的检查：
真实 WebUI 上的肉眼效果、F12 判据、真鼠标托盘操作、切换项的免刷新即时性、真实首屏耗时。

> 验收记录是 M1–M2 那次 revision 的快照（托盘当时 5 项，现为 6 项，见记录里的「后续变更」注记）。

## 目录

| 路径 | 内容 |
|---|---|
| `src-tauri/src/` | Rust 侧：`paths`（配置/资源路径）、`config`（配置与归一化）、`injector`（注入载荷）、`tray`（托盘）、`commands`（IPC + 窗口/错误页/加载观测） |
| `src-tauri/assets/fnos-mods/` | 上游 vendored 资源 + `LICENSE` + `NOTICE`（含上游 popup 的 `popup.html` / `popup.js`） |
| `src-tauri/inject/` | `shim.js`（页面侧 chrome.* 兼容层 + 上报通道）、`bootstrap.js`（配置装配与注入闸门） |
| `ui/settings/` | 设置窗前端（零依赖、零构建链）：`settings.html` + `app.js`（本壳区域与宿主桥）、`chrome-shim.js`（iframe 里的 chrome.* 兼容层）、`popup.html`/`popup.js`（上游 UI 的逐字节副本 + 一行标签）、内置错误页 `error.html` |
| `tests/` | Node 契约测试 |
| `docs/superpowers/specs/` | 设计文档 |
| `docs/acceptance/` | 端到端验收记录 |

## 许可

本仓**自身**的代码没有单独的 LICENSE 文件；随附的第三方资源（`src-tauri/assets/fnos-mods/`）
遵循上游 **FnOS UI Mods Non-Commercial License 1.0**，全文见该目录下的 `LICENSE`，
来源与改动说明见 `NOTICE`。使用本应用即表示你接受：**不得用于任何商业用途**。
