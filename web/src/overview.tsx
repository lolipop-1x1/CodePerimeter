import { Button, Select, SelectItem, Tag } from '@carbon/react';
import { useEffect, useRef, useState } from 'react';
import { DataState, GridTable, TableCell, TableRow } from './components';
import { basename, eventLabel, eventPath, number, processLabel, timestamp } from './format';
import { usePolling } from './hooks';
import type { Directory, PageResult, Status, StoredEvent, Summary } from './types';
import type { Selection } from './details';

export interface RecordScope { directory?: string; pid?: string; filePath?: string; key: number }
export function HealthStrip({ status }: { status?: Status }) {
  const host = status?.host;
  const gaps = host && (host.collector_dropped_lines + host.reader_dropped_frames + host.database_gap_events + host.memory_dropped_notifications > 0);
  const collector = !status ? '加载中' : status.service.status_error && !host ? '状态未知' : status.service.paused ? '已暂停' : host ? stateLabel(host.collector_state) : !status.service.installed ? '未安装' : '宿主不可用';
  return <div className="health-strip" aria-label="各组件运行状态"><div><span>系统采集</span><strong data-state={status?.service.paused ? 'paused' : host?.collector_state}>{collector}</strong></div><div><span>证据保存</span><strong data-state={host?.database_state}>{host ? stateLabel(host.database_state) : '未知'}</strong></div><div><span>通知</span><strong>{host ? host.notify_session_active ? '发送端在线，到屏待确认' : '发送端未连接' : '未知'}</strong></div><div><span>覆盖</span>{gaps ? <Tag type="warm-gray">有缺口</Tag> : <strong>{host ? '查看来源诊断' : '未知'}</strong>}</div></div>;
}
export function stateLabel(value: string): string {
  return ({ healthy: '正常', ready: '就绪', running: '运行中', connected: '已连接', connecting: '连接中', reconnecting: '重新连接中', active: '运行中', ok: '正常', idle: '等待事件', paused: '已暂停', stopped: '已停止', stalled: '事件流停滞', disconnected: '已断流', failed: '失败', succeeded: '已成功', cancelled: '已取消', degraded: '有缺口', gap: '有缺口', coverage_gap: '覆盖有缺口', permission_denied: '权限不足', unavailable: '不可用', waiting: '等待采集', initializing: '初始化中', pending: '等待中', observed: '已观察', waiting_for_collector: '等待采集器', source_unhealthy: '来源异常' } as Record<string, string>)[value] ?? '未知状态';
}

export function OverviewPage({ active, select, navigateRecords }: { active: boolean; select: (selection: Selection) => void; navigateRecords: (scope?: Omit<RecordScope, 'key'>) => void }) {
  const [hours, setHours] = useState('24');
  const [windowStart, setWindowStart] = useState(() => Date.now() - 86400000);
  const summary = usePolling<Summary>('/api/console', { action: 'summary', payload: { since_ms: windowStart } }, active);
  const directories = usePolling<Directory[]>('/api/console', { action: 'directories' }, active, 1000);
  const events = usePolling<PageResult<StoredEvent>>('/api/console', { action: 'events_page', payload: { filter: { limit: 6 } } }, active);
  return <div className="form-stack">
    <DataState loading={summary.loading} error={summary.error} empty={!summary.data}>
      {summary.data && <><div className="metric-band"><div><span>有效监控目录</span><strong>{number(directories.data?.filter(directory => directory.effective).length)}</strong></div><div><span>窗口内文件活动</span><strong>{number(summary.data.stats.recent.events)}</strong></div><div><span>窗口内归档命令迹象</span><strong>{number(summary.data.trend.reduce((count, point) => count + point.archive, 0))}</strong></div><div><span>待处理告警</span><strong>{number(summary.data.pending_alerts)}</strong></div></div>
        <section className="panel trend-panel"><div className="section-heading"><h2>文件活动趋势</h2><div className="trend-range"><Select id="trend-range" labelText="统计范围" hideLabel value={hours} onChange={event => { setHours(event.target.value); setWindowStart(Date.now() - Number(event.target.value) * 3600000); }} size="sm"><SelectItem value="1" text="最近 1 小时" /><SelectItem value="24" text="最近 24 小时" /><SelectItem value="168" text="最近 7 天" /></Select></div></div><Trend points={summary.data.trend} /></section>
        <div className="summary-pair"><section className="panel"><h2>活跃进程</h2>{summary.data.top_processes.length ? <GridTable label="窗口内活跃进程" headings={['进程', '活动数', '记录']}>
          {summary.data.top_processes.map(process => <TableRow key={process.pid + (process.executable ?? '')}><TableCell>{basename(process.executable)} ({process.pid})</TableCell><TableCell>{number(process.count)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => navigateRecords({ pid: String(process.pid) })}>查看</Button></TableCell></TableRow>)}
        </GridTable> : <p className="helper">窗口内没有进程活动记录。</p>}</section><section className="panel"><h2>频繁访问的文件</h2>{summary.data.top_files.length ? <GridTable label="窗口内频繁访问文件" headings={['文件', '活动数', '记录']}>
          {summary.data.top_files.map(file => <TableRow key={file.path}><TableCell className="path">{file.path}</TableCell><TableCell>{number(file.count)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => navigateRecords({ filePath: file.path })}>查看</Button></TableCell></TableRow>)}
        </GridTable> : <p className="helper">窗口内没有文件路径记录。</p>}</section></div>
      </>}
    </DataState>
    <section className="panel"><div className="section-heading"><h2>最近活动</h2><Button kind="ghost" size="sm" onClick={() => navigateRecords()}>查看全部活动</Button></div><DataState loading={events.loading} error={events.error} empty={!events.data?.items.length}>
      <GridTable label="最近文件活动" headings={['接收时间', '进程', '活动', '文件', '详情']}>
        {events.data?.items.map(stored => <TableRow key={stored.id}><TableCell>{timestamp(stored.event.received_timestamp_ms)}</TableCell><TableCell>{processLabel(stored.event.process)}</TableCell><TableCell>{eventLabel(stored.event)}</TableCell><TableCell className="path">{eventPath(stored.event)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => select({ kind: 'event', id: stored.id })}>查看</Button></TableCell></TableRow>)}
      </GridTable>
    </DataState></section>
  </div>;
}

function Trend({ points }: { points: Summary['trend'] }) {
  const chart = useRef<SVGSVGElement>(null);
  const [width, setWidth] = useState(820);
  const hasPoints = points.length > 0;
  useEffect(() => {
    if (!chart.current) return;
    const observer = new ResizeObserver(([entry]) => { if (entry.contentRect.width > 0) setWidth(Math.max(280, Math.round(entry.contentRect.width))); });
    observer.observe(chart.current);
    return () => observer.disconnect();
  }, [hasPoints]);
  if (!points.length) return <div className="empty-state"><h3>暂无趋势数据</h3><p>监控活动保存后，会按接收时间形成趋势。</p></div>;
  const maximum = Math.max(1, ...points.flatMap(point => [point.open, point.mmap]));
  const left = 44, right = width - 26, bottom = 172, top = 18;
  const first = points[0].timestamp_ms, last = points.at(-1)!.timestamp_ms;
  const x = (value: number) => left + (last === first ? 0.5 : (value - first) / (last - first)) * (right - left);
  const y = (value: number) => bottom - (value / maximum) * (bottom - top);
  const line = (kind: 'open' | 'mmap') => points.map(point => `${x(point.timestamp_ms)},${y(point[kind])}`).join(' ');
  return <><div className="chart-legend"><span className="open-series">打开</span><span className="mmap-series">映射</span><span className="helper">按事件接收时间统计，纵轴为活动数</span></div><svg ref={chart} className="trend-chart" viewBox={`0 0 ${width} 210`} role="img" aria-label={`文件活动趋势，${points.length} 个时间点，最高 ${maximum} 次`}>
    {[0, 0.5, 1].map(ratio => <g key={ratio}><line className="chart-grid" x1={left} x2={right} y1={y(maximum * ratio)} y2={y(maximum * ratio)} /><text x={left - 9} y={y(maximum * ratio) + 4} textAnchor="end">{Math.round(maximum * ratio)}</text></g>)}
    <polyline className="open-line" fill="none" points={line('open')} /><polyline className="mmap-line" fill="none" points={line('mmap')} />
    {points.length === 1 && <><circle cx={x(first)} cy={y(points[0].open)} r={3} className="open-line" fill="var(--accent)" /><circle cx={x(first)} cy={y(points[0].mmap)} r={3} className="mmap-line" fill="var(--muted)" /></>}
    {(last === first ? [first] : [first, first + (last - first) / 2, last]).map((value, index) => <text key={index} x={x(value)} y={197} textAnchor="middle">{new Date(value).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false })}</text>)}
  </svg></>;
}
