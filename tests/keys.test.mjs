// keys.js（P0-3）的契约：标签页快捷键只认四个白名单组合，且不抢页面编辑区的按键。
//
// 这个脚本跑在每个标签页里（焦点在哪就在哪生效），把组合键写成 `document.title` 的
// `FNOSKEY:<action>` 控制前缀，由宿主 `commands.rs::handle_tab_key` 执行。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const KEYS = readFileSync(new URL('../src-tauri/inject/keys.js', import.meta.url), 'utf8');

/** 假环境：document + 顶层 window + 最小定时器。 */
function fakeEnv(opts = {}) {
  const listeners = [];
  const doc = {
    title: opts.title === undefined ? 'fnOS 桌面' : opts.title,
    _created: 0,
    addEventListener(type, fn) { if (type === 'keydown') listeners.push(fn); },
    createElement() { this._created += 1; return {}; },
  };
  const timers = [];
  const win = {
    document: doc,
    top: null,
    self: null,
    setTimeout(fn, ms) { timers.push({ fn, ms }); return timers.length; },
    clearTimeout() {},
  };
  win.top = win;
  win.self = win;
  return { win, doc, listeners, timers };
}

function load(env) {
  const fn = new Function('window', 'globalThis', KEYS + '\nreturn window;');
  const w = fn(env.win, env.win);
  return w;
}

/** 造一个键盘事件；`over` 可覆盖任意字段（repeat/altKey/target/...）。 */
function key(over = {}) {
  return {
    key: 't',
    ctrlKey: true,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    repeat: false,
    target: null,
    prevented: false,
    stopped: false,
    preventDefault() { this.prevented = true; },
    stopPropagation() { this.stopped = true; },
    ...over,
  };
}

/** 派发一个按键，返回事件对象 + 期间写出的标题。 */
function press(env, ev) {
  for (const fn of env.listeners) fn(ev);
  return ev;
}

test('源码级：只认四个动作、只认物理 Ctrl、不碰 DOM 内容、不开 IPC', () => {
  assert.ok(KEYS.includes("var KEY_TITLE_PREFIX = 'FNOSKEY:';"), '前缀必须写死成 FNOSKEY:');
  for (const act of ["'new'", "'close'", "'next'", "'prev'"]) {
    assert.ok(KEYS.includes(act), `白名单动作缺失：${act}`);
  }
  assert.ok(KEYS.includes('if (!e.ctrlKey || e.altKey || e.metaKey) return null;'),
    '只认物理 Ctrl（Alt/Meta 留给系统）');
  assert.ok(KEYS.includes('e.repeat'), '长按不得连发');
  assert.ok(KEYS.includes('isContentEditable'), '编辑区不得抢键');
  assert.ok(!KEYS.includes('innerHTML'), '不写页面内容');
  assert.ok(!KEYS.includes('invoke('), '不开 IPC（走既有的标题通道）');
  assert.ok(KEYS.includes('W.top === W.self'), '只在顶层文档工作');
});

test('Ctrl+T → new，Ctrl+W → close，Ctrl+Tab → next，Ctrl+Shift+Tab → prev', () => {
  const cases = [
    [{ key: 't' }, 'new'],
    [{ key: 'T' }, 'new'],
    [{ key: 'w' }, 'close'],
    [{ key: 'Tab' }, 'next'],
    [{ key: 'Tab', shiftKey: true }, 'prev'],
  ];
  for (const [over, expected] of cases) {
    const env = fakeEnv();
    load(env);
    const ev = press(env, key(over));
    assert.equal(env.doc.title, `FNOSKEY:${expected}`, `按键应映射到 ${expected}`);
    assert.equal(ev.prevented, true, '消费掉的组合键必须 preventDefault');
    assert.equal(ev.stopped, true, '也不该继续冒泡给页面');
    // 用完把标题还回去
    env.timers[env.timers.length - 1].fn();
    assert.equal(env.doc.title, 'fnOS 桌面', '控制标题必须还原成页面原标题');
  }
});

test('非白名单一律不碰：不 preventDefault、不写标题', () => {
  const cases = [
    { key: 't', ctrlKey: false },              // 裸 t：页面要能正常输入
    { key: 't', altKey: true },                // Alt+T 属于系统/菜单
    { key: 't', metaKey: true },               // Win+T
    { key: 'a' },                              // Ctrl+A 全选：留给页面
    { key: 'w', shiftKey: true },              // Ctrl+Shift+W：不是我们要的
    { key: 'Escape' },
    { key: 'F5' },
    { key: 't', repeat: true },                // 长按
  ];
  for (const over of cases) {
    const env = fakeEnv();
    load(env);
    const ev = press(env, key(over));
    assert.equal(env.doc.title, 'fnOS 桌面', `不得写控制标题：${JSON.stringify(over)}`);
    assert.equal(ev.prevented, false, `不得吞掉非白名单按键：${JSON.stringify(over)}`);
  }
});

test('编辑区（input / textarea / contenteditable）：会关掉东西的两个不抢，切标签照常', () => {
  for (const target of [
    { tagName: 'INPUT' },
    { tagName: 'TEXTAREA' },
    { tagName: 'SELECT' },
    { tagName: 'DIV', isContentEditable: true },
  ]) {
    // Ctrl+T / Ctrl+W 在输入框里会让人瞬间丢表单：必须让给页面
    for (const over of [{ key: 't' }, { key: 'w' }]) {
      const env = fakeEnv();
      load(env);
      const ev = press(env, key({ ...over, target }));
      assert.equal(env.doc.title, 'fnOS 桌面', `编辑区不得发 ${over.key}：${target.tagName}`);
      assert.equal(ev.prevented, false, '编辑区里的按键不得被吃掉');
    }
    // 纯导航（Ctrl+Tab）在编辑区里也应该能用——它不销毁任何东西
    const env = fakeEnv();
    load(env);
    press(env, key({ key: 'Tab', target }));
    assert.equal(env.doc.title, 'FNOSKEY:next', `编辑区里 Ctrl+Tab 仍应切标签：${target.tagName}`);
  }
});

test('还原只在「标题仍是我们写的控制串」时发生（页面自己改了就不覆盖）', () => {
  const env = fakeEnv();
  load(env);
  press(env, key({ key: 'Tab' }));
  assert.equal(env.doc.title, 'FNOSKEY:next');
  // 页面在这期间自己改了标题（SPA 路由切换）：还原必须放手
  env.doc.title = '文件管理 - MiniNas';
  env.timers[env.timers.length - 1].fn();
  assert.equal(env.doc.title, '文件管理 - MiniNas', '不得把页面自己的标题改回去');
});

test('连续两次按键共用同一个还原定时器（不堆定时器）', () => {
  const env = fakeEnv();
  load(env);
  press(env, key({ key: 'Tab' }));
  press(env, key({ key: 'w' }));
  assert.equal(env.doc.title, 'FNOSKEY:close', '第二次命令覆盖第一次（标题通道只能表达最后一条）');
  assert.equal(env.timers.length, 2, '第二次应清掉前一个定时器再挂一个');
});

test('诊断口可读（页面上不画任何东西）', () => {
  const env = fakeEnv();
  const w = load(env);
  assert.equal(typeof w.__FNOS_KEYS_STATE__, 'function');
  const st = w.__FNOS_KEYS_STATE__();
  assert.equal(st.prefix, 'FNOSKEY:');
  assert.equal(st.pending, false);
  press(env, key({ key: 't' }));
  assert.equal(w.__FNOS_KEYS_STATE__().pending, true, '有待还原的标题时要如实反映');
});
