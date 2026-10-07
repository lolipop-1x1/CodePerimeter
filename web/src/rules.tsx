import { t, useLocale } from './i18n';
import { useEffect, useState } from 'react';
import { Button, TextInput, Toggle } from '@carbon/react';
import { consoleApi } from './api';
import { DataState, Notice } from './components';
import { useAction, usePolling } from './hooks';
import type { RulesSettings } from './types';

const ruleDefinitions = [
  { field: 'bulk_enabled' as const, get title() { return t('rules.bulkFileAccess'); }, get description() { return t('rules.theSameProcessInstanceAccessesMultipleDifferent'); } },
  { field: 'archive_command_enabled' as const, get title() { return t('rules.archiveCommandIndicator'); }, get description() { return t('rules.recognizesCreateUpdateAndCompressionOperationsFrom'); } },
  { field: 'archive_output_enabled' as const, get title() { return t('rules.archiveOutputIndicator'); }, get description() { return t('rules.associatesCreationOrModificationOfCandidateArchive'); } },
];
type Fields = { threshold: string; window: string; merge: string; correlation: string };
function settingsFields(settings: RulesSettings): Fields { return { threshold: String(settings.bulk_file_threshold), window: String(settings.bulk_window_ms / 1000), merge: String(settings.alert_merge_window_ms / 1000), correlation: String(settings.archive_correlation_window_ms / 1000) }; }
export function validateRuleFields(fields: Fields): string | null {
  const threshold = Number(fields.threshold);
  if (!fields.threshold || !Number.isInteger(threshold) || threshold < 1 || threshold > 512) return 'rules.theBulkThresholdMustBeAnInteger';
  for (const key of ['window', 'merge', 'correlation'] as const) {
    const value = Number(fields[key]) * 1000;
    if (!fields[key] || !Number.isFinite(value) || Math.abs(value - Math.round(value)) > 0.000001 || value < 100 || value > 3600000) return 'rules.timeMustBeBetweenAndSecondsWith';
  }
  return null;
}

export function RulesPage({ active }: { active: boolean }) {
  useLocale();
  const state = usePolling<RulesSettings>('/api/console', { action: 'rules_get' }, active, 1000);
  const [settings, setSettings] = useState<RulesSettings>();
  const [fields, setFields] = useState<Fields>({ threshold: '', window: '', merge: '', correlation: '' });
  const [dirty, setDirty] = useState(false);
  const [validation, setValidation] = useState<string>();
  const action = useAction();
  useEffect(() => { if (state.data && !dirty) { setSettings(state.data); setFields(settingsFields(state.data)); } }, [state.data?.version, dirty]);
  const change = (key: keyof Fields, value: string) => { setFields(current => ({ ...current, [key]: value })); setDirty(true); };
  const save = () => {
    const invalid = validateRuleFields(fields);
    if (invalid) { setValidation(invalid); return; }
    if (!settings) return;
    setValidation(undefined);
    const next = { ...settings, bulk_file_threshold: Number(fields.threshold), bulk_window_ms: Math.round(Number(fields.window) * 1000), alert_merge_window_ms: Math.round(Number(fields.merge) * 1000), archive_correlation_window_ms: Math.round(Number(fields.correlation) * 1000) };
    void action.run(() => consoleApi<RulesSettings>('rules_set', { settings: next }), 'rules.rulesSavedEffectiveImmediatelyForSubsequentEvents').then(result => { if (result) { setSettings(result); setFields(settingsFields(result)); setDirty(false); state.refresh(); } });
  };
  return <DataState loading={state.loading} error={state.error} empty={!settings}>
    {settings && <form className="form-stack" noValidate onSubmit={event => { event.preventDefault(); save(); }}>
      <section className="panel"><div className="section-heading"><h2>{t('rules.threeBuiltInRules')}</h2><span className="helper">{t('rules.version', { version: state.data?.version })}</span></div>
        {ruleDefinitions.map(rule => <div className="rule-row" key={rule.field}><div><h3>{rule.title}</h3><p>{rule.description}</p></div><Toggle id={rule.field} aria-label={t('rules.toggle', { rule: rule.title })} labelA={t('rules.off')} labelB={t('rules.on')} toggled={settings[rule.field]} onToggle={toggled => { setSettings(current => current && { ...current, [rule.field]: toggled }); setDirty(true); }} /></div>)}
      </section>
      <section className="panel"><h2>{t('rules.globalParameters')}</h2><div className="form-grid">
        <TextInput id="bulk-threshold" size="md" type="number" min={1} max={512} step={1} labelText={t('rules.bulkThresholdDistinctFiles')} helperText={t('rules.toDistinctFilesPerProcessAndCounting')} value={fields.threshold} onChange={event => change('threshold', event.target.value)} />
        <TextInput id="bulk-window" size="md" type="number" min={0.1} max={3600} step={0.1} labelText={t('rules.countingWindowSeconds')} helperText={t('rules.rollingCountOfDistinctFilesFromTo')} value={fields.window} onChange={event => change('window', event.target.value)} />
        <TextInput id="alert-merge" size="md" type="number" min={0.1} max={3600} step={0.1} labelText={t('rules.alertMergeWindowSeconds')} helperText={t('rules.sameProcessAndRuleMergeWithinThe')} value={fields.merge} onChange={event => change('merge', event.target.value)} />
        <TextInput id="archive-correlation" size="md" type="number" min={0.1} max={3600} step={0.1} labelText={t('rules.archiveCorrelationWindowSeconds')} helperText={t('rules.associationWindowBetweenProjectActivityAndCandidate')} value={fields.correlation} onChange={event => change('correlation', event.target.value)} />
      </div></section>
      <Notice error={validation ?? action.error} success={action.success} />
      {settings.version !== state.data?.version && <Notice error={t('rules.backgroundRuleVersionChangedYourEditsRemain')} />}
      <div className="actions"><Button type="submit" size="md" disabled={!dirty || action.busy || settings.version !== state.data?.version}>{action.busy ? t('rules.saving') : t('rules.saveRules')}</Button><Button kind="tertiary" size="md" disabled={action.busy} onClick={() => { if (state.data) { setSettings(state.data); setFields(settingsFields(state.data)); setDirty(false); setValidation(undefined); action.reset(); } }}>{t('rules.reload')}</Button></div>
      <p className="evidence-note">{t('rules.disablingAlertRulesStillRetainsFileActivity')}</p>
    </form>}
  </DataState>;
}
