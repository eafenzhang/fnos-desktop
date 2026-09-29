/* fnOS Desktop Shell — 应用窗口的自绘标题栏（T14c 修复轮 13）
 *
 * 为什么自绘：`window.open` 开出来的应用窗口原先带的是 **Windows 原生标题栏**，与桌面里
 * 那些窗口（上游 mod 的 mac / windows 风格）**长得不一样**。用户要求统一样式。
 *
 * 为什么画在应用自己的页面上（而不是套一层 iframe 外框）：实测 Hermes Studio 这类应用会
 * **frame-bust**（把自己提升成顶层文档）——套 iframe 会被它顶掉。画在应用页面里没有
 * 这层限制：窗口里的顶层文档始终是应用本身，我们只在它上面加一条固定高度的标题栏。
 *
 * 规格由宿主在建窗时通过 initialization_script 写进 `window.__FNOS_APP_CHROME__`
 * （`{ style, label, title }`，见 `commands.rs::open_app_window`）：
 *   style = 'mac'     → 左侧交通灯 + 居中标题（与桌面里的窗口同一套观感）
 *   style = 'windows' → 左对齐标题 + 右侧 最小化/最大化/关闭
 *
 * 窗口按钮走 Tauri IPC（`plugin:window|*`），权限见 `capabilities/app-windows.json`
 * ——那份能力只授本壳这个窗label（`app-*`）的窗口操作，不含任何应用命令。
 */
(function () {
  'use strict';

  var SPEC = window.__FNOS_APP_CHROME__;
  if (!SPEC || typeof SPEC !== 'object') return; // 没有规格就不画（老宿主/非应用窗口）

  var STYLE = SPEC.style === 'windows' ? 'windows' : 'mac';
  var LABEL = String(SPEC.label || '');
  var TITLE = String(SPEC.title || document.title || '');
  var BAR_ID = 'fnos-app-chrome';
  var BAR_H = 36;

  function paint() {
    if (document.getElementById(BAR_ID)) return;
    if (!document.body) return;

    // 让出标题栏的高度：`html` 加 padding 且用 border-box（`100%` 布局不会溢出；
    // 少数用 `100vh` 的页面会比视口高 36px，滚动条由应用自己处理——比盖住它的顶部好）
    document.documentElement.style.setProperty('padding-top', BAR_H + 'px', 'important');
    document.documentElement.style.setProperty('box-sizing', 'border-box', 'important');

    var bar = document.createElement('div');
    bar.id = BAR_ID;
    bar.setAttribute('data-tauri-drag-region', ''); // 拖拽移动（Tauri 内建拖拽区）
    bar.setAttribute('style', [
      'position:fixed', 'left:0', 'right:0', 'top:0', 'height:' + BAR_H + 'px',
      'display:flex', 'align-items:center', 'gap:10px', 'padding:0 12px',
      'box-sizing:border-box', 'z-index:2147483000',
      'background:#1f2023', 'color:#d8dadd', 'border-bottom:1px solid #2e3034',
      'font:12px/1.4 system-ui,"Microsoft YaHei",sans-serif',
      'user-select:none', '-webkit-user-select:none',
    ].join(';'));

    var title = document.createElement('div');
    title.textContent = TITLE;
    title.setAttribute('data-tauri-drag-region', '');
    title.setAttribute('style',
      'position:absolute;left:0;right:0;text-align:center;pointer-events:none;' +
      'white-space:nowrap;overflow:hidden;text-overflow:ellipsis;padding:0 120px;');

    function windowCommand(name) {
      var api = window.__TAURI_INTERNALS__;
      if (!api || typeof api.invoke !== 'function') return Promise.reject(new Error('IPC 不可用'));
      return api.invoke('plugin:window|' + name, { label: LABEL });
    }

    function makeButton(text, color, command, extraStyle) {
      var b = document.createElement('button');
      b.type = 'button';
      b.textContent = text;
      b.title = command;
      b.setAttribute('style', [
        'border:0', 'cursor:pointer', 'padding:0', 'margin:0',
        'background:' + (color || 'transparent'), 'color:#d8dadd',
        'font:12px/1 system-ui,monospace',
      ].join(';') + ';' + (extraStyle || ''));
      b.addEventListener('mousedown', function (e) { e.stopPropagation(); }); // 不触发拖拽
      b.addEventListener('click', function (e) {
        e.stopPropagation();
        windowCommand(command).catch(function (error) {
          console.warn('[fnos] 窗口命令失败：' + command + '：' + String((error && error.message) || error));
        });
      });
      return b;
    }

    if (STYLE === 'mac') {
      // 交通灯：与桌面窗口一致（红/黄/绿，直径 12px）
      var dots = document.createElement('div');
      dots.setAttribute('style', 'display:flex;gap:8px;position:relative;z-index:1;');
      var colors = { close: '#ff5f57', minimize: '#febc2e', toggle_maximize: '#28c840' };
      var order = ['close', 'minimize', 'toggle_maximize'];
      for (var i = 0; i < order.length; i++) {
        var cmd = order[i];
        dots.appendChild(makeButton('', colors[cmd], cmd,
          'width:12px;height:12px;border-radius:50%;'));
      }
      bar.appendChild(dots);
      bar.appendChild(title);
    } else {
      title.setAttribute('style',
        'flex:1 1 auto;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;' +
        'position:relative;z-index:1;');
      bar.appendChild(title);
      var buttons = document.createElement('div');
      buttons.setAttribute('style', 'display:flex;margin-left:auto;position:relative;z-index:1;');
      buttons.appendChild(makeButton('\u2014', '', 'minimize', 'width:42px;height:' + BAR_H + 'px;'));
      buttons.appendChild(makeButton('\u25A2', '', 'toggle_maximize', 'width:42px;height:' + BAR_H + 'px;'));
      buttons.appendChild(makeButton('\u2715', '', 'close', 'width:42px;height:' + BAR_H + 'px;'));
      bar.appendChild(buttons);
    }

    // 双击标题栏 = 最大化/还原（与系统标题栏的习惯一致）
    bar.addEventListener('dblclick', function () {
      windowCommand('toggle_maximize').catch(function () { /* 失败只记控制台 */ });
    });

    document.body.appendChild(bar);
  }

  if (document.body) paint();
  else document.addEventListener('DOMContentLoaded', paint);
})();
