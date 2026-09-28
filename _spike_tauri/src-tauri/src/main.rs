// 方案 C（Tauri 2）探针 —— throwaway spike，只用于验证可行性，不是正式代码。
//
// 验证 4 件事：
//   ① 窗口能否直接加载外部 https 站点（WebviewUrl::External）
//   ② initialization_script 是否在远程页面「文档解析前」执行（document_start 等价物）
//   ③ 真实 mods 产物（basic_mod.css 195KB）能否在远程页面注入并解析
//   ④ 托盘右键菜单（含勾选项/设置窗/退出）与主进程 eval 注入是否工作

use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    webview::PageLoadEvent,
    Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri::Runtime;

const TARGET_URL: &str = "https://fnos.net/";

/// 真实 mods 产物（仓库 main 分支，195500 字节），作为注入载荷做真实性验证。
const MODS_CSS: &str = include_str!("../../assets/basic_mod.css");

/// 第一段初始化脚本：把真实 mods CSS 内容交给页面上下文（JSON 转义，绝对安全）。
fn css_bootstrap() -> String {
    format!(
        "window.__FNOS_PROBE_CSS__ = {};",
        serde_json::to_string(MODS_CSS).expect("css 序列化失败")
    )
}

/// 第二段初始化脚本：document_start 时机执行 + 注入 + 自证面板。
const INIT_JS: &str = r#"(function () {
  var P = {
    initAt: performance.now(),
    readyStateAtInit: document.readyState,
    href: location.href,
    tauriInternals: typeof window.__TAURI_INTERNALS__,
    guardMatch: null,
    cssInjected: false,
    cssRulesEarly: null,
    cssRulesFinal: null,
    cssBytes: null,
    sheets: null,
    isTop: (window.top === window),
    cssError: null,
    csSquircle: null,
    csRound: null,
    dclAt: null,
    evalAt: null
  };
  window.__FNOS_PROBE__ = P;
  P.guardMatch = /(^|\.)fnos\.net$/i.test(location.hostname);

  function mountStyle() {
    var css = window.__FNOS_PROBE_CSS__ || '';
    if (!css) { P.cssError = 'css 载荷为空'; return true; }
    var root = document.head || document.documentElement;
    if (!root) return false;
    try {
      var st = document.createElement('style');
      st.id = '__fnos_mods_style__';
      st.textContent = css;
      root.appendChild(st);
      P.cssInjected = true;
      // 注意：appendChild 后立刻读 cssRules 会拿到「部分解析」的规则数（大表异步解析），
      // 这里只作早期参考，权威值在 panel() 里（DOMContentLoaded 之后）再读一次。
      try { P.cssRulesEarly = st.sheet ? st.sheet.cssRules.length : 'sheet-null'; }
      catch (e) { P.cssError = 'cssRules 读取被拒: ' + e; }
    } catch (e) { P.cssError = String(e); }
    return true;
  }
  if (!mountStyle()) {
    var iv = setInterval(function () { if (mountStyle()) clearInterval(iv); }, 1);
    setTimeout(function () { clearInterval(iv); }, 5000);
  }

  function lines() {
    return [
      '【方案C 探针】Tauri initialization_script 已在远程页面执行 —— 不是 eval，是文档解析前注入',
      'origin             = ' + location.origin + '    guard(*.fnos.net)=' + P.guardMatch,
      'readyState@init    = ' + P.readyStateAtInit + '    (loading 即为 document_start 等价时机)',
      'initAt / dclAt(ms) = ' + P.initAt.toFixed(1) + ' / ' + (P.dclAt === null ? '-' : P.dclAt.toFixed(1)),
      '注入目标           = ' + (P.isTop ? '顶层文档' : '子框架(iframe)'),
      '真实 mods CSS      = ' + (P.cssInjected
        ? ('已注入 ' + P.cssBytes + ' 字节(UTF-8)，解析出 ' + P.cssRulesFinal + ' 条规则 —— 早期读数 ' + P.cssRulesEarly + ' 条（证明异步解析）')
        : ('未注入：' + P.cssError)),
      'styleSheets        = ' + P.sheets + ' 个',
      'corner-shape 支持  = squircle:' + P.csSquircle + ' / round:' + P.csRound + '   (smooth 是非法值，故不用它判定)',
      'chromium           = ' + ((navigator.userAgent.match(/Chrome\/[\d.]+/) || ['?'])[0]),
      '__TAURI_INTERNALS__= ' + P.tauriInternals + '    (外部站点是否自动获得 IPC)',
      '主进程 eval 注入   = ' + (P.evalAt === null ? '尚未执行' : ('OK @ ' + P.evalAt.toFixed(1) + ' ms'))
    ];
  }

  function panel() {
    if (document.getElementById('__fnos_probe_panel__')) return;
    P.dclAt = performance.now();
    // 权威读数：等到 DOMContentLoaded 之后，样式表已解析完
    var st = document.getElementById('__fnos_mods_style__');
    try { P.cssRulesFinal = (st && st.sheet) ? st.sheet.cssRules.length : 'n/a'; } catch (e) { P.cssRulesFinal = 'blocked'; }
    try { P.cssBytes = window.__FNOS_PROBE_CSS__ ? new Blob([window.__FNOS_PROBE_CSS__]).size : 0; } catch (e) { P.cssBytes = 'n/a'; }
    P.sheets = document.styleSheets.length;
    if (window.CSS && CSS.supports) {
      P.csSquircle = CSS.supports('corner-shape', 'squircle');
      P.csRound = CSS.supports('corner-shape', 'round');
    } else { P.csSquircle = P.csRound = '无 CSS.supports'; }
    var el = document.createElement('div');
    el.id = '__fnos_probe_panel__';
    el.style.cssText = 'position:fixed;top:0;left:0;right:0;z-index:2147483647;' +
      'background:rgba(8,12,18,.96);color:#c8ffd4;border-bottom:2px solid #34d399;' +
      'font:12px/1.6 Consolas,monospace;padding:10px 14px;white-space:pre-wrap';
    el.textContent = lines().join('\n');
    (document.body || document.documentElement).appendChild(el);
  }

  window.__FNOS_PROBE_REFRESH__ = function () {
    var el = document.getElementById('__fnos_probe_panel__');
    if (el) el.textContent = lines().join('\n');
  };
  window.__FNOS_PROBE_MARK_EVAL__ = function () {
    P.evalAt = performance.now();
    window.__FNOS_PROBE_REFRESH__();
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', function () { setTimeout(panel, 60); });
  } else {
    setTimeout(panel, 60);
  }
})();"#;

/// 32x32 内存生成托盘图标（圆点），免去图片资源依赖。
fn tray_icon() -> Image<'static> {
    let (w, h) = (32u32, 32u32);
    let mut rgba: Vec<u8> = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            if (dx * dx + dy * dy).sqrt() < 14.0 {
                rgba.extend_from_slice(&[46, 144, 255, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Image::new_owned(rgba, w, h)
}

fn open_settings<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        println!("[probe] 设置窗已存在 -> 前置显示");
        return;
    }
    match WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("fnOS 设置（探针）")
        .inner_size(820.0, 580.0)
        .build()
    {
        Ok(_) => println!("[probe] ✅ 设置窗创建成功（本地 WebviewUrl::App，与外部站点窗口并存）"),
        Err(e) => println!("[probe] ❌ 设置窗创建失败: {e}"),
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            println!("[probe] setup 开始；tauri {}", tauri::VERSION);

            // ---------- 托盘 ----------
            let inject_item =
                CheckMenuItem::with_id(app, "inject", "注入 mods", true, true, None::<&str>)?;
            let toggle_item =
                MenuItem::with_id(app, "toggle", "显示 / 隐藏主窗口", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "系统设置", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(
                app,
                &[&inject_item, &toggle_item, &settings_item, &sep, &quit_item],
            )?;

            let inject_for_handler = inject_item.clone();
            TrayIconBuilder::with_id("main-tray")
                .icon(tray_icon())
                .tooltip("fnOS 桌面壳 · 方案C 探针")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "inject" => match inject_for_handler.is_checked() {
                        Ok(v) => println!("[probe] 托盘勾选项「注入 mods」-> {v}"),
                        Err(e) => println!("[probe] 读取勾选状态失败: {e}"),
                    },
                    "toggle" => {
                        if let Some(w) = app.get_webview_window("main") {
                            match w.is_visible() {
                                Ok(true) => {
                                    let _ = w.hide();
                                    println!("[probe] 主窗口 -> 隐藏");
                                }
                                _ => {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                    println!("[probe] 主窗口 -> 显示");
                                }
                            }
                        }
                    }
                    "settings" => open_settings(app),
                    "quit" => {
                        println!("[probe] 退出");
                        app.exit(0);
                    }
                    other => println!("[probe] 未处理菜单项: {other}"),
                })
                .build(app)?;
            println!("[probe] ✅ 托盘图标 + 右键菜单创建成功（勾选项/显示隐藏/系统设置/退出）");

            // ---------- 主窗口：外部 URL + document_start 注入 ----------
            let url = TARGET_URL.parse().expect("URL 解析失败");
            let script_for_log = format!("[probe] 注入载荷: mods CSS {} 字节", MODS_CSS.len());
            println!("{script_for_log}");

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("fnOS 方案C 探针")
                .inner_size(1200.0, 820.0)
                .initialization_script(css_bootstrap())
                .initialization_script(INIT_JS)
                .on_page_load(|w, payload| {
                    println!("[probe] page-load {:?} {}", payload.event(), payload.url());
                    if payload.event() == PageLoadEvent::Finished {
                        let js = "try{ if(window.__FNOS_PROBE_MARK_EVAL__) window.__FNOS_PROBE_MARK_EVAL__(); \
                                  document.title='[eval-ok] '+document.title; }catch(e){}";
                        match w.eval(js) {
                            Ok(()) => println!("[probe] ✅ 主进程 eval 注入远程页面：已下发"),
                            Err(e) => println!("[probe] ❌ eval 失败: {e}"),
                        }
                        let w2 = w.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(800));
                            match w2.title() {
                                Ok(t) => println!("[probe] 远程页面标题（eval 之后）= {t}"),
                                Err(e) => println!("[probe] 读取标题失败: {e}"),
                            }
                        });
                    }
                })
                .build()?;
            println!("[probe] ✅ 主窗口已创建，加载 {TARGET_URL}");

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri 应用启动失败");
}
