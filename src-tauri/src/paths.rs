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
