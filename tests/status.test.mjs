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
  MAX_TRIGGER_REASON_CHARS,
  originOfHttpUrl,
  reportVerdict,
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

// ------------------------------------------------ Task 13a：页面上报（据实化）

/**
 * 一条**真实的**上报：宿主把上游 `content-script.js:2681-2687` 的
 * `chrome.runtime.sendMessage({type:'FNOS_INJECTION_TRIGGERED', triggerReason, origin, href, timestamp})`
 * 放进信封的 `payload`（原文，一个字段不改），`dir:'out'` = 上游自己发出去的。
 */
function report(over = {}, payloadOver = {}) {
  return {
    type: 'FNOS_INJECTION_TRIGGERED',
    dir: 'out',
    payload: {
      type: 'FNOS_INJECTION_TRIGGERED',
      triggerReason: 'auto_whitelist',
      origin: 'http://127.0.0.1:8793',
      href: 'http://127.0.0.1:8793/index.html',
      timestamp: 1759000000000,
      ...payloadOver,
    },
    ...over,
  };
}

test('reportVerdict: 只有「上游自己发出的注入链信号」才算注入已触发', () => {
  assert.deepEqual(reportVerdict(report()), { injected: true, reason: 'auto_whitelist' });
  // payload 缺 triggerReason：仍然是「已触发」，只是没有原因可报（不许因此降级）
  assert.deepEqual(reportVerdict(report({}, { triggerReason: undefined })), { injected: true, reason: null });
});

test('reportVerdict: 其余一切形状都不升级（宁可退回弱文案）', () => {
  for (const bad of [
    null,
    undefined,
    'FNOS_INJECTION_TRIGGERED',
    42,
    {},
    // type 不在上报协议里（brief 里那个凭空的 FNOS_PAGE_STATUS 就属于这一档）
    { type: 'FNOS_PAGE_STATUS', dir: 'out', payload: { injected: true } },
    // 应答方向：别人问它、它作答，不代表这次加载触发过注入
    report({ dir: 'response' }),
    // dir 缺失 / 类型错误
    report({ dir: undefined }),
    report({ dir: true }),
    // 应用项列表那种应答（type 是请求类型）也不得升级
    { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', dir: 'out', payload: { items: [] } },
  ]) {
    assert.deepEqual(reportVerdict(bad), { injected: false, reason: null }, `不该升级：${JSON.stringify(bad)}`);
  }
});

test('statusFor: 有真实上报 → 才敢说「注入链已触发」（不再只说「脚本已注册」）', () => {
  const m = statusFor(cfg(), page({
    url: 'http://127.0.0.1:8793/index.html',
    origin: 'http://127.0.0.1:8793',
    recognized: true,
  }), report());
  assert.equal(m.kind, 'ok');
  assert.ok(m.text.includes('已回报上游注入链触发'), m.text);
  assert.ok(m.text.includes('auto_whitelist'), `必须带上上游给的 triggerReason：${m.text}`);
  assert.ok(m.text.includes('http://127.0.0.1:8793'), m.text);
  assert.ok(!m.text.includes('注入脚本已注册'), '有上报时不该再用弱文案');
});

test('statusFor: 没有上报 → 保持 Task 11 的弱文案（不谎称已注入）', () => {
  const p = page({ url: 'http://127.0.0.1:8793/index.html', origin: 'http://127.0.0.1:8793', recognized: true });
  for (const r of [null, undefined, report({ dir: 'response' }), { type: 'FNOS_CHECK', dir: 'out', payload: {} }]) {
    const m = statusFor(cfg(), p, r);
    assert.equal(m.kind, 'ok');
    assert.ok(m.text.includes('注入脚本已注册'), m.text);
    assert.ok(!m.text.includes('已回报'), `不得声称有上报：${m.text}`);
  }
});

test('statusFor: 上报不得盖过加载失败 / 正在加载（宿主自己的错误证据优先）', () => {
  const failed = statusFor(cfg(), page({ loadFailed: true, url: 'http://127.0.0.1:1/', lastError: '连接被拒绝' }), report());
  assert.equal(failed.kind, 'error');
  assert.ok(failed.text.includes('加载失败'), failed.text);
  assert.ok(!failed.text.includes('已回报'), failed.text);
  assert.deepEqual(failed.actions, ['retry']);

  const loading = statusFor(cfg(), page({ loading: true }), report());
  assert.equal(loading.kind, 'warn');
  assert.ok(loading.text.includes('正在加载'), loading.text);
  assert.ok(!loading.text.includes('已回报'), loading.text);
});

test('statusFor: 页面上报了注入链、但宿主没认出这个地址 → 两件事都要说', () => {
  // 上游还有 DOM 签名 / fnos-token / appcgi 资源三条签名路径（cs:2746-2779），
  // 因此「宿主未识别」与「页面已注入」可以同时为真，不能只说一半。
  const m = statusFor(cfg(), page({ recognized: false }), report({}, { triggerReason: 'auto_suspected' }));
  assert.equal(m.kind, 'ok');
  assert.ok(m.text.includes('已回报上游注入链触发'), m.text);
  assert.ok(m.text.includes('auto_suspected'), m.text);
  assert.ok(m.text.includes('宿主未将该地址识别为 fnOS WebUI'), m.text);
  assert.ok(!m.text.includes('未检测到 fnOS WebUI'), '已经确知注入了，不能再说「未检测到」');
});

test('statusFor: 上报里带的应用项应答不得被当成注入证据（Task 13b 的前置契约）', () => {
  const items = { type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS', dir: 'response', payload: { items: [{ key: 'a' }], titles: ['a'] } };
  const m = statusFor(cfg(), page({ recognized: true }), items);
  assert.ok(m.text.includes('注入脚本已注册'), m.text);
  assert.ok(!m.text.includes('已回报'), m.text);
});

// ---------------------------------- fix round 1 / Minor 4：分支次序不得自相矛盾

test('statusFor: 注入开关关着 → 页面即便回报了「注入链已触发」也不升级', () => {
  // 缺陷本体：旧次序先判 verdict.injected，于是同一个状态条会同时出现
  // 「注入开关：关闭」与「页面已回报上游注入链触发」（而标题通道对页面公开可写——
  // Task 13a 报告 §4.3 的第 4 个读数正是页面自己伪造这种形状的实测）。
  const off = statusFor(cfg({ shell: { injectEnabled: false } }), page({ recognized: true }), report());
  assert.equal(off.kind, 'warn');
  assert.ok(off.text.includes('注入开关：关闭'), off.text);
  assert.ok(!off.text.includes('已回报'), `开关关着时不得声称已回报注入链：${off.text}`);
  assert.ok(off.text.includes('但注入开关已关闭'), off.text);

  // 未识别 + 开关关着：宿主的两件事都要说，且保留一键白名单（与 Task 11 的行为一致）
  const offUnknown = statusFor(cfg({ shell: { injectEnabled: false } }), page({ recognized: false }), report());
  assert.ok(offUnknown.text.includes('未检测到 fnOS WebUI'), offUnknown.text);
  assert.ok(!offUnknown.text.includes('已回报'), offUnknown.text);
  assert.deepEqual(offUnknown.actions, ['whitelist']);

  // 开关开着时仍然照旧升级（次序改动没有把强态一起关掉）
  const on = statusFor(cfg(), page({ recognized: true }), report());
  assert.ok(on.text.includes('已回报上游注入链触发'), on.text);
});

test('statusFor: fnOS 官网页即便有上报也不声称已注入（官网按设计不注入）', () => {
  const home = page({ url: 'https://fnos.net/', origin: 'https://fnos.net', recognized: true, officialHome: true });
  const m = statusFor(cfg(), home, report());
  assert.equal(m.kind, 'ok');
  assert.ok(m.text.includes('fnOS 官网'), m.text);
  assert.ok(!m.text.includes('已回报'), `官网页不得声称注入链触发：${m.text}`);
  // 开关关着 + 官网页：仍然是官网文案（两支都与「不注入」一致，不冲突）
  const off = statusFor(cfg({ shell: { injectEnabled: false } }), home, report());
  assert.ok(off.text.includes('fnOS 官网'), off.text);
  assert.ok(!off.text.includes('已回报'), off.text);
});

test('reportVerdict: 页面可控的 triggerReason 进 UI 前被整形（去控制字符 + 按码点截断）', () => {
  const hostile = 'auto\u0000_whitelist\n[fnos] 页面上报已接受：伪造行\t' + 'x'.repeat(200);
  const v = reportVerdict(report({}, { triggerReason: hostile }));
  assert.equal(v.injected, true, '整形 reason 不改变「这条算不算注入证据」的判定');
  assert.ok(!/[\u0000-\u001f\u007f-\u009f]/.test(v.reason), `不得含控制字符：${JSON.stringify(v.reason)}`);
  assert.ok(
    Array.from(v.reason).length <= MAX_TRIGGER_REASON_CHARS + 1,
    `截断到上限 + 省略号：${Array.from(v.reason).length}`
  );
  assert.ok(v.reason.endsWith('…'), v.reason);
  assert.ok(!v.reason.includes('\n'));
  // 同一份文本画到状态条上也必须还是一行（UIA 读到的 accessible name 就是评审证据）
  const m = statusFor(cfg(), page({ recognized: true }), report({}, { triggerReason: hostile }));
  assert.equal(m.text.split('\n').length, 1, m.text);
  assert.ok(m.text.includes('已回报'), m.text);
  assert.ok(!m.text.includes('伪造行\u0000'), '控制字符必须已被替换');
  // 全是空白 / 控制字符 → 没有原因可说（退回不带 triggerReason 的强文案）
  assert.deepEqual(reportVerdict(report({}, { triggerReason: ' \n\t ' })), { injected: true, reason: null });
  // 正常值逐字保留（不截断、不折叠内部文本）
  assert.deepEqual(reportVerdict(report()), { injected: true, reason: 'auto_whitelist' });
});
