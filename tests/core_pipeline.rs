// 合成 schema 1／message 9 输入只验证模块契约，不代表真实 ES、3 秒或桌面展示验收。
use chrono::DateTime;
use codeperimeter::eslogger::EsloggerAdapter;
use codeperimeter::model::{AlertRule, EventKind};
use codeperimeter::rules::{RuleConfig, RuleEngine, RuleOutput};
use codeperimeter::storage::{AlertFilter, EventFilter, Storage};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tempfile::{Builder, TempDir};

const SOURCE_EPOCH: i64 = 1_767_225_600_000;
const RECEIVE_EPOCH: i64 = SOURCE_EPOCH + 86_400_000;

struct Pipeline {
    adapter: EsloggerAdapter,
    rules: RuleEngine,
    storage: Storage,
    global_seq: u64,
    event_seq: HashMap<u64, u64>,
}

impl Pipeline {
    fn new(database: &Path, roots: Vec<PathBuf>) -> Self {
        let mut storage = Storage::open(database).unwrap();
        for root in &roots {
            storage
                .add_directory(root, "manual", RECEIVE_EPOCH)
                .unwrap();
        }
        Self {
            adapter: EsloggerAdapter::new("synthetic-pipeline-run"),
            rules: RuleEngine::new(roots, RuleConfig::default()).unwrap(),
            storage,
            global_seq: 0,
            event_seq: HashMap::new(),
        }
    }

    fn feed(&mut self, event_type: u64, payload: Value, offset_ms: i64) -> RuleOutput {
        self.feed_as(
            event_type,
            payload,
            offset_ms,
            process(200, 7, "synthetic-reader"),
        )
    }

    fn feed_as(
        &mut self,
        event_type: u64,
        payload: Value,
        offset_ms: i64,
        process: Value,
    ) -> RuleOutput {
        let event_seq = self.event_seq.entry(event_type).or_default();
        let received_ms = RECEIVE_EPOCH + self.global_seq as i64;
        let native = json!({
            "schema_version": 1, "version": 9, "action_type": 1,
            "event_type": event_type, "seq_num": *event_seq,
            "global_seq_num": self.global_seq,
            "time": DateTime::from_timestamp_millis(SOURCE_EPOCH + offset_ms)
                .unwrap().to_rfc3339(),
            "process": process, "event": payload,
            "synthetic_body": "SYNTHETIC_PRIVATE_RAW_BODY",
        });
        *event_seq += 1;
        self.global_seq += 1;
        let parsed = self.adapter.parse_line(&native.to_string(), received_ms);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert_eq!(parsed.schema_version, Some(1));
        assert_eq!(parsed.message_version, Some(9));
        let event = parsed.event.expect("原生 JSON 必须先经过 adapter");
        let output = self.rules.process(&event);
        assert!(output.health.is_empty(), "{:?}", output.health);
        // 与宿主一致：规则先分析，只有与保护目录有关的标准事件才保存。
        if !output.matched_directories.is_empty() {
            assert!(
                self.storage
                    .record_event(&event, &output.matched_directories)
                    .unwrap()
            );
        }
        for alert in &output.alerts {
            self.storage.record_alert_at(alert, received_ms).unwrap();
        }
        output
    }
}

fn fixture() -> TempDir {
    Builder::new()
        .prefix("codeperimeter-pipeline-")
        .tempdir_in("/private/tmp")
        .unwrap()
}

fn root(temp: &TempDir, name: &str) -> PathBuf {
    let path = temp.path().join(name);
    fs::create_dir(&path).unwrap();
    path.canonicalize().unwrap()
}

fn source(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, b"synthetic file contents").unwrap();
    path
}

fn process(pid: u32, version: u32, executable: &str) -> Value {
    json!({
        "audit_token": {"pid": pid, "pidversion": version}, "ppid": 1,
        "executable": {"path": format!("/usr/bin/{executable}"), "path_truncated": false},
        "signing_id": "example.synthetic", "team_id": null,
    })
}

fn file(path: &Path) -> Value {
    let metadata = fs::metadata(path).unwrap();
    json!({
        "path": path, "path_truncated": false,
        "stat": {"st_mode": metadata.mode(), "st_dev": metadata.dev(), "st_ino": metadata.ino()},
    })
}

fn open(path: &Path, flags: i64) -> Value {
    json!({"open": {"fflag": flags, "file": file(path)}})
}

fn mmap(path: &Path, protection: i64) -> Value {
    json!({"mmap": {"protection": protection, "max_protection": 7, "source": file(path)}})
}

#[test]
fn readable_open_and_mmap_share_file_identity_and_merge_sqlite_outbox_across_roots() {
    let temp = fixture();
    let root_a = root(&temp, "project-a");
    let root_b = root(&temp, "project-b");
    let neighbor = root(&temp, "project-a-neighbor");
    let files: Vec<_> = (0..50)
        .map(|index| {
            source(
                if index % 2 == 0 { &root_a } else { &root_b },
                &format!("file-{index}.rs"),
            )
        })
        .collect();
    let mut pipeline = Pipeline::new(
        &temp.path().join("monitor.sqlite"),
        vec![root_a.clone(), root_b.clone()],
    );

    assert!(
        pipeline
            .feed(10, open(&files[0], 1), 1_000)
            .alerts
            .is_empty()
    );
    // 相同 dev／ino 的可读映射增加访问次数，不增加不同文件数。
    assert!(
        pipeline
            .feed(20, mmap(&files[0], 1), 1_001)
            .alerts
            .is_empty()
    );
    let write_only = source(&root_a, "write-only.rs");
    let non_read_mapping = source(&root_b, "non-read-mapping.rs");
    assert!(
        pipeline
            .feed(10, open(&write_only, 2), 1_002)
            .alerts
            .is_empty()
    );
    assert!(
        pipeline
            .feed(20, mmap(&non_read_mapping, 2), 1_003)
            .alerts
            .is_empty()
    );
    let outside = source(&neighbor, "outside.rs");
    let outside_result = pipeline.feed(10, open(&outside, 1), 1_004);
    assert!(outside_result.alerts.is_empty());
    assert!(outside_result.matched_directories.is_empty());

    for (index, path) in files.iter().enumerate().take(49).skip(1) {
        let output = if index % 2 == 0 {
            pipeline.feed(10, open(path, 1), 1_100 + index as i64 * 50)
        } else {
            pipeline.feed(20, mmap(path, 1), 1_100 + index as i64 * 50)
        };
        assert!(output.alerts.is_empty(), "49 个不同文件前不应告警");
    }
    assert_eq!(
        pipeline
            .storage
            .pending_notification_summary()
            .unwrap()
            .count,
        0
    );
    let triggered = pipeline.feed(10, open(&files[49], 1), 5_000);
    let first = &triggered.alerts[0];
    assert_eq!(first.rule, AlertRule::BulkFileAccess);
    assert!(first.is_new);
    assert_eq!((first.unique_files, first.activity_count), (50, 51));
    assert_eq!(first.roots, vec![root_a.clone(), root_b.clone()]);
    assert_eq!(first.first_timestamp_ms, SOURCE_EPOCH + 1_000);
    assert_eq!(
        pipeline.storage.pending_notifications(10).unwrap()[0].created_timestamp_ms,
        RECEIVE_EPOCH + 53
    );

    // 第二个独立 10 秒窗口仍在上次告警后 60 秒内，只更新原告警与原 outbox。
    for (index, path) in files.iter().enumerate() {
        let output = pipeline.feed(10, open(path, 1), 55_000 + index as i64 * 50);
        if index < 49 {
            assert!(output.alerts.is_empty());
        } else {
            assert_eq!(output.alerts[0].id, first.id);
            assert!(!output.alerts[0].is_new);
        }
    }
    let saved = pipeline
        .storage
        .query_alerts(&AlertFilter::default())
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].last_timestamp_ms, SOURCE_EPOCH + 57_450);
    assert_eq!((saved[0].unique_files, saved[0].activity_count), (50, 51));
    assert_eq!(
        pipeline.storage.pending_notifications(10).unwrap()[0].alert,
        saved[0]
    );
    assert_eq!(
        pipeline
            .storage
            .pending_notification_summary()
            .unwrap()
            .count,
        1
    );

    for (index, path) in files.iter().enumerate() {
        let output = pipeline.feed(10, open(path, 1), 120_000 + index as i64 * 50);
        if index == 49 {
            assert!(output.alerts[0].is_new);
            assert_ne!(output.alerts[0].id, first.id);
        }
    }
    assert_eq!(
        pipeline
            .storage
            .query_alerts(&AlertFilter::default())
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        pipeline
            .storage
            .pending_notification_summary()
            .unwrap()
            .count,
        2
    );
    let queried = pipeline
        .storage
        .query_events(&EventFilter {
            limit: 1_000,
            ..EventFilter::default()
        })
        .unwrap();
    assert_eq!(queried.len(), 153);
    assert!(
        queried
            .iter()
            .all(|row| !row.directories.contains(&neighbor))
    );
    for path in [&write_only, &non_read_mapping] {
        let row = pipeline
            .storage
            .query_events(&EventFilter {
                file_path: Some(path.clone()),
                ..EventFilter::default()
            })
            .unwrap();
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].event.file.as_ref().unwrap().readable, Some(false));
    }
    assert_eq!(
        pipeline
            .storage
            .query_events(&EventFilter {
                directory: Some(root_b),
                kind: Some(EventKind::Mmap),
                ..EventFilter::default()
            })
            .unwrap()
            .len(),
        25
    );
}

#[test]
fn ten_second_window_expires_old_source_events_even_when_receive_times_are_adjacent() {
    let temp = fixture();
    let root = root(&temp, "project");
    let files: Vec<_> = (0..50)
        .map(|index| source(&root, &format!("file-{index}.rs")))
        .collect();
    let mut pipeline = Pipeline::new(&temp.path().join("monitor.sqlite"), vec![root.clone()]);
    for (index, path) in files.iter().take(49).enumerate() {
        assert!(
            pipeline
                .feed(10, open(path, 1), index as i64 * 100)
                .alerts
                .is_empty()
        );
    }
    // 接收只相隔 1 毫秒，但源时间已过 10 秒，旧的 49 个文件不能累计到门槛。
    assert!(
        pipeline
            .feed(20, mmap(&files[49], 1), 15_001)
            .alerts
            .is_empty()
    );
    assert_eq!(
        pipeline
            .storage
            .pending_notification_summary()
            .unwrap()
            .count,
        0
    );
    for (index, path) in files.iter().take(49).enumerate() {
        let output = pipeline.feed(10, open(path, 1), 15_002 + index as i64);
        if index < 48 {
            assert!(output.alerts.is_empty());
        } else {
            assert!(output.alerts[0].is_new);
            assert_eq!(output.alerts[0].first_timestamp_ms, SOURCE_EPOCH + 15_001);
            assert_eq!(output.alerts[0].last_timestamp_ms, SOURCE_EPOCH + 15_050);
            assert_eq!(
                (
                    output.alerts[0].unique_files,
                    output.alerts[0].activity_count
                ),
                (50, 50)
            );
        }
    }
    let alerts = pipeline
        .storage
        .query_alerts(&AlertFilter {
            directory: Some(root),
            rule: Some(AlertRule::BulkFileAccess),
            ..AlertFilter::default()
        })
        .unwrap();
    assert_eq!(alerts.len(), 1);
    let pending = pipeline.storage.pending_notifications(10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].alert, alerts[0]);
    assert_eq!(pending[0].created_timestamp_ms, RECEIVE_EPOCH + 98);
    assert_eq!(pipeline.storage.cumulative_stats().unwrap().events, 99);
}

#[test]
fn native_archive_exec_and_temporary_output_persist_correlated_paths_without_raw_arguments() {
    let temp = fixture();
    let root_a = root(&temp, "project-a");
    let root_b = root(&temp, "project-b");
    let source_a = source(&root_a, "source.rs");
    let source_b = source(&root_b, "data.rs");
    let output_path = source(temp.path(), "snapshot.tar.gz");
    let database = temp.path().join("monitor.sqlite");
    let mut pipeline = Pipeline::new(&database, vec![root_a.clone(), root_b.clone()]);
    let before_exec = process(400, 7, "synthetic-client");
    let tar = process(400, 8, "tar");
    pipeline.feed_as(10, open(&source_a, 1), 0, before_exec.clone());
    let command = pipeline.feed_as(9, json!({"exec": {
        "target": tar.clone(),
        "args": ["SYNTHETIC_PRIVATE_ARGV0", "-czf", "snapshot.tar.gz", "-C", root_a, "source.rs", source_b],
        "env": ["SYNTHETIC_PRIVATE_ENV=hidden"],
        "cwd": {"path": temp.path(), "path_truncated": false},
    }}), 100, before_exec);
    assert_eq!(command.alerts.len(), 1);
    assert_eq!(command.alerts[0].rule, AlertRule::ArchiveCommand);
    assert_eq!(command.alerts[0].process.pid_version, Some(8));
    assert_eq!(command.alerts[0].first_timestamp_ms, SOURCE_EPOCH + 100);
    assert_eq!(
        command.alerts[0].roots,
        vec![root_a.clone(), root_b.clone()]
    );
    assert!(command.alerts[0].evidence_paths.contains(&output_path));
    pipeline.feed_as(10, open(&source_a, 1), 200, tar.clone());
    pipeline.feed_as(20, mmap(&source_b, 1), 300, tar.clone());
    let write = json!({"write": {"target": file(&output_path)}});
    let correlated = pipeline.feed_as(33, write.clone(), 400, tar.clone());
    assert_eq!(correlated.alerts[0].rule, AlertRule::ArchiveOutput);
    assert_eq!(
        correlated.alerts[0].roots,
        vec![root_a.clone(), root_b.clone()]
    );
    assert_eq!(correlated.alerts[0].first_timestamp_ms, SOURCE_EPOCH + 200);
    for path in [&source_a, &source_b, &output_path] {
        assert!(correlated.alerts[0].evidence_paths.contains(path));
    }
    let repeated = pipeline.feed_as(33, write.clone(), 500, tar);
    assert_eq!(repeated.alerts[0].id, correlated.alerts[0].id);
    assert!(!repeated.alerts[0].is_new);
    // 相同输出后缀与路径不允许给未访问项目的另一个进程补造归档关联。
    let unrelated = pipeline.feed_as(33, write, 600, process(500, 9, "synthetic-worker"));
    assert!(unrelated.alerts.is_empty());
    assert!(unrelated.matched_directories.is_empty());

    drop(pipeline);
    let storage = Storage::open(&database).unwrap();
    let events = storage
        .query_events(&EventFilter {
            limit: 100,
            ..EventFilter::default()
        })
        .unwrap();
    assert_eq!(events.len(), 6);
    let exec = events
        .iter()
        .find(|row| row.event.kind == EventKind::Exec)
        .unwrap();
    let archive = exec.event.archive.as_ref().unwrap();
    assert_eq!(archive.tool, "tar");
    assert_eq!(archive.input_paths, vec![source_a, source_b]);
    assert_eq!(archive.output_path, Some(output_path.clone()));
    assert_eq!(archive.cwd, Some(temp.path().to_path_buf()));
    assert_eq!(exec.event.process.pid_version, Some(8));
    let outputs = storage
        .query_events(&EventFilter {
            directory: Some(root_b.clone()),
            file_path: Some(output_path),
            kind: Some(EventKind::Write),
            ..EventFilter::default()
        })
        .unwrap();
    assert_eq!(outputs.len(), 2);
    assert!(
        outputs
            .iter()
            .all(|row| row.directories == vec![root_a.clone(), root_b.clone()])
    );
    let alerts = storage.query_alerts(&AlertFilter::default()).unwrap();
    assert_eq!(alerts.len(), 2);
    assert_eq!(
        alerts
            .iter()
            .find(|alert| alert.rule == AlertRule::ArchiveOutput)
            .unwrap()
            .activity_count,
        2
    );
    let pending = storage.pending_notifications(10).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(storage.pending_notification_summary().unwrap().count, 2);
    let persisted = serde_json::to_string(&(events, alerts, pending)).unwrap();
    for forbidden in [
        "\"args\"",
        "\"env\"",
        "SYNTHETIC_PRIVATE_ARGV0",
        "SYNTHETIC_PRIVATE_ENV",
        "SYNTHETIC_PRIVATE_RAW_BODY",
    ] {
        assert!(
            !persisted.contains(forbidden),
            "持久化 DTO 泄漏 {forbidden}"
        );
    }
    drop(storage);
    let database_bytes = fs::read(database).unwrap();
    for marker in [
        "SYNTHETIC_PRIVATE_ARGV0",
        "SYNTHETIC_PRIVATE_ENV",
        "SYNTHETIC_PRIVATE_RAW_BODY",
    ] {
        assert!(
            !database_bytes
                .windows(marker.len())
                .any(|window| window == marker.as_bytes()),
            "SQLite 泄漏 {marker}"
        );
    }
}
