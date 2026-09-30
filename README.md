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
  独立窗口；只有命中 fnOS WebUI 的页面才注入，其余站点原样浏览。
- **托盘常驻**：显示窗口 / 重新加载 / 系统设置 / 检查更新 / 退出；关闭窗口默认收到托盘
  （`closeToTray` 可关）。
- **检查更新 + 自动升级**：托盘一键检查 GitHub Releases；有新版本时**自动下载安装包**
  （下载到配置目录 `updates\`，同名复用、`.part` 原子落盘、体积健全性校验），确认后
  **静默安装并自动重启**。下载地址的 host 与路径被钉死在本仓库的发布资产下，
  本地环回 / 内网地址一律拒绝。
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
- **设置窗 = 上游原版界面**：设置窗里就是上游扩展自己的 popup UI（字节不改动，
  装在 iframe 里），经本壳的 `chrome.*` 兼容层直接读写真实配置，改完免刷新生效。
  「完美图标」（启动台图标形状统一 / 逐项重绘）等上游功能在本壳下完整可用。
- **内置错误页与自动重试**：加载失败 / 离线时切到内置错误页（15s → 30s → 60s → 120s
  退避，第 5 次停止），也可手动重试。

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
| 设置窗本地状态 | `%APPDATA%\com.fnos.desktop\local-store.json` |
| 登录壁纸导入文件 | `%APPDATA%\com.fnos.desktop\`（文件名带内容指纹） |
| 更新下载缓存 | `%APPDATA%\com.fnos.desktop\updates\` |
| WebView2 缓存 | `%LOCALAPPDATA%\com.fnos.desktop\EBWebView` |
| 配置目录覆盖 | 环境变量 `FNOS_DESKTOP_CONFIG_DIR`（隔离测试 / 便携） |

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

**改了 `ui/` 之后必须强制重编译**：`frontendDist` 资产在编译期嵌进二进制，`tauri-codegen`
不为它发 `rerun-if-changed`。任选其一：

```powershell
cargo clean -p fnos-desktop ; cargo build
(Get-Item src\main.rs).LastWriteTime = Get-Date ; cargo build
```

## 发版（GitHub Actions 自动构建）

[`.github/workflows/release.yml`](.github/workflows/release.yml) 有两条触发线：

- **push 到 `master`**：只做构建冒烟（及时暴露编译 / 打包错误，不发版）；
- **push 标签 `v*`**：构建 NSIS 安装包并**自动创建 GitHub Release** 上传——这就是「发布新版本」。

发版流程：

1. 把 `src-tauri/tauri.conf.json` 与 `src-tauri/Cargo.toml` 的 `version` **一起**改成新版本
   （两者必须一致：前者决定安装包名，后者是托盘「检查更新」比对用的当前版本）；
2. commit 后 `git tag vX.Y.Z && git push origin master vX.Y.Z`；
3. CI 构建完成即发布，已装用户通过托盘「检查更新」自动升级。

## 目录

| 路径 | 内容 |
|---|---|
| `src-tauri/src/` | Rust 侧：`paths`（配置/资源路径）、`config`（配置与归一化）、`injector`（注入载荷与资产嵌入）、`tray`（托盘）、`updater`（检查更新 / 自动下载 / 静默安装）、`commands`（IPC + 窗口 / 错误页 / 加载观测） |
| `src-tauri/inject/` | `shim.js`（页面侧 chrome.\* 兼容层 + 上报通道）、`bootstrap.js`（配置装配与注入闸门）、`dock.js`（Dock 自动隐藏 + 空间回收）、`keepalive.js`（登录保活）、`windowpos.js`（桌面内窗口居中 / 最大化往返跟随）、`appchrome.js`（本壳应用窗口的自绘标题栏） |
| `src-tauri/assets/fnos-mods/` | 上游 vendored 资源 + `LICENSE` + `NOTICE`（provenance 与 SHA-256 表） |
| `ui/settings/` | 设置窗前端（零依赖、零构建链）：上游 `popup.html`/`popup.js` 逐字节副本 + `chrome-shim.js`（iframe 里的 chrome.\* 兼容层）+ `app.js`（宿主桥）+ 内置错误页 |
| `tests/` | Node 契约测试（145 项）与 Rust 单测（115 项） |
| `docs/` | 设计文档与验收记录 |
| `.github/workflows/` | 自动构建 / 发版 |

## 失败模式与恢复

| 现象 | 行为 |
|---|---|
| 页面不是 fnOS WebUI | 上游签名判定不注入；设置窗提示并支持一键加白名单 |
| 外链 CSS 被拦 / `mod.js` 的 data URL 被拦 | `adoptedStyleSheets` 兜底 / 兜底执行原始脚本 |
| 主窗口加载失败 / 离线 | 内置错误页 + 三种重试入口 + 自动退避（第 5 次停止） |
| 页面 20 秒不完成 | 看门狗兜底判失败 |
| `config.json` 损坏 | 回退默认值，原文件保留为 `.bak` |
| 检查更新网络失败 | 托盘弹窗如实告知，绝不静默 |

## 已知限制

- **不含字体文件**：上游 popup 的字体导入控件在上游 HTML 里本就是注释掉的；
  `customFont*` 写入请求被如实拒绝（不假装成功）。字体覆盖只能用本机已装字体或网络字体。
- **载荷约 423KB / 每次导航**（7 个 CSS + `mod.js` + `content-script.js`），每次导航重新注入。
- **失败判定只覆盖首次导航**：页面内跳转失败不切错误页。
- **release 版没有控制台**（`windows_subsystem = "windows"`），排障用 debug 版。
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
