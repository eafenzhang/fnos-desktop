// Task 11：内置错误页必须是**自包含的应用资产**，且**绝不**携带 mods 载荷/CSS。
//
// 为什么静态断言有意义：错误页的整条契约就是「宿主在这里不注册初始化脚本 + 页面自己不
// 依赖任何被注入的资源」。前者由 Rust 侧的建窗路径保证（`build_error_window` 刻意不调
// `initialization_script`），后者只有这份文件本身能证明——一旦有人给它加一条
// `<link rel="stylesheet" href="basic_mod.css">` 或把载荷拼进来，「错误页不带 mods」的
// 不变量就静默失效了。运行时证据（窗口标题里的自检结论 + UIA 读到的文本）见报告。
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const HTML = readFileSync(new URL('../ui/settings/error.html', import.meta.url), 'utf8');

test('错误页在 frontendDist（ui/settings/）内，是能被 App 协议服务的资产', () => {
  // tauri.conf.json 的 frontendDist = "../ui/settings"；`tauri::WebviewUrl::App("error.html")`
  // 只能解析到该目录下的文件。放到 ui/ 根目录（brief 的字面写法）会 404。
  assert.ok(HTML.includes('__FNOS_SET_ERROR__'), '错误页必须定义宿主回调');
});

test('错误页自包含：没有外链脚本/样式（不依赖任何资产文件）', () => {
  assert.ok(!/<script[^>]+src=/i.test(HTML), '不得引用外部脚本');
  assert.ok(!/<link\b/i.test(HTML), '不得引用外部样式表');
  assert.ok(!/<img\b/i.test(HTML), '不得引用外部图片');
  assert.ok(HTML.includes('<style>'), '样式必须是内联的');
});

test('错误页不携带 mods 载荷（只读检测，不做任何赋值）', () => {
  assert.ok(!/__FNOS_SHELL__\s*=/.test(HTML), '错误页不得写入 mods 载荷');
  assert.ok(!/__FNOS_BOOTSTRAP__\s*=/.test(HTML), '错误页不得写入 bootstrap');
  assert.ok(!HTML.includes('basic_mod.css'), '错误页不得引用 mods 样式文件');
  assert.ok(!HTML.includes('mod.js'), '错误页不得引用 mods 脚本');
});

test('错误页把自己的自检结论写进 document.title（宿主的可观测通道）', () => {
  // 宿主把 document.title 镜像到窗口标题并在 stderr 打一行，验证脚本据此断言
  // 「错误页没有 mods 痕迹」这一不变量；因此标题必须由**检测结果**决定，而不是硬编码。
  assert.ok(/document\.title\s*=/.test(HTML), '错误页必须设置 document.title');
  assert.ok(HTML.includes('未注入 mods'), '标题必须区分「未注入 mods」');
  assert.ok(HTML.includes('检测到 mods 载荷'), '标题必须区分「检测到 mods 载荷」（异常）');
  assert.ok(HTML.includes("getElementById('fnos-ui-mods-"), '自检必须查真实的 mods 元素 id');
});

test('错误页把失败地址 / 原因 / 下一步 / 自动重试退避都画出来', () => {
  for (const id of ['eurl', 'ereason', 'erretry', 'ertips']) {
    assert.ok(HTML.includes(`id="${id}"`), `缺少 id=${id} 的节点`);
  }
  assert.ok(HTML.includes('重新加载主窗口'), '必须告诉用户托盘里的重试入口');
  assert.ok(HTML.includes('设置窗'), '必须告诉用户设置窗里的重试入口');
  // 退避用尽后的那一支：不谎称「还会重试」，也不谎报次数语义（failures = 连续失败次数）
  assert.ok(HTML.includes('自动重试已停止'), '退避用尽时必须明确说自动重试已停止');
  assert.ok(HTML.includes('已连续失败'), '必须按「连续失败次数」而不是「已重试次数」措辞');
});
