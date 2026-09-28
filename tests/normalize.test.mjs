// 设置窗归一化（spec §6.5 / §8.4）：显示的值必须是**归一化之后**的值，
// 与 Rust 侧 `Config::normalize` 同语义，避免「设置窗显示 A、页面按 B 生效」。
import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeMods, clampLightness, DEFAULT_BRAND_COLOR } from '../ui/settings/normalize.js';

test('明度夹取与非法值回落', () => {
  assert.equal(clampLightness('#ffffff'), '#b3b3b3');
  assert.equal(clampLightness('#000000'), '#4d4d4d');
  assert.equal(clampLightness('#0066ff'), '#0066ff');
  assert.equal(clampLightness('bogus'), '#0066ff');
});

test('枚举与数字回落', () => {
  const out = normalizeMods({
    titlebarStyle: 'nope', launchpadStyle: 'wat', desktopIconLayoutMode: 'x',
    desktopIconPerColumn: 999, fontWeight: 'bold', lockscreenDefaultUsername: 'a'.repeat(100)
  });
  assert.equal(out.titlebarStyle, 'windows');
  assert.equal(out.launchpadStyle, 'classic');
  assert.equal(out.desktopIconLayoutMode, 'adaptive');
  assert.equal(out.desktopIconPerColumn, 16);
  assert.equal(out.fontWeight, '');
  assert.equal(out.lockscreenDefaultUsername.length, 80);
});

test('redrawMap 只保留合法 prefect_icon 路径', () => {
  const out = normalizeMods({
    launchpadIconRedrawMap: { a: 'prefect_icon/emby.png', b: '../etc/passwd', c: 'prefect_icon/BAD.png' }
  });
  assert.deepEqual(Object.keys(out.launchpadIconRedrawMap), ['a']);
});

test('unknown 键被丢弃，已知键保留', () => {
  // brief 原文这里写 `brandColor: '#123456'` 并断言原样保留 —— 但那与 brief 自己的
  // `clampLightness` 矛盾：`#123456` 的明度 20.4% 会被夹到 30% → `#1a4d7f`。按其
  // 「已知键保留」的意图改用夹取区间内的颜色，夹取本身由下一条测试单独锁定。
  const out = normalizeMods({ brandColor: '#3399cc', evil: 1 });
  assert.equal(out.brandColor, '#3399cc');
  assert.equal('evil' in out, false);
});

test('明度夹取在 normalizeMods 里生效（§6.5：显示的值=生效的值）', () => {
  assert.equal(normalizeMods({ brandColor: '#123456' }).brandColor, '#1a4d7f');
  assert.equal(normalizeMods({ brandColor: '#ffffff' }).brandColor, '#b3b3b3');
});

// ——— 本轮在 brief 之外补的锁定（都是「显示一致性」直接依赖的性质） ———

test('白名单与 Rust normalize 对齐：trim + 小写 + 去重 + 丢空', () => {
  const out = normalizeMods({
    enabledOrigins: [' HTTP://NAS.LOCAL:8000 ', 'http://nas.local:8000', 'https://a.b', '', '   ']
  });
  assert.deepEqual(out.enabledOrigins, ['http://nas.local:8000', 'https://a.b']);
});

test('幂等：归一化两次与一次结果相同（避免每次 set_config 后值漂移）', () => {
  const once = normalizeMods({
    brandColor: '#ffffff', desktopIconPerColumn: '99', enabledOrigins: [' HTTP://A '],
    titlebarStyle: 'mac', lockscreenDefaultUsername: 'x'.repeat(90), fontWeight: '450'
  });
  assert.deepEqual(normalizeMods(once), once);
});

test('缺失/非对象输入：全部已知键回落默认值', () => {
  const out = normalizeMods(undefined);
  assert.equal(out.brandColor, DEFAULT_BRAND_COLOR);
  assert.equal(out.desktopIconPerColumn, 8);
  assert.equal(out.launchpadStyle, 'classic');
  assert.deepEqual(out.enabledOrigins, []);
  assert.deepEqual(out.launchpadIconRedrawMap, {});
  assert.deepEqual(Object.keys(out).sort(), [
    'autoEnableSuspectedFnOS', 'basePresetEnabled', 'brandColor', 'customCodeEnabled',
    'desktopIconLayoutEnabled', 'desktopIconLayoutMode', 'desktopIconPerColumn',
    'desktopIconPerColumnEnabled', 'enabledOrigins', 'fontFaceName', 'fontFamily',
    'fontFeatureSettings', 'fontMonospaceFamily', 'fontOverrideEnabled', 'fontUrl',
    'fontWeight', 'launchpadIconMaskOnlyKeys', 'launchpadIconRedrawKeys',
    'launchpadIconRedrawMap', 'launchpadIconScaleEnabled', 'launchpadIconScaleSelectedKeys',
    'launchpadStyle', 'lockscreenDefaultUsername', 'titlebarStyle', 'windowAnimationBlurEnabled'
  ]);
});
