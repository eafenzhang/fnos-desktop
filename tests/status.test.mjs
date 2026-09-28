// Task 11：设置窗顶部状态条（spec §12.3）与 WebView2 低版本提示的纯函数契约。
//
// 这些函数是「状态条说什么」的**唯一**判据：`app.js` 只负责把返回值画到 `#status`
// 上并挂两个按钮。因此这里逐条钉住文案与动作，运行时（UIA 读 `#status` 的
// accessible name）只需要验证「同一份判据被画出来了」，不必再复述一遍分支。
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  addCurrentOriginToWhitelist,
  cornerShapeHint,
  originOfHttpUrl,
  statusFor,
  webviewMajor,
} from '../ui/settings/status.js';

/** 一份最小可用的 `get_config` 回包。 */
function cfg(over = {}) {
  return {
    schemaVersion: 1,
    mods: { enabledOrigins: [], ...(over.mods || {}) },
    local: {},
    shell: { injectEnabled: true, homeUrl: 'https://fnos.net/', ...(over.shell || {}) },
    meta: { recoveredFromBackup: false, ...(over.meta || {}) },
  };
}

/** 一份最小可用的 `get_page_state` 回包。 */
function page(over = {}) {
  return {
    url: 'http://127.0.0.1:8793/plain.html',
    origin: 'http://127.0.0.1:8793',
    recognized: false,
    officialHome: false,
    loadFailed: false,
    errorPage: false,
    loading: false,
    lastError: null,
    nextRetrySeconds: null,
    injectEnabled: true,
    ...over,
  };
}

// ---------------------------------------------------------------- origin 解析

test('originOfHttpUrl: 只认绝对 http(s)，默认端口按 location.origin 语义省略', () => {
  assert.equal(originOfHttpUrl('http://127.0.0.1:8793/plain.html'), 'http://127.0.0.1:8793');
  assert.equal(originOfHttpUrl('https://nas.example.com:8000/a/b?c=1#d'), 'https://nas.example.com:8000');
  assert.equal(originOfHttpUrl('http://host:80/x'), 'http://host');
  assert.equal(originOfHttpUrl('https://host:443/x'), 'https://host');
  assert.equal(originOfHttpUrl('  HTTPS://NAS.LOCAL:5666/ui  '), 'https://nas.local:5666');
});

test('originOfHttpUrl: 非 http(s) / 相对 / 无 host 一律 null（不许返回字面量 "null"）', () => {
  for (const bad of [
    'nas.example.com:8000', // scheme 被当成 `nas.example.com`，`.origin` 是字符串 "null"
    'nas:8000',
    'mailto:a@b.c',
    'data:text/html,<b>x</b>',
    'javascript:alert(1)',
    'file:///C:/x',
    'http://',
    '',
    '   ',
    null,
    undefined,
    42,
  ]) {
    assert.equal(originOfHttpUrl(bad), null, `应当拒绝：${String(bad)}`);
  }
});

test('addCurrentOriginToWhitelist: 追加 + ASCII 小写 + 去重（与 Rust normalize 同语义）', () => {
  assert.deepEqual(addCurrentOriginToWhitelist('HTTP://NAS.LOCAL:8000', ['http://a']), [
    'http://a',
    'http://nas.local:8000',
  ]);
  // 已在名单里（大小写不同）→ 原样返回同一份内容，不产生重复条目
  assert.deepEqual(addCurrentOriginToWhitelist('http://a', [' HTTP://A ']), ['http://a']);
  // 空 / 非法 origin：名单保持不变（调用方据此禁用按钮）
  assert.deepEqual(addCurrentOriginToWhitelist('', ['http://a']), ['http://a']);
  assert.deepEqual(addCurrentOriginToWhitelist(null, []), []);
});

// ------------------------------------------------------------ WebView2 版本

test('webviewMajor: 只接受 x[.y.z…] 形状，其余 null（未知不谎报）', () => {
  assert.equal(webviewMajor('148.0.3967.54'), 148);
  assert.equal(webviewMajor('139'), 139);
  assert.equal(webviewMajor('138.0.1.2'), 138);
  for (const bad of ['', 'abc', '138abc', 'v139.0', null, undefined, 139]) {
    assert.equal(webviewMajor(bad), null, `应当拒绝：${String(bad)}`);
  }
});

test('cornerShapeHint: 仅当主版本 < 139 时提示，未知版本不提示', () => {
  assert.equal(cornerShapeHint('148.0.3967.54'), null);
  assert.equal(cornerShapeHint('139.0.0.0'), null);
  const hint = cornerShapeHint('138.0.3351.48');
  assert.ok(hint.includes('139'), `提示必须点明门槛版本：${hint}`);
  assert.ok(hint.includes('corner-shape'), `提示必须点明退化项：${hint}`);
  assert.equal(cornerShapeHint(undefined), null, '取不到版本时不得声称会退化');
  assert.equal(cornerShapeHint('abc'), null);
});

// ------------------------------------------------------------------ 状态条

test('statusFor: 配置还没读到时是 pending', () => {
  const m = statusFor(null, null);
  assert.equal(m.kind, 'pending');
  assert.ok(m.text.includes('正在读取配置'), m.text);
  assert.deepEqual(m.actions, []);
});

test('statusFor: 配置已加载 + 注入开关状态（spec §12.3 的第 (i) 态）', () => {
  const on = statusFor(cfg(), page({ recognized: true }));
  assert.ok(on.text.includes('配置已加载'), on.text);
  assert.ok(on.text.includes('注入开关：开启'), on.text);
  const off = statusFor(cfg({ shell: { injectEnabled: false } }), page({ recognized: true }));
  assert.ok(off.text.includes('注入开关：关闭'), off.text);
});

test('statusFor: 主窗口状态未知时明确说未知，不猜', () => {
  const m = statusFor(cfg(), null);
  assert.equal(m.kind, 'warn');
  assert.ok(m.text.includes('配置已加载'), m.text);
  assert.ok(m.text.includes('未知'), m.text);
  assert.deepEqual(m.actions, []);
});

test('statusFor: 未检测到 fnOS WebUI → 提示 + 一键加入白名单（第 (ii) 态）', () => {
  const m = statusFor(cfg(), page());
  assert.equal(m.kind, 'warn');
  assert.ok(m.text.includes('未检测到 fnOS WebUI'), m.text);
  assert.ok(m.text.includes('http://127.0.0.1:8793'), m.text);
  assert.deepEqual(m.actions, ['whitelist']);
  assert.equal(m.origin, 'http://127.0.0.1:8793');
});

test('statusFor: 未检测到且解析不出 origin 时不给按钮（没有可加入的东西）', () => {
  const m = statusFor(cfg(), page({ url: 'chrome-error://chromewebdata/', origin: null }));
  assert.equal(m.kind, 'warn');
  assert.deepEqual(m.actions, []);
});

test('statusFor: 加载失败 → error + 重试（第 (iii) 态）', () => {
  const m = statusFor(cfg(), page({
    url: 'http://127.0.0.1:1/',
    origin: 'http://127.0.0.1:1',
    loadFailed: true,
    errorPage: true,
    lastError: '连接被拒绝',
  }));
  assert.equal(m.kind, 'error');
  assert.ok(m.text.includes('加载失败'), m.text);
  assert.ok(m.text.includes('http://127.0.0.1:1/'), m.text);
  assert.ok(m.text.includes('连接被拒绝'), m.text);
  assert.deepEqual(m.actions, ['retry']);
});

test('statusFor: 正在加载时不宣称任何结果', () => {
  const m = statusFor(cfg(), page({ loading: true, url: 'http://127.0.0.1:8793/index.html' }));
  assert.equal(m.kind, 'warn');
  assert.ok(m.text.includes('正在加载'), m.text);
  assert.ok(!m.text.includes('已注册'), m.text);
  assert.deepEqual(m.actions, []);
});

test('statusFor: 白名单命中 → 只说「脚本已注册」，不谎称注入已生效（第 (iv) 态）', () => {
  const m = statusFor(cfg(), page({
    url: 'http://127.0.0.1:8793/index.html',
    origin: 'http://127.0.0.1:8793',
    recognized: true,
  }));
  assert.equal(m.kind, 'ok');
  assert.ok(m.text.includes('注入脚本已注册'), m.text);
  assert.ok(m.text.includes('http://127.0.0.1:8793'), m.text);
  assert.ok(!m.text.includes('已生效'), '仅凭白名单命中不得声称注入已生效');
  assert.deepEqual(m.actions, []);
});

test('statusFor: fnOS 官网（根域）按设计不注入，必须如实说明', () => {
  const m = statusFor(cfg(), page({
    url: 'https://fnos.net/',
    origin: 'https://fnos.net',
    recognized: true,
    officialHome: true,
  }));
  assert.equal(m.kind, 'ok');
  assert.ok(m.text.includes('fnOS 官网'), m.text);
  assert.ok(!m.text.includes('未检测到'), m.text);
  assert.ok(!m.text.includes('已注册'), '官网不注入，不得声称已注册注入脚本');
  assert.deepEqual(m.actions, []);
});

test('statusFor: 白名单命中了但注入开关关着 → 说清楚是开关的问题', () => {
  const m = statusFor(cfg({ shell: { injectEnabled: false } }), page({ recognized: true }));
  assert.equal(m.kind, 'warn');
  assert.ok(m.text.includes('注入开关已关闭'), m.text);
  assert.ok(!m.text.includes('已注册'), m.text);
});

test('statusFor: meta.recoveredFromBackup → 状态条必须回显配置损坏回退（第 D 项）', () => {
  const m = statusFor(cfg({ meta: { recoveredFromBackup: true } }), page({ recognized: true }));
  assert.equal(m.kind, 'warn', '回退过备份就不是「一切正常」');
  assert.ok(m.text.includes('config.json.bak'), m.text);
  assert.ok(m.text.includes('配置'), m.text);
});

test('statusFor: 没有回退过备份时不出现 .bak 文案（不虚报）', () => {
  const m = statusFor(cfg(), page({ recognized: true }));
  assert.ok(!m.text.includes('.bak'), m.text);
});
