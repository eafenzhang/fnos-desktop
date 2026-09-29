// 设置窗的**应用层**行为（Review round 1 的三条修复 + 打磨项 + T14b fix round 1）：
//
// A. `adoptConfig` 不得把 IPC 已归一化的值再夹一次（§8.4 的正面落实）；T14b fix round 1
//    起设置窗**没有任何 mods 归一化**——镜像函数已删除，本文件用源码级断言锁住
//    「app.js 不得引用 normalize.js」
// B. origin 添加只接受 http(s)，拒绝 `"null"` 系（scheme-less / mailto / data / javascript）
// D. 关于页外链走 `open_url` 命令（命令名与参数形状与 Rust 侧一致）
//
// 这些断言依赖「导入 app.js 不需要 DOM」：`boot()` 只在真实页面（有 `#upstreamHost`）里自动执行。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import {
  adoptConfig, appItemsAnswer, appItemsFromReport, applyPatchFromPopup, compliancePaths,
  isTooLargeReport, pageCheckAnswer, reportSlots, state,
} from '../ui/settings/app.js';
import * as bridge from '../ui/settings/bridge.js';
import { PREFECT_ICON_PATH, normalizeOrigin, parseHttpOrigin } from '../ui/settings/normalize.js';
import { VENDOR_DIR } from '../ui/settings/app.js';

test('导入 app.js 不触发 boot（无 DOM 也能单测内部逻辑）', async () => {
  // `boot()` 只挂在「有 #upstreamHost 的真实页面」上（Task 14b 起判据是这个 id），
  // 若无守卫，仅 import 就会抛错。导入成功 + state 仍是初始值即是证明。
  assert.equal(state.config, null);
  assert.equal(state.error, null);
});

// ---------- A：IPC 值原样采纳，设置窗没有任何 mods 归一化 ----------

test('A: 已归一化的品牌色原样进入界面（#cec1b2 → #c4b4a2 的回归案例）', () => {
  // Rust `normalize_brand_color("#cec1b2")` 的实际输出——config.json 里存的就是它，
  // 页面实际生效的也是它。JS 侧曾经有的一份镜像夹取（`clampLightness`，**不是不动点**：
  // 再夹一次 `#c4b4a2` → `#c4b4a1`）已随 T14b fix round 1 删除——镜像存在本身就会诱导
  // 「回包再过一遍」的旧缺陷路径，所以这里只剩「原样采纳」一条事实。
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

test('A: 设置窗不再有任何 mods 归一化（app.js 不得引用 normalize.js）', () => {
  // T14b 用上游 popup 取代了 schema 驱动界面，`app.js` 的提交路径只剩 `shell.*` 两键，
  // mods 归一化没有调用方；镜像函数（`clampLightness` / `normalizeMods` /
  // `normalizeModsEntry` / `isPrefectIconPath`）已按评审意见从 normalize.js 删除。
  // 这里用源码级断言防「借尸还魂」：只要 app.js 重新引用 normalize.js，说明有人试图
  // 在设置窗里恢复归一化——那必须连同 Rust 侧的唯一归一化声明一起重新设计，不许静默加回。
  const app = readFileSync(new URL('../ui/settings/app.js', import.meta.url), 'utf8');
  assert.ok(!app.includes("from './normalize.js'"), 'app.js 不得引用 normalize.js');
  for (const dead of ['clampLightness', 'normalizeMods', 'normalizeModsEntry', 'isPrefectIconPath']) {
    assert.ok(!app.includes(dead), `app.js 不得出现镜像函数名 ${dead}`);
  }
  // normalize.js 里被保留下来的只有两件事：跨语言镜像常量与 origin 整理（含解析）。
  // 按导出判（注释里提这些名字是允许的——文档要能说清删了什么），按字符串判会误伤。
  const normalize = readFileSync(new URL('../ui/settings/normalize.js', import.meta.url), 'utf8');
  for (const gone of ['normalizeMods', 'normalizeModsEntry', 'clampLightness', 'isPrefectIconPath']) {
    assert.ok(!normalize.includes(`export function ${gone}`), `normalize.js 不得再导出 ${gone}（已按评审意见删除）`);
    assert.ok(!normalize.includes(`export const ${gone}`), `normalize.js 不得再导出 ${gone}（已按评审意见删除）`);
  }
  assert.ok(!normalize.includes('export const MODS_KEYS'), 'normalize.js 不得再导出 MODS_KEYS（已随镜像删除）');
  assert.deepEqual(
    (normalize.match(/^export (?:const|function) \w+/gm) || []).sort(),
    ['export const PREFECT_ICON_PATH', 'export function normalizeOrigin', 'export function parseHttpOrigin'].sort(),
    'normalize.js 的导出面只剩 PREFECT_ICON_PATH / normalizeOrigin / parseHttpOrigin'
  );
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

test('F: 内置图标清单必须与 vendored 目录逐条一致（Task 14b 起改为比对两份真实目录）', () => {
  // T14b 之前这里守的是 `schema.js::PREFECT_ICONS` 这份手写清单；分组 UI 退休、schema.js
  // 删除之后，清单的**唯一**事实来源是 vendored 目录本身。现在要守的是打包不变式：
  // 上游 popup 能 `chrome.runtime.getURL('prefect_icon/<name>.png')` 命名的每一张图，
  // 都必须在**设置窗资产根**下真的存在（否则上游的逐项重绘下拉会永远显示「未找到重绘图标」）。
  const vendored = new URL('../src-tauri/assets/fnos-mods/prefect_icon/', import.meta.url);
  const shipped = new URL('../ui/settings/prefect_icon/', import.meta.url);
  const names = (dir) => readdirSync(dir).filter((n) => n.toLowerCase().endsWith('.png')).sort();
  const vendoredPngs = names(vendored);
  assert.equal(vendoredPngs.length, 14, `vendored 的完美图标必须有 14 张，实得 ${vendoredPngs.length}`);
  assert.deepEqual(names(shipped), vendoredPngs, 'ui/settings/prefect_icon 必须与 vendored 目录逐文件一致');
  // icon-map.json 也要在（上游 `loadPrefectIconMapConfig` 直接 fetch 它）
  assert.ok(readdirSync(shipped).includes('icon-map.json'), 'ui/settings/prefect_icon 必须有 icon-map.json');
  // 磁盘上的 camelCase 文件名（panIndex.png）与 icon-map.json 里的小写键并存
  assert.ok(vendoredPngs.includes('panIndex.png'));
  assert.ok(!vendoredPngs.includes('panindex.png'));
  // 逐项重绘写进配置的值必须过得了 R30 的大小写不敏感正则（normalize.js 的镜像常量；
  // 合法性的最终收口在 Rust `is_valid_prefect_icon_path`，两侧输入表逐行相同）
  for (const name of vendoredPngs) {
    const path = `prefect_icon/${name}`;
    assert.equal(PREFECT_ICON_PATH.test(path), true, path);
  }
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

test('F: appItemsAnswer 的三种结局（拿不到 → null；空列表 → null；可用 → 应答）', () => {
  // 没有可用数据：没上报 / 形状不对 / 上游确实回报了 0 个 —— 都返回 null，
  // 让调用方去「请页面重汇报一次」而不是凭空造一份列表。
  assert.equal(appItemsAnswer(null), null);
  assert.equal(appItemsAnswer({}), null);
  assert.equal(appItemsAnswer({ type: 'FNOS_INJECTION_TRIGGERED', payload: {} }), null);
  assert.equal(appItemsAnswer(reportWith([])), null, '空列表 = 还没有可用数据');
  assert.equal(appItemsAnswer({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { items: 7 } }), null);

  // 可用列表：items 逐项过形状闸门，titles 只收字符串
  const two = [{ key: '/a', title: 'A' }, { key: '/b', title: 'B' }];
  const answer = appItemsAnswer({
    type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', dir: 'response',
    payload: { items: two, titles: ['A', 'B', 7, null] },
  });
  assert.deepEqual(answer.items, two);
  assert.deepEqual(answer.titles, ['A', 'B'], 'titles 里的非字符串一律丢掉');
  assert.equal(answer.itemCount, 2);
  assert.equal(answer.tooLarge, undefined);
});

test('F: 事件处理器写出的 patch 原样转发（提交路径不会吞掉配置；合法性由 Rust 收口）', () => {
  // 上游 popup 写的是 `launchpadIconRedrawMap` / `…RedrawKeys` / `…ScaleSelectedKeys` /
  // `…MaskOnlyKeys` 这四个键（popup.js:559-581），本壳只在 `FNOS_APPLY` 那条路径上转发它们。
  // T14b fix round 1 起设置窗不再做任何归一化（镜像函数已删除）：patch 原样进 `set_config`，
  // 由 Rust 的 `Config::normalize` 收口——这里锁「转发不丢键不变形」+「写进的图标路径
  // 过得了 R30 正则」（camelCase 的 panIndex.png 正是旧全小写正则会吞掉的那个值）。
  const patch = applyPatchFromPopup({
    launchpadIconScaleSelectedKeys: [], launchpadIconMaskOnlyKeys: [],
    launchpadIconRedrawKeys: ['/a.png'], launchpadIconRedrawMap: { '/a.png': 'prefect_icon/panIndex.png' },
  });
  assert.deepEqual(patch.mods.launchpadIconRedrawMap, { '/a.png': 'prefect_icon/panIndex.png' });
  assert.deepEqual(patch.mods.launchpadIconRedrawKeys, ['/a.png']);
  assert.equal(PREFECT_ICON_PATH.test(patch.mods.launchpadIconRedrawMap['/a.png']), true);
});

test('F: 上游消息里的每个派生键都落进 patch（白名单），其余一概不进', () => {
  const patch = applyPatchFromPopup({
    basePresetEnabled: false,
    titlebarStyle: 'mac',
    desktopIconPerColumn: 12,
    brandColor: '#123456',
    lockscreenDefaultUsername: 'me',
    fontSettings: {
      enabled: true, family: 'A', monospaceFamily: 'B', weight: '600',
      featureSettings: '"liga" 1', faceName: 'X', url: 'https://x/y.woff2',
    },
    customCodeSettings: { css: 'body{}', js: 'void 0' },
    // 这些是「资源要不要重算」的提示，本壳没有照着做的动作 → 不进 patch
    refreshFontAsset: true, refreshCustomCode: true, refreshLoginWallpaper: true,
    type: 'FNOS_APPLY',
  });
  assert.deepEqual(patch.mods, {
    basePresetEnabled: false, titlebarStyle: 'mac', desktopIconPerColumn: 12,
    brandColor: '#123456', lockscreenDefaultUsername: 'me',
    fontOverrideEnabled: true, fontFamily: 'A', fontMonospaceFamily: 'B', fontWeight: '600',
    fontFeatureSettings: '"liga" 1', fontFaceName: 'X', fontUrl: 'https://x/y.woff2',
  });
  assert.deepEqual(patch.local, { customCssCode: 'body{}', customJsCode: 'void 0' });
  // 未知键 / 非对象输入不得产生任何 patch（空 patch 由调用方直接短路，不发 IPC）
  assert.deepEqual(applyPatchFromPopup({ type: 'FNOS_APPLY', evil: 1 }), {});
  assert.deepEqual(applyPatchFromPopup(null), {});
  assert.deepEqual(applyPatchFromPopup({ fontSettings: 'x', customCodeSettings: 7 }), {});
});

test('F: 列表过大的诊断被认出来并原样带给上游（不让它干等「未读取到应用」）', () => {
  // shim 在「装不下」时回的诊断（shim.js::noteAppItemsSend）：它必须被认出来，
  // 而且**必须排在「空列表 → null」之前**——诊断的 items 就是空数组。
  const diag = {
    type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS',
    dir: 'response',
    payload: { items: [], titles: [], tooLarge: true, itemCount: 260 },
  };
  assert.equal(isTooLargeReport(diag), true);
  const answer = appItemsAnswer(diag);
  assert.equal(answer.tooLarge, true);
  assert.deepEqual(answer.items, []);
  assert.equal(answer.itemCount, 260, '必须把项数原样带出去（说明框要用它）');
  // 关键：诊断**不是** null——若返回 null，调用方会去请页面重汇报一次，而装不下就是装不下，
  // 重问一万次也一样，用户会永远看不到原因。
  assert.notEqual(appItemsAnswer(diag), null);

  // 判据本身要收紧：别的 type、字符串 "true"、缺 payload 都不算诊断
  for (const bad of [
    { type: 'FNOS_CHECK', payload: { tooLarge: true } },
    { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { tooLarge: 'true', itemCount: 9 } },
    { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: {} },
    { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' },
    null,
    'x',
  ]) {
    assert.equal(isTooLargeReport(bad), false, `不得被当成诊断：${JSON.stringify(bad)}`);
  }
  // 项数不是有限数时不把它当数字带出去（页面可控文本不进任何数字位置）
  const noCount = appItemsAnswer(
    { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', payload: { tooLarge: true, itemCount: '很多很多' } }
  );
  assert.equal(noCount.tooLarge, true);
  assert.equal(noCount.itemCount, 0);
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

test('G: 应用项列表只读 appItems 槽位（绝不读状态条的证据槽位）', () => {
  // 源码级断言：应用项那条通路里只允许出现 appItemsReport。两个槽位接错的症状很隐蔽
  // ——列表看起来「还没收到」，而状态条看起来正常——所以用一条机械锁钉住接线本身。
  const src = readFileSync(new URL('../ui/settings/app.js', import.meta.url), 'utf8');
  const start = src.indexOf('export function appItemsAnswer(');
  const end = src.indexOf('function sleep(');
  assert.ok(start > 0 && end > start, 'app.js 必须有 appItemsAnswer 与紧随其后的 sleep');
  const block = src.slice(start, end);
  assert.ok(block.includes('appItemsFromReport(report)'), block.slice(0, 200));
  assert.ok(!block.includes('state.report'), '不得读状态条槽位');
  // answerAppItems（异步那一半）同理：只碰 state.appItemsReport
  const asyncStart = src.indexOf('export async function answerAppItems(');
  const asyncEnd = src.indexOf('export function pageCheckAnswer(');
  assert.ok(asyncStart > 0 && asyncEnd > asyncStart);
  const asyncBlock = src.slice(asyncStart, asyncEnd);
  assert.ok(asyncBlock.includes('appItemsAnswer(state.appItemsReport)'), asyncBlock.slice(0, 200));
  assert.ok(!asyncBlock.includes('state.report'), '不得读状态条槽位');
});

// ---------- H：Task 14b —— 上游 UI 的判定、外壳桥、退休的分组 UI ----------

test('H: pageCheckAnswer 只用宿主观测（识别或页面回报的注入链），错误页一律 false', () => {
  const injected = { type: 'FNOS_INJECTION_TRIGGERED', dir: 'out', payload: {} };
  assert.deepEqual(pageCheckAnswer({ recognized: true }, null), { isFnOSWebUi: true });
  assert.deepEqual(pageCheckAnswer({ recognized: false }, injected), { isFnOSWebUi: true });
  assert.deepEqual(pageCheckAnswer({ recognized: false }, null), { isFnOSWebUi: false });
  // 错误页 / 加载失败：对着一张错误页说 true 只会让上游白干活
  assert.deepEqual(pageCheckAnswer({ recognized: true, errorPage: true }, injected), { isFnOSWebUi: false });
  assert.deepEqual(pageCheckAnswer({ recognized: true, loadFailed: true }, injected), { isFnOSWebUi: false });
  // 取不到主窗口状态（IPC 失败）→ false，绝不猜成 true
  assert.deepEqual(pageCheckAnswer(null, injected), { isFnOSWebUi: false });
  // 伪造的注入回报不升级（与 status.js::reportVerdict 同源）：dir 不是 out 不算
  assert.deepEqual(
    pageCheckAnswer({ recognized: false }, { type: 'FNOS_INJECTION_TRIGGERED', dir: 'response', payload: {} }),
    { isFnOSWebUi: false }
  );
});

test('H: 上游 UI 被装进 iframe，且**只在配置快照就位之后**才创建（getManifest 是同步的）', () => {
  const src = readFileSync(new URL('../ui/settings/app.js', import.meta.url), 'utf8');
  // boot 的次序：先 getConfig + 画状态条，最后才 mountUpstreamFrame
  const boot = src.slice(src.indexOf('export async function boot()'));
  const adoptAt = boot.indexOf('adoptConfig(await api.getConfig())');
  const mountAt = boot.indexOf('mountUpstreamFrame()');
  assert.ok(adoptAt > 0 && mountAt > adoptAt, '必须先取到配置（写快照）再挂上游帧');
  // 快照对象是**同一个引用**（就地更新），否则 shim 的同步读会读到旧对象
  assert.ok(src.includes('window[SNAPSHOT_KEY] = snapshot;'), '桥必须把 snapshot 对象挂到 window 上');
  assert.ok(src.includes('snapshot.config = state.config;'), 'adoptConfig 必须就地更新快照');
  // 帧的源必须是上游页面本体，不是本壳重新拼的 HTML
  assert.ok(src.includes("frame.setAttribute('src', UPSTREAM_PAGE)"), '帧的 src 必须是 UPSTREAM_PAGE');
  // 宿主桥必须**白名单**派发：出现 `HOST_COMMANDS` 表 + 未命中即拒绝
  assert.ok(src.includes('HOST_COMMANDS'), '必须有命令白名单表');
  assert.ok(src.includes('设置窗没有映射这条命令'), '表外的命令名必须被明确拒绝');
});

test('H: 退休的分组 UI 不得留下任何可达引用（settings.html / app.js 都没有 nav/pane/schema）', () => {
  const html = readFileSync(new URL('../ui/settings/settings.html', import.meta.url), 'utf8');
  const app = readFileSync(new URL('../ui/settings/app.js', import.meta.url), 'utf8');
  for (const dead of ['id="nav"', 'id="pane"', 'schema.js', 'SCHEMA']) {
    assert.ok(!html.includes(dead), `settings.html 不得再引用 ${dead}`);
  }
  for (const dead of ["from './schema.js'", 'renderGroup', 'fieldEl(', 'iconChoiceOptions', 'applyIconSelection']) {
    assert.ok(!app.includes(dead), `app.js 不得再引用 ${dead}`);
  }
  // schema.js 这个模块本身已经删除
  assert.throws(() => readFileSync(new URL('../ui/settings/schema.js', import.meta.url), 'utf8'));
  // 上游界面是**主内容**：html 里必须有它的宿主 id（app.js 的 boot 判据也用它）
  assert.ok(html.includes('id="upstreamHost"'), 'settings.html 必须有上游界面宿主');
  assert.ok(html.includes('id="about"'), 'settings.html 必须保留关于/合规容器');
  assert.ok(html.includes('id="status"'), 'settings.html 必须保留状态条');
});

test('T14c: Dock 自动隐藏的开关入口在上游界面里（chrome-shim 注入，右侧外壳栏已删）', () => {
  const app = readFileSync(new URL('../ui/settings/app.js', import.meta.url), 'utf8');
  const shim = readFileSync(new URL('../ui/settings/chrome-shim.js', import.meta.url), 'utf8');
  const html = readFileSync(new URL('../ui/settings/settings.html', import.meta.url), 'utf8');
  // 旧入口必须真正退场：右侧栏删了，app.js 不得残留 dock 开关的任何痕迹
  assert.ok(!app.includes('f_shell_dockAutoHide'), 'app.js 不得再有旧外壳区的 Dock 开关');
  assert.ok(!app.includes("commitShell('dockAutoHide'"), 'app.js 不得再经 commitShell 提交 dockAutoHide');
  assert.ok(!html.includes('shell-side'), 'settings.html 不得再有右侧栏');
  // 新入口在上游界面里：同款行样式 + 诚实标注 + 幂等
  assert.ok(shim.includes("id = 'fnosShellDockToggle'"), '必须有本壳命名空间的开关 id');
  assert.ok(shim.includes("getElementById('siteToggle')"), '必须锚定上游的「为当前站点注入」行');
  assert.ok(shim.includes("closest('.row')"), '插入点必须是上游的 .row');
  assert.ok(shim.includes("dataset.fnosShell = '1'"), '注入的行必须带本壳标记（诚实边界）');
  assert.ok(shim.includes("'自动隐藏 Dock（本壳）'"), '行文字必须标注「本壳」');
  assert.ok(shim.includes("getElementById('fnosShellDockToggle')"), '幂等闸门必须存在');
  // 数据通路：初始态读父 frame 的配置快照，切换写 set_config（宿主免刷新推给主窗口）
  assert.ok(shim.includes('configNow().shell'), '初始态必须读配置快照');
  assert.ok(shim.includes("callHost('set_config', { patch: { shell: { dockAutoHide: !!input.checked } } })"),
    '切换必须经 set_config 的 shell patch');
  assert.ok(shim.includes('input.checked = !input.checked'), '写失败必须回滚开关状态（不假装保存成功）');
});

test('H: 新增的三个 IPC 命令在桥里的名字/参数形状与 Rust 侧一致', async () => {
  const calls = [];
  const prev = globalThis.window;
  globalThis.window = {
    __TAURI_INTERNALS__: { invoke: (cmd, args) => { calls.push([cmd, args]); return Promise.resolve({}); } },
  };
  try {
    await bridge.getLocalStore();
    await bridge.setLocalStore({ updateCheckState: '{"a":1}' });
    await bridge.requestAppItems();
  } finally {
    if (prev === undefined) delete globalThis.window;
    else globalThis.window = prev;
  }
  assert.deepEqual(calls, [
    ['get_local_store', undefined],
    ['set_local_store', { patch: { updateCheckState: '{"a":1}' } }],
    ['request_app_items', undefined],
  ]);
});
