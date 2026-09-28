import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const SHIM = readFileSync(new URL('../src-tauri/inject/shim.js', import.meta.url), 'utf8');
const BOOT = readFileSync(new URL('../src-tauri/inject/bootstrap.js', import.meta.url), 'utf8');

function fakeDom() {
  const nodes = [];
  const doc = {
    nodes,
    head: { appendChild: (n) => nodes.push(n) },
    getElementById: (id) => nodes.find((n) => n.id === id) || null,
    createElement: (tag) => ({ tag, setAttribute() {}, style: {} }),
    documentElement: { appendChild: () => {} },
    addEventListener(ev, fn) { (this._listeners ||= {})[ev] = fn; },
    adoptedStyleSheets: [],
    readyState: 'loading'
  };
  return doc;
}

function load(shell, doc, opts = {}) {
  const installed = [];
  class CSSStyleSheetStub {
    constructor() { this.cssText = ''; }
    replaceSync(text) { this.cssText = text; installed.push(this); }
  }
  const cssStub = { supports: () => true };
  const setTimeoutStub = (fn) => { (doc._timers ||= []).push(fn); return (doc._timers.length); };
  const win = { __FNOS_SHELL__: shell, document: doc, CSS: cssStub, CSSStyleSheet: CSSStyleSheetStub };
  win.window = win;
  const fn = new Function(
    'window', 'document', 'TextEncoder', 'queueMicrotask', 'btoa', 'CSS', 'CSSStyleSheet', 'setTimeout',
    SHIM + '\n' + BOOT + '\nreturn window;'
  );
  const w = fn(win, doc, TextEncoder, queueMicrotask, (s) => Buffer.from(s, 'binary').toString('base64'),
               cssStub, CSSStyleSheetStub, setTimeoutStub);
  w.__installedSheets = installed;
  return w;
}

const SHELL = {
  mods: {}, local: {},
  assets: { 'basic_mod.css': 'body{color:red}', 'mod.js': 'window.__MOD_RAN__=(window.__MOD_RAN__||0)+1;' }
};

test('bootstrap 暴露版本与受管 CSS id 列表', () => {
  const doc = fakeDom();
  const w = load(SHELL, doc);
  assert.equal(typeof w.__FNOS_BOOTSTRAP__.version, 'string');
  assert.ok(w.__FNOS_BOOTSTRAP__.cssIds.includes('fnos-ui-mods-basic-style'));
});

test('link 未生效时用 adoptedStyleSheets 补装 CSS', () => {
  const doc = fakeDom();
  const w = load(SHELL, doc);
  w.__FNOS_BOOTSTRAP__.installFallbackCss();
  assert.ok(w.__FNOS_BOOTSTRAP__.fallbackInstalled >= 1);
  assert.ok(doc.adoptedStyleSheets.length >= 1);
  assert.equal(doc.adoptedStyleSheets[0], w.__installedSheets[0]);
  assert.equal(w.__installedSheets[0].cssText, 'body{color:red}');
});

test('mod.js 未执行时兜底执行且只执行一次', () => {
  const doc = fakeDom();
  const w = load(SHELL, doc);
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  assert.equal(w.__MOD_RAN__, 1);
  assert.equal(w.__FNOS_MOD_EXECUTED__, true);
});

test('mod.js 已执行时 ensureModJs 是 no-op（不重复执行、不标记兜底）', () => {
  const doc = fakeDom();
  const w = load(SHELL, doc);
  // 模拟 shim 的 data: <script> 路径已经跑过：标记已置位
  w.__FNOS_MOD_EXECUTED__ = true;
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  assert.equal(w.__MOD_RAN__, undefined);
  assert.equal(w.__FNOS_BOOTSTRAP__.modFallbackUsed, false);
});

test('7 个受管 CSS 文件都可经 adoptedStyleSheets 补装（R29）', () => {
  const doc = fakeDom();
  const files = [
    'basic_mod.css', 'windows_titlebar_mod.css', 'mac_titlebar_mod.css',
    'classic_launchpad_mod.css', 'spotlight_launchpad_mod.css',
    'desktop_icon_mod.css', 'lockscreen_mod.css'
  ];
  const assets = {};
  for (const f of files) assets[f] = '.' + f + '{}';
  const w = load({ mods: {}, local: {}, assets }, doc);
  w.__FNOS_BOOTSTRAP__.installFallbackCss();
  assert.equal(w.__FNOS_BOOTSTRAP__.fallbackInstalled, 7);
  assert.equal(doc.adoptedStyleSheets.length, 7);
  assert.deepEqual(w.__installedSheets.map((s) => s.cssText), files.map((f) => '.' + f + '{}'));
});

test('缺 __FNOS_SHELL__ / assets 时不抛异常', () => {
  const doc = fakeDom();
  const w = load(undefined, doc);
  assert.equal(typeof w.__FNOS_BOOTSTRAP__.version, 'string');
  w.__FNOS_BOOTSTRAP__.installFallbackCss();
  w.__FNOS_BOOTSTRAP__.ensureModJs();
  const w2 = load({ mods: {}, local: {} }, fakeDom());
  w2.__FNOS_BOOTSTRAP__.installFallbackCss();
  w2.__FNOS_BOOTSTRAP__.ensureModJs();
  assert.equal(w2.__FNOS_BOOTSTRAP__.modFallbackUsed, false);
});
