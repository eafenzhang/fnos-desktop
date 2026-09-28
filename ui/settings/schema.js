// 设置窗的**声明式**设置项（spec §8.2）。
//
// 只描述「有什么项、绑定到哪个配置键、用什么控件」，不含任何 DOM/渲染逻辑——
// 渲染在 `app.js`，值归一化在 `normalize.js`。这样加一项只改本文件。
//
// 键名规则：`mods.<上游键>` / `shell.<本壳键>` / `local.<上游 local 键>`。
// 无前缀的键按 `mods.` 处理（`app.js::resolvePath`），与上游 popup 的写法兼容。
//
// 与 spec §8.2 的差异（有意，逐条在报告里说明）：
// - 站点组补 `shell.injectEnabled`：§7 里它是托盘勾选项，但托盘勾选与设置窗必须写同一份配置；
//   放在设置窗才能在不点托盘的情况下翻转它（也是「needsReload 路径」唯一的手动入口）。
// - 完美图标组的逐项三态（P1）与登录壁纸（P1）本轮不做（§8.2 允许后置到 M2 末）。
// - 主题色按 §8.2 给「取色器 + 重置」，并在旁边显示**归一化后**的值（§8.4）。
export const SCHEMA = [
  {
    id: 'site', title: '站点', items: [
      { key: 'shell.injectEnabled', label: '注入 mods（总开关）', type: 'bool', hint: '与托盘「注入 mods」勾选项同一份配置；改动会重建主窗口以换用新载荷' },
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
  { id: 'about', title: '关于', items: [] }
];

/** 上游项目主页（spec §10：非官方 + 非商业声明里必须给出处）。 */
export const UPSTREAM_REPO = 'https://github.com/aurysian-yan/fnOS_UI_Mods';
/** vendored 资源所在的源码/发布路径（spec §10：保留版权声明与许可全文的位置）。 */
export const VENDOR_DIR = 'src-tauri/assets/fnos-mods';
