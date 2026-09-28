use std::path::{Path, PathBuf};

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

// ---------- 随包许可件（spec §10 / Ruling R54） ----------

/// 许可全文在**资源目录**内的相对路径。
///
/// 必须与 `tauri.conf.json` 的 `bundle.resources` 映射**右侧**逐字一致：
/// 左边是源码路径（相对 `src-tauri/`），右边就是这里的值。两处漂移会让关于页指向一个
/// 不存在的文件，因此改这里就要同步改配置（反向同理）。
pub const LICENSE_RESOURCE: &str = "fnos-mods/LICENSE";

/// 来源与改动声明（来源仓库 + 锁定 commit + 各文件 SHA-256 + 本壳的包装性改动清单）。
pub const NOTICE_RESOURCE: &str = "fnos-mods/NOTICE";

/// 源码树里 vendored 资源的位置。仅用于 `resource_dir()` 取不到时兜底
/// （与 `ui/settings/schema.js` 的 `VENDOR_DIR` 同值）。
const VENDOR_SOURCE_DIR: &str = "src-tauri/assets/fnos-mods";

/// 解析一个随包合规件的**实际**位置。
///
/// 为什么用资源目录（R54 的落地依据）：
/// - `resource_dir()` 在 Windows 上就是**主程序所在目录**
///   （`tauri-utils-2.10.0/src/platform.rs:261,272-310`），安装后即
///   `%LOCALAPPDATA%\fnOS\fnos-mods\...`；
/// - NSIS 按 `File /a "/oname=<映射右侧>"` 落到 `$INSTDIR\<右侧>`
///   （`tauri-bundler-2.10.0/src/bundle/windows/nsis/installer.nsi:651-657`），卸载时删同一路径；
/// - 普通 `cargo build` 也会把 resources 拷到 `target/(debug|release)/` 下
///   （`tauri-build-2.7.0/src/lib.rs:171-190` 的 `copy_resources`，由 `build.rs` 走到），
///   所以开发版与安装版报出的是**同一个形状**的路径，且都存在。
///
/// `resource_dir()` 失败（理论上不会）时退回源码树位置：宁可给一个与设置窗
/// `VENDOR_DIR` 文案一致的相对路径，也不给空串。
pub fn compliance_path(resource_dir: Option<&Path>, rel: &str) -> String {
    let raw = match resource_dir {
        // 逐段 push：`rel` 按 `bundle.resources` 映射右侧的写法用 `/` 分隔，而 Windows 上
        // `Path::join` **不会**把 `/` 换成本地分隔符，直接 join 会报出
        // `…\fnOS\fnos-mods/LICENSE` 这种混用分隔符的字符串。
        Some(dir) => {
            let mut path = dir.to_path_buf();
            path.extend(rel.split('/'));
            path.display().to_string()
        }
        // 回落路径与设置窗 `schema.js` 的 `VENDOR_DIR` 写法一致（正斜杠）
        None => {
            let name = Path::new(rel).file_name().unwrap_or_else(|| rel.as_ref());
            format!("{VENDOR_SOURCE_DIR}/{}", name.display())
        }
    };
    // T12 装机实测（安装版「关于」页的 UIA 读数）：`resource_dir()` 返回的是 `\\?\C:\...`，
    // 见 [`strip_verbatim_prefix`]。
    strip_verbatim_prefix(&raw).to_string()
}

/// 去掉 Windows 的 `\\?\` 逐字（verbatim）前缀——**仅在盘符形式**时。
///
/// 为什么会有这个前缀：`resource_dir()` 的源头是
/// `std::env::current_exe().canonicalize()`
/// （`tauri-utils-2.10.0/src/platform/starting_binary.rs:40`），而 Windows 上的
/// `canonicalize` 返回逐字路径 `\\?\C:\...`。它**等价**于 `C:\...`（同一文件），
/// 但「关于」页的用途正是把路径给人看、让人复制到资源管理器/终端里；实测安装版页面上
/// 显示的就是 `\\?\C:\Users\…\fnOS\fnos-mods\LICENSE`，噪音大且容易被当成损坏的路径。
///
/// 只剥盘符形式（`\\?\C:\` / `\\?\C:/`）。`\\?\UNC\server\share` 与
/// `\\?\Volume{…}\` 不做转换——它们剥掉前缀会变成**另一个**路径，宁可不改。
fn strip_verbatim_prefix(path: &str) -> &str {
    let Some(rest) = path.strip_prefix(r"\\?\") else {
        return path;
    };
    let b = rest.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
    {
        rest
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 资源相对路径同时是 `tauri.conf.json` 映射右侧的契约：改这里就得改那里。
    #[test]
    fn resource_relpaths_match_the_bundle_mapping() {
        assert_eq!(LICENSE_RESOURCE, "fnos-mods/LICENSE");
        assert_eq!(NOTICE_RESOURCE, "fnos-mods/NOTICE");
    }

    /// 资源目录可用时，报出的必须是**资源目录下**的路径（安装后的真实路径）。
    #[test]
    fn compliance_path_uses_the_resource_dir_when_available() {
        let dir = Path::new(r"C:\Users\x\AppData\Local\fnOS");
        assert_eq!(
            compliance_path(Some(dir), LICENSE_RESOURCE),
            r"C:\Users\x\AppData\Local\fnOS\fnos-mods\LICENSE"
        );
        assert_eq!(
            compliance_path(Some(dir), NOTICE_RESOURCE),
            r"C:\Users\x\AppData\Local\fnOS\fnos-mods\NOTICE"
        );
        // 绝不能报出源码树路径——那正是 T9 评审的 ⚠️（安装后文件不在那儿）
        assert!(!compliance_path(Some(dir), LICENSE_RESOURCE).contains("assets"));
    }

    /// 资源目录取不到时退回源码树相对路径，写法与 `schema.js` 的 `VENDOR_DIR` 一致。
    #[test]
    fn compliance_path_falls_back_to_the_source_tree() {
        assert_eq!(
            compliance_path(None, LICENSE_RESOURCE),
            "src-tauri/assets/fnos-mods/LICENSE"
        );
        assert_eq!(
            compliance_path(None, NOTICE_RESOURCE),
            "src-tauri/assets/fnos-mods/NOTICE"
        );
    }

    /// 安装版的真实读数就是 `\\?\C:\...`（`current_exe().canonicalize()` 的产物），
    /// 关于页要展示的是**人能直接用的** `C:\...`。
    #[test]
    fn compliance_path_reports_a_usable_path_for_a_verbatim_resource_dir() {
        let dir = Path::new(r"\\?\C:\Users\x\AppData\Local\fnOS");
        assert_eq!(
            compliance_path(Some(dir), LICENSE_RESOURCE),
            r"C:\Users\x\AppData\Local\fnOS\fnos-mods\LICENSE"
        );
        // 没有前缀时不得改动
        assert_eq!(
            compliance_path(Some(Path::new(r"C:\x\fnOS")), LICENSE_RESOURCE),
            r"C:\x\fnOS\fnos-mods\LICENSE"
        );
    }

    /// 前缀只在**盘符**形式下剥掉；UNC / Volume 形式剥掉会得到另一个路径，必须原样保留。
    #[test]
    fn verbatim_prefix_is_stripped_only_for_drive_letter_paths() {
        assert_eq!(strip_verbatim_prefix(r"\\?\C:\a\b"), r"C:\a\b");
        assert_eq!(strip_verbatim_prefix(r"\\?\c:/a"), r"c:/a");
        assert_eq!(strip_verbatim_prefix(r"C:\a\b"), r"C:\a\b");
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\srv\share\a"),
            r"\\?\UNC\srv\share\a"
        );
        assert_eq!(
            strip_verbatim_prefix(r"\\?\Volume{0000}\a"),
            r"\\?\Volume{0000}\a"
        );
        assert_eq!(strip_verbatim_prefix(r"\\?\\a"), r"\\?\\a");
        assert_eq!(strip_verbatim_prefix(""), "");
    }
}
