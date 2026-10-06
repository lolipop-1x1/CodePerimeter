import { useState } from 'react';
import { Button, Checkbox, Select, SelectItem, TextInput } from '@carbon/react';
import { DataState, ExportButton, GridTable, Notice, Pager, TableCell, TableRow } from './components';
import { buildFilter, eventLabel, eventPath, kindLabels, processLabel, ruleLabels, timestamp, validateFilters } from './format';
import { usePolling } from './hooks';
import type { AlertEntry, Directory, PageResult, StoredEvent } from './types';
import type { Selection } from './details';

interface Filters { directory: string; pid: string; kind: string; since: string; until: string; search: string; isRead: string; processed: string; filePath?: string }
const initialFilters: Filters = { directory: '', pid: '', kind: '', since: '', until: '', search: '', isRead: '', processed: '' };

export function RecordsPage({ active, alerts = false, archive = false, select, initialScope }: { active: boolean; alerts?: boolean; archive?: boolean; select: (selection: Selection) => void; initialScope?: { directory?: string; pid?: string; filePath?: string; key: number } }) {
  const [draft, setDraft] = useState<Filters>(initialFilters);
  const [filters, setFilters] = useState<Filters>(initialFilters);
  const [scopeKey, setScopeKey] = useState(0);
  const [error, setError] = useState<string>();
  const [realtime, setRealtime] = useState(true);
  const [cursors, setCursors] = useState<string[]>([]);
  if (initialScope && initialScope.key !== scopeKey && active) {
    const next = { ...initialFilters, directory: initialScope.directory ?? '', pid: initialScope.pid ?? '', filePath: initialScope.filePath };
    setScopeKey(initialScope.key); setDraft(next); setFilters(next); setCursors([]);
  }
  const filter = buildFilter(filters, alerts);
  const cursor = cursors.at(-1);
  const payload = { filter, cursor, search: filters.search || undefined,
    ...(alerts ? { is_read: filters.isRead === '' ? undefined : filters.isRead === 'true', processed: filters.processed === '' ? undefined : filters.processed === 'true' } : { archive_only: archive }) };
  const state = usePolling<PageResult<StoredEvent | AlertEntry>>('/api/console', { action: alerts ? 'alerts_page' : 'events_page', payload }, active, realtime ? 500 : 0);
  const directories = usePolling<Directory[]>('/api/console', { action: 'directories' }, active, 5000);
  const change = (key: keyof Filters, value: string) => setDraft(current => ({ ...current, [key]: value }));
  const apply = () => {
    const invalid = validateFilters(draft) || (new TextEncoder().encode(draft.search).length > 256 ? '搜索内容不能超过 256 字节，请缩短关键词。' : null);
    if (invalid) { setError(invalid); return; }
    setError(undefined); setFilters({ ...draft }); setCursors([]); state.refresh();
  };
  return <div className="form-stack">
    {archive && <div className="coverage-info"><h3>默认识别的归档命令</h3><p className="command-list">tar、bsdtar、zip、gtar、ditto、gzip、pigz、bzip2、pbzip2、xz、zstd、7z、7zz、rar</p><p>覆盖常见直接路径和明确输入到标准输出。列表文件、无法关联的标准输入及程序内部压缩保留覆盖缺口；命令或后缀线索不证明压缩成功。</p></div>}
    <form className="filters" onSubmit={event => { event.preventDefault(); apply(); }}>
      <TextInput id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-search`} labelText="搜索文件或进程" value={draft.search} onChange={event => change('search', event.target.value)} placeholder="输入关键词" size="sm" />
      <Select id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-directory`} labelText="监控目录" size="sm" value={draft.directory} onChange={event => change('directory', event.target.value)}><SelectItem value="" text="全部目录" />{directories.data?.map(directory => <SelectItem key={directory.path} value={directory.path} text={directory.path} />)}</Select>
      <TextInput id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-pid`} labelText="进程 ID" value={draft.pid} onChange={event => change('pid', event.target.value)} inputMode="numeric" size="sm" />
      <Select id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-type`} labelText={alerts ? '告警规则' : '活动类型'} value={draft.kind} onChange={event => change('kind', event.target.value)} size="sm"><SelectItem value="" text="全部类型" />{Object.entries(alerts ? ruleLabels : kindLabels).map(([value, text]) => <SelectItem key={value} value={value} text={text} />)}</Select>
      <TextInput id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-since`} type="datetime-local" labelText="开始时间" value={draft.since} onChange={event => change('since', event.target.value)} size="sm" />
      <TextInput id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-until`} type="datetime-local" labelText="结束时间" value={draft.until} onChange={event => change('until', event.target.value)} size="sm" />
      {alerts && <><Select id="alert-read-filter" labelText="阅读状态" value={draft.isRead} onChange={event => change('isRead', event.target.value)} size="sm"><SelectItem value="" text="全部" /><SelectItem value="false" text="未读" /><SelectItem value="true" text="已读" /></Select><Select id="alert-processed-filter" labelText="处理状态" value={draft.processed} onChange={event => change('processed', event.target.value)} size="sm"><SelectItem value="" text="全部" /><SelectItem value="false" text="待处理" /><SelectItem value="true" text="已处理" /></Select></>}
      <div className="filter-actions"><Button type="submit" size="sm">应用筛选</Button><Button kind="ghost" size="sm" onClick={() => { setDraft(initialFilters); setFilters(initialFilters); setError(undefined); setCursors([]); }}>重置</Button></div>
    </form>
    <Notice error={error} />
    <div className="table-toolbar"><div><Checkbox id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-live`} labelText="实时更新" checked={realtime} onChange={(_, { checked }) => setRealtime(checked)} />{state.updatedAt && <span className="helper">最近更新 {new Date(state.updatedAt).toLocaleTimeString('zh-CN', { hour12: false })}</span>}</div><div className="actions"><Button kind="ghost" size="sm" onClick={state.refresh}>刷新</Button><ExportButton kind={alerts ? 'alerts' : 'events'} filter={filter} isRead={filters.isRead === '' ? undefined : filters.isRead === 'true'} processed={filters.processed === '' ? undefined : filters.processed === 'true'} search={filters.search} archiveOnly={archive} /></div></div>
    <DataState loading={state.loading} error={state.error} empty={!state.data?.items.length} emptyTitle={alerts ? '暂无匹配告警' : '暂无匹配活动'} emptyText="请调整筛选条件；宿主或采集异常时可在设置与诊断查看原因。">
      {state.data && <GridTable label={alerts ? '告警记录' : archive ? '归档迹象记录' : '文件活动记录'} headings={alerts ? ['最近证据', '进程', '规则', '阅读', '处理', '详情'] : ['来源时间', '接收时间', '进程', '活动', '文件', '详情']}>
        {state.data.items.map(item => {
          if (alerts) {
            const entry = item as AlertEntry;
            return <TableRow key={entry.alert.id}><TableCell>{timestamp(entry.alert.last_timestamp_ms)}</TableCell><TableCell>{processLabel(entry.alert.process)}</TableCell><TableCell>{ruleLabels[entry.alert.rule]}</TableCell><TableCell>{entry.is_read ? '已读' : <strong>未读</strong>}</TableCell><TableCell>{entry.processed ? '已处理' : '待处理'}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => select({ kind: 'alert', id: entry.alert.id })}>查看</Button></TableCell></TableRow>;
          }
          const stored = item as StoredEvent;
          return <TableRow key={stored.id}><TableCell>{timestamp(stored.event.source_timestamp_ms)}</TableCell><TableCell>{timestamp(stored.event.received_timestamp_ms)}</TableCell><TableCell>{processLabel(stored.event.process)}</TableCell><TableCell>{eventLabel(stored.event)}</TableCell><TableCell className="path">{eventPath(stored.event)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => select({ kind: 'event', id: stored.id })}>查看</Button></TableCell></TableRow>;
        })}
      </GridTable>}
    </DataState>
    {state.data && <Pager total={state.data.total} count={state.data.items.length} cursor={cursor} page={cursors.length + 1} next={state.data.next_cursor} onPrevious={() => setCursors(current => current.slice(0, -1))} onNext={() => state.data?.next_cursor && setCursors(current => [...current, state.data!.next_cursor!])} />}
    <p className="evidence-note">{alerts ? '时间按最近证据筛选。告警是行为线索，人工已处理与通知状态分别记录，不表示实际拦截。' : '时间按事件接收时间筛选。打开与映射分别展示；未采集的读取字节数、读取完成及压缩结果保留未知。'}</p>
  </div>;
}
