import test, { after } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import english from '../../locales/web/en.json' with { type: 'json' };
import { bundledLanguages, i18n, matchBrowserLanguage, t } from '../src/i18n.ts';
import { eventLabel, number, ruleLabels, timeOnly, timestamp, validateFilters } from '../src/format.ts';
import { errorMessage } from '../src/api.ts';
import type { ActivityEvent } from '../src/types.ts';

after(async () => { await i18n.changeLanguage('zh-CN'); });

test('用户语言覆盖动态标签、旧错误解释、日期数字和证据口径', async () => {
  const event = { kind: 'open', file: { readable: true }, archive: null } as ActivityEvent;
  const invalid = validateFilters({ pid: '-1', since: '', until: '' })!;
  const original = '合成第三方提示';
  await i18n.changeLanguage('zh-CN');
  assert.equal(eventLabel(event), '可读打开');
  assert.match(errorMessage(invalid), /非负整数/);
  await i18n.changeLanguage('en');
  assert.equal(eventLabel(event), 'Readable open');
  assert.equal(ruleLabels.archive_command, 'Archive command indicator');
  assert.match(errorMessage(invalid), /non-negative integer/);
  assert.match(errorMessage('host_unavailable'), /management host is unavailable/);
  assert.equal(errorMessage(original), `Original message (untranslated): ${original}`);
  assert.equal(number(1234567.5), new Intl.NumberFormat('en').format(1234567.5));
  assert.equal(timestamp(1791310080000), new Date(1791310080000).toLocaleString('en', { hourCycle: 'h23' }));
  assert.doesNotMatch(eventLabel(event), /read in full|completed|blocked/i);
});

test('完整句子使用命名参数，英文单复数分别显示，缺失文案回退', async () => {
  await i18n.changeLanguage('en');
  assert.equal(t('common.files', { count: 1, amount: number(1) }), '1 file');
  assert.equal(t('common.files', { count: 2, amount: number(2) }), '2 files');
  assert.equal(t('pagination.summary', { total: '125', page: '2', pageCount: '25' }), '125 records · Page 2 · 25 on this page');
  assert.equal(t('synthetic.missing'), 'Display explanation unavailable.');
  i18n.addResourceBundle('xx', 'translation', { 'synthetic.greeting': 'Synthetic locale' });
  await i18n.changeLanguage('xx');
  assert.equal(t('synthetic.greeting'), 'Synthetic locale');
  assert.equal(ruleLabels.archive_output, 'Archive output indicator');
  assert.doesNotMatch(t('synthetic.missing'), /synthetic\.missing/);
});

test('按首选语言匹配简体中文，未知语言回退英文；新增资源与登记不要求页面分支', () => {
  assert.equal(matchBrowserLanguage(['zh-TW']), 'zh-CN');
  assert.equal(matchBrowserLanguage(['en-GB']), 'en');
  assert.equal(matchBrowserLanguage(['synthetic-unsupported']), 'en');
  assert.deepEqual(bundledLanguages([{ id: 'en', name: 'English' }, { id: 'xx', name: 'Synthetic' }, { id: 'missing', name: 'Missing' }], { en: {}, xx: { greeting: 'Synthetic' } }).map(item => item.id), ['en', 'xx']);
});

test('所有网页静态文案标识均有资源，避免资源整理后静默显示缺失提示', () => {
  const source = new URL('../src/', import.meta.url);
  const keys = new Set(Object.keys(english));
  for (const file of readdirSync(source).filter(name => /\.tsx?$/.test(name))) {
    const code = readFileSync(new URL(file, source), 'utf8');
    for (const match of code.matchAll(/['"]((?:app|components|details|directories|overview|records|rules|settings|format|api|common|language)\.[A-Za-z0-9_.]+)['"]/g)) {
      assert.ok(keys.has(match[1]) || keys.has(match[1] + '_other'), `缺少静态文案资源：${match[1]}`);
    }
  }
});


test('两种语言的午夜显示00点，不使用24点或12小时制', async () => {
  const midnight = new Date(2026, 9, 7, 0, 15, 0).getTime();
  for (const language of ['zh-CN', 'en']) {
    await i18n.changeLanguage(language);
    assert.equal(timeOnly(midnight, true), '00:15');
    assert.match(timestamp(midnight), /00:15:00/);
    assert.doesNotMatch(timestamp(midnight), /24:|AM|PM/);
  }
});
