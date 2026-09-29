// 设置窗归一化的**残余锁定**（T14b fix round 1 之后）。
//
// 设置窗不再做任何 mods 归一化（T14b 托管上游 popup；镜像函数已按评审意见删除，
// 见 `ui/settings/normalize.js` 的头部说明）。本文件因此只剩两件事：
//   1. `PREFECT_ICON_PATH` 常量与 Rust 规则的**跨语言镜像**：两侧共用的 20 行输入表；
//   2. `normalizeOrigin` 的 ASCII 小写语义（R23；`status.js` 的白名单路径在用）。
// 「显示的值 = 生效的值」的其余不变式由 Rust 单测（config.rs / commands.rs）与
// tests/settings.test.mjs 的 A 组（原样采纳 + 源码级断言）锁定。
import test from 'node:test';
import assert from 'node:assert/strict';
import { PREFECT_ICON_PATH, normalizeOrigin } from '../ui/settings/normalize.js';

// ——— R30 的 JS 镜像（fix round 1 / Important 2） ———

/**
 * **两侧共用的输入表**（`config.rs::tests::redraw_map_regex_is_case_insensitive_only`
 * 1658-1698 行的 20 条断言逐行、逐序相同）。列：`[值, 期望, 判据]`。
 *
 * 为什么要有这张表：这条规则写在**两个地方**——`ui/settings/normalize.js` 的
 * `PREFECT_ICON_PATH`（R30 的镜像常量）与 Rust 归一化时的判定
 * （`config.rs::is_valid_prefect_icon_path` ← `Config::normalize` 的 `retain`）。任何一个
 * 落后于另一个都会造成**静默丢配置**（§8.4 的原缺陷类别；fix round 1 之前正是 JS 侧落后，
 * `prefect_icon/Emby.png` 在设置窗里被丢掉、Rust 的放宽规则永远见不到它）。
 *
 * `commands.rs::tests::prefect_icon_rule_mirror_stays_in_step`（Rust 侧）会读本文件的源码并
 * 逐行断言这 20 行**存在且期望值一致**；本文件的用例则断言常量的真实行为。
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
    assert.equal(PREFECT_ICON_PATH.test(v), want, `${JSON.stringify(v)} 期望 ${want}（${why}）`);
  }
  // 回归点（fix round 1）：旧的全小写正则对这些值返回 false，设置窗会把它们静默丢掉。
  for (const v of ['prefect_icon/Emby.png', 'prefect_icon/emby.PNG', 'PREFECT_ICON/emby.PnG', 'Prefect_Icon/Home-Assistant.PNG']) {
    assert.equal(PREFECT_ICON_PATH.test(v), true, `R30：${v} 必须被接受`);
    assert.equal(/^prefect_icon\/[a-z0-9-]+\.png$/.test(v), false, `旧全小写正则确实会丢掉 ${v}`);
  }
});

test('PREFECT_ICON_PATH：字符串形状的补充判定（与 Rust 规则同结论）', () => {
  // 尾随换行必须拒绝：`$`（不带 m）只匹配输入末尾，与 Rust `strip_suffix` 的逐字节比较一致
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/emby.png\n'), false);
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/emby.png\r\n'), false);
  // 其他扩展名 / 无扩展名 / 大写扩展名但不是 png
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/emby.jpeg'), false);
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/emby'), false);
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/emby.JPEG'), false);
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/.PNG'), false);
  // 前缀大小写不敏感，但分隔符必须还是 `/`；前缀之后不得再有路径段
  assert.equal(PREFECT_ICON_PATH.test('prefect_ICON/emby.png'), true);
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon//emby.png'), false);
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/../emby.png'), false);
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/emby.png/x.png'), false);
  assert.equal(PREFECT_ICON_PATH.test(''), false);
  // 长前缀的非 ASCII 输入不得抛异常（Rust 侧对应 `str::get` 的 None）
  assert.equal(PREFECT_ICON_PATH.test('prefect_icon/😀.png'), false);
  // 非字符串的兜底**不再由正则提供**：判定函数（带 `typeof` 守卫）已随镜像一起删除，
  // 常量的消费语义是「调用方保证传入字符串」（Rust 侧由 serde 保证 String）。正则对
  // 非字符串会静默强转或抛错（Symbol），那不是这条镜像要锁的行为。
});

// ——— normalizeOrigin 的 ASCII 小写语义（R23；status.js 的白名单路径在用） ———

test('白名单小写只折 ASCII（= Rust to_ascii_lowercase，不是 Unicode toLowerCase）', () => {
  // Rust `config.rs:584` 用 `to_ascii_lowercase()`：非 ASCII 大写字母保持原样。
  // 若这里用 `String#toLowerCase()`，同一份配置在设置窗里显示的条目会与页面比对的
  // 条目差一个字符（§8.4 的缺陷类别），文档表格声称的「一一对应」也就不成立。
  assert.equal(normalizeOrigin(' HTTP://ПРИМЕР.РФ:8000 '), 'http://ПРИМЕР.РФ:8000');
  assert.equal(normalizeOrigin('HTTP://NAS.ПРИМЕР.local'), 'http://nas.ПРИМЕР.local');
  // 对照：Unicode 折叠会把西里尔大写也压下去（旧实现的行为）
  assert.notEqual('ПРИМЕР'.toLowerCase(), 'ПРИМЕР');
  // trim + 去空格（与 Rust normalize 的白名单整理同义）
  assert.equal(normalizeOrigin('  http://a.b  '), 'http://a.b');
  assert.equal(normalizeOrigin(''), '');
  assert.equal(normalizeOrigin(null), '');
});
