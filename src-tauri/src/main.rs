mod config;
mod paths;

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
