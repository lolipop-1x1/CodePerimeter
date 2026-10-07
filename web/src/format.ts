import { locale, t } from './i18n.ts';
import type { ActivityEvent, AlertRule, ProcessIdentity, Status } from './types.ts';

export function primaryServiceAction(service?: Status['service']): 'install' | 'start' | 'resume' | 'pause' | null {
  if (!service || service.status_error) return null;
  if (!service.installed) return 'install';
  if (service.paused === true) return 'resume';
  return service.collector.loaded ? 'pause' : 'start';
}

export const ruleLabels: Record<AlertRule, string> = {
  get bulk_file_access() { return t('format.bulkFileAccess'); }, get archive_command() { return t('format.archiveCommandIndicator'); }, get archive_output() { return t('format.archiveOutputIndicator'); },
};
export const kindLabels: Record<string, string> = {
  get open() { return t('format.fileOpen'); }, get mmap() { return t('format.fileMapping'); }, get create() { return t('format.create'); }, get write() { return t('format.write'); }, get close() { return t('format.close'); }, get rename() { return t('format.rename'); }, get exec() { return t('format.commandExecution'); }, get fork() { return t('format.processFork'); }, get exit() { return t('format.processExit'); },
};
export const sourceLabels: Record<string, string> = { codex: 'Codex', claude_code: 'Claude Code', zcode: 'ZCode', get manual() { return t('format.manual'); } };
export function eventLabel(event: ActivityEvent): string {
  if (event.archive) return t('format.archiveCommandIndicator');
  if (event.kind === 'open' && event.file?.readable) return t('format.readableOpen');
  if (event.kind === 'mmap' && event.file?.readable) return t('format.readableMapping');
  return kindLabels[event.kind] ?? event.kind;
}
export function basename(path?: string | null): string { return path?.split('/').filter(Boolean).pop() || t('format.identityUnknown'); }
export function processLabel(process: ProcessIdentity): string { return `${basename(process.executable)} (${process.pid})`; }
export function timestamp(value?: number | null): string { return value == null ? t('format.notCollected') : new Date(value).toLocaleString(locale(), { hourCycle: 'h23' }); }
export function timeOnly(value: number, compact = false): string { return new Date(value).toLocaleTimeString(locale(), { hourCycle: 'h23', ...(compact ? { hour: '2-digit', minute: '2-digit' } as const : {}) }); }
export function number(value?: number): string { return value === undefined ? t('format.unknown') : value.toLocaleString(locale()); }
export function eventPath(event: ActivityEvent): string { return event.file?.path ?? event.archive?.input_paths.join(t('common.listSeparator')) ?? event.destination ?? t('format.noFilePath'); }
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
  if (input.pid && (!/^\d+$/.test(input.pid) || Number(input.pid) > 4294967295)) return 'format.processIDMustBeAValidNon';
  const start = input.since ? new Date(input.since).getTime() : null;
  const end = input.until ? new Date(input.until).getTime() : null;
  if ((start !== null && !Number.isFinite(start)) || (end !== null && !Number.isFinite(end))) return 'format.selectAValidQueryTime';
  if (start !== null && end !== null && start > end) return 'format.startTimeCannotBeLaterThanEnd';
  return null;
}
