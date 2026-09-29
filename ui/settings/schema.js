// 设置窗的**声明式**设置项（spec §8.2）。
//
// 只描述「有什么项、绑定到哪个配置键、用什么控件」，不含任何 DOM/渲染逻辑——
// 渲染在 `app.js`，值归一化在 `normalize.js`。这样加一项只改本文件。
//
// 键名规则：`mods.<上游键>` / `shell.<本壳键>` / `local.<上游 local 键>`。
// 无前缀的键按 `mods.` 处理（`app.js::resolvePath`），与上游 popup 的写法兼容。
//
// 与 spec §8.2 的差异（有意，逐条在报告里说明）：
// - 站点组补 `shell.injectEnabled`：§7 里它曾是托盘勾选项，但那个勾选项随托盘精简被删除
//   （T13b fix round 1 订正注释：`tray::sync_menus` 与 `commands::set_inject_enabled` 都没了），
//   现在设置窗的这一项就是唯一的开关入口（也是「needsReload 路径」唯一的手动入口）。
// - 主题色按 §8.2 给「取色器 + 重置」，并在旁边显示**归一化后**的值（§8.4）。
// - Task 13b 补上完美图标的**逐项** UI（`appList`）与登录壁纸（`imageFile`）：这两项的
//   「逐项 / 导入」控件在 T13b 才落地（`appList` 的数据来自主窗口页面的上报，见 app.js）。
export const SCHEMA = [
  {
    id: 'site', title: '站点', items: [
      { key: 'shell.injectEnabled', label: '注入 mods（总开关）', type: 'bool', hint: '开关注入的总闸（唯一的入口在设置窗）；改动会重建主窗口以换用新载荷' },
      { key: 'mods.enabledOrigins', label: '注入白名单', type: 'originList', hint: '命中白名单的站点免 1.5s 探测直接注入；填 NAS 地址保存时会自动并入其 origin' },
      { key: 'shell.nasUrl', label: 'NAS WebUI 地址', type: 'text', hint: '填你自己的 NAS 地址（如 http://192.168.1.10:8000）；保存后自动进入注入白名单' },
      { key: 'mods.autoEnableSuspectedFnOS', label: '自动对疑似飞牛站点启用', type: 'bool' }
    ]
  },
  {
    id: 'basic', title: '基础', items: [
      { key: 'mods.basePresetEnabled', label: '基础美化预设', type: 'bool', hint: '关闭只摘掉标题栏与启动台，基础样式仍生效' },
      { key: 'mods.windowAnimationBlurEnabled', label: '窗口动画模糊', type: 'bool' }
    ]
  },
  {
    id: 'theme', title: '主题', items: [
      { key: 'mods.brandColor', label: '主题色', type: 'color', hint: '明度会被夹到 30%–70%（与页面实际生效的值一致，右侧为归一化结果）' }
    ]
  },
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
  {
    id: 'perfectIcon', title: '完美图标', items: [
      {
        key: 'mods.launchpadIconScaleEnabled',
        label: '完美图标',
        type: 'bool',
        hint: '总开关。上游在它关闭时不处理任何逐项键（缩放/仅遮罩/重绘都不会生效）。'
          + '打开后会把 14 个内置图标嵌进页面（主窗口会重建一次，载荷约 +1.0 MiB）'
      },
      { key: '__appItems', label: '应用逐项设置', type: 'appList' }
    ]
  },
  {
    id: 'wallpaper', title: '登录壁纸', items: [
      {
        key: 'local.loginWallpaperFileName',
        label: '登录页背景图',
        type: 'imageFile',
        hint: 'png / jpg / jpeg / webp，≤ 8 MiB。选中后由宿主写进配置目录（名字带内容指纹），'
          + '再写入本项；主窗口会重建一次以把图片嵌进页面'
      }
    ]
  },
  { id: 'about', title: '关于', items: [] }
];

/**
 * 内置「完美图标」资源（`prefect_icon/<name>.png`）——逐项 UI 的「重绘」下拉用它列选项。
 *
 * **必须与 vendored 目录逐条一致**（14 个文件，磁盘上是 camelCase 的 `panIndex.png`，
 * 这里按上游 `icon-map.json` 的规范写法用小写 `panindex`）。为了不让这份清单漂移，
 * `tests/settings.test.mjs` 用 `fs.readdirSync` 读真实目录做双向比对——加/删一个 PNG 而忘了
 * 改这里，`node --test` 会红。宿主侧对应的那份表在 `src-tauri/src/injector.rs`
 * （`icon_table_matches_the_vendored_directory` 用同样的办法锁着）。
 */
export const PREFECT_ICONS = [
  'alist', 'emby', 'home-assistant', 'icloud', 'it-tools', 'kodi', 'one-panel',
  'oray-hsk', 'panindex', 'qbittorrent', 'quarkpan', 'syncthing', 'transmission', 'xunlei'
];

/** `prefect_icon/<name>.png`（逐项重绘写进 `launchpadIconRedrawMap` 的值）。 */
export function prefectIconPath(name) {
  return `prefect_icon/${name}.png`;
}

/** 上游项目主页（spec §10：非官方 + 非商业声明里必须给出处）。 */
export const UPSTREAM_REPO = 'https://github.com/aurysian-yan/fnOS_UI_Mods';
/** vendored 资源所在的源码/发布路径（spec §10：保留版权声明与许可全文的位置）。 */
export const VENDOR_DIR = 'src-tauri/assets/fnos-mods';
