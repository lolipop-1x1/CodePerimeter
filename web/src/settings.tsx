import { i18n, t, useLocale } from './i18n';
import { useEffect, useState } from 'react';
import { Button, TextInput } from '@carbon/react';
import { consoleApi, errorMessage } from './api';
import { ConfirmModal, DataState, ExportButton, GridTable, Notice, TableCell, TableRow } from './components';
import { notificationLabel } from './details';
import { number, timestamp } from './format';
import { useAction, usePolling } from './hooks';
import { stateLabel } from './overview';
import type { CollectorStreamSample, Health, NotificationRecord, Retention, ServiceOperation, Status } from './types';

export type ServiceAction = 'install' | 'start' | 'pause' | 'resume' | 'uninstall';
export const serviceLabels: Record<ServiceAction, string> = { get install() { return t('settings.installService'); }, get start() { return t('settings.enableMonitoring'); }, get pause() { return t('settings.pauseMonitoring'); }, get resume() { return t('settings.resumeMonitoring'); }, get uninstall() { return t('settings.uninstallService'); } };
export function SettingsPage({ active, status, operation, operate, operationError }: { active: boolean; status?: Status; operation?: ServiceOperation; operate: (action: ServiceAction) => void; operationError?: string }) {
  useLocale();
  const retention = usePolling<Retention>('/api/console', { action: 'retention_get' }, active, 2000);
  const health = usePolling<Health[]>('/api/control', { operation: 'query_health', payload: { filter: { limit: 50 } } }, active, 1000);
  const notifications = usePolling<NotificationRecord[]>('/api/control', { operation: 'query_notifications', payload: { filter: { limit: 50 } } }, active, 1000);
  const [days, setDays] = useState('');
  const [daysDirty, setDaysDirty] = useState(false);
  const [confirm, setConfirm] = useState<'retention' | 'details' | 'cumulative' | 'uninstall'>();
  const [preview, setPreview] = useState<{ counts: Record<string, number>; preview_revision: number }>();
  const action = useAction();
  useEffect(() => { if (retention.data && !daysDirty) setDays(String(retention.data.days)); }, [retention.data?.days, daysDirty]);
  const setRetention = (confirmation = false) => void action.run(() => consoleApi<Retention>('retention_set', { days: Number(days), confirm: confirmation, preview_revision: confirmation ? preview?.preview_revision : undefined }), 'settings.detailRetentionSaved').then(result => { if (result) { setDaysDirty(false); setConfirm(undefined); retention.refresh(); } });
  const prepareRetention = () => {
    if (!/^\d+$/.test(days) || Number(days) < 1 || Number(days) > 3650) { void action.run(() => Promise.reject(new Error('settings.retentionMustBeAnIntegerFromTo'))); return; }
    if (retention.data && Number(days) < retention.data.days) {
      void action.run(() => consoleApi<{ counts: Record<string, number>; preview_revision: number }>('retention_preview', { days: Number(days) })).then(result => { if (result) { setPreview(result); setConfirm('retention'); } });
    } else setRetention();
  };
  const clear = (kind: 'details' | 'cumulative') => void action.run(() => consoleApi(kind === 'details' ? 'clear_details' : 'clear_cumulative', { confirm: true }), kind === 'details' ? 'settings.detailsClearedDirectoryAndRuleSettingsRetained' : 'settings.cumulativeStatisticsCleared').then(result => { if (result) { setConfirm(undefined); retention.refresh(); health.refresh(); notifications.refresh(); } });
  const busy = operation?.state === 'running';
  const service = status?.service.status_error ? undefined : status?.service;
  return <div className="form-stack">
    <section className="panel service-panel"><h2>{t('settings.servicesAndSystemAuthorization')}</h2><p>{t('settings.backgroundMonitoringRunsIndependentlyOfTheWeb')}</p>
      <dl className="fields"><dt>{t('settings.serviceInstallation')}</dt><dd>{status ? status.service.status_error ? t('settings.stateUnknown') : status.service.installed ? status.service.installed_binary_trusted ? t('settings.installedInstallationPermissionsProtected') : t('settings.installedInstallationPermissionsUnverified') : t('settings.notInstalled') : t('settings.loading')}</dd><dt>{t('settings.systemCollectionJob')}</dt><dd>{jobLabel(service?.collector)}</dd><dt>{t('settings.managementHostJob')}</dt><dd>{jobLabel(service?.analyzer)}</dd><dt>{t('settings.notificationJob')}</dt><dd>{jobLabel(service?.notification)}</dd><dt>{t('settings.pauseIntent')}</dt><dd>{service?.paused == null ? t('settings.unknown') : service.paused ? t('settings.pausedRetainedAcrossRestarts') : t('settings.notPaused')}</dd><dt>{t('settings.actualCollectionHealth')}</dt><dd>{status?.host ? stateLabel(status.host.collector_state) : t('settings.hostUnavailableCannotDetermine')}</dd></dl>
      <div className="actions">{(['install', 'start', 'pause', 'resume'] as ServiceAction[]).map(item => <Button key={item} kind={item === 'start' ? 'primary' : 'tertiary'} size="sm" disabled={busy || !status?.service_actions_enabled} onClick={() => operate(item)}>{serviceLabels[item]}</Button>)}<Button kind="danger--ghost" size="sm" disabled={busy || !status?.service_actions_enabled} onClick={() => setConfirm('uninstall')}>{t('settings.uninstallService')}</Button></div>
      {!status?.service_actions_enabled && <p className="helper">{t('settings.systemServiceOperationsAreDisabledInThis')}</p>}
      <Notice error={operationError ?? (status?.service.status_error ? errorMessage(status.service.status_error) : undefined)} success={operation?.state === 'succeeded' ? t('settings.serviceOperationCompletedCheckCollectionAndStorage') : operation?.state === 'cancelled' ? t('settings.systemAuthorizationCancelledSuccessWasNotReported') : undefined} />
      {busy && <div className="operation-progress" role="status">{t('settings.serviceProgress', { operation: serviceLabels[operation.operation as ServiceAction] ?? t('settings.serviceOperation') })}</div>}
      {operation?.report && <ul aria-label={t('settings.serviceOperationSteps')}>{operation.report.steps.map((step, index) => <li key={index}>{step.success ? t('settings.succeeded') : t('settings.failed')}: <span>{t('common.originalText', { text: step.message })}</span></li>)}</ul>}
      <details><summary>{t('settings.fullDiskAccessGuidance')}</summary><p>{t('settings.openMacOSSystemSettingsPrivacySecurityFull')}</p><p>{t('settings.resumeCollectionAfterAuthorizationAndCheckNew')}</p></details>
      <details><summary>{t('settings.systemNotificationPermissionGuidance')}</summary><p>{t('settings.allowCodePerimeterNotificationsInTheMacOSPrompt')}</p><p>{t('settings.disablingSystemNotificationsDoesNotAffectCollection')}</p></details>
    </section>
    <section className="panel"><h2>{t('settings.retentionAndExports')}</h2><DataState loading={retention.loading} error={retention.error} empty={!retention.data}>
      {retention.data && <><div className="retention-form"><TextInput id="retention-days" size="md" type="number" min={1} max={3650} step={1} labelText={t('settings.daysToRetainDetails')} helperText={t('settings.defaultDaysSetToDaysCumulativeStatistics')} value={days} onChange={event => { setDays(event.target.value); setDaysDirty(true); }} /><Button size="md" disabled={!daysDirty || action.busy} onClick={prepareRetention}>{t('settings.saveRetention')}</Button></div><dl className="count-grid">{Object.entries(retention.data.counts).map(([kind, count]) => <div key={kind}><dt>{countLabel(kind)}</dt><dd>{number(count)}</dd></div>)}</dl></>}
    </DataState><div className="actions"><span>{t('settings.allFileActivity')}</span><ExportButton kind="events" filter={{}} /><span>{t('settings.allAlerts')}</span><ExportButton kind="alerts" filter={{}} /></div><p className="helper">{t('settings.recordPageExportsFollowCurrentFiltersExports')}</p></section>
    <section className="panel danger-panel"><h2>{t('settings.clearRecords')}</h2><p>{t('settings.clearDetailsAndCumulativeStatisticsSeparatelyDirectory')}</p><div className="actions"><Button kind="danger--tertiary" size="sm" disabled={!retention.data || action.busy} onClick={() => { action.reset(); setConfirm('details'); }}>{t('settings.clearDetails')}</Button><Button kind="danger--tertiary" size="sm" disabled={!retention.data || action.busy} onClick={() => { action.reset(); setConfirm('cumulative'); }}>{t('settings.clearCumulativeStatistics')}</Button></div><Notice error={action.error} success={action.success} /></section>
    <section className="panel"><h2>{t('settings.collectionAndEvidenceStorageDiagnostics')}</h2>{status?.host && <dl className="fields"><dt>{t('settings.lastEventReceived')}</dt><dd>{timestamp(status.host.last_event_received_ms)}</dd><dt>{t('settings.collectorDroppedLines')}</dt><dd>{number(status.host.collector_dropped_lines)}</dd><dt>{t('settings.hostDroppedFrames')}</dt><dd>{number(status.host.reader_dropped_frames)}</dd><dt>{t('settings.evidenceStorageGaps')}</dt><dd>{number(status.host.database_gap_events)}</dd><dt>{t('settings.droppedInMemoryNotifications')}</dt><dd>{number(status.host.memory_dropped_notifications)}</dd><dt>{t('settings.notificationHeartbeat')}</dt><dd>{timestamp(status.host.notify_session_last_seen_ms)}</dd><dt>{t('settings.retentionCleanupState')}</dt><dd>{stateLabel(status.host.retention_state)}</dd></dl>}
      <CollectorSourceDiagnostics streams={status?.host?.collector_streams} />
      <DataState loading={health.loading} error={health.error} empty={!health.data?.length} emptyTitle={t('settings.noHealthRecordsYet')} emptyText={t('settings.noSavedDiagnosticsYetThisDoesNot')}><GridTable label={t('settings.componentHealthDiagnostics')} headings={[t('settings.time'), t('settings.component'), t('settings.state'), t('settings.reason')]}>
        {health.data?.map(item => <TableRow key={item.id}><TableCell>{timestamp(item.record.observed_timestamp_ms)}</TableCell><TableCell>{item.record.component}</TableCell><TableCell>{stateLabel(item.record.state)}</TableCell><TableCell><p>{healthExplanation(item.record.code)}</p><details><summary>{t('common.originalRecord')}</summary><code>{item.record.code}</code>{item.record.detail && <p className="helper">{t('common.originalText', { text: item.record.detail })}</p>}</details></TableCell></TableRow>)}
      </GridTable></DataState></section>
    <section className="panel"><h2>{t('settings.notificationSubmissionFeedback')}</h2><DataState loading={notifications.loading} error={notifications.error} empty={!notifications.data?.length} emptyTitle={t('settings.noNotificationFeedbackYet')} emptyText={t('settings.actualNotificationDisplayNeedsIndependentObservationA')}><GridTable label={t('settings.notificationSubmissionFeedback')} headings={[t('settings.time'), t('settings.alert'), t('settings.feedback')]}>
      {notifications.data?.map(item => <TableRow key={item.id}><TableCell>{timestamp(item.record.observed_timestamp_ms)}</TableCell><TableCell className="path">{item.record.alert_id}</TableCell><TableCell>{notificationLabel(item.record.outcome)}</TableCell></TableRow>)}
    </GridTable></DataState></section>
    <ConfirmModal open={Boolean(confirm)} heading={confirm === 'retention' ? t('settings.shortenDetailRetention') : confirm === 'uninstall' ? t('settings.uninstallBackgroundService') : confirm === 'details' ? t('settings.clearDetailRecords') : t('settings.clearCumulativeStatistics')} busy={action.busy || busy} error={action.error} close={() => setConfirm(undefined)} submit={() => { if (confirm === 'retention') setRetention(true); else if (confirm === 'uninstall') { operate('uninstall'); setConfirm(undefined); } else if (confirm) clear(confirm); }} text={confirm === 'retention' ? <><p>{t('settings.retentionShorten', { before: number(retention.data?.days), after: number(Number(days)) })}</p><dl className="count-grid">{Object.entries(preview?.counts ?? {}).map(([kind, count]) => <div key={kind}><dt>{countLabel(kind)}</dt><dd>{number(count)}</dd></div>)}</dl><p>{t('settings.relatedHandlingRecordsAreClearedTogetherCumulative')}</p></> : confirm === 'uninstall' ? <p>{t('settings.stopAndUninstallBackgroundMonitoringJobsSettings')}</p> : confirm === 'details' ? <><p>{t('settings.clearAllCurrentDetailsAndRelatedHandling')}</p><dl className="count-grid">{Object.entries(retention.data?.counts ?? {}).map(([kind, count]) => <div key={kind}><dt>{countLabel(kind)}</dt><dd>{number(count)}</dd></div>)}</dl><p>{t('settings.settingsAndCumulativeStatisticsRemainDeletedDetails')}</p></> : <p>{t('settings.resetCumulativeActivityAlertHealthAndNotification')}</p>} />
  </div>;
}
export function CollectorSourceDiagnostics({ streams }: { streams?: Record<string, CollectorStreamSample> }) {
  useLocale();
  const samples = Object.entries(streams ?? {});
  return <div className="form-stack"><h3>{t('settings.latestCollectionSourceSamples')}</h3><p className="helper">{t('settings.sourceToReceiveDifferenceForTheLatest')}</p>
    {samples.length ? <GridTable label={t('settings.latestCollectionSourceSamples')} headings={[t('settings.collectionSource'), t('settings.sourceToReceiveDelay'), t('settings.sampleReceived')]}>
      {samples.map(([source, sample]) => <TableRow key={source}><TableCell>{({ exec: t('settings.commands'), read: t('settings.opensMappings'), write: t('settings.writes'), activity: t('settings.fileActivity'), combined: t('settings.combined') } as Record<string, string>)[source] ?? t('settings.unknownSource')}</TableCell><TableCell>{latestSourceLatency(sample)}</TableCell><TableCell>{typeof sample.last_received_timestamp_ms === 'number' && Number.isFinite(sample.last_received_timestamp_ms) ? timestamp(sample.last_received_timestamp_ms) : t('settings.unknown')}</TableCell></TableRow>)}
    </GridTable> : <p className="helper">{t('settings.noSourceSamplesDelayAndReceiveTime')}</p>}
  </div>;
}
function latestSourceLatency(sample: CollectorStreamSample): string {
  const source = sample.last_source_timestamp_ms, received = sample.last_received_timestamp_ms;
  if (typeof source !== 'number' || typeof received !== 'number' || !Number.isFinite(source) || !Number.isFinite(received)) return t('settings.unknown');
  const difference = received - source;
  return difference < 0 ? t('settings.clockAnomalyMs', { delay: number(difference) }) : `${number(difference)} ms`;
}
function jobLabel(job?: Status['service']['collector']): string {
  if (!job) return t('settings.unknown');
  const parts = [t(job.loaded ? 'settings.loaded' : 'settings.notLoaded'), t(job.running ? 'settings.running' : 'settings.notRunning')];
  if (job.disabled === true) parts.push(t('settings.disabled'));
  if (job.last_exit_code) parts.push(t('settings.exitCode', { code: job.last_exit_code }));
  return parts.join(t('common.listSeparator'));
}
function countLabel(kind: string): string { return ({ events: t('settings.fileActivity'), alerts: t('settings.alert'), health_records: t('settings.healthRecords'), notifications: t('settings.notificationFeedback'), handling_records: t('settings.handlingRecords'), outbox_entries: t('settings.notificationQueue') } as Record<string, string>)[kind] ?? kind; }
export function healthExplanation(code: string): string {
  if (i18n.exists(`health.${code}`)) return t(`health.${code}`);
  if (['install', 'start', 'pause', 'resume', 'uninstall'].includes(code)) return serviceLabels[code as ServiceAction];
  return t('common.unknownExplanation');
}
