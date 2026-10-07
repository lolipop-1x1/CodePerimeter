import test from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';

test('真实构建识别新增资源和登记，切换合成语言并回退英文', async () => {
  const sourceRoot = dirname(fileURLToPath(new URL('../package.json', import.meta.url)));
  const temporary = mkdtempSync(join(tmpdir(), 'codeperimeter-localization-registration-'));
  chmodSync(temporary, 0o700);
  const web = join(temporary, 'web');
  try {
    mkdirSync(join(web, 'src'), { recursive: true });
    mkdirSync(join(temporary, 'locales', 'web'), { recursive: true });
    copyFileSync(join(sourceRoot, 'src', 'i18n.ts'), join(web, 'src', 'i18n.ts'));
    for (const language of ['zh-CN', 'en']) copyFileSync(join(sourceRoot, '..', 'locales', 'web', language + '.json'), join(temporary, 'locales', 'web', language + '.json'));
    writeFileSync(join(temporary, 'locales', 'languages.json'), JSON.stringify([{ id: 'zh-CN', name: '简体中文' }, { id: 'en', name: 'English' }, { id: 'xx', name: 'Synthetic' }]));
    writeFileSync(join(temporary, 'locales', 'web', 'xx.json'), JSON.stringify({ 'synthetic.greeting': 'Synthetic locale' }));
    symlinkSync(join(sourceRoot, 'node_modules'), join(web, 'node_modules'), 'dir');
    const compiled = await build({ root: web, configFile: false, logLevel: 'silent', build: { write: false, minify: false, lib: { entry: join(web, 'src', 'i18n.ts'), formats: ['es'] }, rollupOptions: { output: { inlineDynamicImports: true } } } });
    const result = Array.isArray(compiled) ? compiled[0] : compiled;
    assert.ok('output' in result);
    const output = result.output;
    const code = output.find(item => item.type === 'chunk')!;
    assert.equal(code.type, 'chunk');
    const module = await import('data:text/javascript;base64,' + Buffer.from(code.code).toString('base64'));
    assert.deepEqual(module.languages.map((language: { id: string }) => language.id), ['zh-CN', 'en', 'xx']);
    assert.equal(module.matchBrowserLanguage(['xx']), 'xx');
    await module.i18n.changeLanguage('xx');
    assert.equal(module.t('synthetic.greeting'), 'Synthetic locale');
    assert.equal(module.t('app.overview'), 'Overview');
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});
