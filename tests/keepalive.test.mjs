// keepalive.js（T14c）的契约：登录保活心跳 —— 同源写死路径 + 协议/同源校验 +
// 登录页跳过 + 钩子链式保留 + 失败可观测（页面上不画任何东西）。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const KA = readFileSync(new URL('../src-tauri/inject/keepalive.js', import.meta.url), 'utf8');
const DOCK = readFileSync(new URL('../src-tauri/inject/dock.js', import.meta.url), 'utf8');

/** 假环境：只给 keepalive.js 需要的那几样（document/window/fetch/定时器/location）。 */
function fakeEnv(opts = {}) {
  const doc = {
    _password: !!opts.loginPage,
    querySelector(sel) { return sel === 'input[type="password"]' && this._password ? {} : null; },
  };
  const timers = { interval: null, timeout: null, cleared: 0 };
  const calls = [];
  const win = {
    document: doc,
    location: {
      href: opts.href === undefined ? 'https://nas.example.com/' : opts.href,
      origin: opts.origin === undefined ? 'https://nas.example.com' : opts.origin,
    },
    URL,
    fetch(url, init) {
      calls.push({ url, init });
      if (opts.fetchFails) return Promise.reject(new Error('网络断了'));
      return Promise.resolve({ status: opts.status === undefined ? 200 : opts.status });
    },
    setTimeout(fn, ms) { timers.timeout = { fn, ms }; return 1; },
    clearTimeout() { timers.cleared += 1; timers.timeout = null; },
    setInterval(fn, ms) { timers.interval = { fn, ms }; return 2; },
    clearInterval() { timers.cleared += 1; timers.interval = null; },
  };
  win.top = win;
  win.self = win;
  return { win, doc, timers, calls };
}

function load(env, shell) {
  env.win.__FNOS_SHELL__ = { shell: shell || {} };
  const fn = new Function('window', 'URL', KA + '\nreturn window;');
  return fn(env.win, URL);
}

/** 触发「第一拍」的延迟定时器。 */
async function firstTick(env) {
  const t = env.timers.timeout;
  assert.ok(t, '应当安排第一拍');
  t.fn();
  await new Promise((r) => setImmediate(r));
}

// ---------- 源码级：安全边界与纪律 ----------

test('安全边界：路径写死、无用户可控 URL、发请求前校验协议与同源、不碰 DOM', () => {
  assert.ok(KA.includes("var TOKEN_PATH = '/app/token';"), '路径必须是写死的常量');
  assert.ok(!KA.includes('invoke('), '不开 IPC');
  assert.ok(!KA.includes('innerHTML'), '不碰 DOM 内容');
  assert.ok(!/document\.createElement|appendChild/.test(KA), '页面上不画任何东西');
  // 校验：协议 + 同源（跨源一律不发）
  assert.ok(KA.includes('url.origin !== origin'), '必须做同源校验');
  assert.ok(KA.includes("url.protocol !== 'http:'") && KA.includes("url.protocol !== 'https:'"),
    '必须限制 http/https');
  assert.ok(KA.includes("credentials: 'same-origin'"), 'cookie 只回给同源');
  assert.ok(KA.includes("cache: 'no-store'"), '不吃缓存（每次真的打到服务端）');
  // 顶层文档守卫（与 dock.js 同一条纪律）
  assert.ok(KA.includes('W.top === W.self'), '只在顶层文档工作');
});

test('钩子链式保留：安装 keepalive 不得覆盖 dock.js 已装的 __FNOS_APPLY_SHELL__', () => {
  const env = fakeEnv();
  // dock.js 先装（它自己也会链式保留更早的钩子）
  const fn = new Function('window', DOCK + '\nreturn window;');
  fn(env.win);
  assert.equal(typeof env.win.__FNOS_APPLY_SHELL__, 'function', 'dock 钩子先就位');
  load(env, { dockAutoHide: true });
  // 两个包都拿到 patch：dock 的 class 有动静、keepalive 的间隔有动静
  env.win.__FNOS_APPLY_SHELL__({ dockAutoHide: true, keepAliveMinutes: 5 });
  const st = env.win.__FNOS_KEEPALIVE_STATE__();
  assert.equal(st.minutes, 5, 'keepalive 必须拿到自己那个键');
  assert.equal(st.enabled, true);
});

// ---------- 开关与间隔 ----------

test('默认 10 分钟开跑；0 / 负数 = 关闭；上限夹到一天', () => {
  const on = fakeEnv();
  load(on, {});
  let st = on.win.__FNOS_KEEPALIVE_STATE__();
  assert.equal(st.enabled, true, '缺省即开启（用户要的保活）');
  assert.equal(st.minutes, 10, '默认 10 分钟');
  assert.equal(on.timers.interval.ms, 10 * 60000, '周期 = 分钟 × 60000');
  assert.ok(on.timers.timeout && on.timers.timeout.ms <= 60000, '第一拍不该拖太久');

  for (const off of [0, -5]) {
    const e = fakeEnv();
    load(e, { keepAliveMinutes: off });
    assert.equal(e.win.__FNOS_KEEPALIVE_STATE__().enabled, false, `${off} 必须关闭心跳`);
    assert.equal(e.timers.interval, null, '关闭时不得留下定时器');
  }

  const capped = fakeEnv();
  load(capped, { keepAliveMinutes: 99999 });
  assert.equal(capped.win.__FNOS_KEEPALIVE_STATE__().minutes, 1440, '上限一天');

  // 周期下限 1 分钟（防手改配置写成 0.01 分钟级别的风暴）
  const tiny = fakeEnv();
  load(tiny, { keepAliveMinutes: 0.1 });
  assert.ok(tiny.timers.interval.ms >= 60000, `周期下限 1 分钟，实得 ${tiny.timers.interval.ms}`);
});

// ---------- 打一拍的行为 ----------

test('打一拍：同源 GET /app/token（带凭据、不缓存），并记录状态', async () => {
  const env = fakeEnv();
  load(env, { keepAliveMinutes: 10 });
  await firstTick(env);
  assert.equal(env.calls.length, 1, '应当发一次请求');
  assert.equal(env.calls[0].url, 'https://nas.example.com/app/token');
  assert.equal(env.calls[0].init.method, 'GET');
  assert.equal(env.calls[0].init.credentials, 'same-origin');
  assert.equal(env.calls[0].init.cache, 'no-store');
  const st = env.win.__FNOS_KEEPALIVE_STATE__();
  assert.equal(st.ticks, 1);
  assert.equal(st.lastStatus, 200);
  assert.equal(st.lastError, null);
  assert.ok(st.lastAt > 0, '必须记下时间');
});

test('登录页（有密码输入框）不发请求：不打扰登录，也不假装成功', async () => {
  const env = fakeEnv({ loginPage: true });
  load(env, { keepAliveMinutes: 10 });
  await firstTick(env);
  assert.equal(env.calls.length, 0, '登录页必须跳过');
  const st = env.win.__FNOS_KEEPALIVE_STATE__();
  assert.equal(st.ticks, 0);
  assert.equal(st.onLoginPage, true);
});

test('协议与同源闸门：非 http(s) 或跨源一律不发（宁可不发）', async () => {
  const bad = [
    { href: 'about:blank', origin: 'null' },
    { href: 'file:///C:/x.html', origin: 'null' },
    { href: 'https://evil.example.com/', origin: 'https://nas.example.com' }, // origin 对不上
  ];
  for (const opts of bad) {
    const env = fakeEnv(opts);
    load(env, { keepAliveMinutes: 10 });
    await firstTick(env);
    assert.equal(env.calls.length, 0, `不得发请求：${opts.href}`);
    assert.equal(env.win.__FNOS_KEEPALIVE_STATE__().lastError, null, '跳过不算失败（不谎报错误）');
  }
});

test('会话已死/网络异常：只记录状态，绝不影响页面', async () => {
  const env = fakeEnv({ status: 401 });
  load(env, { keepAliveMinutes: 10 });
  await firstTick(env);
  assert.equal(env.win.__FNOS_KEEPALIVE_STATE__().lastStatus, 401, '状态必须如实可观测');
  assert.equal(env.calls.length, 1);

  const failing = fakeEnv({ fetchFails: true });
  load(failing, { keepAliveMinutes: 10 });
  await firstTick(failing);
  const st = failing.win.__FNOS_KEEPALIVE_STATE__();
  assert.equal(st.lastStatus, null);
  assert.match(String(st.lastError), /网络断了/, '错误文本要留下（排障用）');
});

test('免刷新改间隔：改完立刻按新周期重排；同值幂等；非法形状不动手', () => {
  const env = fakeEnv();
  const w = load(env, { keepAliveMinutes: 10 });
  const first = env.timers.interval;
  w.__FNOS_APPLY_SHELL__({ keepAliveMinutes: 30 });
  assert.notEqual(env.timers.interval, first, '换间隔必须重排定时器');
  assert.equal(env.timers.interval.ms, 30 * 60000);
  const again = env.timers.interval;
  w.__FNOS_APPLY_SHELL__({ keepAliveMinutes: 30 });
  assert.equal(env.timers.interval, again, '同值幂等（不得重建定时器）');
  // 不含本键的 patch（例如只有 dockAutoHide）：本包什么都不做
  const before = env.timers.interval;
  w.__FNOS_APPLY_SHELL__({ dockAutoHide: true });
  assert.equal(env.timers.interval, before);
  w.__FNOS_APPLY_SHELL__(null);
  w.__FNOS_APPLY_SHELL__('x');
  assert.equal(env.timers.interval, before, '非法形状不得拆掉心跳');
  // 关掉
  w.__FNOS_APPLY_SHELL__({ keepAliveMinutes: 0 });
  assert.equal(env.win.__FNOS_KEEPALIVE_STATE__().enabled, false);
  assert.equal(env.timers.interval, null, '关闭必须清掉定时器');
});

test('手动打拍口可用（排障用），且不在页面留痕', async () => {
  const env = fakeEnv();
  const w = load(env, { keepAliveMinutes: 10 });
  await w.__FNOS_KEEPALIVE_TICK__();
  await new Promise((r) => setImmediate(r));
  assert.equal(env.calls.length, 1, '手动口必须真的发一拍');
  assert.equal(typeof w.__FNOS_KEEPALIVE_STATE__, 'function');
});
