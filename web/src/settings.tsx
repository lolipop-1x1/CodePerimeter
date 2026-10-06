import { useEffect, useState } from 'react';
import { Button, TextInput } from '@carbon/react';
import { consoleApi, errorMessage } from './api';
import { ConfirmModal, DataState, ExportButton, GridTable, Notice, TableCell, TableRow } from './components';
import { notificationLabel } from './details';
import { timestamp } from './format';
import { useAction, usePolling } from './hooks';
import { stateLabel } from './overview';
import type { CollectorStreamSample, Health, NotificationRecord, Retention, ServiceOperation, Status } from './types';

export type ServiceAction = 'install' | 'start' | 'pause' | 'resume' | 'uninstall';
export const serviceLabels: Record<ServiceAction, string> = { install: '安装服务', start: '启用监控', pause: '暂停监控', resume: '恢复监控', uninstall: '卸载服务' };
export function SettingsPage({ active, status, operation, operate, operationError }: { active: boolean; status?: Status; operation?: ServiceOperation; operate: (action: ServiceAction) => void; operationError?: string }) {
  const retention = usePolling<Retention>('/api/console', { action: 'retention_get' }, active, 2000);
  const health = usePolling<Health[]>('/api/control', { operation: 'query_health', payload: { filter: { limit: 50 } } }, active, 1000);
  const notifications = usePolling<NotificationRecord[]>('/api/control', { operation: 'query_notifications', payload: { filter: { limit: 50 } } }, active, 1000);
  const [days, setDays] = useState('');
  const [daysDirty, setDaysDirty] = useState(false);
  const [confirm, setConfirm] = useState<'retention' | 'details' | 'cumulative' | 'uninstall'>();
  const [preview, setPreview] = useState<{ counts: Record<string, number>; preview_revision: number }>();
  const action = useAction();
  useEffect(() => { if (retention.data && !daysDirty) setDays(String(retention.data.days)); }, [retention.data?.days, daysDirty]);
  const setRetention = (confirmation = false) => void action.run(() => consoleApi<Retention>('retention_set', { days: Number(days), confirm: confirmation, preview_revision: confirmation ? preview?.preview_revision : undefined }), '明细保留期已保存。').then(result => { if (result) { setDaysDirty(false); setConfirm(undefined); retention.refresh(); } });
  const prepareRetention = () => {
    if (!/^\d+$/.test(days) || Number(days) < 1 || Number(days) > 3650) { void action.run(() => Promise.reject(new Error('保留期需要为 1 至 3650 天的整数。'))); return; }
    if (retention.data && Number(days) < retention.data.days) {
      void action.run(() => consoleApi<{ counts: Record<string, number>; preview_revision: number }>('retention_preview', { days: Number(days) })).then(result => { if (result) { setPreview(result); setConfirm('retention'); } });
    } else setRetention();
  };
  const clear = (kind: 'details' | 'cumulative') => void action.run(() => consoleApi(kind === 'details' ? 'clear_details' : 'clear_cumulative', { confirm: true }), kind === 'details' ? '明细已清除，目录与规则配置保留。' : '累计统计已清除。').then(result => { if (result) { setConfirm(undefined); retention.refresh(); health.refresh(); notifications.refresh(); } });
  const busy = operation?.state === 'running';
  const service = status?.service.status_error ? undefined : status?.service;
  return <div className="form-stack">
    <section className="panel service-panel"><h2>服务与系统授权</h2><p>后台监控与网页独立。管理员密码只在 macOS 系统窗口输入；可以在系统窗口取消操作。</p>
      <dl className="fields"><dt>服务安装</dt><dd>{status ? status.service.status_error ? '状态未知' : status.service.installed ? status.service.installed_binary_trusted ? '已安装，安装程序权限受保护' : '已安装，安装程序权限待验证' : '未安装' : '加载中'}</dd><dt>系统采集任务</dt><dd>{jobLabel(service?.collector)}</dd><dt>管理宿主任务</dt><dd>{jobLabel(service?.analyzer)}</dd><dt>通知任务</dt><dd>{jobLabel(service?.notification)}</dd><dt>暂停意图</dt><dd>{service?.paused == null ? '未知' : service.paused ? '已暂停，跨重启保留' : '未暂停'}</dd><dt>采集实际健康</dt><dd>{status?.host ? stateLabel(status.host.collector_state) : '宿主不可用，无法判断'}</dd></dl>
      <div className="actions">{(['install', 'start', 'pause', 'resume'] as ServiceAction[]).map(item => <Button key={item} kind={item === 'start' ? 'primary' : 'tertiary'} size="sm" disabled={busy || !status?.service_actions_enabled} onClick={() => operate(item)}>{serviceLabels[item]}</Button>)}<Button kind="danger--ghost" size="sm" disabled={busy || !status?.service_actions_enabled} onClick={() => setConfirm('uninstall')}>卸载服务</Button></div>
      {!status?.service_actions_enabled && <p className="helper">此运行环境没有启用系统服务操作，查询与匿名应用测试仍可独立使用。</p>}
      <Notice error={operationError ?? (status?.service.status_error ? errorMessage(status.service.status_error) : undefined)} success={operation?.state === 'succeeded' ? '服务操作已完成，请分别核查采集与保存健康状态。' : operation?.state === 'cancelled' ? '系统授权已取消，未报告操作成功。' : undefined} />
      {busy && <div className="operation-progress" role="status">正在执行{serviceLabels[operation.operation as ServiceAction] ?? '服务操作'}，请查看 macOS 系统授权窗口。历史查询继续可用。</div>}
      {operation?.report && <ul aria-label="服务操作步骤">{operation.report.steps.map((step, index) => <li key={index}>{step.success ? '成功' : '失败'}：{step.message}</li>)}</ul>}
      <details><summary>完全磁盘访问授权指引</summary><p>打开 macOS 系统设置 → 隐私与安全性 → 完全磁盘访问，按安装方案添加系统采集器与受保护的采集程序。网页无法代替系统授予权限。</p><p>授权后恢复采集，并观察新的真实事件。任务已加载或安装成功不代表 eslogger 已取得权限。</p></details>
      <details><summary>系统通知授权指引</summary><p>首次启动通知程序时，在 macOS 弹窗中允许 CodePerimeter 通知。如果没有弹出提醒，请到系统设置 → 通知 → CodePerimeter，开启允许通知。</p><p>关闭系统通知不影响采集和告警记录。通知发送反馈与实际到屏分别核对。</p></details>
    </section>
    <section className="panel"><h2>数据保留与导出</h2><DataState loading={retention.loading} error={retention.error} empty={!retention.data}>
      {retention.data && <><div className="retention-form"><TextInput id="retention-days" size="md" type="number" min={1} max={3650} step={1} labelText="明细保留天数" helperText="默认 30 天，可设置 1 至 3650 天。累计统计保留到单独清除。" value={days} onChange={event => { setDays(event.target.value); setDaysDirty(true); }} /><Button size="md" disabled={!daysDirty || action.busy} onClick={prepareRetention}>保存保留期</Button></div><dl className="count-grid">{Object.entries(retention.data.counts).map(([kind, count]) => <div key={kind}><dt>{countLabel(kind)}</dt><dd>{count.toLocaleString('zh-CN')}</dd></div>)}</dl></>}
    </DataState><div className="actions"><span>全部文件活动</span><ExportButton kind="events" filter={{}} /><span>全部告警</span><ExportButton kind="alerts" filter={{}} /></div><p className="helper">记录页的导出遵循当前筛选；此处导出全部保留记录。匿名模式用于分享，完整模式仅在本机按需使用。</p></section>
    <section className="panel danger-panel"><h2>清理记录</h2><p>明细与累计统计分别清除。保留目录和规则配置，不删除项目文件、原始会话文件；清理后仍可产生新记录。</p><div className="actions"><Button kind="danger--tertiary" size="sm" disabled={!retention.data || action.busy} onClick={() => { action.reset(); setConfirm('details'); }}>清除明细</Button><Button kind="danger--tertiary" size="sm" disabled={!retention.data || action.busy} onClick={() => { action.reset(); setConfirm('cumulative'); }}>清除累计统计</Button></div><Notice error={action.error} success={action.success} /></section>
    <section className="panel"><h2>采集与证据保存诊断</h2>{status?.host && <dl className="fields"><dt>最后接收事件</dt><dd>{timestamp(status.host.last_event_received_ms)}</dd><dt>采集丢弃行数</dt><dd>{status.host.collector_dropped_lines}</dd><dt>宿主丢弃帧数</dt><dd>{status.host.reader_dropped_frames}</dd><dt>证据保存缺口</dt><dd>{status.host.database_gap_events}</dd><dt>内存通知丢弃</dt><dd>{status.host.memory_dropped_notifications}</dd><dt>通知端心跳</dt><dd>{timestamp(status.host.notify_session_last_seen_ms)}</dd><dt>保留期清理状态</dt><dd>{stateLabel(status.host.retention_state)}</dd></dl>}
      <CollectorSourceDiagnostics streams={status?.host?.collector_streams} />
      <DataState loading={health.loading} error={health.error} empty={!health.data?.length} emptyTitle="暂无健康记录" emptyText="暂无已保存的诊断，不等同于所有组件已通过真实验收。"><GridTable label="组件健康诊断" headings={['时间', '组件', '状态', '原因']}>
        {health.data?.map(item => <TableRow key={item.id}><TableCell>{timestamp(item.record.observed_timestamp_ms)}</TableCell><TableCell>{item.record.component}</TableCell><TableCell>{stateLabel(item.record.state)}</TableCell><TableCell>{item.record.code}{item.record.detail && <p className="helper">{item.record.detail}</p>}</TableCell></TableRow>)}
      </GridTable></DataState></section>
    <section className="panel"><h2>通知发送反馈</h2><DataState loading={notifications.loading} error={notifications.error} empty={!notifications.data?.length} emptyTitle="暂无通知反馈" emptyText="系统通知到屏需要独立观察，发送回执不证明用户已阅读。"><GridTable label="通知发送反馈" headings={['时间', '告警', '反馈']}>
      {notifications.data?.map(item => <TableRow key={item.id}><TableCell>{timestamp(item.record.observed_timestamp_ms)}</TableCell><TableCell className="path">{item.record.alert_id}</TableCell><TableCell>{notificationLabel(item.record.outcome)}</TableCell></TableRow>)}
    </GridTable></DataState></section>
    <ConfirmModal open={Boolean(confirm)} heading={confirm === 'retention' ? '缩短明细保留期' : confirm === 'uninstall' ? '卸载后台服务' : confirm === 'details' ? '清除明细记录' : '清除累计统计'} busy={action.busy || busy} error={action.error} close={() => setConfirm(undefined)} submit={() => { if (confirm === 'retention') setRetention(true); else if (confirm === 'uninstall') { operate('uninstall'); setConfirm(undefined); } else if (confirm) clear(confirm); }} text={confirm === 'retention' ? <><p>将保留期从 {retention.data?.days} 天缩短到 {days} 天，清理超过新保留期的记录：</p><dl className="count-grid">{Object.entries(preview?.counts ?? {}).map(([kind, count]) => <div key={kind}><dt>{countLabel(kind)}</dt><dd>{count}</dd></div>)}</dl><p>关联处理记录同步清理，累计统计和目录、规则配置保留。</p></> : confirm === 'uninstall' ? <p>停止并卸载后台监控任务，默认保留配置与记录。卸载后管理宿主可能不可用，网页会继续显示实际状态与安装入口。</p> : confirm === 'details' ? <><p>将清除当前全部明细及关联处理记录：</p><dl className="count-grid">{Object.entries(retention.data?.counts ?? {}).map(([kind, count]) => <div key={kind}><dt>{countLabel(kind)}</dt><dd>{count}</dd></div>)}</dl><p>保留配置与累计统计，删除明细后无法通过本控制台恢复。</p></> : <p>将累计活动、告警、健康与通知统计重置。明细记录、目录与规则配置保留。</p>} />
  </div>;
}
export function CollectorSourceDiagnostics({ streams }: { streams?: Record<string, CollectorStreamSample> }) {
  const samples = Object.entries(streams ?? {});
  return <div className="form-stack"><h3>采集来源最新样本</h3><p className="helper">各来源最近一条事件的来源时间至接收时间差，不是历史最大延迟。接收时间用于判断样本新旧；缺少时间时无法计算，负差表示时钟异常。</p>
    {samples.length ? <GridTable label="采集来源最新样本" headings={['采集来源', '来源至接收延迟', '样本接收时间']}>
      {samples.map(([source, sample]) => <TableRow key={source}><TableCell>{({ exec: '命令', read: '打开／映射', write: '写入', activity: '文件活动', combined: '合并' } as Record<string, string>)[source] ?? '未知来源'}</TableCell><TableCell>{latestSourceLatency(sample)}</TableCell><TableCell>{typeof sample.last_received_timestamp_ms === 'number' && Number.isFinite(sample.last_received_timestamp_ms) ? timestamp(sample.last_received_timestamp_ms) : '未知'}</TableCell></TableRow>)}
    </GridTable> : <p className="helper">暂无来源样本，延迟与接收时间未知。</p>}
  </div>;
}
function latestSourceLatency(sample: CollectorStreamSample): string {
  const source = sample.last_source_timestamp_ms, received = sample.last_received_timestamp_ms;
  if (typeof source !== 'number' || typeof received !== 'number' || !Number.isFinite(source) || !Number.isFinite(received)) return '未知';
  const difference = received - source;
  return difference < 0 ? `时钟异常（${difference.toLocaleString('zh-CN')} ms）` : `${difference.toLocaleString('zh-CN')} ms`;
}
function jobLabel(job?: Status['service']['collector']): string { return !job ? '未知' : `${job.loaded ? '已加载' : '未加载'}，${job.running ? '运行中' : '未运行'}${job.disabled === true ? '，已禁用' : ''}${job.last_exit_code ? `，上次退出码 ${job.last_exit_code}` : ''}`; }
function countLabel(kind: string): string { return ({ events: '文件活动', alerts: '告警', health_records: '健康记录', notifications: '通知反馈', handling_records: '处理记录', outbox_entries: '通知队列' } as Record<string, string>)[kind] ?? kind; }
