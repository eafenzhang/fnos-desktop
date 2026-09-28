// 设置窗顶部状态条（spec §12.3「失败模式与恢复」）。
//
// 四条硬约束：
//
// 1. **只由真实数据得出**。判据全部来自三个 IPC 回包：`get_config`（配置 + `meta`）、
//    `get_page_state`（Rust 侧观测到的主窗口 URL、是否命中白名单、上一次加载的结果）与
//    `get_page_report`（Task 13a：最近一次**页面自己**上报的上游注入链信号）。
//    这里不猜、不缓存「上次大概是什么状态」，也不去 ping 任何地址。
// 2. **弱态必须如实**。宿主能证明的只有「这个 origin 在白名单里 / 是 *.fnos.net →
//    已为当前主窗口注册了 mods 初始化脚本」。页面最终是否真的注入，取决于上游
//    `content-script.js::hasFnOSSignature()` 的判定（宿主看不到），所以**没有上报时**
//    文案只能是「注入脚本已注册」，**不是**「已注入 / 已生效」；fnOS 官网根域更是按设计
//    不注入，必须单独说清楚。
// 3. **强态必须有据**（Task 13a）。只有页面把上游 `FNOS_INJECTION_TRIGGERED`（上游在
//    `startInject()` 末尾自己发出的那条消息）经标题通道回传、且方向是「它自己发的」
//    （`dir === 'out'`）时，才升级成「已回报注入链触发」。别的形状一律不升级
//    （见 `reportVerdict`）。
// 4. 纯函数集中在本文件（Node 可单测）；只有 `statusBar()` 碰 DOM，且取不到 `#status`
//    时安静返回——`tests/settings.test.mjs` 会在无 DOM 的 Node 里 `import` app.js。
// 5. **次序**（fix round 1 / Minor 4）：宿主自己观测到的事实（加载失败/加载中/官网页/
//    注入总开关关闭）永远排在「页面声称的强态」之前——状态条任何一个分支都不许与开头
//    「注入开关：关闭」那一句互相矛盾。页面可控文本进 UI 前一律过 `clipPageText`。
//
// 归一化语义与 Rust 对齐：白名单条目 trim + **只折 ASCII 大写** + 去重（R23 / config.rs）。
import { normalizeOrigin } from './normalize.js';

/** 状态条的四种视觉态（`settings.css` 的 `.status.ok/.warn/.error/.pending`）。 */
export const STATUS_KINDS = ['pending', 'ok', 'warn', 'error'];

/**
 * 绝对 http(s) URL → origin（`scheme://host[:port]`，ASCII 小写、省略默认端口）；其余 `null`。
 *
 * 与 Rust `config::origin_of` 同义，也与 `normalize.js::parseHttpOrigin` 同源，区别只在
 * **输入**：这里的输入来自 Rust（已是合法 URL 或已知的失败地址），因此不需要「用户输入」
 * 那一路的额外提示，只需要「解析不出来就 null」。
 */
export function originOfHttpUrl(url) {
  if (typeof url !== 'string') return null;
  const raw = url.trim();
  if (!/^https?:\/\//i.test(raw)) return null;
  let parsed;
  try {
    parsed = new URL(raw);
  } catch (e) {
    return null;
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return null;
  const origin = normalizeOrigin(parsed.origin);
  return !origin || origin === 'null' ? null : origin;
}

/** 主版本号：只接受 `x[.y.z…]` 形状（`148.0.3967.54` → 148），其余 `null`（未知不谎报）。 */
export function webviewMajor(version) {
  if (typeof version !== 'string') return null;
  const t = version.trim();
  if (!/^\d+(\.\d+)*$/.test(t)) return null;
  return Number(t.split('.')[0]);
}

/** `corner-shape`（squircle 圆角）需要 Chromium/WebView2 **139+**（spec §12.3 / §14）。 */
export const CORNER_SHAPE_MIN_MAJOR = 139;

/**
 * 「关于」页的低版本提示：主版本 < 139 时返回提示文案，否则（含取不到版本）`null`。
 *
 * 取不到版本时**不提示**：`meta.webviewVersion` 为 `None` 只说明宿主没读到运行时版本，
 * 不足以断言「会退化」——宁可不说，也不给用户一条可能是假的告警。
 */
export function cornerShapeHint(version) {
  const major = webviewMajor(version);
  if (major === null || major >= CORNER_SHAPE_MIN_MAJOR) return null;
  return `当前 WebView2 ${String(version).trim()} 低于 ${CORNER_SHAPE_MIN_MAJOR}：`
    + 'squircle 圆角（CSS `corner-shape`）不受支持，形状效果会退化为普通圆角。';
}

/**
 * 把 `origin` 并入白名单（brief 的 `addCurrentOriginToWhitelist`，语义按 R23 收紧）：
 * 条目 trim + 只折 ASCII 大写 + 去重；`origin` 为空/非法时**原样返回**现有名单，
 * 调用方据此禁用「加入白名单」按钮。
 */
export function addCurrentOriginToWhitelist(origin, enabledOrigins) {
  const list = Array.isArray(enabledOrigins) ? enabledOrigins : [];
  const seen = new Set();
  const out = [];
  for (const entry of list) {
    const o = normalizeOrigin(entry);
    if (!o || seen.has(o)) continue;
    seen.add(o);
    out.push(o);
  }
  const self = normalizeOrigin(origin);
  if (self && !seen.has(self)) out.push(self);
  return out;
}

/**
 * 状态条里**页面可控文本**（目前只有 `payload.triggerReason`）的显示上限（码点）。
 *
 * 上游 `notifyInjectionTriggered(triggerReason)` 把原因放进消息，shim 原文转发，宿主只做
 * 四道闸门（长度/JSON/对象/type）——`triggerReason` 的长度可以到整条载荷的上限。
 */
export const MAX_TRIGGER_REASON_CHARS = 60;

/**
 * 页面可控文本进 UI 前的唯一一道整形：控制字符（含换行/制表）与行分隔符换成空格、连续空白
 * 折叠、按**码点**截断到 [`MAX_TRIGGER_REASON_CHARS`]。
 *
 * 为什么需要（fix round 1 的安全审计）：状态条用 `textContent` 画文本（不是 `innerHTML`，
 * 因此没有 XSS），但 `triggerReason` 仍然是**页面写的**：不设上限，一个 4 KB 的
 * `triggerReason` 就能把状态条撑成一大块；控制字符还会漏进无障碍树——UIA 读到的
 * `#status` accessible name 正是本项目的运行期评审证据，换行会把一条读数变成好几行。
 * 截断/折叠是**宿主侧的渲染**，不改变「这条上报是否算注入证据」的判定。
 */
function clipPageText(s) {
  const cleaned = String(s)
    .replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  const cps = Array.from(cleaned);
  if (cps.length <= MAX_TRIGGER_REASON_CHARS) return cleaned;
  return cps.slice(0, MAX_TRIGGER_REASON_CHARS).join('') + '…';
}

/**
 * 最近一次页面上报（`get_page_report` 回包）的判读结果：`{ injected, reason }`。
 *
 * **唯一能升级状态条的证据**：上游 `content-script.js:2678-2692` 的 `notifyInjectionTriggered()`
 * 在 `startInject()` 末尾（`:2743`）`chrome.runtime.sendMessage({type:'FNOS_INJECTION_TRIGGERED',
 * triggerReason, origin, href, timestamp})`；本壳的 shim（`inject/shim.js` 的 `sendMessage`）
 * 把这条消息**原文**放进信封的 `payload`，宿主校验 type 白名单后存内存。
 * 它比「脚本已注册」强一档：说明**上游自己的注入链确实跑到了最后一步**。
 *
 * 不升级的形状（全部按「没有上报」处理，宁可退回弱文案）：
 * - 不是对象 / `null`（没上报、被拒、换页面后作废）；
 * - `type` 不是 `FNOS_INJECTION_TRIGGERED`（例如应用项列表应答）；
 * - `dir !== 'out'`：`'response'` 是**别人问它、它作答**，不代表这次加载触发过注入；
 * - `payload` 缺失 → 仍然算注入已触发（triggerReason 只是附加信息，缺了不影响结论）。
 *
 * `reason` 会过一遍 [`clipPageText`]（页面可控文本进 UI 前的整形）。**注意**：这条通道对
 * 页面公开可写，所以「已触发」只说明某个文档**声称**注入链跑到了最后一步（Task 13a 报告
 * concerns ①：伪造者得不到任何权限，唯一影响就是状态条那句话）。
 */
export function reportVerdict(report) {
  if (!report || typeof report !== 'object') return { injected: false, reason: null };
  if (report.type !== 'FNOS_INJECTION_TRIGGERED') return { injected: false, reason: null };
  if (report.dir !== 'out') return { injected: false, reason: null };
  const payload = report.payload && typeof report.payload === 'object' ? report.payload : null;
  const raw = payload && typeof payload.triggerReason === 'string' ? clipPageText(payload.triggerReason) : '';
  return { injected: true, reason: raw || null };
}

/**
 * 状态条模型：`{ kind, text, actions, origin }`。
 *
 * `actions` 是渲染层要挂的按钮（`'retry'` / `'whitelist'`）：只给出「有真实依据」的动作
 * —— 解析不出 origin 就不给「加入白名单」，没在错误页/加载失败就不给「重试」。
 *
 * `cfg` = `get_config` 回包，`page` = `get_page_state` 回包（可以为 `null` = 还没取到），
 * `report` = `get_page_report` 回包（同样可以为 `null` = 没有可用上报）。
 */
export function statusFor(cfg, page, report) {
  if (!cfg || typeof cfg !== 'object') {
    return { kind: 'pending', text: '正在读取配置…', actions: [], origin: null };
  }
  const shell = cfg.shell && typeof cfg.shell === 'object' ? cfg.shell : {};
  const meta = cfg.meta && typeof cfg.meta === 'object' ? cfg.meta : {};
  const inject = shell.injectEnabled !== false;
  const origin = page && page.origin ? normalizeOrigin(page.origin) : null;
  const where = origin || (page && page.url) || '未知地址';
  const verdict = reportVerdict(report);

  const parts = [`配置已加载，注入开关：${inject ? '开启' : '关闭'}`];
  const actions = [];
  let kind = 'ok';

  // 分支次序（fix round 1 / Minor 4）——**先宿主自己的事实，再页面声称的强态**：
  //   1. 主窗口状态未知 / 加载失败 / 正在加载（宿主自己的观测，最高优先）；
  //   2. 官网页（上游签名正则要求前导点，官网**永远不注入**）；
  //   3. 注入总开关关闭（宿主没为任何窗口注册 mods 载荷，见 commands.rs 的 gap (a)）；
  //   4. 页面回报的「注入链已触发」（唯一致强态的证据）→ 未识别时补一句说明；
  //   5. 未识别（弱态 + 一键白名单）→ 6. 已注册注入脚本（Task 11 的弱文案）。
  // 2、3 必须排在 4 之前：否则会出现「注入开关：关闭」+「页面已回报上游注入链触发」这种
  // 自相矛盾的状态条——而标题通道对页面公开可写，这种形状恰好是页面能自己伪造的
  //（Task 13a 报告 §4.3 的第 4 个读数就是实测的伪造）。「官网页已注入」同理不可能。
  if (!page || typeof page !== 'object') {
    parts.push('主窗口状态未知');
    kind = 'warn';
  } else if (page.loadFailed) {
    parts.push(`主窗口加载失败：${page.url || '未知地址'}`);
    if (page.lastError) parts.push(String(page.lastError));
    kind = 'error';
    actions.push('retry');
  } else if (page.loading) {
    parts.push(`主窗口正在加载：${page.url || '未知地址'}`);
    kind = 'warn';
  } else if (page.officialHome) {
    // fnOS 根域：上游签名正则要求前导点（`(\.fnos\.net)$`），官网因此**永远不注入**。
    // 这一支排在 `!inject` 之前只是为了让文案更贴切（两者都与「不注入」一致，不冲突）。
    parts.push('当前是 fnOS 官网（官方站按设计不注入 mods）');
  } else if (!inject) {
    // 注入开关关着时宿主没有注册任何载荷：任何「已触发」的回报都不可能是本壳的注入链发出的。
    // 未识别的情形两件事都要说（与第 5 支一样保留「加入白名单」入口，行为与 Task 11 一致）。
    parts.push(page.recognized
      ? `页面已命中白名单，但注入开关已关闭：${where}`
      : `未检测到 fnOS WebUI，且注入开关已关闭：${where}`);
    if (!page.recognized && origin) actions.push('whitelist');
    kind = 'warn';
  } else if (verdict.injected) {
    // 页面自己的回报：上游注入链在这一页跑到了 `notifyInjectionTriggered`（最强的一条判据）。
    parts.push(`页面已回报上游注入链触发${verdict.reason ? `（triggerReason=${verdict.reason}）` : ''}：${where}`);
    if (!page.recognized) {
      // 宿主没在白名单/飞牛域名里认出这个地址，但页面自己报了注入链已触发
      //（上游还有 DOM 签名 / token / appcgi 资源三条签名路径）。两句话都必须说出来。
      parts.push('宿主未将该地址识别为 fnOS WebUI（上游按自身签名判定并注入）');
    }
  } else if (!page.recognized) {
    parts.push(`未检测到 fnOS WebUI：${where}`);
    kind = 'warn';
    if (origin) actions.push('whitelist');
  } else {
    parts.push(`注入脚本已注册：${where}`);
  }

  if (meta.recoveredFromBackup) {
    parts.push('配置文件损坏，已回退默认并保留 config.json.bak');
    if (kind === 'ok') kind = 'warn';
  }

  return { kind, text: parts.join('；'), actions, origin };
}

/**
 * 把模型画到 `#status` 上（唯一碰 DOM 的函数）。
 *
 * 只负责文本与 class；按钮由 `app.js` 挂（它才有 IPC 依赖）。取不到节点时返回 `null`
 * ——无 DOM 的单测导入 app.js 时不会因此抛错。
 */
export function statusBar(text, kind) {
  if (typeof document === 'undefined' || typeof document.getElementById !== 'function') return null;
  const el = document.getElementById('status');
  if (!el) return null;
  el.textContent = String(text == null ? '' : text);
  el.className = `status ${kind || ''}`.trim();
  return el;
}
