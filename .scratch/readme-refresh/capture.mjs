// 仅从项目外的匿名浏览器夹具生成公开截图，不连接用户生产入口。
import { chromium } from '../../web/node_modules/@playwright/test/index.mjs';
import { readFileSync, statSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const directory = resolve(process.argv[2] ?? '');
if (!directory.startsWith('/private/tmp/codeperimeter-readme-capture-')) throw new Error('需要独立匿名夹具。');
const metadata = join(directory, 'browser.json');
if ((statSync(directory).mode & 0o077) || (statSync(metadata).mode & 0o077)) throw new Error('夹具权限不足。');
const fixture = JSON.parse(readFileSync(metadata, 'utf8'));
if (!fixture.root.startsWith('/private/tmp/codeperimeter-web-fixture-') || !/^http:\/\/127\.0\.0\.1:\d+$/.test(fixture.session.origin)) throw new Error('拒绝非匿名入口。');
const output = join(root, 'docs/images');
mkdirSync(output, { recursive: true });
const replacements = [
  [fixture.claude_project, '/workspace/demo-api'],
  [fixture.zcode_project, '/workspace/demo-cli'],
  [fixture.project, '/workspace/atlas-app'],
  [fixture.root, '/workspace/demo'],
  ['/usr/bin/synthetic-reader', '/usr/bin/demo-worker'],
];
function anonymous(value) {
  if (typeof value === 'string') {
    for (const [from, to] of replacements) value = value.replaceAll(from, to);
    return value.replace(/source-(\d+)\.txt/g, 'src/module-$1.ts');
  }
  if (Array.isArray(value)) return value.map(anonymous);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, anonymous(item)]));
  return value;
}
const browser = await chromium.launch({ channel: 'chrome', headless: false });
let screenshots = 0;
try {
  for (const locale of ['en', 'zh-CN']) {
    const context = await browser.newContext({ locale: locale === 'en' ? 'en-US' : 'zh-CN', viewport: { width: 1600, height: 1120 }, deviceScaleFactor: 1, reducedMotion: 'reduce' });
    const page = await context.newPage();
    await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== fixture.session.origin) throw new Error('拒绝外部请求。');
    const response = await route.fetch();
    const body = anonymous(await response.json());
    if (url.pathname === '/api/console' && route.request().postDataJSON()?.action === 'summary' && body.ok) {
      // 时间序列仅供页面展示，明确是合成演示，不作为性能或采集结果。
      const end = Date.UTC(2026, 9, 7, 4);
      const values = [4, 7, 5, 12, 28, 17, 8, 46, 35, 18, 62, 26, 11, 20, 13, 8];
      body.data.trend = values.map((open, index) => ({ timestamp_ms: end - (15 - index) * 3600000, open, mmap: Math.floor(open / 4), archive: 0 }));
    }
    await route.fulfill({ response, json: body });
    });
    await page.addInitScript(() => localStorage.setItem('codeperimeter-theme', 'light'));
    await page.request.post(`${fixture.session.origin}/api/language`, { headers: { Authorization: `Bearer ${fixture.session.token}`, Origin: fixture.session.origin }, data: { preference: locale } });
    try { await page.goto(`${fixture.session.origin}/#token=${encodeURIComponent(fixture.session.token)}`); }
    catch { throw new Error('无法打开匿名截图入口。'); }
    const zh = locale === 'zh-CN';
    await page.getByRole('heading', { name: zh ? '监控概览' : 'Monitoring overview', exact: true }).waitFor();
    await page.getByRole('table', { name: zh ? '窗口内频繁访问文件' : 'Frequently accessed files in window' }).waitFor();
    await page.evaluate(() => document.fonts.ready);
    await capture('overview');
    const nav = page.getByRole('navigation', { name: zh ? '主要导航' : 'Main navigation' });
    await nav.getByRole('button', { name: zh ? '告警中心' : 'Alert center', exact: true }).click();
    const table = page.getByRole('table', { name: zh ? '告警记录' : 'Alert records' });
    await table.getByRole('button', { name: zh ? '查看' : 'View', exact: true }).first().click();
    await page.getByRole('button', { name: zh ? '关闭' : 'Close', exact: true }).click();
    await capture('alert-center');
    await nav.getByRole('button', { name: zh ? '规则中心' : 'Rule center', exact: true }).click();
    await page.getByRole('heading', { name: zh ? '三类内置规则' : 'Three built-in rules', exact: true }).waitFor();
    await capture('rules');
    async function capture(name) {
      await page.mouse.move(900, 80);
      await page.evaluate(() => document.fonts.ready);
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      const text = await page.locator('body').innerText();
      if (/(?:\/Users\/|\/private\/|\/var\/folders\/|@|#token=)/i.test(text)) throw new Error('截图包含未匿名化信息，拒绝保存。');
      await page.screenshot({ path: join(output, `console-${name}-${locale}.png`), fullPage: false });
      screenshots += 1;
    }
    await context.close();
  }
  console.log(JSON.stringify({ screenshots, data: 'synthetic', production_service_started: false }));
} finally { await browser.close(); }
