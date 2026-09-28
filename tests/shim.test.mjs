import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const SHIM = readFileSync(new URL('../src-tauri/inject/shim.js', import.meta.url), 'utf8');

function loadShim(shell) {
  const win = { __FNOS_SHELL__: shell };
  win.window = win;
  const fn = new Function('window', 'TextEncoder', 'queueMicrotask', 'btoa', SHIM + '\nreturn window;');
  return fn(win, TextEncoder, queueMicrotask, (s) => Buffer.from(s, 'binary').toString('base64'));
}

const SHELL = {
  mods: { brandColor: '#336699', enabledOrigins: ['http://nas.local'], titlebarStyle: 'mac' },
  local: { customCssCode: 'body{}', customJsCode: '' },
  assets: { 'basic_mod.css': 'body{color:red}', 'mod.js': 'window.__MOD_RAN__=1;' },
};

test('runtime.id 是非空字符串（缺失会导致上游完全注入失败）', () => {
  const w = loadShim(SHELL);
  assert.equal(typeof w.chrome.runtime.id, 'string');
  assert.ok(w.chrome.runtime.id.length > 0);
});

test('getURL 对已知资源返回 data: URL，未知资源返回空串', () => {
  const w = loadShim(SHELL);
  const url = w.chrome.runtime.getURL('basic_mod.css');
  assert.match(url, /^data:text\/css;base64,/);
  assert.equal(Buffer.from(url.split(',')[1], 'base64').toString('utf8'), 'body{color:red}');
  assert.equal(w.chrome.runtime.getURL('nope.css'), '');
});

test('getURL("mod.js") 的载荷会置位执行标记', () => {
  const w = loadShim(SHELL);
  const js = Buffer.from(w.chrome.runtime.getURL('mod.js').split(',')[1], 'base64').toString('utf8');
  assert.match(js, /__MOD_RAN__/);
  assert.match(js, /__FNOS_MOD_EXECUTED__/);
});

test('storage.sync.get 用默认值合并已存配置，且回调必被调用', async () => {
  const w = loadShim(SHELL);
  const defaults = { brandColor: '#0066ff', titlebarStyle: 'windows', unknownKey: 7 };
  const got = await new Promise((res) => w.chrome.storage.sync.get(defaults, res));
  assert.equal(got.brandColor, '#336699');
  assert.equal(got.titlebarStyle, 'mac');
  assert.equal(got.unknownKey, 7);
});

test('storage.sync.get 支持字符串/数组/空 keys 三种形式', async () => {
  const w = loadShim(SHELL);
  const byString = await new Promise((res) => w.chrome.storage.sync.get('brandColor', res));
  assert.deepEqual(Object.keys(byString), ['brandColor']);
  const byArray = await new Promise((res) => w.chrome.storage.sync.get(['brandColor', 'nope'], res));
  assert.deepEqual(Object.keys(byArray), ['brandColor']);
  const all = await new Promise((res) => w.chrome.storage.sync.get(null, res));
  assert.equal(all.titlebarStyle, 'mac');
});

test('storage.local.get(null) 返回全量', async () => {
  const w = loadShim(SHELL);
  const all = await new Promise((res) => w.chrome.storage.local.get(null, res));
  assert.equal(all.customCssCode, 'body{}');
});

test('__FNOS_APPLY_CONFIG__ 派发 onChanged 增量', async () => {
  const w = loadShim(SHELL);
  const events = [];
  w.chrome.storage.onChanged.addListener((changes, area) => events.push([area, changes]));
  w.__FNOS_APPLY_CONFIG__({ mods: { brandColor: '#ff0000' }, local: {} });
  assert.equal(events.length, 1);
  assert.equal(events[0][0], 'sync');
  assert.equal(events[0][1].brandColor.newValue, '#ff0000');
  assert.equal(events[0][1].brandColor.oldValue, '#336699');
});

test('getManifest 返回 meta.modsVersion，缺 meta 时回落 0.0.0', () => {
  const w = loadShim({ ...SHELL, meta: { modsVersion: '1.2.3' } });
  assert.equal(w.chrome.runtime.getManifest().version, '1.2.3');
  const fallback = loadShim(SHELL);
  assert.equal(typeof fallback.chrome.runtime.getManifest().version, 'string');
  assert.equal(fallback.chrome.runtime.getManifest().version, '0.0.0');
});

test('getURL 大小写不敏感（icon-map 值是小写，文件名是 camelCase）', () => {
  const w = loadShim({
    mods: {}, local: {},
    assets: { 'prefect_icon/panIndex.png': 'iVBORw0KGgo=' }
  });
  const lower = w.chrome.runtime.getURL('prefect_icon/panindex.png');
  const exact = w.chrome.runtime.getURL('prefect_icon/panIndex.png');
  assert.ok(lower.length > 0);
  assert.equal(lower, exact);
});

test('binaryAssets 直接产出 base64，不经过文本编码（防 PNG 损坏）', () => {
  const b64 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==';
  const w = loadShim({ mods: {}, local: {}, binaryAssets: { 'prefect_icon/xunlei.png': b64 } });
  assert.equal(w.chrome.runtime.getURL('prefect_icon/xunlei.png'), 'data:image/png;base64,' + b64);
});

test('同一资源同时出现在 assets 与 binaryAssets 时，binaryAssets 优先', () => {
  const w = loadShim({
    mods: {}, local: {},
    assets: { 'prefect_icon/a.png': 'not-base64-text' },
    binaryAssets: { 'prefect_icon/a.png': 'AAAA' }
  });
  assert.equal(w.chrome.runtime.getURL('prefect_icon/a.png'), 'data:image/png;base64,AAAA');
});

test('非字符串资源值返回空串（上游 cs:2622 的注入闸门依赖空串）', () => {
  const w = loadShim({ mods: {}, local: {}, assets: { 'a.css': 123, 'b.css': null, 'c.css': { x: 1 } } });
  assert.equal(w.chrome.runtime.getURL('a.css'), '');
  assert.equal(w.chrome.runtime.getURL('b.css'), '');
  assert.equal(w.chrome.runtime.getURL('c.css'), '');
});

test('非字符串 binaryAssets 值同样返回空串（不得落入文本分支）', () => {
  const w = loadShim({ mods: {}, local: {}, binaryAssets: { 'd.png': 42 } });
  assert.equal(w.chrome.runtime.getURL('d.png'), '');
});

test('pick 用 hasOwnProperty 判定成员，原型键返回空对象且真实键仍可解析', async () => {
  const w = loadShim(SHELL);
  const proto = await new Promise((res) => w.chrome.storage.sync.get('constructor', res));
  assert.deepEqual(proto, {});
  const protoFn = await new Promise((res) => w.chrome.storage.sync.get('toString', res));
  assert.deepEqual(protoFn, {});
  const protoObj = await new Promise((res) => w.chrome.storage.sync.get({ constructor: 1 }, res));
  assert.equal(protoObj.constructor, 1);
  const own = await new Promise((res) => w.chrome.storage.sync.get('brandColor', res));
  assert.deepEqual(own, { brandColor: '#336699' });
  const ownArray = await new Promise((res) => w.chrome.storage.sync.get(['constructor', 'brandColor'], res));
  assert.deepEqual(ownArray, { brandColor: '#336699' });
});

test('无回调的 get 返回 Promise（上游 cs:2548/2574/2594 用 await 形态）', async () => {
  const w = loadShim(SHELL);
  const localDefault = await w.chrome.storage.local.get({ a: 1 });
  assert.deepEqual(localDefault, { a: 1 });
  const merged = await w.chrome.storage.local.get({ customCssCode: 'override', extra: 5 });
  assert.equal(merged.customCssCode, 'body{}');
  assert.equal(merged.extra, 5);
  const syncArray = await w.chrome.storage.sync.get(['brandColor']);
  assert.deepEqual(syncArray, { brandColor: '#336699' });
});
