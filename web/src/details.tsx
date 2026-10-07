import { t, useLocale } from './i18n';
import { useEffect, useState } from 'react';
import { Button, Tag, TextArea } from '@carbon/react';
import { Close } from '@carbon/react/icons';
import { consoleApi } from './api';
import { DataState, GridTable, Notice, TableCell, TableRow } from './components';
import { eventLabel, eventPath, number, processLabel, ruleLabels, timestamp } from './format';
import { useAction, usePolling } from './hooks';
import type { AlertEntry, EventDetail, StoredEvent } from './types';

export type Selection = { kind: 'event'; id: number } | { kind: 'alert'; id: string } | null;
export function DetailPane({ selection, select, close, viewRule, viewProcess }: { selection: Selection; select: (selection: Selection) => void; close: () => void; viewRule: () => void; viewProcess: (pid: number) => void }) {
  useLocale();
  if (!selection) return null;
  return <aside className="detail-pane" aria-label={t('details.recordDetails')}>
    <div className="section-heading detail-heading"><h2>{selection.kind === 'alert' ? t('details.alertDetails') : t('details.activityDetails')}</h2><Button kind="ghost" size="sm" renderIcon={Close} onClick={close}>{t('details.close')}</Button></div>
    <div className="detail-body">{selection.kind === 'alert' ? <AlertDetail id={selection.id} select={select} viewRule={viewRule} viewProcess={viewProcess} /> : <ActivityDetail id={selection.id} select={select} viewProcess={viewProcess} />}</div>
  </aside>;
}

function ActivityDetail({ id, select, viewProcess }: { id: number; select: (selection: Selection) => void; viewProcess: (pid: number) => void }) {
  useLocale();
  const state = usePolling<EventDetail>('/api/console', { action: 'event_detail', payload: { id } });
  return <DataState loading={state.loading} error={state.error} empty={!state.data}>
    {state.data && <><EventFields stored={state.data.event} /><Button kind="ghost" size="sm" onClick={() => viewProcess(state.data!.event.event.process.pid)}>{t('details.viewAllActivityForThisProcessID')}</Button><h3>{t('details.relatedAlerts')}</h3>{state.data.alerts.length ? state.data.alerts.map(entry => <Button key={entry.alert.id} kind="ghost" size="sm" onClick={() => select({ kind: 'alert', id: entry.alert.id })}>{ruleLabels[entry.alert.rule]}</Button>) : <p className="helper">{t('details.noRelatedAlerts')}</p>}</>}
  </DataState>;
}

function EventFields({ stored }: { stored: StoredEvent }) {
  useLocale();
  const event = stored.event;
  return <div className="form-stack"><h3>{eventLabel(event)}</h3><dl className="fields">
    <dt>{t('details.process')}</dt><dd>{processLabel(event.process)}</dd>
    <dt>{t('details.executable')}</dt><dd className="path">{event.process.executable ?? t('details.unknown')}</dd>
    <dt>{t('details.parentProcessID')}</dt><dd>{event.process.ppid ?? t('details.unknown')}</dd>
    <dt>{t('details.processVersion')}</dt><dd>{event.process.pid_version ?? t('details.unknown')}</dd>
    <dt>{t('details.signingIdentity')}</dt><dd>{event.process.signing_id ?? t('details.notCollected')}</dd>
    <dt>{t('details.teamIdentity')}</dt><dd>{event.process.team_id ?? t('details.notCollected')}</dd>
    <dt>{t('details.file')}</dt><dd className="path">{eventPath(event)}</dd>
    <dt>{t('details.destinationPath')}</dt><dd className="path">{event.destination ?? t('details.none')}</dd>
    <dt>{t('details.monitoredDirectories')}</dt><dd className="path">{stored.directories.join(t('common.listSeparator')) || t('details.associationUnknown')}</dd>
    <dt>{t('details.sourceTime')}</dt><dd>{timestamp(event.source_timestamp_ms)}</dd>
    <dt>{t('details.receivedTime')}</dt><dd>{timestamp(event.received_timestamp_ms)}</dd>
    <dt>{t('details.sourceStream')}</dt><dd>{event.source_stream}</dd>
    <dt>{t('details.formatVersion')}</dt><dd>{event.source_schema_version ?? t('details.unknown')} / {event.source_message_version ?? t('details.unknown')}</dd>
    <dt>{t('details.pathIntegrity')}</dt><dd>{event.file?.path_truncated ? t('details.pathTruncated') : t('details.noTruncationFlag')}</dd>
  </dl>
    {event.archive && <><h3>{t('details.commandAssociation')}</h3><dl className="fields"><dt>{t('details.tool')}</dt><dd>{event.archive.tool}</dd><dt>{t('details.inputs')}</dt><dd className="path">{event.archive.input_paths.join(t('common.listSeparator')) || t('details.unknown')}</dd><dt>{t('details.outputs')}</dt><dd className="path">{event.archive.output_paths?.join(t('common.listSeparator')) || event.archive.output_path || t('details.standardOutputOrUnknown')}</dd><dt>{t('details.workingDirectory')}</dt><dd className="path">{event.archive.cwd ?? t('details.notCollected')}</dd></dl></>}
    <p className="evidence-note">{t('details.opensAndMappingsAreAccessEvidenceNot')}</p>
  </div>;
}

function AlertDetail({ id, select, viewRule, viewProcess }: { id: string; select: (selection: Selection) => void; viewRule: () => void; viewProcess: (pid: number) => void }) {
  useLocale();
  const state = usePolling<AlertEntry>('/api/console', { action: 'alert_detail', payload: { id } });
  const [note, setNote] = useState('');
  const [dirty, setDirty] = useState(false);
  const action = useAction();
  useEffect(() => { setDirty(false); setNote(''); action.reset(); }, [id]);
  useEffect(() => { if (state.data && !dirty) setNote(state.data.note); }, [state.data?.note, dirty]);
  const update = (changes: object) => void action.run(() => {
    if ('note' in changes && new TextEncoder().encode(String(changes.note)).length > 2048) return Promise.reject(new Error('details.theHandlingNoteMustNotExceedBytes'));
    return consoleApi<AlertEntry>('alert_update', { id, expected_revision: state.data?.revision, ...changes });
  }, 'details.alertHandlingStateSaved').then(result => { if (result) { if ('note' in changes) { setDirty(false); setNote(result.note); } state.refresh(); } });
  const entry = state.data;
  return <DataState loading={state.loading} error={state.error} empty={!entry}>
    {entry && <div className="form-stack">
      <div className="section-heading"><h3>{ruleLabels[entry.alert.rule]}</h3><Tag type={entry.processed ? 'gray' : 'warm-gray'}>{entry.processed ? t('details.handled') : t('details.pending')}</Tag></div>
      <dl className="fields"><dt>{t('details.process')}</dt><dd>{processLabel(entry.alert.process)}</dd><dt>{t('details.monitoredDirectories')}</dt><dd className="path">{entry.alert.roots.join(t('common.listSeparator'))}</dd><dt>{t('details.readState')}</dt><dd>{entry.is_read ? t('details.read') : t('details.unread')}</dd><dt>{t('details.ruleVersion')}</dt><dd>{entry.rule_snapshot ? entry.rule_version : t('details.unknownLegacyRecord')}</dd><dt>{entry.alert.rule === 'bulk_file_access' ? t('details.uniqueAccessedFiles') : entry.alert.rule === 'archive_command' ? t('details.matchedInputPaths') : t('details.retainedOutputCandidates')}</dt><dd>{entry.alert.rule === 'archive_output' ? entry.alert.archive_output_paths?.length ? t('details.candidateFiles', { count: entry.alert.archive_output_paths.length, amount: number(entry.alert.archive_output_paths.length) }) : t('details.theLegacyRecordHasNoSeparateOutput') : number(entry.alert.unique_files)}</dd><dt>{t('details.activityCount')}</dt><dd>{number(entry.alert.activity_count)}</dd><dt>{t('details.firstGenerated')}</dt><dd>{timestamp(entry.alert.first_timestamp_ms)}</dd><dt>{t('details.latestEvidence')}</dt><dd>{timestamp(entry.alert.last_timestamp_ms)}</dd></dl>
      <h3>{t('details.relatedFiles')}</h3><ul className="path-list">{entry.alert.evidence_paths.map(path => <li className="path" key={path}>{path}</li>)}</ul>
      {!!entry.alert.archive_output_paths?.length && <><h3>{t('details.candidateCompressedArchiveOutputs')}</h3><ul className="path-list">{entry.alert.archive_output_paths.map(path => <li className="path" key={path}>{path}</li>)}</ul></>}
      <h3>{t('details.relatedActivity')}</h3>{entry.events?.length ? <GridTable label={t('details.activityRelatedToThisAlert')} headings={[t('details.receivedTime'), t('details.activity')]}>
        {entry.events.map(stored => <TableRow key={stored.id}><TableCell>{timestamp(stored.event.received_timestamp_ms)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => select({ kind: 'event', id: stored.id })}>{eventLabel(stored.event)}</Button></TableCell></TableRow>)}
      </GridTable> : <p className="helper">{t('details.noRelatedDetailsRetainedAssociationIndicatesOnly')}</p>}
      <p className="helper">{t('details.detailsShowUpToRelatedActivitiesOr')}</p>
      <Button kind="ghost" size="sm" onClick={() => viewProcess(entry.alert.process.pid)}>{t('details.viewAllActivityForThisProcessID')}</Button>
      <h3>{t('details.notificationFeedback')}</h3>{entry.notifications?.length ? <ul className="timeline">{entry.notifications.map(item => <li key={item.id}><strong>{notificationLabel(item.record.outcome)}</strong><span>{timestamp(item.record.observed_timestamp_ms)}</span><details><summary>{t('common.originalRecord')}</summary><code>{item.record.outcome}</code>{item.record.detail && <p>{t('common.originalText', { text: item.record.detail })}</p>}</details></li>)}</ul> : <p className="helper">{t('details.noNotificationFeedbackYetThisDoesNot')}</p>}
      <p className="evidence-note">{t('details.manualHandlingDoesNotMeanTheSystem')}</p>
      <TextArea id="alert-note" labelText={t('details.handlingNote')} helperText={t('common.userNote')} value={note} maxLength={2048} rows={3} onChange={event => { setNote(event.target.value); setDirty(true); }} />
      <Notice error={action.error} success={action.success} />
      <div className="actions"><Button kind="secondary" size="sm" disabled={action.busy} onClick={() => update({ is_read: !entry.is_read })}>{entry.is_read ? t('details.markUnread') : t('details.markRead')}</Button><Button size="sm" disabled={action.busy} onClick={() => update({ processed: !entry.processed, note })}>{entry.processed ? t('details.reopen') : t('details.markHandled')}</Button><Button kind="ghost" size="sm" disabled={!dirty || action.busy} onClick={() => update({ note })}>{t('details.saveNote')}</Button></div>
      {entry.rule_snapshot && <details><summary>{t('details.ruleParametersAtTheTime')}</summary><dl className="fields"><dt>{t('details.bulkThreshold')}</dt><dd>{t('common.files', { count: entry.rule_snapshot.bulk_file_threshold, amount: number(entry.rule_snapshot.bulk_file_threshold) })}</dd><dt>{t('details.countingWindow')}</dt><dd>{t('common.seconds', { count: entry.rule_snapshot.bulk_window_ms/ 1000, amount: number(entry.rule_snapshot.bulk_window_ms / 1000) })}</dd><dt>{t('details.mergeWindow')}</dt><dd>{t('common.seconds', { count: entry.rule_snapshot.alert_merge_window_ms/ 1000, amount: number(entry.rule_snapshot.alert_merge_window_ms / 1000) })}</dd><dt>{t('details.archiveCorrelation')}</dt><dd>{t('common.seconds', { count: entry.rule_snapshot.archive_correlation_window_ms/ 1000, amount: number(entry.rule_snapshot.archive_correlation_window_ms / 1000) })}</dd></dl></details>}
      <Button kind="ghost" size="sm" onClick={viewRule}>{t('details.viewCurrentRules')}</Button>
      <h3>{t('details.handlingHistory')}</h3>{entry.handling_history_truncated && <p className="helper">{t('details.onlyTheLatestHandlingActionsAreShown')}</p>}{entry.handling_history.length ? <ul className="timeline">{entry.handling_history.map((item, index) => <li key={index}><strong>{t('details.handlingStateSummary', { read: item.is_read ? t('details.read') : t('details.unread'), state: item.processed ? t('details.handled') : t('details.pending') })}</strong><span>{timestamp(item.timestamp_ms)}</span>{item.note && <p><span className="helper">{t('common.userNote')}: </span>{item.note}</p>}</li>)}</ul> : <p className="helper">{t('details.noManualHandlingHistoryYet')}</p>}
    </div>}
  </DataState>;
}

export function notificationLabel(outcome: string): string {
  return ({ sent: t('details.submittedDisplayUnconfirmed'), failed: t('details.submissionFailed'), deferred: t('details.deferred'), acknowledged: t('details.queueAcknowledgedDoesNotMeanAPerson') } as Record<string, string>)[outcome] ?? t('common.untranslatedValue', { value: outcome });
}
