import test from 'node:test';
import assert from 'node:assert/strict';
import { buildFilter, eventLabel, eventPath, primaryServiceAction, validateFilters } from '../src/format.ts';
import { errorMessage } from '../src/api.ts';
import type { ActivityEvent, Status } from '../src/types.ts';

test('访问标签保留证据边界，不把打开与映射表示成读取完成', () => {
  const event = { kind: 'open', file: { readable: true, path: '/workspace/project-a/source.txt' }, archive: null } as ActivityEvent;
  assert.equal(eventLabel(event), '可读打开');
  assert.equal(eventLabel({ ...event, kind: 'mmap' }), '可读映射');
  assert.equal(eventLabel({ ...event, file: { ...event.file!, readable: null } }), '文件打开');
  assert.equal(eventPath(event), '/workspace/project-a/source.txt');
});

test('压缩命令标签始终是迹象', () => {
  const event = { kind: 'exec', file: null, archive: { tool: 'gzip', input_paths: ['/workspace/project-a/source.txt'], output_path: null, cwd: null } } as ActivityEvent;
  assert.equal(eventLabel(event), '归档命令迹象');
  assert.equal(eventPath(event), '/workspace/project-a/source.txt');
});

test('空值不发送为筛选参数，告警与活动类型使用不同字段', () => {
  assert.deepEqual(buildFilter({ directory: '', pid: '', kind: '', since: '', until: '' }), { limit: 25 });
  assert.deepEqual(buildFilter({ directory: '/workspace/project-a', pid: '12', kind: 'archive_output', since: '', until: '' }, true), { limit: 25, directory: '/workspace/project-a', pid: 12, rule: 'archive_output' });
});

test('非法进程ID与反向时间范围被阻止，正常历史查询保持有效', () => {
  assert.notEqual(validateFilters({ pid: '1x', since: '', until: '' }), null);
  assert.notEqual(validateFilters({ pid: '4294967296', since: '', until: '' }), null);
  assert.notEqual(validateFilters({ pid: '', since: '2026-10-06T14:00', until: '2026-10-05T14:00' }), null);
  assert.equal(validateFilters({ pid: '0', since: '2026-10-05T14:00', until: '2026-10-06T14:00' }), null);
});

test('产品提示把静态内部错误码翻译为中文', () => {
  assert.match(errorMessage('host_unavailable'), /宿主不可用/);
  assert.match(errorMessage('preview_expired'), /预览已失效/);
  assert.doesNotMatch(errorMessage('unknown_internal_code'), /unknown_internal_code/);
  assert.equal(errorMessage('规则版本冲突，请刷新后重试'), '原始提示（未翻译）：规则版本冲突，请刷新后重试');
});

test('顶部服务操作区分安装、启用、明确暂停和未知状态', () => {
  const service: Status['service'] = { installed: true, installed_binary_trusted: true, collector: { loaded: false, running: false, disabled: null, last_exit_code: null }, analyzer: { loaded: false, running: false, disabled: null, last_exit_code: null }, notification: { loaded: false, running: false, disabled: null, last_exit_code: null }, paused: null, status_error: null };
  assert.equal(primaryServiceAction(service), 'start');
  assert.equal(primaryServiceAction({ ...service, paused: false }), 'start');
  assert.equal(primaryServiceAction({ ...service, paused: true }), 'resume');
  assert.equal(primaryServiceAction({ ...service, collector: { ...service.collector, loaded: true } }), 'pause');
  assert.equal(primaryServiceAction({ ...service, installed: false }), 'install');
  assert.equal(primaryServiceAction({ ...service, status_error: 'service_status_failed' }), null);
  assert.equal(primaryServiceAction(), null);
});
