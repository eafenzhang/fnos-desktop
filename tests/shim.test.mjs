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

// ------------------------------------------------ Task 13a：document.title 上报通道

const HOST_SHELL = { ...SHELL, meta: { shellVersion: '0.1.0', modsVersion: '1.0.2' } };

/** 载入 shim 并挂一个最小 `document`（上报通道只碰 `document.title`）。 */
function loadShimWithDoc(shell, title = '页面自己的标题') {
  const w = { __FNOS_SHELL__: shell, document: { title } };
  w.window = w;
  const fn = new Function('window', 'TextEncoder', 'queueMicrotask', 'btoa', SHIM + '\nreturn window;');
  return fn(w, TextEncoder, queueMicrotask, (s) => Buffer.from(s, 'binary').toString('base64'));
}

/** 上游 `content-script.js:2681-2687` 实际发出的那条消息（逐字形状）。 */
function injectionTriggered(reason = 'auto_whitelist') {
  return {
    type: 'FNOS_INJECTION_TRIGGERED',
    triggerReason: reason,
    origin: 'http://127.0.0.1:8793',
    href: 'http://127.0.0.1:8793/index.html',
    timestamp: 1759000000000,
  };
}

function parseReportTitle(title) {
  assert.ok(title.startsWith('FNOSREPORT:'), `标题必须是上报前缀：${title}`);
  return JSON.parse(title.slice('FNOSREPORT:'.length));
}

test('sendMessage：上游发出注入链信号时，把**原文**写进标题信封（dir=out）', () => {
  const w = loadShimWithDoc(HOST_SHELL);
  w.chrome.runtime.sendMessage(injectionTriggered());
  const body = parseReportTitle(w.document.title);
  assert.equal(body.type, 'FNOS_INJECTION_TRIGGERED');
  assert.equal(body.dir, 'out');
  // payload 必须与上游对象逐字一致（本层不改一个字段）
  assert.deepEqual(body.payload, injectionTriggered());
});

test('sendMessage：上游的应答也回传原文（dir=response），且只回传第一次应答', () => {
  const w = loadShimWithDoc(HOST_SHELL);
  w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    // 非应用项类型的应答必须逐字回传（应用项列表会剥掉 iconSrc，见那条专门的用例）
    if (msg && msg.type === 'FNOS_APPLY') {
      sendResponse({ applied: true, echo: { nested: [1, 2, 3] } });
    }
  });
  w.chrome.runtime.onMessage.addListener(() => { /* 第二个监听者不该覆盖上游的应答 */ });
  w.chrome.runtime.sendMessage({ type: 'FNOS_APPLY' });
  const body = parseReportTitle(w.document.title);
  assert.equal(body.type, 'FNOS_APPLY');
  assert.equal(body.dir, 'response');
  assert.deepEqual(body.payload, { applied: true, echo: { nested: [1, 2, 3] } });
});

test('sendMessage：应用项应答只回传 key/title（iconSrc 一律剥掉）', () => {
  const w = loadShimWithDoc(HOST_SHELL);
  w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg && msg.type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS') {
      sendResponse({ items: [{ key: 'a', title: 'a', iconSrc: 'http://nas/x.png' }], titles: ['a'] });
    }
  });
  w.chrome.runtime.sendMessage({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' });
  const body = parseReportTitle(w.document.title);
  assert.equal(body.type, 'FNOS_GET_LAUNCHPAD_APP_ITEMS');
  assert.equal(body.dir, 'response');
  assert.deepEqual(body.payload, { items: [{ key: 'a', title: 'a' }], titles: ['a'] });
});

test('sendMessage：不在上报协议里的 type 一律不写标题（不给页面当任意信道用）', () => {
  const w = loadShimWithDoc(HOST_SHELL);
  for (const msg of [
    { type: 'PAGE_PRIVATE_THING', x: 1 },
    { type: 'FNOS_PAGE_STATUS', injected: true }, // brief 里凭空发明的那个 type
    'not-an-object',
    null,
  ]) {
    w.document.title = '页面自己的标题';
    w.chrome.runtime.sendMessage(msg);
    assert.equal(w.document.title, '页面自己的标题', `不该写标题：${JSON.stringify(msg)}`);
  }
});

test('sendMessage：没有宿主（载荷缺 meta）时不写标题，且绝不抛进页面', () => {
  const w = loadShimWithDoc(SHELL); // 无 meta.shellVersion = 不是本壳的载荷
  w.chrome.runtime.sendMessage(injectionTriggered());
  assert.equal(w.document.title, '页面自己的标题');
  // 连 document 都没有（宿主不在）也不能抛
  const bare = { __FNOS_SHELL__: HOST_SHELL };
  bare.window = bare;
  const fn = new Function('window', 'TextEncoder', 'queueMicrotask', 'btoa', SHIM + '\nreturn window;');
  const w2 = fn(bare, TextEncoder, queueMicrotask, (s) => Buffer.from(s, 'binary').toString('base64'));
  assert.doesNotThrow(() => w2.chrome.runtime.sendMessage(injectionTriggered()));
});

test('sendMessage：页面自己的杂项消息仍然照旧派发（上报不改动原有语义）', () => {
  const w = loadShimWithDoc(HOST_SHELL);
  const seen = [];
  w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    seen.push(msg && msg.type);
    sendResponse({ ok: true });
  });
  let got = null;
  w.chrome.runtime.sendMessage({ type: 'PAGE_PRIVATE_THING' }, (r) => { got = r; });
  assert.deepEqual(seen, ['PAGE_PRIVATE_THING']);
  assert.deepEqual(got, { ok: true });
  assert.equal(w.document.title, '页面自己的标题', '回调语义未变，但该 type 不写标题');
});

test('上报后标题会还给页面（控制标题不会一直挂在窗口上）', async () => {
  const w = loadShimWithDoc(HOST_SHELL, '页面自己的标题');
  w.chrome.runtime.sendMessage(injectionTriggered());
  assert.ok(w.document.title.startsWith('FNOSREPORT:'));
  await new Promise((r) => setTimeout(r, 700));
  assert.equal(w.document.title, '页面自己的标题');
});

test('超大上报在页面侧就不发送（宿主侧的上限是第二道闸门）', () => {
  const w = loadShimWithDoc(HOST_SHELL);
  const huge = injectionTriggered();
  huge.blob = 'x'.repeat(40000);
  w.chrome.runtime.sendMessage(huge);
  assert.equal(w.document.title, '页面自己的标题');
});

// ------------------------------------------------ Task 13b：二进制资源 / 登录壁纸 / 分片上报

const CHUNK_PREFIX = 'FNOSCHUNK:';
const REPORT_PREFIX = 'FNOSREPORT:';

/** 载入 shim，并把每一次 `document.title` 写入按顺序记下来（分片 / 单条都用它观察）。 */
function loadShimWithTitleLog(shell, title = '页面自己的标题') {
  const writes = [];
  let current = title;
  const w = { __FNOS_SHELL__: shell };
  w.window = w;
  w.document = {
    get title() { return current; },
    set title(v) { current = String(v); writes.push(current); },
  };
  const fn = new Function('window', 'TextEncoder', 'queueMicrotask', 'btoa', SHIM + '\nreturn window;');
  const win = fn(w, TextEncoder, queueMicrotask, (s) => Buffer.from(s, 'binary').toString('base64'));
  return { w: win, writes };
}

/** 上游 `collectLaunchpadAppItems()` 的项形状（content-script.js:603-607）。 */
function appItems(n) {
  return Array.from({ length: n }, (_, i) => ({
    key: `/app-center-static/serviceicon/app${i}/ui/images/icon_1.png`,
    title: `应用 ${i} · 中文标题`,
    iconSrc: '',
  }));
}

/** 把若干片 `FNOSCHUNK:` 标题拼回一条正文（并逐片校验通道上限）。 */
function reassemble(chunks) {
  const parts = [];
  let total = null;
  for (const t of chunks) {
    const payload = t.slice(CHUNK_PREFIX.length);
    const i1 = payload.indexOf(',');
    const i2 = payload.indexOf(',', i1 + 1);
    assert.ok(i1 > 0 && i2 > i1, `分片头部形状：${t.slice(0, 40)}`);
    const seq = Number(payload.slice(0, i1));
    const n = Number(payload.slice(i1 + 1, i2));
    const body = payload.slice(i2 + 1);
    assert.ok(Number.isInteger(seq) && seq >= 0, `seq=${seq}`);
    assert.ok(Number.isInteger(n) && n >= 1 && n <= 8, `total=${n}（宿主上限 8）`);
    assert.ok(seq < n, `seq=${seq} 必须小于 total=${n}`);
    assert.ok(
      Buffer.byteLength(body, 'utf8') <= 3000,
      `单片正文必须 ≤3000 字节，实为 ${Buffer.byteLength(body, 'utf8')}`
    );
    // 整条标题必须仍在 WebView2 的实测通道上限（4096 字节）之内
    assert.ok(Buffer.byteLength(t, 'utf8') <= 4096, `标题 ${Buffer.byteLength(t, 'utf8')} 字节超限`);
    // 按码点切分的证据：UTF-8 往返不产生替换字符（切在半个代理对/半个汉字上会出现 U+FFFD）
    assert.equal(Buffer.from(body, 'utf8').toString('utf8'), body, '切点必须落在码点边界上');
    if (total === null) total = n;
    else assert.equal(n, total, '每片的 total 必须一致');
    parts[seq] = body;
  }
  assert.equal(parts.filter(Boolean).length, total, '必须收到 total 片');
  return parts.join('');
}

test('binaryAssets 里的完整 data URL 原样透传（宿主按扩展名推导 mime）', () => {
  const webp = 'data:image/webp;base64,UklGRg==';
  const w = loadShim({ mods: {}, local: {}, binaryAssets: { 'wall.webp': webp } });
  assert.equal(w.chrome.runtime.getURL('wall.webp'), webp);
  // 大小写不敏感仍然成立（宿主下发的键是小写，上游可能按配置原文去取）
  assert.equal(w.chrome.runtime.getURL('WALL.WEBP'), webp);
  // 裸 base64 的兼容分支没变，且 jpg/jpeg/webp 的 mime 不再回落 text/plain
  const raw = loadShim({
    mods: {}, local: {},
    binaryAssets: { 'a.jpg': 'AAAA', 'b.jpeg': 'AAAA', 'c.webp': 'AAAA' },
  });
  assert.equal(raw.chrome.runtime.getURL('a.jpg'), 'data:image/jpeg;base64,AAAA');
  assert.equal(raw.chrome.runtime.getURL('b.jpeg'), 'data:image/jpeg;base64,AAAA');
  assert.equal(raw.chrome.runtime.getURL('c.webp'), 'data:image/webp;base64,AAAA');
});

test('登录壁纸：配置里的文件名被解析成上游要的 loginWallpaperDataUrl（cs:54/55）', async () => {
  const dataUrl = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==';
  const w = loadShim({
    mods: {},
    local: { loginWallpaperFileName: 'MyWall.PNG' },
    binaryAssets: { 'mywall.png': dataUrl },
  });
  const got = await new Promise((res) => w.chrome.storage.local.get(
    { loginWallpaperDataUrl: '', loginWallpaperFileName: '' }, res
  ));
  assert.equal(got.loginWallpaperDataUrl, dataUrl, '上游读的那个键必须有值');
  assert.equal(got.loginWallpaperFileName, 'MyWall.PNG', '文件名保持配置原文（显示值 = 配置值）');
});

test('登录壁纸：资源缺失 / 名字非法 / 没配 → 不留坏 key（与「没配壁纸」同义）', async () => {
  const read = (w) => new Promise((res) => w.chrome.storage.local.get(
    { loginWallpaperDataUrl: '', loginWallpaperFileName: '' }, res
  ));
  // 配了名字但 binaryAssets 里没有（文件被删 / 太大被跳过）
  const missing = loadShim({ mods: {}, local: { loginWallpaperFileName: 'gone.png' }, binaryAssets: {} });
  assert.equal((await read(missing)).loginWallpaperDataUrl, '');
  // 没配壁纸
  const none = loadShim({ mods: {}, local: {}, binaryAssets: {} });
  assert.equal((await read(none)).loginWallpaperDataUrl, '');
  // 空白名字
  const blank = loadShim({
    mods: {}, local: { loginWallpaperFileName: '   ' },
    binaryAssets: { 'x.png': 'data:image/png;base64,AAAA' },
  });
  assert.equal((await read(blank)).loginWallpaperDataUrl, '');
});

test('登录壁纸：__FNOS_APPLY_CONFIG__ 推来新文件名时重新解析（免刷新生效）', async () => {
  const first = 'data:image/png;base64,AAAA';
  const second = 'data:image/png;base64,BBBB';
  const w = loadShim({
    mods: {},
    local: { loginWallpaperFileName: 'a.png' },
    binaryAssets: { 'a.png': first, 'b.png': second },
  });
  const read = () => new Promise((res) => w.chrome.storage.local.get({ loginWallpaperDataUrl: '' }, res));
  assert.equal((await read()).loginWallpaperDataUrl, first);
  w.__FNOS_APPLY_CONFIG__({ local: { loginWallpaperFileName: 'b.png' } });
  assert.equal((await read()).loginWallpaperDataUrl, second);
  // 换成不存在的资源 → 键被删掉（不是留一个坏值）
  w.__FNOS_APPLY_CONFIG__({ local: { loginWallpaperFileName: 'c.png' } });
  assert.equal((await read()).loginWallpaperDataUrl, '');
});

test('小上报仍走单条通道（状态条依赖的语义逐字未变，不出现分片前缀）', () => {
  const { w, writes } = loadShimWithTitleLog(HOST_SHELL);
  w.chrome.runtime.sendMessage(injectionTriggered());
  assert.equal(writes.length, 1, `只应写一次标题：${JSON.stringify(writes.map((s) => s.slice(0, 24)))}`);
  assert.ok(writes[0].startsWith(REPORT_PREFIX), writes[0].slice(0, 40));
  assert.ok(!writes[0].startsWith(CHUNK_PREFIX));
  assert.equal(JSON.parse(writes[0].slice(REPORT_PREFIX.length)).type, 'FNOS_INJECTION_TRIGGERED');
});

test('大上报拆成 ≤3000 字节的分片（中文标题按字节切、切点落在码点边界上）', async () => {
  const { w, writes } = loadShimWithTitleLog(HOST_SHELL);
  const items = appItems(60);
  w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg && msg.type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS') {
      sendResponse({ items, titles: items.map((i) => i.title) });
    }
  });
  w.chrome.runtime.sendMessage({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' });
  await new Promise((r) => setTimeout(r, 400));

  const chunks = writes.filter((t) => t.startsWith(CHUNK_PREFIX));
  assert.ok(chunks.length >= 2, `必须真的分片，实得 ${chunks.length} 片`);
  assert.ok(chunks.length <= 8, `片数不得超过宿主上限 8，实得 ${chunks.length}`);
  const body = reassemble(chunks);
  assert.ok(Buffer.byteLength(body, 'utf8') > 4000, '被分片的正文必须真的超过单条通道');
  const envelope = JSON.parse(body);
  assert.equal(envelope.type, 'FNOS_GET_LAUNCHPAD_APP_ITEMS');
  assert.equal(envelope.dir, 'response');
  assert.equal(envelope.payload.items.length, 60);
  assert.equal(envelope.payload.items[59].title, items[59].title, '最后一项必须逐字完整（没被截断）');
  assert.equal(envelope.payload.titles.length, 60);
  // 单条路径只剩「上游自己发出的那条请求」（dir:'out'）：正文远超 4000，应答必须分片
  const singles = writes.filter((t) => t.startsWith(REPORT_PREFIX));
  assert.equal(singles.length, 1, `只该有请求本身走单条通道，实得 ${singles.length}`);
  const out = JSON.parse(singles[0].slice(REPORT_PREFIX.length));
  assert.equal(out.dir, 'out');
  assert.equal(out.type, 'FNOS_GET_LAUNCHPAD_APP_ITEMS');
  // 序列走完之后标题要还给页面
  await new Promise((r) => setTimeout(r, 600));
  assert.equal(w.document.title, '页面自己的标题');
});

test('空的应用项应答不上报（否则会把之前那份真实列表覆盖成「0 个应用」）', async () => {
  const { w, writes } = loadShimWithTitleLog(HOST_SHELL);
  w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg && msg.type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS') sendResponse({ items: [], titles: [] });
  });
  w.chrome.runtime.sendMessage({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' });
  await new Promise((r) => setTimeout(r, 200));
  // 只剩「上游自己发出的那条请求」（dir:'out'）：空列表的**应答**一个字节都不上报
  assert.equal(writes.length, 1, `空列表不该有应答上报：${writes.length} 条`);
  assert.equal(JSON.parse(writes[0].slice(REPORT_PREFIX.length)).dir, 'out');

  // 非空列表仍然照常上报（对照组）：out + response 两条
  const ok = loadShimWithTitleLog(HOST_SHELL);
  ok.w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg && msg.type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS') sendResponse({ items: appItems(1), titles: ['x'] });
  });
  ok.w.chrome.runtime.sendMessage({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' });
  await new Promise((r) => setTimeout(r, 200));
  assert.equal(ok.writes.length, 2, 'out + response');
  assert.ok(ok.writes.every((t) => t.startsWith(REPORT_PREFIX)));
  assert.equal(JSON.parse(ok.writes[1].slice(REPORT_PREFIX.length)).dir, 'response');
});

test('超过 8 片能装下的上报在页面侧就不发（宿主侧有同样的硬上限）', () => {
  const { w, writes } = loadShimWithTitleLog(HOST_SHELL);
  w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg && msg.type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS') {
      // 十几 KB 的正文（< 32KiB 早退闸门）→ 需要远超 8 片
      const big = appItems(400).map((it) => ({ ...it, iconSrc: 'x'.repeat(20) }));
      sendResponse({ items: big, titles: big.map((i) => i.title) });
    }
  });
  w.chrome.runtime.sendMessage({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' });
  assert.deepEqual(
    writes.filter((t) => t.startsWith(CHUNK_PREFIX)),
    [],
    '装不下就一片都不发（半条上报没有任何意义）'
  );
  // 只有「请求本身」那一条单条上报（dir:'out'）
  assert.equal(writes.length, 1);
  assert.equal(JSON.parse(writes[0].slice(REPORT_PREFIX.length)).dir, 'out');
});

test('应用项列表只在配置真的启用完美图标时才自动向页面要一次', async () => {
  // 没有开完美图标：一次都不问（也不该在页面里留下重试定时器）
  const off = loadShimWithTitleLog(HOST_SHELL);
  let offCalls = 0;
  const offSend = off.w.chrome.runtime.sendMessage;
  off.w.chrome.runtime.sendMessage = function (...args) { offCalls += 1; return offSend.apply(this, args); };
  await new Promise((r) => setTimeout(r, 2800));
  assert.equal(offCalls, 0, '未启用完美图标时不得向上游要应用项');

  // 开了完美图标：自动要一次，且应答经分片通道回到宿主（页面侧无需做任何事）
  const on = loadShimWithTitleLog({
    ...HOST_SHELL,
    mods: { ...HOST_SHELL.mods, launchpadIconScaleEnabled: true },
  });
  const items = appItems(60);
  on.w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg && msg.type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS') {
      sendResponse({ items, titles: items.map((i) => i.title) });
    }
  });
  await new Promise((r) => setTimeout(r, 2800));
  const chunks = on.writes.filter((t) => t.startsWith(CHUNK_PREFIX));
  assert.ok(chunks.length >= 2, `自动拉取也必须分片送出，实得 ${chunks.length} 片`);
  const envelope = JSON.parse(reassemble(chunks));
  assert.equal(envelope.payload.items.length, 60);
});

test('应用项请求的 type 必须在宿主允许表内（否则整条上报会被丢）', () => {
  // 与 report.rs::REPORT_TYPES 的契约：这里断言页面侧只发这个 type 的应用项请求
  const shim = SHIM;
  assert.ok(shim.includes("'FNOS_GET_LAUNCHPAD_APP_ITEMS'"));
  assert.ok(shim.includes('FNOSCHUNK:'));
  assert.ok(!shim.includes(".invoke("), '页面侧不得出现任何 IPC 调用（R70）');
});

test('应用项应答上报前剥掉 iconSrc（重绘后的 currentSrc 是 91KB 的 data URL，会撑爆整条通道）', async () => {
  // 运行期实测（_t13b-runtime.log 的 `send:early-len:91345`）：开了完美图标并配了重绘之后，
  // 上游 collectLaunchpadAppItems() 的 iconSrc 来自 <img>.currentSrc —— 而那个 src 已经被本壳
  // 的 applyLaunchpadRedrawIcon 换成了内置 PNG 的 data URL（单张约 91 KB）。应答因此会在
  // REPORT_MAX_CHARS（32 Ki 字符）的闸门被**静默丢掉**，设置窗永远收不到逐项列表。
  const DATA_URL = `data:image/png;base64,${'iVBORw0KGgoAAAANSUhEUg'.repeat(4200)}`;
  assert.ok(DATA_URL.length > 90000, `重绘后的 iconSrc 必须够大：${DATA_URL.length}`);
  // 60 个应用 × 每个 91 KB 的内联 data URL ≈ 5.4 MB —— 正是「完美图标 + 重绘」跑起来之后
  // 上游应答的真实量级（实测单条应答 91,345 字符就已经在 32 Ki 闸门被丢）。
  const items = Array.from({ length: 60 }, (_, i) => ({
    key: `/app-center-static/serviceicon/app${i}/ui/images/icon_1.png`,
    title: `应用 ${i} · 中文标题`,
    iconSrc: DATA_URL,
  }));
  assert.ok(JSON.stringify(items).length > 5_000_000, '原始应答必须远大于 32 Ki 字符的闸门');
  const { w, writes } = loadShimWithTitleLog(HOST_SHELL);
  let echoed = null;
  w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg && msg.type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS') {
      const resp = { items, titles: items.map((x) => x.title) };
      echoed = resp;
      sendResponse(resp);
    }
  });
  w.chrome.runtime.sendMessage({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' });
  await new Promise((r) => setTimeout(r, 400));

  const body = reassemble(writes.filter((t) => t.startsWith(CHUNK_PREFIX)));
  const envelope = JSON.parse(body);
  assert.equal(envelope.dir, 'response');
  assert.equal(envelope.payload.items.length, 60, '60 项都要在（只是瘦身，不是丢项）');
  for (const it of envelope.payload.items) {
    assert.deepEqual(Object.keys(it).sort(), ['key', 'title'], `上报项只该有 key/title：${Object.keys(it)}`);
    assert.ok(!JSON.stringify(it).includes('base64'), 'iconSrc（data URL）绝不能进上报');
  }
  assert.deepEqual(envelope.payload.titles, items.map((x) => x.title));
  // 瘦身只作用于**上报的那一份**：sendMessage 的调用者拿到的仍是上游原文（含 iconSrc）
  assert.equal(echoed.items[0].iconSrc, DATA_URL, '页面拿到的应答必须原样保留 iconSrc');
  // 5.4 MB 的正文在剥掉 iconSrc 之后必须落回分片预算（8 × 3000 = 24000 字节）内
  assert.ok(Buffer.byteLength(body, 'utf8') < 24000, `瘦身后仍超预算：${Buffer.byteLength(body, 'utf8')}`);
});

test('iconSrc 缺失/不是字符串的应用项也不会让上报变成畸形', async () => {
  const { w, writes } = loadShimWithTitleLog(HOST_SHELL);
  const items = [
    { key: '/app-center-static/serviceicon/a/ui/images/icon_1.png', title: '甲' },
    { key: '/app-center-static/serviceicon/b/ui/images/icon_1.png' },
    null,
    { title: '没有 key' },
  ];
  w.chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (msg && msg.type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS') sendResponse({ items, titles: ['x'] });
  });
  w.chrome.runtime.sendMessage({ type: 'FNOS_GET_LAUNCHPAD_APP_ITEMS' });
  await new Promise((r) => setTimeout(r, 200));
  const report = writes.filter((t) => t.startsWith(REPORT_PREFIX)).pop();
  const envelope = JSON.parse(report.slice(REPORT_PREFIX.length));
  assert.equal(envelope.dir, 'response');
  assert.deepEqual(
    envelope.payload.items,
    [
      { key: '/app-center-static/serviceicon/a/ui/images/icon_1.png', title: '甲' },
      { key: '/app-center-static/serviceicon/b/ui/images/icon_1.png', title: '' },
      { key: '', title: '没有 key' },
    ],
    '每项都要变成 {key, title} 两个字符串字段；非对象项直接丢弃（null 不许让上报炸掉）'
  );
});

