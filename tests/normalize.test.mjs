// 设置窗归一化（spec §6.5 / §8.4）：设置窗显示的值必须与页面**实际生效**的值一致。
//
// 本轮（fix round 1）修正了这条不变式的实现位置：生效值由 Rust 的 `Config::normalize`
// 决定，IPC 回包就是权威值，界面**原样采纳**；本文件的 `normalizeMods` 只用于「用户刚
// 输入的值 → 提交 patch」这一段。因此这里锁的是两件事：
//   1. `normalizeMods` 的语义与 Rust 逐步对应（下面每一条都写明 Rust 对应物）；
//   2. 二次夹取的危险性（`app.js::adoptConfig` 的回归测试在 tests/settings.test.mjs）。
import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeMods, clampLightness, normalizeOrigin, DEFAULT_BRAND_COLOR } from '../ui/settings/normalize.js';

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

test('desktopIconPerColumn 的两个边界：0 夹到下限 4，非数字回落 8', () => {
  // Rust 侧是 `clamp(4, 16)` + `sanitize_u32_field`（非 u32 → 删除该键 → serde 默认 8）。
  assert.equal(normalizeMods({ desktopIconPerColumn: 0 }).desktopIconPerColumn, 4);
  assert.equal(normalizeMods({ desktopIconPerColumn: -5 }).desktopIconPerColumn, 4);
  assert.equal(normalizeMods({ desktopIconPerColumn: 'abc' }).desktopIconPerColumn, 8);
  assert.equal(normalizeMods({ desktopIconPerColumn: null }).desktopIconPerColumn, 8);
  assert.equal(normalizeMods({ desktopIconPerColumn: undefined }).desktopIconPerColumn, 8);
  assert.equal(normalizeMods({ desktopIconPerColumn: NaN }).desktopIconPerColumn, 8);
  // 非数字类型与空串：Rust `as_f64` 全部取不到值 → 8；JS 的 `Number('')`/`Number(true)`
  // 会得到 0/1，必须显式排除，否则设置窗显示 4 而 Rust 里是 8。
  assert.equal(normalizeMods({ desktopIconPerColumn: '' }).desktopIconPerColumn, 8);
  assert.equal(normalizeMods({ desktopIconPerColumn: '   ' }).desktopIconPerColumn, 8);
  assert.equal(normalizeMods({ desktopIconPerColumn: true }).desktopIconPerColumn, 8);
  assert.equal(normalizeMods({ desktopIconPerColumn: [] }).desktopIconPerColumn, 8);
  assert.equal(normalizeMods({ desktopIconPerColumn: '0x10' }).desktopIconPerColumn, 8);
  assert.equal(normalizeMods({ desktopIconPerColumn: Infinity }).desktopIconPerColumn, 8);
  // 数字字符串按十进制解析（Rust `as_f64` 同样接受）
  assert.equal(normalizeMods({ desktopIconPerColumn: '9' }).desktopIconPerColumn, 9);
  assert.equal(normalizeMods({ desktopIconPerColumn: '12.5' }).desktopIconPerColumn, 13);
  // 小数四舍五入后夹取（Rust `as_u32` 先 round 再 clamp）
  assert.equal(normalizeMods({ desktopIconPerColumn: 4.6 }).desktopIconPerColumn, 5);
});

test('用户名截断按码点：星平面字符不被切断（= Rust chars().take(80)）', () => {
  const emoji = '😀'; // U+1F600：UTF-16 里是代理对（length 2），码点 1 个
  assert.equal(emoji.length, 2);
  const out = normalizeMods({ lockscreenDefaultUsername: emoji.repeat(100) });
  assert.equal(Array.from(out.lockscreenDefaultUsername).length, 80);
  assert.equal(out.lockscreenDefaultUsername, emoji.repeat(80));
  // 混排：前 79 个星平面字符 + 1 个 BMP 字符后截断，尾部不留孤立代理项
  const mixed = emoji.repeat(79) + 'A' + emoji.repeat(20);
  const cut = normalizeMods({ lockscreenDefaultUsername: mixed }).lockscreenDefaultUsername;
  assert.equal(cut, emoji.repeat(79) + 'A');
  assert.equal(cut.length, 79 * 2 + 1); // 码元数 = 码点数，未切断代理对
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

test('白名单小写只折 ASCII（= Rust to_ascii_lowercase，不是 Unicode toLowerCase）', () => {
  // Rust `config.rs:584` 用 `to_ascii_lowercase()`：非 ASCII 大写字母保持原样。
  // 若这里用 `String#toLowerCase()`，同一份配置在设置窗里显示的条目会与页面比对的
  // 条目差一个字符（§8.4 的缺陷类别），文档表格声称的「一一对应」也就不成立。
  assert.equal(normalizeOrigin(' HTTP://ПРИМЕР.РФ:8000 '), 'http://ПРИМЕР.РФ:8000');
  assert.equal(normalizeOrigin('HTTP://NAS.ПРИМЕР.local'), 'http://nas.ПРИМЕР.local');
  // 对照：Unicode 折叠会把西里尔大写也压下去（旧实现的行为）
  assert.notEqual('ПРИМЕР'.toLowerCase(), 'ПРИМЕР');
  assert.deepEqual(
    normalizeMods({ enabledOrigins: ['HTTP://ПРИМЕР.РФ:8000', 'http://ПРИМЕР.РФ:8000'] }).enabledOrigins,
    ['http://ПРИМЕР.РФ:8000']
  );
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
