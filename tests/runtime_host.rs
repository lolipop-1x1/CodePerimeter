use chrono::DateTime;
use codeperimeter::model::now_ms;
use codeperimeter::runtime::{
    ControlRequest, DirectoryImport, NotificationSender, RuntimeOptions, RuntimeStatus,
    notify_burst_with_sender, notify_once_with_sender, request_control,
    run_daemon_with_expected_collector_uid,
};
use codeperimeter::service::{CollectorFrame, write_frame};
use codeperimeter::storage::{AlertFilter, EventFilter, HealthFilter, NotificationFilter};
use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tempfile::{Builder, TempDir};

const BASE_TIME_MS: i64 = 1_790_985_600_000;

enum SourceCommand {
    Frame(CollectorFrame),
    Disconnect,
    Stop,
}

struct SyntheticSource {
    commands: mpsc::Sender<SourceCommand>,
    connected: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl SyntheticSource {
    fn start(path: &Path) -> Self {
        let listener = UnixListener::bind(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        let (command_tx, command_rx) = mpsc::channel();
        let (connected_tx, connected_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                if connected_tx.send(()).is_err() {
                    return;
                }
                loop {
                    match command_rx.recv() {
                        Ok(SourceCommand::Frame(frame)) => {
                            if write_frame(&mut stream, &frame).is_err() {
                                break;
                            }
                        }
                        Ok(SourceCommand::Disconnect) => break,
                        Ok(SourceCommand::Stop) | Err(_) => return,
                    }
                }
            }
        });
        Self {
            commands: command_tx,
            connected: connected_rx,
            thread: Some(thread),
        }
    }

    fn wait_connected(&self) {
        self.connected
            .recv_timeout(Duration::from_secs(5))
            .expect("宿主应连接合成采集器");
    }

    fn send(&self, frame: CollectorFrame) {
        self.commands.send(SourceCommand::Frame(frame)).unwrap();
    }

    fn disconnect(&self) {
        self.commands.send(SourceCommand::Disconnect).unwrap();
    }

    fn stop(mut self) {
        let _ = self.commands.send(SourceCommand::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct RecordingSender {
    messages: Vec<(String, String)>,
}

impl NotificationSender for RecordingSender {
    fn send(&mut self, title: &str, body: &str) -> codeperimeter::Result<()> {
        self.messages.push((title.to_owned(), body.to_owned()));
        Ok(())
    }
}

fn fixture() -> TempDir {
    let temp = Builder::new()
        .prefix("codeperimeter-runtime-")
        .tempdir_in("/private/tmp")
        .unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    temp
}

fn options(root: &Path) -> RuntimeOptions {
    RuntimeOptions {
        collector_socket: root.join("collector.sock"),
        control_socket: root.join("host.sock"),
        database_path: root.join("events.sqlite"),
        bulk_file_threshold: 50,
        bulk_window_ms: 10_000,
    }
}

fn start_host(options: RuntimeOptions) -> JoinHandle<codeperimeter::Result<()>> {
    let uid = unsafe { libc::geteuid() };
    thread::spawn(move || {
        let result = run_daemon_with_expected_collector_uid(options, uid);
        if let Err(error) = &result {
            eprintln!("host test runtime failed: {error}");
        }
        result
    })
}

fn wait_for_socket(path: &Path) {
    wait_until(|| path.exists(), "宿主应建立控制 socket");
}

fn wait_until(mut check: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(check(), "{message}");
}

fn query<T: DeserializeOwned>(socket: &Path, request: ControlRequest) -> T {
    let response = request_control(socket, request.clone())
        .unwrap_or_else(|error| panic!("IPC请求 {request:?} 未完成：{error}"));
    assert!(response.ok, "{:?}", response.error);
    serde_json::from_value(response.data.unwrap()).unwrap()
}

fn stop_host(socket: &Path, handle: JoinHandle<codeperimeter::Result<()>>) {
    let response = request_control(socket, ControlRequest::Stop).unwrap();
    assert!(response.ok);
    handle.join().unwrap().unwrap();
}

fn create_files(directory: &Path, prefix: &str) -> Vec<PathBuf> {
    (0..50)
        .map(|index| {
            let path = directory.join(format!("{prefix}-{index}.txt"));
            fs::write(&path, b"synthetic test bytes").unwrap();
            path
        })
        .collect()
}

fn open_frame(path: &Path, process_id: u32, generation: u32, sequence: u64) -> CollectorFrame {
    let metadata = fs::metadata(path).unwrap();
    let event = json!({
        "schema_version": 1,
        "version": 9,
        "action_type": 1,
        "event_type": 10,
        "seq_num": sequence,
        "global_seq_num": sequence,
        "time": DateTime::from_timestamp_millis(BASE_TIME_MS + (sequence as i64 * 50))
            .unwrap()
            .to_rfc3339(),
        "process": {
            "audit_token": {"pid": process_id, "pidversion": generation},
            "ppid": 1,
            "executable": {"path": "/usr/bin/synthetic-reader", "path_truncated": false},
            "signing_id": "example.synthetic",
            "team_id": null
        },
        "event": {"open": {
            "fflag": 1,
            "file": {
                "path": path,
                "path_truncated": false,
                "stat": {
                    "st_mode": metadata.mode(),
                    "st_dev": metadata.dev(),
                    "st_ino": metadata.ino()
                }
            }
        }}
    });
    CollectorFrame::Line {
        run_id: "synthetic-run-01".into(),
        line: event.to_string(),
        received_timestamp_ms: now_ms(),
    }
}

fn send_batch(
    source: &SyntheticSource,
    files: &[PathBuf],
    process_id: u32,
    generation: u32,
    start: u64,
) {
    for (offset, path) in files.iter().enumerate() {
        source.send(open_frame(
            path,
            process_id,
            generation,
            start + offset as u64,
        ));
        thread::sleep(Duration::from_millis(4));
    }
}

#[test]
fn control_ipc_configures_directories_and_reports_runtime_health() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let handle = start_host(runtime);
    wait_for_socket(&control_socket);

    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.state, "running");
    assert_eq!(status.bulk_file_threshold, 50);
    assert_eq!(status.bulk_window_ms, 10_000);
    assert_eq!(status.observed_events_by_kind["open"], 0);

    let _: Vec<Value> = query(
        &control_socket,
        ControlRequest::AddDirectories {
            entries: vec![DirectoryImport {
                path: root.clone(),
                sources: vec!["manual".into()],
            }],
        },
    );
    let directories: Vec<Value> = query(&control_socket, ControlRequest::ListDirectories);
    assert_eq!(directories.len(), 1);
    assert_eq!(
        directories[0]["path"],
        root.canonicalize().unwrap().to_str().unwrap()
    );

    let _: bool = query(
        &control_socket,
        ControlRequest::RemoveDirectory { path: root },
    );
    let directories: Vec<Value> = query(&control_socket, ControlRequest::ListDirectories);
    assert!(directories.is_empty());
    stop_host(&control_socket, handle);
}

#[test]
fn selected_directory_events_trigger_alerts_once_and_reconnects_are_visible() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let files = create_files(&project, "source");
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let collector_socket = runtime.collector_socket.clone();
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    let _: Vec<Value> = query(
        &control_socket,
        ControlRequest::AddDirectories {
            entries: vec![DirectoryImport {
                path: project.clone(),
                sources: vec!["manual".into()],
            }],
        },
    );
    let source = SyntheticSource::start(&collector_socket);
    source.wait_connected();

    let mut sender = RecordingSender {
        messages: Vec::new(),
    };
    send_batch(&source, &files, 702, 8, 0);
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.observed_events_by_kind["open"] == 50
                && status.persisted_events_by_kind["open"] == 50
        },
        "50个目录内文件访问应写入SQLite",
    );
    let alerts: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryAlerts {
            filter: AlertFilter::default(),
        },
    );
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["rule"], "bulk_file_access");
    assert!(notify_once_with_sender(&control_socket, "helper-session-a", &mut sender).unwrap());
    assert_eq!(sender.messages.len(), 1);
    assert!(sender.messages[0].0.contains("摘要"));
    assert!(
        !notify_once_with_sender(&control_socket, "helper-session-a", &mut sender).unwrap(),
        "历史快照汇总成功后不应重发"
    );

    let outside_dir = temp.path().join("outside");
    fs::create_dir(&outside_dir).unwrap();
    let outside = outside_dir.join("unmonitored.txt");
    fs::write(&outside, b"outside synthetic bytes").unwrap();
    source.send(open_frame(&outside, 702, 8, 100));
    thread::sleep(Duration::from_millis(30));

    // 同一进程在60秒内再次触发滚动门槛，只合并告警，不重新进入outbox。
    let second_files = create_files(&project, "more");
    send_batch(&source, &second_files, 702, 8, 101);
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.observed_events_by_kind["open"] == 101
                && status.persisted_events_by_kind["open"] == 100
                && status.filtered_events_by_kind["open"] == 1
        },
        "第二批目录内访问应完成分析",
    );
    let more_files = create_files(&project, "other-process");
    send_batch(&source, &more_files, 703, 9, 151);
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.observed_events_by_kind["open"] == 151
                && status.persisted_events_by_kind["open"] == 150
        },
        "另一个进程的访问应形成独立告警",
    );
    assert!(notify_once_with_sender(&control_socket, "helper-session-a", &mut sender).unwrap());
    assert_eq!(sender.messages.len(), 2);
    assert!(sender.messages[1].0.contains("批量"));

    let merged_files = create_files(&project, "merged");
    send_batch(&source, &merged_files, 703, 9, 201);
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.observed_events_by_kind["open"] == 201
                && status.persisted_events_by_kind["open"] == 200
        },
        "60秒内合并批次应完成分析",
    );
    let summary: Value = query(
        &control_socket,
        ControlRequest::Stats {
            since_ms: Some(0),
            until_ms: None,
        },
    );
    assert_eq!(summary["cumulative"]["alerts"], 2);
    assert_eq!(summary["cumulative"]["notifications_sent"], 1);
    let pending: Vec<Value> = query(
        &control_socket,
        ControlRequest::PendingNotifications { limit: 10 },
    );
    assert!(pending.is_empty());
    let scoped_events: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryEvents {
            filter: EventFilter {
                directory: Some(project.clone()),
                kind: Some(codeperimeter::model::EventKind::Open),
                limit: 100,
                ..EventFilter::default()
            },
        },
    );
    assert_eq!(scoped_events.len(), 100);
    assert!(
        scoped_events
            .iter()
            .all(|row| row["directories"][0] == project.to_str().unwrap())
    );
    let outside_events: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryEvents {
            filter: EventFilter {
                file_path: Some(outside),
                ..EventFilter::default()
            },
        },
    );
    assert!(outside_events.is_empty());

    // 连续三个不同进程各形成一条告警，通知helper应在一个突发批次内全部发送。
    for (offset, process_id) in [704, 705, 706].into_iter().enumerate() {
        let burst_files = create_files(&project, &format!("burst-{process_id}"));
        send_batch(
            &source,
            &burst_files,
            process_id,
            10 + offset as u32,
            251 + (offset as u64 * 50),
        );
    }
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.observed_events_by_kind["open"] == 351
                && status.persisted_events_by_kind["open"] == 350
        },
        "三组突发访问应完成分析并生成独立告警",
    );
    let burst_count =
        notify_burst_with_sender(&control_socket, "helper-session-a", &mut sender, 8).unwrap();
    assert_eq!(burst_count, 3);
    assert_eq!(sender.messages.len(), 5);

    source.disconnect();
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.collector_state == "reconnecting"
        },
        "采集连接断开应显示重连状态",
    );
    source.wait_connected();
    source.send(CollectorFrame::Heartbeat {
        run_id: "synthetic-run-02".into(),
        dropped_lines: 0,
    });
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.collector_run_id.as_deref() == Some("synthetic-run-02")
        },
        "重连后应显示新的采集代际",
    );
    let health: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryHealth {
            filter: HealthFilter {
                component: Some("collector".into()),
                ..HealthFilter::default()
            },
        },
    );
    assert!(
        health
            .iter()
            .any(|row| row["record"]["code"] == "collector_restarted")
    );

    source.send(CollectorFrame::Status {
        run_id: "synthetic-run-02".into(),
        state: "permission_denied".into(),
        message: "synthetic permission diagnostic".into(),
        dropped_lines: 0,
    });
    source.send(CollectorFrame::Heartbeat {
        run_id: "synthetic-run-02".into(),
        dropped_lines: 1,
    });
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.collector_dropped_lines == 1 && status.collector_state == "permission_denied"
        },
        "包含丢弃计数的桥接心跳不得覆盖采集源的权限诊断",
    );

    source.stop();
    stop_host(&control_socket, host);
}

#[test]
fn database_lock_does_not_stop_analysis_or_ephemeral_notification() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let files = create_files(&project, "locked");
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let database = runtime.database_path.clone();
    let collector_socket = runtime.collector_socket.clone();
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    let _: Vec<Value> = query(
        &control_socket,
        ControlRequest::AddDirectories {
            entries: vec![DirectoryImport {
                path: project,
                sources: vec!["manual".into()],
            }],
        },
    );
    let source = SyntheticSource::start(&collector_socket);
    source.wait_connected();
    let lock = Connection::open(database).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();

    send_batch(&source, &files, 801, 12, 0);
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.observed_events_by_kind["open"] == 50 && status.database_state == "degraded"
        },
        "SQLite被锁后规则仍应处理完批量访问",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.persisted_events_by_kind["open"], 0);
    assert!(status.database_gap_events >= 50);

    let mut sender = RecordingSender {
        messages: Vec::new(),
    };
    assert!(
        notify_once_with_sender(&control_socket, "helper-db-down", &mut sender).unwrap(),
        "SQLite降级时新告警仍应走有界内存通知"
    );
    assert_eq!(sender.messages.len(), 1);
    assert!(sender.messages[0].0.contains("批量"));

    lock.execute_batch("ROLLBACK").unwrap();
    drop(lock);
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.database_state == "ready"
        },
        "释放写锁后SQLite应经冷却重试恢复",
    );
    let alerts: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryAlerts {
            filter: AlertFilter::default(),
        },
    );
    assert_eq!(alerts.len(), 1);
    let notifications: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryNotifications {
            filter: NotificationFilter {
                outcome: Some(codeperimeter::storage::NotificationOutcome::Sent),
                ..NotificationFilter::default()
            },
        },
    );
    assert_eq!(notifications.len(), 1);

    source.stop();
    stop_host(&control_socket, host);
}
