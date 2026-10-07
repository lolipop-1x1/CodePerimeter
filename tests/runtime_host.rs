use chrono::DateTime;
use codeperimeter::model::{SourceStream, now_ms};
use codeperimeter::runtime::{
    ControlRequest, DirectoryImport, NotificationSender, RuntimeOptions, RuntimeStatus,
    notify_burst_with_sender, notify_once_with_sender, request_control,
    run_daemon_with_expected_collector_uid,
};
use codeperimeter::service::{CollectorFrame, CollectorTiming, write_frame};
use codeperimeter::storage::{AlertFilter, EventFilter, HealthFilter, NotificationFilter};
use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::net::Shutdown;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
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

#[derive(Default)]
struct RecordingSender {
    messages: Vec<(String, String)>,
    targets: Vec<codeperimeter::runtime::NotificationTarget>,
}

impl NotificationSender for RecordingSender {
    fn send(
        &mut self,
        title: &str,
        body: &str,
        target: &codeperimeter::runtime::NotificationTarget,
    ) -> codeperimeter::Result<()> {
        self.messages.push((title.to_owned(), body.to_owned()));
        self.targets.push(target.clone());
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
    serde_json::from_value(response.data.unwrap_or(Value::Null)).unwrap()
}

fn collector_eof_health(socket: &Path) -> Vec<Value> {
    query(
        socket,
        ControlRequest::QueryHealth {
            filter: HealthFilter {
                component: Some("collector".into()),
                code: Some("collector_eof".into()),
                ..HealthFilter::default()
            },
        },
    )
}

fn stop_host(socket: &Path, handle: JoinHandle<codeperimeter::Result<()>>) {
    let response = request_control(socket, ControlRequest::Stop).unwrap();
    assert!(response.ok);
    handle.join().unwrap().unwrap();
}

#[test]
fn split_sources_keep_shared_identity_independent_sequences_and_bounded_exec_receipts() {
    let temp = fixture();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let file = project.join("source.txt");
    fs::write(&file, b"anonymous source").unwrap();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let host = start_host(runtime.clone());
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
    let source = SyntheticSource::start(&runtime.collector_socket);
    source.wait_connected();
    let exec_frame = |pid, sequence, input: &Path| {
        let CollectorFrame::Line {
            run_id,
            line,
            received_timestamp_ms,
            ..
        } = open_frame(&file, pid, 7, sequence)
        else {
            unreachable!()
        };
        let mut value: Value = serde_json::from_str(&line).unwrap();
        let mut target = value["process"].clone();
        target["executable"]["path"] = json!("/usr/bin/gzip");
        value["event_type"] = json!(9);
        value["event"] = json!({"exec": {"target": target, "cwd": {"path": project, "path_truncated": false}, "args": ["gzip", "-c", input]}});
        CollectorFrame::Line {
            run_id,
            source_stream: SourceStream::Exec,
            line: value.to_string(),
            received_timestamp_ms,
        }
    };
    let receipt_request = |pid| ControlRequest::ExecReceipt {
        run_id: "synthetic-run-01".into(),
        pid,
        pid_version: 7,
    };
    source.send(exec_frame(700, 1, &file));
    wait_until(
        || query::<Option<Value>>(&control_socket, receipt_request(700)).is_some(),
        "主 exec 应在处理后提供精确回执",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.collector_schema_version, None);
    assert_eq!(status.collector_streams[&SourceStream::Activity].lines, 0);
    let receipt: Option<Value> = query(&control_socket, receipt_request(700));
    assert_eq!(receipt.unwrap()["source_stream"], "exec");
    let wrong_generation: Option<Value> = query(
        &control_socket,
        ControlRequest::ExecReceipt {
            run_id: "synthetic-run-01".into(),
            pid: 700,
            pid_version: 8,
        },
    );
    assert!(wrong_generation.is_none());
    let mut activity = open_frame(&file, 700, 7, 1);
    if let CollectorFrame::Line { source_stream, .. } = &mut activity {
        *source_stream = SourceStream::Activity;
    }
    source.send(activity.clone());
    wait_until(
        || {
            query::<RuntimeStatus>(&control_socket, ControlRequest::Status).persisted_events_by_kind
                ["open"]
                == 1
        },
        "相同序号的另一来源仍应保存",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.collector_schema_version, Some(1));
    assert_eq!(status.collector_message_version, Some(9));
    assert_eq!(status.duplicate_events, 0);
    let events: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryEvents {
            filter: EventFilter::default(),
        },
    );
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event["event"]["source_run_id"] == "synthetic-run-01")
    );
    let mut dedicated_read = activity.clone();
    if let CollectorFrame::Line { source_stream, .. } = &mut dedicated_read {
        *source_stream = SourceStream::Read;
    }
    source.send(dedicated_read);
    wait_until(
        || {
            query::<RuntimeStatus>(&control_socket, ControlRequest::Status)
                .collector_streams
                .get(&SourceStream::Read)
                .is_some_and(|health| health.lines == 1)
        },
        "独立读取来源应动态加入，不能与旧活动流的相同序号冲突",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.collector_streams.len(), 3);
    assert_eq!(status.collector_schema_version, Some(1));
    assert_eq!(status.duplicate_events, 0);
    assert_eq!(status.persisted_events_by_kind["open"], 2);
    let mut dedicated_write = activity.clone();
    if let CollectorFrame::Line {
        source_stream,
        line,
        ..
    } = &mut dedicated_write
    {
        *source_stream = SourceStream::Write;
        let mut value: Value = serde_json::from_str(line).unwrap();
        value["event_type"] = json!(33);
        value["event"] = json!({"write": {"target": value["event"]["open"]["file"]}});
        *line = value.to_string();
    }
    source.send(dedicated_write);
    wait_until(
        || {
            query::<RuntimeStatus>(&control_socket, ControlRequest::Status)
                .persisted_events_by_kind
                .get("write")
                == Some(&1)
        },
        "写入来源同序号应独立传输、解析并保存",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.collector_streams.len(), 4);
    assert_eq!(status.collector_schema_version, Some(1));
    assert_eq!(
        status.collector_streams[&SourceStream::Write].sequence_gaps,
        0
    );
    assert_eq!(status.duplicate_events, 0);
    source.send(activity);
    source.send(exec_frame(701, 2, &file));
    let mut skipped = open_frame(&file, 700, 7, 3);
    if let CollectorFrame::Line { source_stream, .. } = &mut skipped {
        *source_stream = SourceStream::Activity;
    }
    source.send(skipped);
    wait_until(
        || {
            query::<RuntimeStatus>(&control_socket, ControlRequest::Status).collector_streams
                [&SourceStream::Activity]
                .sequence_gaps
                == 2
        },
        "缺口必须只记在对应来源",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(
        status.collector_streams[&SourceStream::Exec].sequence_gaps,
        0
    );
    assert_eq!(status.duplicate_events, 1);
    for sequence in 3..=260 {
        source.send(exec_frame(
            1000 + sequence as u32,
            sequence,
            Path::new("/outside/source.txt"),
        ));
    }
    wait_until(
        || query::<Option<Value>>(&control_socket, receipt_request(1260)).is_some(),
        "最新 exec 回执应可查询",
    );
    assert!(query::<Option<Value>>(&control_socket, receipt_request(700)).is_none());
    source.send(CollectorFrame::Heartbeat {
        run_id: "synthetic-run-02".into(),
        dropped_lines: 0,
    });
    wait_until(
        || {
            query::<RuntimeStatus>(&control_socket, ControlRequest::Status)
                .collector_run_id
                .as_deref()
                == Some("synthetic-run-02")
        },
        "重启必须建立新的来源域",
    );
    assert!(query::<Option<Value>>(&control_socket, receipt_request(1260)).is_none());
    stop_host(&control_socket, host);
    source.stop();
}

#[test]
fn late_activity_association_backfills_the_original_exec_before_saving_its_alert() {
    late_read_association(SourceStream::Activity);
}

#[test]
fn dedicated_read_association_backfills_the_original_exec_before_saving_its_alert() {
    late_read_association(SourceStream::Read);
}

#[test]
fn late_pre_exit_read_backfills_archive_output_and_all_correlated_roots() {
    for (output_inside, sequenced) in [(false, true), (true, true), (true, false), (false, false)] {
        let temp = fixture();
        let input_root = temp.path().join("anonymous-input-project");
        let output_root = temp.path().join("anonymous-output-project");
        fs::create_dir(&input_root).unwrap();
        fs::create_dir(&output_root).unwrap();
        let file = input_root.join("source.txt");
        let output_file = if output_inside {
            output_root.join("synthetic.zip")
        } else {
            temp.path().join("synthetic.zip")
        };
        fs::write(&file, b"anonymous source").unwrap();
        fs::write(&output_file, b"synthetic archive evidence").unwrap();
        let runtime = options(temp.path());
        let control_socket = runtime.control_socket.clone();
        let source = SyntheticSource::start(&runtime.collector_socket);
        let host = start_host(runtime.clone());
        wait_for_socket(&control_socket);
        let _: Vec<Value> = query(
            &control_socket,
            ControlRequest::AddDirectories {
                entries: vec![
                    DirectoryImport {
                        path: input_root.clone(),
                        sources: vec!["manual".into()],
                    },
                    DirectoryImport {
                        path: output_root.clone(),
                        sources: vec!["manual".into()],
                    },
                ],
            },
        );
        source.wait_connected();
        let CollectorFrame::Line { run_id, line, .. } = open_frame(&file, 760, 7, 1) else {
            unreachable!()
        };
        let template: Value = serde_json::from_str(&line).unwrap();
        let make_frame = |kind: u64,
                          stream: SourceStream,
                          sequence: u64,
                          time: i64,
                          received: i64| {
            let mut value = template.clone();
            value["event_type"] = json!(kind);
            value["seq_num"] = json!(if kind == 15 { 1 } else { 0 });
            value["global_seq_num"] = json!(sequence);
            if kind == 13 && !sequenced {
                value.as_object_mut().unwrap().remove("global_seq_num");
                value.as_object_mut().unwrap().remove("seq_num");
            }
            value["time"] = json!(
                DateTime::from_timestamp_millis(BASE_TIME_MS + time)
                    .unwrap()
                    .to_rfc3339()
            );
            if matches!(kind, 13 | 33) {
                let mut target = value["event"]["open"]["file"].clone();
                target["path"] = json!(output_file);
                let metadata = fs::metadata(&output_file).unwrap();
                target["stat"] = json!({"st_mode": metadata.mode(), "st_dev": metadata.dev(), "st_ino": metadata.ino()});
                value["event"] = if kind == 13 {
                    json!({"create": {"destination_type": 0, "destination": {"existing_file": target}}})
                } else {
                    json!({"write": {"target": target}})
                };
            } else if kind == 15 {
                value["event"] = json!({"exit": {"stat": 0}});
            }
            CollectorFrame::Line {
                run_id: run_id.clone(),
                source_stream: stream,
                line: value.to_string(),
                received_timestamp_ms: BASE_TIME_MS + received,
            }
        };
        let create = make_frame(13, SourceStream::Activity, 1, 1_010, 2_000);
        source.send(create.clone());
        if !sequenced {
            // 无序号的两个独立观测都必须保留，各自回填到首次保存的原行。
            source.send(create.clone());
        }
        source.send(make_frame(15, SourceStream::Activity, 2, 1_020, 3_000));
        wait_until(
            || {
                query::<RuntimeStatus>(&control_socket, ControlRequest::Status)
                    .observed_events_by_kind["exit"]
                    == 1
            },
            "先到的输出与退出已处理",
        );
        source.send(make_frame(10, SourceStream::Read, 1, 1_000, 5_000));
        wait_until(
            || {
                let alerts: Vec<Value> = query(
                    &control_socket,
                    ControlRequest::QueryAlerts {
                        filter: AlertFilter::default(),
                    },
                );
                alerts.len() == 1
            },
            "退出前迟到读取应关联已到的归档输出",
        );
        let events: Vec<Value> = query(
            &control_socket,
            ControlRequest::QueryEvents {
                filter: EventFilter::default(),
            },
        );
        let outputs: Vec<_> = events
            .iter()
            .filter(|row| row["event"]["kind"] == "create")
            .collect();
        let expected_outputs = if sequenced { 1 } else { 2 };
        assert_eq!(
            outputs.len(),
            expected_outputs,
            "inside={output_inside}, sequenced={sequenced}"
        );
        let output = outputs[0];
        assert_eq!(output["event"]["source_stream"], "activity");
        assert_eq!(output["event"]["source_timestamp_ms"], BASE_TIME_MS + 1_010);
        assert_eq!(
            output["event"]["received_timestamp_ms"],
            BASE_TIME_MS + 2_000
        );
        let mut roots = vec![input_root.clone()];
        if output_inside {
            roots.push(output_root);
        }
        roots.sort();
        for output in outputs {
            assert_eq!(output["directories"], json!(roots));
        }
        let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
        assert_eq!(
            status.persisted_events_by_kind["create"],
            expected_outputs as u64
        );
        source.send(make_frame(33, SourceStream::Write, 1, 1_015, 6_000));
        wait_until(
            || {
                let alerts: Vec<Value> = query(
                    &control_socket,
                    ControlRequest::QueryAlerts {
                        filter: AlertFilter::default(),
                    },
                );
                alerts.len() == 1 && alerts[0]["activity_count"] == expected_outputs + 1
            },
            "退出前但接收更晚的写入应补充同一告警，不新建通知",
        );
        let writes: Vec<Value> = query(
            &control_socket,
            ControlRequest::QueryEvents {
                filter: EventFilter {
                    kind: Some(codeperimeter::model::EventKind::Write),
                    ..EventFilter::default()
                },
            },
        );
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["directories"], json!(roots));
        if sequenced {
            source.send(create);
            wait_until(
                || {
                    query::<RuntimeStatus>(&control_socket, ControlRequest::Status).duplicate_events
                        == 1
                },
                "回关联不重复保存来源事件",
            );
        }
        let connection = Connection::open(&runtime.database_path).unwrap();
        let counts: (i64, i64, i64) = connection.query_row(
            "SELECT (SELECT COUNT(*) FROM events WHERE kind='create'), (SELECT COUNT(*) FROM alerts), (SELECT COUNT(*) FROM notification_outbox)",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(counts, (expected_outputs as i64, 1, 1));
        let create_count: i64 = connection
            .query_row(
                "SELECT value FROM cumulative_statistics WHERE key='events.create'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(create_count, expected_outputs as i64);
        stop_host(&control_socket, host);
        source.stop();
    }
}

fn late_read_association(read_stream: SourceStream) {
    let temp = fixture();
    let project = temp.path().join("anonymous-project");
    fs::create_dir(&project).unwrap();
    let file = project.join("source.txt");
    fs::write(&file, b"anonymous source").unwrap();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let source = SyntheticSource::start(&runtime.collector_socket);
    let host = start_host(runtime.clone());
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
    source.wait_connected();
    let CollectorFrame::Line { run_id, line, .. } = open_frame(&file, 750, 7, 1) else {
        unreachable!()
    };
    let mut exec: Value = serde_json::from_str(&line).unwrap();
    let mut target = exec["process"].clone();
    target["executable"]["path"] = json!("/usr/bin/gzip");
    exec["event_type"] = json!(9);
    exec["time"] = json!(
        DateTime::from_timestamp_millis(BASE_TIME_MS + 1_010)
            .unwrap()
            .to_rfc3339()
    );
    exec["event"] = json!({"exec": {"target": target, "cwd": {"path": project, "path_truncated": false}, "args": ["gzip", "-c"]}});
    source.send(CollectorFrame::Line {
        source_stream: SourceStream::Exec,
        run_id: run_id.clone(),
        line: exec.to_string(),
        received_timestamp_ms: BASE_TIME_MS + 2_000,
    });
    wait_until(
        || {
            query::<Option<Value>>(
                &control_socket,
                ControlRequest::ExecReceipt {
                    run_id: run_id.clone(),
                    pid: 750,
                    pid_version: 7,
                },
            )
            .is_some()
        },
        "未关联 exec 应已在规则入口处理",
    );
    let events: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryEvents {
            filter: EventFilter::default(),
        },
    );
    assert!(events.is_empty(), "cwd 不能独自证明 stdin 来自项目");
    let mut read: Value = serde_json::from_str(&line).unwrap();
    read["time"] = json!(
        DateTime::from_timestamp_millis(BASE_TIME_MS + 1_000)
            .unwrap()
            .to_rfc3339()
    );
    let activity = CollectorFrame::Line {
        source_stream: read_stream,
        run_id,
        line: read.to_string(),
        received_timestamp_ms: BASE_TIME_MS + 5_000,
    };
    source.send(activity.clone());
    wait_until(
        || {
            query::<RuntimeStatus>(&control_socket, ControlRequest::Status).persisted_events_by_kind
                ["exec"]
                == 1
        },
        "迟到的先行读取应补存已解析 exec",
    );
    let events: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryEvents {
            filter: EventFilter::default(),
        },
    );
    assert_eq!(events.len(), 2);
    let command = events
        .iter()
        .find(|row| row["event"]["kind"] == "exec")
        .unwrap();
    assert_eq!(
        command["event"]["source_timestamp_ms"],
        BASE_TIME_MS + 1_010
    );
    assert_eq!(
        command["event"]["received_timestamp_ms"],
        BASE_TIME_MS + 2_000
    );
    assert_eq!(command["event"]["source_stream"], "exec");
    assert_eq!(command["event"]["global_seq"], 1);
    assert_eq!(command["directories"], json!([project]));
    let alerts: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryAlerts {
            filter: AlertFilter::default(),
        },
    );
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["rule"], "archive_command");
    assert_eq!(alerts[0]["last_timestamp_ms"], BASE_TIME_MS + 1_010);
    assert_eq!(alerts[0]["first_timestamp_ms"], BASE_TIME_MS + 1_000);
    source.send(activity);
    wait_until(
        || query::<RuntimeStatus>(&control_socket, ControlRequest::Status).duplicate_events == 1,
        "来源重复记录仍走既有去重",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.observed_events_by_kind["exec"], 1);
    assert_eq!(status.persisted_events_by_kind["exec"], 1);
    let connection = Connection::open(&runtime.database_path).unwrap();
    let (event_count, outbox_count): (i64, i64) = connection.query_row(
        "SELECT (SELECT COUNT(*) FROM events WHERE kind='exec'), (SELECT COUNT(*) FROM notification_outbox)",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!((event_count, outbox_count), (1, 1));
    stop_host(&control_socket, host);
    source.stop();
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
        source_stream: SourceStream::Combined,
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
        ..Default::default()
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
    assert_eq!(
        sender.messages[0].0,
        codeperimeter::i18n::message(
            &codeperimeter::i18n::current_locale(),
            "notification.summary.title",
            &[]
        )
    );
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
    assert_eq!(
        sender.messages[1].0,
        codeperimeter::i18n::message(
            &codeperimeter::i18n::current_locale(),
            "notification.bulk.title",
            &[]
        )
    );
    assert_eq!(
        sender.targets[0],
        codeperimeter::runtime::NotificationTarget::Alerts
    );
    assert!(
        matches!(&sender.targets[1], codeperimeter::runtime::NotificationTarget::Alert { id } if codeperimeter::model::valid_alert_id(id))
    );

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

    let health_before = collector_eof_health(&control_socket)
        .iter()
        .map(|row| row["id"].as_i64().unwrap())
        .max()
        .unwrap_or(0);
    source.disconnect();
    wait_until(
        || {
            collector_eof_health(&control_socket).iter().any(|row| {
                row["id"].as_i64().unwrap() > health_before
                    && row["record"]["state"] == "reconnecting"
            })
        },
        "采集断线应留下新增的重连健康证据，查询不依赖瞬时状态",
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
        ..Default::default()
    };
    assert!(
        notify_once_with_sender(&control_socket, "helper-db-down", &mut sender).unwrap(),
        "SQLite降级时新告警仍应走有界内存通知"
    );
    assert_eq!(sender.messages.len(), 1);
    assert_eq!(
        sender.messages[0].0,
        codeperimeter::i18n::message(
            &codeperimeter::i18n::current_locale(),
            "notification.bulk.title",
            &[]
        )
    );

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

#[test]
fn disconnected_and_slow_control_clients_do_not_stop_host() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    for request in [b"{\"operation\":\"status\"}\n".as_slice(), b"invalid\n"] {
        let mut stream = UnixStream::connect(&control_socket).unwrap();
        stream.write_all(request).unwrap();
        stream.shutdown(Shutdown::Both).unwrap();
        drop(stream);
        thread::sleep(Duration::from_millis(40));
        assert!(!host.is_finished(), "关闭响应端只影响单个连接");
        let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
        assert_eq!(status.state, "running");
    }
    let mut slow = UnixStream::connect(&control_socket).unwrap();
    slow.write_all(b"{\"operation\":").unwrap();
    thread::sleep(Duration::from_millis(1200));
    assert!(!host.is_finished(), "请求读超时只影响单个连接");
    drop(slow);
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.state, "running");
    stop_host(&control_socket, host);
}

#[test]
fn host_prunes_at_start_and_periodically_preserves_statistics_and_reports_failure() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let database = runtime.database_path.clone();
    let mut storage = codeperimeter::storage::Storage::open(&database).unwrap();
    storage
        .record_health(&codeperimeter::storage::HealthRecord {
            observed_timestamp_ms: 1,
            component: "anonymous".into(),
            code: "expired".into(),
            state: "observed".into(),
            detail: None,
            source: None,
        })
        .unwrap();
    let before = storage.cumulative_stats().unwrap().health_records;
    drop(storage);
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    let health: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryHealth {
            filter: HealthFilter {
                component: Some("anonymous".into()),
                ..HealthFilter::default()
            },
        },
    );
    assert!(health.is_empty(), "启动时应清理旧明细");
    let stats: Value = query(
        &control_socket,
        ControlRequest::Stats {
            since_ms: Some(0),
            until_ms: None,
        },
    );
    assert!(stats["cumulative"]["health_records"].as_u64().unwrap() >= before);
    let external = Connection::open(&database).unwrap();
    external.execute_batch("INSERT INTO health_records(observed_timestamp_ms,component,code,state) VALUES(1,'anonymous','periodic','observed');
        CREATE TRIGGER reject_prune BEFORE DELETE ON health_records BEGIN SELECT RAISE(ABORT,'匿名保留清理故障'); END;").unwrap();
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.retention_state == "failed" && status.database_state == "degraded"
        },
        "定期清理故障应在状态中可见",
    );
    external.execute_batch("DROP TRIGGER reject_prune").unwrap();
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            let health: Vec<Value> = query(
                &control_socket,
                ControlRequest::QueryHealth {
                    filter: HealthFilter {
                        component: Some("anonymous".into()),
                        ..HealthFilter::default()
                    },
                },
            );
            status.retention_state == "ready" && health.is_empty()
        },
        "恢复后应完成定期清理",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert!(status.retention_last_run_ms.is_some());
    stop_host(&control_socket, host);
}

#[test]
fn merged_alert_recovers_outbox_across_restart_and_sent_alert_does_not_repeat() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    for sent_during_failure in [false, true] {
        let temp = fixture();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let files = create_files(&project, "recover");
        let mut runtime = options(temp.path());
        runtime.bulk_file_threshold = 2;
        let control_socket = runtime.control_socket.clone();
        let database = runtime.database_path.clone();
        let collector_socket = runtime.collector_socket.clone();
        let host = start_host(runtime.clone());
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
        let lock = Connection::open(&database).unwrap();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        for (sequence, file) in files[..3].iter().enumerate() {
            source.send(open_frame(file, 880, 14, sequence as u64));
        }
        wait_until(
            || {
                let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
                status.observed_events_by_kind["open"] == 3
                    && status.memory_pending_notifications == 1
            },
            "首次告警与合并均应在写锁期间分析",
        );
        let mut sender = RecordingSender {
            messages: Vec::new(),
            ..Default::default()
        };
        if sent_during_failure {
            assert!(
                notify_once_with_sender(&control_socket, "failure-session", &mut sender).unwrap()
            );
            assert_eq!(sender.messages.len(), 1);
        }
        lock.execute_batch("ROLLBACK").unwrap();
        drop(lock);
        wait_until(
            || {
                let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
                status.database_state == "ready" && status.memory_pending_notifications == 0
            },
            "恢复应将未发送告警交由持久化队列投递",
        );
        let pending: Vec<Value> = query(
            &control_socket,
            ControlRequest::PendingNotifications { limit: 10 },
        );
        assert_eq!(pending.len(), usize::from(!sent_during_failure));
        let alerts: Vec<Value> = query(
            &control_socket,
            ControlRequest::QueryAlerts {
                filter: AlertFilter::default(),
            },
        );
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0]["unique_files"], 3);
        source.stop();
        stop_host(&control_socket, host);
        let host = start_host(runtime);
        wait_for_socket(&control_socket);
        let did_send =
            notify_once_with_sender(&control_socket, "after-restart", &mut sender).unwrap();
        assert_eq!(did_send, !sent_during_failure);
        assert_eq!(sender.messages.len(), 1);
        assert!(!notify_once_with_sender(&control_socket, "after-restart", &mut sender).unwrap());
        stop_host(&control_socket, host);
    }
}

#[test]
fn archive_stdin_gap_keeps_the_parsed_identity_without_saving_unrelated_execs() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let project = temp.path().join("synthetic-project");
    fs::create_dir(&project).unwrap();
    let fence = project.join("fence.bin");
    fs::write(&fence, b"synthetic fence").unwrap();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let source = SyntheticSource::start(&runtime.collector_socket);
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
    source.wait_connected();
    for sequence in [1, 2] {
        let CollectorFrame::Line {
            source_stream,
            run_id,
            line,
            received_timestamp_ms,
        } = open_frame(&fence, 990, 20, sequence)
        else {
            unreachable!();
        };
        let mut value: Value = serde_json::from_str(&line).unwrap();
        let mut target = value["process"].clone();
        target["executable"]["path"] = json!("/usr/bin/gzip");
        if sequence == 2 {
            target["audit_token"].as_object_mut().unwrap().remove("pid");
        }
        value["event_type"] = json!(9);
        value["event"] = json!({"exec": {
            "target": target,
            "cwd": {"path": project, "path_truncated": false},
            "args": ["gzip", "-c"]
        }});
        source.send(CollectorFrame::Line {
            source_stream,
            run_id,
            line: value.to_string(),
            received_timestamp_ms,
        });
    }
    source.send(open_frame(&fence, 991, 3, 3));
    wait_until(
        || {
            let events: Vec<Value> = query(
                &control_socket,
                ControlRequest::QueryEvents {
                    filter: EventFilter::default(),
                },
            );
            events.len() == 1
        },
        "独立来源屏障应在缺口之后保存",
    );
    let health: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryHealth {
            filter: HealthFilter::default(),
        },
    );
    let gap = health
        .iter()
        .find(|row| row["record"]["code"] == "archive_input_source_unknown")
        .unwrap();
    assert_eq!(gap["record"]["source"]["run_id"], "synthetic-run-01");
    assert_eq!(gap["record"]["source"]["pid"], 990);
    assert_eq!(gap["record"]["source"]["pid_version"], 20);
    assert_eq!(gap["record"]["source"]["global_seq"], 1);
    let malformed = health
        .iter()
        .find(|row| row["record"]["source"]["field"] == "process.audit_token.pid")
        .unwrap();
    assert!(malformed["record"]["source"].get("pid").is_none());
    assert!(malformed["record"]["source"].get("pid_version").is_none());
    assert!(malformed["record"]["source"].get("global_seq").is_none());
    let alerts: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryAlerts {
            filter: AlertFilter::default(),
        },
    );
    assert!(alerts.is_empty());
    source.stop();
    stop_host(&control_socket, host);
}

#[test]
fn source_versions_and_structured_gaps_are_queryable_by_run_after_restart() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let files = create_files(&project, "versions");
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let collector_socket = runtime.collector_socket.clone();
    let host = start_host(runtime.clone());
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
    source.send(open_frame(&files[0], 890, 15, 1));
    if let CollectorFrame::Line {
        source_stream,
        run_id,
        line,
        received_timestamp_ms,
    } = open_frame(&files[1], 890, 15, 4)
    {
        let mut value: Value = serde_json::from_str(&line).unwrap();
        value["version"] = json!(10);
        value["event"]["open"]
            .as_object_mut()
            .unwrap()
            .remove("fflag");
        source.send(CollectorFrame::Line {
            source_stream,
            run_id,
            line: value.to_string(),
            received_timestamp_ms,
        });
    }
    for sequence in [5, 6] {
        if let CollectorFrame::Line {
            source_stream,
            run_id,
            line,
            received_timestamp_ms,
        } = open_frame(&files[2], 890, 15, sequence)
        {
            let mut value: Value = serde_json::from_str(&line).unwrap();
            value["schema_version"] = json!(2);
            value["version"] = json!(11);
            source.send(CollectorFrame::Line {
                source_stream,
                run_id,
                line: value.to_string(),
                received_timestamp_ms,
            });
        }
    }
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            let unsupported: Vec<Value> = query(
                &control_socket,
                ControlRequest::QueryHealth {
                    filter: HealthFilter {
                        code: Some("unsupported_schema".into()),
                        ..HealthFilter::default()
                    },
                },
            );
            status.observed_events_by_kind["open"] == 2
                && status.collector_schema_version == Some(2)
                && unsupported.len() == 2
        },
        "支持和不支持的版本均应记录实际来源，重复版本不新增版本记录",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.collector_schema_version, Some(2));
    assert_eq!(status.collector_message_version, Some(11));
    source.stop();
    stop_host(&control_socket, host);
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    let health: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryHealth {
            filter: HealthFilter {
                source_run_id: Some("synthetic-run-01".into()),
                limit: 100,
                ..HealthFilter::default()
            },
        },
    );
    let versions: Vec<_> = health
        .iter()
        .filter(|row| row["record"]["code"] == "source_version_observed")
        .map(|row| row["record"]["source"]["message_version"].as_u64().unwrap())
        .collect();
    assert_eq!(versions, vec![11, 10, 9]);
    assert!(
        health
            .iter()
            .any(|row| row["record"]["source"]["field"] == "event.open.fflag")
    );
    let gaps: Vec<_> = health
        .iter()
        .filter(|row| row["record"]["code"] == "sequence_gap")
        .collect();
    assert_eq!(gaps.len(), 2);
    assert!(
        gaps.iter()
            .all(|row| row["record"]["source"]["missing_events"] == 2)
    );
    let events: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryEvents {
            filter: EventFilter::default(),
        },
    );
    assert_eq!(events[0]["event"]["source_schema_version"], 1);
    assert_eq!(events[0]["event"]["source_message_version"], 10);
    stop_host(&control_socket, host);
}

#[test]
fn collector_burst_preserves_data_order_and_terminal_diagnostic() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let file = project.join("anonymous.txt");
    fs::write(&file, b"anonymous").unwrap();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let source = SyntheticSource::start(&runtime.collector_socket);
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    source.wait_connected();
    let _: Value = query(
        &control_socket,
        ControlRequest::AddDirectories {
            entries: vec![DirectoryImport {
                path: project,
                sources: vec!["manual".into()],
            }],
        },
    );
    for sequence in 1..=512 {
        source.send(open_frame(&file, 900, 1, sequence));
    }
    let timing = CollectorTiming {
        sampled_timestamp_ms: now_ms(),
        source_lines: 512,
        source_bytes: 4096,
        source_send_total_us: 100,
        source_send_max_us: 50,
        bridge_line_write_attempts: 512,
        bridge_write_total_us: 200,
        bridge_write_max_us: 60,
    };
    source.send(CollectorFrame::Metrics {
        run_id: "synthetic-run-01".into(),
        timing: timing.clone(),
    });
    // 旧实例的聚合报告不替换当前实例，且不建立新的事件来源。
    source.send(CollectorFrame::Metrics {
        run_id: "anonymous-stale-run".into(),
        timing: CollectorTiming::default(),
    });
    source.send(CollectorFrame::Status {
        run_id: "synthetic-run-01".into(),
        state: "permission_denied".into(),
        message: "匿名生命周期诊断".into(),
        dropped_lines: 0,
    });
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.persisted_events_by_kind["open"] == 512
                && status.collector_state == "permission_denied"
        },
        "超过两级队列容量的突发及最后诊断均不得静默丢失",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.reader_dropped_frames, 0);
    assert_eq!(status.pipeline_timing.collector, Some(timing));
    assert!(status.pipeline_timing.host_frames >= 515);
    assert!(
        status.pipeline_timing.host_processing_total_us
            >= status.pipeline_timing.host_processing_max_us
    );
    assert!(
        status
            .pipeline_timing
            .source_to_collector_receive_max_ms
            .is_some()
    );
    let rows: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryEvents {
            filter: EventFilter {
                limit: 100,
                ..EventFilter::default()
            },
        },
    );
    let sequences: Vec<u64> = rows
        .iter()
        .map(|row| row["event"]["global_seq"].as_u64().unwrap())
        .collect();
    assert_eq!(sequences, (413..=512).rev().collect::<Vec<_>>());
    let health: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryHealth {
            filter: HealthFilter {
                component: Some("eslogger".into()),
                ..HealthFilter::default()
            },
        },
    );
    assert!(
        !health
            .iter()
            .any(|row| row["record"]["code"] == "global_sequence_gap")
    );
    source.stop();
    stop_host(&control_socket, host);
}

#[test]
fn stalled_source_has_explicit_unknown_coverage_health_and_recovers() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let source = SyntheticSource::start(&runtime.collector_socket);
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    source.wait_connected();
    source.send(CollectorFrame::Heartbeat {
        run_id: "anonymous-idle-run".into(),
        dropped_lines: 0,
    });
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.collector_state == "stalled"
        },
        "持续无心跳时必须报告来源停滞",
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
            .any(|row| row["record"]["code"] == "collector_frame_timeout"
                && row["record"]["state"] == "gap")
    );
    source.send(CollectorFrame::Heartbeat {
        run_id: "anonymous-idle-run".into(),
        dropped_lines: 0,
    });
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.collector_state == "connected"
        },
        "新心跳恢复连接状态但不删除此前覆盖缺口",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.reader_dropped_frames, 0);
    source.stop();
    stop_host(&control_socket, host);
}

#[test]
fn reconnect_reason_changes_remain_visible_and_trusted_socket_recovers() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let runtime = options(temp.path());
    let collector_socket = runtime.collector_socket.clone();
    let control_socket = runtime.control_socket.clone();
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    let health = || -> Vec<Value> {
        query(
            &control_socket,
            ControlRequest::QueryHealth {
                filter: HealthFilter {
                    component: Some("collector".into()),
                    ..HealthFilter::default()
                },
            },
        )
    };
    wait_until(
        || {
            health()
                .iter()
                .any(|row| row["record"]["code"] == "collector_unavailable")
        },
        "缺少端点时应记录来源不可用",
    );

    let listener = UnixListener::bind(&collector_socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    fs::set_permissions(&collector_socket, fs::Permissions::from_mode(0o666)).unwrap();
    wait_until(
        || {
            health()
                .iter()
                .any(|row| row["record"]["code"] == "collector_identity_rejected")
        },
        "重连状态相同但可信路径失败原因变化时仍须记录",
    );
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_eq!(status.collector_state, "reconnecting");
    assert!(status.collector_run_id.is_none());

    fs::set_permissions(&collector_socket, fs::Permissions::from_mode(0o600)).unwrap();
    let mut stream = None;
    wait_until(
        || {
            if let Ok((accepted, _)) = listener.accept() {
                stream = Some(accepted);
                true
            } else {
                false
            }
        },
        "路径信任恢复后应重新连接",
    );
    let mut stream = stream.unwrap();
    write_frame(
        &mut stream,
        &CollectorFrame::Heartbeat {
            run_id: "anonymous-recovered-run".into(),
            dropped_lines: 0,
        },
    )
    .unwrap();
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.collector_state == "connected"
                && status.collector_run_id.as_deref() == Some("anonymous-recovered-run")
        },
        "可信端点恢复后应接收心跳",
    );
    let health = health();
    for code in [
        "collector_unavailable",
        "collector_identity_rejected",
        "collector_connected",
    ] {
        assert!(health.iter().any(|row| row["record"]["code"] == code));
    }
    // 对端保持打开且不再发帧，Stop仍须解除读取等待并回收reader。
    let before = Instant::now();
    stop_host(&control_socket, host);
    assert!(before.elapsed() < Duration::from_secs(2));
    drop(stream);
}

#[test]
fn fast_reconnect_preserves_disconnect_evidence_after_current_state_has_recovered() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = fixture();
    let runtime = options(temp.path());
    let control_socket = runtime.control_socket.clone();
    let source = SyntheticSource::start(&runtime.collector_socket);
    let host = start_host(runtime);
    wait_for_socket(&control_socket);
    source.wait_connected();
    source.send(CollectorFrame::Heartbeat {
        run_id: "anonymous-fast-run-01".into(),
        dropped_lines: 0,
    });
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.collector_run_id.as_deref() == Some("anonymous-fast-run-01")
        },
        "第一代匿名心跳应完成处理",
    );
    let before = collector_eof_health(&control_socket)
        .iter()
        .map(|row| row["id"].as_i64().unwrap())
        .max()
        .unwrap_or(0);

    source.disconnect();
    source.wait_connected();
    source.send(CollectorFrame::Heartbeat {
        run_id: "anonymous-fast-run-02".into(),
        dropped_lines: 0,
    });
    wait_until(
        || {
            let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
            status.collector_state == "connected"
                && status.collector_run_id.as_deref() == Some("anonymous-fast-run-02")
        },
        "查询断线证据前，先确定第二代来源已经恢复",
    );
    let status: RuntimeStatus = query(&control_socket, ControlRequest::Status);
    assert_ne!(
        status.collector_state, "reconnecting",
        "快速恢复后，旧瞬时状态条件已经错过断线"
    );
    let health = collector_eof_health(&control_socket);
    assert!(health.iter().any(
        |row| row["id"].as_i64().unwrap() > before && row["record"]["state"] == "reconnecting"
    ));
    let health: Vec<Value> = query(
        &control_socket,
        ControlRequest::QueryHealth {
            filter: HealthFilter {
                code: Some("collector_restarted".into()),
                ..HealthFilter::default()
            },
        },
    );
    assert!(!health.is_empty());
    source.stop();
    stop_host(&control_socket, host);
}

#[test]
fn console_ipc_rules_scope_workflow_and_pause_survive_restart() {
    use codeperimeter::console::{AlertEntry, ConsoleRequest, Page, RulesSettings};
    use codeperimeter::storage::StoredEvent;
    let temp = fixture();
    let project = temp.path().join("project");
    let excluded = project.join("excluded");
    fs::create_dir_all(&excluded).unwrap();
    fs::write(excluded.join("private.rs"), "合成源码").unwrap();
    for name in ["first.rs", "second.rs", "third.rs", "paused.rs"] {
        fs::write(project.join(name), "合成源码").unwrap();
    }
    let runtime = options(temp.path());
    let socket = runtime.control_socket.clone();
    let host = start_host(runtime.clone());
    wait_for_socket(&socket);
    let _: Vec<Value> = query(
        &socket,
        ControlRequest::AddDirectories {
            entries: vec![
                DirectoryImport {
                    path: project.clone(),
                    sources: vec!["manual".into()],
                },
                DirectoryImport {
                    path: excluded.clone(),
                    sources: vec!["codex".into()],
                },
            ],
        },
    );
    let console = |request| ControlRequest::Console { request };
    let _: Value = query(
        &socket,
        console(ConsoleRequest::DirectorySet {
            path: excluded.clone(),
            enabled: false,
        }),
    );
    let mut settings: RulesSettings = query(&socket, console(ConsoleRequest::RulesGet));
    settings.bulk_file_threshold = 2;
    let saved: RulesSettings = query(&socket, console(ConsoleRequest::RulesSet { settings }));
    assert_eq!(saved.version, 2);
    let source = SyntheticSource::start(&runtime.collector_socket);
    source.wait_connected();
    source.send(open_frame(&excluded.join("private.rs"), 801, 7, 1));
    source.send(open_frame(&project.join("first.rs"), 801, 7, 2));
    source.send(open_frame(&project.join("second.rs"), 801, 7, 3));
    let request_alerts = || {
        console(ConsoleRequest::AlertsPage {
            filter: AlertFilter::default(),
            cursor: None,
            search: None,
            is_read: None,
            processed: None,
        })
    };
    wait_until(
        || query::<Page<AlertEntry>>(&socket, request_alerts()).total == 1,
        "批量规则应通过真实IPC生成告警",
    );
    let page: Page<StoredEvent> = query(
        &socket,
        console(ConsoleRequest::EventsPage {
            filter: EventFilter::default(),
            cursor: None,
            search: None,
            archive_only: false,
        }),
    );
    assert_eq!(page.total, 2);
    assert!(page.items.iter().all(|entry| {
        !entry
            .event
            .file
            .as_ref()
            .unwrap()
            .path
            .starts_with(&excluded)
    }));
    let initial: Page<AlertEntry> = query(&socket, request_alerts());
    let initial = &initial.items[0];
    assert_eq!(initial.rule_version, saved.version);
    let handled: AlertEntry = query(
        &socket,
        console(ConsoleRequest::AlertUpdate {
            id: initial.alert.id.clone(),
            is_read: Some(true),
            processed: Some(true),
            note: Some("合成IPC处理".into()),
            expected_revision: Some(initial.revision),
        }),
    );
    assert!(handled.processed);
    source.send(open_frame(&project.join("third.rs"), 801, 7, 4));
    wait_until(
        || !query::<Page<AlertEntry>>(&socket, request_alerts()).items[0].processed,
        "新增证据应重新打开人工处理状态",
    );
    let _: Value = query(
        &socket,
        console(ConsoleRequest::MonitoringSet { paused: true }),
    );
    source.send(open_frame(&project.join("paused.rs"), 801, 7, 5));
    thread::sleep(Duration::from_millis(150));
    let paused: RuntimeStatus = query(&socket, ControlRequest::Status);
    assert!(paused.monitoring_paused);
    let events: Page<StoredEvent> = query(
        &socket,
        console(ConsoleRequest::EventsPage {
            filter: EventFilter::default(),
            cursor: None,
            search: None,
            archive_only: false,
        }),
    );
    assert_eq!(events.total, 3);
    stop_host(&socket, host);
    source.stop();
    let restarted = start_host(runtime);
    wait_for_socket(&socket);
    let status: RuntimeStatus = query(&socket, ControlRequest::Status);
    assert!(status.monitoring_paused);
    assert_eq!(
        query::<RulesSettings>(&socket, console(ConsoleRequest::RulesGet)),
        saved
    );
    let entries: Page<AlertEntry> = query(&socket, request_alerts());
    assert_eq!(entries.items[0].note, "合成IPC处理");
    let _: Value = query(
        &socket,
        console(ConsoleRequest::MonitoringSet { paused: false }),
    );
    assert!(!query::<RuntimeStatus>(&socket, ControlRequest::Status).monitoring_paused);
    let preview: Value = query(
        &socket,
        console(ConsoleRequest::RetentionPreview { days: 1 }),
    );
    let stale = request_control(
        &socket,
        console(ConsoleRequest::RetentionSet {
            days: 1,
            confirm: true,
            preview_revision: Some(0),
        }),
    )
    .unwrap();
    assert!(!stale.ok);
    let _: Value = query(
        &socket,
        console(ConsoleRequest::RetentionSet {
            days: 1,
            confirm: true,
            preview_revision: preview["preview_revision"].as_u64(),
        }),
    );
    let clear = request_control(
        &socket,
        console(ConsoleRequest::ClearDetails { confirm: false }),
    )
    .unwrap();
    assert!(!clear.ok);
    let _: Value = query(
        &socket,
        console(ConsoleRequest::ClearDetails { confirm: true }),
    );
    let entries: Value = query(&socket, console(ConsoleRequest::Directories));
    assert_eq!(entries.as_array().unwrap().len(), 2);
    assert_eq!(
        query::<RulesSettings>(&socket, console(ConsoleRequest::RulesGet)),
        saved
    );
    let events: Page<StoredEvent> = query(
        &socket,
        console(ConsoleRequest::EventsPage {
            filter: EventFilter::default(),
            cursor: None,
            search: None,
            archive_only: false,
        }),
    );
    assert_eq!(events.total, 0);
    stop_host(&socket, restarted);
}

const HOST_BENCHMARK_SPECS: [(&str, u64, SourceStream, usize, usize); 9] = [
    ("open", 10, SourceStream::Read, 450, 20),
    ("mmap", 20, SourceStream::Read, 450, 0),
    ("exec", 9, SourceStream::Exec, 100, 0),
    ("fork", 11, SourceStream::Activity, 200, 0),
    ("exit", 15, SourceStream::Activity, 200, 0),
    ("create", 13, SourceStream::Activity, 200, 0),
    ("rename", 25, SourceStream::Activity, 200, 0),
    ("close", 12, SourceStream::Activity, 200, 0),
    ("write", 33, SourceStream::Write, 8_000, 20),
];

struct HostBenchmarkGuard(
    PathBuf,
    Option<SyntheticSource>,
    Option<JoinHandle<codeperimeter::Result<()>>>,
);

impl Drop for HostBenchmarkGuard {
    fn drop(&mut self) {
        self.1.take().unwrap().stop();
        let _ = request_control(&self.0, ControlRequest::Stop);
        let _ = self.2.take().unwrap().join();
    }
}

fn benchmark_count(counts: &std::collections::BTreeMap<String, u64>, kind: &str) -> u64 {
    counts.get(kind).copied().unwrap_or(0)
}

fn host_benchmark_frames(
    project_file: &Path,
    received_ms: i64,
    missing_root: Option<&Path>,
) -> Vec<CollectorFrame> {
    let templates: Vec<Value> = include_str!("fixtures/eslogger/events.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let event_time = DateTime::from_timestamp_millis(received_ms)
        .unwrap()
        .to_rfc3339();
    let mut stream_seq = [0_u64; 4];
    let mut frames = Vec::with_capacity(10_000);
    for &(_, event_type, stream, count, monitored) in &HOST_BENCHMARK_SPECS {
        let template = templates
            .iter()
            .find(|v| v["event_type"].as_u64() == Some(event_type))
            .unwrap();
        let lane = ["exec", "read", "write", "activity"]
            .iter()
            .position(|name| *name == stream.as_str())
            .unwrap();
        for sequence in 0..count {
            let mut event = template.clone();
            event["version"] = json!(9);
            event["time"] = json!(event_time);
            event["seq_num"] = json!(sequence);
            event["global_seq_num"] = json!(stream_seq[lane]);
            stream_seq[lane] += 1;
            if sequence < monitored {
                match event_type {
                    10 => event["event"]["open"]["file"]["path"] = json!(project_file),
                    33 => event["event"]["write"]["target"]["path"] = json!(project_file),
                    _ => unreachable!(),
                }
            } else if let Some(root) = missing_root {
                replace_benchmark_paths(&mut event["event"], root);
            }
            event["padding"] = json!("");
            event["padding"] = json!("x".repeat(2_048 - event.to_string().len()));
            let line = event.to_string();
            assert_eq!(line.len(), 2_048);
            frames.push(CollectorFrame::Line {
                run_id: "synthetic-host-benchmark".into(),
                source_stream: stream,
                line,
                received_timestamp_ms: received_ms,
            });
        }
    }
    frames
}

fn replace_benchmark_paths(value: &mut Value, missing_root: &Path) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key == "path"
                    && let Some(path) = value.as_str()
                    && let Ok(suffix) =
                        Path::new(path).strip_prefix("/private/tmp/synthetic-project")
                {
                    *value = json!(missing_root.join(suffix));
                } else {
                    replace_benchmark_paths(value, missing_root);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                replace_benchmark_paths(value, missing_root);
            }
        }
        _ => {}
    }
}

fn run_host_throughput_sample(mode: &str, poll_interval: Duration, deep_missing: bool) {
    let temp = fixture();
    let project = temp.path().join("selected-project");
    fs::create_dir(&project).unwrap();
    let project_file = project.join("tracked-synthetic-file");
    fs::write(&project_file, b"synthetic benchmark bytes").unwrap();
    let runtime = options(temp.path());
    let socket = runtime.control_socket.clone();
    let source = SyntheticSource::start(&runtime.collector_socket);
    let host = start_host(runtime);
    let guard = HostBenchmarkGuard(socket.clone(), Some(source), Some(host));
    wait_for_socket(&socket);
    let source = guard.1.as_ref().unwrap();
    source.wait_connected();
    let _: Value = query(
        &socket,
        ControlRequest::AddDirectories {
            entries: vec![DirectoryImport {
                path: project,
                sources: vec!["manual".into()],
            }],
        },
    );
    let received_ms = now_ms();
    let missing_root = deep_missing.then(|| {
        let suffix: PathBuf = (0..24).map(|_| "missing").collect();
        temp.path().join("unselected-project").join(suffix)
    });
    let frames = host_benchmark_frames(&project_file, received_ms, missing_root.as_deref());
    let started = Instant::now();
    for frame in frames {
        source.send(frame);
    }
    source.send(CollectorFrame::Status {
        run_id: "synthetic-host-benchmark".into(),
        state: "permission_denied".into(),
        message: "synthetic FIFO drain fence".into(),
        dropped_lines: 0,
    });
    let mut queries = 0;
    let status = loop {
        let status: RuntimeStatus = query(&socket, ControlRequest::Status);
        queries += 1;
        if status.collector_state == "permission_denied" {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "合成帧未在超时前排空"
        );
        thread::sleep(poll_interval);
    };
    let elapsed = started.elapsed();
    assert_eq!(status.pipeline_timing.host_frames, 10_001);
    assert_eq!(status.collector_streams.len(), 4);
    assert_eq!(status.collector_schema_version, Some(1));
    assert_eq!(status.collector_message_version, Some(9));
    assert!(
        status
            .collector_streams
            .values()
            .all(|health| health.sequence_gaps == 0)
    );
    for (kind, _, _, count, tracked) in HOST_BENCHMARK_SPECS {
        let actual = (
            benchmark_count(&status.observed_events_by_kind, kind),
            benchmark_count(&status.persisted_events_by_kind, kind),
            benchmark_count(&status.filtered_events_by_kind, kind),
        );
        assert_eq!(
            actual,
            (count as u64, tracked as u64, (count - tracked) as u64)
        );
    }
    assert_eq!(
        (
            status.reader_dropped_frames,
            status.collector_dropped_lines,
            status.database_gap_events,
            status.duplicate_events
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(status.database_state, "ready");
    let average_us = status.pipeline_timing.host_processing_total_us as f64
        / status.pipeline_timing.host_frames as f64;
    eprintln!(
        "synthetic Host mode={mode} events=10000 elapsed_ms={} events_per_sec={:.0} status_queries={queries} observed_queries_per_sec={:.1} host_avg_us={average_us:.1} host_max_us={}",
        elapsed.as_millis(),
        10_000.0 / elapsed.as_secs_f64(),
        queries as f64 / elapsed.as_secs_f64(),
        status.pipeline_timing.host_processing_max_us
    );
}

#[test]
fn four_stream_ten_thousand_frame_host_throughput_with_status_polling() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    run_host_throughput_sample("base_4hz_fence", Duration::from_millis(250), false);
    run_host_throughput_sample("status_30hz", Duration::from_millis(33), false);
}

#[test]
fn four_stream_deep_missing_paths_host_throughput() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    run_host_throughput_sample("deep_missing_4hz_fence", Duration::from_millis(250), true);
    run_host_throughput_sample("deep_missing_30hz", Duration::from_millis(33), true);
}
