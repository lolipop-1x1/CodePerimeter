import { useEffect, useState } from 'react';
import { Button, Tag, TextArea } from '@carbon/react';
import { Close } from '@carbon/react/icons';
import { consoleApi } from './api';
import { DataState, GridTable, Notice, TableCell, TableRow } from './components';
import { eventLabel, eventPath, processLabel, ruleLabels, timestamp } from './format';
import { useAction, usePolling } from './hooks';
import type { AlertEntry, EventDetail, StoredEvent } from './types';

export type Selection = { kind: 'event'; id: number } | { kind: 'alert'; id: string } | null;
export function DetailPane({ selection, select, close, viewRule, viewProcess }: { selection: Selection; select: (selection: Selection) => void; close: () => void; viewRule: () => void; viewProcess: (pid: number) => void }) {
  if (!selection) return null;
  return <aside className="detail-pane" aria-label="记录详情">
    <div className="section-heading detail-heading"><h2>{selection.kind === 'alert' ? '告警详情' : '活动详情'}</h2><Button kind="ghost" size="sm" renderIcon={Close} onClick={close}>关闭</Button></div>
    <div className="detail-body">{selection.kind === 'alert' ? <AlertDetail id={selection.id} select={select} viewRule={viewRule} viewProcess={viewProcess} /> : <ActivityDetail id={selection.id} select={select} viewProcess={viewProcess} />}</div>
  </aside>;
}

function ActivityDetail({ id, select, viewProcess }: { id: number; select: (selection: Selection) => void; viewProcess: (pid: number) => void }) {
  const state = usePolling<EventDetail>('/api/console', { action: 'event_detail', payload: { id } });
  return <DataState loading={state.loading} error={state.error} empty={!state.data}>
    {state.data && <><EventFields stored={state.data.event} /><Button kind="ghost" size="sm" onClick={() => viewProcess(state.data!.event.event.process.pid)}>查看此进程 ID 的全部活动</Button><h3>关联告警</h3>{state.data.alerts.length ? state.data.alerts.map(entry => <Button key={entry.alert.id} kind="ghost" size="sm" onClick={() => select({ kind: 'alert', id: entry.alert.id })}>{ruleLabels[entry.alert.rule]}</Button>) : <p className="helper">没有可关联的告警。</p>}</>}
  </DataState>;
}

function EventFields({ stored }: { stored: StoredEvent }) {
  const event = stored.event;
  return <div className="form-stack"><h3>{eventLabel(event)}</h3><dl className="fields">
    <dt>进程</dt><dd>{processLabel(event.process)}</dd>
    <dt>可执行文件</dt><dd className="path">{event.process.executable ?? '未知'}</dd>
    <dt>父进程 ID</dt><dd>{event.process.ppid ?? '未知'}</dd>
    <dt>进程版本</dt><dd>{event.process.pid_version ?? '未知'}</dd>
    <dt>签名身份</dt><dd>{event.process.signing_id ?? '未采集'}</dd>
    <dt>团队身份</dt><dd>{event.process.team_id ?? '未采集'}</dd>
    <dt>文件</dt><dd className="path">{eventPath(event)}</dd>
    <dt>目标路径</dt><dd className="path">{event.destination ?? '无'}</dd>
    <dt>监控目录</dt><dd className="path">{stored.directories.join('、') || '关联未知'}</dd>
    <dt>来源时间</dt><dd>{timestamp(event.source_timestamp_ms)}</dd>
    <dt>接收时间</dt><dd>{timestamp(event.received_timestamp_ms)}</dd>
    <dt>来源流</dt><dd>{event.source_stream}</dd>
    <dt>格式版本</dt><dd>{event.source_schema_version ?? '未知'} / {event.source_message_version ?? '未知'}</dd>
    <dt>路径完整性</dt><dd>{event.file?.path_truncated ? '路径被截断' : '没有截断标志'}</dd>
  </dl>
    {event.archive && <><h3>命令关联</h3><dl className="fields"><dt>工具</dt><dd>{event.archive.tool}</dd><dt>输入</dt><dd className="path">{event.archive.input_paths.join('、') || '未知'}</dd><dt>输出</dt><dd className="path">{event.archive.output_paths?.join('、') || event.archive.output_path || '标准输出或未知'}</dd><dt>工作目录</dt><dd className="path">{event.archive.cwd ?? '未采集'}</dd></dl></>}
    <p className="evidence-note">打开与映射是访问证据，不代表读完文件。归档命令及输出线索不证明压缩成功、文件内容或外传。</p>
  </div>;
}

function AlertDetail({ id, select, viewRule, viewProcess }: { id: string; select: (selection: Selection) => void; viewRule: () => void; viewProcess: (pid: number) => void }) {
  const state = usePolling<AlertEntry>('/api/console', { action: 'alert_detail', payload: { id } });
  const [note, setNote] = useState('');
  const [dirty, setDirty] = useState(false);
  const action = useAction();
  useEffect(() => { setDirty(false); setNote(''); action.reset(); }, [id]);
  useEffect(() => { if (state.data && !dirty) setNote(state.data.note); }, [state.data?.note, dirty]);
  const update = (changes: object) => void action.run(() => {
    if ('note' in changes && new TextEncoder().encode(String(changes.note)).length > 2048) return Promise.reject(new Error('处理备注不能超过 2048 字节，请缩短内容。'));
    return consoleApi<AlertEntry>('alert_update', { id, expected_revision: state.data?.revision, ...changes });
  }, '告警处理状态已保存。').then(result => { if (result) { if ('note' in changes) { setDirty(false); setNote(result.note); } state.refresh(); } });
  const entry = state.data;
  return <DataState loading={state.loading} error={state.error} empty={!entry}>
    {entry && <div className="form-stack">
      <div className="section-heading"><h3>{ruleLabels[entry.alert.rule]}</h3><Tag type={entry.processed ? 'gray' : 'warm-gray'}>{entry.processed ? '已处理' : '待处理'}</Tag></div>
      <dl className="fields"><dt>进程</dt><dd>{processLabel(entry.alert.process)}</dd><dt>监控目录</dt><dd className="path">{entry.alert.roots.join('、')}</dd><dt>阅读状态</dt><dd>{entry.is_read ? '已读' : '未读'}</dd><dt>规则版本</dt><dd>{entry.rule_snapshot ? entry.rule_version : '未知（旧记录）'}</dd><dt>{entry.alert.rule === 'bulk_file_access' ? '唯一访问文件数' : entry.alert.rule === 'archive_command' ? '匹配的输入路径数' : '保留的候选输出'}</dt><dd>{entry.alert.rule === 'archive_output' ? entry.alert.archive_output_paths?.length ? `${entry.alert.archive_output_paths.length} 个候选文件` : '旧记录未单独保存输出列表' : entry.alert.unique_files}</dd><dt>活动数</dt><dd>{entry.alert.activity_count}</dd><dt>首次生成</dt><dd>{timestamp(entry.alert.first_timestamp_ms)}</dd><dt>最近证据</dt><dd>{timestamp(entry.alert.last_timestamp_ms)}</dd></dl>
      <h3>关联文件</h3><ul className="path-list">{entry.alert.evidence_paths.map(path => <li className="path" key={path}>{path}</li>)}</ul>
      {!!entry.alert.archive_output_paths?.length && <><h3>候选压缩／归档输出</h3><ul className="path-list">{entry.alert.archive_output_paths.map(path => <li className="path" key={path}>{path}</li>)}</ul></>}
      <h3>关联活动</h3>{entry.events?.length ? <GridTable label="告警关联活动" headings={['接收时间', '活动']}>
        {entry.events.map(stored => <TableRow key={stored.id}><TableCell>{timestamp(stored.event.received_timestamp_ms)}</TableCell><TableCell><Button kind="ghost" size="sm" onClick={() => select({ kind: 'event', id: stored.id })}>{eventLabel(stored.event)}</Button></TableCell></TableRow>)}
      </GridTable> : <p className="helper">没有保留的关联明细。关联仅表示时间、进程和目录线索。</p>}
      <p className="helper">详情最多展示 100 条关联活动或通知。按进程 ID 查询更多历史时，可能包括该 ID 的其他运行实例。</p>
      <Button kind="ghost" size="sm" onClick={() => viewProcess(entry.alert.process.pid)}>查看此进程 ID 的全部活动</Button>
      <h3>通知反馈</h3>{entry.notifications?.length ? <ul className="timeline">{entry.notifications.map(item => <li key={item.id}><strong>{notificationLabel(item.record.outcome)}</strong><span>{timestamp(item.record.observed_timestamp_ms)}</span></li>)}</ul> : <p className="helper">尚无通知反馈，不能据此判断已展示。</p>}
      <p className="evidence-note">人工处理不表示系统已经拦截或风险已消除。新证据会重新标为未读、待处理，原备注与历史保留。</p>
      <TextArea id="alert-note" labelText="处理备注" value={note} maxLength={2048} rows={3} onChange={event => { setNote(event.target.value); setDirty(true); }} />
      <Notice error={action.error} success={action.success} />
      <div className="actions"><Button kind="secondary" size="sm" disabled={action.busy} onClick={() => update({ is_read: !entry.is_read })}>{entry.is_read ? '标记未读' : '标记已读'}</Button><Button size="sm" disabled={action.busy} onClick={() => update({ processed: !entry.processed, note })}>{entry.processed ? '重新打开' : '标记已处理'}</Button><Button kind="ghost" size="sm" disabled={!dirty || action.busy} onClick={() => update({ note })}>保存备注</Button></div>
      {entry.rule_snapshot && <details><summary>当时的规则参数</summary><dl className="fields"><dt>批量阈值</dt><dd>{entry.rule_snapshot.bulk_file_threshold} 个文件</dd><dt>统计窗口</dt><dd>{entry.rule_snapshot.bulk_window_ms / 1000} 秒</dd><dt>合并窗口</dt><dd>{entry.rule_snapshot.alert_merge_window_ms / 1000} 秒</dd><dt>归档关联</dt><dd>{entry.rule_snapshot.archive_correlation_window_ms / 1000} 秒</dd></dl></details>}
      <Button kind="ghost" size="sm" onClick={viewRule}>查看当前规则</Button>
      <h3>处理历史</h3>{entry.handling_history_truncated && <p className="helper">仅展示最近 100 次处理，完整处理记录仍保留。</p>}{entry.handling_history.length ? <ul className="timeline">{entry.handling_history.map((item, index) => <li key={index}><strong>{item.is_read ? '已读' : '未读'}，{item.processed ? '已处理' : '待处理'}</strong><span>{timestamp(item.timestamp_ms)}</span>{item.note && <p>{item.note}</p>}</li>)}</ul> : <p className="helper">尚无人工处理记录。</p>}
    </div>}
  </DataState>;
}

export function notificationLabel(outcome: string): string {
  return ({ sent: '已发送，到屏待确认', failed: '发送失败', deferred: '延后发送', acknowledged: '队列已确认，不代表人工已读' } as Record<string, string>)[outcome] ?? outcome;
}
