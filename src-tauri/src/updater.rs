//! 检查更新（T14g：**设置页 → 更新**；托盘菜单只保留基础动作）。
//!
//! 流程（用户要求：设置页里**实时看到下载进度**）：
//!   1. 请求 GitHub Releases 的 `latest`（api.github.com），取 `tag_name` 与当前版本
//!      （`env!("CARGO_PKG_VERSION")`）比较；
//!   2. 有新版本 → 从 release 的 assets 里挑出 NSIS 安装包（`*x64-setup.exe`），
//!      **自动下载**到配置目录的 `updates/` 下（已下载过同名文件就复用）；
//!      下载循环每 ~120ms 把「已下载 / 总大小 / 速度」写进 [`UpdateStatus`]，
//!      设置页轮询 `get_update_state` 渲染进度条——没有事件总线、没有新增 IPC 面；
//!   3. 下载完成 → 设置页出现「立即安装并重启」，确认后静默安装（NSIS `/S` + `/R`）；
//!   4. 已是最新 / 网络失败 / 响应不认识 → 状态里如实写明，设置页直接显示。
//!
//! 本壳**不做**应用内替换升级（那是 tauri-plugin-updater 的签名校验 + 更新清单体系）：
//! NSIS 安装包走 GitHub Releases 已经闭环，设置页要的是「检查 → 进度 → 装好重启」这条链。
//!
//! 请求安全边界（本模块发起的每一个 URL）：
//!   - API 地址是编译期常量（api.github.com），不来自任何响应；
//!   - 下载地址虽然取自 API 的 `browser_download_url`，但必须通过 [`validate_download_url`]：
//!     https scheme、host 严格等于 `github.com`、路径前缀严格等于本仓库的
//!     `/releases/download/`——localhost / 环回 / 私有 / 保留地址与任何其它 host
//!     一律拒绝，文件名另行白名单清洗（防路径穿越）。
//!
//! 线程：HTTP（ureq 阻塞）由命令里的 `spawn_blocking` 调进阻塞线程池——绝不能卡住
//! 主线程或托盘事件循环。

use crate::commands::{save_window_geom, AppState};
use crate::logfile::flog;
use crate::paths::config_dir;
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Write};
use tauri::{AppHandle, Manager, Runtime};

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

/// 安装包下载的**连接级重试次数**（os error 10060 这类瞬态超时；持久被墙要靠镜像）。
const DOWNLOAD_CONNECT_TRIES: u32 = 2;
const DOWNLOAD_RETRY_DELAY_MS: u64 = 1200;

/// 镜像前缀的 host 校验：拒绝本机 / 环回 / 内网 / 链路本地 / 保留地址。
///
/// 先把 host 当 IP 字面量解析（`url::Url::host_str()` 对 IPv6 保留方括号），再用
/// `IpAddr` 的语义判定——字符串前缀判定漏掉了 IPv4-mapped IPv6（`::ffff:127.0.0.1`）
/// 与 `*.localhost`（RFC 6761 全部解析到环回）。
fn host_is_allowed_for_mirror(host: &str) -> bool {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if h.is_empty()
        || h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.ends_with(".arpa")
    {
        return false;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                let o = v4.octets();
                let cgnat = o[0] == 100 && (64..=127).contains(&o[1]);
                let this_network = o[0] == 0;
                !(v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || cgnat
                    || this_network)
            }
            std::net::IpAddr::V6(v6) => {
                // IPv4-mapped（::ffff:a.b.c.d）必须按内嵌的 IPv4 判定，否则
                // [::ffff:127.0.0.1] / [::ffff:192.168.1.5] 会冒充公网地址。
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return host_is_allowed_for_mirror(&v4.to_string());
                }
                let seg = v6.segments();
                let unique_local = (seg[0] & 0xfe00) == 0xfc00; // fc00::/7
                let link_local = (seg[0] & 0xffc0) == 0xfe80; // fe80::/10
                !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || unique_local
                    || link_local)
            }
        };
    }
    true
}

/// 校验用户配置的镜像前缀（`shell.updateMirrorPrefix`）：必须是 https 且 host 通过
/// [`host_is_allowed_for_mirror`]；拼接形态为「前缀 + 完整原始 URL」（ghproxy 系加速
/// 服务的通用格式，如 `https://your-mirror.example.com/https://github.com/...`）。
pub fn validate_mirror_prefix(prefix: &str) -> Result<String, String> {
    let p = prefix.trim();
    if p.is_empty() {
        return Ok(String::new());
    }
    let u = url::Url::parse(p).map_err(|e| format!("镜像前缀不是合法 URL：{e}"))?;
    if u.scheme() != "https" {
        return Err(format!("镜像前缀必须是 https（实际 {}）", u.scheme()));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("镜像前缀不得携带用户信息".into());
    }
    let host = u.host_str().unwrap_or("");
    if host.is_empty() || !host_is_allowed_for_mirror(host) {
        return Err(format!("镜像前缀的 host 不可用（{host}）：不得为本机 / 内网 / 保留地址"));
    }
    Ok(p.trim_end_matches('/').to_string())
}

/// 组装最终下载 URL：未配置镜像 → 原始 GitHub URL；配置了 → `前缀/原始 URL`。
fn resolve_download_url(raw: &str, mirror_prefix: &str) -> Result<String, String> {
    let raw = validate_download_url(raw)?;
    let prefix = validate_mirror_prefix(mirror_prefix)?;
    if prefix.is_empty() {
        Ok(raw)
    } else {
        Ok(format!("{prefix}/{raw}"))
    }
}

/// API 请求超时（秒）。检查更新是低频操作，短超时失败好过长时间吊着。
const API_TIMEOUT_SECS: u64 = 10;
/// 安装包下载超时（秒）。安装包 ~3MB，120s 覆盖慢网络。
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;
/// 下载结果的健全性下限（字节）：NSIS 安装包正常 ~3MB，明显小于它的当失败丢弃。
const MIN_INSTALLER_BYTES: u64 = 1_000_000;
/// 下载结果的硬上限（字节）：正常安装包 ~3MB，256 MiB 足以容纳任何合理增长，
/// 同时能挡住「坏镜像返回无限流」把磁盘写满。Content-Length 与流式读取两道都查。
const MAX_INSTALLER_BYTES: u64 = 256 * 1024 * 1024;

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 检查更新的阶段。设置页（`ui/settings/settings-page.js`）按同一组字符串映射文案与控件，
/// 两处必须逐字一致（有前端契约测试锁定）。
pub mod phase {
    pub const IDLE: &str = "idle";
    pub const CHECKING: &str = "checking";
    pub const UP_TO_DATE: &str = "upToDate";
    pub const DOWNLOADING: &str = "downloading";
    pub const READY: &str = "ready";
    pub const INSTALLING: &str = "installing";
    pub const ERROR: &str = "error";
}

/// 检查更新的**完整对外状态**（`get_update_state` 的返回体）。
///
/// 设置页每 ~300ms 轮询一次；一次检查/下载的每一次进展都写在这里，所以「实时进度」
/// 不需要事件总线——也就没有新增 IPC 面（设置页本来就只读这一个命令）。
///
/// `downloaded` / `total` 是**字节**（`total` 可能缺失：服务器没给 Content-Length），
/// `bytesPerSec` 是最近一个采样窗口的速度（只在 downloading 阶段有意义）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// 见 [`phase`]。
    pub phase: &'static str,
    /// 当前版本（`CARGO_PKG_VERSION`）。
    pub current: &'static str,
    /// 线上最新 tag（拿到 release 之后才有）。
    pub latest: Option<String>,
    /// 给用户看的一行话：检查失败原因 / 「已是最新」/「已下载」等。
    pub message: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: Option<u64>,
    /// `ready` 之后是安装包的绝对路径（`install_update` 只认这个值）。
    pub installer: Option<String>,
    /// 该 release 是否带 sha256 digest（没有 digest 的安装包只能做体积校验，
    /// 设置页必须在安装按钮旁如实标注）。
    pub has_digest: bool,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            phase: phase::IDLE,
            current: current_version(),
            latest: None,
            message: String::new(),
            downloaded: 0,
            total: None,
            bytes_per_sec: None,
            installer: None,
            has_digest: false,
        }
    }
}

impl UpdateStatus {
    /// 正在检查 / 下载 / 安装（设置页据此决定按钮置灰与是否继续轮询）。
    pub fn is_running(&self) -> bool {
        matches!(
            self.phase,
            phase::CHECKING | phase::DOWNLOADING | phase::INSTALLING
        )
    }
}

/// 读当前更新状态（命令与内部共用）。
pub fn status<R: Runtime>(app: &AppHandle<R>) -> UpdateStatus {
    app.state::<AppState>().update.lock().unwrap().clone()
}

/// 就地改状态（没有托管状态时静默跳过——启动早期的调用不该 panic）。
fn edit_status<R: Runtime>(app: &AppHandle<R>, f: impl FnOnce(&mut UpdateStatus)) {
    if let Some(state) = app.try_state::<AppState>() {
        f(&mut state.update.lock().unwrap());
    }
}

/// 进入失败态并写一行日志（设置页直接显示 `message`）。
fn fail<R: Runtime>(app: &AppHandle<R>, message: impl Into<String>) {
    let message = message.into();
    flog!("[fnos] 更新失败：{message}");
    edit_status(app, |s| {
        s.phase = phase::ERROR;
        s.message = message;
        s.bytes_per_sec = None;
    });
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

/// 从 release JSON 里挑出 NSIS 安装包资产，返回（下载地址，文件名，期望的 sha256）。
///
/// 优先 `fnOS_…x64-setup.exe`（本壳的命名形态：productName = fnOS），其余
/// `*x64-setup.exe` 兜底；都没有 → Err（如实告知，不猜别的资产）。
///
/// 第三个返回值是 GitHub Releases API 的 `assets[].digest`（`sha256:<hex>`）里的 hex 部分。
/// **它可能不存在**（GitHub 对 2024 年前上传的资产不回填 digest，本仓库早期的 release 就是
/// 这种），所以类型是 `Option<String>`，由下载路径决定「没有 digest 时怎么办」——
/// 见 [`download_installer_with_progress`] 的保守策略。
///
/// **失败关闭**：资产带了 `digest` 字段但认不出来（别的算法 / 长度不对 / 非字符串）时
/// 直接 `Err`，绝不退化成「没有 digest」——那会让一次格式漂移悄悄关掉校验。
pub fn pick_installer_asset(release: &Value) -> Result<(String, String, Option<String>), String> {
    let assets = release
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "release 响应里没有 assets".to_string())?;
    let mut fallback: Option<(String, String, Option<String>)> = None;
    for asset in assets {
        let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let url = asset
            .get("browser_download_url")
            .and_then(|u| u.as_str())
            .unwrap_or("");
        if !name.ends_with("x64-setup.exe") || url.is_empty() {
            continue;
        }
        let digest = asset_sha256(asset)?;
        if name.starts_with("fnOS_") {
            return Ok((url.to_string(), name.to_string(), digest));
        }
        if fallback.is_none() {
            fallback = Some((url.to_string(), name.to_string(), digest));
        }
    }
    fallback.ok_or_else(|| "发布页上没有找到 x64 安装包（*x64-setup.exe）".to_string())
}

/// 读取一个 release 资产的 digest：
/// - 字段缺失 / `null` → `None`（老 release 的既有形态）；
/// - 字段存在但无法识别（非字符串 / 非 `sha256:<64 hex>`）→ `Err`（失败关闭）。
fn asset_sha256(asset: &Value) -> Result<Option<String>, String> {
    match asset.get("digest") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_str().ok_or_else(|| {
                "release 资产的 digest 不是字符串，已拒绝（无法校验）".to_string()
            })?;
            parse_sha256_digest(raw)
                .map(Some)
                .ok_or_else(|| format!("release 资产的 digest 无法识别（{raw}），已拒绝（无法校验）"))
        }
    }
}

/// 解析 GitHub 的 `digest` 字段：只接受 `sha256:` + 64 位十六进制，其余（别的算法、
/// 长度不对、带空白）一律当「没有 digest」——绝不把认不出的形状当成校验值来用。
fn parse_sha256_digest(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex.to_ascii_lowercase())
}

/// 一个文件的 sha256（小写 hex）。读失败返回 `None`（调用方按「校验不了」处理）。
fn file_sha256(path: &std::path::Path) -> Option<String> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    Some(out)
}

/// 「这个文件能不能当完整安装包用」的判定。
///
/// 有期望哈希时**只认哈希**（大小对、内容错的文件正是 `updateMirrorPrefix` 场景下最可能的
/// 坏结果：镜像返回了一个 HTML 错误页或另一个版本）；没有期望哈希时退回体积下限
/// （保留了 0.3.9 之前的行为：老 release 没有 digest 字段）。
fn installer_looks_complete(path: &std::path::Path, expected_sha256: Option<&str>) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() < MIN_INSTALLER_BYTES {
        return false;
    }
    match expected_sha256 {
        Some(expected) => file_sha256(path).as_deref() == Some(expected),
        None => true,
    }
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

/// 把安装包下载到配置目录 `updates/<文件名>`，返回落盘路径与「是否复用了已有文件」。
///
/// 写 `.part` 临时名、完成后改名，避免留下半截文件被当成完整安装包。
///
/// ## 复用与完整性（P1-5）
///
/// 复用分支原先只比体积（≥ 1 MB 就当成「上次下载好了」）。配上 `updateMirrorPrefix` 之后
/// 这条捷径是危险的：`updates/` 里一个被替换过或半截的同名 exe 会被直接拿去静默安装。
/// 现在复用前必须过 [`installer_looks_complete`]——**有 digest 时严格比对 sha256**，
/// 不匹配就删掉重下。
/// 把 `reader` 分块写进 `writer`，**每块回调一次进度**；超过 `max_bytes` 立即失败。
///
/// 抽成独立函数是为了让「进度单调递增、最后一次是完整大小、超限即停」成为可单测的事实
/// ——HTTP 层本身不可单测（`https_only(true)` + 真实证书），分块循环是这里唯一有分支的逻辑。
fn copy_with_progress(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    max_bytes: u64,
    declared_len: Option<u64>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<u64, String> {
    let mut size: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                size = size.saturating_add(n as u64);
                if size > max_bytes {
                    return Err(format!("安装包超过体积上限（>{max_bytes} 字节），已丢弃"));
                }
                writer
                    .write_all(&buf[..n])
                    .map_err(|e| format!("下载中断：{e}"))?;
                progress(size, declared_len);
            }
            Err(e) => return Err(format!("下载中断：{e}")),
        }
    }
    Ok(size)
}

/// 带进度回调的下载实现：`progress(已下载字节, 声明的总字节)`。
///
/// 回调在**下载线程**里被同步调用（节流由调用方做），只做「写状态」这种廉价动作。
///
/// 目标目录是**参数**（生产走 `config_dir()/updates`）：单测可以直接用临时目录，
/// 不碰真实配置目录。
fn download_installer_with_progress(
    dir: &std::path::Path,
    download_url: &str,
    file_name: &str,
    mirror_prefix: &str,
    expected_sha256: Option<&str>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
    let dest = dir.join(file_name);
    // P1-6：`.part` 名带进程 id。两次「检查更新」并发时（快速连点托盘菜单）它们各写各的
    // 临时文件，不会互相截断；最后两个线程都 rename 到同一个 dest，内容同源同哈希，
    // 谁赢都对。
    let part = dir.join(format!("{file_name}.{}.part", std::process::id()));

    if installer_looks_complete(&dest, expected_sha256) {
        return Ok(dest); // 上一次已经下载好了（比如点了「取消」后再次检查）
    }
    if dest.exists() {
        // 存在但不可用（半截 / 哈希对不上）：删掉，绝不把它当安装包用
        flog!("[fnos] 更新缓存不可用（体积或哈希不符），重新下载：{}", dest.display());
        let _ = std::fs::remove_file(&dest);
    }

    // 直连不通（os error 10060 这类）时按配置的镜像前缀重试；瞬态超时做连接级重试。
    let target = resolve_download_url(download_url, mirror_prefix)?;
    // 连接超时单列 15s：os error 10060 这类连接失败必须快速返回，
    // 不然按总超时 120s 重试一次就是最长 4 分钟白等。
    let agent = ureq::AgentBuilder::new()
        // 只允许 HTTPS：默认会跟随 3xx，若某次重定向被降级到 http，TLS 保护就静默消失了。
        .https_only(true)
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build();
    let mut last_err: Option<String> = None;
    let mut reader = None;
    let mut declared_len: Option<u64> = None;
    for attempt in 1..=DOWNLOAD_CONNECT_TRIES {
        match agent
            .get(&target)
            .set("User-Agent", concat!("fnos-desktop/", env!("CARGO_PKG_VERSION")))
            .call()
        {
            Ok(resp) => {
                // 先看声明的长度：超上限的响应连临时文件都不该创建
                if let Some(len) = resp
                    .header("Content-Length")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                {
                    if len > MAX_INSTALLER_BYTES {
                        return Err(format!(
                            "安装包体积异常（声明 {len} 字节，超过上限 {MAX_INSTALLER_BYTES} 字节），已放弃下载"
                        ));
                    }
                    declared_len = Some(len);
                }
                reader = Some(resp.into_reader());
                break;
            }
            Err(e) => {
                last_err = Some(format!("{e}"));
                if attempt < DOWNLOAD_CONNECT_TRIES {
                    std::thread::sleep(std::time::Duration::from_millis(DOWNLOAD_RETRY_DELAY_MS));
                }
            }
        }
    }
    let Some(mut reader) = reader else {
        return Err(format!(
            "{}。直连 GitHub 不稳定时，可在 config.json 的 shell.updateMirrorPrefix 配置加速前缀（形如 https://your-mirror.example.com/ ，前缀 + 原始 GitHub 地址）后重试",
            last_err.unwrap_or_else(|| "未知错误".into())
        ));
    };
    let mut file = std::fs::File::create(&part)
        .map_err(|e| format!("写安装包失败：{e}"))?;
    // 上限兜底：Content-Length 可能缺失/撒谎，流式写入必须有硬上限，
    // 否则一个坏镜像（或错误页）能把系统盘写满。
    //
    // 手写分块循环（而不是 `io::copy`）是为了**每块都能报进度**：设置页的进度条
    // 直接读这里写下的字节数。块大小 64 KiB——每秒几百次回调，节流在调用方。
    let copied = copy_with_progress(
        reader.as_mut(),
        &mut file,
        MAX_INSTALLER_BYTES,
        declared_len,
        progress,
    );
    drop(file);
    let size = match copied {
        Ok(size) => size,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    if size < MIN_INSTALLER_BYTES {
        let _ = std::fs::remove_file(&part);
        return Err(format!("下载的安装包不完整（{size} 字节）"));
    }
    progress(size, declared_len.or(Some(size)));
    // P1-5：下载完成后的**内容**校验。配了镜像前缀时，GitHub 的 HTTPS 不再是唯一来源，
    // 所以「大小够」不代表「内容对」——必须拿 release API 给的 digest 比对；没有 digest
    // 的老 release 退回体积判定（并在日志里如实标出来，免得把「没校验」当成「校验通过」）。
    if let Some(expected) = expected_sha256 {
        let actual = file_sha256(&part);
        if actual.as_deref() != Some(expected) {
            let _ = std::fs::remove_file(&part);
            return Err(format!(
                "安装包校验失败（sha256 不匹配，期望 {expected}，实际 {}）。已丢弃，请重试；若反复出现请检查 shell.updateMirrorPrefix 指向的镜像是否可信。",
                actual.unwrap_or_else(|| "读取失败".into())
            ));
        }
    } else {
        flog!(
            "[fnos] 该 release 的资产没有 digest 字段，安装包只做了体积校验（{} 字节）",
            size
        );
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
fn spawn_installer_with_retry(
    installer: &std::path::Path,
    retry_delay: std::time::Duration,
) -> Result<std::process::Child, String> {
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
            Ok(child) => return Ok(child),
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
    let mut child = spawn_installer_with_retry(
        installer,
        std::time::Duration::from_millis(INSTALL_SPAWN_RETRY_DELAY_MS),
    )
    .map_err(|e| format!("{e}（安装包路径：{}；也可以稍后在文件管理器里手动运行它）", installer.display()))?;
    // 安装器（onInit 阶段）会结束本进程；万一没结束，这里主动退，把文件锁让出来。
    // 但先给安装器一个失败窗口：SmartScreen/杀软拦截或用户取消时它会很快退出——
    // 这时外壳必须留下来，否则用户既没更新，也没了正在用的窗口。
    std::thread::sleep(std::time::Duration::from_millis(800));
    if let Ok(Some(status)) = child.try_wait() {
        if !status.success() {
            return Err(format!(
                "安装器启动后立即退出（{status}），可能被 SmartScreen / 杀毒软件拦截。\n\n安装包路径：{}；可在文件管理器里手动运行。",
                installer.display()
            ));
        }
    }
    save_window_geom(app);
    app.exit(0);
    Ok(())
}

/// 「检查更新」的重入闸门（P1-6）。
///
/// 设置页的「检查更新」按钮在状态进入 checking/downloading 后会置灰，但轮询有间隔、
/// 用户也可能连点；第一次下载最长 120 秒。两次并发下载各写各的 `.part` 本身已经安全
/// （见下载实现的进程 id 后缀），但并发两轮会让状态互相覆盖、还白下一遍。用一次 CAS
/// 让后来者直接复用当前这一轮的状态（`start_update_check` 返回的就是它）。
static UPDATE_CHECK_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 重入闸门的守卫：无论函数从哪条路径返回，`Drop` 都会把闸门放开。
struct UpdateCheckGuard;

impl UpdateCheckGuard {
    /// 抢闸门；已在运行则返回 `None`。
    fn acquire() -> Option<Self> {
        use std::sync::atomic::Ordering;
        if UPDATE_CHECK_RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Some(Self)
        } else {
            None
        }
    }
}

impl Drop for UpdateCheckGuard {
    fn drop(&mut self) {
        UPDATE_CHECK_RUNNING.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// 设置页「检查更新」的入口（**阻塞**函数；命令侧用 `spawn_blocking` 调用）。
///
/// 全程只写 [`UpdateStatus`]：没有消息框、没有弹窗——设置页轮询状态自己渲染。
pub fn run_check<R: Runtime>(app: AppHandle<R>) {
    let Some(_guard) = UpdateCheckGuard::acquire() else {
        return; // 已在跑：设置页读到的就是那一轮的状态
    };
    edit_status(&app, |s| {
        *s = UpdateStatus {
            phase: phase::CHECKING,
            message: "正在检查更新…".into(),
            ..UpdateStatus::default()
        };
    });
    let current = current_version();
    let mirror_prefix = crate::commands::current(&app).shell.update_mirror_prefix;
    let release = match fetch_latest_release() {
        Ok(r) => r,
        Err(e) => {
            return fail(
                &app,
                format!("检查更新失败：{e}。请确认本机可以访问 github.com。"),
            );
        }
    };
    let tag = release
        .get("tag_name")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    edit_status(&app, |s| s.latest = Some(tag.clone()));
    // 两条「无法识别」先各自如实报告（比一句笼统的「没有更新」有用得多），
    // 真正的比较只走 `is_newer` 这一处——两处口径分叉正是这类版本判断最容易出的错。
    if parse_tag_version(&tag).is_none() {
        return fail(&app, format!("线上版本号无法识别（tag：{tag}）。"));
    }
    if parse_tag_version(current).is_none() {
        return fail(&app, format!("当前版本号无法识别（{current}）。"));
    }
    if !is_newer(&tag, current) {
        edit_status(&app, |s| {
            s.phase = phase::UP_TO_DATE;
            s.message = format!("已是最新版本（v{current}）。");
        });
        return;
    }

    let (download_url, file_name, expected_sha256) = match pick_installer_asset(&release) {
        Ok((url, name, digest)) if safe_asset_name(&name) => (url, name, digest),
        Ok((_, name, _)) => {
            return fail(&app, format!("资产文件名不可用（{name}），已放弃下载。"));
        }
        Err(e) => return fail(&app, e),
    };
    let validated = match validate_download_url(&download_url) {
        Ok(u) => u,
        Err(e) => return fail(&app, e),
    };
    edit_status(&app, |s| {
        s.phase = phase::DOWNLOADING;
        s.message = format!("正在下载 {tag}…");
        s.downloaded = 0;
        s.total = None;
        s.bytes_per_sec = None;
        s.has_digest = expected_sha256.is_some();
    });

    // 进度节流：每 120ms（以及最后一包）写一次状态。64 KiB 一块，慢网络下也够细腻；
    // 不节流的话每秒几百次 Mutex 争用纯属浪费。
    let mut last_report = std::time::Instant::now();
    let mut last_bytes: u64 = 0;
    let app_for_progress = app.clone();
    let updates_dir = config_dir().join("updates");
    let result = download_installer_with_progress(
        &updates_dir,
        &validated,
        &file_name,
        &mirror_prefix,
        expected_sha256.as_deref(),
        &mut |done, total| {
            let now = std::time::Instant::now();
            if now.duration_since(last_report).as_millis() < 120 && Some(done) != total {
                return;
            }
            let dt = now.duration_since(last_report).as_secs_f64().max(0.001);
            let speed = (done.saturating_sub(last_bytes) as f64 / dt) as u64;
            last_report = now;
            last_bytes = done;
            edit_status(&app_for_progress, |s| {
                s.phase = phase::DOWNLOADING;
                s.downloaded = done;
                s.total = total;
                s.bytes_per_sec = Some(speed);
            });
        },
    );
    match result {
        Ok(path) => edit_status(&app, |s| {
            s.phase = phase::READY;
            s.downloaded = s.total.unwrap_or(s.downloaded);
            s.bytes_per_sec = None;
            s.installer = Some(path.display().to_string());
            // 没有 digest 的老 release 只能按体积判定：这是真实的安全缺口，安装按钮旁
            // 必须明确写出来（不能只在 app.log 里留一行用户永远看不到的字）。
            s.message = if s.has_digest {
                format!("新版本 {tag} 已下载（sha256 已校验）。")
            } else {
                format!("新版本 {tag} 已下载（该 release 没有 digest，只做了体积校验）。")
            };
        }),
        Err(e) => fail(&app, e),
    }
}

/// 安装包路径的准入：必须是 `config_dir()/updates/` 下的普通文件。
///
/// `installer` 由宿主自己写进状态，但设置页也会读到它；安装前再校验一次，让
/// 「谁把状态改坏了」不可能变成「执行任意 exe」。
pub fn installer_path_is_trusted(path: &std::path::Path) -> bool {
    let Ok(dir) = config_dir().join("updates").canonicalize() else {
        return false;
    };
    let Ok(file) = path.canonicalize() else {
        return false;
    };
    file.starts_with(&dir) && file.is_file()
}

/// 设置页「立即安装并重启」的入口（阻塞；命令侧 `spawn_blocking`）。
pub fn install_ready<R: Runtime>(app: AppHandle<R>) {
    let (phase_now, installer) = {
        let s = status(&app);
        (s.phase, s.installer.clone())
    };
    if phase_now != phase::READY {
        return fail(&app, "还没有可安装的更新包，请先检查更新。");
    }
    let Some(installer) = installer else {
        return fail(&app, "更新包路径缺失，请重新检查更新。");
    };
    let path = std::path::PathBuf::from(&installer);
    if !installer_path_is_trusted(&path) {
        return fail(&app, "更新包路径不在配置目录的 updates/ 下，已拒绝安装。");
    }
    edit_status(&app, |s| {
        s.phase = phase::INSTALLING;
        s.message = "正在安装并重启…".into();
    });
    if let Err(e) = start_install(&app, &path) {
        fail(&app, e);
    }
}

/// 子进程不闪控制台窗口（taskkill / 安装器 spawn 共用）。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
                  "browser_download_url": "https://github.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/fnOS_0.2.0_x64-setup.exe",
                  "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" },
                { "name": "latest.json",
                  "browser_download_url": "https://github.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/latest.json" }
            ]
        });
        let (url, name, digest) = pick_installer_asset(&release).expect("必须挑中 NSIS 安装包");
        assert_eq!(name, "fnOS_0.2.0_x64-setup.exe");
        assert!(url.ends_with("/fnOS_0.2.0_x64-setup.exe"));
        assert_eq!(
            digest.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "资产带 digest 时必须把它带出来（P1-5 的校验依据）"
        );

        // 老 release 没有 digest 字段：不算错，返回 None（下载路径退回体积判定）
        let no_digest = json!({
            "tag_name": "v0.1.0",
            "assets": [{ "name": "fnOS_0.1.0_x64-setup.exe",
                         "browser_download_url": "https://github.com/eafenzhang/fnos-desktop/releases/download/v0.1.0/fnOS_0.1.0_x64-setup.exe" }]
        });
        assert_eq!(pick_installer_asset(&no_digest).unwrap().2, None);

        let empty = json!({ "tag_name": "v0.2.0", "assets": [] });
        assert!(pick_installer_asset(&empty).is_err(), "没有资产要如实失败");
        let none = json!({ "tag_name": "v0.2.0" });
        assert!(pick_installer_asset(&none).is_err(), "缺 assets 字段要如实失败");

        // digest 字段存在但认不出（别的算法 / 非字符串 / 空串）：必须失败关闭，
        // 不得退化成「没有 digest」继续按体积放行。
        for bad_digest in [json!("md5:abcdef"), json!(5), json!("")] {
            let release = json!({
                "tag_name": "v0.2.0",
                "assets": [{
                    "name": "fnOS_0.2.0_x64-setup.exe",
                    "browser_download_url": "https://github.com/eafenzhang/fnos-desktop/releases/download/v0.2.0/fnOS_0.2.0_x64-setup.exe",
                    "digest": bad_digest
                }]
            });
            assert!(
                pick_installer_asset(&release).is_err(),
                "无法识别的 digest 必须拒绝（失败关闭）"
            );
        }
    }

    /// `digest` 只认 `sha256:` + 64 位十六进制。别的算法 / 长度不对 / 带杂字符一律当
    /// T14g：设置页按这组阶段字符串渲染文案与控件，两处必须逐字一致
    /// （前端 `tests/settings-page.test.mjs` 锁同一组字面量）。
    #[test]
    fn update_status_phases_and_serialization_are_stable() {
        assert_eq!(phase::IDLE, "idle");
        assert_eq!(phase::CHECKING, "checking");
        assert_eq!(phase::UP_TO_DATE, "upToDate");
        assert_eq!(phase::DOWNLOADING, "downloading");
        assert_eq!(phase::READY, "ready");
        assert_eq!(phase::INSTALLING, "installing");
        assert_eq!(phase::ERROR, "error");

        let idle = UpdateStatus::default();
        assert_eq!(idle.current, current_version(), "当前版本必须来自 Cargo.toml");
        assert!(!idle.is_running());
        let json = serde_json::to_value(&idle).expect("状态必须可序列化");
        assert_eq!(json["phase"], "idle");
        assert_eq!(json["current"], current_version());
        assert!(
            json.get("bytesPerSec").is_some() && json.get("hasDigest").is_some(),
            "键名必须是 camelCase（设置页直接读）"
        );
        assert!(json.get("bytes_per_sec").is_none());

        for (p, running) in [
            (phase::CHECKING, true),
            (phase::DOWNLOADING, true),
            (phase::INSTALLING, true),
            (phase::UP_TO_DATE, false),
            (phase::READY, false),
            (phase::ERROR, false),
        ] {
            let s = UpdateStatus {
                phase: p,
                ..UpdateStatus::default()
            };
            assert_eq!(s.is_running(), running, "{p} 的 is_running 判据");
        }
    }

    /// T14g：下载进度回调的语义——分块推进、单调不减、最后一次是完整大小、
    /// 声明长度原样透传、超限立即失败（不留半截）。
    #[test]
    fn copy_with_progress_reports_each_chunk_and_enforces_the_cap() {
        let body = vec![0x5au8; 200 * 1024 + 7];
        let mut reader = std::io::Cursor::new(body.clone());
        let mut out: Vec<u8> = Vec::new();
        let mut seen: Vec<u64> = Vec::new();
        let mut totals: Vec<Option<u64>> = Vec::new();
        let size = copy_with_progress(
            &mut reader,
            &mut out,
            1024 * 1024,
            Some(body.len() as u64),
            &mut |done, total| {
                seen.push(done);
                totals.push(total);
            },
        )
        .expect("上限之内必须成功");
        assert_eq!(size, body.len() as u64);
        assert_eq!(out, body, "写出的字节必须与输入逐字节一致");
        assert!(seen.len() >= 3, "200 KiB / 64 KiB 块至少三次回调：{seen:?}");
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "进度必须严格递增");
        assert_eq!(seen.last().copied(), Some(body.len() as u64), "最后一次必须是完整大小");
        assert_eq!(totals[0], Some(body.len() as u64), "声明长度必须原样透传");

        // 超限：立即失败（不会先写满上限再报错），且错误信息带上限
        let mut reader = std::io::Cursor::new(vec![0u8; 300 * 1024]);
        let mut out: Vec<u8> = Vec::new();
        let err = copy_with_progress(&mut reader, &mut out, 128 * 1024, None, &mut |_, _| {})
            .unwrap_err();
        assert!(err.contains("体积上限"), "错误必须说明原因：{err}");
        assert!(out.len() <= 128 * 1024, "超限时写出的字节不得超过上限");
    }

    /// 安装包路径的准入只认 `config_dir()/updates/` 下的普通文件——状态被改坏也不能变成
    /// 「执行任意 exe」。这里测负例（不碰真实配置目录）：不存在的路径、updates 目录之外的文件。
    #[test]
    fn installer_path_outside_updates_is_rejected() {
        let outside = std::env::temp_dir().join(format!("fnos-outside-{}.exe", std::process::id()));
        std::fs::write(&outside, b"MZ").unwrap();
        assert!(
            !installer_path_is_trusted(&outside),
            "updates/ 之外的文件必须拒绝"
        );
        let _ = std::fs::remove_file(&outside);
        assert!(
            !installer_path_is_trusted(&std::env::temp_dir().join("does-not-exist.exe")),
            "不存在的路径必须拒绝"
        );
        // 源码锁：判据必须真的锚在 updates 目录上（不能只判 is_file）
        let code = code();
        assert!(
            code.contains("config_dir().join(\"updates\").canonicalize()")
                && code.contains("file.starts_with(&dir)"),
            "准入必须做「canonicalize 后前缀比对」"
        );
    }

    /// 「没有 digest」——把认不出的形状当校验值用，等于用垃圾数据做安全判定。
    #[test]
    fn sha256_digest_is_parsed_strictly() {
        let hex64 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(parse_sha256_digest(&format!("sha256:{hex64}")).as_deref(), Some(hex64));
        // 大写十六进制允许（GitHub 用小写，但别的地方可能大写），统一折成小写
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{}", hex64.to_uppercase())).as_deref(),
            Some(hex64)
        );
        assert_eq!(parse_sha256_digest(&format!(" sha256:{hex64} ")).as_deref(), Some(hex64));

        for bad in [
            "".to_string(),
            hex64.to_string(),                                   // 缺算法前缀
            format!("sha512:{hex64}"),                           // 别的算法
            format!("sha256:{}", &hex64[..63]),                  // 少一位
            format!("sha256:{hex64}0"),                          // 多一位
            format!("sha256:{}", "g".repeat(64)),                // 非十六进制
            "sha256:".to_string(),
        ] {
            assert_eq!(parse_sha256_digest(&bad), None, "必须拒绝：{bad}");
        }
    }

    /// 复用判定：有期望哈希时**只认哈希**；没有时退回体积下限。
    #[test]
    fn installer_reuse_requires_the_content_to_match() {
        let dir = std::env::temp_dir().join(format!("fnos-upd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("fnOS_9.9.9_x64-setup.exe");
        // 灌到体积下限之上（内容随意，这里只验证「哈希说了算」）
        let payload = vec![0x41u8; MIN_INSTALLER_BYTES as usize + 16];
        std::fs::write(&file, &payload).unwrap();

        let good = file_sha256(&file).expect("必须能算出哈希");
        assert!(
            installer_looks_complete(&file, Some(&good)),
            "哈希对得上 → 可复用"
        );
        assert!(
            !installer_looks_complete(&file, Some(&"0".repeat(64))),
            "哈希对不上 → 绝不能复用（这正是镜像返回错误内容的情形）"
        );
        assert!(
            installer_looks_complete(&file, None),
            "没有 digest 的老 release：退回体积判定"
        );

        // 体积不到下限：无论有没有哈希都不可用
        let small = dir.join("small.exe");
        std::fs::write(&small, b"x").unwrap();
        assert!(!installer_looks_complete(&small, None));
        assert!(!installer_looks_complete(&dir.join("missing.exe"), None));

        let _ = std::fs::remove_dir_all(&dir);
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
        // T14g：入口从托盘移到设置页——状态机 + 命令侧 spawn_blocking
        assert!(
            code.contains("pub fn run_check") && code.contains("pub fn install_ready"),
            "设置页入口（run_check / install_ready）必须在"
        );
        assert!(
            code.contains("download_installer_with_progress"),
            "下载必须走带进度回调的实现（设置页的实时进度条依赖它）"
        );
        // 用户要求：自动下载安装，而不是打开页面
        assert!(
            code.contains("download_installer_with_progress") && code.contains("start_install"),
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
        assert!(
            code.contains("try_wait()") && code.contains("!status.success()"),
            "安装器启动后立即失败时必须保留外壳并报错，不能直接退出"
        );
    }

    /// 镜像前缀校验（T14d：Release 资产域名直连 os error 10060，用户可手配加速前缀）。
    /// 安全边界：只放行 https；拒绝 localhost / 环回 / 私有 / 内网 / 保留地址 / userinfo。
    #[test]
    fn mirror_prefix_is_https_and_public_hosts_only() {
        assert_eq!(validate_mirror_prefix("").unwrap(), "", "空 = 不用镜像");
        assert_eq!(validate_mirror_prefix("  ").unwrap(), "", "空白同空");
        assert_eq!(
            validate_mirror_prefix("https://gh-proxy.example.com/").unwrap(),
            "https://gh-proxy.example.com",
            "公网 https 且剥尾斜杠"
        );
        for bad in [
            "http://gh-proxy.example.com/",                    // 非 https
            "https://user:pass@gh-proxy.example.com/",         // 带 userinfo
            "https://localhost:8080/",                         // 本机
            "https://127.0.0.1/",                              // 环回
            "https://::1/",
            "https://192.168.1.5/",                            // 私有网段
            "https://10.1.2.3/",
            "https://172.16.0.9/",
            "https://100.64.0.1/",                             // CGNAT
            "https://169.254.0.1/",                            // 链路本地
            "https://[fd00::1]/",                              // ULA
            "https://[::ffff:127.0.0.1]/",                     // IPv4-mapped 环回（旧判定漏网）
            "https://[::ffff:c0a8:105]/",                      // IPv4-mapped 192.168.1.5
            "https://foo.localhost/",                          // RFC 6761：*.localhost 全部是环回
            "not a url",
        ] {
            assert!(validate_mirror_prefix(bad).is_err(), "必须拒绝：{bad}");
        }
        assert!(
            validate_mirror_prefix("https://[2001:4860:4860::8888]/").is_ok(),
            "公网 IPv6 字面量必须放行"
        );
        // 拼接形态：前缀 + 完整原始 URL（ghproxy 系加速服务通用格式）
        let raw = format!(
            "https://{DOWNLOAD_HOST}/{APP_REPO_SLUG}{DOWNLOAD_PATH_PREFIX}v0.9.9/fnOS_0.9.9_x64-setup.exe"
        );
        let no_mirror = resolve_download_url(&raw, "").unwrap();
        assert_eq!(no_mirror, raw, "未配镜像 = 原始地址直连");
        let mirrored = resolve_download_url(&raw, "https://gh-proxy.example.com/").unwrap();
        assert!(mirrored.starts_with("https://gh-proxy.example.com/https://github.com/"),
            "镜像形态 = 前缀 + 原始地址（实测 {mirrored}）");
        // 原始地址本身仍要过 GitHub 钉死校验（镜像不得洗掉它）
        assert!(
            resolve_download_url("https://evil.example.com/anything.exe", "https://gh-proxy.example.com/").is_err(),
            "镜像不得绕过原始 URL 的 host/路径校验"
        );

        // IPv6 地址段判定不得误伤**以 fc/fd 开头的普通域名**——这是 `starts_with("fc")`
        // 那版判定的实际风险（`fcdn.*` / `fdroid-mirror.*` 这类镜像域名会被无故拒绝）。
        for domain in [
            "https://fcdn.example.com/",
            "https://fdroid-mirror.example.org/",
            "https://fc.example.net/",
        ] {
            assert!(
                validate_mirror_prefix(domain).is_ok(),
                "以 fc/fd 开头的公网域名必须放行：{domain}"
            );
        }
        // IPv6 字面量才按地址段拒绝（含 IPv4 映射形式与 ULA）
        for bad in [
            "https://[fe80::1]/",
            "https://[::]/",
            "https://[::1]/",
            "https://[fd12:3456::1]/",
            "https://[fc00::1]/",
        ] {
            assert!(validate_mirror_prefix(bad).is_err(), "IPv6 保留段必须拒绝：{bad}");
        }
    }
}
