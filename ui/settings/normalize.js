// 设置窗归一化的**残余文件**（T14b fix round 1 之后）。
//
// T14b 用上游 popup（iframe 托管）取代了本壳的 schema 驱动界面，`app.js` 的提交路径只剩
// `shell.*` 两键（injectEnabled / nasUrl），设置窗**没有任何 mods 归一化可做**。此前镜像
// Rust `Config::normalize` 的整套函数（`clampLightness` / `normalizeMods` /
// `normalizeModsEntry` / `isPrefectIconPath`）已按评审意见删除——「显示的值 = 生效的值」
// 现在只由两条支撑：JS 原样采纳 IPC 回包（`app.js::adoptConfig`，不做任何二次归一化）
// + Rust 是唯一的归一化实现（`config.rs::Config::normalize`，在 `load` 与每次
// `set_config` 后运行）。回归锁在 tests/settings.test.mjs 的 A 组（含源码级断言：
// app.js 不得引用本文件）。
//
// 本文件因此只剩两件事：
//
// 1. `PREFECT_ICON_PATH` —— R30 的跨语言镜像常量。它的**生产消费者**已经没有了
//    （退休的 `schema.js` 是最后一个调用判定函数的地方），但常量与它的大小写语义**保留**：
//    `ui/settings/chrome-shim.js::getURL` 依赖同一条大小写不敏感语义（内嵌资产表大小写
//    敏感，`prefect_icon/panIndex.png` 与 `prefect_icon/panindex.png` 必须解析到同一份
//    字节），而 Rust 的 `commands.rs::tests::prefect_icon_rule_mirror_stays_in_step`
//    逐字锁着这行正则文本 + tests/normalize.test.mjs 的 20 行输入表。
//    **改一侧必须同时改另一侧，否则那条 Rust 用例会红。**
// 2. `normalizeOrigin` / `parseHttpOrigin` —— 白名单条目的整理与用户输入的 origin 解析
//    （`status.js` 的白名单/状态条路径在用 `normalizeOrigin`；R23 的 ASCII 小写语义）。

/**
 * 上游 `launchpadIconRedrawMap` 取值约束的正则：`^prefect_icon/[a-z0-9-]+\.png$`，
 * **按大小写不敏感**判定——与 Rust `config.rs::is_valid_prefect_icon_path` 逐条同义
 * （R30 的镜像常量；fix round 1 才对齐大小写）。
 *
 * 为什么需要 `i`：图标资源在磁盘上是 camelCase（`src-tauri/assets/fnos-mods/prefect_icon/`
 * 下就是 `panIndex.png` 这类名字），shim 建索引时把小写化后的键当唯一键
 * （`inject/shim.js` 的 `assetIndex[String(k).toLowerCase()]`，`tests/shim.test.mjs`
 * 锁着「大小写不敏感」），因此 `prefect_icon/Emby.png` / `PREFECT_ICON/emby.PNG` 在
 * 运行期**都能解析到同一份资源**。Rust 侧放行这些值；chrome-shim 的 `getURL` 同样把
 * 整条路径折成小写再解析（`canonicalAssetPath`）。
 *
 * 放宽的**只有大小写这一维**（与 Rust 完全一致）：`..` 穿越、子目录
 * （`prefect_icon/sub/a.png`）、反斜杠（`prefect_icon\a.png`：查表键用 `/`，反斜杠
 * 永远解析不到资源）、双扩展名（`a.png.png`）、空名、空格、非 ASCII 一律照旧拒绝。
 *
 * 三个容易写错的语义细节（都选了与 Rust 同义的那一种）：
 * - `/i` **不带 `u`** 时只折 ASCII（`[a-z]` 只是多匹配 `A-Z`，不会把 `K`(U+212A) 折成
 *   `k`），与 Rust 的 `eq_ignore_ascii_case` 同一种语义；带上 `u` 反而引入 Unicode 折叠，
 *   两侧就不再同义了。
 * - `$`（不带 `m`）只匹配输入末尾，所以尾随换行**仍是拒绝**——与 Rust 的 `strip_suffix`
 *   逐字节比较一致（`prefect_icon/a.png\n` 两侧都拒）。
 * - 名称字符集不含 `.`，所以 `a.png.png` 是「主体含 `.`」而被拒（与 Rust 的判定顺序同结论）。
 *
 * 两侧的**输入表**逐行相同：Rust 侧是 `config.rs::tests::redraw_map_regex_is_case_insensitive_only`
 * （20 条断言 + `normalize()` 的 retain/幂等两条），JS 侧是 `tests/normalize.test.mjs` 的
 * `PREFECT_ICON_PATH_TABLE`；`commands.rs::tests::prefect_icon_rule_mirror_stays_in_step`
 * 还逐字锁住本常量的正则文本（含 `i`）并逐行核对那张表。
 */
export const PREFECT_ICON_PATH = /^prefect_icon\/[a-z0-9-]+\.png$/i;

/** 白名单条目：trim + **只小写 ASCII**（R23：上游按 `location.origin` 大小写敏感比较）。
 *
 * 用 `to_ascii_lowercase` 的语义（`[A-Z]` 逐个映射），不用 `String#toLowerCase()`：
 * 后者按 Unicode 折叠（`ПРИМЕР` → `пример`），而 Rust 侧是 `to_ascii_lowercase()`，
 * 非 ASCII 大写字母**保持原样**（`config.rs:584`）。白名单在状态条/外壳里的显示字符串
 * 必须与页面实际比对的字符串逐字符相同，否则手改过的条目在界面上是另一串字（§8.4 的
 * 原缺陷类别）。
 */
export function normalizeOrigin(origin) {
  return asciiLower(String(origin == null ? '' : origin).trim());
}

/** 只折 ASCII 大写字母（= Rust `str::to_ascii_lowercase`）。 */
function asciiLower(s) {
  return s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32));
}

/**
 * 用户输入的站点地址 → 规范 origin（`scheme://host[:port]`，ASCII 小写、省略默认端口）。
 *
 * **只接受 `http://` / `https://` 开头的绝对地址**，其余一律返回空串（调用方视为校验失败）。
 * 为什么不能只靠 `new URL(value).origin` + `if (!origin)`：URL 的 scheme 允许含 `.`，
 * 于是 `nas.example.com:8000` 被当成「scheme = `nas.example.com`」、`8000` 成了 opaque path，
 * `.origin` 返回**字符串 `"null"`**；`"null"` 是 truthy，旧写法拦不住，junk 会被写进
 * `config.json` 的白名单（渲染成一个永远匹配不上的条目）。`nas:8000` / `mailto:` /
 * `data:` / `javascript:` / `ftp:` 同理。
 *
 * 与 `status.js::originOfHttpUrl` 同源、区别只在**输入**：这里的输入是用户手打的
 * （任意 junk 都可能），那边来自 Rust（已是合法 URL 或已知的失败地址）。
 */
export function parseHttpOrigin(input) {
  const raw = String(input == null ? '' : input).trim();
  // 第一道闸：scheme 必须是 http(s)。正则通过不代表能解析，所以下面还要真解析一次。
  if (!/^https?:\/\//i.test(raw)) return '';
  let url;
  try {
    url = new URL(raw);
  } catch (e) {
    return '';
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return '';
  const origin = normalizeOrigin(url.origin);
  // 对 http(s) 不该出现 `"null"`；留作回归防线（这个字面量正是缺陷 B 的载体）。
  return origin === 'null' ? '' : origin;
}
