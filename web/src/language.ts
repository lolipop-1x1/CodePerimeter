import { useEffect, useRef, useState } from 'react';
import { api, errorMessage } from './api';
import { i18n, languages, matchBrowserLanguage, t, useLocale } from './i18n';

interface LanguageSettings { preference: string; locale: string; languages: { id: string; name: string }[] }

export function useLanguagePreference(authenticated: boolean) {
  useLocale();
  const [settings, setSettings] = useState<LanguageSettings>({ preference: 'system', locale: i18n.resolvedLanguage ?? 'en', languages });
  const [error, setError] = useState<string>();
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const revision = useRef(0);
  const apply = (next: LanguageSettings) => {
    setSettings(next);
    void i18n.changeLanguage(next.locale);
  };
  useEffect(() => {
    document.documentElement.lang = i18n.resolvedLanguage ?? 'en';
  }, [i18n.resolvedLanguage]);
  useEffect(() => {
    if (!authenticated) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    let controller: AbortController | undefined;
    let loadNumber = 0;
    const load = async () => {
      const currentLoad = ++loadNumber;
      const observedRevision = revision.current;
      if (!pending.current) {
        controller = new AbortController();
        try {
          const next = await api<LanguageSettings>('/api/language', undefined, controller.signal);
          if (alive && currentLoad === loadNumber && !pending.current && observedRevision === revision.current) { apply(next); setError(undefined); }
        } catch (issue) {
          if (alive && !(issue instanceof DOMException && issue.name === 'AbortError')) setError(issue instanceof Error ? issue.message : 'language.loadFailed');
        }
      }
      if (alive && currentLoad === loadNumber) timer = setTimeout(load, 5000);
    };
    const visible = () => { if (!document.hidden) { clearTimeout(timer); controller?.abort(); void load(); } };
    void load();
    document.addEventListener('visibilitychange', visible);
    return () => { alive = false; clearTimeout(timer); controller?.abort(); document.removeEventListener('visibilitychange', visible); };
  }, [authenticated]);
  const select = async (preference: string) => {
    if (pending.current || preference === settings.preference) return;
    const previous = settings;
    revision.current += 1;
    pending.current = true;
    setSaving(true); setError(undefined);
    apply({ ...settings, preference, locale: preference === 'system' ? matchBrowserLanguage(navigator.languages) : preference });
    try { apply(await api<LanguageSettings>('/api/language', { preference })); }
    catch (issue) { apply(previous); setError(issue instanceof Error ? issue.message : 'language.saveFailed'); }
    finally { pending.current = false; setSaving(false); }
  };
  return { preference: settings.preference, languages: settings.languages.filter(item => languages.some(bundled => bundled.id === item.id)), saving,
    label: settings.preference === 'system' ? t('language.followSystem') : settings.languages.find(item => item.id === settings.preference)?.name ?? t('language.followSystem'),
    error: error ? errorMessage(error) : undefined, select };
}
