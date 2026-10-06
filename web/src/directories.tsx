import { useState } from 'react';
import { Button, Checkbox, Tag, TextInput } from '@carbon/react';
import { api, consoleApi, controlApi } from './api';
import { ConfirmModal, DataState, GridTable, Notice, TableCell, TableRow } from './components';
import { sourceLabels, timestamp } from './format';
import { useAction, usePolling } from './hooks';
import type { Directory, HistoryPreview } from './types';

export function DirectoriesPage({ active }: { active: boolean }) {
  const state = usePolling<Directory[]>('/api/console', { action: 'directories' }, active, 1000);
  const [path, setPath] = useState('');
  const [preview, setPreview] = useState<HistoryPreview>();
  const [selected, setSelected] = useState<number[]>([]);
  const [remove, setRemove] = useState<Directory>();
  const [removePreview, setRemovePreview] = useState<{ removes_exclusion: boolean; coverage_may_expand: boolean; enabled_ancestors: string[]; affected_descendants: string[]; after: Directory[] }>();
  const action = useAction();
  const add = () => {
    if (!path.startsWith('/') || !path.trim()) { void action.run(() => Promise.reject(new Error('请输入以 / 开头的完整目录路径。'))); return; }
    void action.run(() => controlApi('add_directories', { entries: [{ path, sources: ['manual'] }] }), '目录已纳入监控配置。').then(result => { if (result) { setPath(''); state.refresh(); } });
  };
  const toggle = (directory: Directory) => void action.run(() => consoleApi('directory_set', { path: directory.path, enabled: !directory.enabled }), directory.enabled ? '目录已停用，整个子树成为排除项。' : '目录已启用。').then(() => state.refresh());
  const removeDirectory = () => void action.run(() => controlApi('remove_directory', { path: remove?.path }), '监控配置已移除，项目文件与历史记录保留。').then(result => { if (result) { setRemove(undefined); state.refresh(); } });
  return <div className="form-stack">
    <section className="panel"><h2>添加监控目录</h2><form className="directory-add" onSubmit={event => { event.preventDefault(); add(); }}><TextInput id="manual-directory" size="md" labelText="目录完整路径" value={path} onChange={event => setPath(event.target.value)} placeholder="/workspace/project-a" /><div className="actions"><Button kind="tertiary" size="md" disabled={action.busy} onClick={() => void action.run(() => api<{ path: string | null; cancelled: boolean }>('/api/directory/pick', {})).then(result => { if (result?.path) setPath(result.path); })}>选择目录</Button><Button type="submit" size="md" disabled={action.busy || !path}>添加</Button></div></form><p className="helper">只管理监控范围，不修改项目文件。停用项会排除其整个子树，优先于启用的父目录。</p></section>
    <Notice error={action.error} success={action.success} />
    <section className="panel"><div className="section-heading"><h2>当前目录</h2><span className="helper">{state.data?.length ?? '未知'} 项配置</span></div>
      <DataState loading={state.loading} error={state.error} empty={!state.data?.length} emptyTitle="尚未配置目录" emptyText="手动添加目录，或从下方的历史候选选择项目。">
        <GridTable label="监控目录配置" headings={['目录', '来源', '实际覆盖', '添加时间', '操作']}>
          {state.data?.map(directory => <TableRow key={directory.path}><TableCell className="path">{directory.path}{!directory.exists && <Tag type="warm-gray">路径不存在</Tag>}</TableCell><TableCell>{directory.sources.map(source => sourceLabels[source] ?? source).join('、')}</TableCell><TableCell>{!directory.enabled ? <strong>停用并排除子树</strong> : directory.effective ? '启用' : '被排除'}{directory.excluded_by && <p className="helper path">排除项：{directory.excluded_by}</p>}</TableCell><TableCell>{timestamp(directory.added_at_ms)}</TableCell><TableCell><div className="actions"><Button kind="ghost" size="sm" disabled={action.busy} onClick={() => toggle(directory)}>{directory.enabled ? '停用' : '启用'}</Button><Button kind="danger--ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => consoleApi<NonNullable<typeof removePreview>>('directory_remove_preview', { path: directory.path })).then(result => { if (result) { setRemovePreview(result); setRemove(directory); } })}>移除</Button></div></TableCell></TableRow>)}
        </GridTable>
      </DataState>
    </section>
    <section className="panel"><div className="section-heading"><div><h2>从历史会话发现目录</h2><p>Codex、Claude Code 与 ZCode 的目录元信息，本次选择后导入；新会话不会自动扩大监控范围。</p></div><Button kind="tertiary" size="sm" disabled={action.busy} onClick={() => void action.run(() => api<HistoryPreview>('/api/history/preview', {})).then(result => { if (result) { setPreview(result); setSelected([]); } })}>{action.busy ? '正在加载' : '预览候选'}</Button></div>
      {preview && <><div className="preview-summary"><span>候选 {preview.candidates.length} 项</span><span>覆盖缺口 {preview.gaps.reduce((sum, gap) => sum + gap.count, 0)} 项</span><Button kind="ghost" size="sm" onClick={() => setSelected(preview.candidates.flatMap((candidate, index) => candidate.status === 'available' ? [index] : []))}>选择可用候选</Button><Button kind="ghost" size="sm" onClick={() => setSelected([])}>取消选择</Button></div>
        <GridTable label="历史项目目录候选" headings={['选择', '目录', '状态', '来源']}>
          {preview.candidates.map((candidate, index) => <TableRow key={index}><TableCell><Checkbox id={`candidate-${index}`} labelText={`选择候选 ${index + 1}`} hideLabel checked={selected.includes(index)} disabled={candidate.status !== 'available'} onChange={(_, { checked }) => setSelected(current => checked ? [...current, index] : current.filter(value => value !== index))} /></TableCell><TableCell className="path">{candidate.canonical_path ?? candidate.raw_paths.join('、')}</TableCell><TableCell>{candidateStatus(candidate.status)}</TableCell><TableCell>{Array.from(new Set(candidate.origins.map(origin => sourceLabels[origin.source] ?? origin.source))).join('、')}</TableCell></TableRow>)}
        </GridTable>
        <div className="actions"><Button size="md" disabled={!selected.length || action.busy} onClick={() => void action.run(() => api('/api/history/import', { preview_id: preview.preview_id, indices: selected }), '选中的可用目录已导入。').then(result => { if (result) { setSelected([]); state.refresh(); } })}>导入选中 {selected.length} 项</Button></div>
        {preview.gaps.length > 0 && <details><summary>发现覆盖缺口</summary><ul className="path-list">{preview.gaps.map((gap, index) => <li key={index}>{sourceLabels[gap.source] ?? gap.source}：{gapLabel(gap.kind)}（{gap.count}）</li>)}</ul></details>}
        {preview.versions.length > 0 && <details><summary>已观察来源版本</summary><ul className="path-list">{preview.versions.map((version, index) => <li key={index}>{sourceLabels[version.source] ?? version.source} {version.version}：{version.compatibility_validated ? '已验证格式' : '格式兼容未验证'}</li>)}</ul></details>}
      </>}
    </section>
    <ConfirmModal open={Boolean(remove)} heading="移除监控配置" busy={action.busy} error={action.error} close={() => setRemove(undefined)} submit={removeDirectory} text={<><p className="path">{remove?.path}</p><p>只移除这一条目录配置，不删除项目文件或已有记录。</p><p>{removePreview?.coverage_may_expand ? '移除排除项后，子树可能重新被启用的父目录覆盖。' : '实际范围按剩余启用目录与排除项重新计算。'}</p><p>重叠父目录：{removePreview?.enabled_ancestors.join('、') || '无'}</p>{removePreview?.affected_descendants.length ? <><p>受影响的子目录配置：</p><ul className="path-list">{removePreview.affected_descendants.map(path => <li className="path" key={path}>{path}</li>)}</ul></> : null}</>} />
  </div>;
}

function candidateStatus(status: string): string { return ({ available: '可用', missing: '路径不存在', unresolved: '未解析', inaccessible: '无法访问', not_directory: '不是目录' } as Record<string, string>)[status] ?? status; }
function gapLabel(kind: string): string { return ({ missing_source: '来源不存在', io_error: '无法读取', malformed_record: '记录格式异常', incomplete_record: '目录信息不完整', record_too_long: '超长记录已跳过', unsupported_format: '格式不支持', missing_directory: '项目目录不存在', missing_version: '缺少版本', version_not_validated: '版本兼容未验证', directory_changes_not_validated: '目录变化格式未验证', read_timed_out: '读取超时', skipped_symlink: '已跳过符号链接' } as Record<string, string>)[kind] ?? kind; }
