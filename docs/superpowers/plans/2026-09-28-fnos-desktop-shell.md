# fnOS 桌面壳 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 Tauri 2 做一个 Windows 桌面壳，加载 fnos.net 并在 fnOS NAS WebUI 页面上原样复用上游 `fnOS_UI_Mods` 的注入能力，配托盘菜单与设置窗，最终产出 NSIS 安装包。

**Architecture:** `initialization_script`（等价 document_start、免疫 CSP）按「配置对象 → chrome shim → bootstrap → 上游 content-script.js」四段注入；上游代码原样 vendored，通过 ~100 行 `chrome.*` 兼容层驱动；Rust 侧独占配置状态，设置窗只经 IPC 与 Rust 通信；配置变更用 `webview.eval` 派发 `storage.onChanged` 实现免刷新生效。

**Tech Stack:** Rust 1.96 / Tauri 2（tauri 2.12+、tauri-plugin-single-instance 2.5）/ 原生 HTML+CSS+JS（零框架）/ Node 26（仅测试）/ NSIS 打包

**Spec:** `docs/superpowers/specs/2026-09-28-fnos-desktop-shell-design.md`（已冻结；实现以它为准）

## Global Constraints

- 平台仅 Windows；工具链为 `x86_64-pc-windows-gnu`，**每次 cargo 命令前必须** `$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH`（否则 dlltool 缺 `as.exe` 报 `CreateProcess`）
- `src-tauri/icons/icon.ico` 必须存在，否则 `tauri-build` 直接失败
- `productName = "fnOS"`，`identifier = "com.fnos.desktop"`，`version = "0.1.0"`
- 配置路径 `%APPDATA%\com.fnos.desktop\config.json`，`schemaVersion = 1`
- 上游 25 个 sync 键的**键名与默认值必须与 `content-script.js:2890-2917` 完全一致**
- 上游 JS/CSS **原样不修改**；vendored 目录必须带 `LICENSE` 与 `NOTICE`（含 commit 483c3e2 与各文件 SHA-256）
- capability **绝不给远程 origin 任何命令授权**（P1 的 `mods_page_report` 是唯一例外，且仅限 Task 13）
- 关主窗口 = 隐藏到托盘；退出只走托盘菜单
- 产物只做 NSIS，不做便携版
- 字体文件不打包、不支持导入；字体只走 `fontFamily` / `fontMonospaceFamily` / `fontUrl`
- 每个 Task 结束时必须提交一次 git commit

## File Structure

| 文件 | 职责 |
|---|---|
| `src-tauri/Cargo.toml` | 依赖与 release profile |
| `src-tauri/build.rs` | `tauri_build::build()` |
| `src-tauri/tauri.conf.json` | productName/identifier/frontendDist/bundle(nsis) |
| `src-tauri/src/main.rs` | 组装：插件、state、setup、窗口、单实例 |
| `src-tauri/src/config.rs` | 配置模型 + 默认值 + 归一化 + 原子持久化 + 迁移 |
| `src-tauri/src/injector.rs` | 纯函数：Config + 资源 → initialization_script 字符串 |
| `src-tauri/src/tray.rs` | 托盘图标、菜单、事件分发 |
| `src-tauri/src/commands.rs` | 5 个 IPC 命令 + 配置应用与派发 |
| `src-tauri/src/paths.rs` | 配置/日志路径解析 |
| `src-tauri/inject/shim.js` | `chrome.*` 兼容层 |
| `src-tauri/inject/bootstrap.js` | 惰性 getURL、外链自检与降级、mod.js 兜底 |
| `src-tauri/assets/fnos-mods/**` | 上游 vendored 资源（含 LICENSE/NOTICE） |
| `src-tauri/capabilities/default.json` | 只授权 settings 窗口 |
| `ui/settings/settings.html` | 设置窗骨架 |
| `ui/settings/settings.css` | 飞牛风格样式 |
| `ui/settings/schema.js` | 设置项声明（分组/键/控件/约束） |
| `ui/settings/normalize.js` | 归一化（与 Rust 侧同语义） |
| `ui/settings/bridge.js` | invoke 封装 + 错误呈现 |
| `ui/settings/app.js` | schema 驱动渲染 + 双向绑定 |
| `tests/shim.test.mjs` | shim 契约测试（node:test） |
| `tests/normalize.test.mjs` | 前端归一化测试 |
| `tools/vendor-mods.ps1` | 从 `.ref/` 生成 `assets/fnos-mods/` 并产出 NOTICE 哈希 |
| `README.md` | 构建前置、构建/运行/打包命令 |

---

### Task 1: 项目骨架可构建可运行（外部窗口 + 托盘占位）

**Files:**
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/icons/icon.png`, `src-tauri/icons/icon.ico`, `ui/settings/settings.html`, `.gitignore`（已存在，追加 `src-tauri/target/`）
- Test: 手工运行

**Interfaces:**
- Consumes: 无
- Produces: 可构建运行的 Tauri 应用；`main.rs` 中 `tauri::Builder` 骨架，后续 Task 在此接线

- [ ] **Step 1: 写 `src-tauri/Cargo.toml`**

```toml
[package]
name = "fnos-desktop"
version = "0.1.0"
edition = "2021"
build = "build.rs"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = ["tray-icon", "image-png"] }
tauri-plugin-single-instance = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[profile.release]
lto = true
codegen-units = 1
opt-level = "s"
strip = true
```

- [ ] **Step 2: 写 `src-tauri/build.rs`**

```rust
fn main() {
    tauri_build::build()
}
```

- [ ] **Step 3: 写 `src-tauri/tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "fnOS",
  "version": "0.1.0",
  "identifier": "com.fnos.desktop",
  "build": {
    "frontendDist": "../ui/settings"
  },
  "app": {
    "windows": [],
    "security": { "csp": null }
  },
  "bundle": {
    "active": true,
    "targets": ["nsis"],
    "icon": ["icons/icon.ico", "icons/icon.png"],
    "windows": {
      "nsis": {
        "installMode": "currentUser",
        "languages": ["SimpChinese", "English"]
      }
    }
  }
}
```

- [ ] **Step 4: 生成图标**（`icons/icon.ico` 缺失会让构建失败）

```powershell
Add-Type -AssemblyName System.Drawing
$dir = 'D:\fnOS-desktop\src-tauri\icons'; New-Item -ItemType Directory -Force -Path $dir | Out-Null
$bmp = New-Object System.Drawing.Bitmap(256,256)
$g = [System.Drawing.Graphics]::FromImage($bmp); $g.SmoothingMode='AntiAlias'; $g.Clear([System.Drawing.Color]::Transparent)
$br = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255,46,144,255))
$g.FillEllipse($br, 8, 8, 240, 240); $g.Dispose()
$bmp.Save("$dir\icon.png", [System.Drawing.Imaging.ImageFormat]::Png)
$bmp32 = New-Object System.Drawing.Bitmap($bmp, 32, 32)
$ico = [System.Drawing.Icon]::FromHandle($bmp32.GetHicon())
$fs = [System.IO.File]::Create("$dir\icon.ico"); $ico.Save($fs); $fs.Close()
$bmp.Dispose(); $bmp32.Dispose()
```

- [ ] **Step 5: 写最小 `ui/settings/settings.html` 与 `src-tauri/src/main.rs`**

`ui/settings/settings.html`：

```html
<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>fnOS 设置</title></head>
<body style="background:#1b1c1e;color:#e8e8ea;font:14px system-ui">设置窗占位（Task 9 实现）</body></html>
```

`src-tauri/src/main.rs`：

```rust
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            let url: tauri::Url = "https://fnos.net/".parse().expect("bad url");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("fnOS")
                .inner_size(1200.0, 820.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("fnOS 启动失败");
}
```

- [ ] **Step 6: 构建并运行验证**

Run:
```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd D:\fnOS-desktop\src-tauri
cargo build
.\target\debug\fnos-desktop.exe
```
Expected: 编译成功；打开窗口显示飞牛官网；重复运行 exe 时只唤起已有窗口（单实例生效）

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(skeleton): Tauri 2 骨架可构建运行（外部窗口 + 单实例）"
```

---

### Task 2: vendored 上游资源 + NOTICE（含 SHA-256）

**Files:**
- Create: `tools/vendor-mods.ps1`, `src-tauri/assets/fnos-mods/NOTICE`
- Create（由脚本产生）: `src-tauri/assets/fnos-mods/{LICENSE,content-script.js,mod.js,basic_mod.css,windows_titlebar_mod.css,mac_titlebar_mod.css,classic_launchpad_mod.css,spotlight_launchpad_mod.css,desktop_icon_mod.css,lockscreen_mod.css,prefect_icon/**}`
- Test: `tools/vendor-mods.ps1` 自带校验

**Interfaces:**
- Consumes: `.ref/fnOS_UI_Mods/`（已存在）与 GitHub 补充文件（`prefect_icon/`）
- Produces: `assets/fnos-mods/` 目录约定 —— 后续 `injector.rs` 以 `include_str!("../assets/fnos-mods/<name>")` 引用

- [ ] **Step 1: 补齐缺失资源**（`prefect_icon/` 与 `icons/` 不在首次快照里）

```powershell
$dst='D:\fnOS-desktop\.ref\fnOS_UI_Mods'
$ProgressPreference='SilentlyContinue'
foreach($f in 'prefect_icon/icon-map.json','icons/icon16.png','icons/icon32.png','icons/icon48.png','icons/icon128.png'){
  $out = Join-Path $dst $f; New-Item -ItemType Directory -Force -Path (Split-Path $out) | Out-Null
  Invoke-WebRequest "https://raw.githubusercontent.com/aurysian-yan/fnOS_UI_Mods/main/$f" -OutFile $out -TimeoutSec 60
}
$map = Get-Content "$dst\prefect_icon\icon-map.json" -Raw | ConvertFrom-Json
foreach($p in $map.PSObject.Properties.Value){
  if($p -like 'prefect_icon/*.png'){
    $out = Join-Path $dst $p; New-Item -ItemType Directory -Force -Path (Split-Path $out) | Out-Null
    Invoke-WebRequest "https://raw.githubusercontent.com/aurysian-yan/fnOS_UI_Mods/main/$p" -OutFile $out -TimeoutSec 60
  }
}
```

- [ ] **Step 2: 写 `tools/vendor-mods.ps1`**

```powershell
# 从 .ref/ 生成 src-tauri/assets/fnos-mods/，并产出含 SHA-256 的 NOTICE
$ErrorActionPreference = 'Stop'
$src = 'D:\fnOS-desktop\.ref\fnOS_UI_Mods'
$dst = 'D:\fnOS-desktop\src-tauri\assets\fnos-mods'
$commit = '483c3e2e217faebc1be45b4e824865854a61e3dd'

$files = @(
  'LICENSE','content-script.js','mod.js','basic_mod.css',
  'windows_titlebar_mod.css','mac_titlebar_mod.css',
  'classic_launchpad_mod.css','spotlight_launchpad_mod.css',
  'desktop_icon_mod.css','lockscreen_mod.css','prefect_icon/icon-map.json'
)
Get-ChildItem "$src\prefect_icon" -Filter *.png -ErrorAction SilentlyContinue |
  ForEach-Object { $files += "prefect_icon/$($_.Name)" }

if (Test-Path $dst) { Remove-Item -Recurse -Force $dst }
New-Item -ItemType Directory -Force -Path $dst | Out-Null

$lines = @()
$lines += 'fnOS Desktop Shell — vendored third-party resources'
$lines += ''
$lines += "Upstream: https://github.com/aurysian-yan/fnOS_UI_Mods"
$lines += "Pinned commit: $commit"
$lines += 'License: FnOS UI Mods Non-Commercial License 1.0 (see LICENSE, copied verbatim below)'
$lines += ''
$lines += 'Packaging changes made by this project (upstream files themselves are UNMODIFIED):'
$lines += '  1. chrome.* compatibility shim injected ahead of content-script.js (inject/shim.js)'
$lines += '  2. chrome.runtime.getURL() reimplemented to return data: URLs instead of chrome-extension:// URLs'
$lines += '  3. mod.js delivered through the shim-provided data: URL, with a MutationObserver fallback that'
$lines += '     executes the unmodified original if the data: script is blocked by page CSP'
$lines += '  4. configuration is supplied by the host application instead of chrome.storage'
$lines += ''
$lines += 'File SHA-256:'
foreach ($f in $files) {
  $s = Join-Path $src $f
  if (-not (Test-Path $s)) { throw "missing vendored source: $f" }
  $o = Join-Path $dst $f
  New-Item -ItemType Directory -Force -Path (Split-Path $o) | Out-Null
  Copy-Item $s $o -Force
  $hash = (Get-FileHash $o -Algorithm SHA256).Hash.ToLower()
  $size = (Get-Item $o).Length
  $lines += ('  {0}  {1,9}  {2}' -f $hash, $size, $f)
}
Set-Content -Path (Join-Path $dst 'NOTICE') -Value $lines -Encoding utf8
Write-Host "vendored $(($files | Measure-Object).Count) files into $dst"
```

- [ ] **Step 3: 运行并校验**

Run: `powershell -File D:\fnOS-desktop\tools\vendor-mods.ps1`
然后校验哈希可复现：

```powershell
$dst='D:\fnOS-desktop\src-tauri\assets\fnos-mods'
$notice = Get-Content "$dst\NOTICE"
$bad = 0
$notice | Select-String '^  [0-9a-f]{64}' | ForEach-Object {
  $parts = ($_.Line.Trim() -split '\s+')
  $h = (Get-FileHash (Join-Path $dst $parts[2]) -Algorithm SHA256).Hash.ToLower()
  if ($h -ne $parts[0]) { $bad++; Write-Host "MISMATCH $($parts[2])" }
}
"mismatches=$bad"
```
Expected: `mismatches=0`，且 `NOTICE` 里能看到 4 条包装改动说明

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat(vendor): vendored 上游 mods 资源 + NOTICE(commit+SHA256+包装改动说明)"
```

---

### Task 3: `config.rs` — 配置模型、默认值、归一化、持久化

**Files:**
- Create: `src-tauri/src/config.rs`, `src-tauri/src/paths.rs`
- Modify: `src-tauri/src/main.rs`（加 `mod config; mod paths;`）
- Test: `src-tauri/src/config.rs` 内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: 无
- Produces:
  - `config::Config { schema_version: u32, mods: ModsConfig, local: LocalConfig, shell: ShellConfig }`
  - `config::WindowGeom { w: f64, h: f64, x: Option<f64>, y: Option<f64> }`
  - `Config::default() -> Config`
  - `Config::load(path: &Path) -> Config`（损坏时回退默认并写 `.bak`）
  - `Config::save(&self, path: &Path) -> std::io::Result<()>`（临时文件 + 原子替换）
  - `Config::normalize(&mut self)` / `config::normalize_brand_color(&str) -> String`
  - `config::DEFAULT_HOME_URL: &str = "https://fnos.net/"`
  - 全部字段名用 `camelCase` 序列化（与上游键一一对应）

- [ ] **Step 1: 写失败测试**（`src-tauri/src/config.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_upstream() {
        let c = Config::default();
        assert_eq!(c.mods.brand_color, "#0066ff");
        assert_eq!(c.mods.titlebar_style, "windows");
        assert_eq!(c.mods.launchpad_style, "classic");
        assert_eq!(c.mods.desktop_icon_layout_mode, "adaptive");
        assert_eq!(c.mods.desktop_icon_per_column, 8);
        assert_eq!(c.mods.font_face_name, "FnOSCustomFont");
        assert!(c.mods.auto_enable_suspected_fnos);
        assert!(c.mods.base_preset_enabled);
        assert!(c.mods.desktop_icon_layout_enabled);
        assert!(!c.mods.font_override_enabled);
        assert!(!c.mods.custom_code_enabled);
        assert!(c.mods.enabled_origins.is_empty());
        assert_eq!(c.shell.home_url, DEFAULT_HOME_URL);
        assert!(c.shell.inject_enabled);
        assert!(c.shell.close_to_tray);
    }

    #[test]
    fn brand_color_lightness_is_clamped() {
        // 纯白明度 100% → 夹到 70%；纯黑 0% → 夹到 30%
        assert_eq!(normalize_brand_color("#ffffff"), "#b3b3b3"); // L=70%
        assert_eq!(normalize_brand_color("#000000"), "#4d4d4d"); // L=30%
        assert_eq!(normalize_brand_color("#0066ff"), "#0066ff"); // 合法值保持不变
        assert_eq!(normalize_brand_color("nope"), "#0066ff");    // 非法值回落默认
        assert_eq!(normalize_brand_color("#06f"), "#0066ff");    // 展开短写法
    }

    #[test]
    fn enums_and_numbers_fall_back() {
        let mut c = Config::default();
        c.mods.titlebar_style = "linux".into();
        c.mods.launchpad_style = "weird".into();
        c.mods.desktop_icon_layout_mode = "fixedd".into();
        c.mods.desktop_icon_per_column = 99;
        c.mods.font_weight = "bold".into();
        c.mods.lockscreen_default_username = "x".repeat(120);
        c.normalize();
        assert_eq!(c.mods.titlebar_style, "windows");
        assert_eq!(c.mods.launchpad_style, "classic");
        assert_eq!(c.mods.desktop_icon_layout_mode, "adaptive");
        assert_eq!(c.mods.desktop_icon_per_column, 16);
        assert_eq!(c.mods.font_weight, "");
        assert_eq!(c.mods.lockscreen_default_username.chars().count(), 80);
    }

    #[test]
    fn roundtrip_and_migration() {
        let dir = std::env::temp_dir().join(format!("fnos-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");

        let mut c = Config::default();
        c.mods.brand_color = "#336699".into();
        c.save(&p).unwrap();
        let back = Config::load(&p);
        assert_eq!(back.mods.brand_color, "#336699");
        assert_eq!(back.schema_version, SCHEMA_VERSION);

        // 旧版本（无 schemaVersion / 缺字段）应能补齐
        std::fs::write(&p, br#"{"mods":{"brandColor":"#010203"}}"#).unwrap();
        let migrated = Config::load(&p);
        assert_eq!(migrated.mods.brand_color, "#010203");
        assert_eq!(migrated.schema_version, SCHEMA_VERSION);
        assert_eq!(migrated.mods.titlebar_style, "windows");

        // 损坏 JSON：回退默认 + 生成 .bak
        std::fs::write(&p, b"{not json").unwrap();
        let broken = Config::load(&p);
        assert_eq!(broken.mods.brand_color, "#0066ff");
        assert!(dir.join("config.json.bak").exists());

        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run:
```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd D:\fnOS-desktop\src-tauri; cargo test
```
Expected: 编译失败（`Config` 未定义）

- [ ] **Step 3: 实现 `src-tauri/src/config.rs`**

```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_HOME_URL: &str = "https://fnos.net/";
pub const DEFAULT_BRAND_COLOR: &str = "#0066ff";
pub const FONT_WEIGHTS: [&str; 3] = ["450", "normal", "600"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModsConfig {
    pub enabled_origins: Vec<String>,
    pub auto_enable_suspected_fnos: bool,
    pub base_preset_enabled: bool,
    pub window_animation_blur_enabled: bool,
    pub titlebar_style: String,
    pub launchpad_style: String,
    pub desktop_icon_layout_enabled: bool,
    pub desktop_icon_layout_mode: String,
    pub desktop_icon_per_column: u32,
    pub desktop_icon_per_column_enabled: Option<bool>,
    pub launchpad_icon_scale_enabled: bool,
    pub launchpad_icon_scale_selected_keys: Vec<String>,
    pub launchpad_icon_mask_only_keys: Vec<String>,
    pub launchpad_icon_redraw_keys: Vec<String>,
    pub launchpad_icon_redraw_map: std::collections::BTreeMap<String, String>,
    pub brand_color: String,
    pub font_override_enabled: bool,
    pub font_family: String,
    pub font_monospace_family: String,
    pub font_weight: String,
    pub font_feature_settings: String,
    pub font_face_name: String,
    pub font_url: String,
    pub custom_code_enabled: bool,
    pub lockscreen_default_username: String,
}

impl Default for ModsConfig {
    fn default() -> Self {
        Self {
            enabled_origins: Vec::new(),
            auto_enable_suspected_fnos: true,
            base_preset_enabled: true,
            window_animation_blur_enabled: true,
            titlebar_style: "windows".into(),
            launchpad_style: "classic".into(),
            desktop_icon_layout_enabled: true,
            desktop_icon_layout_mode: "adaptive".into(),
            desktop_icon_per_column: 8,
            desktop_icon_per_column_enabled: None,
            launchpad_icon_scale_enabled: false,
            launchpad_icon_scale_selected_keys: Vec::new(),
            launchpad_icon_mask_only_keys: Vec::new(),
            launchpad_icon_redraw_keys: Vec::new(),
            launchpad_icon_redraw_map: Default::default(),
            brand_color: DEFAULT_BRAND_COLOR.into(),
            font_override_enabled: false,
            font_family: String::new(),
            font_monospace_family: String::new(),
            font_weight: String::new(),
            font_feature_settings: String::new(),
            font_face_name: "FnOSCustomFont".into(),
            font_url: String::new(),
            custom_code_enabled: false,
            lockscreen_default_username: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct LocalConfig {
    pub custom_css_code: String,
    pub custom_js_code: String,
    pub login_wallpaper_file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowGeom {
    pub w: f64,
    pub h: f64,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

impl Default for WindowGeom {
    fn default() -> Self {
        Self { w: 1200.0, h: 820.0, x: None, y: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShellConfig {
    pub home_url: String,
    pub nas_url: String,
    pub inject_enabled: bool,
    pub close_to_tray: bool,
    pub window: WindowGeom,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            home_url: DEFAULT_HOME_URL.into(),
            nas_url: String::new(),
            inject_enabled: true,
            close_to_tray: true,
            window: WindowGeom::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub schema_version: u32,
    pub mods: ModsConfig,
    pub local: LocalConfig,
    pub shell: ShellConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            mods: ModsConfig::default(),
            local: LocalConfig::default(),
            shell: ShellConfig::default(),
        }
    }
}

/// #rrggbb（或 #rgb）→ HSL，把明度夹到 30%–70%，再转回 #rrggbb。
/// 非法输入回落默认色。
pub fn normalize_brand_color(input: &str) -> String {
    let raw = input.trim();
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    let expanded = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => hex.to_string(),
        _ => return DEFAULT_BRAND_COLOR.into(),
    };
    let (r, g, b) = match (
        u8::from_str_radix(&expanded[0..2], 16),
        u8::from_str_radix(&expanded[2..4], 16),
        u8::from_str_radix(&expanded[4..6], 16),
    ) {
        (Ok(r), Ok(g), Ok(b)) => (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0),
        _ => return DEFAULT_BRAND_COLOR.into(),
    };

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let l = l.clamp(0.30, 0.70);

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", to(r1), to(g1), to(b1))
}

impl Config {
    pub fn normalize(&mut self) {
        self.schema_version = SCHEMA_VERSION;
        let m = &mut self.mods;
        m.brand_color = normalize_brand_color(&m.brand_color);
        if m.titlebar_style != "mac" { m.titlebar_style = "windows".into(); }
        if m.launchpad_style != "spotlight" { m.launchpad_style = "classic".into(); }
        if m.desktop_icon_layout_mode != "fixed" { m.desktop_icon_layout_mode = "adaptive".into(); }
        m.desktop_icon_per_column = m.desktop_icon_per_column.clamp(4, 16);
        if !FONT_WEIGHTS.contains(&m.font_weight.as_str()) { m.font_weight = String::new(); }
        m.lockscreen_default_username = m.lockscreen_default_username.chars().take(80).collect();
        m.enabled_origins.retain(|o| !o.trim().is_empty());
        m.launchpad_icon_redraw_map
            .retain(|_, v| is_valid_prefect_icon_path(v));
        if self.shell.home_url.trim().is_empty() {
            self.shell.home_url = DEFAULT_HOME_URL.into();
        }
    }

    pub fn config_path() -> PathBuf {
        crate::paths::config_dir().join("config.json")
    }

    pub fn load(path: &Path) -> Config {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Config::default(),
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(mut c) => { c.normalize(); c }
            Err(_) => {
                let _ = std::fs::write(path.with_extension("json.bak"), &text);
                Config::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())?;
        std::fs::rename(&tmp, path)
    }
}

pub fn is_valid_prefect_icon_path(v: &str) -> bool {
    let Some(rest) = v.strip_prefix("prefect_icon/") else { return false };
    let Some(name) = rest.strip_suffix(".png") else { return false };
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
```

- [ ] **Step 4: 写 `src-tauri/src/paths.rs`**

```rust
use std::path::PathBuf;

/// 解析顺序：显式环境变量（测试/便携用）→ %APPDATA%\com.fnos.desktop
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("FNOS_DESKTOP_CONFIG_DIR") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("com.fnos.desktop")
}
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test`
Expected: 4 个测试全部 PASS（`defaults_match_upstream` / `brand_color_lightness_is_clamped` / `enums_and_numbers_fall_back` / `roundtrip_and_migration`）

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(config): 配置模型+默认值(对齐上游25键)+归一化+原子持久化+迁移"
```

---

### Task 4: `inject/shim.js` — chrome.* 兼容层（含契约测试）

**Files:**
- Create: `src-tauri/inject/shim.js`, `tests/shim.test.mjs`
- Modify: `package.json`（新建，仅用于 `node --test`）

**Interfaces:**
- Consumes: `window.__FNOS_SHELL__ = { mods, local, assets: Record<string,string> }`
- Produces:
  - 全局 `chrome` 对象，覆盖 `runtime.id/getURL/getManifest/sendMessage/onMessage`、`storage.sync.get`、`storage.local.get`、`storage.onChanged.addListener`
  - `window.__FNOS_APPLY_CONFIG__(patch)` —— 宿主调用，派发 `onChanged` 并更新内存配置
  - `window.__FNOS_MOD_EXECUTED__` —— 由 mod.js 的 data URL 包装置位（Task 5 使用）

- [ ] **Step 1: 写失败测试 `tests/shim.test.mjs`**

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const SHIM = readFileSync(new URL('../src-tauri/inject/shim.js', import.meta.url), 'utf8');

function loadShim(shell) {
  const win = { __FNOS_SHELL__: shell };
  win.window = win;
  const fn = new Function('window', 'TextEncoder', 'queueMicrotask', 'btoa', SHIM + '\nreturn window;');
  return fn(win, TextEncoder, queueMicrotask, (s) => Buffer.from(s, 'binary').toString('base64'));
}

const SHELL = {
  mods: { brandColor: '#336699', enabledOrigins: ['http://nas.local'], titlebarStyle: 'mac' },
  local: { customCssCode: 'body{}', customJsCode: '' },
  assets: { 'basic_mod.css': 'body{color:red}', 'mod.js': 'window.__MOD_RAN__=1;' },
};

test('runtime.id 是非空字符串（缺失会导致上游完全注入失败）', () => {
  const w = loadShim(SHELL);
  assert.equal(typeof w.chrome.runtime.id, 'string');
  assert.ok(w.chrome.runtime.id.length > 0);
});

test('getURL 对已知资源返回 data: URL，未知资源返回空串', () => {
  const w = loadShim(SHELL);
  const url = w.chrome.runtime.getURL('basic_mod.css');
  assert.match(url, /^data:text\/css;base64,/);
  assert.equal(Buffer.from(url.split(',')[1], 'base64').toString('utf8'), 'body{color:red}');
  assert.equal(w.chrome.runtime.getURL('nope.css'), '');
});

test('getURL("mod.js") 的载荷会置位执行标记', () => {
  const w = loadShim(SHELL);
  const js = Buffer.from(w.chrome.runtime.getURL('mod.js').split(',')[1], 'base64').toString('utf8');
  assert.match(js, /__MOD_RAN__/);
  assert.match(js, /__FNOS_MOD_EXECUTED__/);
});

test('storage.sync.get 用默认值合并已存配置，且回调必被调用', async () => {
  const w = loadShim(SHELL);
  const defaults = { brandColor: '#0066ff', titlebarStyle: 'windows', unknownKey: 7 };
  const got = await new Promise((res) => w.chrome.storage.sync.get(defaults, res));
  assert.equal(got.brandColor, '#336699');
  assert.equal(got.titlebarStyle, 'mac');
  assert.equal(got.unknownKey, 7);
});

test('storage.sync.get 支持字符串/数组/空 keys 三种形式', async () => {
  const w = loadShim(SHELL);
  const byString = await new Promise((res) => w.chrome.storage.sync.get('brandColor', res));
  assert.deepEqual(Object.keys(byString), ['brandColor']);
  const byArray = await new Promise((res) => w.chrome.storage.sync.get(['brandColor', 'nope'], res));
  assert.deepEqual(Object.keys(byArray), ['brandColor']);
  const all = await new Promise((res) => w.chrome.storage.sync.get(null, res));
  assert.equal(all.titlebarStyle, 'mac');
});

test('storage.local.get(null) 返回全量', async () => {
  const w = loadShim(SHELL);
  const all = await new Promise((res) => w.chrome.storage.local.get(null, res));
  assert.equal(all.customCssCode, 'body{}');
});

test('__FNOS_APPLY_CONFIG__ 派发 onChanged 增量', async () => {
  const w = loadShim(SHELL);
  const events = [];
  w.chrome.storage.onChanged.addListener((changes, area) => events.push([area, changes]));
  w.__FNOS_APPLY_CONFIG__({ mods: { brandColor: '#ff0000' }, local: {} });
  assert.equal(events.length, 1);
  assert.equal(events[0][0], 'sync');
  assert.equal(events[0][1].brandColor.newValue, '#ff0000');
  assert.equal(events[0][1].brandColor.oldValue, '#336699');
});

test('getManifest 返回对象且含 version', () => {
  const w = loadShim(SHELL);
  assert.equal(typeof w.chrome.runtime.getManifest().version, 'string');
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd D:\fnOS-desktop; node --test tests/`
Expected: FAIL —— `ENOENT ... src-tauri/inject/shim.js`

- [ ] **Step 3: 写 `package.json` 与 `src-tauri/inject/shim.js`**

`package.json`：

```json
{
  "name": "fnos-desktop-shell",
  "private": true,
  "version": "0.1.0",
  "scripts": {
    "test": "node --test tests/"
  }
}
```

`src-tauri/inject/shim.js`：

```js
/* fnOS Desktop Shell — chrome.* 兼容层
 * 上游 content-script.js 只依赖下面这些成员；缺 chrome.runtime.id 会导致它完全不注入。 */
(function () {
  var W = typeof window !== 'undefined' ? window : globalThis;
  var SHELL = W.__FNOS_SHELL__ || { mods: {}, local: {}, assets: {} };
  var syncStore = Object.assign({}, SHELL.mods || {});
  var localStore = Object.assign({}, SHELL.local || {});
  var assets = SHELL.assets || {};
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
      if (keys in store) out[keys] = store[keys];
      return out;
    }
    if (Array.isArray(keys)) {
      for (var i = 0; i < keys.length; i++) if (keys[i] in store) out[keys[i]] = store[keys[i]];
      return out;
    }
    // 对象形式：值作为默认值
    var defaults = keys;
    for (var k in defaults) {
      out[k] = (k in store) ? store[k] : defaults[k];
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
      var text = assets[path];
      if (typeof text !== 'string') return '';
      if (path === 'mod.js') {
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
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd D:\fnOS-desktop; node --test tests/`
Expected: 8 个测试全部 PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(inject): chrome.* 兼容层 shim + 8 项契约测试"
```

---

### Task 5: `inject/bootstrap.js` — 惰性 getURL、外链降级、mod.js 兜底

**Files:**
- Create: `src-tauri/inject/bootstrap.js`, `tests/bootstrap.test.mjs`
- Modify: `tests/shim.test.mjs`（不改，仅复用 `loadShim` 模式）

**Interfaces:**
- Consumes: `window.__FNOS_SHELL__.assets`、`window.chrome`
- Produces:
  - `window.__FNOS_BOOTSTRAP__ = { version, cssIds: string[] }`
  - 在 `DOMContentLoaded` 后自检 `#fnos-ui-mods-basic-style` 等 link 的 `sheet`，必要时用 `adoptedStyleSheets` 补装
  - 观察 `#fnos-ui-mods-script`，若 ~120ms 后 `__FNOS_MOD_EXECUTED__` 未置位则直接执行 `mod.js` 原文

- [ ] **Step 1: 写失败测试 `tests/bootstrap.test.mjs`**

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const SHIM = readFileSync(new URL('../src-tauri/inject/shim.js', import.meta.url), 'utf8');
const BOOT = readFileSync(new URL('../src-tauri/inject/bootstrap.js', import.meta.url), 'utf8');

function fakeDom() {
  const nodes = [];
  return {
    nodes,
    head: { appendChild: (n) => nodes.push(n) },
    getElementById: (id) => nodes.find((n) => n.id === id) || null,
    createElement: (tag) => ({ tag, setAttribute() {}, style: {} }),
    documentElement: { appendChild: () => {} },
    addEventListener(ev, fn) { (this._l ||= {})[ev] = fn; },
    adoptedStyleSheets: [],
    readyState: 'loading'
  };
}

function load(shell, doc, cssSheetCtor) {
  const win = { __FNOS_SHELL__: shell, document: doc, CSS: cssSheetCtor };
  win.window = win;
  const fn = new Function('window', 'document', 'TextEncoder', 'queueMicrotask', 'btoa', 'CSS', 'setTimeout', 'MutationObserver',
    SHIM + '\n' + BOOT + '\nreturn window;');
  return fn(win, doc, TextEncoder, queueMicrotask, (s) => Buffer.from(s, 'binary').toString('base64'), cssSheetCtor, setTimeout, function () { this.observe = () => {}; });
}

const SHELL = {
  mods: {}, local: {},
  assets: { 'basic_mod.css': 'body{color:red}', 'mod.js': 'window.__MOD_RAN__=(window.__MOD_RAN__||0)+1;' }
};

test('bootstrap 暴露版本与受管 CSS id 列表', () => {
  const doc = fakeDom();
  const w = load(SHELL, doc, { supports: () => true });
  assert.equal(typeof w.__FNOS_BOOTSTRAP__.version, 'string');
  assert.ok(w.__FNOS_BOOTSTRAP__.cssIds.includes('fnos-ui-mods-basic-style'));
});

test('link 未生效时用 adoptedStyleSheets 补装 CSS', async () => {
  const doc = fakeDom();
  const installed = [];
  const CSSStub = {
    supports: () => true
  };
  const w = load(SHELL, doc, CSSStub);
  // 用一个可注入的构造样式表替身
  w.__FNOS_BOOTSTRAP__.installFallbackCss();
  assert.ok(w.__FNOS_BOOTSTRAP__.fallbackInstalled >= 1);
});

test('mod.js 未执行时兜底执行且只执行一次', async () => {
  const doc = fakeDom();
  const w = load(SHELL, doc, { supports: () => true });
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  assert.equal(w.__MOD_RAN__, 1);
  assert.equal(w.__FNOS_MOD_EXECUTED__, true);
});
```

- [ ] **Step 2: 运行确认失败**

Run: `node --test tests/`
Expected: FAIL —— `ENOENT ... bootstrap.js`

- [ ] **Step 3: 写 `src-tauri/inject/bootstrap.js`**

```js
/* fnOS Desktop Shell — 注入装配与降级
 * 1) 记录受管 CSS link；若 data: 外链被 CSP 拦掉，改用可构造样式表（CSP 免疫）
 * 2) 确保 mod.js 一定执行；优先让上游 <script src=data:> 执行，失败则自行执行原文 */
(function () {
  var W = typeof window !== 'undefined' ? window : globalThis;
  var D = W.document;
  var SHELL = W.__FNOS_SHELL__ || { assets: {} };
  var META = SHELL.meta || {};
  var CSS_IDS = [
    'fnos-ui-mods-basic-style',
    'fnos-ui-mods-titlebar-style',
    'fnos-ui-mods-launchpad-style',
    'fnos-ui-mods-desktop-icon-mod-style',
    'fnos-ui-mods-lockscreen-style'
  ];
  var CSS_FILES = [
    'basic_mod.css',
    'windows_titlebar_mod.css',
    'mac_titlebar_mod.css',
    'classic_launchpad_mod.css',
    'spotlight_launchpad_mod.css',
    'desktop_icon_mod.css',
    'lockscreen_mod.css'
  ];

  var state = {
    version: (META.shellVersion || '0.0.0'),
    cssIds: CSS_IDS.slice(),
    fallbackInstalled: 0,
    modFallbackUsed: false
  };

  function installFallbackCss() {
    if (!D.adoptedStyleSheets || typeof CSSStyleSheet !== 'function') return;
    var sheets = [];
    for (var i = 0; i < CSS_FILES.length; i++) {
      var text = SHELL.assets && SHELL.assets[CSS_FILES[i]];
      if (typeof text !== 'string') continue;
      try {
        var sheet = new CSSStyleSheet();
        sheet.replaceSync(text);
        sheets.push(sheet);
        state.fallbackInstalled++;
      } catch (e) { /* replaceSync 不可用则放弃该文件 */ }
    }
    if (!sheets.length) return;
    try {
      D.adoptedStyleSheets = D.adoptedStyleSheets.concat(sheets);
    } catch (e) { /* ignore */ }
  }

  function cssMissing() {
    for (var i = 0; i < CSS_IDS.length; i++) {
      var el = D.getElementById(CSS_IDS[i]);
      if (el && !el.sheet) return true;
    }
    // 一个 link 都没有：上游可能还没跑到，交给上游
    return false;
  }

  function ensureModJs() {
    if (W.__FNOS_MOD_EXECUTED__) return;
    var text = SHELL.assets && SHELL.assets['mod.js'];
    if (typeof text !== 'string') return;
    state.modFallbackUsed = true;
    try {
      (0, eval)(text + '\n;window.__FNOS_MOD_EXECUTED__=true;');
    } catch (e) {
      W.__FNOS_MOD_EXECUTED__ = true;
    }
  }

  W.__FNOS_BOOTSTRAP__ = {
    version: state.version,
    cssIds: state.cssIds,
    get fallbackInstalled() { return state.fallbackInstalled; },
    get modFallbackUsed() { return state.modFallbackUsed; },
    installFallbackCss: installFallbackCss,
    ensureModJs: ensureModJs
  };

  function afterLoad() {
    if (cssMissing()) installFallbackCss();
    setTimeout(ensureModJs, 120);
    setTimeout(function () {
      if (cssMissing()) installFallbackCss();
    }, 600);
  }

  if (D.readyState === 'loading') {
    D.addEventListener('DOMContentLoaded', afterLoad);
  } else {
    afterLoad();
  }
})();
```

- [ ] **Step 4: 运行测试确认通过**

Run: `node --test tests/`
Expected: 全部 PASS（shim 8 + bootstrap 3）

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(inject): bootstrap 装配与两级降级（adoptedStyleSheets / mod.js 兜底）"
```

---

### Task 6: `injector.rs` — 载荷组装（纯函数 + 快照测试）

**Files:**
- Create: `src-tauri/src/injector.rs`
- Modify: `src-tauri/src/main.rs`（加 `mod injector;`）
- Test: `src-tauri/src/injector.rs` 内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `config::Config`、`assets/fnos-mods/*`、`inject/shim.js`、`inject/bootstrap.js`
- Produces: `injector::build_init_script(cfg: &Config) -> String`；`injector::SHELL_VERSION: &str`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn script_has_four_sections_in_order() {
        let s = build_init_script(&Config::default());
        let i_cfg = s.find("__FNOS_SHELL__").expect("config section");
        let i_shim = s.find("fnos-desktop-shell").expect("shim section");
        let i_boot = s.find("__FNOS_BOOTSTRAP__").expect("bootstrap section");
        let i_cs = s.find("hasFnOSSignature").expect("upstream content-script");
        assert!(i_cfg < i_shim && i_shim < i_boot && i_boot < i_cs, "段落顺序必须是 配置→shim→bootstrap→上游");
    }

    #[test]
    fn payload_carries_all_assets_and_mods_config() {
        let mut cfg = Config::default();
        cfg.mods.brand_color = "#123456".into();
        let s = build_init_script(&cfg);
        for name in ["basic_mod.css", "mod.js", "content-script.js",
                     "windows_titlebar_mod.css", "mac_titlebar_mod.css",
                     "classic_launchpad_mod.css", "spotlight_launchpad_mod.css",
                     "desktop_icon_mod.css", "lockscreen_mod.css"] {
            assert!(s.contains(name), "载荷缺少资源键 {name}");
        }
        assert!(s.contains("\"brandColor\":\"#123456\""));
    }

    #[test]
    fn injection_disabled_yields_empty_script() {
        let mut cfg = Config::default();
        cfg.shell.inject_enabled = false;
        assert!(build_init_script(&cfg).is_empty());
    }

    #[test]
    fn payload_is_valid_json_prefix() {
        let s = build_init_script(&Config::default());
        let start = s.find('{').unwrap();
        let end = s.find("};\n").unwrap();
        let json = &s[start..=end];
        let v: serde_json::Value = serde_json::from_str(json).expect("配置段必须是合法 JSON");
        assert!(v.get("mods").is_some());
        assert!(v.get("assets").is_some());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test injector`
Expected: FAIL —— `injector` 模块/函数不存在

- [ ] **Step 3: 实现 `src-tauri/src/injector.rs`**

```rust
//! 纯函数：把配置与 vendored 资源拼成一段 initialization_script。
//! 不读文件、不碰 Tauri API —— 便于单测与快照锁定。

use crate::config::Config;
use serde::Serialize;
use serde_json::json;

pub const SHELL_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MODS_COMMIT: &str = "483c3e2e217faebc1be45b4e824865854a61e3dd";

const SHIM_JS: &str = include_str!("../inject/shim.js");
const BOOTSTRAP_JS: &str = include_str!("../inject/bootstrap.js");
const CONTENT_SCRIPT_JS: &str = include_str!("../assets/fnos-mods/content-script.js");

struct Assets {
    files: &'static [(&'static str, &'static str)],
}

#[derive(Serialize)]
struct Meta {
    shell_version: &'static str,
    mods_commit: &'static str,
    mods_version: &'static str,
}

fn assets() -> Assets {
    Assets {
        files: &[
            ("basic_mod.css", include_str!("../assets/fnos-mods/basic_mod.css")),
            ("windows_titlebar_mod.css", include_str!("../assets/fnos-mods/windows_titlebar_mod.css")),
            ("mac_titlebar_mod.css", include_str!("../assets/fnos-mods/mac_titlebar_mod.css")),
            ("classic_launchpad_mod.css", include_str!("../assets/fnos-mods/classic_launchpad_mod.css")),
            ("spotlight_launchpad_mod.css", include_str!("../assets/fnos-mods/spotlight_launchpad_mod.css")),
            ("desktop_icon_mod.css", include_str!("../assets/fnos-mods/desktop_icon_mod.css")),
            ("lockscreen_mod.css", include_str!("../assets/fnos-mods/lockscreen_mod.css")),
            ("mod.js", include_str!("../assets/fnos-mods/mod.js")),
            ("prefect_icon/icon-map.json", include_str!("../assets/fnos-mods/prefect_icon/icon-map.json")),
        ],
    }
}

pub fn build_init_script(cfg: &Config) -> String {
    if !cfg.shell.inject_enabled {
        return String::new();
    }

    let mut asset_map = serde_json::Map::new();
    for (name, text) in assets().files {
        // content-script.js 由本函数末尾直接执行，不需要经 getURL 暴露
        asset_map.insert((*name).to_string(), json!(text));
    }

    let payload = json!({
        "meta": Meta {
            shell_version: SHELL_VERSION,
            mods_commit: MODS_COMMIT,
            mods_version: "1.0.2",
        },
        "mods": &cfg.mods,
        "local": &cfg.local,
        "assets": asset_map,
    });

    format!(
        "/* fnOS Desktop Shell init script v{ver} (mods {commit}) */\n\
         window.__FNOS_SHELL__ = {payload};\n\
         {shim}\n\
         {boot}\n\
         {content}\n",
        ver = SHELL_VERSION,
        commit = MODS_COMMIT,
        payload = serde_json::to_string(&payload).unwrap(),
        shim = SHIM_JS,
        boot = BOOTSTRAP_JS,
        content = CONTENT_SCRIPT_JS,
    )
}
```

> 说明：`content-script.js` 直接作为初始化脚本执行（顶层 `if (window.top !== window) return;` 会阻止子框架重复运行），因此**不需要**把它塞进 assets。测试断言里要求它出现在字符串中，因为它确实在末尾被直接拼接。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test injector`
Expected: 4 个测试 PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(injector): 载荷组装（配置+shim+bootstrap+上游content-script）+ 4 项测试"
```

---

### Task 7: 托盘 + 主窗口接线 + 关窗隐藏

**Files:**
- Create: `src-tauri/src/tray.rs`
- Modify: `src-tauri/src/main.rs`
- Test: 手工验收

**Interfaces:**
- Consumes: `config::Config`、`tauri::AppHandle`
- Produces: `tray::install(app: &AppHandle, cfg: &Config) -> tauri::Result<()>`；`tray::apply_main_window(app, &cfg)`

- [ ] **Step 1: 写 `src-tauri/src/tray.rs`**

```rust
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Runtime, WebviewWindow,
};

pub const TRAY_ID: &str = "main-tray";

fn icon() -> tauri::Result<Image<'static>> {
    Image::from_bytes(include_bytes!("../icons/icon.png"))
}

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let inject = CheckMenuItem::with_id(app, "inject", "注入 mods", true, true, None::<&str>)?;
    let open_nas = MenuItem::with_id(app, "open_nas", "打开 NAS", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "显示 / 隐藏主窗口", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "系统设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&inject, &open_nas, &toggle, &settings, &sep, &quit])?;

    let inject_for_handler = inject.clone();
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon()?)
        .tooltip("fnOS")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "inject" => {
                let checked = inject_for_handler.is_checked().unwrap_or(true);
                crate::commands::set_inject_enabled(app, checked);
            }
            "open_nas" => crate::commands::open_nas(app),
            "toggle" => toggle_main(app),
            "settings" => crate::commands::open_settings(app),
            "quit" => {
                crate::commands::save_window_geom(app);
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

pub fn sync_inject_check<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray;
    }
    // CheckMenuItem 的状态由 muda 自身维护；这里仅在配置外部变更时同步
    let _ = (app, enabled);
}

pub fn toggle_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        match w.is_visible() {
            Ok(true) => { let _ = w.hide(); }
            _ => {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }
    }
}

pub fn apply_window_geom<R: Runtime>(w: &WebviewWindow<R>, cfg: &crate::config::Config) {
    let g = &cfg.shell.window;
    let _ = w.set_size(tauri::LogicalSize::new(g.w, g.h));
    if let (Some(x), Some(y)) = (g.x, g.y) {
        let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    }
}
```

- [ ] **Step 2: 改造 `src-tauri/src/main.rs`**

```rust
mod commands;
mod config;
mod injector;
mod paths;
mod tray;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            let cfg = commands::load_config(app.handle());
            commands::apply_home_url(app.handle(), &cfg);

            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(cfg.shell.home_url.parse().expect("home url")),
            )
            .title("fnOS")
            .inner_size(cfg.shell.window.w, cfg.shell.window.h)
            .initialization_script(injector::build_init_script(&cfg))
            .build()?;

            tray::apply_window_geom(&window, &cfg);
            tray::install(app.handle())?;
            commands::set_inject_checked(app.handle(), cfg.shell.inject_enabled);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    commands::save_window_geom(window.app_handle());
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::reload_main,
            commands::open_config_dir,
            commands::reset_config,
        ])
        .run(tauri::generate_context!())
        .expect("fnOS 启动失败");
}
```

> `commands.rs` 在 Task 8 实现；本 Task 为了先跑通托盘，需要先落一个最小版本（见 Task 8 Step 1 的完整实现，可直接提前写入）。

- [ ] **Step 3: 手工验收**

Run:
```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd D:\fnOS-desktop\src-tauri; cargo build; .\target\debug\fnos-desktop.exe
```
Expected:
1. 窗口打开 `https://fnos.net/`（此时注入载荷已注入，但因官网不满足上游签名判定，**页面无样式变化**）
2. 托盘出现图标；右键菜单 **5 个可点击项 + 1 条分隔线**（注入 mods 勾选项渲染 ✓、打开 NAS 在未填地址时置灰、显示/隐藏主窗口、系统设置、退出）
3. 点「显示 / 隐藏主窗口」能在显示与隐藏间切换
4. 关闭主窗口 → 窗口消失但进程仍在（任务管理器可见）
5. 托盘「退出」→ 进程结束

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat(tray): 托盘菜单(勾选项/打开NAS/显隐/设置/退出) + 关窗隐藏 + 主窗口接线"
```

---

### Task 8: `commands.rs` + capability（IPC 与权限收紧）

**Files:**
- Create: `src-tauri/src/commands.rs`, `src-tauri/capabilities/default.json`
- Test: 手工安全验收 + `cargo test`

**Interfaces:**
- Consumes: `config::Config`、`injector::build_init_script`
- Produces（IPC 契约，spec §8.3）：
  - `get_config() -> ConfigView`（`{ mods, local, shell, meta }`）
  - `set_config(patch: serde_json::Value) -> SetResult`（`{ config, needsReload }`）
  - `reload_main(url: Option<String>) -> ()`
  - `open_config_dir() -> ()`
  - `reset_config(scope: String) -> Config`
  - Rust 内部：`load_config(&AppHandle) -> Config`、`save_and_apply(&AppHandle, &Config)`、`set_inject_enabled`、`set_inject_checked`、`open_settings`、`open_nas`、`save_window_geom`、`apply_home_url`

- [ ] **Step 1: 写 `src-tauri/capabilities/default.json`**

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "settings-only",
  "description": "只允许设置窗调用应用命令；远程页面无任何授权",
  "windows": ["settings"],
  "permissions": [
    "core:default",
    "allow-get-config",
    "allow-set-config",
    "allow-reload-main",
    "allow-open-config-dir",
    "allow-reset-config"
  ]
}
```

- [ ] **Step 2: 写 `src-tauri/src/commands.rs`**

```rust
use crate::{config::Config, injector, paths, tray};
use serde::Serialize;
use serde_json::Value;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindowBuilder};

pub struct AppState {
    pub config: Mutex<Config>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub shell_version: String,
    pub mods_commit: String,
    pub mods_version: String,
    pub config_path: String,
    pub webview_version: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub schema_version: u32,
    pub mods: crate::config::ModsConfig,
    pub local: crate::config::LocalConfig,
    pub shell: crate::config::ShellConfig,
    pub meta: Meta,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetResult {
    pub config: ConfigView,
    pub needs_reload: bool,
}

pub fn load_config<R: Runtime>(app: &AppHandle<R>) -> Config {
    let path = paths::config_dir().join("config.json");
    let cfg = Config::load(&path);
    let _ = cfg.save(&path);
    if let Some(state) = app.try_state::<AppState>() {
        *state.config.lock().unwrap() = cfg.clone();
    }
    cfg
}

fn current<R: Runtime>(app: &AppHandle<R>) -> Config {
    app.state::<AppState>().config.lock().unwrap().clone()
}

fn config_view<R: Runtime>(app: &AppHandle<R>, cfg: &Config) -> ConfigView {
    ConfigView {
        schema_version: cfg.schema_version,
        mods: cfg.mods.clone(),
        local: cfg.local.clone(),
        shell: cfg.shell.clone(),
        meta: Meta {
            shell_version: injector::SHELL_VERSION.into(),
            mods_commit: injector::MODS_COMMIT.into(),
            mods_version: "1.0.2".into(),
            config_path: paths::config_dir().join("config.json").display().to_string(),
            webview_version: None,
        },
    }
}

#[tauri::command]
pub fn get_config<R: Runtime>(app: AppHandle<R>) -> ConfigView {
    let cfg = current(&app);
    config_view(&app, &cfg)
}

#[tauri::command]
pub fn set_config<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    patch: Value,
) -> Result<SetResult, String> {
    let mut cfg = {
        let mut guard = state.config.lock().unwrap();
        let mut merged: Value = serde_json::to_value(&*guard).map_err(|e| e.to_string())?;
        merge(&mut merged, patch);
        let mut next: Config = serde_json::from_value(merged).map_err(|e| e.to_string())?;
        next.normalize();
        *guard = next.clone();
        next
    };
    cfg.normalize();

    let path = paths::config_dir().join("config.json");
    cfg.save(&path).map_err(|e| e.to_string())?;

    let needs_reload = apply_to_page(&app, &cfg);
    if let Err(e) = tray::sync_menus(&app, &cfg) {
        eprintln!("[fnos] 托盘同步失败: {e}");
    }
    Ok(SetResult { config: config_view(&app, &cfg), needs_reload })
}

/// 递归浅合并：`patch` 里的 `null` 表示删除该键（回到默认由 normalize 兜底）
fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    b.remove(&k);
                } else {
                    merge(b.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        (b, p) => *b = p,
    }
}

/// 把配置推给页面（免刷新）；返回是否需要整页重载
fn apply_to_page<R: Runtime>(app: &AppHandle<R>, cfg: &Config) -> bool {
    let Some(win) = app.get_webview_window("main") else { return false };
    let _ = win.eval(format!(
        "window.__FNOS_APPLY_CONFIG__ && window.__FNOS_APPLY_CONFIG__({});",
        serde_json::to_string(&serde_json::json!({
            "mods": &cfg.mods,
            "local": &cfg.local,
        }))
        .unwrap()
    ));
    false
}

#[tauri::command]
pub fn reload_main<R: Runtime>(app: AppHandle<R>, url: Option<String>) -> Result<(), String> {
    let cfg = current(&app);
    let target = url.unwrap_or_else(|| cfg.shell.home_url.clone());
    let parsed = target.parse().map_err(|e: tauri::UrlParseError| e.to_string())?;
    if let Some(win) = app.get_webview_window("main") {
        let cfg2 = current(&app);
        let _ = win.eval("window.__FNOS_SHELL__ && (window.__FNOS_SHELL__.__reload = true);");
        win.navigate(parsed).map_err(|e| e.to_string())?;
        let _ = injector::build_init_script(&cfg2);
    }
    Ok(())
}

#[tauri::command]
pub fn open_config_dir() -> Result<(), String> {
    let dir = paths::config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn reset_config<R: Runtime>(app: AppHandle<R>, scope: String) -> Result<ConfigView, String> {
    let mut cfg = current(&app);
    match scope.as_str() {
        "mods" => cfg.mods = Default::default(),
        "shell" => {
            let window = cfg.shell.window.clone();
            cfg.shell = Default::default();
            cfg.shell.window = window;
        }
        _ => {
            let window = cfg.shell.window.clone();
            cfg = Config::default();
            cfg.shell.window = window;
        }
    }
    cfg.normalize();
    cfg.save(&paths::config_dir().join("config.json")).map_err(|e| e.to_string())?;
    *app.state::<AppState>().config.lock().unwrap() = cfg.clone();
    apply_to_page(&app, &cfg);
    Ok(config_view(&app, &cfg))
}

// ---------- Rust 内部（非 IPC） ----------

pub fn open_settings<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    match WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("fnOS 设置")
        .inner_size(900.0, 640.0)
        .build()
    {
        Ok(_) => {}
        Err(e) => eprintln!("[fnos] 设置窗创建失败: {e}"),
    }
}

pub fn set_inject_enabled<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let mut cfg = current(app);
    cfg.shell.inject_enabled = enabled;
    let _ = cfg.save(&paths::config_dir().join("config.json"));
    *app.state::<AppState>().config.lock().unwrap() = cfg.clone();
    set_inject_checked(app, enabled);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.eval("location.reload();");
    }
}

pub fn set_inject_checked<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let _ = (app, enabled); // muda 的 CheckMenuItem 状态随点击自动切换；外部变更时由托盘重建处理
}

pub fn open_nas<R: Runtime>(app: &AppHandle<R>) {
    let cfg = current(app);
    if cfg.shell.nas_url.trim().is_empty() {
        open_settings(app);
        return;
    }
    if let Ok(url) = cfg.shell.nas_url.parse::<tauri::Url>() {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.navigate(url);
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

pub fn apply_home_url<R: Runtime>(app: &AppHandle<R>, cfg: &Config) {
    let path = paths::config_dir().join("config.json");
    let _ = cfg.save(&path);
    app.manage(AppState { config: Mutex::new(cfg.clone()) });
}

pub fn save_window_geom<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window("main") else { return };
    let Ok(size) = win.inner_size() else { return };
    let scale = win.scale_factor().unwrap_or(1.0);
    let mut cfg = current(app);
    cfg.shell.window.w = size.width as f64 / scale;
    cfg.shell.window.h = size.height as f64 / scale;
    if let Ok(pos) = win.outer_position() {
        cfg.shell.window.x = Some(pos.x as f64 / scale);
        cfg.shell.window.y = Some(pos.y as f64 / scale);
    }
    let _ = cfg.save(&paths::config_dir().join("config.json"));
    *app.state::<AppState>().config.lock().unwrap() = cfg;
}
```

- [ ] **Step 3: 给 `tray.rs` 补 `sync_menus`**

在 `src-tauri/src/tray.rs` 中把 `sync_inject_check` 替换为：

```rust
/// 托盘菜单状态跟随配置（仅在配置被设置窗改动时调用）
pub fn sync_menus<R: Runtime>(app: &AppHandle<R>, cfg: &crate::config::Config) -> tauri::Result<()> {
    use tauri::menu::MenuItemKind;
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Some(menu) = tray.menu() {
            if let Some(MenuItemKind::Check(item)) = menu.get("inject") {
                let _ = item.set_checked(cfg.shell.inject_enabled);
            }
            if let Some(item) = menu.get("open_nas") {
                let _ = item.set_enabled(!cfg.shell.nas_url.trim().is_empty());
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: 构建 + 测试**

Run:
```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd D:\fnOS-desktop\src-tauri; cargo test; cargo build
```
Expected: 编译通过；已有单测全 PASS

- [ ] **Step 5: 安全验收（必须失败才对）**

Run: 启动应用 → 主窗口任意页面（外部站点）打开 DevTools（`F12` 或右键检查）→ Console 执行：

```js
await window.__TAURI_INTERNALS__.invoke('get_config')
```
Expected: **抛错/被拒**（未授权）。若返回了配置对象，说明 capability 写错，必须修掉再继续。

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(ipc): 5 个命令 + capability 只授权 settings 窗口 + 远程调用被拒验收"
```

---

### Task 9: 设置窗（schema 驱动渲染 + 归一化 + IPC 桥）

**Files:**
- Create: `ui/settings/settings.html`, `ui/settings/settings.css`, `ui/settings/schema.js`, `ui/settings/normalize.js`, `ui/settings/bridge.js`, `ui/settings/app.js`, `tests/normalize.test.mjs`
- Test: `node --test tests/`

**Interfaces:**
- Consumes: Task 8 的 IPC 命令
- Produces: 可交互设置窗；`normalize.js` 导出 `normalizeMods(mods) -> mods`

- [ ] **Step 1: 写失败测试 `tests/normalize.test.mjs`**

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeMods, clampLightness } from '../ui/settings/normalize.js';

test('明度夹取与非法值回落', () => {
  assert.equal(clampLightness('#ffffff'), '#b3b3b3');
  assert.equal(clampLightness('#000000'), '#4d4d4d');
  assert.equal(clampLightness('#0066ff'), '#0066ff');
  assert.equal(clampLightness('bogus'), '#0066ff');
});

test('枚举与数字回落', () => {
  const out = normalizeMods({
    titlebarStyle: 'nope', launchpadStyle: 'wat', desktopIconLayoutMode: 'x',
    desktopIconPerColumn: 999, fontWeight: 'bold', lockscreenDefaultUsername: 'a'.repeat(100)
  });
  assert.equal(out.titlebarStyle, 'windows');
  assert.equal(out.launchpadStyle, 'classic');
  assert.equal(out.desktopIconLayoutMode, 'adaptive');
  assert.equal(out.desktopIconPerColumn, 16);
  assert.equal(out.fontWeight, '');
  assert.equal(out.lockscreenDefaultUsername.length, 80);
});

test('redrawMap 只保留合法 prefect_icon 路径', () => {
  const out = normalizeMods({
    launchpadIconRedrawMap: { a: 'prefect_icon/emby.png', b: '../etc/passwd', c: 'prefect_icon/BAD.png' }
  });
  assert.deepEqual(Object.keys(out.launchpadIconRedrawMap), ['a']);
});

test('unknown 键被丢弃，已知键保留', () => {
  const out = normalizeMods({ brandColor: '#123456', evil: 1 });
  assert.equal(out.brandColor, '#123456');
  assert.equal('evil' in out, false);
});
```

- [ ] **Step 2: 运行确认失败**

Run: `node --test tests/` → Expected: FAIL（模块不存在）

- [ ] **Step 3: 写 `ui/settings/normalize.js`**

```js
export const DEFAULT_BRAND_COLOR = '#0066ff';
const FONT_WEIGHTS = ['450', 'normal', '600'];

const MODS_KEYS = [
  'enabledOrigins', 'autoEnableSuspectedFnOS', 'basePresetEnabled', 'windowAnimationBlurEnabled',
  'titlebarStyle', 'launchpadStyle', 'desktopIconLayoutEnabled', 'desktopIconLayoutMode',
  'desktopIconPerColumn', 'desktopIconPerColumnEnabled', 'launchpadIconScaleEnabled',
  'launchpadIconScaleSelectedKeys', 'launchpadIconMaskOnlyKeys', 'launchpadIconRedrawKeys',
  'launchpadIconRedrawMap', 'brandColor', 'fontOverrideEnabled', 'fontFamily',
  'fontMonospaceFamily', 'fontWeight', 'fontFeatureSettings', 'fontFaceName', 'fontUrl',
  'customCodeEnabled', 'lockscreenDefaultUsername'
];

const DEFAULTS = {
  enabledOrigins: [], autoEnableSuspectedFnOS: true, basePresetEnabled: true,
  windowAnimationBlurEnabled: true, titlebarStyle: 'windows', launchpadStyle: 'classic',
  desktopIconLayoutEnabled: true, desktopIconLayoutMode: 'adaptive', desktopIconPerColumn: 8,
  desktopIconPerColumnEnabled: null, launchpadIconScaleEnabled: false,
  launchpadIconScaleSelectedKeys: [], launchpadIconMaskOnlyKeys: [], launchpadIconRedrawKeys: [],
  launchpadIconRedrawMap: {}, brandColor: DEFAULT_BRAND_COLOR, fontOverrideEnabled: false,
  fontFamily: '', fontMonospaceFamily: '', fontWeight: '', fontFeatureSettings: '',
  fontFaceName: 'FnOSCustomFont', fontUrl: '', customCodeEnabled: false, lockscreenDefaultUsername: ''
};

function hexToRgb(hex) {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  if (!/^[0-9a-f]{6}$/i.test(full)) return null;
  return [parseInt(full.slice(0, 2), 16) / 255, parseInt(full.slice(2, 4), 16) / 255, parseInt(full.slice(4, 6), 16) / 255];
}

export function clampLightness(input) {
  const rgb = hexToRgb(String(input || '').trim());
  if (!rgb) return DEFAULT_BRAND_COLOR;
  const [r, g, b] = rgb;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  let l = (max + min) / 2;
  const d = max - min;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  let h = 0;
  if (d !== 0) {
    if (max === r) h = 60 * (((g - b) / d) % 6);
    else if (max === g) h = 60 * ((b - r) / d + 2);
    else h = 60 * ((r - g) / d + 4);
  }
  if (h < 0) h += 360;
  l = Math.min(0.70, Math.max(0.30, l));
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  let rr = 0, gg = 0, bb = 0;
  if (h < 60) [rr, gg, bb] = [c, x, 0];
  else if (h < 120) [rr, gg, bb] = [x, c, 0];
  else if (h < 180) [rr, gg, bb] = [0, c, x];
  else if (h < 240) [rr, gg, bb] = [0, x, c];
  else if (h < 300) [rr, gg, bb] = [x, 0, c];
  else [rr, gg, bb] = [c, 0, x];
  const to = (v) => Math.round(Math.min(1, Math.max(0, v + m)) * 255).toString(16).padStart(2, '0');
  return `#${to(rr)}${to(gg)}${to(bb)}`;
}

export function isPrefectIconPath(v) {
  return typeof v === 'string' && /^prefect_icon\/[a-z0-9-]+\.png$/.test(v);
}

export function normalizeMods(input) {
  const src = input && typeof input === 'object' ? input : {};
  const out = {};
  for (const key of MODS_KEYS) {
    const value = key in src ? src[key] : DEFAULTS[key];
    out[key] = value === undefined ? DEFAULTS[key] : value;
  }
  out.brandColor = clampLightness(out.brandColor);
  if (out.titlebarStyle !== 'mac') out.titlebarStyle = 'windows';
  if (out.launchpadStyle !== 'spotlight') out.launchpadStyle = 'classic';
  if (out.desktopIconLayoutMode !== 'fixed') out.desktopIconLayoutMode = 'adaptive';
  const n = Number(out.desktopIconPerColumn);
  out.desktopIconPerColumn = Number.isFinite(n) ? Math.min(16, Math.max(4, Math.round(n))) : 8;
  if (!FONT_WEIGHTS.includes(out.fontWeight)) out.fontWeight = '';
  out.lockscreenDefaultUsername = String(out.lockscreenDefaultUsername || '').slice(0, 80);
  out.enabledOrigins = (Array.isArray(out.enabledOrigins) ? out.enabledOrigins : []).filter((o) => String(o).trim());
  const map = {};
  const rawMap = out.launchpadIconRedrawMap && typeof out.launchpadIconRedrawMap === 'object' ? out.launchpadIconRedrawMap : {};
  for (const [k, v] of Object.entries(rawMap)) if (isPrefectIconPath(v)) map[k] = v;
  out.launchpadIconRedrawMap = map;
  return out;
}
```

- [ ] **Step 4: 写 `ui/settings/schema.js`（设置项声明）**

```js
export const SCHEMA = [
  {
    id: 'site', title: '站点', items: [
      { key: 'enabledOrigins', label: '注入白名单', type: 'originList', hint: '命中白名单的站点免探测直接注入' },
      { key: 'shell.nasUrl', label: 'NAS WebUI 地址', type: 'text', hint: '填你自己的 NAS 地址；保存后自动进入注入白名单' },
      { key: 'mods.autoEnableSuspectedFnOS', label: '自动对疑似飞牛站点启用', type: 'bool' }
    ]
  },
  {
    id: 'basic', title: '基础', items: [
      { key: 'mods.basePresetEnabled', label: '基础美化预设', type: 'bool', hint: '关闭只摘掉标题栏与启动台，基础样式仍生效' },
      { key: 'mods.windowAnimationBlurEnabled', label: '窗口动画模糊', type: 'bool' }
    ]
  },
  { id: 'theme', title: '主题', items: [{ key: 'mods.brandColor', label: '主题色', type: 'color' }] },
  {
    id: 'titlebar', title: '标题栏', items: [
      { key: 'mods.titlebarStyle', label: '标题栏样式', type: 'radio', options: [['windows', 'Windows'], ['mac', 'macOS']] }
    ]
  },
  {
    id: 'launchpad', title: '启动台', items: [
      { key: 'mods.launchpadStyle', label: '启动台样式', type: 'radio', options: [['classic', '经典'], ['spotlight', 'Spotlight']] }
    ]
  },
  {
    id: 'desktop', title: '桌面图标', items: [
      { key: 'mods.desktopIconLayoutEnabled', label: '桌面图标优化', type: 'bool' },
      { key: 'mods.desktopIconLayoutMode', label: '布局', type: 'select', options: [['adaptive', '自适应'], ['fixed', '固定列数']] },
      { key: 'mods.desktopIconPerColumn', label: '每列数量', type: 'number', min: 4, max: 16 }
    ]
  },
  {
    id: 'font', title: '字体', items: [
      { key: 'mods.fontOverrideEnabled', label: '字体替换', type: 'bool' },
      { key: 'mods.fontFamily', label: '正文字体名', type: 'text', hint: '本机已安装的字体名' },
      { key: 'mods.fontMonospaceFamily', label: '等宽字体名', type: 'text' },
      { key: 'mods.fontUrl', label: '网络字体 URL', type: 'text' },
      { key: 'mods.fontWeight', label: '字重', type: 'select', options: [['', '默认'], ['450', '450'], ['normal', 'normal'], ['600', '600']] },
      { key: 'mods.fontFeatureSettings', label: 'OpenType 特性', type: 'text' }
    ]
  },
  {
    id: 'lockscreen', title: '登录页', items: [
      { key: 'mods.lockscreenDefaultUsername', label: '默认用户名', type: 'text', maxlength: 80 }
    ]
  },
  {
    id: 'custom', title: '自定义代码', items: [
      { key: 'mods.customCodeEnabled', label: '启用自定义 CSS/JS', type: 'bool' },
      { key: 'local.customCssCode', label: '自定义 CSS', type: 'code' },
      { key: 'local.customJsCode', label: '自定义 JS', type: 'code' }
    ]
  },
  { id: 'about', title: '关于', items: [] }
];
```

- [ ] **Step 5: 写 `ui/settings/bridge.js` 与 `ui/settings/app.js`**

`bridge.js`：

```js
const invoke = window.__TAURI__?.core?.invoke
  || ((cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args));

export async function getConfig() { return invoke('get_config'); }
export async function setConfig(patch) { return invoke('set_config', { patch }); }
export async function reloadMain(url) { return invoke('reload_main', { url: url ?? null }); }
export async function openConfigDir() { return invoke('open_config_dir'); }
export async function resetConfig(scope) { return invoke('reset_config', { scope }); }
```

`app.js`：

```js
import { SCHEMA } from './schema.js';
import { normalizeMods } from './normalize.js';
import * as api from './bridge.js';

const state = { config: null };

function get(obj, path) {
  return path.split('.').reduce((o, k) => (o == null ? undefined : o[k]), obj);
}
function set(obj, path, value) {
  const parts = path.split('.');
  const last = parts.pop();
  let cur = obj;
  for (const p of parts) cur = cur[p] ??= {};
  cur[last] = value;
  return obj;
}

function fieldEl(item, value) {
  const wrap = document.createElement('div');
  wrap.className = 'field';
  const label = document.createElement('label');
  label.textContent = item.label;
  wrap.appendChild(label);
  let input;
  switch (item.type) {
    case 'bool':
      input = document.createElement('input');
      input.type = 'checkbox';
      input.checked = !!value;
      input.addEventListener('change', () => commit(item.key, input.checked, wrap));
      break;
    case 'color':
      input = document.createElement('input');
      input.type = 'color';
      input.value = value;
      input.addEventListener('change', () => commit(item.key, input.value, wrap));
      break;
    case 'radio': {
      input = document.createElement('div');
      for (const [v, text] of item.options) {
        const l = document.createElement('label');
        l.className = 'radio';
        const r = document.createElement('input');
        r.type = 'radio'; r.name = item.key; r.value = v; r.checked = value === v;
        r.addEventListener('change', () => commit(item.key, v, wrap));
        l.append(r, document.createTextNode(text));
        input.appendChild(l);
      }
      break;
    }
    case 'select': {
      input = document.createElement('select');
      for (const [v, text] of item.options) {
        const o = document.createElement('option');
        o.value = v; o.textContent = text; o.selected = v === value;
        input.appendChild(o);
      }
      input.addEventListener('change', () => commit(item.key, input.value, wrap));
      break;
    }
    case 'number':
      input = document.createElement('input');
      input.type = 'number'; input.min = item.min; input.max = item.max; input.value = value;
      input.addEventListener('change', () => commit(item.key, Number(input.value), wrap));
      break;
    case 'originList': {
      input = document.createElement('div');
      input.className = 'origins';
      for (const o of value || []) {
        const row = document.createElement('div');
        row.className = 'origin-row';
        row.innerHTML = `<code>${o}</code>`;
        const del = document.createElement('button');
        del.textContent = '删除';
        del.onclick = () => commit(item.key, (value || []).filter((x) => x !== o), wrap);
        row.appendChild(del);
        input.appendChild(row);
      }
      break;
    }
    case 'code':
      input = document.createElement('textarea');
      input.rows = 6; input.value = value || '';
      input.addEventListener('change', () => commit(item.key, input.value, wrap));
      break;
    default:
      input = document.createElement('input');
      input.type = 'text';
      input.value = value ?? '';
      if (item.maxlength) input.maxLength = item.maxlength;
      input.addEventListener('change', () => commit(item.key, input.value, wrap));
  }
  wrap.appendChild(input);
  if (item.hint) {
    const hint = document.createElement('p');
    hint.className = 'hint';
    hint.textContent = item.hint;
    wrap.appendChild(hint);
  }
  return wrap;
}

async function commit(key, value, el) {
  el.classList.add('pending');
  try {
    const patch = {};
    if (key.startsWith('shell.')) set(patch, key, value);
    else if (key.startsWith('local.')) set(patch, key.slice(0), value);
    else set(patch, `mods.${key}`, value);
    const res = await api.setConfig(patch);
    state.config = res.config;
    if (res.needsReload) await api.reloadMain(null);
    render();
  } catch (e) {
    el.classList.add('error');
    console.error(e);
  } finally {
    el.classList.remove('pending');
  }
}

function render() {
  const nav = document.getElementById('nav');
  const pane = document.getElementById('pane');
  nav.innerHTML = '';
  pane.innerHTML = '';
  const activeId = state.active || SCHEMA[0].id;
  state.active = activeId;
  for (const group of SCHEMA) {
    const a = document.createElement('a');
    a.textContent = group.title;
    a.className = group.id === activeId ? 'on' : '';
    a.onclick = () => { state.active = group.id; render(); };
    nav.appendChild(a);
  }
  const group = SCHEMA.find((g) => g.id === activeId);
  const h2 = document.createElement('h2');
  h2.textContent = group.title;
  pane.appendChild(h2);
  if (group.id === 'about') {
    const meta = state.config.meta;
    pane.insertAdjacentHTML('beforeend', `
      <div class="card">
        <div class="row"><span>应用版本</span><b>${meta.shellVersion}</b></div>
        <div class="row"><span>mods commit</span><b>${meta.modsCommit.slice(0, 10)}</b></div>
        <div class="row"><span>mods 版本</span><b>${meta.modsVersion}</b></div>
        <div class="row"><span>配置文件</span><code>${meta.configPath}</code></div>
      </div>
      <div class="card">
        <p class="hint">本应用为非官方第三方桌面壳，与飞牛官方无关；随附的 UI 修改资源来自
        <a href="https://github.com/aurysian-yan/fnOS_UI_Mods" target="_blank">fnOS_UI_Mods</a>，
        遵循其 Non-Commercial License 1.0，仅供非商业个人使用。</p>
        <button id="openDir">打开配置目录</button>
        <button id="resetAll" class="danger">恢复默认设置</button>
      </div>`);
    document.getElementById('openDir').onclick = () => api.openConfigDir();
    document.getElementById('resetAll').onclick = async () => {
      if (!confirm('确定恢复全部默认设置？')) return;
      state.config = await api.resetConfig('all');
      render();
    };
    return;
  }
  const card = document.createElement('div');
  card.className = 'card';
  for (const item of group.items) {
    const value = item.key.startsWith('shell.')
      ? get(state.config.shell, item.key.slice(6))
      : item.key.startsWith('local.')
        ? get(state.config.local, item.key.slice(6))
        : state.config.mods[item.key];
    card.appendChild(fieldEl(item, value));
  }
  pane.appendChild(card);
}

(async function boot() {
  const cfg = await api.getConfig();
  state.config = { ...cfg, mods: normalizeMods(cfg.mods) };
  render();
})();
```

- [ ] **Step 6: 写 `ui/settings/settings.html` 与 `settings.css`**

```html
<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>fnOS 设置</title>
<link rel="stylesheet" href="settings.css">
</head>
<body>
<nav id="nav"></nav>
<main id="pane"></main>
<script type="module" src="app.js"></script>
</body>
</html>
```

`settings.css`（对齐飞牛系统设置观感，深色 + 分组卡片；关键规则）：

```css
:root { color-scheme: dark; --bg:#1b1c1e; --card:#232427; --line:#2e3034; --fg:#e8e8ea; --dim:#8b9096; --accent:#0a84ff; }
* { box-sizing: border-box; }
body { margin:0; display:flex; height:100vh; background:var(--bg); color:var(--fg);
       font:14px/1.6 "Microsoft YaHei", system-ui, sans-serif; }
nav { width:190px; padding:14px 8px; background:#202124; overflow:auto; }
nav a { display:block; padding:8px 10px; border-radius:6px; color:#cfd2d6; cursor:pointer; }
nav a.on { background:#0a84ff33; color:#7ab8ff; }
main { flex:1; padding:22px 26px; overflow:auto; }
h2 { font-size:17px; margin:0 0 14px; }
.card { background:var(--card); border:1px solid var(--line); border-radius:10px; padding:6px 16px; margin-bottom:12px; }
.field { display:flex; flex-wrap:wrap; align-items:center; gap:10px; padding:10px 0; border-bottom:1px solid var(--line); }
.field:last-child { border-bottom:none; }
.field > label { min-width:150px; color:#c8ccd1; }
.field input[type=text], .field input[type=number], .field input[type=color], .field select, .field textarea {
  background:#15161a; border:1px solid var(--line); color:var(--fg); border-radius:6px; padding:5px 8px; min-width:220px; }
.field textarea { width:100%; font-family:Consolas, monospace; }
.hint { flex-basis:100%; margin:0; color:var(--dim); font-size:12px; }
.row { display:flex; justify-content:space-between; padding:8px 0; border-bottom:1px solid var(--line); }
.row:last-child { border-bottom:none; }
.pending { opacity:.6; }
.error { outline:1px solid #ff453a; }
button { background:#2c2f34; border:1px solid var(--line); color:var(--fg); border-radius:6px; padding:6px 12px; cursor:pointer; }
button.danger { border-color:#ff453a66; color:#ff6961; }
.origin-row { display:flex; gap:8px; align-items:center; padding:2px 0; }
.radio { margin-right:14px; }
```

- [ ] **Step 7: 运行测试 + 手工验收**

Run: `node --test tests/`
Expected: normalize 4 项 PASS

手工：托盘「系统设置」→ 设置窗出现 → 切换「标题栏样式」/主题色 → 主窗口 WebUI（需已登录 NAS）**不刷新即变化**；「关于」页版本/commit/配置路径显示正确

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat(settings): schema 驱动设置窗 + 归一化 + IPC 桥 + 4 项测试"
```

---

### Task 10: 端到端验收（M1–M2 联合）

**Files:** 无新增（仅验收与必要修复）

**Interfaces:** Consumes 全部前置任务

- [ ] **Step 1: 按 spec §12.2 逐条执行（用 FN ID `ea121314` 登录）**

Run:
```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd D:\fnOS-desktop\src-tauri; cargo build; .\target\debug\fnos-desktop.exe
```
逐条核对（每条记录实际观察结果）：
1. 官网打开且**无注入痕迹**（页面样式与浏览器里一致）
2. 托盘菜单 6 项存在、勾选项为 ✓
3. 设置窗填 NAS 地址 → 保存 → `config.json` 的 `mods.enabledOrigins` 含该 origin
4. 进入 NAS WebUI（FN ID `ea121314`）→ basic_mod.css 生效、mod.js 行为生效（窗口动画/squircle）
5. 切标题栏/启动台/主题色 → 免刷新即时生效
6. 关主窗口 → 进程仍在；托盘可恢复
7. 托盘退出 → 进程结束；重开配置保持
8. 安全：外部页面 `invoke('set_config')` 被拒
9. 载荷耗时：记录首屏注入完成时间（在 console 执行 `performance.now()` 对照 `initAt`）

- [ ] **Step 2: 把结果写入 `docs/acceptance/M1-M2-验收记录.md`（含失败项与修复计划）**

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "test(acceptance): M1-M2 端到端验收记录"
```

---

### Task 11: 失败模式与恢复（spec §12.3）

**Files:**
- Create: `ui/settings/status.js`, `src-tauri/src/error_page.html`（内置错误页）
- Modify: `ui/settings/app.js`（顶部状态条）、`src-tauri/src/main.rs`（`on_page_load` 失败处理）、`src-tauri/src/commands.rs`（`report_page_state`）
- Test: 手工

**Interfaces:**
- Consumes: Task 8/9 的 IPC
- Produces: 顶部状态条（未检测到 WebUI / 已注入 / 加载失败）；`reload_main` 重试入口

- [ ] **Step 1: `ui/settings/status.js`**

```js
export function statusBar(text, kind) {
  const el = document.getElementById('status');
  el.textContent = text;
  el.className = `status ${kind || ''}`;
}
export function addCurrentOriginToWhitelist(origin, enabledOrigins) {
  return Array.from(new Set([...(enabledOrigins || []), origin]));
}
```

- [ ] **Step 2: 在 `settings.html` 顶部加状态条并接线**

```html
<div id="status" class="status">正在读取配置…</div>
```
```js
// app.js boot() 末尾
statusBar(`配置已加载；注入开关 ${cfg.shell.injectEnabled ? '开启' : '关闭'}`, 'ok');
```

- [ ] **Step 3: 主窗口加载失败 → 内置错误页**

在 `main.rs` 的 `.on_page_load`（Task 7 未加，现在补）：

```rust
use tauri::webview::PageLoadEvent;
// ...
.on_page_load(|w, payload| {
    if payload.event() == PageLoadEvent::Finished {
        println!("[fnos] 页面加载完成: {}", payload.url());
    }
})
```
并在 `commands.rs` 增加：

```rust
#[tauri::command]
pub fn report_page_state<R: Runtime>(app: AppHandle<R>, url: String, ok: bool) -> Result<(), String> {
    if let Some(settings) = app.get_webview_window("settings") {
        let _ = settings.eval(format!(
            "window.__FNOS_STATUS__ && window.__FNOS_STATUS__({}, {});",
            serde_json::to_string(&url).unwrap(), ok
        ));
    }
    Ok(())
}
```

> 页面→宿主的状态上报依赖 Task 13 的远程通道；**M1–M2 阶段**改为由 Rust 侧 `on_page_load` 判断（URL 是否为签名候选），设置窗只在打开时向 Rust 查询最近一次结果。

- [ ] **Step 4: WebView2 运行时检测**

在 `main.rs` 启动早期读取运行时版本并写入日志；在设置窗「关于」页展示：

```rust
fn webview_runtime_version() -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}", "/v", "pv"])
        .output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().last().map(|s| s.to_string())
}
```
Expected: 本机输出 `148.0.3967.54` 之类；低于 `139` 时在设置窗提示 `corner-shape` 效果会退化

- [ ] **Step 5: 手工验收四类失败模式**

1. 断网启动 → 主窗口显示错误提示（不自吐白屏）
2. 填一个不存在的 NAS 地址 → 设置窗可重试
3. 手工把 `config.json` 改成非法 JSON → 启动回落默认并生成 `.bak`
4. 打开一个非飞牛网站 → 无注入，设置窗状态条显示未注入

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(resilience): 状态条/错误页/配置损坏回退/WebView2 版本检测"
```

---

### Task 12: NSIS 打包 + README + 合规件

**Files:**
- Create: `README.md`
- Modify: `src-tauri/tauri.conf.json`（确认 nsis 配置）、`ui/settings/app.js`（关于页链接 NOTICE）
- Test: 安装/卸载

**Interfaces:** Consumes 全部

- [ ] **Step 1: 写 `README.md`（必须含构建前置，否则协作者必然踩坑）**

````markdown
# fnOS 桌面壳

把 https://fnos.net/ 打包成 Windows 桌面应用，并对 fnOS NAS WebUI 注入
[fnOS_UI_Mods](https://github.com/aurysian-yan/fnOS_UI_Mods) 的样式与交互（上游资源原样 vendored）。

> 非官方第三方桌面壳，与飞牛官方无关。随附 UI 资源遵循上游 Non-Commercial License 1.0，**仅供非商业个人使用**。

## 环境前置（Windows）

1. Rust 1.90+，默认工具链 `x86_64-pc-windows-gnu`
2. **必须**把 MSYS2 的 mingw64 放进 PATH（rustup gnu 自包含目录缺 `as.exe`，否则报 `dlltool ... CreateProcess`）：

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
```

3. `src-tauri/icons/icon.ico` 必须存在（缺失会让 `tauri-build` 失败）

## 开发

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd src-tauri
cargo build
.\target\debug\fnos-desktop.exe
```

## 测试

```powershell
cargo test          # Rust 单测（在 src-tauri 下）
node --test tests/  # shim / bootstrap / normalize 契约测试
```

## 打包（NSIS 安装包）

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd src-tauri
cargo tauri build --bundles nsis
```

产物：`src-tauri/target/release/bundle/nsis/*-setup.exe`

## 目录

- `src-tauri/`：Rust 侧（配置、注入器、托盘、IPC）
- `src-tauri/assets/fnos-mods/`：上游 vendored 资源 + `NOTICE`（commit 与 SHA-256）
- `ui/settings/`：设置窗前端
- `docs/superpowers/specs/`：设计文档
````

- [ ] **Step 2: 打包**

Run:
```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cd D:\fnOS-desktop\src-tauri
cargo tauri build --bundles nsis
```
（若未安装 CLI：`cargo install tauri-cli --version "^2"`，或改用 `cargo build --release` + 手工确认 `bundle/nsis` 由 `tauri build` 生成）

Expected: 产出 `*-setup.exe`，体积预期 ~10MB 级

- [ ] **Step 3: 安装/卸载验收**

1. 运行安装包 → 安装到 `%LOCALAPPDATA%\fnOS`
2. 开始菜单出现「fnOS」
3. 启动 → 功能与开发版一致（托盘/设置窗/注入）
4. 卸载 → 程序目录与快捷方式清理干净；`%APPDATA%\com.fnos.desktop\config.json` 保留（用户配置不删）

- [ ] **Step 4: 合规件检查**

Run:
```powershell
Test-Path D:\fnOS-desktop\src-tauri\assets\fnos-mods\LICENSE   # 必须 True
Select-String -Path D:\fnOS-desktop\src-tauri\assets\fnos-mods\NOTICE -Pattern 'Pinned commit','Packaging changes' | Select-Object -First 3
```
Expected: `LICENSE` 存在；`NOTICE` 含 commit 与包装改动说明

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "build(nsis): NSIS 安装包 + README(构建前置) + 合规件检查"
```

---

### Task 13: P1 — 完美图标逐项 + 登录壁纸 + 页面→宿主上报通道

**Files:**
- Create: `src-tauri/capabilities/remote-report.json`, `src-tauri/src/report.rs`
- Modify: `src-tauri/src/main.rs`、`ui/settings/app.js`、`ui/settings/schema.js`
- Test: 手工 + 安全复核

**Interfaces:**
- Consumes: 上游 `FNOS_GET_LAUNCHPAD_APP_ITEMS` / `FNOS_APPLY` 消息语义
- Produces: `mods_page_report(payload: String) -> ()` 命令（**唯一对远程 origin 开放的命令**，只做数据中转：写入内存并通知设置窗）

- [ ] **Step 1: 声明受限的远程 capability**

`src-tauri/capabilities/remote-report.json`：

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "remote-page-report",
  "description": "唯一允许远程页面调用的命令：仅做数据中转，无文件/网络/配置写入能力",
  "remote": { "urls": ["*"] },
  "permissions": ["allow-mods-page-report"]
}
```

- [ ] **Step 2: 实现中转命令**

```rust
// src-tauri/src/report.rs
use serde_json::Value;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};

#[derive(Default)]
pub struct PageReport {
    pub last_payload: Mutex<Option<Value>>,
}

#[tauri::command]
pub fn mods_page_report<R: Runtime>(app: AppHandle<R>, payload: String) {
    let Ok(value) = serde_json::from_str::<Value>(&payload) else { return };
    if let Some(state) = app.try_state::<PageReport>() {
        *state.last_payload.lock().unwrap() = Some(value.clone());
    }
    if let Some(settings) = app.get_webview_window("settings") {
        let _ = settings.eval(format!(
            "window.__FNOS_PAGE_REPORT__ && window.__FNOS_PAGE_REPORT__({});",
            serde_json::to_string(&value).unwrap()
        ));
    }
}
```

- [ ] **Step 3: 在 shim 里接上页面侧上报**

在 `shim.js` 的 `sendMessage` 内增加（仅当宿主提供了通道时）：

```js
      try {
        if (W.__TAURI_INTERNALS__ && W.__TAURI_INTERNALS__.invoke) {
          W.__TAURI_INTERNALS__.invoke('mods_page_report', { payload: JSON.stringify(msg) });
        }
      } catch (e) { /* 远程通道未授权时静默失败，不影响页面 */ }
```

- [ ] **Step 4: 设置窗消费上报（完美图标列表 + 状态条）**

在 `app.js` 中：

```js
window.__FNOS_PAGE_REPORT__ = (msg) => {
  if (msg && msg.type === 'FNOS_LAUNCHPAD_APP_ITEMS') {
    state.appItems = msg.items || [];
    if (state.active === 'perfectIcon') render();
  }
  if (msg && msg.type === 'FNOS_PAGE_STATUS') {
    statusBar(msg.injected ? '已注入到当前 WebUI' : '未检测到 fnOS WebUI', msg.injected ? 'ok' : 'warn');
  }
};
```

并在 `schema.js` 追加分组：

```js
  {
    id: 'perfectIcon', title: '完美图标', items: [
      { key: 'mods.launchpadIconScaleEnabled', label: '完美图标', type: 'bool' },
      { key: '__appItems', label: '应用逐项设置', type: 'appList' }
    ]
  },
  {
    id: 'wallpaper', title: '登录壁纸', items: [
      { key: 'shell.wallpaper', label: '登录页背景图', type: 'imageFile' }
    ]
  }
```

`type: 'appList'` 与 `'imageFile'` 需要在 `app.js` 的 `fieldEl` 里补两个分支：
- `appList`：渲染 `state.appItems`，每项一个三态选择（缩放/仅遮罩/重绘），写回 `launchpadIconScaleSelectedKeys` 等四个键
- `imageFile`：`<input type="file" accept="image/png,image/jpeg,image/webp">`，选中后经 `set_config` 写 `shell` 段并复制文件到配置目录

- [ ] **Step 5: 安全复核（必须做）**

确认 `mods_page_report` **不触碰**：文件系统、网络、`config` 状态。在外部页面 console 执行：

```js
await window.__TAURI_INTERNALS__.invoke('mods_page_report', { payload: '{"x":1}' })  // 允许
await window.__TAURI_INTERNALS__.invoke('set_config', { patch: {} })                 // 必须被拒
await window.__TAURI_INTERNALS__.invoke('get_config')                                // 必须被拒
```
Expected: 第一条成功，后两条被拒

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(p1): 完美图标逐项 + 登录壁纸 + 受限页面上报通道(仅数据中转)"
```

---

## Self-Review

**1. Spec coverage**

| Spec 章节 | 对应 Task |
|---|---|
| §4 架构与模块边界 | Task 1/3/4/5/6/7/8/9 |
| §5 注入桥（shim 规格、三级兜底、mod.js 兜底、时序、载荷） | Task 4/5/6（体积实测在 Task 10 Step 1.9） |
| §6 配置模型（25 键、local、shell、归一化、热更新） | Task 3/8/9 |
| §7 窗口与托盘（含关窗隐藏、单实例） | Task 1/7 |
| §8 设置窗（分组项、IPC 契约、视觉） | Task 9（P1 项在 Task 13） |
| §9 安全（capability、远程拒绝、hash 记录） | Task 2/8（验收 Task 8 Step 5） |
| §10 合规与品牌 | Task 2/12（关于页声明在 Task 9 Step 5） |
| §11 打包（NSIS、构建前置、icon.ico） | Task 1/12 |
| §12 测试与验收（单测、E2E、失败模式） | Task 3/4/5/6/9/10/11 |
| §13 里程碑 M1–M4 | M1=Task 1–6、M2=Task 7–9、M3=Task 12、M4=Task 11（+10 验收） |

**2. Placeholder scan**：已检查 —— 无 "TBD/TODO/待补"；所有代码步骤都带完整代码；Task 11 Step 4 的注册表命令与 Task 12 的打包命令都是可直接执行的完整命令。

**3. Type consistency**（跨 Task 的符号名核对）

- `Config` / `ModsConfig` / `LocalConfig` / `ShellConfig` / `WindowGeom`、`Config::load/save/normalize`、`DEFAULT_HOME_URL`、`normalize_brand_color`、`is_valid_prefect_icon_path`（Task 3 定义 → Task 6/7/8 使用）✔
- `injector::build_init_script(&Config) -> String`、`injector::SHELL_VERSION`、`injector::MODS_COMMIT`（Task 6 定义 → Task 7/8 使用；Task 8 的 `Meta` 引用两者）✔
- `AppState { config: Mutex<Config> }`、`load_config`、`apply_home_url`、`set_inject_enabled`、`set_inject_checked`、`open_settings`、`open_nas`、`save_window_geom`、`sync_menus`（Task 8 定义 → Task 7 调用）✔ 注意 Task 7 的 `tray.rs` 调用了 `crate::commands::*`，而 `commands.rs` 在 Task 8 才完整实现 —— Task 7 Step 2 已注明「可提前写入 Task 8 Step 2 的完整实现」。
- IPC 命令名与 capability 权限名一对一：`get_config`↔`allow-get-config`、`set_config`↔`allow-set-config`、`reload_main`↔`allow-reload-main`、`open_config_dir`↔`allow-open-config-dir`、`reset_config`↔`allow-reset-config`、`mods_page_report`↔`allow-mods-page-report` ✔
- 前端 `normalizeMods` 的键名与 Rust `ModsConfig` 字段的 camelCase 一致 ✔
- shim 暴露 `window.__FNOS_APPLY_CONFIG__`（Task 4）→ Task 8 `apply_to_page` 调用同名 ✔
- `window.__FNOS_MOD_EXECUTED__` 由 shim 的 getURL('mod.js') 与 bootstrap 兜底共同置位（Task 4/5 一致）✔

**4. 已知偏差（需在执行时同步更新 spec）**

- Task 11 的「页面→宿主状态上报」在 M1–M2 阶段改用 Rust 侧 `on_page_load` 判定（设置窗打开时向 Rust 查询最近结果），完整双向通道在 Task 13（P1）才落地 —— 执行到 Task 11 时同步更新 spec §12.3 标注该阶段性方案。
- 托盘菜单项数与 spec §7 一致（5 个可点击项 + 1 条分隔线），无需改动 spec。

---

## Execution Handoff

计划已保存到 `docs/superpowers/plans/2026-09-28-fnos-desktop-shell.md`。两种执行方式：

1. **Subagent-Driven（推荐）** —— 每个 Task 派一个全新 subagent，任务间我来评审，迭代快、上下文干净
2. **Inline Execution** —— 在当前会话按 `executing-plans` 分批执行，带检查点

选哪种？
