// chrome.* 兼容层的**键分区**锁（Task 14b）。
//
// 背景：设置窗的主界面自 T14b 起是上游原样的 `popup.js`，它读写的每一个存储键都必须在本壳
// 里有一个**明确**的归宿——mods 段、local 段、设置窗本地存储、宿主导入命令，或者「如实拒绝」。
// 最容易出的错是「新上游键悄悄落进某个兜底分支」：界面看起来能用，写下去的东西却没人消费。
//
// 所以这个文件不调用任何函数，而是**解析两份源码文本**做机械对账：
//   1. 从 `ui/settings/popup.js` 里取出它真实使用的键（`storage.sync.get` 的默认值对象、
//      `FONT_LOCAL_*` / `LOGIN_WALLPAPER_LOCAL_*` / `UPDATE_STATE_LOCAL_KEY` /
//      `CUSTOM_CSS_LOCAL_KEY` / `CUSTOM_JS_LOCAL_KEY` 这些常量）；
//   2. 从 `ui/settings/chrome-shim.js` 里取出分区表；
//   3. 逐条断言「popup 用到的每个键恰好被分类一次」，且两边的 sync 键集合相等。
//
// 上游升级导致键变化时，这个测试会红——那时必须回来重新分类（而不是让它静默兜底）。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8');
const POPUP_JS = read('../ui/settings/popup.js');
const SHIM = read('../ui/settings/chrome-shim.js');
const VENDORED_POPUP_JS = read('../src-tauri/assets/fnos-mods/popup.js');
const APP_JS = read('../ui/settings/app.js');

/** 从 shim 里的 `const NAME = ['a', 'b'];` 取字符串列表（解析源码，不执行）。 */
function shimList(name) {
  const m = SHIM.match(new RegExp(`const ${name} = \\[([^\\]]*)\\];`));
  assert.ok(m, `chrome-shim.js 必须有 \`const ${name} = […];\``);
  return [...m[1].matchAll(/'([^']+)'/g)].map((x) => x[1]);
}

/** popup.js 的 `chrome.storage.sync.get({...})` 默认值对象的键。 */
function popupSyncKeys() {
  const start = POPUP_JS.indexOf('chrome.storage.sync.get({');
  assert.ok(start > 0, 'popup.js 必须有一次 chrome.storage.sync.get({默认值})');
  const end = POPUP_JS.indexOf('});', start);
  assert.ok(end > start, '默认值对象的结尾必须是 `});`');
  const block = POPUP_JS.slice(start, end);
  return [...block.matchAll(/^\s{4}([A-Za-z_][A-Za-z0-9_]*):/gm)].map((m) => m[1]);
}

/** popup.js 里那几个 `const X_LOCAL_KEY = "…";` 常量的值（本地存储的键）。 */
function popupLocalKeys() {
  const keys = [];
  const re = /const ([A-Z_]*LOCAL_[A-Z_]*KEY) = "([^"]+)";/g;
  for (const m of POPUP_JS.matchAll(re)) keys.push({ name: m[1], value: m[2] });
  assert.ok(keys.length >= 6, `popup.js 里应当有 6 个以上的 *_LOCAL_*_KEY 常量，实得 ${keys.length}`);
  return keys;
}

test('分区：sync 段的键集合 == popup.js 请求的默认值键集合 == ModsConfig 的 25 键', () => {
  const requested = popupSyncKeys();
  const declared = shimList('SYNC_MODS_KEYS');
  assert.deepEqual(declared, requested, 'chrome-shim.js 的 SYNC_MODS_KEYS 必须与 popup.js 逐键同序');
  assert.equal(new Set(declared).size, declared.length, '不得有重复键');

  // 与 Rust 的 ModsConfig 字段表同源（camelCase 序列化后的键名）
  const configRs = read('../src-tauri/src/config.rs');
  const structBody = configRs.slice(
    configRs.indexOf('pub struct ModsConfig {'),
    configRs.indexOf('impl Default for ModsConfig'),
  );
  const rustFields = [...structBody.matchAll(/^\s{4}pub ([a-z0-9_]+):/gm)].map((m) => m[1]);
  const camel = rustFields.map((f) => f.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase()));
  // 两个显式 rename 的键（大小写缩写）与两个别处声明的键在这里补齐
  const expected = camel
    .map((k) => (k === 'autoEnableSuspectedFnos' ? 'autoEnableSuspectedFnOS' : k))
    .sort();
  assert.deepEqual([...declared].sort(), expected,
    `sync 键必须是 ModsConfig 的字段集（Rust 侧共 ${expected.length} 个，JS 侧 ${declared.length} 个）`);
  assert.equal(declared.length, 25, 'spec §6.2：mods 段是上游 25 键');
});

test('分区：popup.js 的每个本地存储键都恰好被分类一次（没有兜底黑箱）', () => {
  const configKeys = shimList('LOCAL_CONFIG_KEYS');
  const storeKeys = shimList('LOCAL_STORE_KEYS');
  const fontKeys = shimList('LOCAL_FONT_KEYS');

  // 壁纸两键在 shim 里是两个单值常量（它们的处理路径不是「一堆同类键」）
  const wallpaper = [
    SHIM.match(/const LOCAL_WALLPAPER_DATA_KEY = '([^']+)';/),
    SHIM.match(/const LOCAL_WALLPAPER_NAME_KEY = '([^']+)';/),
  ].map((m) => {
    assert.ok(m, 'chrome-shim.js 必须有 LOCAL_WALLPAPER_DATA_KEY / _NAME_KEY 两个常量');
    return m[1];
  });

  const classified = [...configKeys, ...storeKeys, ...fontKeys, ...wallpaper];
  assert.equal(new Set(classified).size, classified.length, '同一个键不得被分类两次');

  // popup.js 里出现的每个本地键都必须被分类（壁纸的 data 键同时是 upload-* 变量名，单独比）
  for (const { name, value } of popupLocalKeys()) {
    if (value === 'loginWallpaperDataUrl' || value === 'loginWallpaperFileName') continue;
    assert.ok(classified.includes(value), `popup.js 的 ${name} = "${value}" 没有被分类：${classified.join('、')}`);
  }
  // 壁纸两键按常量逐字比对
  const declaredWallpaper = [
    POPUP_JS.match(/const LOGIN_WALLPAPER_LOCAL_DATA_KEY = "([^"]+)";/)[1],
    POPUP_JS.match(/const LOGIN_WALLPAPER_LOCAL_NAME_KEY = "([^"]+)";/)[1],
  ];
  assert.deepEqual(wallpaper, declaredWallpaper, '壁纸两键必须与 popup.js 的常量逐字一致');
  // 字体三键必须一字不差地落在「拒绝」这一类里
  const popupFont = [
    POPUP_JS.match(/const FONT_LOCAL_DATA_KEY = "([^"]+)";/)[1],
    POPUP_JS.match(/const FONT_LOCAL_NAME_KEY = "([^"]+)";/)[1],
    POPUP_JS.match(/const FONT_LOCAL_FORMAT_KEY = "([^"]+)";/)[1],
  ];
  assert.deepEqual(fontKeys, popupFont, 'D4：字体三键必须整体落在拒绝分类里');
});

test('分区：更新检查状态进本地存储，自定义代码进 local 段（不是反过来）', () => {
  const storeKeys = shimList('LOCAL_STORE_KEYS');
  assert.deepEqual(storeKeys, [POPUP_JS.match(/const UPDATE_STATE_LOCAL_KEY = "([^"]+)";/)[1]]);

  const configKeys = shimList('LOCAL_CONFIG_KEYS');
  assert.deepEqual(configKeys, [
    POPUP_JS.match(/const CUSTOM_CSS_LOCAL_KEY = "([^"]+)";/)[1],
    POPUP_JS.match(/const CUSTOM_JS_LOCAL_KEY = "([^"]+)";/)[1],
  ]);
  // 与 Rust 的 LocalConfig 字段同名（这两个键本壳有真实字段，逐项注入真的用它们）
  const configRs = read('../src-tauri/src/config.rs');
  const structBody = configRs.slice(
    configRs.indexOf('pub struct LocalConfig {'),
    configRs.indexOf('pub struct WindowGeom'),
  );
  for (const key of configKeys) {
    const snake = key.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
    assert.ok(structBody.includes(`pub ${snake}:`), `LocalConfig 必须有 ${snake}（键 ${key}）`);
  }
});

test('分区：shim 只发 popup.js 真的会发的那三个页面消息 type', () => {
  // 只看 `type === '…'` 这种**分派**处：那才是「本层认得哪些页面消息」的事实来源。
  const dispatched = [...new Set([...SHIM.matchAll(/type === '(FNOS_[A-Z_]+)'/g)].map((m) => m[1]))].sort();
  assert.deepEqual(dispatched, [
    'FNOS_APPLY',
    'FNOS_CHECK',
    'FNOS_GET_LAUNCHPAD_APP_ITEMS',
    'FNOS_GET_LAUNCHPAD_APP_TITLES',
  ], `分派的 type 必须就是上游那三个请求 + titles 别称：${dispatched.join('、')}`);

  // shim 里的其它 FNOS_* 字面量只允许是本层自己的 postMessage 协议名（不得混进页面消息）
  const all = [...new Set([...SHIM.matchAll(/'(FNOS_[A-Z_]+)'/g)].map((m) => m[1]))].sort();
  assert.deepEqual(all, [
    'FNOS_APPLY', 'FNOS_CHECK', 'FNOS_GET_LAUNCHPAD_APP_ITEMS',
    'FNOS_GET_LAUNCHPAD_APP_TITLES', 'FNOS_SHIM_REPLY', 'FNOS_SHIM_REQUEST',
  ], `shim 里出现的 FNOS_* 只有那四个页面消息与两个本层协议名：${all.join('、')}`);

  // 三条请求必须逐字出现在上游 popup.js 里（不发明消息类型）
  for (const ty of ['FNOS_APPLY', 'FNOS_CHECK', 'FNOS_GET_LAUNCHPAD_APP_ITEMS']) {
    assert.ok(POPUP_JS.includes(`"${ty}"`), `${ty} 必须真的出现在上游 popup.js 里`);
  }
  // TITLES 是上游**同一个 if 分支**里的另一个 type（popup.js 自己不请求它），
  // 但上游 content-script.js 必须真的用那个字符串应答。
  assert.ok(!POPUP_JS.includes('FNOS_GET_LAUNCHPAD_APP_TITLES'), 'popup.js 不主动请求 titles');
  const contentScript = read('../src-tauri/assets/fnos-mods/content-script.js');
  assert.ok(contentScript.includes('FNOS_GET_LAUNCHPAD_APP_TITLES'),
    'titles 这个 type 必须来自上游 content-script.js（不是本壳发明的）');
});

test('分区：上游 popup.js 是逐字节原样的，只有 popup.html 多一行 shim 标签', () => {
  assert.equal(POPUP_JS, VENDORED_POPUP_JS, 'ui/settings/popup.js 必须与 vendored 副本逐字节相同');
  assert.ok(!POPUP_JS.includes('chrome-shim'), 'popup.js 不得被本壳改过');
  // 上游的字体导入控件在**上游自己的 HTML 里**就是注释掉的（popup.html:1180-1188），
  // 所以 D4 的拒绝是纵深防御：代码路径存在（popup.js:2161-2196），控件不在。
  const html = read('../ui/settings/popup.html');
  const vendoredHtml = read('../src-tauri/assets/fnos-mods/popup.html');
  assert.equal(html.length, vendoredHtml.length + 43, '只允许插入那一行 43 字节的 shim 标签');
  assert.ok(html.includes('<script src="./chrome-shim.js"></script>'));
  assert.ok(vendoredHtml.includes('<!-- <div class="field">\n        <p class="field-label">导入字体文件')
    || vendoredHtml.includes('导入字体文件'), '上游 popup.html 里字体导入控件是注释状态');
});

test('宿主桥：命令白名单包含 shim 真的会发的每一条，且不含未映射的命令名', () => {
  const used = [...SHIM.matchAll(/callHost\('([a-z_]+)'/g)].map((m) => m[1]);
  const unique = [...new Set(used)].sort();
  assert.deepEqual(unique, [
    'app_items', 'apply', 'get_config', 'get_local_store', 'get_page_state',
    'import_wallpaper', 'open_url', 'page_check', 'reload_main', 'set_config',
    'set_local_store',
  ], `shim 用到的宿主命令：${unique.join('、')}`);
  for (const cmd of unique) {
    assert.ok(APP_JS.includes(`${cmd}:`), `app.js 的 HOST_COMMANDS 必须映射 ${cmd}`);
  }
  // 宿主侧不许出现「把命令名原样转给 IPC」的兜底（那等于把白名单变成任意命令）
  assert.ok(!APP_JS.includes('api.invoke(data.cmd'), '不得把 iframe 给的命令名直接转发');
});

// ---------- 品牌与链接的运行时适配（T14c 修复轮 6） ----------

test('品牌与链接：本仓库是唯一事实来源，适配在运行时做（上游 HTML 不动）', () => {
  // 单一事实来源：本仓库地址只写一次，链接改指与离线应答都从它派生
  const repoLiteral = [...SHIM.matchAll(/https:\/\/github\.com\/[A-Za-z0-9._-]+\/fnos-desktop/g)]
    .map((m) => m[0]);
  assert.deepEqual([...new Set(repoLiteral)], ['https://github.com/eafenzhang/fnos-desktop'],
    '本仓库地址（含 /commits 派生的那处）必须只指向这一个仓库');
  assert.ok(SHIM.includes("const APP_REPO_URL = 'https://github.com/eafenzhang/fnos-desktop';"),
    '本仓库地址必须是具名常量（单一事实来源）');
  assert.ok(SHIM.includes("const APP_DISPLAY_NAME = 'fnOS Desktop';"),
    '界面显示名必须是具名常量');
  // 品牌适配的三件事：标题、字标替身、链接重定向
  assert.ok(SHIM.includes('document.title = APP_DISPLAY_NAME'), '文档标题必须改为本应用名');
  assert.ok(SHIM.includes("document.createElement('span')") && SHIM.includes('BRAND_ID'),
    '头部矢量字标必须有文本替身（上游字标是路径，改不了字）');
  assert.ok(SHIM.includes("querySelector('.card.header .info svg.logo')"),
    '字标替身必须锚定上游自己的 header logo 选择器');
  assert.ok(SHIM.includes('retargetUpstreamLinks'), '必须把上游仓库链接改指本仓库');
  assert.ok(SHIM.includes('attributeFilter'), '必须盯住 href 变更（上游会重写提交链接）');
  // 上游标记：适配认得的是**上游仓库前缀**，不是写死的那两条锚点
  assert.ok(SHIM.includes("const UPSTREAM_REPO_PREFIX = 'https://github.com/aurysian-yan/FnOS_UI_Mods';"),
    '必须以上游仓库前缀识别待改链接');
  // 离线更新应答的链接同样落在本仓库（上游据此重写 #latestCommitLink 的 href）
  assert.ok(SHIM.includes("'https://github.com/eafenzhang/fnos-desktop/commits';"),
    '离线应答的提交链接必须指向本仓库');
  assert.ok(!/GITHUB_COMMITS_PAGE_URL\s*=\s*\n?\s*'https:\/\/github\.com\/aurysian-yan/.test(SHIM),
    '提交链接不得再回指上游仓库');
  // app.js 的 iframe 标题（无障碍名）同步改名
  assert.ok(APP_JS.includes('设置界面（fnOS Desktop）'), 'iframe 标题必须用本应用名');
  assert.ok(!APP_JS.includes('fnOS UI Mods popup'), 'app.js 不得残留上游品牌名');
});
