//! 普通用户宿主：独占 SQLite、分析受保护目录活动，并通过本地 IPC 服务 CLI 与通知进程。

use crate::Result;
use crate::eslogger::EsloggerAdapter;
use crate::model::{ActivityEvent, Alert, EventKind, now_ms};
use crate::rules::{RuleConfig, RuleEngine};
use crate::service::{
    CollectorClient, CollectorFrame, CollectorTiming, read_bounded_line, verify_peer_uid,
};
use crate::storage::{
    AlertFilter, DirectoryConfig, EventFilter, HealthFilter, HealthRecord, NotificationFilter,
    NotificationOutcome, NotificationRecord, PendingNotification, PendingNotificationSummary,
    SourceContext, Storage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const CONTROL_LINE_LIMIT: usize = 256 * 1024;
const CONTROL_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
const COLLECTOR_QUEUE_CAPACITY: usize = 64;
const DEDUP_CAPACITY: usize = 8_192;
const MEMORY_ALERT_CAPACITY: usize = 256;
const MAX_QUERY_LIMIT: usize = 100;
const MAX_NOTIFY_BURST: usize = 8;
const DB_RETRY_COOLDOWN: Duration = Duration::from_secs(5);
const RETENTION_INTERVAL: Duration = Duration::from_secs(60 * 60);
const NOTIFY_SESSION_TIMEOUT_MS: i64 = 30_000;
const NOTIFY_RETRY_COOLDOWN_MS: i64 = 5_000;
const EVENT_KINDS: [&str; 9] = [
    "open", "mmap", "exec", "fork", "exit", "create", "write", "rename", "close",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub collector_socket: PathBuf,
    pub control_socket: PathBuf,
    pub database_path: PathBuf,
    pub bulk_file_threshold: usize,
    pub bulk_window_ms: i64,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            collector_socket: PathBuf::new(),
            control_socket: PathBuf::new(),
            database_path: PathBuf::new(),
            bulk_file_threshold: 50,
            bulk_window_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryImport {
    pub path: PathBuf,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", content = "payload", rename_all = "snake_case")]
pub enum ControlRequest {
    Status,
    AddDirectories {
        entries: Vec<DirectoryImport>,
    },
    RemoveDirectory {
        path: PathBuf,
    },
    ListDirectories,
    QueryEvents {
        filter: EventFilter,
    },
    QueryAlerts {
        filter: AlertFilter,
    },
    QueryHealth {
        filter: HealthFilter,
    },
    QueryNotifications {
        filter: NotificationFilter,
    },
    Stats {
        since_ms: Option<i64>,
        until_ms: Option<i64>,
    },
    ClearCumulativeStats,
    PendingNotifications {
        limit: usize,
    },
    NotifyPoll {
        session_id: String,
    },
    NotifySummaryResult {
        session_id: String,
        sequence: i64,
        success: bool,
    },
    NotifyAlertResult {
        session_id: String,
        alert_id: String,
        success: bool,
    },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControlResponse {
    pub ok: bool,
    pub error: Option<String>,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineTiming {
    pub collector: Option<CollectorTiming>,
    pub host_frames: u64,
    pub host_processing_total_us: u64,
    pub host_processing_max_us: u64,
    pub source_to_collector_receive_max_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub state: String,
    pub uid: u32,
    pub bulk_file_threshold: usize,
    pub bulk_window_ms: i64,
    pub collector_state: String,
    pub collector_run_id: Option<String>,
    pub collector_schema_version: Option<u64>,
    pub collector_message_version: Option<u64>,
    pub collector_dropped_lines: u64,
    pub reader_dropped_frames: u64,
    pub database_state: String,
    pub database_gap_events: u64,
    pub database_error: Option<String>,
    pub retention_state: String,
    pub retention_last_run_ms: Option<i64>,
    pub last_event_received_ms: Option<i64>,
    pub pipeline_timing: PipelineTiming,
    pub observed_events_by_kind: BTreeMap<String, u64>,
    pub persisted_events_by_kind: BTreeMap<String, u64>,
    pub filtered_events_by_kind: BTreeMap<String, u64>,
    pub duplicate_events: u64,
    pub memory_pending_notifications: usize,
    pub memory_dropped_notifications: u64,
    pub notify_session_active: bool,
    pub notify_session_last_seen_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NotifyPollResult {
    Summary {
        summary: PendingNotificationSummary,
        sequence: i64,
    },
    Alert {
        alert: Alert,
        persisted: bool,
    },
    Idle {
        database_state: String,
        memory_pending: bool,
    },
}

pub trait NotificationSender {
    fn send(&mut self, title: &str, body: &str) -> Result<()>;
}

#[derive(Debug, Clone)]
struct NotifySession {
    id: String,
    last_seen_ms: i64,
    initialized: bool,
    summary: Option<(PendingNotificationSummary, i64)>,
    summary_retry_after_ms: i64,
}

#[derive(Debug, Clone)]
struct MemoryAlert {
    alert: Alert,
    retry_after_ms: i64,
}

#[derive(Debug, Clone)]
struct RetryAlert {
    retry_after_ms: i64,
}

#[derive(Debug)]
struct RuntimeCounters {
    observed: BTreeMap<String, u64>,
    persisted: BTreeMap<String, u64>,
    filtered: BTreeMap<String, u64>,
    duplicates: u64,
}

impl Default for RuntimeCounters {
    fn default() -> Self {
        Self {
            observed: zero_event_counts(),
            persisted: zero_event_counts(),
            filtered: zero_event_counts(),
            duplicates: 0,
        }
    }
}

#[derive(Debug, Default)]
struct Deduper {
    keys: HashSet<String>,
    order: VecDeque<String>,
}

impl Deduper {
    fn insert(&mut self, event: &ActivityEvent) -> bool {
        let Some(key) = event_dedup_key(event) else {
            return true;
        };
        if !self.keys.insert(key.clone()) {
            return false;
        }
        self.order.push_back(key);
        while self.order.len() > DEDUP_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.keys.remove(&oldest);
            }
        }
        true
    }
}

enum ReaderMessage {
    Frame(CollectorFrame),
    State { state: String, code: String },
}

struct DaemonState {
    uid: u32,
    bulk_file_threshold: usize,
    bulk_window_ms: i64,
    storage: Storage,
    rules: RuleEngine,
    adapter: Option<EsloggerAdapter>,
    adapter_run_id: Option<String>,
    source_state: String,
    source_diagnostic_state: Option<String>,
    database_state: String,
    database_error: Option<String>,
    database_retry_at: Instant,
    database_retry_cooldown: Duration,
    database_gap_events: u64,
    retention_state: String,
    retention_last_run_ms: Option<i64>,
    retention_at: Instant,
    retention_interval: Duration,
    collector_dropped_lines: u64,
    reader_dropped_frames: Arc<AtomicU64>,
    last_event_received_ms: Option<i64>,
    pipeline_timing: PipelineTiming,
    counters: RuntimeCounters,
    deduper: Deduper,
    memory_alerts: VecDeque<MemoryAlert>,
    deferred_alerts: VecDeque<Alert>,
    memory_dropped_notifications: u64,
    sent_memory_alerts: VecDeque<String>,
    retry_alerts: BTreeMap<String, RetryAlert>,
    notify_session: Option<NotifySession>,
    last_collector_drop_count: u64,
    last_health_record_at: BTreeMap<String, i64>,
}

fn zero_event_counts() -> BTreeMap<String, u64> {
    EVENT_KINDS
        .into_iter()
        .map(|kind| (kind.to_owned(), 0))
        .collect()
}

fn event_kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Open => "open",
        EventKind::Mmap => "mmap",
        EventKind::Exec => "exec",
        EventKind::Fork => "fork",
        EventKind::Exit => "exit",
        EventKind::Create => "create",
        EventKind::Write => "write",
        EventKind::Rename => "rename",
        EventKind::Close => "close",
    }
}

fn event_dedup_key(event: &ActivityEvent) -> Option<String> {
    if let Some(sequence) = event.global_seq {
        return Some(format!("{}:g:{sequence}", event.source_run_id));
    }
    event.event_seq.map(|sequence| {
        format!(
            "{}:e:{}:{sequence}",
            event.source_run_id,
            event_kind_name(event.kind)
        )
    })
}

fn response_ok<T: Serialize>(data: &T) -> ControlResponse {
    match serde_json::to_value(data) {
        Ok(data) => ControlResponse {
            ok: true,
            error: None,
            data: Some(data),
        },
        Err(_) => response_error("响应序列化失败"),
    }
}

fn response_error(message: &str) -> ControlResponse {
    ControlResponse {
        ok: false,
        error: Some(message.to_owned()),
        data: None,
    }
}

fn json_value<T: Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

/// 对本地 JSONL 控制 socket 发送一次请求。
pub fn request_control(path: &Path, request: ControlRequest) -> Result<ControlResponse> {
    let uid = unsafe { libc::geteuid() };
    check_control_socket(path, uid)?;
    let mut stream = UnixStream::connect(path)?;
    verify_peer_uid(&stream, uid)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let encoded = serde_json::to_vec(&request)?;
    if encoded.len() > CONTROL_LINE_LIMIT {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "控制请求超过大小上限").into());
    }
    stream.write_all(&encoded)?;
    stream.write_all(b"\n")?;
    let mut reader = BufReader::new(stream);
    let bytes = read_bounded_line(&mut reader, CONTROL_RESPONSE_LIMIT)?;
    let bytes =
        bytes.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "宿主未返回响应"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn check_control_socket(path: &Path, uid: u32) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let parent = fs::symlink_metadata(
        path.parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "控制路径缺少父目录"))?,
    )?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || !parent.is_dir()
        || parent.file_type().is_symlink()
        || parent.uid() != uid
        || parent.mode() & 0o022 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "控制 socket 路径或权限不可信",
        ));
    }
    Ok(())
}

fn ensure_private_parent(path: &Path, uid: u32) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "控制 socket 路径必须是绝对路径",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "控制路径缺少父目录"))?;
    fs::create_dir_all(parent)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "控制目录必须由宿主用户拥有且仅用户可访问",
        ));
    }
    Ok(())
}

fn bind_control(path: &Path, uid: u32) -> io::Result<UnixListener> {
    ensure_private_parent(path, uid)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() || metadata.uid() != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "拒绝替换非本用户拥有的控制路径",
            ));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, "宿主已运行"));
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn lock_single_host(path: &Path) -> io::Result<File> {
    let lock_path = path.with_extension("lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "同一用户的宿主已运行",
        ));
    }
    Ok(file)
}

/// 启动生产宿主；root 只运行转发采集器，不执行分析或打开数据库。
pub fn run_daemon(options: RuntimeOptions) -> Result<()> {
    run_daemon_inner(options, 0, DB_RETRY_COOLDOWN, RETENTION_INTERVAL)
}

/// 明确限定于本机合成采集器测试；生产 CLI 不暴露此身份覆盖。
#[doc(hidden)]
pub fn run_daemon_with_expected_collector_uid(
    options: RuntimeOptions,
    expected_uid: u32,
) -> Result<()> {
    run_daemon_inner(
        options,
        expected_uid,
        Duration::from_millis(250),
        Duration::from_millis(250),
    )
}

fn run_daemon_inner(
    options: RuntimeOptions,
    expected_collector_uid: u32,
    retry_cooldown: Duration,
    retention_interval: Duration,
) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    if uid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "root 仅运行采集转发器；宿主分析与 SQLite 必须以普通用户运行",
        )
        .into());
    }
    if expected_collector_uid != 0 && expected_collector_uid != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "生产宿主只能接收 root 采集器；测试身份必须是当前普通用户",
        )
        .into());
    }
    ensure_private_parent(&options.control_socket, uid)?;
    ensure_private_parent(&options.database_path, uid)?;
    if let Ok(metadata) = fs::symlink_metadata(&options.database_path)
        && (!metadata.is_file() || metadata.file_type().is_symlink() || metadata.uid() != uid)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "数据库必须是宿主用户拥有的普通文件",
        )
        .into());
    }
    let _lock = lock_single_host(&options.control_socket)?;
    let listener = bind_control(&options.control_socket, uid)?;
    let storage = Storage::open(&options.database_path)?;
    let roots = storage.active_directory_paths()?;
    let rules = RuleEngine::new(
        roots,
        RuleConfig {
            bulk_file_threshold: options.bulk_file_threshold,
            bulk_window_ms: options.bulk_window_ms,
            ..RuleConfig::default()
        },
    )?;
    let (sender, receiver) = mpsc::sync_channel(COLLECTOR_QUEUE_CAPACITY);
    let reader_dropped_frames = Arc::new(AtomicU64::new(0));
    let reader_stopping = Arc::new(AtomicBool::new(false));
    let reader = spawn_collector_reader(
        options.collector_socket.clone(),
        expected_collector_uid,
        sender,
        Arc::clone(&reader_dropped_frames),
        Arc::clone(&reader_stopping),
    );
    let mut state = DaemonState {
        uid,
        bulk_file_threshold: options.bulk_file_threshold,
        bulk_window_ms: options.bulk_window_ms,
        storage,
        rules,
        adapter: None,
        adapter_run_id: None,
        source_state: "connecting".into(),
        source_diagnostic_state: None,
        database_state: "ready".into(),
        database_error: None,
        database_retry_at: Instant::now(),
        database_retry_cooldown: retry_cooldown,
        database_gap_events: 0,
        retention_state: "pending".into(),
        retention_last_run_ms: None,
        retention_at: Instant::now(),
        retention_interval,
        collector_dropped_lines: 0,
        reader_dropped_frames,
        last_event_received_ms: None,
        pipeline_timing: PipelineTiming::default(),
        counters: RuntimeCounters::default(),
        deduper: Deduper::default(),
        memory_alerts: VecDeque::new(),
        deferred_alerts: VecDeque::new(),
        memory_dropped_notifications: 0,
        sent_memory_alerts: VecDeque::new(),
        retry_alerts: BTreeMap::new(),
        notify_session: None,
        last_collector_drop_count: 0,
        last_health_record_at: BTreeMap::new(),
    };
    state.write_health(
        "runtime",
        "host_started",
        "ready",
        "普通用户宿主已启动；只分析选定目录内的事件。",
    );
    state.prune_if_due();
    let mut stopping = false;
    while !stopping {
        stopping = accept_control(&listener, &mut state)?;
        if stopping {
            break;
        }
        let mut processed = 0;
        while processed < COLLECTOR_QUEUE_CAPACITY {
            match receiver.try_recv() {
                Ok(message) => {
                    state.handle_reader_message(message);
                    processed += 1;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        state.retry_database_if_due();
        state.prune_if_due();
        if processed == 0 {
            // 空闲时阻塞等待新帧并即时唤醒；控制请求仍最多等待十毫秒。
            if let Ok(message) = receiver.recv_timeout(Duration::from_millis(10)) {
                state.handle_reader_message(message);
            }
        }
    }
    reader_stopping.store(true, Ordering::Relaxed);
    drop(receiver);
    reader
        .join()
        .map_err(|_| io::Error::other("采集读取线程异常退出"))?;
    drop(listener);
    let _ = fs::remove_file(&options.control_socket);
    Ok(())
}

fn send_reader_message(
    sender: &mpsc::SyncSender<ReaderMessage>,
    message: ReaderMessage,
    stopping: &AtomicBool,
) -> bool {
    // 有界队列释放容量时立即唤醒；宿主停止先释放 receiver，再回收读取线程。
    !stopping.load(Ordering::Relaxed) && sender.send(message).is_ok()
}

fn spawn_collector_reader(
    socket_path: PathBuf,
    expected_uid: u32,
    sender: mpsc::SyncSender<ReaderMessage>,
    dropped: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut last_state = String::new();
        let mut last_connect_error = None;
        while !stopping.load(Ordering::Relaxed) {
            let mut client = match CollectorClient::connect_expected(&socket_path, expected_uid) {
                Ok(client) => {
                    last_connect_error = None;
                    if last_state != "connected" {
                        if !send_reader_message(
                            &sender,
                            ReaderMessage::State {
                                state: "connected".into(),
                                code: "collector_connected".into(),
                            },
                            &stopping,
                        ) {
                            return;
                        }
                        last_state = "connected".into();
                    }
                    client
                }
                Err(error) => {
                    let code = if error.kind() == io::ErrorKind::PermissionDenied {
                        "collector_identity_rejected"
                    } else {
                        "collector_unavailable"
                    };
                    if last_state != "reconnecting" || last_connect_error != Some(code) {
                        if !send_reader_message(
                            &sender,
                            ReaderMessage::State {
                                state: "reconnecting".into(),
                                code: code.into(),
                            },
                            &stopping,
                        ) {
                            return;
                        }
                        last_state = "reconnecting".into();
                        last_connect_error = Some(code);
                    }
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
            };
            let mut last_frame_at = Instant::now();
            while !stopping.load(Ordering::Relaxed) {
                match client.read_frame() {
                    Ok(frame) => {
                        last_frame_at = Instant::now();
                        if last_state == "stalled" {
                            if !send_reader_message(
                                &sender,
                                ReaderMessage::State {
                                    state: "connected".into(),
                                    code: "collector_stream_resumed".into(),
                                },
                                &stopping,
                            ) {
                                return;
                            }
                            last_state = "connected".into();
                        }
                        if !send_reader_message(&sender, ReaderMessage::Frame(frame), &stopping) {
                            return;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock
                                | io::ErrorKind::TimedOut
                                | io::ErrorKind::Interrupted
                        ) =>
                    {
                        // 系统桥接每秒有心跳；暂时拥塞保留连接，持续无帧另报覆盖未知。
                        if last_frame_at.elapsed() >= Duration::from_secs(3)
                            && last_state != "stalled"
                        {
                            if !send_reader_message(
                                &sender,
                                ReaderMessage::State {
                                    state: "stalled".into(),
                                    code: "collector_frame_timeout".into(),
                                },
                                &stopping,
                            ) {
                                return;
                            }
                            last_state = "stalled".into();
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                        dropped.fetch_add(1, Ordering::Relaxed);
                        if !send_reader_message(
                            &sender,
                            ReaderMessage::State {
                                state: "coverage_gap".into(),
                                code: "collector_invalid_frame".into(),
                            },
                            &stopping,
                        ) {
                            return;
                        }
                    }
                    Err(error) => {
                        if last_state != "reconnecting" {
                            if !send_reader_message(
                                &sender,
                                ReaderMessage::State {
                                    state: "reconnecting".into(),
                                    code: if error.kind() == io::ErrorKind::UnexpectedEof {
                                        "collector_eof"
                                    } else {
                                        "collector_read_error"
                                    }
                                    .into(),
                                },
                                &stopping,
                            ) {
                                return;
                            }
                            last_state = "reconnecting".into();
                        }
                        break;
                    }
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    })
}

fn accept_control(listener: &UnixListener, state: &mut DaemonState) -> Result<bool> {
    match listener.accept() {
        Ok((mut stream, _)) => {
            if verify_peer_uid(&stream, state.uid).is_err() {
                return Ok(false);
            }
            Ok(handle_control_connection(&mut stream, state).unwrap_or(false))
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn handle_control_connection(stream: &mut UnixStream, state: &mut DaemonState) -> io::Result<bool> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let request = match read_bounded_line(&mut reader, CONTROL_LINE_LIMIT) {
        Ok(Some(bytes)) => serde_json::from_slice::<ControlRequest>(&bytes),
        Ok(None) | Err(_) => {
            let _ = write_control_response(stream, &response_error("控制请求为空或超过上限"));
            return Ok(false);
        }
    };
    match request {
        Ok(request) => {
            let (response, stop) = state.handle_control(request);
            // 单个客户端关闭或超时不决定宿主生命周期；合法停止请求仍生效。
            let _ = write_control_response(stream, &response);
            Ok(stop)
        }
        Err(_) => {
            let _ = write_control_response(stream, &response_error("控制请求格式不受支持"));
            Ok(false)
        }
    }
}

fn write_control_response(stream: &mut UnixStream, response: &ControlResponse) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(response).map_err(io::Error::other)?;
    if bytes.len() > CONTROL_RESPONSE_LIMIT {
        bytes = serde_json::to_vec(&response_error("控制响应超过大小上限"))
            .map_err(io::Error::other)?;
    }
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")
}

impl DaemonState {
    fn status(&self) -> RuntimeStatus {
        let now = now_ms();
        RuntimeStatus {
            state: "running".into(),
            uid: self.uid,
            bulk_file_threshold: self.bulk_file_threshold,
            bulk_window_ms: self.bulk_window_ms,
            collector_state: self.source_state.clone(),
            collector_run_id: self.adapter_run_id.clone(),
            collector_schema_version: self
                .adapter
                .as_ref()
                .and_then(|adapter| adapter.health().schema_version),
            collector_message_version: self
                .adapter
                .as_ref()
                .and_then(|adapter| adapter.health().message_version),
            collector_dropped_lines: self.collector_dropped_lines,
            reader_dropped_frames: self.reader_dropped_frames.load(Ordering::Relaxed),
            database_state: self.database_state.clone(),
            database_gap_events: self.database_gap_events,
            database_error: self.database_error.clone(),
            retention_state: self.retention_state.clone(),
            retention_last_run_ms: self.retention_last_run_ms,
            last_event_received_ms: self.last_event_received_ms,
            pipeline_timing: self.pipeline_timing.clone(),
            observed_events_by_kind: self.counters.observed.clone(),
            persisted_events_by_kind: self.counters.persisted.clone(),
            filtered_events_by_kind: self.counters.filtered.clone(),
            duplicate_events: self.counters.duplicates,
            memory_pending_notifications: self.memory_alerts.len(),
            memory_dropped_notifications: self.memory_dropped_notifications,
            notify_session_active: self.notify_session.as_ref().is_some_and(|session| {
                now.saturating_sub(session.last_seen_ms) <= NOTIFY_SESSION_TIMEOUT_MS
            }),
            notify_session_last_seen_ms: self
                .notify_session
                .as_ref()
                .map(|session| session.last_seen_ms),
        }
    }

    fn handle_control(&mut self, request: ControlRequest) -> (ControlResponse, bool) {
        let result = match request {
            ControlRequest::Status => json_value(self.status()),
            ControlRequest::AddDirectories { entries } => {
                self.add_directories(&entries).and_then(json_value)
            }
            ControlRequest::RemoveDirectory { path } => {
                self.remove_directory(&path).and_then(json_value)
            }
            ControlRequest::ListDirectories => self.storage.list_directories().and_then(json_value),
            ControlRequest::QueryEvents { mut filter } => {
                filter.limit = filter.limit.min(MAX_QUERY_LIMIT);
                self.storage.query_events(&filter).and_then(json_value)
            }
            ControlRequest::QueryAlerts { mut filter } => {
                filter.limit = filter.limit.min(MAX_QUERY_LIMIT);
                self.storage.query_alerts(&filter).and_then(json_value)
            }
            ControlRequest::QueryHealth { mut filter } => {
                filter.limit = filter.limit.min(MAX_QUERY_LIMIT);
                self.storage.query_health(&filter).and_then(json_value)
            }
            ControlRequest::QueryNotifications { mut filter } => {
                filter.limit = filter.limit.min(MAX_QUERY_LIMIT);
                self.storage
                    .query_notifications(&filter)
                    .and_then(json_value)
            }
            ControlRequest::Stats { since_ms, until_ms } => {
                let until = until_ms.unwrap_or_else(now_ms);
                let since = since_ms.unwrap_or_else(|| until.saturating_sub(24 * 60 * 60 * 1_000));
                self.storage.recent_stats(since, until).and_then(|recent| {
                    self.storage.cumulative_stats().map(|cumulative| {
                        serde_json::json!({
                            "since_ms": since,
                            "until_ms": until,
                            "recent": recent,
                            "cumulative": cumulative
                        })
                    })
                })
            }
            ControlRequest::ClearCumulativeStats => self.clear_cumulative_stats(),
            ControlRequest::PendingNotifications { limit } => self
                .storage
                .pending_notifications(limit.min(MAX_QUERY_LIMIT))
                .and_then(json_value),
            ControlRequest::NotifyPoll { session_id } => {
                self.notify_poll(&session_id).and_then(json_value)
            }
            ControlRequest::NotifySummaryResult {
                session_id,
                sequence,
                success,
            } => self
                .notify_summary_result(&session_id, sequence, success)
                .map(|_| serde_json::json!({"accepted": true})),
            ControlRequest::NotifyAlertResult {
                session_id,
                alert_id,
                success,
            } => self
                .notify_alert_result(&session_id, &alert_id, success)
                .map(|_| serde_json::json!({"accepted": true})),
            ControlRequest::Stop => {
                return (response_ok(&serde_json::json!({"stopping": true})), true);
            }
        };
        match result {
            Ok(data) => (
                ControlResponse {
                    ok: true,
                    error: None,
                    data: Some(data),
                },
                false,
            ),
            Err(_) => (
                response_error("宿主操作失败；错误详情未包含原始事件内容"),
                false,
            ),
        }
    }

    fn add_directories(&mut self, entries: &[DirectoryImport]) -> Result<Vec<DirectoryConfig>> {
        if entries.len() > 256 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "单次目录导入最多支持256个候选",
            )
            .into());
        }
        let now = now_ms();
        for entry in entries {
            let canonical = fs::canonicalize(&entry.path)?;
            if !canonical.is_dir() || entry.sources.is_empty() || entry.sources.len() > 16 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "导入目录必须存在、为目录且至少含一个来源",
                )
                .into());
            }
            for source in &entry.sources {
                if source.trim().is_empty()
                    || source.len() > 64
                    || !source.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':')
                    })
                {
                    return Err(
                        io::Error::new(io::ErrorKind::InvalidInput, "目录来源名称不合法").into(),
                    );
                }
                if self.database_state != "ready" {
                    return Err(io::Error::other("数据库正处于写入冷却期").into());
                }
                if let Err(error) = self.storage.add_directory(&canonical, source, now) {
                    self.database_write_failed(0);
                    return Err(error);
                }
            }
        }
        self.refresh_roots()?;
        self.storage.list_directories()
    }

    fn remove_directory(&mut self, path: &Path) -> Result<bool> {
        if self.database_state != "ready" {
            return Err(io::Error::other("数据库正处于写入冷却期").into());
        }
        match self.storage.remove_directory(path) {
            Ok(removed) => {
                self.refresh_roots()?;
                Ok(removed)
            }
            Err(error) => {
                self.database_write_failed(0);
                Err(error)
            }
        }
    }

    fn clear_cumulative_stats(&mut self) -> Result<Value> {
        if self.database_state != "ready" {
            return Err(io::Error::other("数据库正处于写入冷却期").into());
        }
        match self.storage.clear_cumulative_stats() {
            Ok(()) => Ok(serde_json::json!({"cleared": true, "details_preserved": true})),
            Err(error) => {
                self.database_write_failed(0);
                Err(error)
            }
        }
    }

    fn refresh_roots(&mut self) -> Result<()> {
        self.rules
            .replace_roots(self.storage.active_directory_paths()?)?;
        Ok(())
    }

    fn handle_reader_message(&mut self, message: ReaderMessage) {
        match message {
            ReaderMessage::State { state, code } => {
                if state != "connected" || self.source_diagnostic_state.is_none() {
                    self.source_state = state.clone();
                }
                self.write_health(
                    "collector",
                    &code,
                    if state == "stalled" { "gap" } else { &state },
                    if state == "stalled" {
                        "连续三秒未收到帧或心跳；连接保留，来源覆盖未知。"
                    } else {
                        "采集器连接状态变化；覆盖以状态记录为准。"
                    },
                );
            }
            ReaderMessage::Frame(frame) => {
                let started = Instant::now();
                self.handle_frame(frame);
                let elapsed_us = started.elapsed().as_micros() as u64;
                self.pipeline_timing.host_frames =
                    self.pipeline_timing.host_frames.saturating_add(1);
                self.pipeline_timing.host_processing_total_us = self
                    .pipeline_timing
                    .host_processing_total_us
                    .saturating_add(elapsed_us);
                self.pipeline_timing.host_processing_max_us =
                    self.pipeline_timing.host_processing_max_us.max(elapsed_us);
            }
        }
    }

    fn handle_frame(&mut self, frame: CollectorFrame) {
        match frame {
            CollectorFrame::Metrics { run_id, timing } => {
                // 指标只补充当前实例的状态，不建立 ES 事件或驱动健康判定。
                if self.adapter_run_id.as_deref() == Some(run_id.as_str()) {
                    self.pipeline_timing.collector = Some(timing);
                }
            }
            CollectorFrame::Line {
                run_id,
                line,
                received_timestamp_ms,
            } => {
                if run_id.len() > 128 {
                    self.write_health(
                        "collector",
                        "invalid_run_id",
                        "dropped",
                        "采集实例标识超过上限。",
                    );
                    return;
                }
                self.ensure_adapter(&run_id);
                let previous_version = self.adapter.as_ref().and_then(|adapter| {
                    let health = adapter.health();
                    (health.lines > 0).then_some((health.schema_version, health.message_version))
                });
                let outcome = self
                    .adapter
                    .as_mut()
                    .map(|adapter| adapter.parse_line(&line, received_timestamp_ms));
                let Some(outcome) = outcome else {
                    return;
                };
                let source = SourceContext {
                    run_id,
                    schema_version: outcome.schema_version,
                    message_version: outcome.message_version,
                    field: None,
                    missing_events: None,
                    pid: None,
                    pid_version: None,
                    global_seq: None,
                };
                if previous_version != Some((outcome.schema_version, outcome.message_version)) {
                    self.write_source_health(
                        "source_version_observed",
                        "observed",
                        "实际来源版本已观察；未知版本保持为空。",
                        source.clone(),
                    );
                }
                for issue in &outcome.issues {
                    self.write_source_health(
                        &issue.code,
                        "degraded",
                        &issue.message,
                        SourceContext {
                            field: issue.field.clone(),
                            missing_events: issue.missing_events,
                            pid: outcome.event.as_ref().map(|event| event.process.pid),
                            pid_version: outcome
                                .event
                                .as_ref()
                                .and_then(|event| event.process.pid_version),
                            global_seq: outcome.event.as_ref().and_then(|event| event.global_seq),
                            ..source.clone()
                        },
                    );
                }
                if let Some(event) = outcome.event {
                    self.handle_event(event);
                }
            }
            CollectorFrame::Heartbeat {
                run_id,
                dropped_lines,
            } => {
                if run_id.len() > 128 {
                    self.write_health(
                        "collector",
                        "invalid_run_id",
                        "dropped",
                        "采集实例标识超过上限。",
                    );
                    return;
                }
                self.ensure_adapter(&run_id);
                self.collector_dropped_lines = self.collector_dropped_lines.max(dropped_lines);
                if dropped_lines > self.last_collector_drop_count {
                    self.write_health(
                        "collector",
                        "collector_dropped_lines",
                        "gap",
                        "采集器队列有事件未能转发；监控覆盖存在缺口。",
                    );
                    self.last_collector_drop_count = dropped_lines;
                }
                if self.source_diagnostic_state.is_none() {
                    self.source_state = "connected".into();
                }
            }
            CollectorFrame::Status {
                run_id,
                state,
                message: _,
                dropped_lines,
            } => {
                if run_id.len() > 128 {
                    self.write_health(
                        "collector",
                        "invalid_run_id",
                        "dropped",
                        "采集实例标识超过上限。",
                    );
                    return;
                }
                self.ensure_adapter(&run_id);
                self.collector_dropped_lines = self.collector_dropped_lines.max(dropped_lines);
                let state: String = state.chars().take(32).collect();
                self.source_state = state.clone();
                self.source_diagnostic_state = if state == "connected" {
                    None
                } else {
                    Some(state.clone())
                };
                self.write_health(
                    "collector",
                    &format!("collector_{state}"),
                    &state,
                    "采集器报告状态变化；原始诊断未保存。",
                );
            }
        }
    }

    fn ensure_adapter(&mut self, run_id: &str) {
        if self.adapter_run_id.as_deref() == Some(run_id) {
            return;
        }
        let changed = self.adapter_run_id.is_some();
        self.pipeline_timing.collector = None;
        self.adapter_run_id = Some(run_id.to_owned());
        self.adapter = Some(EsloggerAdapter::new(run_id));
        self.source_state = "connected".into();
        self.source_diagnostic_state = None;
        if changed {
            self.deduper = Deduper::default();
            self.last_collector_drop_count = 0;
            self.write_health(
                "collector",
                "collector_restarted",
                "gap",
                "采集器实例已变化；重启前后的事件流不视为连续。",
            );
        }
    }

    fn handle_event(&mut self, event: ActivityEvent) {
        let kind = event_kind_name(event.kind).to_owned();
        *self.counters.observed.entry(kind.clone()).or_default() += 1;
        self.last_event_received_ms = Some(event.received_timestamp_ms);
        if let Some(timestamp) = event.source_timestamp_ms {
            let lag = event.received_timestamp_ms.saturating_sub(timestamp);
            self.pipeline_timing.source_to_collector_receive_max_ms = Some(
                self.pipeline_timing
                    .source_to_collector_receive_max_ms
                    .map_or(lag, |previous| previous.max(lag)),
            );
        }
        if !self.deduper.insert(&event) {
            self.counters.duplicates += 1;
            self.write_health(
                "runtime",
                "duplicate_event",
                "dropped",
                "同一来源序号重复，已丢弃重复记录。",
            );
            return;
        }
        let output = self.rules.process(&event);
        for health in &output.health {
            self.write_health("rules", &health.code, "observed", &health.detail);
        }
        if output.matched_directories.is_empty() {
            *self.counters.filtered.entry(kind.clone()).or_default() += 1;
        } else if self.database_state == "ready" {
            match self
                .storage
                .record_event(&event, &output.matched_directories)
            {
                Ok(true) => *self.counters.persisted.entry(kind).or_default() += 1,
                Ok(false) => {}
                Err(_) => self.database_write_failed(1),
            }
        } else {
            self.database_gap_events = self.database_gap_events.saturating_add(1);
        }
        for alert in output.alerts {
            if self.database_state == "ready" {
                match self.storage.record_alert_at(&alert, now_ms()) {
                    Ok(_) => {}
                    Err(_) => {
                        self.database_write_failed(1);
                        self.queue_alert(alert);
                    }
                }
            } else {
                self.database_gap_events = self.database_gap_events.saturating_add(1);
                self.queue_alert(alert);
            }
        }
    }

    fn queue_alert(&mut self, alert: Alert) {
        if let Some(existing) = self
            .memory_alerts
            .iter_mut()
            .find(|pending| pending.alert.id == alert.id)
        {
            existing.alert = alert.clone();
        } else if alert.is_new {
            if self.memory_alerts.len() == MEMORY_ALERT_CAPACITY {
                self.memory_alerts.pop_front();
                self.memory_dropped_notifications += 1;
            }
            self.memory_alerts.push_back(MemoryAlert {
                alert: alert.clone(),
                retry_after_ms: 0,
            });
        }
        if let Some(existing) = self
            .pending_alerts_mut()
            .find(|pending| pending.id == alert.id)
        {
            let is_new = existing.is_new || alert.is_new;
            *existing = alert;
            existing.is_new = is_new;
        } else if self.pending_alert_count() < MEMORY_ALERT_CAPACITY {
            self.deferred_alerts_mut().push_back(alert);
        } else {
            self.memory_dropped_notifications += 1;
        }
    }

    fn pending_alert_count(&self) -> usize {
        self.deferred_alerts.len()
    }

    fn pending_alerts_mut(&mut self) -> impl Iterator<Item = &mut Alert> {
        self.deferred_alerts.iter_mut()
    }

    fn deferred_alerts_mut(&mut self) -> &mut VecDeque<Alert> {
        &mut self.deferred_alerts
    }

    fn database_write_failed(&mut self, affected_events: u64) {
        self.database_state = "degraded".into();
        self.database_error =
            Some("SQLite 写入失败；分析继续，重试期间记录会出现覆盖缺口。".into());
        self.database_retry_at = Instant::now() + self.database_retry_cooldown;
        self.database_gap_events = self.database_gap_events.saturating_add(affected_events);
    }

    fn retry_database_if_due(&mut self) {
        if self.database_state == "ready" || Instant::now() < self.database_retry_at {
            return;
        }
        self.database_retry_at = Instant::now() + self.database_retry_cooldown;
        let recovery = HealthRecord {
            source: None,
            observed_timestamp_ms: now_ms(),
            component: "runtime".into(),
            code: "database_recovered".into(),
            state: "ready".into(),
            detail: Some("SQLite 写入已恢复；故障期间存在未持久化的事件。".into()),
        };
        if self.storage.record_health(&recovery).is_err() {
            return;
        }
        self.database_state = "ready".into();
        self.database_error = None;
        while let Some(mut alert) = self.deferred_alerts.front().cloned() {
            if self.sent_memory_alerts.iter().any(|id| id == &alert.id) {
                alert.is_new = false;
            }
            if self.storage.record_alert_at(&alert, now_ms()).is_err() {
                self.database_write_failed(0);
                return;
            }
            self.deferred_alerts.pop_front();
            // 恢复成功后由持久化 outbox 接管未发送告警，避免同时从内存再次投递。
            self.memory_alerts
                .retain(|pending| pending.alert.id != alert.id);
        }
        while let Some(alert_id) = self.sent_memory_alerts.front().cloned() {
            let feedback = NotificationRecord {
                alert_id,
                observed_timestamp_ms: now_ms(),
                outcome: NotificationOutcome::Sent,
                detail: None,
            };
            if self.storage.record_notification(&feedback).is_err() {
                self.database_write_failed(0);
                return;
            }
            self.sent_memory_alerts.pop_front();
        }
    }

    fn prune_if_due(&mut self) {
        if self.database_state != "ready" || Instant::now() < self.retention_at {
            return;
        }
        let now = now_ms();
        match self.storage.prune_expired(now) {
            Ok(pruned) => {
                self.retention_state = "ready".into();
                self.retention_last_run_ms = Some(now);
                self.retention_at = Instant::now() + self.retention_interval;
                if pruned.events
                    + pruned.alerts
                    + pruned.health_records
                    + pruned.notifications
                    + pruned.outbox_entries
                    > 0
                {
                    self.write_health(
                        "storage",
                        "details_expired",
                        "gap",
                        "超过30天的明细已清理，累计统计保留；过期部分无法完整追溯。",
                    );
                }
            }
            Err(_) => {
                self.retention_state = "failed".into();
                self.retention_at = Instant::now() + self.database_retry_cooldown;
                self.database_write_failed(0);
            }
        }
    }

    fn write_source_health(
        &mut self,
        code: &str,
        state: &str,
        detail: &str,
        source: SourceContext,
    ) {
        if self.database_state != "ready"
            || self
                .storage
                .record_health(&HealthRecord {
                    observed_timestamp_ms: now_ms(),
                    component: "eslogger".into(),
                    code: code.to_owned(),
                    state: state.to_owned(),
                    detail: Some(detail.to_owned()),
                    source: Some(source),
                })
                .is_err()
        {
            self.database_write_failed(0);
        }
    }

    fn write_health(&mut self, component: &str, code: &str, state: &str, detail: &str) {
        let now = now_ms();
        let key = format!("{component}:{code}");
        if self
            .last_health_record_at
            .get(&key)
            .is_some_and(|last| now.saturating_sub(*last) < 60_000)
        {
            return;
        }
        self.last_health_record_at.insert(key.clone(), now);
        while self.last_health_record_at.len() > 128 {
            if let Some(oldest) = self
                .last_health_record_at
                .iter()
                .min_by_key(|(_, timestamp)| *timestamp)
                .map(|(key, _)| key.clone())
            {
                self.last_health_record_at.remove(&oldest);
            }
        }
        if self.database_state != "ready"
            || self
                .storage
                .record_health(&HealthRecord {
                    source: None,
                    observed_timestamp_ms: now,
                    component: component.to_owned(),
                    code: code.chars().take(64).collect(),
                    state: state.chars().take(32).collect(),
                    detail: Some(detail.chars().take(512).collect()),
                })
                .is_err()
        {
            self.database_write_failed(0);
        }
    }

    fn notify_poll(&mut self, session_id: &str) -> Result<NotifyPollResult> {
        if session_id.is_empty()
            || session_id.len() > 128
            || !session_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "通知会话标识不合法").into());
        }
        let now = now_ms();
        let is_new_session = self
            .notify_session
            .as_ref()
            .is_none_or(|session| session.id != session_id);
        if is_new_session {
            self.notify_session = Some(NotifySession {
                id: session_id.to_owned(),
                last_seen_ms: now,
                initialized: false,
                summary: None,
                summary_retry_after_ms: 0,
            });
        }
        if let Some(session) = self.notify_session.as_mut() {
            session.last_seen_ms = now;
        }

        let needs_summary = self
            .notify_session
            .as_ref()
            .is_some_and(|session| !session.initialized);
        if needs_summary && self.database_state == "ready" {
            match self.storage.pending_notification_summary() {
                Ok(summary) if summary.count > 0 => {
                    let sequence = summary
                        .latest_sequence
                        .ok_or_else(|| io::Error::other("通知快照缺少序列号"))?;
                    if let Some(session) = self.notify_session.as_mut() {
                        session.summary = Some((summary, sequence));
                        session.initialized = true;
                        session.summary_retry_after_ms = 0;
                    }
                }
                Ok(_) => {
                    if let Some(session) = self.notify_session.as_mut() {
                        session.initialized = true;
                    }
                }
                Err(_) => self.database_write_failed(0),
            }
        } else if needs_summary && self.database_state != "ready" {
            // 数据库不可读时先保留初始化状态，恢复后仍先做历史汇总。
        }

        if let Some((summary, sequence)) = self
            .notify_session
            .as_ref()
            .and_then(|session| session.summary.clone())
        {
            let retry_at = self
                .notify_session
                .as_ref()
                .map_or(0, |session| session.summary_retry_after_ms);
            if now >= retry_at {
                return Ok(NotifyPollResult::Summary { summary, sequence });
            }
        }

        if let Some(index) = self
            .memory_alerts
            .iter()
            .position(|pending| pending.retry_after_ms <= now)
        {
            if let Some(pending) = self.memory_alerts.get_mut(index) {
                pending.retry_after_ms = now.saturating_add(NOTIFY_RETRY_COOLDOWN_MS);
                return Ok(NotifyPollResult::Alert {
                    alert: pending.alert.clone(),
                    persisted: false,
                });
            }
        }

        if self.database_state == "ready" {
            match self.storage.pending_notifications(64) {
                Ok(pending) => {
                    for PendingNotification { alert, .. } in pending {
                        if self.sent_memory_alerts.iter().any(|id| id == &alert.id) {
                            continue;
                        }
                        let retry_at = self
                            .retry_alerts
                            .get(&alert.id)
                            .map_or(0, |retry| retry.retry_after_ms);
                        if retry_at > now {
                            continue;
                        }
                        self.set_alert_retry(
                            &alert.id,
                            now.saturating_add(NOTIFY_RETRY_COOLDOWN_MS),
                        );
                        return Ok(NotifyPollResult::Alert {
                            alert,
                            persisted: true,
                        });
                    }
                }
                Err(_) => self.database_write_failed(0),
            }
        }
        Ok(NotifyPollResult::Idle {
            database_state: self.database_state.clone(),
            memory_pending: !self.memory_alerts.is_empty(),
        })
    }

    fn notify_summary_result(
        &mut self,
        session_id: &str,
        sequence: i64,
        success: bool,
    ) -> Result<()> {
        let Some(session) = self.notify_session.as_mut() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "通知会话不存在").into());
        };
        if session.id != session_id
            || session.summary.as_ref().map(|(_, current)| *current) != Some(sequence)
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "通知汇总快照已变化").into());
        }
        if !success {
            session.summary_retry_after_ms = now_ms().saturating_add(NOTIFY_RETRY_COOLDOWN_MS);
            self.write_health(
                "notify",
                "summary_send_failed",
                "retrying",
                "桌面通知未确认发送；待处理摘要仍保留。",
            );
            return Ok(());
        }
        if self.database_state != "ready" {
            if let Some(session) = self.notify_session.as_mut() {
                session.summary_retry_after_ms = now_ms().saturating_add(NOTIFY_RETRY_COOLDOWN_MS);
            }
            return Err(io::Error::other("数据库不可用，汇总确认将在重试后完成").into());
        }
        match self
            .storage
            .acknowledge_pending_notifications_through(sequence, now_ms())
        {
            Ok(_) => {
                if let Some(session) = self.notify_session.as_mut() {
                    session.summary = None;
                    session.initialized = true;
                    session.summary_retry_after_ms = 0;
                }
                Ok(())
            }
            Err(error) => {
                self.database_write_failed(0);
                if let Some(session) = self.notify_session.as_mut() {
                    session.summary_retry_after_ms =
                        now_ms().saturating_add(NOTIFY_RETRY_COOLDOWN_MS);
                }
                Err(error)
            }
        }
    }

    fn notify_alert_result(
        &mut self,
        session_id: &str,
        alert_id: &str,
        success: bool,
    ) -> Result<()> {
        if self
            .notify_session
            .as_ref()
            .is_none_or(|session| session.id != session_id)
        {
            return Err(io::Error::new(io::ErrorKind::NotFound, "通知会话不存在").into());
        }
        let now = now_ms();
        let is_memory = self
            .memory_alerts
            .iter()
            .any(|pending| pending.alert.id == alert_id);
        if success {
            if self.database_state == "ready" {
                let record = NotificationRecord {
                    alert_id: alert_id.to_owned(),
                    observed_timestamp_ms: now,
                    outcome: NotificationOutcome::Sent,
                    detail: None,
                };
                if self.storage.record_notification(&record).is_err() {
                    self.database_write_failed(0);
                    self.remember_sent_alert(alert_id);
                }
            } else {
                self.remember_sent_alert(alert_id);
            }
            self.memory_alerts
                .retain(|pending| pending.alert.id != alert_id);
            self.retry_alerts.remove(alert_id);
            if is_memory && self.database_state != "ready" {
                // 发送已成功但 alert 尚未落库；保留成功事实以避免恢复后重复通知。
                self.remember_sent_alert(alert_id);
            }
        } else {
            self.set_alert_retry(alert_id, now.saturating_add(NOTIFY_RETRY_COOLDOWN_MS));
            if self.database_state == "ready" && !is_memory {
                let record = NotificationRecord {
                    alert_id: alert_id.to_owned(),
                    observed_timestamp_ms: now,
                    outcome: NotificationOutcome::Failed,
                    detail: Some("系统通知发送失败；将在冷却后重试。".into()),
                };
                if self.storage.record_notification(&record).is_err() {
                    self.database_write_failed(0);
                }
            }
            if let Some(pending) = self
                .memory_alerts
                .iter_mut()
                .find(|pending| pending.alert.id == alert_id)
            {
                pending.retry_after_ms = now.saturating_add(NOTIFY_RETRY_COOLDOWN_MS);
            }
        }
        Ok(())
    }

    fn remember_sent_alert(&mut self, alert_id: &str) {
        if self
            .sent_memory_alerts
            .iter()
            .any(|existing| existing == alert_id)
        {
            return;
        }
        self.sent_memory_alerts.push_back(alert_id.to_owned());
        while self.sent_memory_alerts.len() > MEMORY_ALERT_CAPACITY {
            self.sent_memory_alerts.pop_front();
            self.memory_dropped_notifications += 1;
        }
    }

    fn set_alert_retry(&mut self, alert_id: &str, retry_after_ms: i64) {
        self.retry_alerts
            .insert(alert_id.to_owned(), RetryAlert { retry_after_ms });
        while self.retry_alerts.len() > MEMORY_ALERT_CAPACITY {
            if let Some(first) = self.retry_alerts.keys().next().cloned() {
                self.retry_alerts.remove(&first);
            }
        }
    }
}

/// 通知helper单轮；fake sender 仅供合成测试注入，不由CLI选择。
#[doc(hidden)]
pub fn notify_once_with_sender(
    control_socket: &Path,
    session_id: &str,
    sender: &mut impl NotificationSender,
) -> Result<bool> {
    let response = request_control(
        control_socket,
        ControlRequest::NotifyPoll {
            session_id: session_id.to_owned(),
        },
    )?;
    if !response.ok {
        return Err(
            io::Error::other(response.error.unwrap_or_else(|| "宿主通知请求失败".into())).into(),
        );
    }
    let result: NotifyPollResult = serde_json::from_value(
        response
            .data
            .ok_or_else(|| io::Error::other("通知响应缺少数据"))?,
    )?;
    match result {
        NotifyPollResult::Summary { summary, sequence } => {
            let body = summary_notification_body(&summary);
            let sent = sender.send("CodePerimeter 活动摘要", &body).is_ok();
            notify_send_result(
                control_socket,
                ControlRequest::NotifySummaryResult {
                    session_id: session_id.to_owned(),
                    sequence,
                    success: sent,
                },
            )?;
            Ok(sent)
        }
        NotifyPollResult::Alert { alert, persisted } => {
            let title = match alert.rule {
                crate::model::AlertRule::BulkFileAccess => "检测到批量文件访问",
                crate::model::AlertRule::ArchiveCommand => "检测到归档命令",
                crate::model::AlertRule::ArchiveOutput => "检测到归档输出",
            };
            let body = format!(
                "PID {} · {} 个文件 · {}",
                alert.process.pid,
                alert.unique_files,
                if persisted {
                    "已记录"
                } else {
                    "暂存于内存"
                }
            );
            let sent = sender.send(title, &body).is_ok();
            notify_send_result(
                control_socket,
                ControlRequest::NotifyAlertResult {
                    session_id: session_id.to_owned(),
                    alert_id: alert.id,
                    success: sent,
                },
            )?;
            Ok(sent)
        }
        NotifyPollResult::Idle { .. } => Ok(false),
    }
}

/// 连续发送有限数量的待处理通知，生产helper用它降低告警突发时的排队延迟。
#[doc(hidden)]
pub fn notify_burst_with_sender(
    control_socket: &Path,
    session_id: &str,
    sender: &mut impl NotificationSender,
    maximum: usize,
) -> Result<usize> {
    let maximum = maximum.clamp(1, MAX_NOTIFY_BURST);
    let mut sent = 0;
    for _ in 0..maximum {
        if !notify_once_with_sender(control_socket, session_id, sender)? {
            break;
        }
        sent += 1;
    }
    Ok(sent)
}

fn notify_send_result(control_socket: &Path, request: ControlRequest) -> Result<()> {
    let response = request_control(control_socket, request)?;
    if !response.ok {
        return Err(io::Error::other(
            response
                .error
                .unwrap_or_else(|| "通知发送结果未能记录".into()),
        )
        .into());
    }
    Ok(())
}

fn summary_notification_body(summary: &PendingNotificationSummary) -> String {
    let rules = summary
        .by_rule
        .iter()
        .map(|count| format!("{:?}: {}", count.rule, count.count))
        .collect::<Vec<_>>()
        .join("，");
    if rules.is_empty() {
        format!("有 {} 条待处理活动告警", summary.count)
    } else {
        format!("有 {} 条待处理活动告警（{}）", summary.count, rules)
    }
}

/// 生产通知helper；保持轮询直到launchd停止该用户进程。
pub fn run_notify(control_socket: &Path) -> Result<()> {
    let session_id = format!("notify-{}-{}", std::process::id(), now_ms());
    let mut sender = OsascriptSender;
    loop {
        match notify_burst_with_sender(control_socket, &session_id, &mut sender, MAX_NOTIFY_BURST) {
            Ok(sent) if sent > 0 => continue,
            Ok(_) => thread::sleep(Duration::from_millis(250)),
            Err(_) => {
                eprintln!("CodePerimeter 通知宿主暂不可用，将重试。");
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

struct OsascriptSender;

impl NotificationSender for OsascriptSender {
    fn send(&mut self, title: &str, body: &str) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let script = "on run argv\ndisplay notification (item 1 of argv) with title (item 2 of argv)\nend run";
            let status = Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg(script)
                .arg(body)
                .arg(title)
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(io::Error::other("系统通知发送失败").into())
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (title, body);
            Err(io::Error::new(io::ErrorKind::Unsupported, "系统通知仅支持 macOS").into())
        }
    }
}

#[cfg(test)]
mod reader_tests {
    use super::*;

    #[test]
    fn reader_distinguishes_idle_stall_recovery_invalid_frame_and_eof_and_joins_on_stop() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("collector.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let (sender, receiver) = mpsc::sync_channel(2);
        let stopping = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicU64::new(0));
        let reader = spawn_collector_reader(
            path,
            unsafe { libc::geteuid() },
            sender,
            Arc::clone(&drops),
            Arc::clone(&stopping),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            ReaderMessage::State { code, .. } if code == "collector_connected")
        );
        let heartbeat = CollectorFrame::Heartbeat {
            run_id: "anonymous".into(),
            dropped_lines: 0,
        };
        crate::service::write_frame(&mut stream, &heartbeat).unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            ReaderMessage::Frame(_)
        ));
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(4)).unwrap(),
            ReaderMessage::State { state, code } if state == "stalled" && code == "collector_frame_timeout")
        );
        crate::service::write_frame(&mut stream, &heartbeat).unwrap();
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            ReaderMessage::State { code, .. } if code == "collector_stream_resumed")
        );
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            ReaderMessage::Frame(_)
        ));
        stream
            .write_all(b"anonymous-invalid-private-marker\n")
            .unwrap();
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            ReaderMessage::State { state, code } if state == "coverage_gap" && code == "collector_invalid_frame")
        );
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        drop(stream);
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            ReaderMessage::State { code, .. } if code == "collector_eof")
        );
        stopping.store(true, Ordering::Relaxed);
        drop(receiver);
        reader.join().unwrap();
    }

    #[test]
    fn queued_lifecycle_message_waits_for_capacity_and_stop_cancels_backpressure() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(ReaderMessage::State {
                state: "connected".into(),
                code: "collector_connected".into(),
            })
            .unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::spawn(move || {
            send_reader_message(
                &sender,
                ReaderMessage::State {
                    state: "reconnecting".into(),
                    code: "collector_eof".into(),
                },
                &worker_stopping,
            )
        });
        thread::sleep(Duration::from_millis(20));
        assert!(!worker.is_finished());
        receiver.recv().unwrap();
        assert!(worker.join().unwrap());
        assert!(
            matches!(receiver.recv().unwrap(), ReaderMessage::State { code, .. } if code == "collector_eof")
        );

        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(ReaderMessage::Frame(CollectorFrame::Heartbeat {
                run_id: "anonymous".into(),
                dropped_lines: 0,
            }))
            .unwrap();
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::spawn(move || {
            send_reader_message(
                &sender,
                ReaderMessage::Frame(CollectorFrame::Heartbeat {
                    run_id: "anonymous".into(),
                    dropped_lines: 0,
                }),
                &worker_stopping,
            )
        });
        thread::sleep(Duration::from_millis(20));
        stopping.store(true, Ordering::Relaxed);
        drop(receiver);
        assert!(!worker.join().unwrap());
    }
}
