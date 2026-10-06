use codeperimeter::model::{
    ActivityEvent, Alert, AlertRule, ArchiveCommand, EventKind, FileEvidence, ProcessIdentity,
    SourceStream,
};
use codeperimeter::rules::{RuleConfig, RuleEngine};
use codeperimeter::storage::{
    AlertFilter, AlertWrite, CumulativeStats, EventFilter, HealthFilter, HealthRecord,
    NotificationFilter, NotificationOutcome, NotificationRecord, SourceContext, Storage,
};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn fixture() -> TempDir {
    tempfile::tempdir().expect("临时目录创建成功")
}

#[test]
fn legacy_health_source_json_defaults_new_numeric_identity_fields() {
    let source: SourceContext = serde_json::from_value(serde_json::json!({
        "run_id": "synthetic-legacy-run",
        "schema_version": 1,
        "message_version": 9,
        "field": "event.exec.args",
        "missing_events": null
    }))
    .unwrap();
    assert_eq!(source.pid, None);
    assert_eq!(source.pid_version, None);
    assert_eq!(source.global_seq, None);
    let serialized = serde_json::to_value(source).unwrap();
    assert!(serialized.get("pid").is_none());
    assert!(serialized.get("pid_version").is_none());
    assert!(serialized.get("global_seq").is_none());
}

#[test]
fn legacy_event_and_source_json_default_to_combined_stream() {
    let mut legacy =
        serde_json::to_value(event(EventKind::Exec, None, Some(1), 2, 30, Some(7))).unwrap();
    legacy.as_object_mut().unwrap().remove("source_stream");
    let parsed: ActivityEvent = serde_json::from_value(legacy).unwrap();
    assert_eq!(
        serde_json::to_value(parsed).unwrap()["source_stream"],
        "combined"
    );
    let source: SourceContext = serde_json::from_value(serde_json::json!({
        "run_id": "synthetic-legacy-run", "schema_version": 1, "message_version": 9,
        "field": null, "missing_events": null
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(source).unwrap()["source_stream"],
        "combined"
    );
}

#[test]
fn rules_share_known_generations_but_keep_unknown_observation_instances_in_their_stream() {
    let temp = fixture();
    let root = protected_root(&temp, "partitioned-project");
    for generation in [Some(7), None] {
        let mut engine = RuleEngine::new(vec![root.clone()], rule_config(2)).unwrap();
        let mut first = read_event(root.join("first.rs"), 1_000, 1_000, 30, generation);
        first.source_stream = SourceStream::Activity;
        assert!(engine.process(&first).alerts.is_empty());
        let mut command = event(EventKind::Exec, None, Some(1_010), 1_010, 30, generation);
        command.source_stream = SourceStream::Exec;
        command.archive = Some(ArchiveCommand {
            tool: "gzip".into(),
            input_paths: Vec::new(),
            output_path: None,
            output_paths: Vec::new(),
            cwd: None,
        });
        let command_result = engine.process(&command);
        if generation.is_some() {
            assert_eq!(command_result.alerts.len(), 1);
            assert_eq!(command_result.alerts[0].rule, AlertRule::ArchiveCommand);
            assert_eq!(command_result.alerts[0].roots, vec![root.clone()]);
        } else {
            assert!(command_result.alerts.is_empty());
        }
        let mut second = read_event(root.join("second.rs"), 1_020, 1_020, 30, generation);
        second.source_stream = SourceStream::Activity;
        let second_result = engine.process(&second);
        assert_eq!(second_result.alerts.len(), 1);
        assert_eq!(second_result.alerts[0].rule, AlertRule::BulkFileAccess);
        assert_eq!(second_result.alerts[0].unique_files, 2);
        let mut exit = event(EventKind::Exit, None, Some(1_030), 1_030, 30, generation);
        exit.source_stream = SourceStream::Activity;
        engine.process(&exit);
        let mut third = read_event(root.join("third.rs"), 1_040, 1_040, 30, generation);
        third.source_stream = SourceStream::Activity;
        assert!(engine.process(&third).alerts.is_empty());
    }
}

#[test]
fn partitioned_archive_association_uses_source_order_when_the_read_arrives_late() {
    let temp = fixture();
    let root = protected_root(&temp, "anonymous-order-project");
    for exec_first in [false, true] {
        let mut engine = RuleEngine::new(vec![root.clone()], rule_config(50)).unwrap();
        let mut read = read_event(root.join("source.rs"), 1_000, 5_000, 30, Some(7));
        read.source_stream = SourceStream::Activity;
        let mut command = event(EventKind::Exec, None, Some(1_010), 2_000, 30, Some(7));
        command.source_stream = SourceStream::Exec;
        command.archive = Some(ArchiveCommand {
            tool: "gzip".into(),
            input_paths: Vec::new(),
            output_path: None,
            output_paths: Vec::new(),
            cwd: None,
        });
        let output = if exec_first {
            assert!(engine.process(&command).alerts.is_empty());
            engine.process(&read)
        } else {
            assert!(engine.process(&read).alerts.is_empty());
            engine.process(&command)
        };
        assert_eq!(output.alerts.len(), 1, "已知进程关联不应依赖两路到达顺序");
        assert_eq!(output.alerts[0].rule, AlertRule::ArchiveCommand);
        assert_eq!(output.alerts[0].last_timestamp_ms, 1_010);
        assert_eq!(output.alerts[0].roots, vec![root.clone()]);
        if exec_first {
            assert_eq!(
                output.reassociated_exec,
                Some((command.clone(), vec![root.clone()]))
            );
        } else {
            assert!(output.reassociated_exec.is_none());
        }
        let repeated = engine.process(&read);
        assert!(repeated.reassociated_exec.is_none());
        assert!(
            !repeated
                .alerts
                .iter()
                .any(|alert| alert.rule == AlertRule::ArchiveCommand)
        );
    }
}

#[test]
fn partitioned_archive_output_survives_late_reads_and_pre_exit_stream_reordering() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let root = protected_root(&temp, "anonymous-output-project");
    for order in 0..4 {
        let mut engine = RuleEngine::new(vec![root.clone()], rule_config(50)).unwrap();
        let mut read = read_event(root.join("source.rs"), 1_000, 5_000, 30, Some(7));
        read.source_stream = SourceStream::Read;
        let mut output = event(
            EventKind::Create,
            Some(temp.path().join("synthetic-output.zip")),
            Some(1_010),
            2_000,
            30,
            Some(7),
        );
        output.source_stream = SourceStream::Activity;
        let mut exit = event(EventKind::Exit, None, Some(1_020), 3_000, 30, Some(7));
        exit.source_stream = SourceStream::Activity;
        let result = match order {
            0 => {
                assert!(engine.process(&output).alerts.is_empty());
                engine.process(&read)
            }
            1 => {
                engine.process(&output);
                engine.process(&exit);
                engine.process(&read)
            }
            2 => {
                engine.process(&exit);
                engine.process(&output);
                engine.process(&read)
            }
            3 => {
                engine.process(&read);
                engine.process(&exit);
                output.kind = EventKind::Write;
                output.source_stream = SourceStream::Write;
                output.received_timestamp_ms = 6_000;
                engine.process(&output)
            }
            _ => unreachable!(),
        };
        assert_eq!(result.alerts.len(), 1, "order={order}");
        assert_eq!(result.alerts[0].rule, AlertRule::ArchiveOutput);
        assert_eq!(result.alerts[0].first_timestamp_ms, 1_000);
        assert_eq!(result.alerts[0].last_timestamp_ms, 1_010);
        assert_eq!(result.alerts[0].roots, vec![root.clone()]);
    }
}

#[test]
fn repeated_output_event_adds_correlated_roots_without_duplicate_rows_or_counts() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let input_root = protected_root(&temp, "anonymous-input-project");
    let output_root = protected_root(&temp, "anonymous-output-project");
    let mut storage = Storage::open(temp.path().join("evidence.sqlite")).unwrap();
    for root in [&input_root, &output_root] {
        storage.add_directory(root, "manual", 1).unwrap();
    }
    let mut output = event(
        EventKind::Create,
        Some(output_root.join("synthetic.zip")),
        Some(1_010),
        2_000,
        30,
        Some(7),
    );
    output.source_stream = SourceStream::Activity;
    output.global_seq = Some(1);
    assert!(
        storage
            .record_event(&output, std::slice::from_ref(&output_root))
            .unwrap()
    );
    for _ in 0..2 {
        assert!(
            !storage
                .record_event(&output, &[input_root.clone(), output_root.clone()])
                .unwrap()
        );
    }
    let events = storage.query_events(&EventFilter::default()).unwrap();
    assert_eq!(events.len(), 1);
    let mut roots = vec![input_root, output_root];
    roots.sort();
    assert_eq!(events[0].directories, roots);
    assert_eq!(storage.cumulative_stats().unwrap().events, 1);
}

#[test]
fn late_archive_outputs_reject_unproven_paths_identity_time_and_exit_boundaries() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let root = protected_root(&temp, "anonymous-output-boundaries");
    let excluded = root.join("excluded");
    std::fs::create_dir(&excluded).unwrap();
    for case in 0..24 {
        let mut config = rule_config(50);
        if case == 21 {
            config.max_process_states = 1;
        }
        let mut engine = RuleEngine::new(vec![root.clone()], config).unwrap();
        let mut read = read_event(root.join("source.rs"), 1_000, 5_000, 30, Some(7));
        read.source_stream = SourceStream::Read;
        let mut original = event(
            EventKind::Create,
            Some(temp.path().join("synthetic.zip")),
            Some(1_010),
            2_000,
            30,
            Some(7),
        );
        original.source_stream = SourceStream::Activity;
        let mut exit = event(EventKind::Exit, None, Some(1_020), 3_000, 30, Some(7));
        exit.source_stream = SourceStream::Activity;
        match case {
            0 => read.source_timestamp_ms = Some(1_020),
            1 => original.source_timestamp_ms = Some(100_000),
            2 => read.source_timestamp_ms = None,
            3 => original.source_timestamp_ms = None,
            4 => {
                read.process.pid_version = None;
                original.process.pid_version = None;
            }
            5 => read.process.pid_version = Some(8),
            6 => read.process.pid = 31,
            7 => read.source_run_id = "synthetic-other-run".into(),
            8 => {
                read.source_stream = SourceStream::Combined;
                original.source_stream = SourceStream::Combined;
            }
            9 => read.source_stream = SourceStream::Combined,
            10 => read.file.as_mut().unwrap().readable = Some(false),
            11 => read.file.as_mut().unwrap().path_truncated = true,
            12 => original.file.as_mut().unwrap().path_truncated = true,
            13 => original.file.as_mut().unwrap().is_regular = Some(false),
            14 => {
                engine
                    .replace_scope(vec![root.clone()], vec![excluded.clone()])
                    .unwrap();
                read.file.as_mut().unwrap().path = excluded.join("source.rs");
            }
            15 => {
                engine
                    .replace_scope(vec![root.clone()], vec![excluded.clone()])
                    .unwrap();
                original.file.as_mut().unwrap().path = excluded.join("synthetic.zip");
            }
            16 => read.file.as_mut().unwrap().is_regular = Some(false),
            17 => exit.source_timestamp_ms = Some(990),
            18 => {
                exit.source_timestamp_ms = Some(1_005);
                engine.process(&exit);
            }
            19 => exit.source_timestamp_ms = None,
            20 => exit.source_stream = SourceStream::Combined,
            21 => {}
            22 => read.source_stream = SourceStream::Write,
            23 => original.source_stream = SourceStream::Read,
            _ => unreachable!(),
        }
        assert!(engine.process(&original).alerts.is_empty());
        if matches!(case, 17 | 19 | 20) {
            engine.process(&exit);
        }
        if case == 21 {
            engine.process(&read_event(
                root.join("other.rs"),
                1_000,
                4_000,
                31,
                Some(8),
            ));
        }
        let result = engine.process(&read);
        assert!(result.reassociated_outputs.is_empty(), "case={case}");
        assert!(
            !result
                .alerts
                .iter()
                .any(|alert| alert.rule == AlertRule::ArchiveOutput),
            "case={case}"
        );
    }
}

#[test]
fn pending_archive_output_capacity_is_bounded_and_reports_lost_candidates() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let root = protected_root(&temp, "anonymous-output-capacity");
    let mut engine = RuleEngine::new(
        vec![root.clone()],
        RuleConfig {
            max_evidence_paths: 2,
            ..rule_config(50)
        },
    )
    .unwrap();
    for index in 0..3 {
        let mut original = event(
            EventKind::Create,
            Some(temp.path().join(format!("synthetic-{index}.zip"))),
            Some(1_010 + index),
            2_000 + index,
            30,
            Some(7),
        );
        original.source_stream = SourceStream::Activity;
        let result = engine.process(&original);
        assert_eq!(
            result
                .health
                .iter()
                .any(|item| item.code == "archive_output_state_capacity"),
            index == 2
        );
    }
    let mut read = read_event(root.join("source.rs"), 1_000, 5_000, 30, Some(7));
    read.source_stream = SourceStream::Read;
    let result = engine.process(&read);
    assert_eq!(result.reassociated_outputs.len(), 2);
    assert_eq!(result.alerts.len(), 2);
    assert_eq!(result.alerts[1].activity_count, 2);
    assert!(engine.process(&read).reassociated_outputs.is_empty());
}

#[test]
fn late_archive_association_rejects_unproven_identity_time_and_read_evidence() {
    let temp = fixture();
    let root = protected_root(&temp, "anonymous-late-project");
    for case in 0..14 {
        let mut config = rule_config(50);
        if case == 13 {
            config.max_process_states = 1;
        }
        let mut engine = RuleEngine::new(vec![root.clone()], config).unwrap();
        let mut read = read_event(root.join("source.rs"), 1_000, 5_000, 30, Some(7));
        read.source_stream = SourceStream::Activity;
        let mut command = event(EventKind::Exec, None, Some(1_010), 2_000, 30, Some(7));
        command.source_stream = SourceStream::Exec;
        command.archive = Some(ArchiveCommand {
            tool: "gzip".into(),
            input_paths: Vec::new(),
            output_path: None,
            output_paths: Vec::new(),
            cwd: None,
        });
        match case {
            0 => read.source_timestamp_ms = Some(1_020),
            1 => command.source_timestamp_ms = Some(100_000),
            2 => read.source_timestamp_ms = None,
            3 => command.source_timestamp_ms = None,
            4 => {
                read.process.pid_version = None;
                command.process.pid_version = None;
            }
            5 => read.process.pid_version = Some(8),
            6 => read.process.pid = 31,
            7 => read.source_run_id = "anonymous-other-run".into(),
            8 => {
                read.source_stream = SourceStream::Combined;
                command.source_stream = SourceStream::Combined;
            }
            9 => read.source_stream = SourceStream::Combined,
            10 => read.file.as_mut().unwrap().readable = Some(false),
            11 => read.file.as_mut().unwrap().path_truncated = true,
            12 | 13 => {}
            _ => unreachable!(),
        }
        assert!(engine.process(&command).alerts.is_empty());
        if case == 12 {
            let mut exit = event(EventKind::Exit, None, Some(990), 3_000, 30, Some(7));
            exit.source_stream = SourceStream::Activity;
            engine.process(&exit);
        } else if case == 13 {
            let mut other = read_event(root.join("other.rs"), 1_000, 4_000, 31, Some(8));
            other.source_stream = SourceStream::Activity;
            engine.process(&other);
        }
        let result = engine.process(&read);
        assert!(result.reassociated_exec.is_none(), "case={case}");
        assert!(
            !result
                .alerts
                .iter()
                .any(|alert| alert.rule == AlertRule::ArchiveCommand),
            "case={case}"
        );
    }
}

#[test]
fn late_archive_candidate_is_single_and_keeps_only_prior_project_roots() {
    let temp = fixture();
    let root = protected_root(&temp, "anonymous-prior-project");
    let future_root = protected_root(&temp, "anonymous-future-project");
    let mut engine =
        RuleEngine::new(vec![root.clone(), future_root.clone()], rule_config(2)).unwrap();
    let mut command = event(EventKind::Exec, None, Some(1_010), 2_000, 30, Some(7));
    command.source_stream = SourceStream::Exec;
    command.archive = Some(ArchiveCommand {
        tool: "gzip".into(),
        input_paths: Vec::new(),
        output_path: None,
        output_paths: Vec::new(),
        cwd: None,
    });
    engine.process(&command);
    command.source_timestamp_ms = Some(1_015);
    command.received_timestamp_ms = 2_005;
    command.global_seq = Some(2);
    engine.process(&command);
    let mut future = read_event(future_root.join("future.rs"), 1_020, 3_000, 30, Some(7));
    future.source_stream = SourceStream::Activity;
    assert!(engine.process(&future).reassociated_exec.is_none());
    let mut prior = read_event(root.join("source.rs"), 1_000, 5_000, 30, Some(7));
    prior.source_stream = SourceStream::Activity;
    let result = engine.process(&prior);
    assert_eq!(
        result.reassociated_exec,
        Some((command, vec![root.clone()]))
    );
    let alert = result
        .alerts
        .iter()
        .find(|alert| alert.rule == AlertRule::ArchiveCommand)
        .unwrap();
    assert_eq!(alert.last_timestamp_ms, 1_015);
    assert_eq!(alert.roots, vec![root]);
    assert!(
        result
            .alerts
            .iter()
            .any(|alert| alert.rule == AlertRule::BulkFileAccess)
    );
    assert!(result.matched_directories.contains(&future_root));
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
        source_stream: SourceStream::Combined,
        source_schema_version: Some(1),
        source_message_version: Some(9),
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
        output_paths: Vec::new(),
        cwd: Some(PathBuf::from("/tmp")),
    });
    assert!(engine.process(&unrelated).alerts.is_empty());

    let mut command = event(EventKind::Exec, None, Some(2_000), 2_000, 50, Some(1));
    command.archive = Some(ArchiveCommand {
        tool: "/usr/bin/zip".into(),
        input_paths: vec![PathBuf::from("source.rs")],
        output_path: Some(temp.path().join("synthetic.zip")),
        output_paths: Vec::new(),
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

#[test]
fn stdin_cwd_alone_is_not_project_evidence_but_explicit_outputs_and_reads_are() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let mut engine = RuleEngine::new(vec![root.clone()], rule_config(50)).unwrap();
    let mut command = event(EventKind::Exec, None, Some(1_000), 1_000, 50, Some(1));
    command.archive = Some(ArchiveCommand {
        tool: "gzip".into(),
        input_paths: Vec::new(),
        output_path: None,
        output_paths: Vec::new(),
        cwd: Some(root.clone()),
    });
    let unknown_source = engine.process(&command);
    assert!(unknown_source.alerts.is_empty());
    assert!(unknown_source.matched_directories.is_empty());
    command.archive.as_mut().unwrap().output_path = Some(root.join("stdin-result.gz"));
    let output_evidence = engine.process(&command);
    assert_eq!(output_evidence.alerts[0].rule, AlertRule::ArchiveCommand);
    assert_eq!(output_evidence.alerts[0].roots, [root.clone()]);
    command.archive.as_mut().unwrap().output_path = None;
    engine.process(&read_event(
        root.join("source.rs"),
        2_000,
        2_000,
        51,
        Some(1),
    ));
    command.process.pid = 51;
    command.source_timestamp_ms = Some(2_001);
    command.received_timestamp_ms = 2_001;
    let observed_input = engine.process(&command);
    assert_eq!(observed_input.alerts[0].roots, [root]);
}

#[test]
fn multiple_output_paths_associate_all_roots_and_legacy_archive_json_remains_readable() {
    let temp = fixture();
    let root_a = protected_root(&temp, "project-a");
    let root_b = protected_root(&temp, "project-b");
    let outputs = vec![root_a.join("first.rs.gz"), root_b.join("second.rs.gz")];
    let mut engine =
        RuleEngine::new(vec![root_a.clone(), root_b.clone()], rule_config(50)).unwrap();
    let mut command = event(EventKind::Exec, None, Some(1_000), 1_000, 50, Some(1));
    command.archive = Some(ArchiveCommand {
        tool: "gzip".into(),
        input_paths: vec![
            PathBuf::from("/private/tmp/unrelated-first.rs"),
            PathBuf::from("/private/tmp/unrelated-second.rs"),
        ],
        output_path: None,
        output_paths: outputs.clone(),
        cwd: None,
    });
    let output = engine.process(&command);
    assert_eq!(output.alerts[0].roots, [root_a.clone(), root_b.clone()]);
    assert_eq!(output.matched_directories, [root_a.clone(), root_b.clone()]);
    for path in outputs {
        assert!(output.alerts[0].evidence_paths.contains(&path));
    }
    let database = temp.path().join("multi-output.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    storage
        .record_event(&command, &output.matched_directories)
        .unwrap();
    storage.record_alert(&output.alerts[0]).unwrap();
    let mut legacy = command.clone();
    legacy.received_timestamp_ms = 2_000;
    legacy.archive.as_mut().unwrap().output_paths.clear();
    legacy.archive.as_mut().unwrap().output_path = Some(root_a.join("legacy.gz"));
    let old_json = serde_json::to_string(&legacy).unwrap();
    assert!(!old_json.contains("output_paths"));
    let decoded: ActivityEvent = serde_json::from_str(&old_json).unwrap();
    assert!(decoded.archive.as_ref().unwrap().output_paths.is_empty());
    storage.record_event(&decoded, &[root_a]).unwrap();
    drop(storage);
    let reopened = Storage::open(&database).unwrap();
    let events = reopened.query_events(&EventFilter::default()).unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().any(|row| row.event == command));
    assert!(events.iter().any(|row| row.event == legacy));
    let saved = reopened.query_alerts(&AlertFilter::default()).unwrap();
    let mut expected = output.alerts;
    expected[0].is_new = false;
    assert_eq!(saved, expected);
    assert_eq!(reopened.pending_notifications(10).unwrap().len(), 1);
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
    assert_eq!(storage.schema_version().unwrap(), 4);
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
            source: None,
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
fn sqlite_partitioned_sequences_are_distinct_and_combined_accepts_legacy_rows() {
    let temp = fixture();
    let database = temp.path().join("partitioned.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    let mut legacy = event(EventKind::Exec, None, Some(100), 100, 60, Some(2));
    legacy.global_seq = Some(7);
    assert!(storage.record_event(&legacy, &[]).unwrap());
    drop(storage);
    // 模拟既有 schema 3 的 Combined 行；旧格式没有来源流字段。
    Connection::open(&database)
        .unwrap()
        .execute(
            "UPDATE events SET sequence_key='g:7', event_json=json_remove(event_json,'$.source_stream')",
            [],
        )
        .unwrap();
    let mut storage = Storage::open(&database).unwrap();
    assert!(!storage.record_event(&legacy, &[]).unwrap());
    let mut command = legacy.clone();
    command.source_stream = SourceStream::Exec;
    assert!(storage.record_event(&command, &[]).unwrap());
    assert!(!storage.record_event(&command, &[]).unwrap());
    let mut activity = legacy.clone();
    activity.kind = EventKind::Exit;
    activity.source_stream = SourceStream::Activity;
    assert!(storage.record_event(&activity, &[]).unwrap());
    assert!(!storage.record_event(&activity, &[]).unwrap());
    assert_eq!(storage.schema_version().unwrap(), 4);
    let rows = storage.query_events(&EventFilter::default()).unwrap();
    assert_eq!(rows.len(), 3);
    for source_stream in [
        SourceStream::Combined,
        SourceStream::Exec,
        SourceStream::Activity,
    ] {
        assert_eq!(
            rows.iter()
                .filter(|row| row.event.source_stream == source_stream)
                .count(),
            1
        );
    }
    let mut fallback = event(EventKind::Open, None, Some(101), 101, 60, Some(2));
    fallback.event_seq = Some(9);
    assert!(storage.record_event(&fallback, &[]).unwrap());
    fallback.source_stream = SourceStream::Activity;
    assert!(storage.record_event(&fallback, &[]).unwrap());
    assert!(!storage.record_event(&fallback, &[]).unwrap());
    assert_eq!(
        storage.query_events(&EventFilter::default()).unwrap().len(),
        5
    );
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
            source: None,
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
         ALTER TABLE health_records DROP COLUMN source_json;
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
    assert_eq!(migrated.schema_version().unwrap(), 4);
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
    assert_eq!(reopened.schema_version().unwrap(), 4);
    assert_eq!(reopened.pending_notification_summary().unwrap().count, 1);
}

#[test]
fn schema_v2_migration_preserves_legacy_rows_statistics_and_new_source_context() {
    let temp = fixture();
    let root = protected_root(&temp, "project");
    let database = temp.path().join("monitor.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    storage.add_directory(&root, "manual", 100).unwrap();
    let mut observed = read_event(root.join("file.rs"), 100, 110, 95, Some(1));
    observed.source_schema_version = None;
    observed.source_message_version = None;
    storage
        .record_event(&observed, std::slice::from_ref(&root))
        .unwrap();
    storage
        .record_health(&HealthRecord {
            observed_timestamp_ms: 110,
            component: "synthetic".into(),
            code: "legacy".into(),
            state: "ready".into(),
            detail: None,
            source: None,
        })
        .unwrap();
    storage
        .record_alert_at(&sample_alert("pending-v2", &root, 95, 100), 110)
        .unwrap();
    let events = storage.query_events(&EventFilter::default()).unwrap();
    let health = storage.query_health(&HealthFilter::default()).unwrap();
    let alerts = storage.query_alerts(&AlertFilter::default()).unwrap();
    let pending = storage.pending_notifications(10).unwrap();
    let stats = storage.cumulative_stats().unwrap();
    drop(storage);
    let legacy = Connection::open(&database).unwrap();
    legacy.execute_batch("BEGIN;
        UPDATE events SET event_json = json_remove(event_json,'$.source_schema_version','$.source_message_version');
        ALTER TABLE health_records DROP COLUMN source_json;
        PRAGMA user_version = 2; COMMIT;").unwrap();
    drop(legacy);
    let mut migrated = Storage::open(&database).unwrap();
    assert_eq!(migrated.schema_version().unwrap(), 4);
    assert_eq!(
        migrated.query_events(&EventFilter::default()).unwrap(),
        events
    );
    assert_eq!(
        migrated.query_health(&HealthFilter::default()).unwrap(),
        health
    );
    assert_eq!(
        migrated.query_alerts(&AlertFilter::default()).unwrap(),
        alerts
    );
    assert_eq!(migrated.pending_notifications(10).unwrap(), pending);
    assert_eq!(migrated.cumulative_stats().unwrap(), stats);
    let source = SourceContext {
        run_id: "anonymous-run".into(),
        source_stream: SourceStream::Combined,
        schema_version: Some(1),
        message_version: Some(9),
        field: Some("global_seq_num".into()),
        missing_events: Some(3),
        pid: Some(42),
        pid_version: Some(7),
        global_seq: Some(11),
    };
    migrated
        .record_health(&HealthRecord {
            observed_timestamp_ms: 120,
            component: "eslogger".into(),
            code: "sequence_gap".into(),
            state: "degraded".into(),
            detail: Some("观察到事件序号间隙，覆盖存在缺口".into()),
            source: Some(source.clone()),
        })
        .unwrap();
    drop(migrated);
    let reopened = Storage::open(&database).unwrap();
    let rows = reopened
        .query_health(&HealthFilter {
            source_run_id: Some("anonymous-run".into()),
            ..HealthFilter::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].record.source, Some(source));
    assert_eq!(
        reopened.cumulative_stats().unwrap().health_records,
        stats.health_records + 1
    );
}
