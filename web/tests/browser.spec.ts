import { test, expect, type Page } from '@playwright/test';
import { readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';

interface Fixture { session: { origin: string; token: string }; project: string; claude_project: string; zcode_project: string; root: string; host_socket: string }
function fixture(): { directory: string; data: Fixture } {
  const directory = process.env.CODEPERIMETER_BROWSER_FIXTURE;
  if (!directory) throw new Error('未指定项目外私有浏览器fixture。');
  const location = resolve(directory);
  const projectRoot = resolve('..');
  if (location === projectRoot || location.startsWith(projectRoot + '/')) throw new Error('浏览器fixture不得位于项目中。');
  const metadata = statSync(location), file = statSync(join(location, 'browser.json'));
  if ((metadata.mode & 0o077) !== 0 || (file.mode & 0o077) !== 0) throw new Error('浏览器fixture权限需要限制为当前用户。');
  const data = JSON.parse(readFileSync(join(location, 'browser.json'), 'utf8')) as Fixture;
  if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(data.session.origin)) throw new Error('浏览器测试只接受本机回环入口。');
  return { directory: location, data };
}
async function open(page: Page, data: Fixture) {
  const language = await page.request.post(`${data.session.origin}/api/language`, { headers: { Authorization: `Bearer ${data.session.token}`, Origin: data.session.origin }, data: { preference: 'zh-CN' } });
  expect(language.ok(), '匿名验收明确使用简体中文，不依赖本机首选语言').toBeTruthy();
  try { await page.goto(`${data.session.origin}/#token=${encodeURIComponent(data.session.token)}`); }
  catch { throw new Error('无法打开匿名本机控制台。'); }
  await expect(page.getByRole('heading', { name: '监控概览', exact: true })).toBeVisible();
  expect(new URL(page.url()).hash.length === 0, '认证fragment已经移除').toBeTruthy();
}
async function navigate(page: Page, name: string) { await page.getByRole('navigation', { name: '主要导航' }).getByRole('button', { name, exact: true }).click(); }
async function chooseTheme(page: Page, name: string) {
  await page.getByRole('button', { name: /^外观：/ }).click();
  await page.getByRole('menuitemradio', { name, exact: true }).click();
}

test('真实HTTP、宿主和SQLite完成七页操作闭环', async ({ page }, info) => {
  const { directory, data } = fixture();
  writeFileSync(join(directory, 'inject'), '', { mode: 0o600 });
  const pageErrors: string[] = [];
  page.on('pageerror', () => pageErrors.push('页面运行异常'));
  await open(page, data);
  await expect(page.getByText('窗口内文件活动', { exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('heading', { name: '监控概览', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: '请重新打开控制台' })).toHaveCount(0);

  await navigate(page, '告警中心');
  const alertTable = page.getByRole('table', { name: '告警记录' });
  await expect(alertTable.getByRole('button', { name: '查看', exact: true }).first()).toBeVisible();
  await alertTable.getByRole('button', { name: '查看', exact: true }).first().click();
  const detail = page.getByRole('complementary', { name: '记录详情' });
  await expect(detail.getByText('批量文件访问', { exact: true })).toBeVisible();
  await page.screenshot({ path: info.outputPath('alert-detail-light.png'), fullPage: true });
  await detail.getByLabel('处理备注', { exact: true }).fill('合成验收备注');
  await detail.getByRole('button', { name: '标记已处理', exact: true }).click();
  await expect(detail.getByRole('button', { name: '重新打开', exact: true })).toBeVisible();
  await detail.getByRole('button', { name: '标记已读', exact: true }).click();
  await expect(detail.getByRole('button', { name: '标记未读', exact: true })).toBeVisible();
  const database = new DatabaseSync(join(data.root, 'events.sqlite'), { readOnly: true });
  const statement = database.prepare('SELECT alert_id, revision, is_read, processed FROM alert_metadata WHERE processed=1 AND is_read=1 ORDER BY revision DESC LIMIT 1');
  const baseline = statement.get();
  if (!baseline) throw new Error('匿名告警处理尚未持久化。');
  const observe = database.prepare('SELECT revision,is_read,processed FROM alert_metadata WHERE alert_id=?');
  let lastUnchangedAt = performance.now();
  let persistedLowerBound: number | undefined;
  let observedAt: number | undefined;
  const timer = setInterval(() => {
    const started = performance.now();
    const row = observe.get(baseline.alert_id as string);
    if (row && Number(row.revision) > Number(baseline.revision) && !row.is_read && !row.processed && persistedLowerBound === undefined) { persistedLowerBound = lastUnchangedAt; observedAt = started; }
    else if (persistedLowerBound === undefined) lastUnchangedAt = started;
  }, 5);
  try {
    writeFileSync(join(directory, 'inject'), '', { mode: 0o600 });
    await expect(detail.getByRole('button', { name: '标记已处理', exact: true })).toBeVisible();
    await expect(detail.getByRole('button', { name: '标记已读', exact: true })).toBeVisible();
    const visibleAt = performance.now();
    if (persistedLowerBound === undefined || observedAt === undefined) throw new Error('未观察到新的持久化证据。');
    const upper = Math.ceil(visibleAt - persistedLowerBound);
    writeFileSync(join(directory, 'browser-latency.json'), JSON.stringify({ persist_to_visible_upper_ms: upper, persistence_observation_interval_ms: Math.ceil(observedAt - persistedLowerBound), goal_ms: 1000 }), { mode: 0o600 });
    expect(upper <= 1000, '已保存的新证据在前台1秒内可见').toBeTruthy();
  } finally { clearInterval(timer); database.close(); }
  await expect(detail.getByLabel('处理备注', { exact: true })).toHaveValue('合成验收备注');
  await detail.getByRole('button', { name: '关闭', exact: true }).click();

  await navigate(page, '文件活动');
  const eventPage = page.getByRole('region', { name: '文件活动', exact: true });
  const events = page.getByRole('table', { name: '文件活动记录' });
  await expect(events.getByRole('row')).toHaveCount(26);
  const firstPage = await events.getByRole('row').nth(1).innerText();
  await page.getByRole('button', { name: '下一页', exact: true }).click();
  await expect(eventPage.getByText(/第 2 页/)).toBeVisible();
  await expect.poll(async () => (await events.getByRole('row').nth(1).innerText()) !== firstPage, { message: '第二页使用真实不同记录' }).toBeTruthy();
  await page.getByRole('button', { name: '上一页', exact: true }).click();
  await expect(eventPage.getByText(/第 1 页/)).toBeVisible();
  await eventPage.getByLabel('搜索文件或进程', { exact: true }).fill('source-001.txt');
  await eventPage.getByRole('button', { name: '应用筛选', exact: true }).click();
  const filterButton = await eventPage.getByRole('button', { name: '应用筛选', exact: true }).boundingBox();
  expect(Boolean(filterButton && filterButton.height >= 32), '正式组件保留可操作的点击尺寸').toBeTruthy();
  await expect.poll(async () => (await events.getByRole('row').count()) > 1).toBeTruthy();
  await expect(events.getByRole('row').nth(1).getByText(/source-001\.txt/)).toBeVisible();
  await eventPage.getByRole('button', { name: '导出', exact: true }).click();
  const exportDialog = page.getByRole('dialog', { name: '导出当前筛选范围' });
  await expect(exportDialog).toBeVisible();
  await page.screenshot({ path: info.outputPath('export-dialog-light.png') });
  await page.keyboard.press('Tab');
  await expect.poll(async () => exportDialog.evaluate(dialog => dialog.contains(document.activeElement)), { message: '弹窗中的键盘焦点保留在操作区域' }).toBeTruthy();
  await page.keyboard.press('Escape');
  await expect(exportDialog).not.toBeVisible();
  await eventPage.getByRole('button', { name: '导出', exact: true }).click();
  const downloadWait = page.waitForEvent('download');
  await exportDialog.getByRole('button', { name: '下载', exact: true }).click();
  const download = await downloadWait;
  const downloadPath = info.outputPath('filtered-anonymous.json');
  await download.saveAs(downloadPath);
  const exported = readFileSync(downloadPath, 'utf8');
  expect(!exported.includes(data.project), '匿名导出移除项目身份').toBeTruthy();
  expect(!exported.includes('合成验收备注'), '匿名导出移除自由文本').toBeTruthy();
  await expect(exportDialog).not.toBeVisible();
  await eventPage.getByRole('button', { name: '导出', exact: true }).click();
  await exportDialog.getByLabel('文件格式', { exact: true }).selectOption('csv');
  const csvWait = page.waitForEvent('download');
  await exportDialog.getByRole('button', { name: '下载', exact: true }).click();
  const csv = await csvWait;
  const csvPath = info.outputPath('filtered-anonymous.csv');
  await csv.saveAs(csvPath);
  const csvContent = readFileSync(csvPath, 'utf8');
  expect(csvContent.trim().split('\n').length > 1 && !csvContent.includes(data.project), 'CSV下载包含数据并替换项目身份').toBeTruthy();
  await expect(exportDialog).not.toBeVisible();
  await eventPage.getByRole('button', { name: '重置', exact: true }).click();
  await expect(events.getByRole('row')).toHaveCount(26);

  await navigate(page, '监控目录');
  const directories = page.getByRole('table', { name: '监控目录配置' });
  await expect(directories.getByRole('row')).toHaveCount(2);
  await directories.getByRole('button', { name: '停用', exact: true }).click();
  await expect(directories.getByText('停用并排除子树')).toBeVisible();
  await directories.getByRole('button', { name: '启用', exact: true }).click();
  await expect(directories.getByRole('button', { name: '停用', exact: true })).toBeVisible();
  await page.getByRole('button', { name: '预览候选', exact: true }).click();
  const candidates = page.getByRole('table', { name: '历史项目目录候选' });
  await expect(candidates.getByRole('row')).toHaveCount(4);
  for (const source of ['Codex', 'Claude Code', 'ZCode']) {
    await expect(candidates.getByText(source, { exact: true })).toBeVisible();
  }
  await page.getByRole('button', { name: '选择可用候选', exact: true }).click();
  await page.getByRole('button', { name: /导入选中 \d+ 项/ }).click();
  await expect(page.getByText('选中的可用目录已导入。')).toBeVisible();
  await expect(directories.getByRole('row')).toHaveCount(4);
  const configuration = new DatabaseSync(join(data.root, 'events.sqlite'), { readOnly: true });
  try {
    for (const [path, source] of [[data.project, 'codex'], [data.claude_project, 'claude_code'], [data.zcode_project, 'zcode']]) {
      const stored = configuration.prepare('SELECT COUNT(*) AS count FROM monitored_directories d JOIN directory_sources s ON d.path=s.directory_path WHERE d.path=? AND s.source=?').get(path, source);
      expect(Number(stored?.count), '三种选中历史目录及来源都已保存到权威宿主').toBe(1);
    }
  } finally { configuration.close(); }

  await navigate(page, '规则中心');
  const threshold = page.getByLabel('批量阈值（不同文件数）', { exact: true });
  await expect(threshold).toHaveValue('50');
  await threshold.fill('513');
  await page.getByRole('button', { name: '保存规则', exact: true }).click();
  await expect(page.getByText('批量阈值必须为 1 至 512 的整数。')).toBeVisible();
  await threshold.fill('51');
  await page.getByRole('button', { name: '保存规则', exact: true }).click();
  await expect(page.getByText('规则已保存，对后续事件即时生效。')).toBeVisible();
  await page.reload();
  await navigate(page, '规则中心');
  await expect(page.getByLabel('批量阈值（不同文件数）', { exact: true })).toHaveValue('51');

  await navigate(page, '归档迹象');
  await expect(page.getByRole('heading', { name: '默认识别的归档命令', exact: true })).toBeVisible();
  await expect(page.getByText(/tar、bsdtar、zip、gtar、ditto、gzip、pigz/)).toBeVisible();
  await expect(page.getByRole('heading', { name: '暂无匹配活动' })).toBeVisible();

  await navigate(page, '设置与诊断');
  await expect(page.getByRole('heading', { name: '服务与系统授权', exact: true })).toBeVisible();
  const retention = page.getByLabel('明细保留天数', { exact: true });
  await expect(retention).toHaveValue('30');
  await retention.fill('1');
  await page.getByRole('button', { name: '保存保留期', exact: true }).click();
  const retentionDialog = page.getByRole('dialog', { name: '缩短明细保留期' });
  await expect(retentionDialog).toBeVisible();
  await page.screenshot({ path: info.outputPath('retention-dialog-light.png') });
  await retentionDialog.getByRole('button', { name: '确认', exact: true }).click();
  await expect(retentionDialog).not.toBeVisible();
  await expect(page.getByText('明细保留期已保存。')).toBeVisible();
  await chooseTheme(page, '深色');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.screenshot({ path: info.outputPath('settings-dark.png'), fullPage: true });
  await chooseTheme(page, '浅色');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await navigate(page, '概览');
  await expect.poll(async () => await page.locator('.metric-band > div').first().locator('strong').innerText() !== '未知').toBeTruthy();
  await page.screenshot({ path: info.outputPath('overview-light.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole('heading', { name: '监控概览', exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), '窄屏布局不横向溢出').toBeTruthy();
  await page.setViewportSize({ width: 1440, height: 960 });

  await navigate(page, '设置与诊断');
  await page.getByRole('button', { name: '清除明细', exact: true }).click();
  const clearDialog = page.getByRole('dialog', { name: '清除明细记录' });
  await expect(clearDialog).toBeVisible();
  await clearDialog.getByRole('button', { name: '取消', exact: true }).click();
  await expect(clearDialog).not.toBeVisible();
  await page.getByRole('button', { name: '清除明细', exact: true }).click();
  await clearDialog.getByRole('button', { name: '确认', exact: true }).click();
  await expect(clearDialog).not.toBeVisible();
  await navigate(page, '文件活动');
  await expect(page.getByRole('heading', { name: '暂无匹配活动' })).toBeVisible();
  expect(pageErrors.length === 0, '没有页面运行异常').toBeTruthy();
});

test('七页双主题与四档窗口没有页面溢出，外观菜单及规则开关支持键盘', async ({ page }, info) => {
  const { directory, data } = fixture();
  const headers = { Authorization: `Bearer ${data.session.token}`, Origin: data.session.origin };
  const current = await page.request.post(`${data.session.origin}/api/console`, { headers, data: { action: 'rules_get' } });
  const settings = (await current.json()).data;
  const saved = await page.request.post(`${data.session.origin}/api/console`, { headers, data: { action: 'rules_set', payload: { settings: { ...settings, bulk_enabled: true, bulk_file_threshold: 50 } } } });
  expect((await saved.json()).ok, '布局验收恢复标准规则并注入合成记录').toBeTruthy();
  writeFileSync(join(directory, 'inject'), '', { mode: 0o600 });
  const errors: string[] = [];
  page.on('pageerror', () => errors.push('页面运行异常'));
  await open(page, data);
  await navigate(page, '文件活动');
  await expect(page.getByRole('table', { name: '文件活动记录' }).getByRole('button', { name: '查看', exact: true }).first()).toBeVisible();
  const scrollArea = page.getByRole('region', { name: '文件活动记录，可滚动表格', exact: true });
  await scrollArea.focus();
  await page.keyboard.press('ArrowDown');
  await expect.poll(() => scrollArea.evaluate(element => element.scrollTop), { message: '记录表格可以用键盘滚动查看' }).toBeGreaterThan(0);
  await expect(page.getByText('仅本机访问', { exact: true })).toHaveCount(0);
  const appearance = page.getByRole('button', { name: /^外观：/ });
  await appearance.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('menuitemradio', { name: '跟随系统', exact: true })).toHaveAttribute('aria-checked', 'true');
  await page.getByRole('menuitemradio', { name: '浅色', exact: true }).focus();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitemradio', { name: '深色', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(appearance).toBeFocused();
  await page.reload();
  await expect(appearance).toHaveAccessibleName('外观：深色');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await chooseTheme(page, '跟随系统');
  await page.emulateMedia({ colorScheme: 'dark' });
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.emulateMedia({ colorScheme: 'light' });
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');

  const pages = ['概览', '监控目录', '文件活动', '归档迹象', '告警中心', '规则中心', '设置与诊断'];
  for (const theme of ['浅色', '深色']) {
    await chooseTheme(page, theme);
    for (const width of [1920, 1440, 1024, 720]) {
      await page.setViewportSize({ width, height: 960 });
      for (const [index, name] of pages.entries()) {
        await navigate(page, name);
        await expect(page.locator('.page-content > section:not([hidden]) .loading')).toHaveCount(0);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), `${name}／${theme}／${width}px 没有页面横向溢出`).toBeTruthy();
        if (name === '规则中心') {
          await expect(page.getByRole('switch')).toHaveCount(3);
          expect(await page.locator('.rule-row').evaluateAll(rows => rows.every(row => {
            const bounds = row.getBoundingClientRect();
            const control = row.querySelector('.cds--toggle')!.getBoundingClientRect();
            return control.right <= bounds.right + 1 && row.scrollWidth <= row.clientWidth + 1;
          })), '规则开关与状态文字保留在各自规则行内').toBeTruthy();
        }
        if (name === '概览') {
          const chart = page.getByRole('img', { name: /^文件活动趋势/ });
          await expect(chart).toBeVisible();
          expect((await chart.boundingBox())!.height <= 240, '趋势图高度不随窗口宽度膨胀').toBeTruthy();
          expect((await page.getByLabel('统计范围', { exact: true }).boundingBox())!.width <= 160, '时间选择器保持紧凑').toBeTruthy();
        }
        if (width === 1440 || name === '规则中心') await page.screenshot({ path: info.outputPath(`page-${index}-${theme === '浅色' ? 'light' : 'dark'}-${width}.png`), fullPage: true });
      }
      await navigate(page, '文件活动');
      await page.getByRole('table', { name: '文件活动记录' }).getByRole('button', { name: '查看', exact: true }).first().click();
      const detail = page.getByRole('complementary', { name: '记录详情' });
      await expect(detail.getByRole('button', { name: '关闭', exact: true })).toBeVisible();
      expect(await detail.evaluate(element => element.scrollWidth <= element.clientWidth + 1), '详情字段与长路径不横向溢出').toBeTruthy();
      await detail.getByRole('button', { name: '关闭', exact: true }).click();
      await page.getByRole('button', { name: '导出', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: '导出当前筛选范围' });
      await expect(dialog).toBeVisible();
      const bounds = (await dialog.boundingBox())!;
      expect(bounds.x >= 0 && bounds.x + bounds.width <= width, '确认弹窗完整保留在当前窗口内').toBeTruthy();
      if (width === 1440) await page.screenshot({ path: info.outputPath(`export-dialog-${theme === '浅色' ? 'light' : 'dark'}.png`) });
      await dialog.getByRole('button', { name: '取消', exact: true }).click();
    }
  }
  await navigate(page, '规则中心');
  const toggle = page.getByRole('switch', { name: '批量文件访问开关', exact: true });
  await expect(toggle).toBeChecked();
  await toggle.focus();
  await page.keyboard.press('Space');
  await expect(toggle).not.toBeChecked();
  await page.keyboard.press('Space');
  await expect(toggle).toBeChecked();
  const track = page.locator('.rule-row').filter({ has: toggle }).locator('.cds--toggle__switch');
  await track.click();
  await expect(toggle).not.toBeChecked();
  await track.click();
  await expect(toggle).toBeChecked();
  expect(errors).toEqual([]);
});

test('无入口令牌明确展示恢复路径，不触发未授权的业务请求', async ({ page }) => {
  const { data } = fixture();
  try { await page.goto(data.session.origin); } catch { throw new Error('无法打开匿名本机控制台。'); }
  await expect(page.getByRole('heading', { name: '请重新打开控制台', exact: true })).toBeVisible();
  await expect(page.getByText('codeperimeter ui', { exact: true })).toBeVisible();
});

test('通知深链接打开指定告警，刷新与过期记录均有明确结果', async ({ page }) => {
  const { directory, data } = fixture();
  const headers = { Authorization: `Bearer ${data.session.token}`, Origin: data.session.origin };
  const current = await page.request.post(`${data.session.origin}/api/console`, { headers, data: { action: 'rules_get' } });
  const settings = (await current.json()).data;
  const saved = await page.request.post(`${data.session.origin}/api/console`, { headers, data: { action: 'rules_set', payload: { settings: { ...settings, bulk_enabled: true, bulk_file_threshold: 50 } } } });
  expect((await saved.json()).ok, '匿名通知测试恢复标准批量门槛').toBeTruthy();
  const database = new DatabaseSync(join(data.root, 'events.sqlite'), { readOnly: true });
  try {
    const before = Number(database.prepare('SELECT COUNT(*) AS count FROM events').get()?.count);
    writeFileSync(join(directory, 'inject'), '', { mode: 0o600 });
    await expect.poll(() => Number(database.prepare('SELECT COUNT(*) AS count FROM events').get()?.count), { message: '等待本轮合成事件保存' }).toBe(before + 50);
    const query = database.prepare('SELECT id,alert_json FROM alerts ORDER BY last_timestamp_ms DESC LIMIT 1');
    await expect.poll(() => Boolean(query.get()), { message: '等待合成告警保存' }).toBeTruthy();
    const alert = query.get()!;
    const id = String(alert.id);
    const record = JSON.parse(String(alert.alert_json));
    const metadata = database.prepare('SELECT is_read,processed FROM alert_metadata WHERE alert_id=?');
    const baseline = metadata.get(id);
    try { await page.goto(`${data.session.origin}/?alert=${encodeURIComponent(id)}#token=${encodeURIComponent(data.session.token)}`); }
    catch { throw new Error('无法打开匿名通知详情入口。'); }
    await page.bringToFront();
    await expect(page.getByRole('heading', { name: '告警中心', exact: true })).toBeVisible();
    const detail = page.getByRole('complementary', { name: '记录详情' });
    await expect(detail.getByLabel('处理备注', { exact: true })).toBeVisible();
    await expect(detail.getByText(new RegExp(`\\(${record.process.pid}\\)`))).toBeVisible();
    expect(new URL(page.url()).hash.length === 0, '入口令牌已从地址栏移除').toBeTruthy();
    expect(metadata.get(id)).toEqual(baseline);
    const foreground = await page.context().newPage();
    try {
      const loaded = page.waitForResponse(response => response.url().endsWith('/api/console') && response.request().postDataJSON()?.action === 'alert_detail');
      await page.reload();
      expect((await loaded).status(), '后台标签页首次加载仍读取详情').toBe(200);
    } finally { await foreground.close(); await page.bringToFront(); }
    await expect(detail.getByLabel('处理备注', { exact: true })).toBeVisible();
    await detail.getByRole('button', { name: '关闭', exact: true }).click();
    await expect(detail).not.toBeVisible();
    await page.goBack();
    await expect(detail.getByLabel('处理备注', { exact: true })).toBeVisible();
    await page.goto(`${data.session.origin}/?alert=synthetic-expired`);
    await expect(page.getByText('这条告警明细暂不可用，可能已过期、被清除或尚未保存。请关闭详情查看告警列表。')).toBeVisible();
    await detail.getByRole('button', { name: '关闭', exact: true }).click();
    await expect(page.getByRole('table', { name: '告警记录' })).toBeVisible();
    await page.goto(`${data.session.origin}/?alert=${encodeURIComponent('../other')}`);
    await expect(page.getByText('通知中的告警入口无效，请在告警中心查找记录。')).toBeVisible();
  } finally { database.close(); }
});

async function chooseLanguage(page: Page, name: string) {
  await page.getByRole('button', { name: /^(语言：|Language:)/ }).click();
  await page.getByRole('menuitemradio', { name, exact: true }).click();
}

test('中英文覆盖七页、详情弹窗、旧错误、偏好同步和英文长文本排版', async ({ page }, info) => {
  test.setTimeout(240000);
  const { directory, data } = fixture();
  const headers = { Authorization: `Bearer ${data.session.token}`, Origin: data.session.origin };
  writeFileSync(join(directory, 'inject'), '', { mode: 0o600 });
  const errors: string[] = [];
  page.on('pageerror', () => errors.push('页面运行异常'));
  await open(page, data);
  await navigate(page, '监控目录');
  await page.getByLabel('目录完整路径', { exact: true }).fill('synthetic-relative-path');
  await page.getByRole('button', { name: '添加', exact: true }).click();
  await expect(page.getByText('请输入以 / 开头的完整目录路径。', { exact: true }).filter({ visible: true })).toBeVisible();
  await chooseLanguage(page, 'English');
  await expect(page.getByRole('heading', { name: 'Monitored directories', exact: true })).toBeVisible();
  await expect(page.getByText('Enter a full directory path starting with /.', { exact: true }).filter({ visible: true })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Monitoring overview', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Language: English', exact: true })).toBeVisible();

  const second = await page.context().newPage();
  try {
    await second.goto(`${data.session.origin}/#token=${encodeURIComponent(data.session.token)}`);
    await expect(second.getByRole('heading', { name: 'Monitoring overview', exact: true })).toBeVisible();
    await chooseLanguage(page, '简体中文');
    await second.bringToFront();
    await expect(second.getByRole('heading', { name: '监控概览', exact: true })).toBeVisible();
    await chooseLanguage(second, 'English');
    await page.bringToFront();
    await expect(page.getByRole('heading', { name: 'Monitoring overview', exact: true })).toBeVisible();
  } finally { await second.close(); }

  await page.route('**/api/language', async route => {
    if (route.request().method() === 'POST') await route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ ok: false, data: null, error: 'language_save_failed' }) });
    else await route.continue();
  });
  await chooseLanguage(page, '简体中文');
  await expect(page.getByRole('button', { name: 'Language: English', exact: true })).toBeVisible();
  await expect(page.getByText('Language preferences could not be saved. The previous language has been restored.', { exact: true })).toBeVisible();
  await page.unroute('**/api/language');

  const pages = ['Overview', 'Monitored directories', 'File activity', 'Archive indicators', 'Alert center', 'Rule center', 'Settings and diagnostics'];
  for (const theme of ['Light', 'Dark']) {
    await page.getByRole('button', { name: /^Appearance:/ }).click();
    await page.getByRole('menuitemradio', { name: theme, exact: true }).click();
    for (const width of [1920, 1440, 1024, 720]) {
      await page.setViewportSize({ width, height: 960 });
      for (const name of pages) {
        await page.getByRole('navigation', { name: 'Main navigation' }).getByRole('button', { name, exact: true }).click();
        await expect(page.locator('.page-content > section:not([hidden]) .loading')).toHaveCount(0);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), `${name}/${theme}/${width}px`).toBeTruthy();
        const visibleText = await page.locator('.page-content > section:not([hidden])').innerText();
        expect(visibleText, '英文产品页面没有内部文案key泄漏').not.toMatch(/\b(?:app|components|details|directories|overview|records|rules|settings|format|api)\.[a-zA-Z]+/);
        if (name === 'Rule center') {
          await expect(page.getByRole('switch')).toHaveCount(3);
          expect(await page.locator('.rule-row').evaluateAll(rows => rows.every(row => {
            const bounds = row.getBoundingClientRect(), toggle = row.querySelector('.cds--toggle')!.getBoundingClientRect();
            return toggle.right <= bounds.right + 1 && row.scrollWidth <= row.clientWidth + 1;
          })), '英文规则开关没有溢出').toBeTruthy();
          await expect(page.getByRole('heading', { name: 'Bulk file access', exact: true })).toBeVisible();
        }
      }
      await page.getByRole('navigation', { name: 'Main navigation' }).getByRole('button', { name: 'File activity', exact: true }).click();
      const table = page.getByRole('table', { name: 'File activity records' });
      await table.getByRole('button', { name: 'View', exact: true }).first().click();
      const detail = page.getByRole('complementary', { name: 'Record details' });
      await expect(detail.getByText('Readable open', { exact: true })).toBeVisible();
      expect(await detail.evaluate(element => element.scrollWidth <= element.clientWidth + 1), '英文详情与长路径没有溢出').toBeTruthy();
      await detail.getByRole('button', { name: 'Close', exact: true }).click();
      await page.getByRole('button', { name: 'Export', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: 'Export current results' });
      await expect(dialog.getByLabel('Anonymous sharing mode', { exact: true })).toBeChecked();
      const bounds = (await dialog.boundingBox())!;
      expect(bounds.x >= 0 && bounds.x + bounds.width <= width, '英文弹窗保留在窗口内').toBeTruthy();
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
      if (width === 1440) await page.screenshot({ path: info.outputPath(`localization-${theme.toLowerCase()}.png`), fullPage: true });
    }
  }
  await page.setViewportSize({ width: 1440, height: 960 });
  const preference = await page.request.get(`${data.session.origin}/api/language`, { headers });
  expect((await preference.json()).data.preference).toBe('en');
  await chooseLanguage(page, 'Follow system');
  await expect(page.getByRole('button', { name: /^(语言：跟随系统|Language: Follow system)$/ })).toBeVisible();
  await expect.poll(async () => (await (await page.request.get(`${data.session.origin}/api/language`, { headers })).json()).data.preference).toBe('system');
  // 恢复中文匿名fixture，后续测试不会依赖本机的系统语言。
  await page.request.post(`${data.session.origin}/api/language`, { headers, data: { preference: 'zh-CN' } });
  expect(errors).toEqual([]);
});
