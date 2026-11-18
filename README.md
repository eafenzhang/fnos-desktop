# fnOS Desktop（fnOS 桌面壳）

把 [飞牛 fnOS](https://www.fnnas.com/) NAS 的 WebUI 装进一个 Windows 桌面窗口：常驻托盘、
登录保活、Dock 自动隐藏、应用窗口居中，并向页面注入
[fnOS_UI_Mods](https://github.com/aurysian-yan/fnOS_UI_Mods) 的界面美化
（上游资源**原样 vendored、未作修改**）。

> **非官方**：本项目是第三方桌面壳，**不是飞牛（fnOS）官方产品**，与飞牛官方无任何关联，
> 也未获其授权或认可。随附的界面修改资源遵循上游 **FnOS UI Mods Non-Commercial License 1.0**，
> **仅供非商业个人使用**，禁止任何商业用途。

## 功能

- **桌面窗口**：把 fnOS WebUI（默认 `https://fnos.net/`，可配置为自己的 NAS 地址）装进
  独立窗口；只有命中 fnOS WebUI 的页面才注入，其余站点原样浏览。完整载荷还带**运行时
  origin 闸门**：同一个 Webview 里导航到第三方站点后，`__FNOS_SHELL__`、mods、dock、
  keepalive 立即停止执行；`FNOSREPORT:` / `FNOSPROBE:` / `FNOSCHUNK:` 控制标题也只接受
  配置过的 fnOS origin。
- **托盘常驻**：左键单击显示/前置主窗口；右键菜单含 显示窗口 / 重新加载 / 系统设置 /
  检查更新 / 退出；关闭窗口默认收到托盘（`closeToTray` 可关）。
- **检查更新 + 自动升级**：托盘一键检查 GitHub Releases；有新版本时**自动下载安装包**
  （下载到配置目录 `updates\`，同名复用前先校验、`.part` 原子落盘 + 进程内唯一名、
  体积健全性校验、**sha256 与 release 的 `assets[].digest` 比对**），确认后
  **静默安装并自动重启**。下载地址的 host 与路径被钉死在本仓库的发布资产下，
  本地环回 / 内网地址一律拒绝；重复点击有重入闸门，不会并发下载。
- **多标签页 + 自带标题栏**：主窗口是无边框窗口，顶部一条自绘栏（品牌图标 + 标签 + ─ □ ×）；
  fnOS 里 `window.open` 出来的外部应用开成标签页而不是独立窗口，同一页面被反复
  `window.open` 时**复用切换、不重复开**（FN Connect 中继流程靠这条止血）。
  标签条支持 `Ctrl+T` / `Ctrl+W` / `Ctrl+Tab` / `Ctrl+Shift+Tab`，带 `role=tab` /
  `aria-selected`、键盘焦点与横向滚动，标签多时当前页自动滚入视野。
- **登录 / 授权弹窗**：OAuth、SSO 与 `about:blank` 弹窗走原生 WebView2 弹窗，保留
  `window.opener`、`window.open` 返回值与 `window.close()`；只有落在配置的 fnOS origin
  上的文档才注入完整载荷，GitHub / Google / DeepSeek 等第三方登录页不执行任何壳代码。
  直接 `window.open('/signin')` 的弹窗首个文档还有页面加载后的补注入兜底。
- **全屏**：标题栏最小化左侧的全屏按钮；全屏时隐藏 fnOS Dock 与 **主屏 + 副屏** Windows
  任务栏，标题栏也隐藏，鼠标移到顶部时悬浮显示；`Esc` 或按钮退出。任务栏隐藏期间写
  `taskbar-hidden.flag`，进程被强杀后下次启动会自动恢复，不会把桌面留在「无任务栏」状态。
- **本地日志**：所有 `[fnos]` 日志同时写 `%APPDATA%\com.fnos.desktop\app.log`
  （4 MiB 轮转；`FNOS_LOG=0` 关）——release 版没有控制台，这是唯一可回传的现场。
- **Dock 自动隐藏**（`inject/dock.js`，默认关，设置里开）：平时滑出屏幕、指针顶到边缘
  热区才唤出；并把 Dock 让出的那条宽度还给桌面（样式层声明式回收，无闪烁）。全屏应用
  不再为 Dock 让位。
- **桌面内应用窗口居中**（`inject/windowpos.js`）：fnOS 自己打开的应用窗口默认落在
  桌面正中；窗口自身的最大化 ↔ 还原往返、**外壳程序窗口**在 Windows 上最大化 / 还原，
  窗口都保持居中（按「中心偏移比例」跟随，拖到一边的窗口保持相对位置）；用户拖动
  / 缩放不被干预。fnOS 窗口管理器的整体重排会被当场纠正（微任务内完成，画不出中间帧）。
- **登录保活**（`inject/keepalive.js`）：fnOS 登录态是会话级 cookie，桌面闲置久了会失效；
  本壳按 `keepAliveMinutes`（默认 10 分钟，0 = 关）在同源上请求一次令牌接口续期。
  路径写死 + 同源校验，无用户可控 URL。
- **设置窗 = 上游原版界面**：设置窗里就是上游扩展自己的 popup UI（`popup.js` 逐字节
  不改动；`popup.html` 仅多一行 `chrome-shim.js` 引入，见 `NOTICE`），装在 iframe 里，
  经本壳的 `chrome.*` 兼容层直接读写真实配置，改完免刷新生效。
  「完美图标」（启动台图标形状统一 / 逐项重绘）等上游功能在本壳下完整可用。
- **内置错误页与自动重试**：加载失败 / 离线时切到内置错误页（15s → 30s → 60s → 120s
  退避，第 5 次停止）；托盘「重新加载」可手动重试。

## 安装

到 [Releases](https://github.com/eafenzhang/fnos-desktop/releases/latest) 下载
`fnOS_<版本>_x64-setup.exe` 双击安装（或 `/S` 静默）。per-user 安装、不需要管理员，
装到 `%LOCALAPPDATA%\fnOS`，并创建开始菜单与桌面快捷方式。托盘「检查更新」即可完成
后续升级，无需再手动下载。

> 安装包未签名：双击时 SmartScreen 可能拦一下（「更多信息 → 仍要运行」）。

## 配置与数据

| 项 | 位置 |
|---|---|
| 配置 | `%APPDATA%\com.fnos.desktop\config.json`（损坏时自动备份为 `config.json.bak`） |
| **运行日志** | `%APPDATA%\com.fnos.desktop\app.log`（超过 4 MiB 轮转为 `app.log.1`，磁盘占用 ≤ 8 MiB；`FNOS_LOG=0` 可关） |
| 设置窗本地状态 | `%APPDATA%\com.fnos.desktop\local-store.json` |
| 登录壁纸导入文件 | `%APPDATA%\com.fnos.desktop\`（文件名带内容指纹） |
| 更新下载缓存 | `%APPDATA%\com.fnos.desktop\updates\` |
| WebView2 缓存 | `%LOCALAPPDATA%\com.fnos.desktop\EBWebView` |
| 配置目录覆盖 | 环境变量 `FNOS_DESKTOP_CONFIG_DIR`（隔离测试 / 便携） |

**排障先看 `app.log`**：release 版没有控制台（`windows_subsystem = "windows"`），所以白屏 /
加载失败 / `window.open` 裁定 / 标签页创建失败 / 更新下载与校验这些现场，唯一能回传的
文本就是它。日志行格式 `YYYY-MM-DD HH:MM:SS.mmmZ [fnos] …`（UTC），页面可控文本一律
经 `report::log_safe` 收口，不会伪造出额外行。

## 从源码构建

Windows 前置：Rust（gnu 工具链 + MSYS2 mingw64 在 `PATH` 最前）、Node.js 18+（跑测试）、
`cargo-tauri` 2.x、WebView2 运行时（Win11 自带）。

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH

cd src-tauri
cargo build                 # debug：保留控制台，[fnos] 日志直接可见
cargo test                  # Rust 单测（配置往返、注入载荷、IPC 契约、托盘/更新器逻辑）
cargo tauri build --bundles nsis   # 产物 target\release\bundle\nsis\fnOS_<版本>_x64-setup.exe
cd ..

npm test                    # 前端契约测试（shim / bootstrap / dock / windowpos / 设置窗 / 上报通道）
```

**改了 `ui/` 之后不需要手动 touch**：`build.rs` 已对 `../ui/settings` 整棵树发了
`cargo:rerun-if-changed`，build script 重跑 → 本 crate 重编 → `generate_context!` 重新嵌入
资产。（此前的做法是手动 `cargo clean -p` 或改 `main.rs` 时间戳，靠记性，容易漏。）

> 背景：`tauri::generate_context!()` 在**宏展开期**读 `frontendDist` 并把资产嵌进二进制，
> 而 `tauri-codegen` 不为同 crate 的资产目录发 `rerun-if-changed`（`tauri-build` 只在
> `Attributes::codegen(...)` + `tauri_build_context!` 那条路里发）。由 build script 代为
> 声明是改动最小、且不需要动运行时宏的方案。

## 发版（GitHub Actions 自动构建）

[`.github/workflows/release.yml`](.github/workflows/release.yml) 有三条触发线：

- **push 到 `master` / PR**：先跑**测试门禁**（前端契约测试 + Rust 单测），全绿再做构建冒烟
  （及时暴露编译 / 打包错误，不发版）；测试红了直接拦住后面的 build job；
- **push 标签 `v*`**：测试通过后构建 NSIS 安装包并**自动创建 GitHub Release** 上传——
  这就是「发布新版本」。

发版流程：

1. 把 `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml` 与 `package.json` 的 `version`
   **一起**改成新版本（三者必须一致：conf 决定安装包名，Cargo 是托盘「检查更新」比对用的
   当前版本，package.json 由契约测试钉住）；
2. commit 后 `git tag vX.Y.Z && git push origin master vX.Y.Z`（tag 必须是严格 `vX.Y.Z`；
   CI 会校验 tag 与三处版本一致，不一致直接失败）；
3. CI 先建 **draft** Release，上传安装包并校验资产存在且带 `sha256:` digest 后才自动发布；
   已装用户通过托盘「检查更新」自动升级。

> 本地构建用 GNU 工具链（见上），GitHub Actions 的 `windows-latest` 用 MSVC 构建发布包；
> 两者都能产出 NSIS 安装包，差异只在 host 工具链，不影响产物。

## 目录

| 路径 | 内容 |
|---|---|
| `src-tauri/src/` | Rust 侧：`paths`（配置/资源路径）、`config`（配置与归一化）、`injector`（注入载荷与资产嵌入）、`logfile`（release 版日志落盘）、`tray`（托盘）、`updater`（检查更新 / 自动下载 / 校验 / 静默安装）、`commands`（IPC + 窗口 / 错误页 / 加载观测 / 标签页） |
| `src-tauri/inject/` | `shim.js`（页面侧 chrome.\* 兼容层 + 上报通道）、`bootstrap.js`（配置装配与注入闸门）、`dock.js`（Dock 自动隐藏 + 空间回收）、`keepalive.js`（登录保活）、`windowpos.js`（桌面内窗口居中 / 最大化往返跟随）、`keys.js`（标签快捷键 + 同源 iframe 转发） |
| `src-tauri/assets/fnos-mods/` | 上游 vendored 资源 + `LICENSE` + `NOTICE`（provenance 与 SHA-256 表） |
| `ui/settings/` | 设置窗前端（零依赖、零构建链）：上游 `popup.js` 逐字节副本 + `popup.html`（仅多一行 `chrome-shim.js` 引入）+ `chrome-shim.js`（iframe 里的 chrome.\* 兼容层）+ `app.js`（宿主桥）+ `titlebar.*`（一行式标签栏）+ 内置错误页 |
| `tests/` | Node 契约测试（173 项）与 Rust 单测（146 项） |
| `docs/` | 设计文档与验收记录 |
| `.github/workflows/` | 测试门禁 / 自动构建 / 发版 |

## 失败模式与恢复

| 现象 | 行为 |
|---|---|
| 页面不是 fnOS WebUI | 上游签名判定不注入；运行时 origin 闸门再拦一次；设置窗提示并支持一键加白名单 |
| 外链 CSS 被拦 / `mod.js` 的 data URL 被拦 | `adoptedStyleSheets` 兜底 / 兜底执行原始脚本 |
| 主窗口加载失败 / 离线 | 内置错误页 + 托盘「重新加载」+ 自动退避（第 5 次停止） |
| 页面 20 秒不完成 | 看门狗兜底判失败 |
| `config.json` 损坏 | 回退默认值，原文件保留为 `.bak` |
| `config.json` 来自更高版本 | 不降级覆盖；原文件备份为 `.bak`，本次用默认配置启动 |
| 全屏时进程被强杀 | 留下 `taskbar-hidden.flag`；下次启动无条件恢复主副屏任务栏并清除标记 |
| 第三方登录页（GitHub / Google / DeepSeek） | 原生弹窗保留登录语义，但运行时闸门拒绝执行全部 mods / shim |
| 检查更新网络失败 | 托盘弹窗如实告知，绝不静默 |
| 下载的安装包校验失败 | 比对 GitHub Release 的 `assets[].digest`（sha256）；不匹配即丢弃重下，绝不执行 |
| 重复点「检查更新」 | 有重入闸门：第二次点击直接提示「正在检查 / 下载更新」，不并发下载 |
| 第三方应用页乱写 `document.title` | 控制标题只接受配置过的 fnOS origin，外部页无法伪造加载失败或覆盖状态证据 |

## 已知限制

- **不含字体文件**：上游 popup 的字体导入控件在上游 HTML 里本就是注释掉的；
  `customFont*` 写入请求被如实拒绝（不假装成功）。字体覆盖只能用本机已装字体或网络字体。
- **完整载荷约 423KB / 每次导航**（7 个 CSS + `mod.js` + `content-script.js`），只发给
  桌面根页面与登录/授权页；`/p`、`/seek` 这类应用标签只注入几 KB 的快捷键核心脚本。
  脚本正文按配置指纹缓存在内存里，开新标签页不再重读壁纸 / 重做 base64。
- **失败判定只覆盖首次导航**：页面内跳转失败不切错误页。
- **release 版没有控制台**（`windows_subsystem = "windows"`）：日志落在
  `%APPDATA%\com.fnos.desktop\app.log`，排障看它即可，不必再换 debug 版。
- **安装包未签名**（见上）。
- **不要拷贝 `fnos-desktop.exe` 到别处运行**：副本会静默退出；从安装目录或
  `target\<profile>\` 原地运行。

## 合规

上游许可是 **FnOS UI Mods Non-Commercial License 1.0**：禁止商业用途；分发必须保留
版权声明与许可全文；修改版必须明确说明改动。因此：

- `src-tauri/assets/fnos-mods/` 保留上游 `LICENSE` 原文 + `NOTICE`（来源仓库、锁定
  commit、各文件 SHA-256、本壳的包装性改动清单）；
- 两份文件通过 `bundle.resources` 随安装包分发（安装目录 `fnos-mods\` 下），卸载时一并删除。

本仓自身代码没有单独的 LICENSE；使用本应用即表示你接受上游的**非商业**约束。
