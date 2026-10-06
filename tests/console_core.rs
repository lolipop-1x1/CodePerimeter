use codeperimeter::console::RulesSettings;
use codeperimeter::model::{
    ActivityEvent, Alert, AlertRule, ArchiveCommand, EventKind, FileEvidence, ProcessIdentity,
    SourceStream, now_ms,
};
use codeperimeter::rules::{RuleConfig, RuleEngine};
use codeperimeter::storage::{
    AlertFilter, EventFilter, HealthRecord, NotificationOutcome, NotificationRecord, Storage,
};
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn identity() -> ProcessIdentity {
    ProcessIdentity {
        pid: 100,
        pid_version: Some(7),
        ppid: Some(1),
        executable: Some("/usr/bin/synthetic-reader".into()),
        signing_id: None,
        team_id: None,
    }
}
fn event(path: PathBuf, sequence: u64, time: i64) -> ActivityEvent {
    ActivityEvent {
        source_run_id: "synthetic-console".into(),
        source_stream: SourceStream::Activity,
        source_schema_version: Some(1),
        source_message_version: Some(9),
        source_timestamp_ms: Some(time),
        received_timestamp_ms: time,
        global_seq: Some(sequence),
        event_seq: Some(sequence),
        kind: EventKind::Open,
        process: identity(),
        file: Some(FileEvidence {
            path,
            path_truncated: false,
            device: None,
            inode: None,
            is_regular: Some(true),
            readable: Some(true),
        }),
        destination: None,
        modified: None,
        archive: None,
    }
}
fn alert(id: &str, time: i64) -> Alert {
    Alert {
        id: id.into(),
        rule: AlertRule::BulkFileAccess,
        process: identity(),
        roots: vec!["/private/tmp/synthetic-project".into()],
        first_timestamp_ms: time,
        last_timestamp_ms: time,
        unique_files: 50,
        activity_count: 50,
        evidence_paths: vec!["/private/tmp/synthetic-project/source.rs".into()],
        is_new: true,
    }
}

#[test]
fn schema_v3_migration_keeps_unknown_old_rules_and_legacy_query_contracts() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let database = temp.path().join("evidence.sqlite");
    let old = alert("old", now_ms());
    {
        let mut storage = Storage::open(&database).unwrap();
        storage.record_alert(&old).unwrap();
    }
    let connection = Connection::open(&database).unwrap();
    connection.execute_batch("DROP TABLE alert_handling_history; DROP TABLE alert_metadata; DROP TABLE directory_state; DROP TABLE console_settings; PRAGMA user_version=3;").unwrap();
    drop(connection);
    let storage = Storage::open(&database).unwrap();
    assert_eq!(storage.schema_version().unwrap(), 4);
    assert_eq!(
        storage.query_alerts(&AlertFilter::default()).unwrap(),
        vec![Alert {
            is_new: false,
            ..old
        }]
    );
    let entry = storage.console_alert_entry("old", true).unwrap();
    assert_eq!(entry.rule_version, 0);
    assert!(entry.rule_snapshot.is_none());
}

#[test]
fn settings_directory_exclusions_and_pause_intent_survive_reopen() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let root = temp.path().join("project");
    let child = root.join("excluded");
    std::fs::create_dir_all(&child).unwrap();
    let database = temp.path().join("evidence.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    storage.add_directory(&root, "manual", 1).unwrap();
    storage.add_directory(&child, "codex", 2).unwrap();
    storage.set_console_directory(&child, false).unwrap();
    storage.set_monitoring_paused(true).unwrap();
    let mut settings = storage
        .initialize_console_rules(RulesSettings::default())
        .unwrap();
    settings.bulk_enabled = false;
    settings.bulk_file_threshold = 27;
    settings.bulk_window_ms = 5500;
    let saved = storage.save_console_rules(settings.clone()).unwrap();
    assert_eq!(saved.version, 2);
    assert!(storage.save_console_rules(settings).is_err());
    drop(storage);
    let reopened = Storage::open(&database).unwrap();
    assert_eq!(reopened.console_rules().unwrap(), saved);
    assert!(reopened.monitoring_paused().unwrap());
    assert_eq!(
        reopened.active_directory_paths().unwrap(),
        vec![root.clone()]
    );
    assert_eq!(
        reopened.excluded_directory_paths().unwrap(),
        vec![child.clone()]
    );
    let directories = reopened.console_directories().unwrap();
    let excluded = directories
        .iter()
        .find(|entry| entry.path == child)
        .unwrap();
    assert!(!excluded.enabled);
    assert!(!excluded.effective);
    assert_eq!(excluded.excluded_by, Some(child));
    assert!(
        directories
            .iter()
            .find(|entry| entry.path == root)
            .unwrap()
            .effective
    );
}

#[test]
fn exclusions_override_enabled_descendants_and_disabled_rules_keep_activity() {
    let root = PathBuf::from("/private/tmp/synthetic-project");
    let excluded = root.join("private");
    let nested = excluded.join("nested");
    let mut rules = RuleEngine::new(
        vec![root.clone(), nested.clone()],
        RuleConfig {
            bulk_file_threshold: 2,
            ..Default::default()
        },
    )
    .unwrap();
    rules
        .replace_scope(vec![root.clone(), nested], vec![excluded.clone()])
        .unwrap();
    assert!(
        rules
            .process(&event(excluded.join("source.rs"), 1, 1000))
            .matched_directories
            .is_empty()
    );
    let mut settings = RulesSettings::from_config(&RuleConfig {
        bulk_file_threshold: 2,
        ..Default::default()
    });
    settings.bulk_enabled = false;
    settings.archive_command_enabled = false;
    settings.archive_output_enabled = false;
    rules.replace_settings(&settings).unwrap();
    for (i, name) in ["a.rs", "b.rs"].iter().enumerate() {
        let result = rules.process(&event(root.join(name), i as u64 + 2, 1010 + i as i64));
        assert_eq!(result.matched_directories, vec![root.clone()]);
        assert!(result.alerts.is_empty());
    }
    let mut command = event(root.join("a.rs"), 9, 1100);
    command.kind = EventKind::Exec;
    command.archive = Some(ArchiveCommand {
        tool: "gzip".into(),
        input_paths: vec![root.join("a.rs")],
        output_path: None,
        output_paths: vec![],
        cwd: None,
    });
    assert!(rules.process(&command).alerts.is_empty());
    settings.bulk_enabled = true;
    settings.archive_command_enabled = true;
    settings.archive_output_enabled = true;
    settings.version += 1;
    rules.replace_settings(&settings).unwrap();
    assert!(
        rules
            .process(&event(root.join("c.rs"), 10, 1200))
            .alerts
            .is_empty()
    );
    let first = rules
        .process(&event(root.join("d.rs"), 11, 1201))
        .alerts
        .remove(0);
    settings.version += 1;
    rules.replace_settings(&settings).unwrap();
    rules.process(&event(root.join("e.rs"), 12, 1210));
    let second = rules
        .process(&event(root.join("f.rs"), 13, 1211))
        .alerts
        .remove(0);
    assert_ne!(first.id, second.id);
    assert!(second.is_new);
    let mut excluded_output = event(excluded.join("snapshot.zip"), 14, 1212);
    excluded_output.kind = EventKind::Write;
    let result = rules.process(&excluded_output);
    assert!(result.alerts.is_empty());
    assert!(result.matched_directories.is_empty());
}

#[test]
fn new_evidence_reopens_workflow_keeps_notes_history_snapshot_and_notification() {
    let mut storage = Storage::open(":memory:").unwrap();
    let settings = storage
        .initialize_console_rules(RulesSettings::default())
        .unwrap();
    let mut source = alert("workflow", now_ms());
    storage.record_alert(&source).unwrap();
    let initial = storage.console_alert_entry(&source.id, false).unwrap();
    assert_eq!(initial.rule_version, 1);
    assert_eq!(initial.rule_snapshot, Some(settings.clone()));
    let handled = storage
        .console_alert_update(
            &source.id,
            Some(true),
            Some(true),
            Some("合成测试备注"),
            Some(initial.revision),
        )
        .unwrap();
    assert!(handled.processed && handled.is_read);
    assert_eq!(handled.handling_history.len(), 1);
    assert!(
        storage
            .console_alert_update(
                &source.id,
                None,
                None,
                Some("过期更新"),
                Some(initial.revision)
            )
            .is_err()
    );
    storage.record_alert(&source).unwrap();
    assert!(
        storage
            .console_alert_entry(&source.id, false)
            .unwrap()
            .processed
    );
    source.activity_count += 1;
    source.last_timestamp_ms += 1;
    source.is_new = false;
    storage.record_alert(&source).unwrap();
    let reopened = storage.console_alert_entry(&source.id, false).unwrap();
    assert!(!reopened.processed && !reopened.is_read);
    assert_eq!(reopened.note, "合成测试备注");
    assert_eq!(reopened.handling_history, handled.handling_history);
    assert!(reopened.revision > handled.revision);
    assert_eq!(reopened.rule_snapshot, Some(settings.clone()));
    assert_eq!(storage.pending_notification_summary().unwrap().count, 1);
    let mut updated = settings;
    updated.bulk_window_ms = 5000;
    storage.save_console_rules(updated).unwrap();
    assert_eq!(
        storage
            .console_alert_entry(&source.id, false)
            .unwrap()
            .rule_version,
        1
    );
}

#[test]
fn event_and_alert_pages_cover_all_rows_without_new_insert_or_merge_duplicates() {
    let mut storage = Storage::open(":memory:").unwrap();
    storage
        .initialize_console_rules(RulesSettings::default())
        .unwrap();
    let root = Path::new("/private/tmp/synthetic-project");
    for i in 0..237 {
        storage
            .record_event(
                &event(root.join(format!("source-{i}.rs")), i, 1000 + i as i64 % 3),
                &[root.to_path_buf()],
            )
            .unwrap();
        storage
            .record_alert(&alert(&format!("alert-{i}"), 1000 + i as i64))
            .unwrap();
    }
    let first = storage
        .console_events_page(&EventFilter::default(), None, None, false)
        .unwrap();
    assert_eq!(first.total, 237);
    assert_eq!(first.items.len(), 100);
    storage
        .record_event(
            &event(root.join("new.rs"), 300, 9999),
            &[root.to_path_buf()],
        )
        .unwrap();
    let mut ids: HashSet<i64> = first.items.iter().map(|item| item.id).collect();
    let mut cursor = first.next_cursor;
    while let Some(value) = cursor {
        let page = storage
            .console_events_page(&EventFilter::default(), Some(&value), None, false)
            .unwrap();
        assert_eq!(page.total, 237);
        for item in page.items {
            assert!(ids.insert(item.id));
        }
        cursor = page.next_cursor;
    }
    assert_eq!(ids.len(), 237);
    let first = storage
        .console_alerts_page(&AlertFilter::default(), None, None, None, None)
        .unwrap();
    assert_eq!(first.total, 237);
    let mut ids: HashSet<String> = first
        .items
        .iter()
        .map(|item| item.alert.id.clone())
        .collect();
    let mut changed = alert("alert-0", 50_000);
    changed.is_new = false;
    storage.record_alert(&changed).unwrap();
    storage.record_alert(&alert("new-alert", 60_000)).unwrap();
    let mut cursor = first.next_cursor;
    while let Some(value) = cursor {
        let page = storage
            .console_alerts_page(&AlertFilter::default(), Some(&value), None, None, None)
            .unwrap();
        assert_eq!(page.total, 237);
        for item in page.items {
            assert!(ids.insert(item.alert.id));
        }
        cursor = page.next_cursor;
    }
    assert_eq!(ids.len(), 237);
    let filtered = storage
        .console_events_page(&EventFilter::default(), None, Some("source-1.rs"), false)
        .unwrap();
    assert_eq!(filtered.items.len(), 1);
    assert!(
        storage
            .console_events_page(&EventFilter::default(), Some("bad cursor"), None, false)
            .is_err()
    );
}

#[test]
fn clear_and_retention_keep_configuration_cumulative_and_project_files() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let file = root.join("source.rs");
    std::fs::write(&file, "synthetic").unwrap();
    let mut storage = Storage::open(":memory:").unwrap();
    storage.add_directory(&root, "manual", 1).unwrap();
    let settings = storage
        .initialize_console_rules(RulesSettings::default())
        .unwrap();
    let old = now_ms() - 3 * 86_400_000;
    storage
        .record_event(&event(file.clone(), 1, old), std::slice::from_ref(&root))
        .unwrap();
    let source = alert("old", old);
    storage.record_alert(&source).unwrap();
    storage
        .console_alert_update(&source.id, Some(true), None, Some("匿名备注"), None)
        .unwrap();
    storage
        .record_notification(&NotificationRecord {
            alert_id: source.id.clone(),
            observed_timestamp_ms: old,
            outcome: NotificationOutcome::Sent,
            detail: None,
        })
        .unwrap();
    storage
        .record_health(&HealthRecord {
            source: None,
            observed_timestamp_ms: old,
            component: "synthetic".into(),
            code: "fixture".into(),
            state: "ready".into(),
            detail: None,
        })
        .unwrap();
    assert!(storage.set_console_retention(1, false).is_err());
    storage.set_console_retention(1, true).unwrap();
    assert_eq!(storage.console_counts().unwrap()["events"], 0);
    assert_eq!(storage.console_counts().unwrap()["handling_records"], 0);
    assert!(file.exists());
    storage
        .record_event(
            &event(file.clone(), 2, now_ms()),
            std::slice::from_ref(&root),
        )
        .unwrap();
    assert!(storage.clear_console_details(false).is_err());
    let cumulative = storage.cumulative_stats().unwrap();
    storage.clear_console_details(true).unwrap();
    assert_eq!(storage.cumulative_stats().unwrap(), cumulative);
    assert_eq!(storage.console_rules().unwrap(), settings);
    assert_eq!(storage.list_directories().unwrap().len(), 1);
    assert!(file.exists());
    storage.clear_cumulative_stats().unwrap();
    assert_eq!(storage.cumulative_stats().unwrap().events, 0);
}

#[test]
fn retention_preview_counts_cascades_and_failed_shrink_rolls_back_policy() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let database = temp.path().join("evidence.sqlite");
    let mut storage = Storage::open(&database).unwrap();
    let old = now_ms() - 3 * 86_400_000;
    let source = alert("expired", old);
    storage.record_alert(&source).unwrap();
    storage
        .console_alert_update(&source.id, Some(true), None, Some("合成记录"), None)
        .unwrap();
    storage
        .record_notification(&NotificationRecord {
            alert_id: source.id,
            observed_timestamp_ms: now_ms(),
            outcome: NotificationOutcome::Sent,
            detail: None,
        })
        .unwrap();
    let preview = storage
        .console_retention_preview(1, now_ms() - 86_400_000)
        .unwrap();
    assert_eq!(preview["counts"]["alerts"], 1);
    assert_eq!(preview["counts"]["notifications"], 1);
    assert_eq!(preview["counts"]["handling_records"], 1);
    let connection = Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TRIGGER synthetic_block_delete BEFORE DELETE ON alerts BEGIN SELECT RAISE(ABORT,'synthetic deletion fault'); END;").unwrap();
    assert!(storage.set_console_retention(1, true).is_err());
    assert_eq!(storage.retention_days().unwrap(), 30);
    assert_eq!(storage.console_counts().unwrap()["alerts"], 1);
    assert_eq!(storage.console_counts().unwrap()["handling_records"], 1);
}

#[test]
fn remove_preview_reports_expansion_without_changing_configuration() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let root = temp.path().join("project");
    let excluded = root.join("excluded");
    let nested = excluded.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let mut storage = Storage::open(":memory:").unwrap();
    for path in [&root, &excluded, &nested] {
        storage.add_directory(path, "manual", 1).unwrap();
    }
    storage.set_console_directory(&excluded, false).unwrap();
    let preview = storage.console_directory_remove_preview(&excluded).unwrap();
    assert_eq!(preview["coverage_may_expand"], true);
    assert_eq!(preview["removes_exclusion"], true);
    assert_eq!(preview["after"].as_array().unwrap().len(), 2);
    assert_eq!(storage.list_directories().unwrap().len(), 3);
    assert!(
        !storage
            .console_directories()
            .unwrap()
            .iter()
            .find(|entry| entry.path == nested)
            .unwrap()
            .effective
    );
    storage.remove_directory(&excluded).unwrap();
    assert!(
        storage
            .console_directories()
            .unwrap()
            .iter()
            .find(|entry| entry.path == nested)
            .unwrap()
            .effective
    );
}

#[test]
fn archive_association_details_do_not_mix_same_pid_from_different_source_runs() {
    let root = Path::new("/private/tmp/synthetic-project");
    let mut storage = Storage::open(":memory:").unwrap();
    let first = event(root.join("source.rs"), 1, 1000);
    let mut second = first.clone();
    second.source_run_id = "another-synthetic-run".into();
    storage.record_event(&first, &[root.to_path_buf()]).unwrap();
    storage
        .record_event(&second, &[root.to_path_buf()])
        .unwrap();
    let mut source = alert("synthetic-console:100:v7:archive_output:1", 1000);
    source.rule = AlertRule::ArchiveOutput;
    storage.record_alert(&source).unwrap();
    let page = storage
        .console_events_page(&EventFilter::default(), None, None, true)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].event.source_run_id, "synthetic-console");
    let detail = storage.console_alert_entry(&source.id, true).unwrap();
    assert_eq!(detail.events.unwrap().len(), 1);
    assert_eq!(
        storage.console_event_detail(page.items[0].id).unwrap()["alerts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn stale_cursor_and_invalid_rule_settings_are_rejected() {
    let mut storage = Storage::open(":memory:").unwrap();
    let root = Path::new("/private/tmp/synthetic-project");
    for i in 0..101 {
        storage
            .record_event(
                &event(root.join(format!("source-{i}.rs")), i, now_ms()),
                &[root.to_path_buf()],
            )
            .unwrap();
    }
    let first = storage
        .console_events_page(&EventFilter::default(), None, None, false)
        .unwrap();
    let cursor = first.next_cursor.unwrap();
    assert!(
        storage
            .console_events_page(
                &EventFilter::default(),
                Some(&cursor),
                Some("different search"),
                false
            )
            .is_err()
    );
    storage.clear_console_details(true).unwrap();
    assert!(
        storage
            .console_events_page(&EventFilter::default(), Some(&cursor), None, false)
            .is_err()
    );
    let invalid = RulesSettings {
        bulk_file_threshold: 513,
        ..Default::default()
    };
    assert!(storage.save_console_rules(invalid).is_err());
    let invalid = RulesSettings {
        bulk_window_ms: 99,
        ..Default::default()
    };
    assert!(storage.save_console_rules(invalid).is_err());
}

#[test]
fn independent_engine_instances_cannot_overwrite_the_same_source_alert() {
    let root = PathBuf::from("/private/tmp/synthetic-project");
    let build = || {
        RuleEngine::new(
            vec![root.clone()],
            RuleConfig {
                bulk_file_threshold: 1,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let mut first = build();
    let mut second = build();
    let evidence = event(root.join("source.rs"), 1, 1000);
    assert_ne!(
        first.process(&evidence).alerts[0].id,
        second.process(&evidence).alerts[0].id
    );
}

#[test]
fn response_byte_budget_preserves_complete_pagination_for_large_records() {
    let root = Path::new("/private/tmp/synthetic-project");
    let mut storage = Storage::open(":memory:").unwrap();
    for i in 0..25 {
        let mut large = event(root.join(format!("source-{i}.rs")), i, 1000);
        large.source_run_id = format!("synthetic-{i}-{}", "synthetic".repeat(12_000));
        storage.record_event(&large, &[root.to_path_buf()]).unwrap();
    }
    let first = storage
        .console_events_page(&EventFilter::default(), None, None, false)
        .unwrap();
    assert!(first.items.len() < 25);
    assert!(serde_json::to_vec(&first).unwrap().len() < 2 * 1024 * 1024);
    let mut count = first.items.len();
    let mut cursor = first.next_cursor;
    while let Some(value) = cursor {
        let next = storage
            .console_events_page(&EventFilter::default(), Some(&value), None, false)
            .unwrap();
        count += next.items.len();
        cursor = next.next_cursor;
    }
    assert_eq!(count, 25);
    let note = "synthetic".repeat(220);
    for i in 0..8 {
        let id = format!("synthetic-alert-{i}");
        storage.record_alert(&alert(&id, 1000)).unwrap();
        for _ in 0..100 {
            storage
                .console_alert_update(&id, None, None, Some(&note), None)
                .unwrap();
        }
    }
    let first = storage
        .console_alerts_page(&AlertFilter::default(), None, None, None, None)
        .unwrap();
    assert!(first.items.len() < 8);
    assert!(serde_json::to_vec(&first).unwrap().len() < 2 * 1024 * 1024);
    let mut count = first.items.len();
    let mut cursor = first.next_cursor;
    while let Some(value) = cursor {
        let next = storage
            .console_alerts_page(&AlertFilter::default(), Some(&value), None, None, None)
            .unwrap();
        count += next.items.len();
        cursor = next.next_cursor;
    }
    assert_eq!(count, 8);
}
