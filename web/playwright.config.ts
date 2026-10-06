import { defineConfig } from '@playwright/test';
import { mkdtempSync, chmodSync, mkdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

// 截图、下载和报告位于项目外，测试仅允许匿名生产链路fixture。
const output = process.env.CODEPERIMETER_BROWSER_OUTPUT ?? mkdtempSync(join(tmpdir(), 'codeperimeter-browser-output-'));
const projectRoot = resolve('..');
if (resolve(output) === projectRoot || resolve(output).startsWith(projectRoot + '/')) throw new Error('浏览器测试产物不得保存在项目中。');
mkdirSync(output, { recursive: true, mode: 0o700 });
chmodSync(output, 0o700);
export default defineConfig({
  testDir: './tests',
  testMatch: '**/browser.spec.ts',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120000,
  expect: { timeout: 10000 },
  reporter: [['list']],
  outputDir: output,
  use: { channel: 'chrome', headless: false, reducedMotion: 'reduce', viewport: { width: 1440, height: 960 }, screenshot: 'off', trace: 'off', video: 'off', acceptDownloads: true },
});
