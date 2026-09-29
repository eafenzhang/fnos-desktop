// 设置窗的**应用层**行为（Review round 1 的三条修复 + 打磨项）：
//
// A. `adoptConfig` 不得把 IPC 已归一化的值再夹一次（§8.4 的正面落实）
// B. origin 添加只接受 http(s)，拒绝 `"null"` 系（scheme-less / mailto / data / javascript）
// D. 关于页外链走 `open_url` 命令（命令名与参数形状与 Rust 侧一致）
//
// 这些断言依赖「导入 app.js 不需要 DOM」：`boot()` 只在真实页面（有 `#pane`）里自动执行。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import {
  adoptConfig, appItemsFromReport, appListEmptyText, applyIconSelection, compliancePaths,
  iconChoiceOptions, iconSelectionFor, reportSlots, state,
} from '../ui/settings/app.js';
import * as bridge from '../ui/settings/bridge.js';
import {
  clampLightness, normalizeMods, normalizeModsEntry, normalizeOrigin, parseHttpOrigin
} from '../ui/settings/normalize.js';
import { PREFECT_ICONS, VENDOR_DIR, prefectIconPath } from '../ui/settings/schema.js';

test('导入 app.js 不触发 boot（无 DOM 也能单测内部逻辑）', async () => {
  // `boot()` 现在挂在 `window.addEventListener('focus')` 上并会 `render()`（要 #nav/#pane），
  // 若无守卫，仅 import 就会抛错。导入成功 + state 仍是初始值即是证明。
  assert.equal(state.config, null);
  assert.equal(state.active, null);
  assert.equal(state.error, null);
});

// ---------- A：IPC 值不再二次夹取 ----------

test('A: 已归一化的品牌色原样进入界面（#cec1b2 → #c4b4a2 的回归案例）', () => {
  // Rust `normalize_brand_color("#cec1b2")` 的实际输出——config.json 里存的就是它，
  // 页面实际生效的也是它。
  assert.equal(clampLightness('#cec1b2'), '#c4b4a2');
  // 而 `clampLightness` **不是不动点**：再夹一次变成 #c4b4a1。这正是缺陷 A 的机制，
  // 也是「任何已归一化的值都不能再过一遍 clampLightness」的原因。
  assert.equal(clampLightness('#c4b4a2'), '#c4b4a1');
  // 旧 adoptConfig 路径（`normalizeMods(raw.mods)`）复现缺陷：界面会画成 #c4b4a1
  assert.equal(normalizeMods({ brandColor: '#c4b4a2' }).brandColor, '#c4b4a1');
  // 新路径：原样采纳
  adoptConfig({ mods: { brandColor: '#c4b4a2' } });
  assert.equal(state.config.mods.brandColor, '#c4b4a2');
  assert.notEqual(state.config.mods.brandColor, '#c4b4a1');
});

test('A: adoptConfig 不做任何枚举/数字回落（IPC 回包是权威值）', () => {
  adoptConfig({
    mods: {
      brandColor: '#c4b4a2',
      titlebarStyle: 'whatever-rust-returned',
      launchpadStyle: 'whatever-rust-returned',
      desktopIconLayoutMode: 'whatever-rust-returned',
      desktopIconPerColumn: 99,
      fontWeight: 'custom-weight',
      enabledOrigins: ['HTTP://NAS.LOCAL:8000', 'HTTP://NAS.LOCAL:8000']
    }
  });
  assert.deepEqual(state.config.mods, {
    brandColor: '#c4b4a2',
    titlebarStyle: 'whatever-rust-returned',
    launchpadStyle: 'whatever-rust-returned',
    desktopIconLayoutMode: 'whatever-rust-returned',
    desktopIconPerColumn: 99,
    fontWeight: 'custom-weight',
    enabledOrigins: ['HTTP://NAS.LOCAL:8000', 'HTTP://NAS.LOCAL:8000']
  });
});

test('A: 缺 mods / 非对象输入不炸，形状兜底为空对象', () => {
  adoptConfig({ shell: { injectEnabled: true } });
  assert.deepEqual(state.config.mods, {});
  assert.equal(state.config.shell.injectEnabled, true);
  adoptConfig(null); // 不改动已有 state
  assert.deepEqual(state.config.mods, {});
});

test('A: 用户刚输入的值仍在提交前归一化（归一化的唯一入口）', () => {
  // 与 Rust 同一步的产物：这两条决定「用户输入 → patch」与「Rust 归一化」语义一致。
  assert.equal(normalizeModsEntry('brandColor', '#cec1b2'), '#c4b4a2');
  assert.equal(normalizeModsEntry('brandColor', '#ffffff'), '#b3b3b3');
  assert.equal(normalizeModsEntry('titlebarStyle', 'nope'), 'windows');
  assert.equal(normalizeModsEntry('desktopIconPerColumn', 999), 16);
  assert.equal(normalizeModsEntry('enabledOrigins', [' HTTP://A.B ', 'http://a.b'])[0], 'http://a.b');
  assert.equal(normalizeModsEntry('lockscreenDefaultUsername', 'x'.repeat(90)).length, 80);
});

// ---------- B：origin 校验 ----------

test('B: 旧写法为什么拦不住 scheme-less 输入（缺陷机理）', () => {
  // URL 的 scheme 允许含 `.`，于是 `nas.example.com:8000` 被当成 scheme，
  // `8000` 成了 opaque path，`.origin` 返回**字符串 `"null"`**——truthy。
  assert.equal(new URL('nas.example.com:8000').origin, 'null');
  assert.equal(new URL('nas:8000').origin, 'null');
  assert.equal(new URL('mailto:a@b.c').origin, 'null');
  assert.equal(new URL('javascript:alert(1)').origin, 'null');
  assert.equal(new URL('data:text/html,x').origin, 'null');
  assert.ok('null'); // 真值 → 旧的 `if (!origin)` 分支永远不会走到
});

test('B: 只接受 http(s) 绝对地址，其余一律拒绝', () => {
  const accepted = [
    ['http://nas.local:5666', 'http://nas.local:5666'],
    ['https://NAS.Example.com', 'https://nas.example.com'],
    ['HTTP://NAS.LOCAL:8000', 'http://nas.local:8000'],
    ['  https://nas.local  ', 'https://nas.local'],
    ['http://192.168.1.10:8000/ui/index.html', 'http://192.168.1.10:8000'],
    ['http://nas.local:80/', 'http://nas.local'], // 默认端口省略（= Rust origin_of）
    ['https://nas.local:443/ui', 'https://nas.local']
  ];
  for (const [input, expect] of accepted) {
    assert.equal(parseHttpOrigin(input), expect, `应接受并规范化：${input}`);
  }

  const rejected = [
    'nas.example.com:8000', // 无 scheme（旧实现的 junk 来源）
    'nas:8000',
    'nas.example.com',
    'localhost:8000',
    'mailto:a@b.c',
    'data:text/html,<script>alert(1)</script>',
    'javascript:alert(1)',
    'ftp://nas.local',
    'file:///C:/Windows/System32/calc.exe',
    'about:blank',
    '//nas.local:8000',
    'http://',
    'https://',
    '',
    '   '
  ];
  for (const input of rejected) {
    assert.equal(parseHttpOrigin(input), '', `应拒绝：${input}`);
  }
});

test('B: 拒绝时不产生任何白名单条目（"null" 不再可能落盘）', () => {
  // 旧行为的产物就是这个字符串；这里锁死它不可能再作为条目出现。
  const origin = parseHttpOrigin('nas.example.com:8000');
  assert.equal(origin, '');
  assert.notEqual(normalizeOrigin(String(origin)), 'null');
  const list = [origin].filter(Boolean);
  assert.deepEqual(list, []);
});

// ---------- D：关于页外链的命令接线 ----------

test('D: openUrl 调用的命令名/参数形状与 Rust open_url 一致', async () => {
  const calls = [];
  const prev = globalThis.window;
  globalThis.window = { __TAURI_INTERNALS__: { invoke: (cmd, args) => { calls.push([cmd, args]); return Promise.resolve(); } } };
  try {
    await bridge.openUrl('https://github.com/aurysian-yan/fnOS_UI_Mods');
  } finally {
    if (prev === undefined) delete globalThis.window;
    else globalThis.window = prev;
  }
  assert.deepEqual(calls, [['open_url', { url: 'https://github.com/aurysian-yan/fnOS_UI_Mods' }]]);
});

test('D: 桥不可用时返回 rejected Promise（不白屏），且不吞掉命令名', async () => {
  const prev = globalThis.window;
  delete globalThis.window;
  try {
    await assert.rejects(() => bridge.openUrl('https://example.com'), /IPC 桥不可用.*open_url/s);
  } finally {
    if (prev !== undefined) globalThis.window = prev;
  }
});

test('D: importWallpaper 的命令名/参数形状与 Rust import_wallpaper 一致（camelCase 约定）', async () => {
  // tauri 的命令参数默认按 camelCase 从 JS 取值（`data_base64` ↔ `dataBase64`），
  // 名字写错的症状是运行期 "invalid args `data_base64`"，所以在这里钉死形状。
  const calls = [];
  const prev = globalThis.window;
  globalThis.window = {
    __TAURI_INTERNALS__: { invoke: (cmd, args) => { calls.push([cmd, args]); return Promise.resolve('wall-abc.png'); } },
  };
  try {
    const stored = await bridge.importWallpaper('我的壁纸.png', 'iVBORw0KGgo=');
    assert.equal(stored, 'wall-abc.png');
  } finally {
    if (prev === undefined) delete globalThis.window;
    else globalThis.window = prev;
  }
  assert.deepEqual(calls, [['import_wallpaper', { name: '我的壁纸.png', dataBase64: 'iVBORw0KGgo=' }]]);
});

// ---------- E：关于页的合规件路径（T12 / R54） ----------

test('E: 关于页优先展示宿主解析出的随包路径，而不是源码树路径', () => {
  const installed = {
    licensePath: 'C:\\Users\\u\\AppData\\Local\\fnOS\\fnos-mods\\LICENSE',
    noticePath: 'C:\\Users\\u\\AppData\\Local\\fnOS\\fnos-mods\\NOTICE'
  };
  assert.deepEqual(compliancePaths(installed), {
    license: installed.licensePath,
    notice: installed.noticePath
  });
  // 关键不变式：有宿主路径时**不得**再出现 `src-tauri/assets/...`（安装后磁盘上没有它）
  assert.equal(compliancePaths(installed).license.includes(VENDOR_DIR), false);
});

test('E: 老宿主（meta 缺字段/整个缺失）回落源码树路径，绝不画 undefined', () => {
  for (const meta of [{}, undefined, null, { shellVersion: '0.1.0' }]) {
    const p = compliancePaths(meta);
    assert.equal(p.license, `${VENDOR_DIR}/LICENSE`);
    assert.equal(p.notice, `${VENDOR_DIR}/NOTICE`);
    assert.equal(p.license.includes('undefined'), false);
  }
  // 只有一个字段时另一条也各自回落（两行不共享一个判据）
  const half = compliancePaths({ licensePath: 'X:\\a\\LICENSE' });
  assert.equal(half.license, 'X:\\a\\LICENSE');
  assert.equal(half.notice, `${VENDOR_DIR}/NOTICE`);
});

// ---------- F：完美图标逐项（Task 13b） ----------

/** 一张「上游真的回报过」的应用项上报（`ReportEntry.value` 的形状：{type,dir,payload}）。 */
function reportWith(items) {
  return { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', dir: 'response', payload: { items, titles: [] } };
}

test('F: 内置图标清单必须与 vendored 目录逐条一致（防清单漂移）', () => {
  const dir = new URL('../src-tauri/assets/fnos-mods/prefect_icon/', import.meta.url);
  const onDisk = readdirSync(dir)
    .filter((n) => n.toLowerCase().endsWith('.png'))
    .map((n) => n.slice(0, n.length - 4).toLowerCase())
    .sort();
  const listed = [...PREFECT_ICONS].sort();
  assert.deepEqual(listed, onDisk, 'schema.js 的 PREFECT_ICONS 必须与 vendored 目录集合相等');
  assert.equal(listed.length, 14);
  // 逐项重绘写进配置的值必须过得了 normalize.js 的 `isPrefectIconPath`（R30 的大小写不敏感正则）
  for (const name of PREFECT_ICONS) {
    const path = prefectIconPath(name);
    assert.equal(normalizeModsEntry('launchpadIconRedrawMap', { k: path }).k, path, path);
    assert.equal(path, path.toLowerCase(), `${path} 应当是小写（icon-map.json 的规范写法）`);
  }
  // 磁盘上的 camelCase 文件名（panIndex.png）落在小写清单里
  assert.ok(PREFECT_ICONS.includes('panindex'));
  assert.ok(!PREFECT_ICONS.includes('panIndex'));
});

test('F: appItemsFromReport 只认形状（没有上报 / 别的 type / 形状不对 → null；空数组是 []）', () => {
  assert.equal(appItemsFromReport(null), null);
  assert.equal(appItemsFromReport(undefined), null);
  assert.equal(appItemsFromReport({}), null);
  assert.equal(appItemsFromReport({ type: 'FNOS_INJECTION_TRIGGERED', payload: {} }), null);
  assert.equal(appItemsFromReport(reportWith([])).length, 0, '空数组 != 没有数据');
  const two = [{ key: '/a', title: 'A', iconSrc: '' }, { key: '/b', title: 'B', iconSrc: '' }];
  assert.deepEqual(appItemsFromReport(reportWith(two)), two);
  // 缺 key / key 不是字符串 / 不是对象的元素一律丢掉（页面可控文本不得进 UI 结构）
  const messy = appItemsFromReport(reportWith([
    { key: '/ok' }, { title: 'no key' }, { key: 7 }, null, 'x', { key: '' }, { key: '/ok2' },
  ]));
  assert.deepEqual(messy.map((i) => i.key), ['/ok', '/ok2']);
  // items 不是数组 → null
  assert.equal(appItemsFromReport({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { items: 'x' } }), null);
  assert.equal(appItemsFromReport({ type: 'FNOS_GET_LAUNCHPAD_APP_TITLES', payload: {} }), null);
});

test('F: 逐项处置的读写与上游互斥语义一致（cs:755-794）', () => {
  const key = '/app-center-static/serviceicon/emby/ui/images/icon_1.png';
  const base = { launchpadIconScaleSelectedKeys: [], launchpadIconMaskOnlyKeys: [], launchpadIconRedrawKeys: [], launchpadIconRedrawMap: {} };
  assert.equal(iconSelectionFor(base, key), 'off');

  // 选「重绘：emby」→ redrawKeys 与 redrawMap 同时写（上游要求两者都在）
  const red = applyIconSelection(base, key, `redraw:${prefectIconPath('emby')}`);
  assert.deepEqual(red.launchpadIconRedrawKeys, [key]);
  assert.deepEqual(red.launchpadIconRedrawMap, { [key]: 'prefect_icon/emby.png' });
  assert.deepEqual(red.launchpadIconScaleSelectedKeys, []);
  assert.deepEqual(red.launchpadIconMaskOnlyKeys, []);
  assert.equal(iconSelectionFor(red, key), 'redraw:prefect_icon/emby.png');

  // 改选「缩放」→ 必须从重绘里摘干净（上游会把 redrawSet 里的 key 从另两个列表剔除）
  const scale = applyIconSelection(red, key, 'scale');
  assert.deepEqual(scale.launchpadIconScaleSelectedKeys, [key]);
  assert.deepEqual(scale.launchpadIconRedrawKeys, []);
  assert.deepEqual(scale.launchpadIconRedrawMap, {});
  assert.equal(iconSelectionFor(scale, key), 'scale');

  // 改选「仅遮罩」→ 同上，且与缩放互斥
  const mask = applyIconSelection(scale, key, 'mask');
  assert.deepEqual(mask.launchpadIconMaskOnlyKeys, [key]);
  assert.deepEqual(mask.launchpadIconScaleSelectedKeys, []);
  assert.equal(iconSelectionFor(mask, key), 'mask');

  // 回到「不处理」→ 四个键里都不留这一项
  const off = applyIconSelection(mask, key, 'off');
  assert.deepEqual(off.launchpadIconMaskOnlyKeys, []);
  assert.deepEqual(off.launchpadIconScaleSelectedKeys, []);
  assert.deepEqual(off.launchpadIconRedrawKeys, []);
  assert.deepEqual(off.launchpadIconRedrawMap, {});
  assert.equal(iconSelectionFor(off, key), 'off');

  // 不认识的处置值 / 不在清单里的图标路径都不写任何东西（不给 junk 留缝）
  for (const bad of ['redraw:../x', 'redraw:prefect_icon/not-bundled.png', 'redraw:', 'nope', 7, null]) {
    const out = applyIconSelection(off, key, bad);
    assert.deepEqual(out.launchpadIconRedrawKeys, [], `bad=${String(bad)}`);
    assert.deepEqual(out.launchpadIconRedrawMap, {}, `bad=${String(bad)}`);
  }

  // 其它应用项不受影响（只动被改的那一个 key）
  const other = '/app-center-static/serviceicon/xunlei/ui/images/icon_1.png';
  const both = applyIconSelection(red, other, 'scale');
  assert.deepEqual(both.launchpadIconRedrawKeys, [key], '另一个应用的重绘保留');
  assert.deepEqual(both.launchpadIconScaleSelectedKeys, [other]);

  // 入参不可变（applyIconSelection 不得改写 state.config 里的对象）
  const frozen = { ...base };
  applyIconSelection(frozen, key, 'scale');
  assert.deepEqual(frozen, base, '入参对象不得被改写');
});

test('F: 事件处理器的写回值经 normalizeModsEntry 后仍然是同一份（提交路径不会吞掉配置）', () => {
  const key = '/app-center-static/serviceicon/emby/ui/images/icon_1.png';
  const patch = applyIconSelection({}, key, `redraw:${prefectIconPath('panindex')}`);
  for (const [k, v] of Object.entries(patch)) {
    assert.deepEqual(normalizeModsEntry(k, v), v, k);
  }
});

test('F: 逐项下拉的选项集合（不处理/缩放/仅遮罩 + 14 个内置重绘目标）', () => {
  const opts = iconChoiceOptions();
  assert.deepEqual(opts.slice(0, 3).map((o) => o[0]), ['off', 'scale', 'mask']);
  assert.equal(opts.length, 3 + PREFECT_ICONS.length);
  for (const [value, text] of opts.slice(3)) {
    assert.ok(value.startsWith('redraw:prefect_icon/'), value);
    assert.ok(text.startsWith('重绘：'), text);
    assert.ok(text.length > 3 && !text.includes('/'), '显示文本只用内置名');
  }
});

test('F: 没有可用列表时的文案据实（四种情形四句话；页面可控 type 只回显 FNOS_* 形状）', () => {
  const noReport = appListEmptyText(null, null);
  assert.ok(noReport.includes('尚未收到'), noReport);
  assert.ok(noReport.includes('打开配置目录'), noReport);

  const empty = appListEmptyText(reportWith([]), []);
  assert.ok(empty.includes('0 个应用项'), empty);

  // 只发出过请求（`dir:'out'`，shim 自己发的）→ 「已经问过、还没有可用应答」，不是「形状不可用」
  const asked = appListEmptyText({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', dir: 'out', payload: {} }, null);
  assert.ok(asked.includes('已经向页面请求过'), asked);
  assert.ok(!asked.includes('形状不可用'), asked);

  const otherType = appListEmptyText({ type: 'FNOS_INJECTION_TRIGGERED', dir: 'out', payload: {} }, null);
  assert.ok(otherType.includes('FNOS_INJECTION_TRIGGERED'), otherType);

  const badShape = appListEmptyText({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { items: 7 } }, null);
  assert.ok(badShape.includes('形状不可用'), badShape);

  // 页面可控的 type 不得原样进 UI：奇怪形状一律走「形状不可用 / 未收到」的固定说法
  for (const evil of ['<img src=x onerror=alert(1)>', 'evil\ntype', 'FNOS_x', 'A'.repeat(200), 7]) {
    const text = appListEmptyText({ type: evil, payload: {} }, null);
    assert.equal(typeof text, 'string');
    assert.ok(!text.includes(String(evil)), `不得回显：${String(evil).slice(0, 20)}`);
    assert.equal(text.split('\n').length, 1, '文案必须只有一行');
  }
});

test('F: 应用项列表取不到时各种形状都不会被当成「有数据」', () => {
  // 应用项槽位由 fetchPageSnapshot（reportSlots）写入；这里直接检查纯判据（不涉及 DOM）
  for (const r of [null, undefined, {}, { type: 'FNOS_CHECK', payload: {} }, { payload: { items: [] } }]) {
    const items = appItemsFromReport(r);
    if (r && r.type === 'FNOS_CHECK') assert.equal(items, null);
    if (!r) assert.equal(items, null);
  }
});

// ---------- G：两个上报槽位的接线（T13b 审计发现的回归修复） ----------

test('G: get_page_report 的信封被拆成 report / appItems 两个槽位（缺字段/非对象一律回落 null）', () => {
  const inj = { type: 'FNOS_INJECTION_TRIGGERED', dir: 'out', payload: {} };
  const list = reportWith([{ key: '/a/icon_1.png' }]);
  assert.deepEqual(reportSlots({ report: inj, appItems: list }), { report: inj, appItems: list });
  // 老宿主 / 半升级状态：回包是 null、空对象、或者根本就是一条裸上报（13a 的形状）→ 两个槽位都不猜
  for (const bad of [null, undefined, {}, 'x', 7, [], inj]) {
    assert.deepEqual(
      reportSlots(bad), { report: null, appItems: null },
      `非信封回包不得被猜成任何一个槽位：${JSON.stringify(bad)}`
    );
  }
  assert.deepEqual(reportSlots({ report: null, appItems: null }), { report: null, appItems: null });
  // 单个槽位缺失/不是对象时，只回落那一个
  assert.deepEqual(reportSlots({ report: inj }), { report: inj, appItems: null });
  assert.deepEqual(reportSlots({ report: 'x', appItems: list }), { report: null, appItems: list });
  // state 必须真的带两个槽位（不是把应用项塞回 report）
  assert.ok(Object.prototype.hasOwnProperty.call(state, 'report'));
  assert.ok(Object.prototype.hasOwnProperty.call(state, 'appItemsReport'));
});

test('G: 逐项列表只读 appItems 槽位（绝不读状态条的证据槽位）', () => {
  // 源码级断言：`case 'appList'` 这一段里只允许出现 appItemsReport。两个槽位接错的症状很隐蔽
  // ——列表看起来「还没收到」，而状态条看起来正常——所以用一条机械锁钉住接线本身。
  const src = readFileSync(new URL('../ui/settings/app.js', import.meta.url), 'utf8');
  const start = src.indexOf("case 'appList'");
  const end = src.indexOf("case 'imageFile'");
  assert.ok(start > 0 && end > start, 'app.js 必须同时有 appList 与 imageFile 分支');
  const block = src.slice(start, end);
  assert.ok(block.includes('appItemsFromReport(state.appItemsReport)'), block.slice(0, 200));
  assert.ok(block.includes('appListEmptyText(state.appItemsReport,'), block.slice(0, 200));
  assert.ok(!block.includes('appItemsFromReport(state.report)'), '不得读状态条槽位');
  assert.ok(!block.includes('appListEmptyText(state.report,'), '不得读状态条槽位');
});
