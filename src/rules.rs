use crate::Result;
use crate::model::{
    ActivityEvent, Alert, AlertRule, EventKind, FileEvidence, ProcessIdentity, SourceStream,
};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const DEFAULT_BULK_WINDOW_MS: i64 = 10_000;
const DEFAULT_BULK_FILE_THRESHOLD: usize = 50;
const DEFAULT_ALERT_MERGE_WINDOW_MS: i64 = 60_000;
const DEFAULT_ARCHIVE_CORRELATION_WINDOW_MS: i64 = 60_000;
const DEFAULT_MAX_PROCESS_STATES: usize = 256;
const DEFAULT_MAX_FILES_PER_PROCESS: usize = 512;
const DEFAULT_MAX_EVIDENCE_PATHS: usize = 16;
const HEALTH_REPEAT_WINDOW_MS: i64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleConfig {
    pub bulk_window_ms: i64,
    pub bulk_file_threshold: usize,
    pub alert_merge_window_ms: i64,
    pub archive_correlation_window_ms: i64,
    pub max_process_states: usize,
    pub max_files_per_process: usize,
    pub max_evidence_paths: usize,
}

impl Default for RuleConfig {
    fn default() -> Self {
        Self {
            bulk_window_ms: DEFAULT_BULK_WINDOW_MS,
            bulk_file_threshold: DEFAULT_BULK_FILE_THRESHOLD,
            alert_merge_window_ms: DEFAULT_ALERT_MERGE_WINDOW_MS,
            archive_correlation_window_ms: DEFAULT_ARCHIVE_CORRELATION_WINDOW_MS,
            max_process_states: DEFAULT_MAX_PROCESS_STATES,
            max_files_per_process: DEFAULT_MAX_FILES_PER_PROCESS,
            max_evidence_paths: DEFAULT_MAX_EVIDENCE_PATHS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleHealth {
    pub observed_timestamp_ms: i64,
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleOutput {
    pub alerts: Vec<Alert>,
    pub health: Vec<RuleHealth>,
    pub matched_directories: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProcessKey {
    source_run_id: String,
    pid: u32,
    generation: ProcessGeneration,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProcessGeneration {
    Source(u32),
    Observed(SourceStream, u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UnknownProcessKey {
    source_run_id: String,
    source_stream: SourceStream,
    pid: u32,
}

#[derive(Debug, Clone, Copy)]
struct UnknownProcessInstance {
    id: u64,
    last_received_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FileKey {
    Object { device: u64, inode: u64 },
    Path(PathBuf),
}

#[derive(Debug, Clone)]
struct FileTouch {
    timestamp_ms: i64,
    path: PathBuf,
    roots: Vec<PathBuf>,
}

#[derive(Debug, Default)]
struct ProcessState {
    source_watermark_ms: Option<i64>,
    last_read_received_ms: i64,
    bulk_files: HashMap<FileKey, FileTouch>,
    recent_reads: VecDeque<FileTouch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AlertKey {
    process: ProcessKey,
    rule: AlertRule,
}

#[derive(Debug, Clone)]
struct PendingAlert {
    alert: Alert,
    last_event_timestamp_ms: i64,
}

#[derive(Debug, Clone)]
struct AlertCandidate {
    rule: AlertRule,
    process: ProcessIdentity,
    roots: Vec<PathBuf>,
    first_timestamp_ms: i64,
    event_timestamp_ms: i64,
    received_timestamp_ms: i64,
    unique_files: usize,
    activity_count: u64,
    evidence_paths: Vec<PathBuf>,
}

pub struct RuleEngine {
    roots: Vec<PathBuf>,
    config: RuleConfig,
    process_states: HashMap<ProcessKey, ProcessState>,
    unknown_processes: HashMap<UnknownProcessKey, UnknownProcessInstance>,
    next_observed_instance: u64,
    alerts: HashMap<AlertKey, PendingAlert>,
    next_alert_id: u64,
    health_last_emitted: HashMap<String, i64>,
}

impl RuleEngine {
    pub fn new(roots: Vec<PathBuf>, config: RuleConfig) -> Result<Self> {
        validate_config(&config)?;
        let roots = normalize_roots(roots)?;
        Ok(Self {
            roots,
            config,
            process_states: HashMap::new(),
            unknown_processes: HashMap::new(),
            next_observed_instance: 0,
            alerts: HashMap::new(),
            next_alert_id: 0,
            health_last_emitted: HashMap::new(),
        })
    }

    pub fn replace_roots(&mut self, roots: Vec<PathBuf>) -> Result<()> {
        self.roots = normalize_roots(roots)?;
        self.process_states.clear();
        self.unknown_processes.clear();
        self.alerts.clear();
        Ok(())
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn process(&mut self, event: &ActivityEvent) -> RuleOutput {
        let mut output = RuleOutput::default();
        let event_timestamp_ms = event
            .source_timestamp_ms
            .unwrap_or(event.received_timestamp_ms);

        if event.source_timestamp_ms.is_none() {
            self.push_health(
                &mut output,
                "source_timestamp_missing",
                event.received_timestamp_ms,
                "事件缺少来源时间；批量窗口和告警时间暂以接收时间降级计算。",
            );
        }

        if event.process.pid_version.is_none() {
            self.push_health(
                &mut output,
                "process_generation_unknown",
                event.received_timestamp_ms,
                "来源未提供 PID 代际；规则按本进程内观察分段关联，不代表精确进程实例。",
            );
        }

        let process_key = self.process_key(event, &mut output.health);
        output.matched_directories = self.event_directories(event);

        if event.source_timestamp_ms.is_some()
            && self
                .process_states
                .get(&process_key)
                .and_then(|state| state.source_watermark_ms)
                .is_some_and(|watermark| event_timestamp_ms < watermark)
        {
            self.push_health(
                &mut output,
                "source_timestamp_out_of_order",
                event.received_timestamp_ms,
                "来源事件时间早于该进程已观察的读事件水位；窗口保留最大来源时间，旧事件不回退窗口，归档关联只使用先于该事件的读取证据。",
            );
        }

        if is_read_event(event.kind) {
            if let Some(candidate) =
                self.observe_read(event, &process_key, event_timestamp_ms, &mut output.health)
            {
                output
                    .matched_directories
                    .extend(candidate.roots.iter().cloned());
                output
                    .alerts
                    .push(self.emit_alert(&process_key, candidate, &mut output.health));
            }
        }

        if let Some(candidate) =
            self.archive_command_candidate(event, &process_key, event_timestamp_ms)
        {
            output
                .matched_directories
                .extend(candidate.roots.iter().cloned());
            output
                .alerts
                .push(self.emit_alert(&process_key, candidate, &mut output.health));
        }

        if let Some(candidate) = self.archive_output_candidate(
            event,
            &process_key,
            event_timestamp_ms,
            &mut output.health,
        ) {
            output
                .matched_directories
                .extend(candidate.roots.iter().cloned());
            output
                .alerts
                .push(self.emit_alert(&process_key, candidate, &mut output.health));
        }

        sort_dedup_paths(&mut output.matched_directories);

        if event.kind == EventKind::Exit {
            self.process_states.remove(&process_key);
            self.unknown_processes.remove(&UnknownProcessKey {
                source_run_id: event.source_run_id.clone(),
                source_stream: event.source_stream,
                pid: event.process.pid,
            });
        }

        output
    }

    fn process_key(&mut self, event: &ActivityEvent, health: &mut Vec<RuleHealth>) -> ProcessKey {
        if let Some(pid_version) = event.process.pid_version {
            return ProcessKey {
                source_run_id: event.source_run_id.clone(),
                pid: event.process.pid,
                generation: ProcessGeneration::Source(pid_version),
            };
        }

        let unknown_key = UnknownProcessKey {
            source_run_id: event.source_run_id.clone(),
            source_stream: event.source_stream,
            pid: event.process.pid,
        };

        if event.kind == EventKind::Exec {
            if let Some(old_instance) = self.unknown_processes.remove(&unknown_key) {
                self.process_states.remove(&ProcessKey {
                    source_run_id: event.source_run_id.clone(),
                    pid: event.process.pid,
                    generation: ProcessGeneration::Observed(event.source_stream, old_instance.id),
                });
            }
        }

        let instance = if let Some(instance) = self.unknown_processes.get_mut(&unknown_key) {
            instance.last_received_ms = instance.last_received_ms.max(event.received_timestamp_ms);
            instance.id
        } else {
            if self.unknown_processes.len() >= self.config.max_process_states
                && let Some(oldest_key) = self
                    .unknown_processes
                    .iter()
                    .min_by_key(|(_, instance)| instance.last_received_ms)
                    .map(|(key, _)| key.clone())
            {
                if let Some(oldest) = self.unknown_processes.remove(&oldest_key) {
                    self.process_states.remove(&ProcessKey {
                        source_run_id: oldest_key.source_run_id.clone(),
                        pid: oldest_key.pid,
                        generation: ProcessGeneration::Observed(
                            oldest_key.source_stream,
                            oldest.id,
                        ),
                    });
                }
                self.push_health_records(
                    health,
                    "process_state_capacity",
                    event.received_timestamp_ms,
                    "未知代际进程索引达到内存上限；最久未活动状态已清除。",
                );
            }
            self.next_observed_instance = self.next_observed_instance.saturating_add(1);
            let id = self.next_observed_instance;
            self.unknown_processes.insert(
                unknown_key.clone(),
                UnknownProcessInstance {
                    id,
                    last_received_ms: event.received_timestamp_ms,
                },
            );
            id
        };

        ProcessKey {
            source_run_id: unknown_key.source_run_id,
            pid: unknown_key.pid,
            generation: ProcessGeneration::Observed(event.source_stream, instance),
        }
    }

    fn observe_read(
        &mut self,
        event: &ActivityEvent,
        process_key: &ProcessKey,
        event_timestamp_ms: i64,
        health: &mut Vec<RuleHealth>,
    ) -> Option<AlertCandidate> {
        let Some(file) = event.file.as_ref() else {
            self.push_health_records(
                health,
                "read_path_missing",
                event.received_timestamp_ms,
                "打开或映射事件没有文件路径。",
            );
            return None;
        };

        if file.path_truncated {
            self.push_health_records(
                health,
                "path_truncated",
                event.received_timestamp_ms,
                "文件路径已截断，无法可靠判断保护目录边界。",
            );
            return None;
        }

        if file.readable != Some(true) {
            if file.readable.is_none() {
                self.push_health_records(
                    health,
                    "readability_unknown",
                    event.received_timestamp_ms,
                    "打开或映射事件没有可读权限证据。",
                );
            }
            return None;
        }

        if file.is_regular == Some(false) {
            return None;
        }

        if file.is_regular.is_none() {
            self.push_health_records(
                health,
                "file_type_unknown",
                event.received_timestamp_ms,
                "文件类型未知；该路径只作为访问线索。",
            );
        }

        let Some(path) = normalize_path(&file.path) else {
            self.push_health_records(
                health,
                "path_not_absolute",
                event.received_timestamp_ms,
                "文件路径不是可判断边界的绝对路径。",
            );
            return None;
        };
        let roots = self.matching_roots(&path);
        if roots.is_empty() || roots.iter().any(|root| root == &path) {
            return None;
        }

        let key = file_key(file, path.clone());
        self.ensure_process_state(process_key, event.received_timestamp_ms, health);
        let config = self.config.clone();
        let mut state_evicted = false;
        let candidate = {
            let state = self.process_states.get_mut(process_key)?;
            state.last_read_received_ms =
                state.last_read_received_ms.max(event.received_timestamp_ms);
            let watermark = state
                .source_watermark_ms
                .map_or(event_timestamp_ms, |known| known.max(event_timestamp_ms));
            state.source_watermark_ms = Some(watermark);
            let bulk_cutoff = watermark.saturating_sub(config.bulk_window_ms);
            let read_history_window_ms = config
                .bulk_window_ms
                .max(config.archive_correlation_window_ms);
            let read_history_cutoff = watermark.saturating_sub(read_history_window_ms);
            state
                .bulk_files
                .retain(|_, touch| touch.timestamp_ms >= bulk_cutoff);
            state
                .recent_reads
                .retain(|touch| touch.timestamp_ms >= read_history_cutoff);

            if event_timestamp_ms >= bulk_cutoff {
                state_evicted |= insert_touch(
                    &mut state.bulk_files,
                    key.clone(),
                    FileTouch {
                        timestamp_ms: event_timestamp_ms,
                        path: path.clone(),
                        roots: roots.clone(),
                    },
                    config.max_files_per_process,
                );
            }
            if event_timestamp_ms >= read_history_cutoff {
                if state.recent_reads.len() >= config.max_files_per_process {
                    let oldest = state
                        .recent_reads
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, touch)| touch.timestamp_ms)
                        .map(|(index, _)| index)
                        .unwrap_or_default();
                    state.recent_reads.remove(oldest);
                    state_evicted = true;
                }
                state.recent_reads.push_back(FileTouch {
                    timestamp_ms: event_timestamp_ms,
                    path: path.clone(),
                    roots: roots.clone(),
                });
            }

            if event_timestamp_ms < bulk_cutoff
                || state.bulk_files.len() < config.bulk_file_threshold
            {
                None
            } else {
                let mut all_roots = Vec::new();
                let mut evidence_paths = Vec::new();
                let mut activity_count = 0_u64;
                let mut first_timestamp_ms = event_timestamp_ms;
                for touch in &state.recent_reads {
                    if touch.timestamp_ms >= bulk_cutoff {
                        activity_count += 1;
                        first_timestamp_ms = first_timestamp_ms.min(touch.timestamp_ms);
                    }
                }
                for touch in state.bulk_files.values() {
                    all_roots.extend(touch.roots.iter().cloned());
                    evidence_paths.push(touch.path.clone());
                }
                sort_dedup_paths(&mut all_roots);
                sort_dedup_paths(&mut evidence_paths);
                evidence_paths.truncate(config.max_evidence_paths);

                Some(AlertCandidate {
                    rule: AlertRule::BulkFileAccess,
                    process: event.process.clone(),
                    roots: all_roots,
                    first_timestamp_ms,
                    event_timestamp_ms: watermark,
                    received_timestamp_ms: event.received_timestamp_ms,
                    unique_files: state.bulk_files.len(),
                    activity_count,
                    evidence_paths,
                })
            }
        };

        if state_evicted {
            self.push_health_records(
                health,
                "file_state_capacity",
                event.received_timestamp_ms,
                "进程文件或访问频次状态达到内存上限；可能低估批量活动或遗漏归档来源关联。",
            );
        }
        candidate
    }

    fn archive_command_candidate(
        &mut self,
        event: &ActivityEvent,
        process_key: &ProcessKey,
        event_timestamp_ms: i64,
    ) -> Option<AlertCandidate> {
        let archive = event.archive.as_ref()?;
        if !is_archive_tool(&archive.tool) {
            return None;
        }

        let mut roots = Vec::new();
        let mut evidence_paths = Vec::new();
        let mut resolved_inputs = Vec::new();
        for input in &archive.input_paths {
            if let Some(path) = resolve_archive_path(input, archive.cwd.as_deref()) {
                let path_roots = self.matching_roots(&path);
                if !path_roots.is_empty() {
                    roots.extend(path_roots.iter().cloned());
                    evidence_paths.push(path.clone());
                }
                resolved_inputs.push(path);
            }
        }

        let resolved_outputs: Vec<_> = archive
            .output_path
            .iter()
            .chain(&archive.output_paths)
            .filter_map(|path| resolve_archive_path(path, archive.cwd.as_deref()))
            .collect();
        for output in &resolved_outputs {
            roots.extend(self.matching_roots(output));
        }

        if let Some((read_roots, read_paths)) =
            self.recent_read_summary(process_key, event_timestamp_ms)
        {
            roots.extend(read_roots);
            evidence_paths.extend(read_paths);
        }

        sort_dedup_paths(&mut roots);
        if roots.is_empty() {
            return None;
        }
        for output in resolved_outputs {
            if !self.matching_roots(&output).is_empty() || is_temporary_path(&output) {
                evidence_paths.push(output);
            }
        }
        sort_dedup_paths(&mut evidence_paths);
        evidence_paths.truncate(self.config.max_evidence_paths);

        let first_timestamp_ms = self
            .recent_read_first_timestamp(process_key, event_timestamp_ms)
            .unwrap_or(event_timestamp_ms);

        Some(AlertCandidate {
            rule: AlertRule::ArchiveCommand,
            process: event.process.clone(),
            roots,
            first_timestamp_ms,
            event_timestamp_ms,
            received_timestamp_ms: event.received_timestamp_ms,
            unique_files: {
                sort_dedup_paths(&mut resolved_inputs);
                resolved_inputs
                    .iter()
                    .filter(|path| !self.matching_roots(path).is_empty())
                    .count()
            },
            activity_count: 1,
            evidence_paths,
        })
    }

    fn archive_output_candidate(
        &mut self,
        event: &ActivityEvent,
        process_key: &ProcessKey,
        event_timestamp_ms: i64,
        health: &mut Vec<RuleHealth>,
    ) -> Option<AlertCandidate> {
        if !matches!(
            event.kind,
            EventKind::Create | EventKind::Write | EventKind::Rename
        ) {
            return None;
        }
        if event.file.as_ref().is_some_and(|file| file.path_truncated) {
            self.push_health_records(
                health,
                "path_truncated",
                event.received_timestamp_ms,
                "归档输出候选的文件路径已截断。",
            );
            return None;
        }
        if event
            .file
            .as_ref()
            .is_some_and(|file| file.is_regular == Some(false))
        {
            return None;
        }

        let mut output_paths = Vec::new();
        if let Some(file) = event.file.as_ref() {
            if let Some(path) = normalize_path(&file.path) {
                output_paths.push(path);
            }
        }
        if let Some(destination) = event.destination.as_deref().and_then(normalize_path) {
            output_paths.push(destination);
        }
        sort_dedup_paths(&mut output_paths);
        let output = output_paths.into_iter().find(|path| {
            is_archive_output_candidate(path) && is_output_location(path, &self.roots)
        })?;

        let (mut roots, mut read_paths) =
            self.recent_read_summary(process_key, event_timestamp_ms)?;
        if roots.is_empty() {
            return None;
        }
        let first_timestamp_ms = self
            .recent_read_first_timestamp(process_key, event_timestamp_ms)
            .unwrap_or(event_timestamp_ms);
        roots.extend(self.matching_roots(&output));
        roots.sort();
        roots.dedup();
        read_paths.push(output.clone());
        sort_dedup_paths(&mut read_paths);
        read_paths.truncate(self.config.max_evidence_paths);

        Some(AlertCandidate {
            rule: AlertRule::ArchiveOutput,
            process: event.process.clone(),
            roots,
            first_timestamp_ms,
            event_timestamp_ms,
            received_timestamp_ms: event.received_timestamp_ms,
            unique_files: 1,
            activity_count: 1,
            evidence_paths: read_paths,
        })
    }

    fn recent_read_summary(
        &self,
        process_key: &ProcessKey,
        timestamp_ms: i64,
    ) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
        let state = self.process_states.get(process_key)?;
        let mut roots = Vec::new();
        let mut paths = Vec::new();
        for touch in &state.recent_reads {
            if touch.timestamp_ms <= timestamp_ms
                && timestamp_ms.saturating_sub(touch.timestamp_ms)
                    <= self.config.archive_correlation_window_ms
            {
                roots.extend(touch.roots.iter().cloned());
                paths.push(touch.path.clone());
            }
        }
        sort_dedup_paths(&mut roots);
        sort_dedup_paths(&mut paths);
        (!roots.is_empty()).then_some((roots, paths))
    }

    fn recent_read_first_timestamp(
        &self,
        process_key: &ProcessKey,
        timestamp_ms: i64,
    ) -> Option<i64> {
        self.process_states
            .get(process_key)?
            .recent_reads
            .iter()
            .filter(|touch| {
                touch.timestamp_ms <= timestamp_ms
                    && timestamp_ms.saturating_sub(touch.timestamp_ms)
                        <= self.config.archive_correlation_window_ms
            })
            .map(|touch| touch.timestamp_ms)
            .min()
    }

    fn ensure_process_state(
        &mut self,
        key: &ProcessKey,
        received_timestamp_ms: i64,
        health: &mut Vec<RuleHealth>,
    ) {
        if self.process_states.contains_key(key) {
            if let Some(state) = self.process_states.get_mut(key) {
                state.last_read_received_ms =
                    state.last_read_received_ms.max(received_timestamp_ms);
            }
            return;
        }

        if self.process_states.len() >= self.config.max_process_states
            && let Some(oldest_key) = self
                .process_states
                .iter()
                .min_by_key(|(_, state)| state.last_read_received_ms)
                .map(|(key, _)| key.clone())
        {
            self.process_states.remove(&oldest_key);
            if let ProcessGeneration::Observed(source_stream, instance) = oldest_key.generation {
                let unknown_key = UnknownProcessKey {
                    source_run_id: oldest_key.source_run_id.clone(),
                    source_stream,
                    pid: oldest_key.pid,
                };
                if self
                    .unknown_processes
                    .get(&unknown_key)
                    .is_some_and(|current| current.id == instance)
                {
                    self.unknown_processes.remove(&unknown_key);
                }
            }
            self.push_health_records(
                health,
                "process_state_capacity",
                received_timestamp_ms,
                "进程关联状态达到内存上限，最久未活动状态已清除；相关归档关联可能缺失。",
            );
        }

        self.process_states
            .insert(key.clone(), ProcessState::default());
    }

    fn matching_roots(&self, path: &Path) -> Vec<PathBuf> {
        self.roots
            .iter()
            .filter(|root| path.starts_with(root))
            .cloned()
            .collect()
    }

    fn event_directories(&self, event: &ActivityEvent) -> Vec<PathBuf> {
        let mut directories = Vec::new();
        if let Some(file) = event.file.as_ref()
            && !file.path_truncated
            && let Some(path) = normalize_path(&file.path)
        {
            directories.extend(self.matching_roots(&path));
        }
        if let Some(path) = event.destination.as_deref().and_then(normalize_path) {
            directories.extend(self.matching_roots(&path));
        }
        if let Some(archive) = event.archive.as_ref() {
            for input in &archive.input_paths {
                if let Some(path) = resolve_archive_path(input, archive.cwd.as_deref()) {
                    directories.extend(self.matching_roots(&path));
                }
            }
            for output in archive
                .output_path
                .iter()
                .chain(&archive.output_paths)
                .filter_map(|path| resolve_archive_path(path, archive.cwd.as_deref()))
            {
                directories.extend(self.matching_roots(&output));
            }
        }
        sort_dedup_paths(&mut directories);
        directories
    }

    fn emit_alert(
        &mut self,
        process_key: &ProcessKey,
        candidate: AlertCandidate,
        health: &mut Vec<RuleHealth>,
    ) -> Alert {
        let key = AlertKey {
            process: process_key.clone(),
            rule: candidate.rule,
        };
        let should_merge = self.alerts.get(&key).is_some_and(|pending| {
            pending
                .last_event_timestamp_ms
                .abs_diff(candidate.event_timestamp_ms)
                <= self.config.alert_merge_window_ms as u64
        });

        if should_merge {
            if let Some(pending) = self.alerts.get_mut(&key) {
                pending.alert.first_timestamp_ms = pending
                    .alert
                    .first_timestamp_ms
                    .min(candidate.first_timestamp_ms);
                pending.alert.last_timestamp_ms = pending
                    .alert
                    .last_timestamp_ms
                    .max(candidate.event_timestamp_ms);
                pending.alert.unique_files = pending.alert.unique_files.max(candidate.unique_files);
                pending.alert.activity_count = if candidate.rule == AlertRule::BulkFileAccess {
                    pending.alert.activity_count.max(candidate.activity_count)
                } else {
                    pending
                        .alert
                        .activity_count
                        .saturating_add(candidate.activity_count)
                };
                pending.alert.roots.extend(candidate.roots);
                sort_dedup_paths(&mut pending.alert.roots);
                pending
                    .alert
                    .evidence_paths
                    .extend(candidate.evidence_paths);
                sort_dedup_paths(&mut pending.alert.evidence_paths);
                pending
                    .alert
                    .evidence_paths
                    .truncate(self.config.max_evidence_paths);
                pending.last_event_timestamp_ms = pending
                    .last_event_timestamp_ms
                    .max(candidate.event_timestamp_ms);
                let mut updated = pending.alert.clone();
                updated.is_new = false;
                return updated;
            }
        }

        self.next_alert_id = self.next_alert_id.saturating_add(1);
        let generation = match process_key.generation {
            ProcessGeneration::Source(value) => format!("v{value}"),
            ProcessGeneration::Observed(_, value) => format!("o{value}"),
        };
        if self.alerts.len() >= self.config.max_process_states.saturating_mul(3)
            && let Some(oldest_key) = self
                .alerts
                .iter()
                .min_by_key(|(_, pending)| pending.last_event_timestamp_ms)
                .map(|(key, _)| key.clone())
        {
            self.alerts.remove(&oldest_key);
            self.push_health_records(
                health,
                "alert_state_capacity",
                candidate.received_timestamp_ms,
                "告警合并状态达到内存上限；后续相同活动可能重新生成告警。",
            );
        }

        let alert = Alert {
            id: format!(
                "{}:{}:{}:{}:{}",
                process_key.source_run_id,
                process_key.pid,
                generation,
                alert_rule_name(candidate.rule),
                self.next_alert_id
            ),
            rule: candidate.rule,
            process: candidate.process,
            roots: candidate.roots,
            first_timestamp_ms: candidate.first_timestamp_ms,
            last_timestamp_ms: candidate.event_timestamp_ms,
            unique_files: candidate.unique_files,
            activity_count: candidate.activity_count,
            evidence_paths: candidate.evidence_paths,
            is_new: true,
        };
        self.alerts.insert(
            key,
            PendingAlert {
                alert: alert.clone(),
                last_event_timestamp_ms: candidate.event_timestamp_ms,
            },
        );
        alert
    }

    fn push_health(
        &mut self,
        output: &mut RuleOutput,
        code: &str,
        timestamp_ms: i64,
        detail: &str,
    ) {
        self.push_health_records(&mut output.health, code, timestamp_ms, detail);
    }

    fn push_health_records(
        &mut self,
        health: &mut Vec<RuleHealth>,
        code: &str,
        timestamp_ms: i64,
        detail: &str,
    ) {
        if self
            .health_last_emitted
            .get(code)
            .is_some_and(|last| timestamp_ms.saturating_sub(*last) < HEALTH_REPEAT_WINDOW_MS)
        {
            return;
        }
        self.health_last_emitted
            .insert(code.to_string(), timestamp_ms);
        health.push(RuleHealth {
            observed_timestamp_ms: timestamp_ms,
            code: code.to_string(),
            detail: detail.to_string(),
        });
    }
}

fn validate_config(config: &RuleConfig) -> Result<()> {
    if config.bulk_window_ms <= 0
        || config.bulk_file_threshold == 0
        || config.alert_merge_window_ms <= 0
        || config.archive_correlation_window_ms <= 0
        || config.max_process_states == 0
        || config.max_files_per_process < config.bulk_file_threshold
        || config.max_evidence_paths == 0
    {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            "规则参数必须为正数，且每进程文件上限不能小于批量阈值。",
        )));
    }
    Ok(())
}

fn normalize_roots(roots: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut normalized = Vec::new();
    for root in roots {
        let Some(path) = normalize_path(&root) else {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "保护目录必须是绝对路径。",
            )));
        };
        normalized.push(path);
    }
    sort_dedup_paths(&mut normalized);
    Ok(normalized)
}

pub(crate) fn normalize_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let lexical = lexical_normalize(path)?;
    let mut cursor = lexical.clone();
    let mut suffix = Vec::new();

    loop {
        match fs::canonicalize(&cursor) {
            Ok(mut resolved) => {
                for component in suffix.iter().rev() {
                    resolved.push(component);
                }
                return lexical_normalize(&resolved);
            }
            Err(_) => {
                let name = cursor.file_name()?.to_os_string();
                suffix.push(name);
                if !cursor.pop() {
                    return Some(lexical);
                }
            }
        }
    }
}

fn lexical_normalize(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized.is_absolute().then_some(normalized)
}

fn file_key(file: &FileEvidence, normalized_path: PathBuf) -> FileKey {
    match (file.device, file.inode) {
        (Some(device), Some(inode)) => FileKey::Object { device, inode },
        _ => FileKey::Path(normalized_path),
    }
}

fn insert_touch(
    touches: &mut HashMap<FileKey, FileTouch>,
    key: FileKey,
    touch: FileTouch,
    limit: usize,
) -> bool {
    if let Some(existing) = touches.get_mut(&key) {
        if touch.timestamp_ms >= existing.timestamp_ms {
            existing.timestamp_ms = touch.timestamp_ms;
            existing.path = touch.path;
        }
        existing.roots.extend(touch.roots);
        sort_dedup_paths(&mut existing.roots);
        return false;
    }

    let evicted = if touches.len() >= limit
        && let Some(oldest_key) = touches
            .iter()
            .min_by_key(|(_, existing)| existing.timestamp_ms)
            .map(|(key, _)| key.clone())
    {
        touches.remove(&oldest_key);
        true
    } else {
        false
    };
    touches.insert(key, touch);
    evicted
}

fn is_read_event(kind: EventKind) -> bool {
    matches!(kind, EventKind::Open | EventKind::Mmap)
}

fn is_archive_tool(tool: &str) -> bool {
    let name = Path::new(tool)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(tool)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "tar"
            | "gtar"
            | "bsdtar"
            | "zip"
            | "7z"
            | "7zz"
            | "ditto"
            | "gzip"
            | "pigz"
            | "bzip2"
            | "pbzip2"
            | "xz"
            | "zstd"
            | "rar"
    )
}

fn resolve_archive_path(path: &Path, cwd: Option<&Path>) -> Option<PathBuf> {
    if path.is_absolute() {
        return normalize_path(path);
    }
    normalize_path(&cwd?.join(path))
}

fn is_archive_output_candidate(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar", ".tgz", ".tbz", ".tbz2", ".txz",
        ".zip", ".7z", ".rar", ".gz", ".bz2", ".xz", ".zst",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

fn is_output_location(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root)) || is_temporary_path(path)
}

fn is_temporary_path(path: &Path) -> bool {
    let known_roots = [
        Path::new("/tmp"),
        Path::new("/private/tmp"),
        Path::new("/var/tmp"),
        Path::new("/private/var/tmp"),
    ];
    if known_roots.iter().any(|root| path.starts_with(root)) {
        return true;
    }

    let components: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    components.len() >= 5
        && components[0] == "var"
        && components[1] == "folders"
        && components[4] == "T"
        || components.len() >= 6
            && components[0] == "private"
            && components[1] == "var"
            && components[2] == "folders"
            && components[5] == "T"
}

fn sort_dedup_paths(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

fn alert_rule_name(rule: AlertRule) -> &'static str {
    match rule {
        AlertRule::BulkFileAccess => "bulk_file_access",
        AlertRule::ArchiveCommand => "archive_command",
        AlertRule::ArchiveOutput => "archive_output",
    }
}
