import type { ActivityEvent, AlertRule, ProcessIdentity, Status } from './types.ts';

export function primaryServiceAction(service?: Status['service']): 'install' | 'start' | 'resume' | 'pause' | null {
  if (!service || service.status_error) return null;
  if (!service.installed) return 'install';
  if (service.paused === true) return 'resume';
  return service.collector.loaded ? 'pause' : 'start';
}

export const ruleLabels: Record<AlertRule, string> = {
  bulk_file_access: '批量文件访问', archive_command: '归档命令迹象', archive_output: '归档输出迹象',
};
export const kindLabels: Record<string, string> = {
  open: '文件打开', mmap: '文件映射', create: '创建', write: '写入', close: '关闭', rename: '重命名', exec: '命令执行', fork: '进程派生', exit: '进程退出',
};
export const sourceLabels: Record<string, string> = { codex: 'Codex', claude_code: 'Claude Code', zcode: 'ZCode', manual: '手动' };
export function eventLabel(event: ActivityEvent): string {
  if (event.archive) return '归档命令迹象';
  if (event.kind === 'open' && event.file?.readable) return '可读打开';
  if (event.kind === 'mmap' && event.file?.readable) return '可读映射';
  return kindLabels[event.kind] ?? event.kind;
}
export function basename(path?: string | null): string { return path?.split('/').filter(Boolean).pop() || '身份未知'; }
export function processLabel(process: ProcessIdentity): string { return `${basename(process.executable)} (${process.pid})`; }
export function timestamp(value?: number | null): string { return value == null ? '未采集' : new Date(value).toLocaleString('zh-CN', { hour12: false }); }
export function timeOnly(value: number): string { return new Date(value).toLocaleTimeString('zh-CN', { hour12: false }); }
export function number(value?: number): string { return value === undefined ? '未知' : value.toLocaleString('zh-CN'); }
export function eventPath(event: ActivityEvent): string { return event.file?.path ?? event.archive?.input_paths.join('、') ?? event.destination ?? '无文件路径'; }
export function buildFilter(input: { directory: string; pid: string; kind: string; since: string; until: string; filePath?: string }, alerts = false) {
  const filter: Record<string, unknown> = { limit: 25 };
  if (input.directory) filter.directory = input.directory;
  if (input.pid) filter.pid = Number(input.pid);
  if (input.kind) filter[alerts ? 'rule' : 'kind'] = input.kind;
  if (input.since) filter.since_ms = new Date(input.since).getTime();
  if (input.until) filter.until_ms = new Date(input.until).getTime();
  if (input.filePath) filter.file_path = input.filePath;
  return filter;
}
export function validateFilters(input: { pid: string; since: string; until: string }): string | null {
  if (input.pid && (!/^\d+$/.test(input.pid) || Number(input.pid) > 4294967295)) return '进程 ID 需要为有效的非负整数。';
  const start = input.since ? new Date(input.since).getTime() : null;
  const end = input.until ? new Date(input.until).getTime() : null;
  if ((start !== null && !Number.isFinite(start)) || (end !== null && !Number.isFinite(end))) return '请选择有效的查询时间。';
  if (start !== null && end !== null && start > end) return '开始时间不能晚于结束时间。';
  return null;
}
