import { i18n, t, useLocale } from './i18n';
import { useId, useState, type ReactNode } from 'react';
import { Button, Checkbox, InlineNotification, Modal, Select, SelectItem, SkeletonText, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@carbon/react';
import { Document } from '@carbon/react/icons';
import { number } from './format';
import { downloadExport } from './api';
import { useAction } from './hooks';

export function Notice({ error, success }: { error?: string; success?: string }) {
  useLocale();
  if (!error && !success) return null;
  const message = error ?? success;
  return <InlineNotification className="notice" kind={error ? 'error' : 'success'} statusIconDescription={error ? t('components.operationIncomplete') : t('components.operationComplete')} title={error ? t('components.operationIncomplete') : t('components.operationComplete')} subtitle={message && i18n.exists(message) ? t(message) : message} hideCloseButton lowContrast />;
}

export function DataState({ loading, error, empty, children, emptyTitle = t('components.noRecordsYet'), emptyText = t('components.newMonitoringRecordsWillAppearHere') }: {
  loading: boolean; error?: string; empty: boolean; children: ReactNode; emptyTitle?: string; emptyText?: string;
}) {
  useLocale();
  if (loading) return <div aria-busy="true" aria-label={t('components.loading')} className="loading"><SkeletonText heading /><SkeletonText paragraph lineCount={4} /></div>;
  if (error) return <Notice error={error} />;
  if (empty) return <div className="empty-state"><Document size={28} aria-hidden="true" /><h3>{emptyTitle}</h3><p>{emptyText}</p></div>;
  return <>{children}</>;
}

export function GridTable({ headings, children, label }: { headings: string[]; children: ReactNode; label: string }) {
  useLocale();
  return <div className="table-scroll" role="region" aria-label={t('components.scrollableTable', { label: label })} tabIndex={0}><Table size="md" aria-label={label} useZebraStyles={false}>
    <TableHead><TableRow>{headings.map(heading => <TableHeader key={heading}>{heading}</TableHeader>)}</TableRow></TableHead>
    <TableBody>{children}</TableBody>
  </Table></div>;
}

export { TableRow, TableCell };

export function Pager({ cursor, next, total, page, count, onNext, onPrevious }: { cursor?: string; next: string | null; total: number; page: number; count: number; onNext: () => void; onPrevious: () => void }) {
  useLocale();
  return <div className="pager" aria-label={t('components.pagination')}><span>{t('pagination.summary', { total: number(total), page: number(page), pageCount: number(count) })}</span><div>
    <Button kind="ghost" size="sm" disabled={!cursor} onClick={onPrevious}>{t('components.previous')}</Button>
    <Button kind="ghost" size="sm" disabled={!next} onClick={onNext}>{t('components.next')}</Button>
  </div></div>;
}

export function ExportButton({ kind, filter, search, archiveOnly, isRead, processed }: { kind: 'events' | 'alerts'; filter: object; search?: string; archiveOnly?: boolean; isRead?: boolean; processed?: boolean }) {
  useLocale();
  const id = useId();
  const [open, setOpen] = useState(false);
  const [format, setFormat] = useState('json');
  const [anonymous, setAnonymous] = useState(true);
  const action = useAction();
  return <>
    <Button kind="tertiary" size="sm" onClick={() => { action.reset(); setOpen(true); }}>{t('components.export')}</Button>
    <Modal open={open} selectorPrimaryFocus="select" closeButtonLabel={t('components.close')} onRequestClose={() => !action.busy && setOpen(false)} modalHeading={t('components.exportCurrentResults')} primaryButtonText={action.busy ? t('components.exporting') : t('components.download')} secondaryButtonText={t('components.cancel')} primaryButtonDisabled={action.busy} onRequestSubmit={() => void action.run(async () => { await downloadExport({ kind, filter, search: search || undefined, archive_only: archiveOnly, is_read: isRead, processed, format, anonymous }); return true; }).then(result => { if (result) setOpen(false); })}>
      <p>{t('components.theExportIncludesAllRecordsMatchingThe')}</p>
      <div className="form-stack"><Select id={`${id}-export-format`} labelText={t('components.fileFormat')} value={format} onChange={event => setFormat(event.target.value)}><SelectItem value="json" text="JSON" /><SelectItem value="csv" text="CSV" /></Select>
        <Checkbox id={`${id}-export-anonymous`} labelText={t('components.anonymousSharingMode')} checked={anonymous} onChange={(_, { checked }) => setAnonymous(checked)} />
        <p className="helper">{t('components.anonymousModeReplacesPathsAndProcessIdentities')}</p>
        <Notice error={action.error} />
      </div>
    </Modal>
  </>;
}

export function ConfirmModal({ open, heading, text, busy, error, submit, close }: { open: boolean; heading: string; text: ReactNode; busy: boolean; error?: string; submit: () => void; close: () => void }) {
  useLocale();
  return <Modal open={open} danger closeButtonLabel={t('components.close')} modalHeading={heading} primaryButtonText={busy ? t('components.working') : t('components.confirm')} secondaryButtonText={t('components.cancel')} primaryButtonDisabled={busy} onRequestClose={() => !busy && close()} onRequestSubmit={submit}>
    <div className="form-stack">{text}<Notice error={error} /></div>
  </Modal>;
}
