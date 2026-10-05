//! 检查更新（T14c 修复轮 21）：托盘「检查更新」菜单项的实现。
//!
//! 流程（用户要求：**自动下载更新**，不是打开页面）：
//!   1. 请求 GitHub Releases 的 `latest`（api.github.com），取 `tag_name` 与当前版本
//!      （`env!("CARGO_PKG_VERSION")`）比较；
//!   2. 有新版本 → 从 release 的 assets 里挑出 NSIS 安装包（`*x64-setup.exe`），
//!      **自动下载**到配置目录的 `updates/` 下（已下载过同名文件就复用）；
//!   3. 弹原生消息框询问是否立即安装——「确定」则静默安装（NSIS `/S`）并让安装器
//!      装完自动重启应用（`/R`）；安装器自己会先结束正在运行的旧进程（tauri 的
//!      NSIS 模板在静默模式下会 kill 正在运行的应用，文件锁不构成问题）；
//!   4. 已是最新 / 网络失败 / 响应不认识 → 原生消息框如实告知，绝不静默。
//!
//! 本壳**不做**应用内替换升级（那是 tauri-plugin-updater 的签名校验 + 更新清单体系）：
//! NSIS 安装包走 GitHub Releases 已经闭环，托盘要的是「检查 → 下载 → 装好重启」这条链。
//!
//! 请求安全边界（本模块发起的每一个 URL）：
//!   - API 地址是编译期常量（api.github.com），不来自任何响应；
//!   - 下载地址虽然取自 API 的 `browser_download_url`，但必须通过 [`validate_download_url`]：
//!     https scheme、host 严格等于 `github.com`、路径前缀严格等于本仓库的
//!     `/releases/download/`——localhost / 环回 / 私有 / 保留地址与任何其它 host
//!     一律拒绝，文件名另行白名单清洗（防路径穿越）。
//!
//! 线程：HTTP（ureq 阻塞）与 MessageBoxW（阻塞直到点击）都由托盘事件回调里的
//! `spawn_blocking` 调进阻塞线程池——绝不能卡住托盘。

use crate::commands::save_window_geom;
use crate::paths::config_dir;
use serde_json::Value;
use tauri::{AppHandle, Runtime};

/// 本壳的 GitHub 仓库（`owner/repo`）。
///
/// 必须与 `ui/settings/chrome-shim.js` 的 `APP_REPO_URL`
/// （`https://github.com/eafenzhang/fnos-desktop`）指向同一个仓库——
/// tests/chrome-shim.test.mjs 有跨语言一致性断言。
pub const APP_REPO_SLUG: &str = "eafenzhang/fnos-desktop";

/// GitHub Releases 的 latest 端点（跟随仓库最新的**正式**发布）。
pub const RELEASES_LATEST_API: &str =
    "https://api.github.com/repos/eafenzhang/fnos-desktop/releases/latest";

/// 下载 URL 的 host 白名单与路径前缀（与 [`APP_REPO_SLUG`] 同源）。
const DOWNLOAD_HOST: &str = "github.com";
const DOWNLOAD_PATH_PREFIX: &str = "/releases/download/";

/// API 请求超时（秒）。检查更新是低频操作，短超时失败好过长时间吊着。
const API_TIMEOUT_SECS: u64 = 10;
/// 安装包下载超时（秒）。安装包 ~3MB，120s 覆盖慢网络。
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;
/// 下载结果的健全性下限（字节）：NSIS 安装包正常 ~3MB，明显小于它的当失败丢弃。
const MIN_INSTALLER_BYTES: u64 = 1_000_000;

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 把 release 的 tag 解析成版本三元组：`v0.2.1` / `0.2.1` / `V1.2.3` → `(1,2,3)`。
///
/// 只认「可选 v 前缀 + 三段纯数字」——其它形状（`v1.2` / `v1.2.3.4` / `v1.2.x` /
/// 预发布后缀）一律 `None`，调用方按「无法识别」如实处理，不猜。
pub fn parse_tag_version(tag: &str) -> Option<(u64, u64, u64)> {
    let t = tag.trim();
    let t = t.strip_prefix(['v', 'V']).unwrap_or(t);
    let mut parts = t.split('.');
    let major = parts.next()?.trim().parse().ok()?;
    let minor = parts.next()?.trim().parse().ok()?;
    let patch = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// 线上 tag 是否比当前版本**新**（纯数字三元组比较；任一侧解析不了 = 不算更新）。
pub fn is_newer(latest_tag: &str, current: &str) -> bool {
    match (parse_tag_version(latest_tag), parse_tag_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// 下载文件名白名单：只允许 ASCII 字母数字与 `. _ -`（不含任何路径分隔符）。
/// 任何越界字符都拒绝——下载 URL 来自 API 响应，不能把它的尾部当文件名直接用。
fn safe_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// 校验从 API 响应里拿到的下载地址。除 https + host 白名单外，路径必须落在本仓库的
/// `/releases/download/` 下——host 不可能是 localhost / 环回 / 私有 / 保留地址，
/// 因为它被**钉死**在常量上，而不是从响应里读的。
pub fn validate_download_url(raw: &str) -> Result<String, String> {
    let u = url::Url::parse(raw).map_err(|e| format!("下载地址不是合法 URL：{e}"))?;
    if u.scheme() != "https" {
        return Err(format!("下载地址必须是 https（实际 {}）", u.scheme()));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("下载地址不得携带用户信息".into());
    }
    let host = u.host_str().unwrap_or("");
    if !host.eq_ignore_ascii_case(DOWNLOAD_HOST) {
        return Err(format!("下载地址的 host 不在白名单（实际 {host}）"));
    }
    let prefix = format!("/{APP_REPO_SLUG}{DOWNLOAD_PATH_PREFIX}");
    if !u.path().starts_with(&prefix) {
        return Err(format!("下载地址不在本仓库的发布资产下（{}）", u.path()));
    }
    Ok(u.to_string())
}

/// 从 release JSON 里挑出 NSIS 安装包资产，返回（下载地址，文件名）。
///
/// 优先 `fnOS_…x64-setup.exe`（本壳的命名形态：productName = fnOS），其余
/// `*x64-setup.exe` 兜底；都没有 → Err（如实告知，不猜别的资产）。
pub fn pick_installer_asset(release: &Value) -> Result<(String, String), String> {
    let assets = release
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "release 响应里没有 assets".to_string())?;
    let mut fallback: Option<(String, String)> = None;
    for asset in assets {
        let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let url = asset
            .get("browser_download_url")
            .and_then(|u| u.as_str())
            .unwrap_or("");
        if !(name.ends_with("x64-setup.exe") && !url.is_empty()) {
            continue;
        }
        if name.starts_with("fnOS_") {
            return Ok((url.to_string(), name.to_string()));
        }
        if fallback.is_none() {
            fallback = Some((url.to_string(), name.to_string()));
        }
    }
    fallback.ok_or_else(|| "发布页上没有找到 x64 安装包（*x64-setup.exe）".to_string())
}

/// 请求 latest release 的完整 JSON。任何失败（DNS / 超时 / 非 2xx / JSON 不认识）都带
/// 简短原因返回 `Err`——托盘那边会原样告诉用户。
fn fetch_latest_release() -> Result<Value, String> {
    let body = ureq::get(RELEASES_LATEST_API)
        .set("User-Agent", concat!("fnos-desktop/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(API_TIMEOUT_SECS))
        .call()
        .map_err(|e| format!("{e}"))?
        .into_string()
        .map_err(|e| format!("读取响应失败：{e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("响应不是合法 JSON：{e}"))
}

/// 把安装包下载到配置目录 `updates/<文件名>`（已存在同名且体积达标 → 直接复用），
/// 返回落盘路径。写 `.part` 临时名、完成后改名，避免留下半截文件被当成完整安装包。
fn download_installer(download_url: &str, file_name: &str) -> Result<std::path::PathBuf, String> {
    let dir = config_dir().join("updates");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
    let dest = dir.join(file_name);
    let part = dir.join(format!("{file_name}.part"));

    if dest.exists() {
        let size = std::fs::metadata(&dest)
            .map(|m| m.len())
            .unwrap_or(0);
        if size >= MIN_INSTALLER_BYTES {
            return Ok(dest); // 上一次已经下载好了（比如点了「取消」后再次检查）
        }
    }

    let mut reader = ureq::get(download_url)
        .set("User-Agent", concat!("fnos-desktop/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .call()
        .map_err(|e| format!("下载失败：{e}"))?
        .into_reader();
    let mut file = std::fs::File::create(&part)
        .map_err(|e| format!("写安装包失败：{e}"))?;
    std::io::copy(&mut reader, &mut file).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("下载中断：{e}")
    })?;
    let size = file
        .metadata()
        .map(|m| m.len())
        .unwrap_or(0);
    drop(file);
    if size < MIN_INSTALLER_BYTES {
        let _ = std::fs::remove_file(&part);
        return Err(format!("下载的安装包不完整（{size} 字节）"));
    }
    std::fs::rename(&part, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("安装包落盘失败：{e}")
    })?;
    Ok(dest)
}

/// 启动安装器时的瞬态文件锁对策（T14d 实测缺陷，用户截图「os error 32」）：
/// ① 刚下载完的安装包会被杀毒 / Defender 短暂扫描锁定（用户点「确定」时扫描可能未结束）；
/// ② 上一次启动的安装器实例没退干净——运行中的 exe 文件本身就是锁。
/// 对策：结束同名残留安装器进程 + 有界重试；仍失败才如实报错（附手动运行的建议）。
const INSTALL_SPAWN_TRIES: u32 = 3;
const INSTALL_SPAWN_RETRY_DELAY_MS: u64 = 1500;

/// 结束与安装包**同名**的残留安装器进程（运行中的 exe 文件被自己锁着）。
/// taskkill 按映像名精确匹配我们的安装包文件名（`fnOS_x.y.z_x64-setup.exe`），
/// 不会碰到别的进程；找不到该进程时 taskkill 非零码退出——无所谓。
fn kill_stale_installer(installer: &std::path::Path) {
    let Some(name) = installer.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/F", "/IM", name]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd.spawn();
}

/// 带重试地启动安装器（`/S` 静默 + `/R` 装完自动重启）。每次失败后：结束同名残留
/// 进程 → 等 [`INSTALL_SPAWN_RETRY_DELAY_MS`] → 再试；[`INSTALL_SPAWN_TRIES`] 次仍失败
/// 才把错误交回调用方（弹窗里会带上手动运行的路径建议）。
fn spawn_installer_with_retry(installer: &std::path::Path, retry_delay: std::time::Duration) -> Result<(), String> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 1..=INSTALL_SPAWN_TRIES {
        let mut cmd = std::process::Command::new(installer);
        cmd.arg("/S").arg("/R");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW); // 静默安装本就无窗口，兜住控制台闪现
        }
        match cmd.spawn() {
            Ok(_) => return Ok(()),
            Err(e) => {
                last = Some(e);
                if attempt < INSTALL_SPAWN_TRIES {
                    kill_stale_installer(installer);
                    std::thread::sleep(retry_delay);
                }
            }
        }
    }
    Err(format!(
        "重试 {} 次仍无法启动安装器：{}（安装包可能被杀毒软件或上一次安装进程占用）",
        INSTALL_SPAWN_TRIES,
        last.map(|e| e.to_string()).unwrap_or_else(|| "未知错误".into())
    ))
}

/// 静默安装：NSIS `/S` + `/R`（装完自动重启应用）。tauri 的 NSIS 模板在静默模式下
/// 会先结束正在运行的旧进程，所以文件锁不构成问题；这里再兜一层——800ms 后主动退出。
fn start_install<R: Runtime>(app: &AppHandle<R>, installer: &std::path::Path) -> Result<(), String> {
    spawn_installer_with_retry(installer, std::time::Duration::from_millis(INSTALL_SPAWN_RETRY_DELAY_MS))
        .map_err(|e| format!("{e}（安装包路径：{}；也可以稍后在文件管理器里手动运行它）", installer.display()))?;
    // 安装器（onInit 阶段）会结束本进程；万一没结束，这里主动退，把文件锁让出来。
    std::thread::sleep(std::time::Duration::from_millis(800));
    save_window_geom(app);
    app.exit(0);
    Ok(())
}

/// 托盘「检查更新」的入口（**阻塞**函数；托盘侧用 `spawn_blocking` 调用）。
pub fn check_from_tray<R: Runtime>(app: AppHandle<R>) {
    let current = current_version();
    let release = match fetch_latest_release() {
        Ok(r) => r,
        Err(e) => {
            message_box(
                "fnOS 检查更新",
                &format!("检查更新失败：{e}\n\n请确认本机可以访问 github.com。"),
                MB_ICON_WARNING,
            );
            return;
        }
    };
    let tag = release
        .get("tag_name")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let Some(latest) = parse_tag_version(&tag) else {
        message_box(
            "fnOS 检查更新",
            &format!("线上版本号无法识别（tag：{tag}）。"),
            MB_ICON_WARNING,
        );
        return;
    };
    let Some(cur) = parse_tag_version(current) else {
        message_box(
            "fnOS 检查更新",
            &format!("当前版本号无法识别（{current}）。"),
            MB_ICON_WARNING,
        );
        return;
    };
    if latest <= cur {
        message_box(
            "fnOS 检查更新",
            &format!("已是最新版本（v{current}）。"),
            MB_ICON_INFO,
        );
        return;
    }

    // 有新版本：自动下载（文件已存在则复用），然后询问是否立即安装。
    let (download_url, file_name) = match pick_installer_asset(&release) {
        Ok((url, name)) if safe_asset_name(&name) => (url, name),
        Ok((_, name)) => {
            message_box(
                "fnOS 检查更新",
                &format!("资产文件名不可用（{name}），已放弃下载。"),
                MB_ICON_WARNING,
            );
            return;
        }
        Err(e) => {
            message_box("fnOS 检查更新", &format!("{e}"), MB_ICON_WARNING);
            return;
        }
    };
    let validated = match validate_download_url(&download_url) {
        Ok(u) => u,
        Err(e) => {
            message_box("fnOS 检查更新", &format!("{e}"), MB_ICON_WARNING);
            return;
        }
    };
    let installer = match download_installer(&validated, &file_name) {
        Ok(p) => p,
        Err(e) => {
            message_box("fnOS 检查更新", &format!("{e}"), MB_ICON_WARNING);
            return;
        }
    };

    let choice = message_box(
        "fnOS 检查更新",
        &format!(
            "发现新版本 {tag}（当前 v{current}），安装包已下载。\n\n点「确定」立即安装并自动重启；点「取消」下次再说。"
        ),
        MB_ICON_INFO | MB_OKCANCEL,
    );
    if choice == ID_OK {
        if let Err(e) = start_install(&app, &installer) {
            message_box("fnOS 检查更新", &format!("{e}"), MB_ICON_WARNING);
        }
    }
}

// ---------- 原生 MessageBox（Windows）与跨平台桩 ----------

const MB_ICON_INFO: u32 = 0x40; // MB_ICONINFORMATION
const MB_ICON_WARNING: u32 = 0x30; // MB_ICONWARNING
const MB_OKCANCEL: u32 = 0x1;
const ID_OK: i32 = 1;

/// 子进程不闪控制台窗口（taskkill / 安装器 spawn 共用）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 弹一个原生消息框（阻塞直到用户关闭；`MB_TOPMOST` 保证不被主窗口压在底下）。
/// 返回被点击的按钮 ID（`MB_OKCANCEL` 时用于区分确定/取消）。
#[cfg(windows)]
fn message_box(title: &str, text: &str, flags: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_TOPMOST};
    let mut wide_title: Vec<u16> = title.encode_utf16().collect();
    wide_title.push(0);
    let mut wide_text: Vec<u16> = text.encode_utf16().collect();
    wide_text.push(0);
    // 空 HWND = 桌面消息框；本壳的窗口不作为父窗（托盘动作没有“所属窗口”的语义）
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide_text.as_ptr(),
            wide_title.as_ptr(),
            flags | MB_OK | MB_TOPMOST,
        )
    }
}

/// 非 Windows 的编译桩（本壳只在 Windows 上跑，留着它是为了让 `cargo check` 在
/// 任何平台都不需要条件编译整个模块）。
#[cfg(not(windows))]
fn message_box(title: &str, text: &str, _flags: u32) -> i32 {
    eprintln!("[fnos] {title}: {text}");
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CODE_END: &str = "#[cfg(test)]";

    /// 只看测试模块之前的代码区（include_str 会把本测试自己的断言字符串也算进去）。
    fn code() -> String {
        let src = include_str!("updater.rs");
        src[..src.find(CODE_END).expect("测试模块存在")].to_string()
    }

    #[test]
    fn tag_versions_parse_strictly() {
        assert_eq!(parse_tag_version("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_tag_version("0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_tag_version(" V1.10.3 "), Some((1, 10, 3)));
        assert_eq!(parse_tag_version("v1.2"), None, "少一段不算版本");
        assert_eq!(parse_tag_version("v1.2.3.4"), None, "多一段不算版本");
        assert_eq!(parse_tag_version("v1.2.x"), None, "非数字不算版本");
        assert_eq!(parse_tag_version("v1.2.3-beta"), None, "预发布后缀不猜");
        assert_eq!(parse_tag_version(""), None);
    }

    #[test]
    fn is_newer_is_strictly_numeric() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("v0.1.0", "0.1.0"), "同版本不算新");
        assert!(!is_newer("v0.1.9", "0.2.0"), "旧版不算新");
        assert!(!is_newer("垃圾", "0.1.0"), "识别不了就不能当“有更新”");
    }

    #[test]
    fn asset_picker_prefers_fnos_nsis_and_honestly_fails() {
        let release = json!({
            "tag_name": "v0.2.0",
            "assets": [
                { "name": "fnOS_0.2.0_x64-setup.exe",
                  "browser_download_url": "https://github.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/fnOS_0.2.0_x64-setup.exe" },
                { "name": "latest.json",
                  "browser_download_url": "https://github.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/latest.json" }
            ]
        });
        let (url, name) = pick_installer_asset(&release).expect("必须挑中 NSIS 安装包");
        assert_eq!(name, "fnOS_0.2.0_x64-setup.exe");
        assert!(url.ends_with("/fnOS_0.2.0_x64-setup.exe"));

        let empty = json!({ "tag_name": "v0.2.0", "assets": [] });
        assert!(pick_installer_asset(&empty).is_err(), "没有资产要如实失败");
        let none = json!({ "tag_name": "v0.2.0" });
        assert!(pick_installer_asset(&none).is_err(), "缺 assets 字段要如实失败");
    }

    #[test]
    fn download_url_is_pinned_to_the_repo() {
        let good = format!(
            "https://{DOWNLOAD_HOST}/{APP_REPO_SLUG}{DOWNLOAD_PATH_PREFIX}v0.2.0/fnOS_0.2.0_x64-setup.exe"
        );
        assert_eq!(validate_download_url(&good).as_deref(), Ok(good.as_str()));
        // 逐条拒绝：非 https / 别的 host / 别的仓库 / 带用户信息 / 本机与内网地址
        for bad in [
            "http://github.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/a.exe",
            "https://evil.example.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/a.exe",
            "https://objects.githubusercontent.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/a.exe",
            "https://github.com/other/repo/releases/download/v0.2.0/a.exe",
            "https://user@github.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/a.exe",
            "https://localhost/eafenzhang/fnos-desktop/releases/download/v0.2.0/a.exe",
            "https://127.0.0.1/eafenzhang/fnos-desktop/releases/download/v0.2.0/a.exe",
            "https://192.168.1.5/eafenzhang/fnos-desktop/releases/download/v0.2.0/a.exe",
            "https://10.0.0.9/eafenzhang/fnos-desktop/releases/download/v0.2.0/a.exe",
            "not a url",
        ] {
            assert!(validate_download_url(bad).is_err(), "必须拒绝：{bad}");
        }
    }

    #[test]
    fn asset_names_are_whitelisted_against_traversal() {
        assert!(safe_asset_name("fnOS_0.2.0_x64-setup.exe"));
        assert!(!safe_asset_name("..\\evil.exe"), "路径穿越必须拒绝");
        assert!(!safe_asset_name("a/b.exe"), "分隔符必须拒绝");
        assert!(!safe_asset_name("a b.exe"), "空格不在白名单");
        assert!(!safe_asset_name(""), "空名拒绝");
        assert!(!safe_asset_name(&"x".repeat(129)), "过长拒绝");
    }

    /// 常量与消费方的锚定：仓库 slug、API 指向同一个 repo；当前版本来自 Cargo.toml。
    #[test]
    fn endpoints_are_anchored_to_the_repo() {
        let code = code();
        assert!(
            code.contains(&format!("pub const APP_REPO_SLUG: &str = \"{APP_REPO_SLUG}\";")),
            "repo slug 常量必须在"
        );
        assert!(
            code.contains("releases/latest") && code.contains(APP_REPO_SLUG),
            "API 必须落在同一个仓库下"
        );
        assert!(
            code.contains("env!(\"CARGO_PKG_VERSION\")"),
            "当前版本必须来自 Cargo.toml（与安装包版本同源），不得手写字面量"
        );
        assert!(code.contains("check_from_tray"), "托盘入口必须在");
        // 用户要求：自动下载安装，而不是打开页面
        assert!(
            code.contains("download_installer") && code.contains("start_install"),
            "必须自动下载并静默安装"
        );
        assert!(
            !code.contains("releases/latest\") // 打开页面") && !code.contains("open_in_browser"),
            "不得退回「打开发布页」的旧方案"
        );
    }

    /// 安装器启动重试（T14d 实测：os error 32 = 文件被占用——杀毒扫描/上次安装进程残留）。
    #[test]
    fn spawn_installer_retries_then_reports_clearly() {
        // 不存在的路径：每次 spawn 都失败 → 走满重试 → 错误信息带次数与原因
        let missing = std::env::temp_dir().join(format!("fnos-no-installer-{}.exe", std::process::id()));
        let err = spawn_installer_with_retry(&missing, std::time::Duration::ZERO).unwrap_err();
        assert!(err.contains("3 次"), "错误信息必须说明重试次数：{err}");
        assert!(err.contains("os error"), "错误信息必须带上底层原因：{err}");
        assert!(
            err.contains("杀毒软件") || err.contains("占用"),
            "错误信息必须提示占用来源：{err}"
        );
        let src = include_str!("updater.rs");
        let code_end = src.find("#[cfg(test)]").unwrap();
        let code = &src[..code_end];
        assert!(
            code.contains("安装包路径：{}"),
            "start_install 的错误必须带安装包路径（用户手动运行的后路）"
        );
        assert!(
            code.contains("kill_stale_installer"),
            "重试之间必须清同名残留安装器进程（运行中的 exe 自锁）"
        );
    }
}
