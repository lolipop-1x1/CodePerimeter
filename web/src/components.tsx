import { useId, useState, type ReactNode } from 'react';
import { Button, Checkbox, InlineNotification, Modal, Select, SelectItem, SkeletonText, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@carbon/react';
import { Document } from '@carbon/react/icons';
import { downloadExport } from './api';
import { useAction } from './hooks';

export function Notice({ error, success }: { error?: string; success?: string }) {
  if (!error && !success) return null;
  return <InlineNotification className="notice" kind={error ? 'error' : 'success'} title={error ? '操作未完成' : '操作完成'} subtitle={error ?? success} hideCloseButton lowContrast />;
}

export function DataState({ loading, error, empty, children, emptyTitle = '暂无记录', emptyText = '监控产生新记录后会在这里显示。' }: {
  loading: boolean; error?: string; empty: boolean; children: ReactNode; emptyTitle?: string; emptyText?: string;
}) {
  if (loading) return <div aria-busy="true" aria-label="正在加载" className="loading"><SkeletonText heading /><SkeletonText paragraph lineCount={4} /></div>;
  if (error) return <Notice error={error} />;
  if (empty) return <div className="empty-state"><Document size={28} aria-hidden="true" /><h3>{emptyTitle}</h3><p>{emptyText}</p></div>;
  return <>{children}</>;
}

export function GridTable({ headings, children, label }: { headings: string[]; children: ReactNode; label: string }) {
  return <div className="table-scroll" role="region" aria-label={`${label}，可滚动表格`} tabIndex={0}><Table size="md" aria-label={label} useZebraStyles={false}>
    <TableHead><TableRow>{headings.map(heading => <TableHeader key={heading}>{heading}</TableHeader>)}</TableRow></TableHead>
    <TableBody>{children}</TableBody>
  </Table></div>;
}

export { TableRow, TableCell };

export function Pager({ cursor, next, total, page, count, onNext, onPrevious }: { cursor?: string; next: string | null; total: number; page: number; count: number; onNext: () => void; onPrevious: () => void }) {
  return <div className="pager" aria-label="分页"><span>共 {total.toLocaleString('zh-CN')} 条，第 {page} 页，本页 {count} 条</span><div>
    <Button kind="ghost" size="sm" disabled={!cursor} onClick={onPrevious}>上一页</Button>
    <Button kind="ghost" size="sm" disabled={!next} onClick={onNext}>下一页</Button>
  </div></div>;
}

export function ExportButton({ kind, filter, search, archiveOnly, isRead, processed }: { kind: 'events' | 'alerts'; filter: object; search?: string; archiveOnly?: boolean; isRead?: boolean; processed?: boolean }) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [format, setFormat] = useState('json');
  const [anonymous, setAnonymous] = useState(true);
  const action = useAction();
  return <>
    <Button kind="tertiary" size="sm" onClick={() => { action.reset(); setOpen(true); }}>导出</Button>
    <Modal open={open} selectorPrimaryFocus="select" closeButtonLabel="关闭" onRequestClose={() => !action.busy && setOpen(false)} modalHeading="导出当前筛选范围" primaryButtonText={action.busy ? '正在导出' : '下载'} secondaryButtonText="取消" primaryButtonDisabled={action.busy} onRequestSubmit={() => void action.run(async () => { await downloadExport({ kind, filter, search: search || undefined, archive_only: archiveOnly, is_read: isRead, processed, format, anonymous }); return true; }).then(result => { if (result) setOpen(false); })}>
      <p>导出包含当前筛选范围的全部记录，不限于当前页。下载文件只保存在你选择的本机位置。</p>
      <div className="form-stack"><Select id={`${id}-export-format`} labelText="文件格式" value={format} onChange={event => setFormat(event.target.value)}><SelectItem value="json" text="JSON" /><SelectItem value="csv" text="CSV" /></Select>
        <Checkbox id={`${id}-export-anonymous`} labelText="匿名分享模式" checked={anonymous} onChange={(_, { checked }) => setAnonymous(checked)} />
        <p className="helper">匿名模式替换路径与进程身份，移除处理备注等自由文本；关闭后导出完整字段。</p>
        <Notice error={action.error} />
      </div>
    </Modal>
  </>;
}

export function ConfirmModal({ open, heading, text, busy, error, submit, close }: { open: boolean; heading: string; text: ReactNode; busy: boolean; error?: string; submit: () => void; close: () => void }) {
  return <Modal open={open} danger closeButtonLabel="关闭" modalHeading={heading} primaryButtonText={busy ? '正在操作' : '确认'} secondaryButtonText="取消" primaryButtonDisabled={busy} onRequestClose={() => !busy && close()} onRequestSubmit={submit}>
    <div className="form-stack">{text}<Notice error={error} /></div>
  </Modal>;
}
