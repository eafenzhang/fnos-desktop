// 设置页（T14g）的契约：
//   · 分页集合与 settings.html 的 data-section / data-panel 一一对应；
//   · 更新阶段的字符串与 Rust `updater::phase` 逐字一致（跨语言锁）；
//   · 进度/文案/按钮的纯函数（实时下载进度的渲染依据）；
//   · 设置页 → Rust 的命令名在 main.rs / build.rs / capabilities 三处都齐；
//   · 纪律：页面可控文本只用 textContent（不 innerHTML）。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (p) => readFileSync(new URL(p, import.meta.url), 'utf8');
const PAGE = read('../ui/settings/settings-page.js');
const HTML = read('../ui/settings/settings.html');
const RUST_UPDATER = read('../src-tauri/src/updater.rs');
const RUST_MAIN = read('../src-tauri/src/main.rs');
const RUST_BUILD = read('../src-tauri/build.rs');
const CAPS = read('../src-tauri/capabilities/default.json');

// 纯函数模块在 Node 里可直接 import（模块加载只在真实页面才 boot）。
const page = await import('../ui/settings/settings-page.js');

test('分页集合与 settings.html 逐项对应（侧边栏 + 面板）', () => {
  assert.deepEqual(page.SECTIONS, ['general', 'appearance', 'desktop', 'update', 'advanced', 'about']);
  for (const id of page.SECTIONS) {
    assert.ok(HTML.includes(`data-section="${id}"`), `侧边栏缺 ${id}`);
    assert.ok(HTML.includes(`data-panel="${id}"`), `面板缺 ${id}`);
  }
  const sections = [...HTML.matchAll(/data-section="([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(sections, page.SECTIONS, '侧边栏顺序必须与 SECTIONS 一致');
});

test('更新阶段字符串与 Rust updater::phase 逐字一致', () => {
  for (const phase of page.UPDATE_PHASES) {
    assert.ok(
      RUST_UPDATER.includes(`pub const ${phase.toUpperCase().replace('UPTODATE', 'UP_TO_DATE')}: &str = "${phase}"`)
      || RUST_UPDATER.includes(`= "${phase}";`),
      `Rust 侧缺阶段 ${phase}`
    );
  }
  // 反向：Rust 的 phase 模块里出现的字符串值必须都在前端清单里
  const rustValues = [...RUST_UPDATER.matchAll(/pub const [A-Z_]+: &str = "([A-Za-z]+)";/g)].map((m) => m[1]);
  for (const v of rustValues) {
    if (!['http', 'https'].includes(v)) {
      assert.ok(page.UPDATE_PHASES.includes(v), `前端不认识 Rust 阶段 ${v}`);
    }
  }
});

test('patchFor 只带一条键；非法路径直接拒绝', () => {
  assert.deepEqual(page.patchFor('shell.closeToTray', true), { shell: { closeToTray: true } });
  assert.deepEqual(page.patchFor('mods.brandColor', '#0066ff'), { mods: { brandColor: '#0066ff' } });
  for (const bad of ['', 'shell', 'shell.', '.x', 'a.b.c']) {
    assert.throws(() => page.patchFor(bad, 1), /段\.键/, `必须拒绝：${bad}`);
  }
});

test('readPath 按段取值，取不到给 undefined', () => {
  const cfg = { shell: { nasUrl: '' }, mods: { brandColor: '#0066ff' } };
  assert.equal(page.readPath(cfg, 'mods.brandColor'), '#0066ff');
  assert.equal(page.readPath(cfg, 'shell.nasUrl'), '');
  assert.equal(page.readPath(cfg, 'shell.missing'), undefined);
  assert.equal(page.readPath(null, 'a.b'), undefined);
});

test('字节/速度/进度格式化', () => {
  assert.equal(page.formatBytes(0), '0 B');
  assert.equal(page.formatBytes(999), '999 B');
  assert.equal(page.formatBytes(1536), '1.5 KB');
  assert.equal(page.formatBytes(3 * 1024 * 1024), '3.0 MB');
  assert.equal(page.formatBytes(NaN), '—');
  assert.equal(page.formatSpeed(0), '', '0 速度不显示噪声');
  assert.equal(page.formatSpeed(2048), '2.0 KB/s');

  assert.equal(page.progressPercent({ downloaded: 50, total: 100 }), 50);
  assert.equal(page.progressPercent({ downloaded: 100, total: 100 }), 100);
  assert.equal(page.progressPercent({ downloaded: 120, total: 100 }), 100, '不显示 120%');
  assert.equal(page.progressPercent({ downloaded: 5, total: null }), null, '总量未知 → 不给假进度');
  assert.equal(page.progressPercent({ downloaded: -1, total: 10 }), null);

  assert.equal(page.progressText({ downloaded: 512, total: 1024 }), '50% · 512 B / 1.0 KB');
  assert.equal(page.progressText({ downloaded: 512, total: null }), '已下载 512 B');
  assert.equal(page.progressSpeed({ bytesPerSec: 4096 }), '4.0 KB/s');
});

test('阶段 → 文案 / 按钮状态', () => {
  assert.match(page.updateHeadline({ phase: 'idle' }), /检查新版本/);
  assert.equal(page.updateHeadline({ phase: 'checking' }), '正在检查更新…');
  assert.equal(page.updateHeadline({ phase: 'downloading', latest: 'v0.4.0' }), '正在下载 v0.4.0…');
  assert.equal(page.updateHeadline({ phase: 'downloading' }), '正在下载更新…');
  assert.equal(page.updateHeadline({ phase: 'installing' }), '正在安装并重启…');
  assert.equal(page.updateHeadline({ phase: 'error', message: '网络不可达' }), '网络不可达');
  assert.equal(page.updateHeadline({ phase: 'upToDate', message: '已是最新版本（v0.4.0）。' }), '已是最新版本（v0.4.0）。');
  assert.equal(page.updateHeadline(null), page.updateHeadline({ phase: 'idle' }));

  assert.deepEqual(page.updateButtons({ phase: 'checking' }), { checkDisabled: true, installVisible: false });
  assert.deepEqual(page.updateButtons({ phase: 'downloading' }), { checkDisabled: true, installVisible: false });
  assert.deepEqual(page.updateButtons({ phase: 'installing' }), { checkDisabled: true, installVisible: false });
  assert.deepEqual(page.updateButtons({ phase: 'ready' }), { checkDisabled: false, installVisible: true });
  assert.deepEqual(page.updateButtons({ phase: 'error' }), { checkDisabled: false, installVisible: false });
});

test('设置页 → Rust：命令在 main.rs / build.rs / capabilities 三处都齐', () => {
  for (const cmd of ['get_update_state', 'start_update_check', 'install_update']) {
    assert.ok(RUST_MAIN.includes(`commands::${cmd}`), `main.rs 未注册 ${cmd}`);
    assert.ok(RUST_BUILD.includes(`"${cmd}"`), `build.rs 的命令清单缺 ${cmd}`);
    assert.ok(CAPS.includes(`"allow-${cmd.replace(/_/g, '-')}"`), `capabilities 未授权 ${cmd}`);
  }
  const BRIDGE = read('../ui/settings/bridge.js');
  for (const fn of ['getUpdateState', 'startUpdateCheck', 'installUpdate']) {
    assert.ok(BRIDGE.includes(`export function ${fn}`), `bridge 缺 ${fn}`);
  }
});

test('上游设置窗口已退役：没有独立页面/命令/授权，设置项全部在本页', () => {
  // 用户决定：取消上游设置页面，设置项移植进本页。这条锁住「不会再长回来」。
  assert.throws(() => read('../ui/settings/upstream.html'), 'upstream.html 必须已删除');
  assert.throws(() => read('../src-tauri/capabilities/upstream.json'), '上游窗口授权必须已删除');
  assert.ok(!RUST_MAIN.includes('open_upstream_settings'), 'main.rs 不得再注册上游窗口命令');
  assert.ok(!RUST_BUILD.includes('open_upstream_settings'), 'build.rs 不得再列该命令');
  assert.ok(!CAPS.includes('allow-open-upstream-settings'), 'capabilities 不得再授权该命令');
  for (const dead of ['app.js', 'chrome-shim.js', 'popup.html', 'popup.js', 'normalize.js']) {
    assert.throws(() => read(`../ui/settings/${dead}`), `${dead} 必须已删除（上游设置 UI 退役）`);
  }
  assert.ok(!HTML.includes('id="upstreamHost"'), '设置页不得再嵌上游宿主');
});

test('上游设置项已移植：每个配置键在本页都有控件', () => {
  // 逐键锁：上游 popup 暴露的配置项（除它自己的更新检查器）都必须在本页可改。
  const keys = [
    'mods.autoEnableSuspectedFnOS',
    'mods.brandColor',
    'mods.titlebarStyle',
    'mods.launchpadStyle',
    'mods.windowAnimationBlurEnabled',
    'mods.basePresetEnabled',
    'mods.fontOverrideEnabled',
    'mods.fontFamily',
    'mods.fontUrl',
    'mods.fontMonospaceFamily',
    'mods.fontWeight',
    'mods.fontFeatureSettings',
    'mods.desktopIconLayoutEnabled',
    'mods.desktopIconLayoutMode',
    'mods.desktopIconPerColumn',
    'mods.launchpadIconScaleEnabled',
    'mods.customCodeEnabled',
    'mods.lockscreenDefaultUsername',
    'local.customCssCode',
    'local.customJsCode',
  ];
  for (const key of keys) {
    assert.ok(HTML.includes(`data-patch="${key}"`), `${key} 没有控件`);
  }
  // 白名单与壁纸走专用控件（不是 data-patch）
  assert.ok(HTML.includes('id="siteToggle"'), '「为当前站点注入」开关必须存在');
  assert.ok(HTML.includes('id="wallpaperPick"') && HTML.includes('id="wallpaperFile"'), '壁纸导入控件必须存在');
  assert.ok(HTML.includes('id="iconItemsRefresh"') && HTML.includes('id="iconItemsList"'), '完美图标逐项控件必须存在');
  // 页面逻辑必须真的接到这些能力上
  for (const needle of ['getPageState', 'requestAppItems', 'getPageReport', 'importWallpaper', 'selectionPatch']) {
    assert.ok(PAGE.includes(needle), `settings-page.js 缺 ${needle}`);
  }
});

test('站点白名单：origin 解析 / 大小写不敏感去重 / 自带放行识别', () => {
  assert.equal(page.originOf('https://NAS.local:5666/webui/'), 'https://nas.local:5666');
  assert.equal(page.originOf('http://192.168.1.10'), 'http://192.168.1.10');
  assert.equal(page.originOf('javascript:alert(1)'), '');
  assert.equal(page.originOf('not a url'), '');

  assert.ok(page.originsInclude(['https://A.fnos.net'], 'https://a.fnos.net'));
  assert.ok(!page.originsInclude([], 'https://a.fnos.net'));
  assert.deepEqual(page.toggleOrigin(['https://x', 'https://Y'], 'https://y', false), ['https://x']);
  assert.deepEqual(page.toggleOrigin(['https://x'], 'https://Y', true), ['https://x', 'https://y']);
  assert.deepEqual(page.toggleOrigin(['https://x'], '', true), ['https://x'], '空 origin 不改');

  // 桌面地址 / 官网入口的 origin 是自带放行的（开关恒真且禁用）
  const forced = page.configForcedOrigins({ shell: { nasUrl: 'https://ea121314.fnos.net/', homeUrl: 'https://fnos.net/' } });
  assert.deepEqual(forced, ['https://ea121314.fnos.net', 'https://fnos.net']);
});

test('纪律：设置页不写 innerHTML / 不引入高开销样式', () => {
  // 注释里可以提「绝不 innerHTML」，但不得真的写它
  assert.ok(!/\.innerHTML\s*=/.test(PAGE), '页面可控文本一律 textContent');
  const CSS = read('../ui/settings/settings.css');
  // 注释里可以说明「不引入」，但不得真的声明它
  assert.ok(!/backdrop-filter\s*:/.test(CSS), '设置页不得引入背景模糊（低配机器上最贵的一笔）');
});
