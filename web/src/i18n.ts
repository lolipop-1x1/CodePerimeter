import i18next, { type TOptions } from 'i18next';
import { initReactI18next, useTranslation } from 'react-i18next';
import registeredLanguages from '../../locales/languages.json' with { type: 'json' };
import chinese from '../../locales/web/zh-CN.json' with { type: 'json' };
import english from '../../locales/web/en.json' with { type: 'json' };

// 资源随程序打包，新增语言只需提供资源并登记名称。
const catalogs: Record<string, Record<string, string>> = { 'zh-CN': chinese, en: english };
// Vite 构建会替换资源宏，Node 单测没有其环境与宏。
if (import.meta.env?.MODE) {
  const bundled = import.meta.glob<Record<string, string>>(['../../locales/web/*.json', '!../../locales/web/zh-CN.json', '!../../locales/web/en.json'], { eager: true, import: 'default' });
  for (const [path, catalog] of Object.entries(bundled)) catalogs[path.split('/').at(-1)!.replace(/\.json$/, '')] = catalog;
}
export function bundledLanguages(registry: readonly { id: string; name: string }[], resources: Record<string, Record<string, string>>) {
  return registry.filter(language => Boolean(resources[language.id]));
}
export const languages = bundledLanguages(registeredLanguages, catalogs);

export function matchBrowserLanguage(preferred: readonly string[]): string {
  for (const language of preferred) {
    const exact = languages.find(item => item.id.toLowerCase() === language.toLowerCase());
    if (exact) return exact.id;
    const base = language.toLowerCase().split('-')[0];
    const match = languages.find(item => item.id.toLowerCase().split('-')[0] === base);
    if (match) return match.id;
  }
  return 'en';
}

export const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: typeof window === 'undefined' ? 'zh-CN' : matchBrowserLanguage(navigator.languages),
  fallbackLng: 'en',
  resources: Object.fromEntries(languages.map(language => [language.id, { translation: catalogs[language.id] }])),
  keySeparator: false,
  interpolation: { escapeValue: false },
  initAsync: false,
});

export function locale(): string { return i18n.resolvedLanguage ?? 'en'; }
export function t(key: string, options?: TOptions): string {
  return i18n.exists(key, options) ? String(i18n.t(key, options)) : String(i18n.t('common.translationUnavailable'));
}
export function useLocale(): string {
  useTranslation(undefined, { i18n });
  return locale();
}
