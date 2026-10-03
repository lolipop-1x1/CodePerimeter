use codeperimeter::model::{
    ActivityEvent, Alert, AlertRule, ArchiveCommand, EventKind, FileEvidence, ProcessIdentity,
};
use codeperimeter::rules::{RuleConfig, RuleEngine};
use codeperimeter::storage::{
    AlertFilter, AlertWrite, CumulativeStats, EventFilter, HealthFilter, HealthRecord,
    NotificationFilter, NotificationOutcome, NotificationRecord, Storage,
};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn fixture() -> TempDir {
    tempfile::tempdir().expect("临时目录创建成功")
}

fn protected_root(temp: &TempDir, name: &str) -> PathBuf {
    let path = temp.path().join(name);
    std::fs::create_dir_all(&path).expect("合成保护目录创建成功");
    std::fs::canonicalize(path).expect("合成保护目录路径规范化成功")
}

fn process(pid: u32, pid_version: Option<u32>) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        pid_version,
        ppid: Some(10),
        executable: Some(PathBuf::from("/usr/bin/synthetic")),
        signing_id: None,
        team_id: None,
    }
}

fn event(
    kind: EventKind,
    path: Option<PathBuf>,
    timestamp_ms: Option<i64>,
    received_timestamp_ms: i64,
    pid: u32,
    pid_version: Option<u32>,
) -> ActivityEvent {
    ActivityEvent {
        source_run_id: "synthetic-run".into(),
        source_timestamp_ms: timestamp_ms,
        received_timestamp_ms,
        global_seq: None,
        event_seq: None,
        kind,
        process: process(pid, pid_version),
        file: path.map(|path| FileEvidence {
            path,
            path_truncated: false,
            device: None,
            inode: None,
            is_regular: Some(true),
            readable: matches!(kind, EventKind::Open | EventKind::Mmap).then_some(true),
        }),
        destination: None,
        modified: None,
        archive: None,
    }
}

fn read_event(
    path: PathBuf,
    timestamp_ms: i64,
    received_timestamp_ms: i64,
    pid: u32,
    pid_version: Option<u32>,
) -> ActivityEvent {
    event(
        EventKind::Open,
        Some(path),
        Some(timestamp_ms),
        received_timestamp_ms,
        pid,
        pid_version,
    )
}

fn rule_config(threshold: usize) -> RuleConfig {
    RuleConfig {
        bulk_file_threshold: threshold,
        ..RuleConfig::default()
    }
}

#[test]
fn only_readable_open_and_mmap_evidence_inside_component_boundaries_counts() {
    let temp = fixture();
    let root_a = protected_root(&temp, "project-a");
    let root_b = protected_root(&temp, "project-b");
    let neighbor = protected_root(&temp, "project-a-copy");
    let mut engine = RuleEngine::new(vec![root_a.clone(), root_b.clone()], rule_config(2))
        .expect("规则初始化成功");

    let mut unknown_read = read_event(root_a.join("unknown.rs"), 1_000, 10_000, 20, Some(1));
    unknown_read.file.as_mut().unwrap().readable = None;
    assert!(engine.process(&unknown_read).alerts.is_empty());

    let mut first = read_event(root_a.join("same.rs"), 1_100, 10_001, 20, Some(1));
    first.file.as_mut().unwrap().device = Some(7);
    first.file.as_mut().unwrap().inode = Some(42);
    let first_output = engine.process(&first);
    assert_eq!(first_output.matched_directories, vec![root_a.clone()]);
    assert!(first_output.alerts.is_empty());

    let mut duplicate = event(
        EventKind::Mmap,
        Some(root_a.join("same.rs")),
        Some(1_200),
        10_002,
        20,
        Some(1),
    );
    duplicate.file.as_mut().unwrap().device = Some(7);
    duplicate.file.as_mut().unwrap().inode = Some(42);
    assert!(engine.process(&duplicate).alerts.is_empty());

    assert!(
        engine
            .process(&read_event(
                neighbor.join("outside.rs"),
                1_300,
                10_003,
                20,
                Some(1)
            ))
            .alerts
            .is_empty()
    );
    let output = engine.process(&read_event(
        root_b.join("second.rs"),
        1_400,
        10_004,
        20,
        Some(1),
    ));
    let alert = output.alerts.first().expect("不同保护目录累计到阈值");
    assert_eq!(alert.rule, AlertRule::BulkFileAccess);
    assert_eq!(alert.unique_files, 2);
    assert_eq!(alert.activity_count, 3);
    assert_eq!(alert.roots, vec![root_a, root_b]);
    assert!(!output.matched_directories.contains(&neighbor));
}

#[test]
fn bulk_window_uses_source_time_and_reports_missing_or_out_of_order_time() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let mut engine = RuleEngine::new(vec![root.clone()], rule_config(50)).unwrap();
    for index in 0..50 {
        let path = root.join(format!("file-{index}.rs"));
        let output = engine.process(&read_event(
            path,
            1_000 + index * 1_000,
            20_000,
            21,
            Some(1),
        ));
        assert!(output.alerts.is_empty());
    }

    let mut missing = read_event(root.join("fallback.rs"), 0, 20_001, 21, Some(1));
    missing.source_timestamp_ms = None;
    let missing_output = engine.process(&missing);
    assert!(
        missing_output
            .health
            .iter()
            .any(|record| record.code == "source_timestamp_missing")
    );

    let mut stale_engine = RuleEngine::new(
        vec![root.clone()],
        RuleConfig {
            bulk_window_ms: 1_000,
            bulk_file_threshold: 2,
            ..RuleConfig::default()
        },
    )
    .unwrap();
    assert!(
        stale_engine
            .process(&read_event(
                root.join("new.rs"),
                10_000,
                30_000,
                22,
                Some(1)
            ))
            .alerts
            .is_empty()
    );
    let stale = stale_engine.process(&read_event(root.join("old.rs"), 8_000, 30_001, 22, Some(1)));
    assert!(stale.alerts.is_empty());
    assert!(
        stale
            .health
            .iter()
            .any(|record| record.code == "source_timestamp_out_of_order")
    );
}

#[test]
fn process_generations_separate_alerts_and_unknown_exec_starts_a_new_segment() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let mut engine = RuleEngine::new(vec![root.clone()], rule_config(2)).unwrap();
    let mut alerts = Vec::new();
    for version in [Some(1), Some(2)] {
        alerts.extend(
            engine
                .process(&read_event(
                    root.join(format!("one-{}.rs", version.unwrap())),
                    1_000,
                    1_000,
                    30,
                    version,
                ))
                .alerts,
        );
        alerts.extend(
            engine
                .process(&read_event(
                    root.join(format!("two-{}.rs", version.unwrap())),
                    1_100,
                    1_100,
                    30,
                    version,
                ))
                .alerts,
        );
    }
    assert_eq!(alerts.len(), 2);
    assert_ne!(alerts[0].id, alerts[1].id);

    let mut unknown_engine = RuleEngine::new(vec![root.clone()], rule_config(2)).unwrap();
    let first = unknown_engine.process(&read_event(root.join("old-a.rs"), 2_000, 2_000, 31, None));
    assert!(first.alerts.is_empty());
    let before_exec =
        unknown_engine.process(&read_event(root.join("old-b.rs"), 2_100, 2_100, 31, None));
    assert!(before_exec.alerts.iter().any(|alert| alert.is_new));

    let exec = event(EventKind::Exec, None, Some(2_200), 2_200, 31, None);
    unknown_engine.process(&exec);
    assert!(
        unknown_engine
            .process(&read_event(root.join("new-a.rs"), 2_300, 2_300, 31, None))
            .alerts
            .is_empty()
    );
    let after_exec =
        unknown_engine.process(&read_event(root.join("new-b.rs"), 2_400, 2_400, 31, None));
    assert!(
        after_exec
            .alerts
            .iter()
            .any(|alert| alert.is_new && alert.process.pid_version.is_none())
    );
    assert!(
        first
            .health
            .iter()
            .any(|record| record.code == "process_generation_unknown")
    );
}

#[test]
fn new_alerts_are_immediate_and_merge_by_source_time_not_receive_burst() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let mut engine = RuleEngine::new(vec![root.clone()], rule_config(2)).unwrap();
    assert!(
        engine
            .process(&read_event(root.join("a.rs"), 1_000, 50_000, 40, Some(5)))
            .alerts
            .is_empty()
    );
    let initial = engine.process(&read_event(root.join("b.rs"), 2_000, 50_001, 40, Some(5)));
    let first_alert = initial.alerts.first().expect("达到阈值即时告警").clone();
    assert!(first_alert.is_new);

    let merged = engine.process(&read_event(root.join("c.rs"), 3_000, 50_002, 40, Some(5)));
    assert_eq!(merged.alerts[0].id, first_alert.id);
    assert!(!merged.alerts[0].is_new);

    engine.process(&read_event(root.join("d.rs"), 63_000, 50_003, 40, Some(5)));
    let later = engine.process(&read_event(root.join("e.rs"), 64_000, 50_004, 40, Some(5)));
    assert!(later.alerts[0].is_new);
    assert_ne!(later.alerts[0].id, first_alert.id);
}

#[test]
fn bounded_process_and_file_state_emit_health_when_evidence_is_evicted() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let process_limited = RuleConfig {
        max_process_states: 1,
        bulk_file_threshold: 2,
        max_files_per_process: 2,
        ..RuleConfig::default()
    };
    let mut engine = RuleEngine::new(vec![root.clone()], process_limited).unwrap();
    engine.process(&read_event(root.join("a.rs"), 1_000, 1_000, 35, Some(1)));
    let evicted = engine.process(&read_event(root.join("b.rs"), 1_100, 1_100, 36, Some(1)));
    assert!(
        evicted
            .health
            .iter()
            .any(|record| record.code == "process_state_capacity")
    );

    let file_limited = RuleConfig {
        bulk_file_threshold: 2,
        max_files_per_process: 2,
        ..RuleConfig::default()
    };
    let mut engine = RuleEngine::new(vec![root.clone()], file_limited).unwrap();
    engine.process(&read_event(root.join("a.rs"), 2_000, 2_000, 37, Some(1)));
    engine.process(&read_event(root.join("b.rs"), 2_100, 2_100, 37, Some(1)));
    let evicted = engine.process(&read_event(root.join("c.rs"), 2_200, 2_200, 37, Some(1)));
    assert!(
        evicted
            .health
            .iter()
            .any(|record| record.code == "file_state_capacity")
    );
}

#[test]
fn archive_candidates_require_project_association_and_output_correlation() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let root_alias = temp.path().join("project");
    let mut engine = RuleEngine::new(vec![root.clone()], rule_config(50)).unwrap();

    let mut unrelated = event(EventKind::Exec, None, Some(1_000), 1_000, 50, Some(1));
    unrelated.archive = Some(ArchiveCommand {
        tool: "/usr/bin/zip".into(),
        input_paths: vec![PathBuf::from("/tmp/unrelated.txt")],
        output_path: Some(PathBuf::from("/tmp/unrelated.zip")),
        cwd: Some(PathBuf::from("/tmp")),
    });
    assert!(engine.process(&unrelated).alerts.is_empty());

    let mut command = event(EventKind::Exec, None, Some(2_000), 2_000, 50, Some(1));
    command.archive = Some(ArchiveCommand {
        tool: "/usr/bin/zip".into(),
        input_paths: vec![PathBuf::from("source.rs")],
        output_path: Some(temp.path().join("synthetic.zip")),
        cwd: Some(root_alias),
    });
    let command_output = engine.process(&command);
    let archive_alert = command_output
        .alerts
        .first()
        .expect("项目输入关联外部归档命令");
    assert_eq!(archive_alert.rule, AlertRule::ArchiveCommand);
    assert_eq!(archive_alert.roots, vec![root.clone()]);
    assert_eq!(command_output.matched_directories, vec![root.clone()]);

    let mut suffix_only_engine = RuleEngine::new(vec![root.clone()], rule_config(50)).unwrap();
    let suffix_only = event(
        EventKind::Create,
        Some(root.join("looks-like.zip")),
        Some(3_000),
        3_000,
        51,
        Some(1),
    );
    assert!(suffix_only_engine.process(&suffix_only).alerts.is_empty());

    let mut output_engine = RuleEngine::new(vec![root.clone()], rule_config(50)).unwrap();
    output_engine.process(&read_event(
        root.join("source.rs"),
        4_000,
        4_000,
        52,
        Some(1),
    ));
    let output = event(
        EventKind::Write,
        Some(PathBuf::from("/tmp/synthetic-output.zip")),
        Some(4_100),
        4_100,
        52,
        Some(1),
    );
    let output_result = output_engine.process(&output);
    assert_eq!(output_result.alerts[0].rule, AlertRule::ArchiveOutput);
    assert_eq!(output_result.alerts[0].roots, vec![root]);
}

fn sample_alert(id: &str, root: &Path, pid: u32, timestamp_ms: i64) -> Alert {
    Alert {
        id: id.into(),
        rule: AlertRule::BulkFileAccess,
        process: process(pid, Some(9)),
        roots: vec![root.to_path_buf()],
        first_timestamp_ms: timestamp_ms,
        last_timestamp_ms: timestamp_ms,
        unique_files: 50,
        activity_count: 50,
        evidence_paths: vec![root.join("file.rs")],
        is_new: true,
    }
}

#[test]
fn query_dtos_and_notification_outcomes_round_trip_as_snake_case_json() {
    let filter = EventFilter {
        kind: Some(EventKind::Mmap),
        file_path: Some(PathBuf::from("/tmp/synthetic.rs")),
        ..EventFilter::default()
    };
    let encoded = serde_json::to_string(&filter).unwrap();
    assert!(encoded.contains("\"mmap\""));
    assert_eq!(
        serde_json::from_str::<EventFilter>(&encoded).unwrap(),
        filter
    );
    let partial: EventFilter = serde_json::from_str(r#"{"kind":"mmap"}"#).unwrap();
    assert_eq!(partial.kind, Some(EventKind::Mmap));
    assert_eq!(partial.limit, EventFilter::default().limit);
    assert_eq!(
        serde_json::to_string(&NotificationOutcome::Sent).unwrap(),
        "\"sent\""
    );
}

#[test]
fn sqlite_directory_event_alert_notification_and_retention_apis_are_atomic() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let root_alias = temp.path().join("project");
    let database = temp.path().join("monitor.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    assert_eq!(storage.schema_version().unwrap(), 2);
    assert!(storage.add_directory(&root, "manual", 100).unwrap());
    assert!(!storage.add_directory(&root_alias, "manual", 101).unwrap());
    assert!(!storage.add_directory(&root_alias, "codex", 102).unwrap());
    let configured = storage.list_directories().unwrap();
    assert_eq!(configured.len(), 1);
    assert_eq!(configured[0].sources, vec!["codex", "manual"]);

    let mut stored = read_event(root.join("file.rs"), 100, 100, 60, Some(2));
    stored.global_seq = Some(7);
    stored.destination = Some(root.join("renamed.rs"));
    assert!(
        storage
            .record_event(&stored, std::slice::from_ref(&root))
            .unwrap()
    );
    assert!(
        !storage
            .record_event(&stored, std::slice::from_ref(&root))
            .unwrap()
    );
    let no_sequence = read_event(root.join("without-seq.rs"), 101, 101, 60, Some(2));
    assert!(storage.record_event(&no_sequence, &[root.clone()]).unwrap());
    assert!(storage.record_event(&no_sequence, &[root.clone()]).unwrap());

    let queried = storage
        .query_events(&EventFilter {
            directory: Some(root.clone()),
            pid: Some(60),
            since_ms: Some(100),
            until_ms: Some(101),
            file_path: Some(root.join("renamed.rs")),
            ..EventFilter::default()
        })
        .unwrap();
    assert_eq!(queried.len(), 1);
    assert_eq!(queried[0].directories, vec![root.clone()]);
    assert_eq!(queried[0].event.destination, Some(root.join("renamed.rs")));

    let alert = sample_alert("alert-one", &root, 60, 100);
    assert_eq!(
        storage.record_alert_at(&alert, 110).unwrap(),
        AlertWrite::Inserted
    );
    let mut update = alert.clone();
    update.is_new = false;
    update.last_timestamp_ms = 120;
    update.activity_count = 55;
    assert_eq!(
        storage.record_alert_at(&update, 120).unwrap(),
        AlertWrite::Updated
    );
    assert_eq!(storage.pending_notification_summary().unwrap().count, 1);
    assert_eq!(
        storage.pending_notifications(10).unwrap()[0].created_timestamp_ms,
        110
    );
    assert_eq!(
        storage.query_alerts(&AlertFilter::default()).unwrap()[0].activity_count,
        55
    );

    storage
        .record_notification(&NotificationRecord {
            alert_id: alert.id.clone(),
            observed_timestamp_ms: 130,
            outcome: NotificationOutcome::Failed,
            detail: Some("合成通知失败".into()),
        })
        .unwrap();
    assert_eq!(storage.pending_notification_summary().unwrap().count, 1);

    let second = sample_alert("alert-two", &root, 61, 140);
    storage.record_alert_at(&second, 141).unwrap();
    storage
        .record_notification(&NotificationRecord {
            alert_id: second.id.clone(),
            observed_timestamp_ms: 145,
            outcome: NotificationOutcome::Deferred,
            detail: Some("桌面会话暂不可用".into()),
        })
        .unwrap();
    assert_eq!(storage.pending_notification_summary().unwrap().count, 2);
    assert_eq!(
        storage
            .acknowledge_notifications(&[second.id.clone()], 150)
            .unwrap(),
        1
    );
    assert_eq!(
        storage
            .acknowledge_notifications(&[second.id], 151)
            .unwrap(),
        0
    );
    storage
        .record_notification(&NotificationRecord {
            alert_id: alert.id.clone(),
            observed_timestamp_ms: 160,
            outcome: NotificationOutcome::Sent,
            detail: None,
        })
        .unwrap();
    assert_eq!(storage.pending_notification_summary().unwrap().count, 0);
    assert_eq!(
        storage
            .query_notifications(&NotificationFilter::default())
            .unwrap()
            .len(),
        4
    );

    storage
        .record_health(&HealthRecord {
            observed_timestamp_ms: 170,
            component: "rules".into(),
            code: "source_timestamp_missing".into(),
            state: "degraded".into(),
            detail: Some("使用接收时间作为降级窗口时钟。".into()),
        })
        .unwrap();
    assert_eq!(
        storage
            .query_health(&HealthFilter {
                code: Some("source_timestamp_missing".into()),
                ..HealthFilter::default()
            })
            .unwrap()
            .len(),
        1
    );

    let before_prune = storage.cumulative_stats().unwrap();
    assert_eq!(before_prune.events, 3);
    assert_eq!(before_prune.alerts, 2);
    assert_eq!(before_prune.notifications_failed, 1);
    assert_eq!(before_prune.notifications_deferred, 1);
    assert_eq!(before_prune.notifications_acknowledged, 1);
    let pruned = storage.prune_expired(31 * 24 * 60 * 60 * 1_000).unwrap();
    assert_eq!(pruned.events, 3);
    assert_eq!(pruned.alerts, 2);
    assert_eq!(pruned.outbox_entries, 2);
    assert_eq!(storage.cumulative_stats().unwrap(), before_prune);
    assert_eq!(
        storage.query_events(&EventFilter::default()).unwrap().len(),
        0
    );
    assert_eq!(storage.pending_notification_summary().unwrap().count, 0);
}

#[test]
fn sqlite_write_failures_are_returned_without_incrementing_partial_statistics() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let database = temp.path().join("monitor.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    let external = Connection::open(&database).unwrap();
    external
        .execute_batch(
            "CREATE TRIGGER reject_synthetic_events
             BEFORE INSERT ON events
             BEGIN SELECT RAISE(ABORT, '合成写入故障'); END;",
        )
        .unwrap();

    let result = storage.record_event(
        &read_event(root.join("file.rs"), 1_000, 1_000, 70, Some(1)),
        &[root.clone()],
    );
    assert!(result.is_err());
    assert_eq!(
        storage.cumulative_stats().unwrap(),
        CumulativeStats::default()
    );

    external
        .execute_batch(
            "DROP TRIGGER reject_synthetic_events;
             CREATE TRIGGER reject_synthetic_outbox
             BEFORE INSERT ON notification_outbox
             BEGIN SELECT RAISE(ABORT, '合成待通知故障'); END;",
        )
        .unwrap();
    assert!(
        storage
            .record_alert_at(&sample_alert("blocked-alert", &root, 70, 1_001), 1_002)
            .is_err()
    );
    assert!(
        storage
            .query_alerts(&AlertFilter::default())
            .unwrap()
            .is_empty()
    );
    assert_eq!(storage.pending_notification_summary().unwrap().count, 0);
    assert_eq!(
        storage.cumulative_stats().unwrap(),
        CumulativeStats::default()
    );
}

#[test]
fn sqlite_writer_lock_returns_before_the_alert_latency_budget() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let database = temp.path().join("monitor.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    let locker = Connection::open(&database).unwrap();
    locker.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let started = Instant::now();
    let result = storage.record_event(
        &read_event(root.join("file.rs"), 1_000, 1_000, 80, Some(1)),
        std::slice::from_ref(&root),
    );
    let elapsed = started.elapsed();
    locker.execute_batch("ROLLBACK;").unwrap();

    assert!(result.is_err());
    assert!(
        elapsed < Duration::from_secs(1),
        "SQLite busy wait: {elapsed:?}"
    );
}

#[test]
fn notification_sequence_snapshot_confirms_large_backlog_without_new_clock_values() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let mut storage = Storage::open(":memory:").unwrap();
    // 超过详情查询上限的历史积压，必须通过数据库批量确认。
    let backlog = 10_001;
    for index in 0..backlog {
        storage
            .record_alert_at(&sample_alert(&format!("old-{index}"), &root, 90, 100), 100)
            .unwrap();
    }
    let summary = storage.pending_notification_summary().unwrap();
    assert_eq!(summary.count, backlog as u64);
    assert_eq!(summary.by_rule[0].count, backlog as u64);
    let cutoff = summary.latest_sequence.unwrap();
    // 摘要返回以后入队，时间既可以相同，也可以因墙钟回拨而更早。
    storage
        .record_alert_at(&sample_alert("new-same-time", &root, 91, 100), 100)
        .unwrap();
    storage
        .record_alert_at(&sample_alert("new-earlier-time", &root, 92, 100), 0)
        .unwrap();
    assert_eq!(
        storage
            .acknowledge_pending_notifications_through(cutoff, 500)
            .unwrap(),
        backlog
    );
    let pending = storage.pending_notifications(10).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].alert.id, "new-earlier-time");
    assert_eq!(pending[1].alert.id, "new-same-time");
    assert!(
        storage
            .pending_notification_summary()
            .unwrap()
            .latest_sequence
            .unwrap()
            > cutoff
    );
    let stats = storage.cumulative_stats().unwrap();
    assert_eq!(stats.notifications_acknowledged, backlog as u64);
    assert_eq!(
        storage
            .recent_stats(0, 1_000)
            .unwrap()
            .notifications_acknowledged,
        backlog as u64
    );
    assert_eq!(
        storage
            .acknowledge_pending_notifications_through(cutoff, 501)
            .unwrap(),
        0
    );
    assert_eq!(storage.cumulative_stats().unwrap(), stats);
    assert_eq!(
        storage
            .recent_stats(0, 1_000)
            .unwrap()
            .notifications_acknowledged,
        backlog as u64
    );

    // 详情全部清理后仍不能复用序号，使旧摘要确认保持安全。
    storage.prune_expired(31 * 24 * 60 * 60 * 1_000).unwrap();
    assert_eq!(
        storage
            .pending_notification_summary()
            .unwrap()
            .latest_sequence,
        None
    );
    storage
        .record_alert_at(&sample_alert("after-prune", &root, 93, 100), 100)
        .unwrap();
    assert!(
        storage
            .pending_notification_summary()
            .unwrap()
            .latest_sequence
            .unwrap()
            > cutoff
    );
    assert_eq!(
        storage
            .acknowledge_pending_notifications_through(cutoff, 502)
            .unwrap(),
        0
    );
}

#[test]
fn notification_snapshot_acknowledgement_rolls_back_all_feedback_and_statistics() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let database = temp.path().join("monitor.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    for id in ["one", "two"] {
        storage
            .record_alert_at(&sample_alert(id, &root, 94, 100), 100)
            .unwrap();
    }
    let summary = storage.pending_notification_summary().unwrap();
    let before = storage.cumulative_stats().unwrap();
    let external = Connection::open(&database).unwrap();
    external
        .execute_batch(
            "CREATE TRIGGER reject_second_acknowledgement
             BEFORE UPDATE ON notification_outbox
             WHEN NEW.alert_id = 'two' AND NEW.acknowledged_timestamp_ms IS NOT NULL
         BEGIN SELECT RAISE(ABORT, '合成通知反馈故障'); END;",
        )
        .unwrap();
    assert!(
        storage
            .acknowledge_pending_notifications_through(summary.latest_sequence.unwrap(), 500)
            .is_err()
    );
    assert_eq!(storage.pending_notification_summary().unwrap().count, 2);
    assert!(
        storage
            .query_notifications(&NotificationFilter::default())
            .unwrap()
            .is_empty()
    );
    assert_eq!(storage.cumulative_stats().unwrap(), before);
}

#[test]
fn schema_v1_migration_preserves_evidence_feedback_and_pending_outbox() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let database = temp.path().join("monitor.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    storage.add_directory(&root, "manual", 100).unwrap();
    let observed = read_event(root.join("file.rs"), 100, 110, 95, Some(1));
    storage
        .record_event(&observed, std::slice::from_ref(&root))
        .unwrap();
    storage
        .record_health(&HealthRecord {
            observed_timestamp_ms: 110,
            component: "synthetic".into(),
            code: "fixture".into(),
            state: "ready".into(),
            detail: None,
        })
        .unwrap();
    storage
        .record_alert_at(&sample_alert("pending-v1", &root, 95, 100), 110)
        .unwrap();
    storage
        .record_alert_at(&sample_alert("sent-v1", &root, 96, 101), 111)
        .unwrap();
    storage
        .record_notification(&NotificationRecord {
            alert_id: "sent-v1".into(),
            observed_timestamp_ms: 112,
            outcome: NotificationOutcome::Sent,
            detail: None,
        })
        .unwrap();
    let directories = storage.list_directories().unwrap();
    let events = storage.query_events(&EventFilter::default()).unwrap();
    let alerts = storage.query_alerts(&AlertFilter::default()).unwrap();
    let health = storage.query_health(&HealthFilter::default()).unwrap();
    let feedback = storage
        .query_notifications(&NotificationFilter::default())
        .unwrap();
    let cumulative = storage.cumulative_stats().unwrap();
    drop(storage);

    // 其余 v1 表未变；按原 v1 定义重建唯一发生演进的 outbox，形成旧库夹具。
    let legacy = Connection::open(&database).unwrap();
    legacy
        .execute_batch(
            "BEGIN;
         CREATE TABLE notification_outbox_v1 (
             alert_id TEXT PRIMARY KEY NOT NULL,
             created_timestamp_ms INTEGER NOT NULL,
             acknowledged_timestamp_ms INTEGER,
             FOREIGN KEY (alert_id) REFERENCES alerts(id) ON DELETE CASCADE
         );
         INSERT INTO notification_outbox_v1
         SELECT alert_id, created_timestamp_ms, acknowledged_timestamp_ms
         FROM notification_outbox ORDER BY sequence;
         DROP TABLE notification_outbox;
         ALTER TABLE notification_outbox_v1 RENAME TO notification_outbox;
         CREATE INDEX notification_outbox_pending
             ON notification_outbox (acknowledged_timestamp_ms, created_timestamp_ms);
         PRAGMA user_version = 1;
         COMMIT;",
        )
        .unwrap();
    assert_eq!(
        legacy
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(legacy);

    let mut migrated = Storage::open(&database).unwrap();
    assert_eq!(migrated.schema_version().unwrap(), 2);
    assert_eq!(migrated.list_directories().unwrap(), directories);
    assert_eq!(
        migrated.query_events(&EventFilter::default()).unwrap(),
        events
    );
    assert_eq!(
        migrated.query_alerts(&AlertFilter::default()).unwrap(),
        alerts
    );
    assert_eq!(
        migrated.query_health(&HealthFilter::default()).unwrap(),
        health
    );
    assert_eq!(
        migrated
            .query_notifications(&NotificationFilter::default())
            .unwrap(),
        feedback
    );
    assert_eq!(migrated.cumulative_stats().unwrap(), cumulative);
    let summary = migrated.pending_notification_summary().unwrap();
    assert_eq!(summary.count, 1);
    assert_eq!(summary.oldest_created_timestamp_ms, Some(110));
    assert_eq!(
        migrated.pending_notifications(10).unwrap()[0].alert.id,
        "pending-v1"
    );
    migrated
        .record_alert_at(&sample_alert("new-v2", &root, 97, 99), 99)
        .unwrap();
    assert_eq!(
        migrated
            .acknowledge_pending_notifications_through(summary.latest_sequence.unwrap(), 120)
            .unwrap(),
        1
    );
    assert_eq!(
        migrated.pending_notifications(10).unwrap()[0].alert.id,
        "new-v2"
    );
    assert_eq!(
        migrated
            .query_notifications(&NotificationFilter {
                alert_id: Some("sent-v1".into()),
                ..NotificationFilter::default()
            })
            .unwrap(),
        feedback
    );
    drop(migrated);
    let reopened = Storage::open(database).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 2);
    assert_eq!(reopened.pending_notification_summary().unwrap().count, 1);
}
