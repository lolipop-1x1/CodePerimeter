import { t, useLocale } from './i18n';
import { Button, Select, SelectItem, Tag } from '@carbon/react';
import { useEffect, useRef, useState } from 'react';
import { DataState, GridTable, TableCell, TableRow } from './components';
import { basename, eventLabel, eventPath, number, processLabel, timeOnly, timestamp } from './format';
import { usePolling } from './hooks';
import type { Directory, PageResult, Status, StoredEvent, Summary } from './types';
import type { Selection } from './details';

export interface RecordScope { directory?: string; pid?: string; filePath?: string; key: number }
export function HealthStrip({ status }: { status?: Status }) {
  useLocale();
  const host = status?.host;
  const gaps = host && (host.collector_dropped_lines + host.reader_dropped_frames + host.database_gap_events + host.memory_dropped_notifications > 0);
  const collector = !status ? t('overview.loading') : status.service.status_error && !host ? t('overview.stateUnknown') : status.service.paused ? t('overview.paused') : host ? stateLabel(host.collector_state) : !status.service.installed ? t('overview.notInstalled') : t('overview.hostUnavailable');
  return <div className="health-strip" aria-label={t('overview.componentStates')}><div><span>{t('overview.systemCollection')}</span><strong data-state={status?.service.paused ? 'paused' : host?.collector_state}>{collector}</strong></div><div><span>{t('overview.evidenceStorage')}</span><strong data-state={host?.database_state}>{host ? stateLabel(host.database_state) : t('overview.unknown')}</strong></div><div><span>{t('overview.notifications')}</span><strong>{host ? host.notify_session_active ? t('overview.senderOnlineDisplayUnconfirmed') : t('overview.senderDisconnected') : t('overview.unknown')}</strong></div><div><span>{t('overview.coverage')}</span>{gaps ? <Tag type="warm-gray">{t('overview.gapsPresent')}</Tag> : <strong>{host ? t('overview.viewSourceDiagnostics') : t('overview.unknown')}</strong>}</div></div>;
}
export function stateLabel(value: string): string {
  return ({ dropped: t('overview.dropped'), retrying: t('overview.retrying'), healthy: t('overview.healthy'), ready: t('overview.ready'), running: t('overview.running'), connected: t('overview.connected'), connecting: t('overview.connecting'), reconnecting: t('overview.reconnecting'), active: t('overview.running'), ok: t('overview.healthy'), idle: t('overview.waitingForEvents'), paused: t('overview.paused'), stopped: t('overview.stopped'), stalled: t('overview.eventStreamStalled'), disconnected: t('overview.disconnected'), failed: t('overview.failed'), succeeded: t('overview.succeeded'), cancelled: t('overview.cancelled'), degraded: t('overview.gapsPresent'), gap: t('overview.gapsPresent'), coverage_gap: t('overview.coverageGaps'), permission_denied: t('overview.permissionDenied'), unavailable: t('overview.unavailable'), waiting: t('overview.waitingForCollection'), initializing: t('overview.initializing'), pending: t('overview.pending'), observed: t('overview.observed'), waiting_for_collector: t('overview.waitingForCollector'), source_unhealthy: t('overview.sourceUnhealthy') } as Record<string, string>)[value] ?? t('overview.unknownState');
}

export function OverviewPage({ active, select, navigateRecords }: { active: boolean; select: (selection: Selection) => void; navigateRecords: (scope?: Omit<RecordScope, 'key'>) => void }) {
  useLocale();
  const [hours, setHours] = useState('24');
  const [windowStart, setWindowStart] = useState(() => Date.now() - 86400000);
  const summary = usePolling<Summary>('/api/console', { action: 'summary', payload: { since_ms: windowStart } }, active);
  const directories = usePolling<Directory[]>('/api/console', { action: 'directories' }, active, 1000);
  const events = usePolling<PageResult<StoredEvent>>('/api/console', { action: 'events_page', payload: { filter: { limit: 6 } } }, active);
  return <div className="form-stack">
    <DataState loading={summary.loading} error={summary.error} empty={!summary.data}>
      {summary.data && <><div className="metric-band"><div><span>{t('overview.effectiveMonitoredDirectories')}</span><strong>{number(directories.data?.filter(directory => directory.effective).length)}</strong></div><div><span>{t('overview.fileActivityInWindow')}</span><strong>{number(summary.data.stats.recent.events)}</strong></div><div><span>{t('overview.archiveCommandIndicatorsInWindow')}</span><strong>{number(summary.data.trend.reduce((count, point) => count + point.archive, 0))}</strong></div><div><span>{t('overview.pendingAlerts')}</span><strong>{number(summary.data.pending_alerts)}</strong></div></div>
        <section className="panel trend-panel"><div className="section-heading"><h2>{t('overview.fileActivityTrend')}</h2><div className="trend-range"><Select id="trend-range" labelText={t('overview.timeRange')} hideLabel value={hours} onChange={event => { setHours(event.target.value); setWindowStart(Date.now() - Number(event.target.value) * 3600000); }} size="sm"><SelectItem value="1" text={t('overview.lastHour')} /><SelectItem value="24" text={t('overview.lastHours')} /><SelectItem value="168" text={t('overview.lastDays')} /></Select></div></div><Trend points={summary.data.trend} /></section>
        <div className="summary-pair"><section className="panel"><h2>{t('overview.activeProcesses')}</h2>{summary.data.top_processes.length ? <GridTable label={t('overview.activeProcessesInWindow')} headings={[t('overview.process'), t('overview.activities'), t('overview.records')]}>
          {summary.data.top_processes.map(process => <TableRow key={process.pid + (process.executable ?? '')}><TableCell>{basename(process.executable)} ({process.pid})</TableCell><TableCell>{number(process.count)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => navigateRecords({ pid: String(process.pid) })}>{t('overview.view')}</Button></TableCell></TableRow>)}
        </GridTable> : <p className="helper">{t('overview.noProcessActivityInThisWindow')}</p>}</section><section className="panel"><h2>{t('overview.frequentlyAccessedFiles')}</h2>{summary.data.top_files.length ? <GridTable label={t('overview.frequentlyAccessedFilesInWindow')} headings={[t('overview.file'), t('overview.activities'), t('overview.records')]}>
          {summary.data.top_files.map(file => <TableRow key={file.path}><TableCell className="path">{file.path}</TableCell><TableCell>{number(file.count)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => navigateRecords({ filePath: file.path })}>{t('overview.view')}</Button></TableCell></TableRow>)}
        </GridTable> : <p className="helper">{t('overview.noFilePathsRecordedInThisWindow')}</p>}</section></div>
      </>}
    </DataState>
    <section className="panel"><div className="section-heading"><h2>{t('overview.recentActivity')}</h2><Button kind="ghost" size="sm" onClick={() => navigateRecords()}>{t('overview.viewAllActivity')}</Button></div><DataState loading={events.loading} error={events.error} empty={!events.data?.items.length}>
      <GridTable label={t('overview.recentFileActivity')} headings={[t('overview.received'), t('overview.process'), t('overview.activity'), t('overview.file'), t('overview.details')]}>
        {events.data?.items.map(stored => <TableRow key={stored.id}><TableCell>{timestamp(stored.event.received_timestamp_ms)}</TableCell><TableCell>{processLabel(stored.event.process)}</TableCell><TableCell>{eventLabel(stored.event)}</TableCell><TableCell className="path">{eventPath(stored.event)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => select({ kind: 'event', id: stored.id })}>{t('overview.view')}</Button></TableCell></TableRow>)}
      </GridTable>
    </DataState></section>
  </div>;
}

function Trend({ points }: { points: Summary['trend'] }) {
  useLocale();
  const chart = useRef<SVGSVGElement>(null);
  const [width, setWidth] = useState(820);
  const hasPoints = points.length > 0;
  useEffect(() => {
    if (!chart.current) return;
    const observer = new ResizeObserver(([entry]) => { if (entry.contentRect.width > 0) setWidth(Math.max(280, Math.round(entry.contentRect.width))); });
    observer.observe(chart.current);
    return () => observer.disconnect();
  }, [hasPoints]);
  if (!points.length) return <div className="empty-state"><h3>{t('overview.noTrendDataYet')}</h3><p>{t('overview.savedMonitoringActivityFormsATrendBy')}</p></div>;
  const maximum = Math.max(1, ...points.flatMap(point => [point.open, point.mmap]));
  const left = 44, right = width - 26, bottom = 172, top = 18;
  const first = points[0].timestamp_ms, last = points.at(-1)!.timestamp_ms;
  const x = (value: number) => left + (last === first ? 0.5 : (value - first) / (last - first)) * (right - left);
  const y = (value: number) => bottom - (value / maximum) * (bottom - top);
  const line = (kind: 'open' | 'mmap') => points.map(point => `${x(point.timestamp_ms)},${y(point[kind])}`).join(' ');
  return <><div className="chart-legend"><span className="open-series">{t('overview.opens')}</span><span className="mmap-series">{t('overview.mappings')}</span><span className="helper">{t('overview.countedByEventReceiveTimeVerticalAxis')}</span></div><svg ref={chart} className="trend-chart" viewBox={`0 0 ${width} 210`} role="img" aria-label={t('overview.fileActivityTrendTimePointsMaximumActivities', { points: number(points.length), maximum: number(maximum) })}>
    {[0, 0.5, 1].map(ratio => <g key={ratio}><line className="chart-grid" x1={left} x2={right} y1={y(maximum * ratio)} y2={y(maximum * ratio)} /><text x={left - 9} y={y(maximum * ratio) + 4} textAnchor="end">{number(Math.round(maximum * ratio))}</text></g>)}
    <polyline className="open-line" fill="none" points={line('open')} /><polyline className="mmap-line" fill="none" points={line('mmap')} />
    {points.length === 1 && <><circle cx={x(first)} cy={y(points[0].open)} r={3} className="open-line" fill="var(--accent)" /><circle cx={x(first)} cy={y(points[0].mmap)} r={3} className="mmap-line" fill="var(--muted)" /></>}
    {(last === first ? [first] : [first, first + (last - first) / 2, last]).map((value, index) => <text key={index} x={x(value)} y={197} textAnchor="middle">{timeOnly(value, true)}</text>)}
  </svg></>;
}
