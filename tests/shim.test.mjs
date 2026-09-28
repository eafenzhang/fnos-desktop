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

test('getManifest 返回对象且含 version', () => {
  const w = loadShim(SHELL);
  assert.equal(typeof w.chrome.runtime.getManifest().version, 'string');
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
