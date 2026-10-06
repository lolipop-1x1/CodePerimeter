import { useEffect, useState } from 'react';
import { Button, TextInput, Toggle } from '@carbon/react';
import { consoleApi } from './api';
import { DataState, Notice } from './components';
import { useAction, usePolling } from './hooks';
import type { RulesSettings } from './types';

const ruleDefinitions = [
  { field: 'bulk_enabled' as const, title: '批量文件访问', description: '同一进程运行实例在滚动窗口内访问多个不同文件，达到门槛立即产生线索。正常搜索、索引或构建也可能触发。' },
  { field: 'archive_command_enabled' as const, title: '归档命令迹象', description: '识别默认名单中的创建、更新或压缩命令，并与监控目录直接输入关联；解压、查看和完整性测试不算压缩告警。' },
  { field: 'archive_output_enabled' as const, title: '归档输出迹象', description: '把候选归档输出的生成、修改和此前项目活动关联。文件名后缀与行为不证明文件格式或内容。' },
];
type Fields = { threshold: string; window: string; merge: string; correlation: string };
function settingsFields(settings: RulesSettings): Fields { return { threshold: String(settings.bulk_file_threshold), window: String(settings.bulk_window_ms / 1000), merge: String(settings.alert_merge_window_ms / 1000), correlation: String(settings.archive_correlation_window_ms / 1000) }; }
export function validateRuleFields(fields: Fields): string | null {
  const threshold = Number(fields.threshold);
  if (!fields.threshold || !Number.isInteger(threshold) || threshold < 1 || threshold > 512) return '批量阈值必须为 1 至 512 的整数。';
  for (const key of ['window', 'merge', 'correlation'] as const) {
    const value = Number(fields[key]) * 1000;
    if (!fields[key] || !Number.isFinite(value) || Math.abs(value - Math.round(value)) > 0.000001 || value < 100 || value > 3600000) return '时间必须在 0.1 至 3600 秒之间，最多精确到毫秒。';
  }
  return null;
}

export function RulesPage({ active }: { active: boolean }) {
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
    void action.run(() => consoleApi<RulesSettings>('rules_set', { settings: next }), '规则已保存，对后续事件即时生效。').then(result => { if (result) { setSettings(result); setFields(settingsFields(result)); setDirty(false); state.refresh(); } });
  };
  return <DataState loading={state.loading} error={state.error} empty={!settings}>
    {settings && <form className="form-stack" noValidate onSubmit={event => { event.preventDefault(); save(); }}>
      <section className="panel"><div className="section-heading"><h2>三类内置规则</h2><span className="helper">当前版本 {state.data?.version}</span></div>
        {ruleDefinitions.map(rule => <div className="rule-row" key={rule.field}><div><h3>{rule.title}</h3><p>{rule.description}</p></div><Toggle id={rule.field} labelText={`${rule.title}开关`} labelA="关闭" labelB="开启" hideLabel toggled={settings[rule.field]} onToggle={toggled => { setSettings(current => current && { ...current, [rule.field]: toggled }); setDirty(true); }} /></div>)}
      </section>
      <section className="panel"><h2>全局参数</h2><div className="form-grid">
        <TextInput id="bulk-threshold" type="number" min={1} max={512} step={1} labelText="批量阈值（不同文件数）" helperText="同一进程、统计窗口内，1 至 512。" value={fields.threshold} onChange={event => change('threshold', event.target.value)} />
        <TextInput id="bulk-window" type="number" min={0.1} max={3600} step={0.1} labelText="统计窗口（秒）" helperText="滚动统计不同文件，0.1 至 3600 秒。" value={fields.window} onChange={event => change('window', event.target.value)} />
        <TextInput id="alert-merge" type="number" min={0.1} max={3600} step={0.1} labelText="告警合并时间（秒）" helperText="同一进程与规则在窗口内合并，首次即时通知。" value={fields.merge} onChange={event => change('merge', event.target.value)} />
        <TextInput id="archive-correlation" type="number" min={0.1} max={3600} step={0.1} labelText="归档关联时间（秒）" helperText="项目活动与候选归档输出的关联窗口。" value={fields.correlation} onChange={event => change('correlation', event.target.value)} />
      </div></section>
      <Notice error={validation ?? action.error} success={action.success} />
      {settings.version !== state.data?.version && <Notice error="后台规则版本已变化。保留的编辑内容尚未保存，请重新加载后调整。" />}
      <div className="actions"><Button type="submit" disabled={!dirty || action.busy || settings.version !== state.data?.version}>{action.busy ? '正在保存' : '保存规则'}</Button><Button kind="secondary" disabled={action.busy} onClick={() => { if (state.data) { setSettings(state.data); setFields(settingsFields(state.data)); setDirty(false); setValidation(undefined); action.reset(); } }}>重新加载</Button></div>
      <p className="evidence-note">关闭告警规则仍保留文件活动。保存后清空尚未完成的统计窗口，新旧版本不继续合并；旧告警按触发时的参数解释，不重新计算。</p>
    </form>}
  </DataState>;
}
