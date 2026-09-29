// 跨语言契约锁（Task 13b fix round 1）。
//
// 背景：本壳有两处「同一个事实被写在两种语言里」的地方，T13b 的评审各点了一条：
//   1. `MAX_WALLPAPER_BYTES` —— Rust（`config.rs` 是唯一的数值来源，`injector.rs` 只是别名）
//      与设置窗 JS（`ui/settings/app.js` 的早退副本）。JS 的那一份没有任何东西钉着 Rust 的，
//      改一边忘另一边就会出现「界面说超限、宿主照收」或反过来。
//   2. 「应用项列表」通道的 type 集合 —— `report.rs`（分流判据）、`app.js`（逐项 UI 认哪个
//      type）、`shim.js`（哪条应答值得瘦身/上报）三处。Minor 4 的成因正是两侧不一致：
//      宿主只认 ITEMS，而 UI 认 ITEMS + TITLES，于是 titles 那条应答会被塞进状态槽。
//
// 这些锁**解析源码文本**而不是调用函数：Rust 常量在测试进程里拿不到，而「读源码」恰好是
// 最直接、也最不会被重构骗过的比对方式（正则只吃固定形状的 `pub const` 声明）。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8');
const CONFIG_RS = read('../src-tauri/src/config.rs');
const INJECTOR_RS = read('../src-tauri/src/injector.rs');
const REPORT_RS = read('../src-tauri/src/report.rs');
const APP_JS = read('../ui/settings/app.js');
const SHIM_JS = read('../src-tauri/inject/shim.js');
// Task 14b：壁纸导入的界面搬到了**设置窗的 chrome.* 兼容层**里（上游 popup 提供文件控件，
// shim 负责把 data URL 交给宿主命令），所以那份「JS 副本」的上限也在那里。
const CHROME_SHIM_JS = read('../ui/settings/chrome-shim.js');

/** 上游 `content-script.js:2853-2855` 同一个分支里的两个请求类型（这份清单是事实来源）。 */
const APP_ITEM_TYPES = [
  'FNOS_GET_LAUNCHPAD_APP_ITEMS',
  'FNOS_GET_LAUNCHPAD_APP_TITLES',
];

/** 取 `pub const NAME: &str = "…";` 的字面量值。 */
function rustStrConst(src, name) {
  const m = src.match(new RegExp(`pub const ${name}: &str = "([^"]+)";`));
  assert.ok(m, `${name} 必须是一条字面量 pub const`);
  return m[1];
}

/** 把 `[A, B]` 这种「常量名列表」解析成常量值列表。 */
function rustConstList(src, listRaw) {
  return listRaw
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
    .map((name) => rustStrConst(src, name));
}

test('跨语言：壁纸大小上限（config.rs 的常量 == chrome-shim.js 的源文本）', () => {
  const m = CONFIG_RS.match(/pub const MAX_WALLPAPER_BYTES: usize = ([^;]+);/);
  assert.ok(m, 'config.rs 必须有 `pub const MAX_WALLPAPER_BYTES: usize = …;`');
  const expr = m[1].trim().replace(/_/g, '');
  // 只允许字面量乘法：不要为了「能求值」而把整段 Rust 丢进 eval
  assert.match(expr, /^[0-9*\s]+$/, `只接受字面量乘法表达式，实为 ${expr}`);
  const rustValue = Function(`"use strict";return (${expr});`)();
  assert.equal(rustValue, 8 * 1024 * 1024, 'Rust 常量本身必须是 8 MiB');
  // JS 侧那一份（`ui/settings/chrome-shim.js`）是**经典脚本**，不能 import（它一加载就要
  // 定义 window.chrome），所以这里解析源码文本——与 Rust 常量那份用同一种手法。
  const js = CHROME_SHIM_JS.match(/const MAX_WALLPAPER_BYTES = ([^;]+);/);
  assert.ok(js, 'chrome-shim.js 必须有 `const MAX_WALLPAPER_BYTES = …;`（壁纸早退闸门）');
  const jsValue = Function(`"use strict";return (${js[1].trim()});`)();
  assert.equal(jsValue, rustValue,
    'chrome-shim.js 的 MAX_WALLPAPER_BYTES 必须与 config.rs 逐位相等（改一边忘另一边 = 界面与宿主互相矛盾）');
  // injector.rs 不许再写一个独立的数值：它只能是 config 常量的别名（同一份事实只有一个来源）
  assert.match(
    INJECTOR_RS,
    /pub const MAX_WALLPAPER_BYTES: usize = config::MAX_WALLPAPER_BYTES;/,
    'injector.rs 的上限必须是 config::MAX_WALLPAPER_BYTES 的别名，不得复制字面量'
  );
});

test('跨语言：应用项通道的 type 集合（report.rs == app.js == shim.js）', () => {
  // ---- report.rs：两个 type 常量 + 分流集合
  const items = rustStrConst(REPORT_RS, 'APP_ITEMS_TYPE');
  const titles = rustStrConst(REPORT_RS, 'APP_ITEMS_TITLES_TYPE');
  const arr = REPORT_RS.match(/pub const APP_ITEMS_TYPES: \[&str; (\d+)\] = \[([^\]]+)\];/);
  assert.ok(arr, 'report.rs 必须有 `pub const APP_ITEMS_TYPES: [&str; N] = […];`');
  const declaredLen = Number(arr[1]);
  const rustList = rustConstList(REPORT_RS, arr[2]);
  assert.equal(rustList.length, declaredLen, '声明的长度必须与元素个数一致（防漏写一个）');
  assert.deepEqual(
    [...rustList].sort(),
    [...APP_ITEM_TYPES].sort(),
    '宿主的分流集合必须恰好是上游同一个 if 里的那两个 type'
  );
  assert.deepEqual([items, titles].sort(), [...APP_ITEM_TYPES].sort());

  // ---- app.js：设置窗认哪些 type（逐项 UI 的数据来源）
  const jsArr = APP_JS.match(/const APP_ITEM_REPORT_TYPES = \[([^\]]+)\];/);
  assert.ok(jsArr, 'app.js 必须有 `const APP_ITEM_REPORT_TYPES = […];`');
  const jsList = [...jsArr[1].matchAll(/'([^']+)'/g)].map((m) => m[1]);
  assert.deepEqual(
    [...jsList].sort(),
    [...rustList].sort(),
    '设置窗与宿主的分流集合必须相等（Minor 4：titles 曾经只在一侧）'
  );

  // ---- shim.js：`isAppItemsType` 的判据（瘦身 / 上报 / 「空列表不回传」都用它）
  const fn = SHIM_JS.match(/function isAppItemsType\(type\) \{([\s\S]*?)\n {2}\}/);
  assert.ok(fn, 'shim.js 必须有 isAppItemsType');
  const shimList = [...fn[1].matchAll(/'([^']+)'/g)].map((m) => m[1]);
  assert.deepEqual(
    [...shimList].sort(),
    [...rustList].sort(),
    'shim 的判据必须与宿主/设置窗一致（否则一条应答在两侧有两种解释）'
  );
  // `TITLES` 这个 type 本壳**从不主动请求**（shim 只发 ITEMS，见 `requestAppItems`）：它出现在
  // 判据里唯一的理由是「上游会用同一个形状应答它」。所以 TITLES 的字面量只允许出现在
  // `isAppItemsType` 与 `REPORT_TYPES`（宿主允许表的页面侧镜像）这两处。
  const titlesLines = SHIM_JS
    .split('\n')
    .map((line, i) => [i + 1, line.trim()])
    .filter(([, line]) => line.includes('FNOS_GET_LAUNCHPAD_APP_TITLES'))
    .filter(([, line]) => !/^FNOS_GET_LAUNCHPAD_APP_TITLES: 1,$/.test(line))
    .filter(([, line]) => !/^return type === 'FNOS_GET_LAUNCHPAD_APP_ITEMS'/.test(line));
  assert.deepEqual(
    titlesLines,
    [],
    `TITLES 只允许出现在 isAppItemsType 与 REPORT_TYPES 里：${JSON.stringify(titlesLines)}`
  );
});
