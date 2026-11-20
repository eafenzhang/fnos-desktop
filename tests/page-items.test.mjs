// page-items.js（T14g 修复轮 3：完美图标逐项 / 应用项解码）的契约：
//   · 图标清单必须与 vendored 目录**集合相等**（少一个 = 选得到但页面拿不到，多一个 = 指向不存在的文件）；
//   · 路径取值约束与 Rust `config::is_valid_prefect_icon_path` 同一形状；
//   · 应用项只认形状（没有上报 / 别的 type / 形状不对 → null；空数组是 []）；
//   · 逐项选择的互斥关系与上游一致：**重绘优先**（进了红绘集合就从缩放/遮罩里去掉）；
//   · 选择 → `set_config` 的四条键补丁形状稳定。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';

const items = await import('../ui/settings/page-items.js');
const VENDOR_DIR = new URL('../src-tauri/assets/fnos-mods/prefect_icon/', import.meta.url);

test('PREFECT_ICONS 与 vendored 目录集合相等（含大小写归一）', () => {
  const onDisk = readdirSync(VENDOR_DIR)
    .filter((n) => n.toLowerCase().endsWith('.png'))
    .map((n) => n.toLowerCase().replace(/\.png$/, ''))
    .sort();
  assert.deepEqual([...items.PREFECT_ICONS].sort(), onDisk, '图标清单必须与磁盘一一对应');
  assert.equal(items.PREFECT_ICONS.length, 14);
});

test('prefectIconPath / isPrefectIconPath 与 Rust 的取值约束一致', () => {
  assert.equal(items.prefectIconPath('emby'), 'prefect_icon/emby.png');
  assert.equal(items.prefectIconPath(' EMby '), 'prefect_icon/emby.png');
  assert.equal(items.prefectIconPath('nope'), null, '不在清单里的名字不猜');
  assert.equal(items.prefectIconPath('../../x'), null);

  for (const ok of ['prefect_icon/emby.png', 'prefect_icon/one-panel.png', 'PREFECT_ICON/EMBY.PNG']) {
    assert.ok(items.isPrefectIconPath(ok), ok);
  }
  for (const bad of ['', 'prefect_icon/', 'prefect_icon/.png', 'prefect_icon/a b.png', '../prefect_icon/emby.png', 'prefect_icon/emby.jpg', 'prefect_icon/emby.png/x']) {
    assert.ok(!items.isPrefectIconPath(bad), bad);
  }
  // Rust 侧同一个约束（源码锁）：两处必须同时放宽/收紧
  const rust = readFileSync(new URL('../src-tauri/src/config.rs', import.meta.url), 'utf8');
  assert.ok(rust.includes('pub fn is_valid_prefect_icon_path(v: &str) -> bool {'), 'Rust 校验函数必须在');
  assert.ok(rust.includes('strip_prefix_ascii_ci(v, "prefect_icon/")'), 'Rust 前缀必须一致');
  assert.ok(rust.includes("is_ascii_alphanumeric() || c == '-'"), 'Rust 字符集必须一致（字母/数字/连字符）');
});

test('pickReportSlots / appItemsFromReport 只认形状', () => {
  assert.deepEqual(items.pickReportSlots(null), { report: null, appItems: null });
  assert.deepEqual(items.pickReportSlots(42), { report: null, appItems: null });
  const env = { report: { type: 'x' }, appItems: { type: 'y' } };
  assert.deepEqual(items.pickReportSlots(env), env);

  const good = {
    type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS',
    payload: { items: [{ key: '/a/icon_1.png', title: '文件管理', iconSrc: 'https://x/y.png' }] },
  };
  assert.deepEqual(items.appItemsFromReport(good), [
    { key: '/a/icon_1.png', title: '文件管理', iconSrc: 'https://x/y.png' },
  ]);
  // 空数组是「确实 0 个应用」，与 null（没有可用数据）不同
  assert.deepEqual(items.appItemsFromReport({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { items: [] } }), []);
  for (const bad of [
    null,
    {},
    { type: 'FNOS_INJECTION_TRIGGERED', payload: { items: [] } },
    { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: {} },
    { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { items: [{ title: 'no key' }] } },
    { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { items: [{ key: '' }] } },
  ]) {
    assert.equal(items.appItemsFromReport(bad), null, JSON.stringify(bad));
  }
  // titles 那条 type 同样接受（上游用同一个形状应答它）
  assert.equal(
    items.appItemsFromReport({ type: 'FNOS_GET_LAUNCHPAD_APP_TITLES', payload: { items: [{ key: 'k' }] } }).length,
    1
  );
  assert.ok(items.isTooLargeReport({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { tooLarge: true, itemCount: 900 } }));
  assert.ok(!items.isTooLargeReport({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { tooLarge: 'true' } }), '字符串不算');
});

test('逐项选择：重绘优先，且不修改入参', () => {
  const base = { scale: ['k1', 'k2'], mask: ['k3'], redraw: {} };
  const frozen = JSON.stringify(base);

  const scaled = items.applyIconSelection(base, 'k1', { scale: false });
  assert.deepEqual(scaled.scale, ['k2']);

  const redrawn = items.applyIconSelection(base, 'k1', { redraw: 'prefect_icon/emby.png' });
  assert.deepEqual(redrawn.redraw, { k1: 'prefect_icon/emby.png' });
  assert.deepEqual(redrawn.scale, ['k2'], '进了红绘集合就不再缩放');
  assert.ok(!redrawn.mask.includes('k1'));

  const unscaled = items.applyIconSelection({ ...base, scale: ['k2'] }, 'k1', { redraw: '' });
  assert.deepEqual(unscaled.redraw, {}, '空值 = 取消重绘');

  assert.equal(JSON.stringify(base), frozen, '入参不得被修改');
  // 非法路径不会被写进 map
  assert.deepEqual(items.applyIconSelection(base, 'k1', { redraw: '../x.png' }).redraw, {});
});

test('selectionFromConfig / selectionPatch：读写往返且补丁形状稳定', () => {
  const mods = {
    launchpadIconScaleSelectedKeys: ['a', 'b', ''],
    launchpadIconMaskOnlyKeys: ['c'],
    launchpadIconRedrawKeys: ['a'],
    launchpadIconRedrawMap: { a: 'prefect_icon/emby.png', bad: '../x.png' },
  };
  const sel = items.selectionFromConfig(mods);
  assert.deepEqual(sel.scale, ['a', 'b'], '空 key 被丢掉');
  assert.deepEqual(sel.mask, ['c']);
  assert.deepEqual(sel.redraw, { a: 'prefect_icon/emby.png' }, '非法路径被丢掉');

  const patch = items.selectionPatch({ scale: ['a', 'b'], mask: ['a', 'c'], redraw: { a: 'prefect_icon/emby.png' } });
  assert.deepEqual(patch, {
    mods: {
      launchpadIconScaleSelectedKeys: ['b'],
      launchpadIconMaskOnlyKeys: ['c'],
      launchpadIconRedrawKeys: ['a'],
      launchpadIconRedrawMap: { a: 'prefect_icon/emby.png' },
    },
  }, '重绘的 key 必须同时从缩放/遮罩里剔除（互斥关系落盘）');

  assert.equal(items.iconNameFromPath('prefect_icon/one-panel.png'), 'one-panel');
  assert.equal(items.iconNameFromPath('nope'), '');
});
