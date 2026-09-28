// 设置窗归一化（spec §6.5 / §8.4）：设置窗显示的值必须与页面**实际生效**的值一致。
//
// 本轮（fix round 1）修正了这条不变式的实现位置：生效值由 Rust 的 `Config::normalize`
// 决定，IPC 回包就是权威值，界面**原样采纳**；本文件的 `normalizeMods` 只用于「用户刚
// 输入的值 → 提交 patch」这一段。因此这里锁的是两件事：
//   1. `normalizeMods` 的语义与 Rust 逐步对应（下面每一条都写明 Rust 对应物）；
//   2. 二次夹取的危险性（`app.js::adoptConfig` 的回归测试在 tests/settings.test.mjs）。
import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeMods, clampLightness, normalizeOrigin, isPrefectIconPath, DEFAULT_BRAND_COLOR } from '../ui/settings/normalize.js';

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

// ——— R30 的 JS 镜像（fix round 1 / Important 2） ———

/**
 * **两侧共用的输入表**（`config.rs::tests::redraw_map_regex_is_case_insensitive_only`
 * 1658-1698 行的 20 条断言逐行、逐序相同）。列：`[值, 期望, 判据]`。
 *
 * 为什么要有这张表：这条规则在**两个实现**里生效——设置窗提交前（本文件
 * `normalizeModsEntry` → `app.js::commit`）与 Rust 的 `load` / `set_config` 归一化
 * （`config.rs::Config::normalize` 的 `retain`）。任何一个落后于另一个都会造成**静默丢
 * 配置**（§8.4 的原缺陷类别；fix round 1 之前正是 JS 侧落后，`prefect_icon/Emby.png`
 * 在设置窗里被丢掉、Rust 的放宽规则永远见不到它）。
 *
 * `commands.rs::tests::prefect_icon_rule_mirror_stays_in_step`（Rust 侧）会读本文件的源码并
 * 逐行断言这 20 行**存在且期望值一致**；本文件的用例则断言 JS 实现的真实行为。
 * 改一侧必须同时改另一侧，否则那条 Rust 用例会红。
 */
const PREFECT_ICON_PATH_TABLE = [
  // 仍然合法：全小写、数字、连字符
  row('prefect_icon/emby.png', true, '全小写基线'),
  row('prefect_icon/home-assistant.png', true, '连字符仍在字符集里'),
  row('prefect_icon/a1-b2.png', true, '数字 + 连字符'),
  // R30 放宽的三处大小写（每一处都单独钉住，免得将来只放宽其中之一还全绿）
  row('prefect_icon/Emby.png', true, '资源名 camelCase（磁盘上是 panIndex.png 这类名字）'),
  row('prefect_icon/emby.PNG', true, '混合大小写扩展名'),
  row('PREFECT_ICON/emby.PnG', true, '目录段 + 扩展名同时混合大小写'),
  row('Prefect_Icon/Home-Assistant.PNG', true, '三处同时混合大小写'),
  // 只放宽大小写：其余约束一个字都没松
  row('../x', false, '穿越'),
  row('prefect_icon/a.png.png', false, '双扩展名（主体含 `.`）'),
  row('prefect_icon/', false, '空名'),
  row('prefect_icon/sub/dir.png', false, '子目录'),
  row('other/emby.png', false, '前缀不是 prefect_icon/'),
  row('prefect_icon\\emby.png', false, '反斜杠（shim 的查表键用 `/`，永远解析不到资源）'),
  row('prefect_icon/em by.png', false, '空格'),
  row('prefect_icon/.png', false, '只有扩展名'),
  row('prefect_icon/emby.png ', false, '尾随空格'),
  row(' prefect_icon/emby.png', false, '前导空格'),
  row('prefect_icon/图标.png', false, '非 ASCII 资源名'),
  row('图标', false, '非 ASCII、比前缀还短'),
  row('prefect_icon/图的.png', false, '非 ASCII 且前缀长度按字节切在字符中间（不得 panic）')
];

/** 输入表的一行。形如 `row('prefect_icon/Emby.png', true, '…')`，Rust 侧逐字匹配这个形状。 */
function row(v, want, why) {
  return { v, want, why };
}

test('PREFECT_ICON_PATH_TABLE：与 Rust config.rs 的 20 条断言逐行相同（含混合大小写接受）', () => {
  assert.equal(PREFECT_ICON_PATH_TABLE.length, 20, '两侧输入表的行数必须一致');
  for (const { v, want, why } of PREFECT_ICON_PATH_TABLE) {
    assert.equal(isPrefectIconPath(v), want, `${JSON.stringify(v)} 期望 ${want}（${why}）`);
  }
  // 回归点（fix round 1）：旧的全小写正则对这些值返回 false，设置窗会把它们静默丢掉。
  for (const v of ['prefect_icon/Emby.png', 'prefect_icon/emby.PNG', 'PREFECT_ICON/emby.PnG', 'Prefect_Icon/Home-Assistant.PNG']) {
    assert.equal(isPrefectIconPath(v), true, `R30：${v} 必须被接受`);
    assert.equal(/^prefect_icon\/[a-z0-9-]+\.png$/.test(v), false, `旧全小写正则确实会丢掉 ${v}`);
  }
});

test('isPrefectIconPath：JS 侧补充形状（与 Rust 规则同结论）', () => {
  // 尾随换行必须拒绝：`$`（不带 m）只匹配输入末尾，与 Rust `strip_suffix` 的逐字节比较一致
  assert.equal(isPrefectIconPath('prefect_icon/emby.png\n'), false);
  assert.equal(isPrefectIconPath('prefect_icon/emby.png\r\n'), false);
  // 其他扩展名 / 无扩展名 / 大写扩展名但不是 png
  assert.equal(isPrefectIconPath('prefect_icon/emby.jpeg'), false);
  assert.equal(isPrefectIconPath('prefect_icon/emby'), false);
  assert.equal(isPrefectIconPath('prefect_icon/emby.JPEG'), false);
  assert.equal(isPrefectIconPath('prefect_icon/.PNG'), false);
  // 前缀大小写不敏感，但分隔符必须还是 `/`；前缀之后不得再有路径段
  assert.equal(isPrefectIconPath('prefect_ICON/emby.png'), true);
  assert.equal(isPrefectIconPath('prefect_icon//emby.png'), false);
  assert.equal(isPrefectIconPath('prefect_icon/../emby.png'), false);
  assert.equal(isPrefectIconPath('prefect_icon/emby.png/x.png'), false);
  assert.equal(isPrefectIconPath(''), false);
  // 非字符串一律 false（Rust 侧由 serde 保证是 String，这里做 JS 侧兜底）
  for (const bad of [null, undefined, 42, true, {}, [], Symbol('x')]) {
    assert.equal(isPrefectIconPath(bad), false, `非字符串必须拒绝：${String(bad)}`);
  }
  // 长前缀的非 ASCII 输入不得抛异常（Rust 侧对应 `str::get` 的 None）
  assert.equal(isPrefectIconPath('prefect_icon/😀.png'), false);
});

test('redrawMap 归一化：retain 与判定同源（混合大小写被保留、原文不改写、幂等）', () => {
  // 与 Rust 的 retain 用例同一组键值（config.rs:1700-1724）：混合大小写的 4 条必须留下来。
  const out = normalizeMods({
    launchpadIconRedrawMap: {
      ok: 'prefect_icon/emby.png',
      upper: 'prefect_icon/Emby.png',
      ext: 'prefect_icon/emby.PNG',
      dir: 'PREFECT_ICON/emby.png',
      dots: 'prefect_icon/a.png.png',
      esc: '../x',
      sub: 'prefect_icon/sub/dir.png',
      bslash: 'prefect_icon\\emby.png'
    }
  });
  assert.deepEqual(Object.keys(out.launchpadIconRedrawMap).sort(), ['dir', 'ext', 'ok', 'upper']);
  assert.equal(out.launchpadIconRedrawMap.upper, 'prefect_icon/Emby.png', '保留的是用户写的原文，不做改写');
  // 幂等：再归一化一次不改变任何东西（否则每次 set_config 都会漂移）
  assert.deepEqual(normalizeMods(out), out);
  // 旧行为对照（fix round 1 的缺陷本体）：单一混合大小写条目以前会被整条丢掉
  assert.deepEqual(
    Object.keys(normalizeMods({ launchpadIconRedrawMap: { c: 'prefect_icon/panIndex.png' } }).launchpadIconRedrawMap),
    ['c'],
    '磁盘上真实存在的 camelCase 名字（panIndex.png）必须活下来'
  );
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
