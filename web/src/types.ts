export type EventKind = 'open' | 'mmap' | 'create' | 'write' | 'close' | 'rename' | 'exec' | 'fork' | 'exit';
export type AlertRule = 'bulk_file_access' | 'archive_command' | 'archive_output';
export interface ProcessIdentity {
  pid: number;
  pid_version: number | null;
  ppid: number | null;
  executable: string | null;
  signing_id: string | null;
  team_id: string | null;
}
export interface ActivityEvent {
  source_run_id: string;
  source_stream: string;
  source_schema_version: number | null;
  source_message_version: number | null;
  source_timestamp_ms: number | null;
  received_timestamp_ms: number;
  global_seq: number | null;
  event_seq: number | null;
  kind: EventKind;
  process: ProcessIdentity;
  file: { path: string; path_truncated: boolean; readable: boolean | null; is_regular: boolean | null } | null;
  destination: string | null;
  modified: boolean | null;
  archive: { tool: string; input_paths: string[]; output_path: string | null; output_paths?: string[]; cwd: string | null } | null;
}
export interface StoredEvent { id: number; event: ActivityEvent; directories: string[] }
export interface Alert {
  id: string;
  rule: AlertRule;
  process: ProcessIdentity;
  roots: string[];
  first_timestamp_ms: number;
  last_timestamp_ms: number;
  unique_files: number;
  activity_count: number;
  evidence_paths: string[];
}
export interface RulesSettings {
  version: number;
  bulk_enabled: boolean;
  archive_command_enabled: boolean;
  archive_output_enabled: boolean;
  bulk_file_threshold: number;
  bulk_window_ms: number;
  alert_merge_window_ms: number;
  archive_correlation_window_ms: number;
}
export interface NotificationRecord {
  id: number;
  record: { alert_id: string; observed_timestamp_ms: number; outcome: string; detail: string | null };
}
export interface AlertEntry {
  alert: Alert;
  is_read: boolean;
  processed: boolean;
  note: string;
  revision: number;
  rule_version: number;
  rule_snapshot: RulesSettings | null;
  handling_history: { timestamp_ms: number; is_read: boolean; processed: boolean; note: string }[];
  handling_history_truncated?: boolean;
  events?: StoredEvent[];
  notifications?: NotificationRecord[];
}
export interface EventDetail { event: StoredEvent; alerts: AlertEntry[] }
export interface Directory {
  path: string;
  sources: string[];
  added_at_ms: number;
  exists: boolean;
  enabled: boolean;
  effective: boolean;
  excluded_by: string | null;
}
export interface Candidate {
  raw_paths: string[];
  canonical_path: string | null;
  status: string;
  origins: { source: string; version: string | null; occurrences: number }[];
}
export interface HistoryPreview {
  preview_id: string;
  candidates: Candidate[];
  gaps: { source: string; kind: string; count: number }[];
  counts: Record<string, number>;
  versions: { source: string; version: string; compatibility_validated: boolean }[];
}
export interface JobStatus { loaded: boolean; running: boolean; disabled: boolean | null; last_exit_code: number | null }
export interface CollectorStreamSample { last_received_timestamp_ms: number | null; last_source_timestamp_ms: number | null }
export interface RuntimeStatus {
  state: string;
  collector_state: string;
  collector_run_id: string | null;
  collector_schema_version: number | null;
  collector_message_version: number | null;
  collector_streams?: Record<string, CollectorStreamSample>;
  collector_dropped_lines: number;
  reader_dropped_frames: number;
  database_state: string;
  database_gap_events: number;
  database_error: string | null;
  retention_state: string;
  last_event_received_ms: number | null;
  notify_session_active: boolean;
  notify_session_last_seen_ms: number | null;
  memory_pending_notifications: number;
  memory_dropped_notifications: number;
  monitoring_paused?: boolean;
}
export interface Status {
  host: RuntimeStatus | null;
  host_error: string | null;
  service: {
    installed: boolean;
    installed_binary_trusted: boolean;
    collector: JobStatus;
    analyzer: JobStatus;
    notification: JobStatus;
    paused: boolean | null;
    status_error: string | null;
  };
  platform: string;
  service_actions_enabled: boolean;
}
export interface Statistics { cumulative: Record<string, number>; recent: Record<string, number> }
export interface Summary {
  stats: Statistics;
  trend: { timestamp_ms: number; open: number; mmap: number; archive: number }[];
  top_processes: { pid: number; executable: string | null; count: number }[];
  top_files: { path: string; count: number }[];
  unread_alerts: number;
  pending_alerts: number;
}
export interface Retention { days: number; counts: Record<string, number> }
export interface Health { id: number; record: { observed_timestamp_ms: number; component: string; code: string; state: string; detail: string | null } }
export interface PageResult<T> { items: T[]; next_cursor: string | null; total: number }
export interface ServiceOperation { id: string; operation: string; state: 'running' | 'succeeded' | 'cancelled' | 'failed'; report?: { steps: { label: string; success: boolean; message: string }[]; data_preserved: boolean } | null; error?: string | null }
