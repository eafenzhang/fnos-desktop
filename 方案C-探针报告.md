# 方案 C（Tauri 2）探针报告

> 探针性质：**throwaway spike**，不是正式代码。目录 `_spike_tauri/`，可随时删除。
> 探针工程：`_spike_tauri/src-tauri/`（Rust 主程序 + 本地设置窗），载荷 `_spike_tauri/assets/basic_mod.css`（仓库 main 分支原样 195500 字节）。
> 结论时间：2026-09-28

---

## 0. 先回答「上一次为什么没有结果」

另一个对话（session `72353f49…`）在开始搭 Tauri 探针的那一步会话中断了 —— 最后一条工具调用是**空调用**（`CALL : {}`），探针从未构建、从未运行，所以没有任何实证结论。本报告是把那个探针真正跑完的结果。

---

## 1. 总判定：**方案 C 成立**，而且比上一轮对话里「不推荐」的判断要好得多

上一轮对话给出的静态结论是「Tauri 2 对远程页面注入支持很弱（只能 eval 自己的窗口）」—— **这个判断是错的**。实证结论：

| # | 探针问题 | 结果 | 硬证据 |
|---|---------|------|--------|
| ① | 能否直接加载外部 https 站点 | ✅ **能** | `WebviewUrl::External(https://fnos.net/)` 窗口正常渲染飞牛 FN Connect 页面（`final_main.png`） |
| ② | 能否在**文档解析前**注入（document_start 等价物） | ✅ **能** | 注入脚本内读到 `document.readyState === "loading"`；`initAt=200.4ms` 远早于 `dclAt=671.0ms`；`注入目标=顶层文档` |
| ③ | 真实 mods 产物能否注入并解析 | ✅ **能** | 195500 字节 `basic_mod.css` 注入后解析出 **149 条规则**，`document.styleSheets` = 2 |
| ④ | 主进程 eval 注入远程页面 | ✅ **能** | `eval` 下发后窗口标题实变为 `[eval-ok] FN Connect 远程访问 - 飞牛 fnOS`（UIA 读到），`OK @ 512.8ms` |
| ⑤ | mods 的 `corner-shape`/squircle 效果是否有内核支撑 | ✅ **有** | `CSS.supports('corner-shape','squircle') === true` / `'round' === true` |
| ⑥ | 托盘图标 + 右键菜单（含勾选项） | ✅ **能** | 托盘图标 UIA 可见（tooltip `fnOS 桌面壳 · 方案C 探针`）；右键菜单四项齐全且勾选项带 ✓（`final_menu_crop.png`） |
| ⑦ | 托盘菜单动作 → 本地设置窗 | ✅ **能** | 菜单事件触发后日志 `✅ 设置窗创建成功（本地 WebviewUrl::App，与外部站点窗口并存）`，`final_settings.png` 与外部站点窗并存 |

---

## 2. 关键实测数据

### 2.1 运行环境

| 项 | 值 |
|---|---|
| tauri | **2.12.0**（tauri-build 2.7.0 / wry 0.57.0 / tray-icon 0.25.1 / muda 0.20.0） |
| Rust | 1.96.0，默认工具链 `x86_64-pc-windows-gnu` |
| WebView2 运行时 | **148.0.3967.54**（= Chromium 148） |
| OS | Windows 11 26100 |

### 2.2 注入时序（`final_main.png` 面板原文）

```
origin             = https://fnos.net    guard(*.fnos.net)=true
readyState@init    = loading    (loading 即为 document_start 等价时机)
initAt / dclAt(ms) = 200.4 / 671.0
注入目标           = 顶层文档
真实 mods CSS      = 已注入 195500 字节(UTF-8)，解析出 149 条规则
styleSheets        = 2 个
corner-shape 支持  = squircle:true / round:true
chromium           = Chrome/148.0.0.0
__TAURI_INTERNALS__= object
主进程 eval 注入   = OK @ 512.8 ms
```

底层机制：`WebviewWindowBuilder::initialization_script()` → WebView2 的 `AddScriptToExecuteOnDocumentCreated`，因此**不受页面 CSP 限制**，且会在每次顶层/子框架导航时执行（SPA 场景天然覆盖）。官方文档也明确建议在脚本里用 `window.location` 做 origin 守卫 —— 与 mods「只对飞牛 WebUI 生效」的设计正好对口。

### 2.3 托盘菜单（`final_menu_crop.png` + `run.log`）

菜单结构（与设计一致）：

```
✓ 注入 mods            → 日志：托盘勾选项「注入 mods」-> false
  显示 / 隐藏主窗口     → 日志：主窗口 -> 隐藏 / 主窗口 -> 显示
  系统设置             → 日志：✅ 设置窗创建成功（本地 WebviewUrl::App，与外部站点窗口并存）
  ────────────────
  退出                → 日志：退出（进程正常结束）
```

四项**全部实测回调成功**（鼠标点击验证；其中显示/隐藏、系统设置也用键盘 ↓↓/↓↓↓ + Enter 复现过）。

---

## 3. 两个必须记下的坑（都会直接卡住构建/结论）

### 3.1 【构建】rustup 的 gnu 工具链缺 `as.exe` → dlltool 失败

现象：

```
error calling dlltool 'dlltool.exe': program not found      # PATH 里没有 dlltool
error: dlltool could not create import library ... dlltool.exe: CreateProcess   # 找到 dlltool 但仍失败
```

根因（`dlltool --verbose` 直接暴露）：

```
Using file: <rustup>\...\self-contained\as        ← 它去找同目录的 as
run: <rustup>\...\self-contained\as --64 -o kernel32.dll:h.o kernel32.dll:h.s
No such file or directory
CreateProcess
```

`rustup` 的 `x86_64-pc-windows-gnu` 自包含目录里只有 `dlltool.exe / ld.exe / x86_64-w64-mingw32-gcc.exe`，**没有 `as.exe`**；而 `windows-sys` 之类的 `raw-dylib` 依赖必须靠 dlltool 生成导入库。同时本机没有 MSVC `link.exe`，也没有 Windows SDK，MSVC 路线走不通。

**解法（已验证）**：本机装有 MSYS2，把它的 mingw64 bin 放进 PATH 即可：

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH   # 提供 as.exe(2.46) / dlltool / gcc 16.1.0
cargo build                                        # 1m22s 全量，增量 15s
```

> 若要走 MSVC 路线，则需安装 VS Build Tools（数 GB）——对探针而言不值得。

### 3.2 【构建】tauri-build 需要 `icons/icon.ico`

```
`...\src-tauri\icons/icon.ico` not found; required for generating a Windows Resource file during tauri-build
```

即使 `bundle.active = false` 也会要求。探针里用 `System.Drawing` 内存生成了一个 32×32 ICO（`_spike_tauri/src-tauri/icons/`）。

### 3.3 【探针自身的两个 bug —— 差一点得出错误结论】

1. **`corner-shape` 假阴性**：第一版用 `CSS.supports('corner-shape','smooth')` 判定 → 返回 **false**。但 `smooth` **不是 `corner-shape` 的合法值**（合法值是 `squircle / scoop / bevel / notch / round`），所以这个 false 是假阴性。改用 `'squircle'` 后为 **true**。
   → 教训：能力探测必须用**合法值**；否则会把「内核支持」误判成「不支持」。
2. **`cssRules.length` 读太早**：`appendChild(style)` 之后立刻读规则数不可靠（大表异步解析）。已改为在 `DOMContentLoaded` 之后读权威值；本次两次读数一致（149/149），故此坑未在本机复现，属预防性修正。
3. **面板里 `CSS长度` 与 `字节数` 不是一回事**：`String.length` 是 UTF-16 码元数，`Blob.size` 才是 UTF-8 字节数。已改用 Blob（195500 字节）。

---

## 4. ⚠️ 安全发现（做正式实现时必须处理）

探针实测：**外部站点页面里 `typeof window.__TAURI_INTERNALS__ === 'object'`** —— Tauri 会把 IPC 桥注入到它创建的每一个 webview，包括 `WebviewUrl::External` 加载的远程页面。

含义：远程页面（fnos.net 或任何被注入的站点）理论上能调用 Tauri 命令。正式实现必须：

- 在 capability 里**只对本应用自己的窗口/命令**授权，绝不给远程 URL 开 remote 权限；
- 主窗口只暴露**必要**命令（读配置/写配置/重注入），并且命令内部做参数校验；
- 更重要的是：**注入的 mods JS/CSS 与页面同处一个 JS 上下文**（初始化脚本是页面级注入，不是隔离世界）。所以「桌面壳」相对浏览器扩展（方案 B）在**权限粒度上是变弱的** —— 这点要在选型时摆到台面上。

---

## 5. 顺带发现：mods 仓库里已有 fnOS 原生应用包（`fpk/`）

`fnOS_UI_Mods` 仓库除浏览器扩展外，还有 `fpk/FnOS_UI_Mods/`：`manifest` / `config/privilege` / `app/server/server.js`(77KB) / `app/server/inject_shell.sh` / `app/www/…`（含 `basic_mod.css`、`mod.js` 等）。

也就是说：**「改飞牛 WebUI 外观」已经有服务端注入的 fpk 路线**。这对方案选型有影响 —— 如果 fpk 已能满足外观改造，Windows 桌面壳的价值就应聚焦在「托盘常驻 + 免开浏览器 + 独立设置窗」这些**壳**的能力上，而不是重复造注入。建议下一步先确认这个 fpk 的定位与你的目标是否重叠。

---

## 6. 方案 C vs 方案 A（基于实测的重新判断）

| 维度 | 方案 A（Electron 注入） | 方案 C（Tauri 2）— 实测后 |
|---|---|---|
| 远程页面 document_start 注入 | ✅ | ✅ **同样成立**（`initialization_script`，CSP 免疫） |
| 托盘 + 勾选菜单 | ✅ | ✅ 原生 `muda`，实测四项动作全通 |
| 本地设置窗与外部窗并存 | ✅ | ✅ `WebviewUrl::App` |
| 体积 | ~80–100MB | **~10MB 级**（用系统 WebView2） |
| 内核可控性 | 自带 Chromium，版本随应用 | **跟随系统 WebView2**（本机 148，`corner-shape` 可用；但低版本客户端有风险，需运行时检测 + 降级） |
| 注入权限粒度 | 扩展 API / 可做隔离 | ❌ **与页面同上下文**，且远程页面自动带 IPC 桥（见 §4） |
| 构建环境（本机） | Node 即可 | ⚠️ 需修 PATH（缺 `as.exe`），或装 VS Build Tools |
| 跨平台一致性 | 高 | 中（各平台 WebView 引擎不同） |

**修正后的建议**：方案 C **可行且不贵**，如果核心诉求是「托盘常驻 + 小体积 + 快速启动」，它比方案 A 更合适；如果核心诉求是「注入行为可控、可隔离、跨机一致性」，方案 A 更稳。二者在注入能力上**没有上一轮以为的那种差距**，差异其实在**体积**与**安全/隔离**这一对权衡上。

---

## 7. 复现步骤

```powershell
# 1) 修工具链（关键）
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH

# 2) 构建（icons/icon.ico 必须存在）
cd D:\fnOS-desktop\_spike_tauri\src-tauri
cargo build

# 3) 运行（保留控制台可看 [probe] 日志）
.\target\debug\fnos-probe.exe
```

验证点：窗口顶部绿色探针面板（`readyState@init=loading`、`corner-shape squircle:true`、mods 149 条规则）；任务栏托盘「显示隐藏的图标」里右键 → 勾选项/显示隐藏/系统设置/退出。

---

## 8. 产物清单

| 文件 | 说明 |
|---|---|
| `_spike_tauri/src-tauri/src/main.rs` | 探针主程序（外部 URL 窗口 + init script + 托盘菜单 + 设置窗） |
| `_spike_tauri/assets/basic_mod.css` | 真实 mods 产物（195500 字节，注入载荷） |
| `_spike_tauri/final_main.png` | 外部站点窗口 + 探针面板（核心证据） |
| `_spike_tauri/final_menu_crop.png` | 托盘右键菜单（含 ✓ 勾选项） |
| `_spike_tauri/final_settings.png` | 本地设置窗（与外部窗并存） |
| `_spike_tauri/run.log` | 运行日志（全部 [probe] 事件） |
| `_spike_tauri/build5.log` | 构建日志 |

---

## 9. 遗留 / 待确认

1. **键盘激活勾选项异常**：菜单打开后 `↓+Enter` / `↓+Space` 命中第 1 项（勾选项）时**未回调**，而第 2、3 项正常；同一项用**鼠标点击正常回调**。疑似与菜单弹出时的 hover 预选/焦点有关，非方案阻塞项，但正式实现里建议再核一遍（或直接用鼠标路径验证）。
2. **WebView2 版本下限**：本机 148 支持 `corner-shape`。正式版需在启动时检测运行时版本，并在低版本上对 squircle 这类效果做**降级**。
3. **子框架注入**：本次实测注入目标是顶层文档。`initialization_script` 文档称也会在所有子框架导航执行，飞牛 WebUI 若有 iframe 场景需再验一次（探针已预留 `注入目标` 字段）。
4. **远程页面 IPC 桥**（§4）需要在正式设计里给出明确的白名单/关闭方案。
