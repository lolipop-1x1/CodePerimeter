import { t, useLocale } from './i18n';
import { useState } from 'react';
import { Button, Checkbox, Select, SelectItem, Tag, TextInput } from '@carbon/react';
import { DataState, ExportButton, GridTable, Notice, Pager, TableCell, TableRow } from './components';
import { buildFilter, eventLabel, eventPath, kindLabels, processLabel, ruleLabels, timeOnly, timestamp, validateFilters } from './format';
import { usePolling } from './hooks';
import type { AlertEntry, Directory, PageResult, StoredEvent } from './types';
import type { Selection } from './details';

interface Filters { directory: string; pid: string; kind: string; since: string; until: string; search: string; isRead: string; processed: string; filePath?: string }
const initialFilters: Filters = { directory: '', pid: '', kind: '', since: '', until: '', search: '', isRead: '', processed: '' };

export function RecordsPage({ active, alerts = false, archive = false, select, initialScope }: { active: boolean; alerts?: boolean; archive?: boolean; select: (selection: Selection) => void; initialScope?: { directory?: string; pid?: string; filePath?: string; key: number } }) {
  useLocale();
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
    const invalid = validateFilters(draft) || (new TextEncoder().encode(draft.search).length > 256 ? 'records.searchTextMustNotExceedBytesShorten' : null);
    if (invalid) { setError(invalid); return; }
    setError(undefined); setFilters({ ...draft }); setCursors([]); state.refresh();
  };
  return <div className="form-stack">
    {archive && <div className="coverage-info"><h3>{t('records.archiveCommandsRecognizedByDefault')}</h3><p className="command-list">{['tar', 'bsdtar', 'zip', 'gtar', 'ditto', 'gzip', 'pigz', 'bzip2', 'pbzip2', 'xz', 'zstd', '7z', '7zz', 'rar'].join(t('common.listSeparator'))}</p><p>{t('records.commonDirectPathsAndExplicitInputTo')}</p></div>}
    <form className="filters" onSubmit={event => { event.preventDefault(); apply(); }}>
      <TextInput id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-search`} labelText={t('records.searchFilesOrProcesses')} value={draft.search} onChange={event => change('search', event.target.value)} placeholder={t('records.enterKeywords')} size="sm" />
      <Select id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-directory`} labelText={t('records.monitoredDirectory')} size="sm" value={draft.directory} onChange={event => change('directory', event.target.value)}><SelectItem value="" text={t('records.allDirectories')} />{directories.data?.map(directory => <SelectItem key={directory.path} value={directory.path} text={directory.path} />)}</Select>
      <TextInput id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-pid`} labelText={t('records.processID')} value={draft.pid} onChange={event => change('pid', event.target.value)} inputMode="numeric" size="sm" />
      <Select id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-type`} labelText={alerts ? t('records.alertRule') : t('records.activityType')} value={draft.kind} onChange={event => change('kind', event.target.value)} size="sm"><SelectItem value="" text={t('records.allTypes')} />{Object.entries(alerts ? ruleLabels : kindLabels).map(([value, text]) => <SelectItem key={value} value={value} text={text} />)}</Select>
      <TextInput id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-since`} type="datetime-local" labelText={t('records.startTime')} value={draft.since} onChange={event => change('since', event.target.value)} size="sm" />
      <TextInput id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-until`} type="datetime-local" labelText={t('records.endTime')} value={draft.until} onChange={event => change('until', event.target.value)} size="sm" />
      {alerts && <><Select id="alert-read-filter" labelText={t('records.readState')} value={draft.isRead} onChange={event => change('isRead', event.target.value)} size="sm"><SelectItem value="" text={t('records.all')} /><SelectItem value="false" text={t('records.unread')} /><SelectItem value="true" text={t('records.read')} /></Select><Select id="alert-processed-filter" labelText={t('records.handlingState')} value={draft.processed} onChange={event => change('processed', event.target.value)} size="sm"><SelectItem value="" text={t('records.all')} /><SelectItem value="false" text={t('records.pending')} /><SelectItem value="true" text={t('records.handled')} /></Select></>}
      <div className="filter-actions"><Button type="submit" size="sm">{t('records.applyFilters')}</Button><Button kind="ghost" size="sm" onClick={() => { setDraft(initialFilters); setFilters(initialFilters); setError(undefined); setCursors([]); }}>{t('records.reset')}</Button></div>
    </form>
    <Notice error={error} />
    <section className="panel records-panel" aria-label={t('records.queryResults')}>
    <div className="table-toolbar"><div><Checkbox id={`${alerts ? 'alerts' : archive ? 'archive' : 'events'}-live`} labelText={t('records.liveUpdates')} checked={realtime} onChange={(_, { checked }) => setRealtime(checked)} />{state.updatedAt && <span className="helper">{t('records.updated', { time: timeOnly(state.updatedAt) })}</span>}</div><div className="actions"><Button kind="ghost" size="sm" onClick={state.refresh}>{t('records.refresh')}</Button><ExportButton kind={alerts ? 'alerts' : 'events'} filter={filter} isRead={filters.isRead === '' ? undefined : filters.isRead === 'true'} processed={filters.processed === '' ? undefined : filters.processed === 'true'} search={filters.search} archiveOnly={archive} /></div></div>
    <DataState loading={state.loading} error={state.error} empty={!state.data?.items.length} emptyTitle={alerts ? t('records.noMatchingAlerts') : t('records.noMatchingActivity')} emptyText={t('records.adjustTheFiltersCheckSettingsAndDiagnostics')}>
      {state.data && <GridTable label={alerts ? t('records.alertRecords') : archive ? t('records.archiveIndicatorRecords') : t('records.fileActivityRecords')} headings={alerts ? [t('records.latestEvidence'), t('records.process'), t('records.rule'), t('records.read2'), t('records.handling'), t('records.details')] : [t('records.sourceTime'), t('records.received'), t('records.process'), t('records.activity'), t('records.file'), t('records.details')]}>
        {state.data.items.map(item => {
          if (alerts) {
            const entry = item as AlertEntry;
            return <TableRow key={entry.alert.id}><TableCell>{timestamp(entry.alert.last_timestamp_ms)}</TableCell><TableCell>{processLabel(entry.alert.process)}</TableCell><TableCell>{ruleLabels[entry.alert.rule]}</TableCell><TableCell>{entry.is_read ? <span className="helper">{t('records.read')}</span> : <strong className="unread-state">{t('records.unread')}</strong>}</TableCell><TableCell><Tag type={entry.processed ? 'gray' : 'warm-gray'}>{entry.processed ? t('records.handled') : t('records.pending')}</Tag></TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => select({ kind: 'alert', id: entry.alert.id })}>{t('records.view')}</Button></TableCell></TableRow>;
          }
          const stored = item as StoredEvent;
          return <TableRow key={stored.id}><TableCell>{timestamp(stored.event.source_timestamp_ms)}</TableCell><TableCell>{timestamp(stored.event.received_timestamp_ms)}</TableCell><TableCell>{processLabel(stored.event.process)}</TableCell><TableCell>{eventLabel(stored.event)}</TableCell><TableCell className="path">{eventPath(stored.event)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => select({ kind: 'event', id: stored.id })}>{t('records.view')}</Button></TableCell></TableRow>;
        })}
      </GridTable>}
    </DataState>
    {state.data && <Pager total={state.data.total} count={state.data.items.length} cursor={cursor} page={cursors.length + 1} next={state.data.next_cursor} onPrevious={() => setCursors(current => current.slice(0, -1))} onNext={() => state.data?.next_cursor && setCursors(current => [...current, state.data!.next_cursor!])} />}
    </section>
    <p className="evidence-note">{alerts ? t('records.timeFiltersUseTheLatestEvidenceAlerts') : t('records.timeFiltersUseEventReceiveTimeOpens')}</p>
  </div>;
}
