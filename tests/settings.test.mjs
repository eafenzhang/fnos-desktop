// 设置窗的**应用层**行为（Review round 1 的三条修复 + 打磨项）：
//
// A. `adoptConfig` 不得把 IPC 已归一化的值再夹一次（§8.4 的正面落实）
// B. origin 添加只接受 http(s)，拒绝 `"null"` 系（scheme-less / mailto / data / javascript）
// D. 关于页外链走 `open_url` 命令（命令名与参数形状与 Rust 侧一致）
//
// 这些断言依赖「导入 app.js 不需要 DOM」：`boot()` 只在真实页面（有 `#pane`）里自动执行。
import test from 'node:test';
import assert from 'node:assert/strict';
import { adoptConfig, state } from '../ui/settings/app.js';
import * as bridge from '../ui/settings/bridge.js';
import {
  clampLightness, normalizeMods, normalizeModsEntry, normalizeOrigin, parseHttpOrigin
} from '../ui/settings/normalize.js';

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
