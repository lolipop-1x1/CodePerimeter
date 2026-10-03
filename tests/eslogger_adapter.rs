use codeperimeter::eslogger::{EsloggerAdapter, MAX_LINE_BYTES, parse_line};
use codeperimeter::model::EventKind;
use serde_json::{Value, json};
use std::path::PathBuf;

fn rows() -> Vec<Value> {
    include_str!("fixtures/eslogger/events.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn parse(value: &Value) -> codeperimeter::eslogger::ParseOutcome {
    parse_line(&value.to_string(), "synthetic-run", 1234)
}

#[test]
fn preserves_nine_event_types_and_execution_generation() {
    let mut adapter = EsloggerAdapter::new("synthetic-run");
    let mut parsed = Vec::new();
    for value in rows() {
        let outcome = adapter.parse_line(&value.to_string(), 1234);
        assert!(outcome.issues.is_empty(), "{:?}", outcome.issues);
        parsed.push(outcome.event.unwrap());
    }
    assert_eq!(
        parsed.iter().map(|event| event.kind).collect::<Vec<_>>(),
        vec![
            EventKind::Open,
            EventKind::Mmap,
            EventKind::Exec,
            EventKind::Fork,
            EventKind::Create,
            EventKind::Write,
            EventKind::Close,
            EventKind::Rename,
            EventKind::Exit
        ]
    );
    assert_eq!(parsed[0].source_timestamp_ms, Some(1767225600123));
    assert_eq!(parsed[0].received_timestamp_ms, 1234);
    assert_eq!(parsed[0].file.as_ref().unwrap().inode, Some(10));
    assert_eq!(parsed[2].process.pid_version, Some(201));
    assert_eq!(parsed[3].process.pid, 101);
    assert_eq!(parsed[3].process.ppid, Some(100));
    assert_eq!(parsed[6].modified, Some(true));
    assert_eq!(
        parsed[7].destination,
        Some(PathBuf::from("/private/tmp/synthetic-snapshot.tar"))
    );
    assert_eq!(adapter.health().parsed_events, 9);
}

#[test]
fn uses_kernel_read_flags_and_actual_mapping_protection() {
    let mut value = rows().remove(0);
    for (flags, readable) in [(0, false), (1, true), (2, false), (3, true)] {
        value["event"]["open"]["fflag"] = json!(flags);
        assert_eq!(
            parse(&value).event.unwrap().file.unwrap().readable,
            Some(readable)
        );
    }
    value["event"]["open"]["fflag"] = Value::Null;
    let outcome = parse(&value);
    assert_eq!(outcome.event.unwrap().file.unwrap().readable, None);
    assert!(
        outcome
            .issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("event.open.fflag"))
    );
    let mut mmap = rows().remove(1);
    mmap["event"]["mmap"]["protection"] = json!(2);
    mmap["event"]["mmap"]["max_protection"] = json!(7);
    assert_eq!(
        parse(&mmap).event.unwrap().file.unwrap().readable,
        Some(false)
    );
    mmap["event"]["mmap"]["protection"] = json!(5);
    assert_eq!(
        parse(&mmap).event.unwrap().file.unwrap().readable,
        Some(true)
    );
    value["event"]["open"]["fflag"] = json!(1);
    value["action"]["result"]["result"]["flags"] = json!(0);
    assert_eq!(
        parse(&value).event.unwrap().file.unwrap().readable,
        Some(false)
    );
}

#[test]
fn reports_missing_fields_truncation_and_non_regular_files() {
    let mut value = rows().remove(0);
    value["process"]["audit_token"]["pidversion"] = Value::Null;
    value["event"]["open"]["file"]["path_truncated"] = json!(true);
    value["event"]["open"]["file"]["stat"]["st_mode"] = json!(16877);
    value["time"] = json!("not-a-time");
    value["global_seq_num"] = Value::Null;
    let outcome = parse(&value);
    let event = outcome.event.unwrap();
    assert_eq!(event.process.pid_version, None);
    assert_eq!(event.source_timestamp_ms, None);
    assert_eq!(event.global_seq, None);
    assert_eq!(event.file.as_ref().unwrap().is_regular, Some(false));
    assert!(event.file.unwrap().path_truncated);
    for code in [
        "process_generation_unavailable",
        "path_truncated",
        "sequence_unavailable",
        "missing_or_invalid_field",
    ] {
        assert!(outcome.issues.iter().any(|issue| issue.code == code));
    }
    value["process"]["audit_token"]["pid"] = Value::Null;
    assert!(parse(&value).event.is_none());
}

#[test]
fn tracks_sequences_before_filtering_and_distinguishes_reset() {
    let mut adapter = EsloggerAdapter::new("first-run");
    let mut value = rows().remove(0);
    assert!(adapter.parse_line(&value.to_string(), 1).issues.is_empty());
    value["global_seq_num"] = json!(4);
    value["seq_num"] = json!(3);
    let outcome = adapter.parse_line(&value.to_string(), 2);
    let gaps: Vec<_> = outcome
        .issues
        .iter()
        .filter(|issue| issue.code == "sequence_gap")
        .collect();
    assert_eq!(gaps.len(), 2);
    assert_eq!(gaps[0].missing_events, Some(3));
    assert_eq!(gaps[1].missing_events, Some(2));
    assert!(
        adapter
            .parse_line(&value.to_string(), 3)
            .issues
            .iter()
            .any(|issue| issue.code == "sequence_regression")
    );
    let mut fresh = EsloggerAdapter::new("second-run");
    let fresh_outcome = fresh.parse_line(&value.to_string(), 3);
    assert!(fresh_outcome.issues.is_empty());
    let first_event = outcome.event.unwrap();
    let second_event = fresh_outcome.event.unwrap();
    assert_ne!(
        first_event.process.key(&first_event.source_run_id),
        second_event.process.key(&second_event.source_run_id)
    );
}

#[test]
fn unknown_events_advance_global_sequence_without_false_gap() {
    let mut adapter = EsloggerAdapter::new("synthetic-run");
    let mut value = rows().remove(0);
    adapter.parse_line(&value.to_string(), 1);
    value["event_type"] = json!(55);
    value["global_seq_num"] = json!(1);
    value["seq_num"] = json!(0);
    let skipped = adapter.parse_line(&value.to_string(), 2);
    assert!(skipped.event.is_none());
    assert!(
        skipped
            .issues
            .iter()
            .any(|issue| issue.code == "unsupported_event")
    );
    value["event_type"] = json!(10);
    value["global_seq_num"] = json!(2);
    value["seq_num"] = json!(1);
    assert!(adapter.parse_line(&value.to_string(), 3).issues.is_empty());
}

fn archive_exec(tool: &str, args: &[&str]) -> Value {
    let mut value = rows().remove(2);
    value["event"]["exec"]["target"]["executable"]["path"] = json!(format!("/usr/bin/{tool}"));
    value["event"]["exec"]["cwd"] =
        json!({"path":"/private/tmp/synthetic-project", "path_truncated":false});
    value["event"]["exec"]["args"] = json!(args);
    value
}

#[test]
fn extracts_only_archive_paths_and_handles_tar_directory_changes() {
    let value = archive_exec(
        "tar",
        &[
            "tar",
            "-czf",
            "/private/tmp/snapshot.tar.gz",
            "-C",
            "src",
            "a.rs",
            "../Cargo.toml",
        ],
    );
    let outcome = parse(&value);
    assert!(outcome.issues.is_empty());
    let archive = outcome.event.unwrap().archive.unwrap();
    assert_eq!(
        archive.output_path,
        Some(PathBuf::from("/private/tmp/snapshot.tar.gz"))
    );
    assert_eq!(
        archive.input_paths,
        vec![
            PathBuf::from("/private/tmp/synthetic-project/src/a.rs"),
            PathBuf::from("/private/tmp/synthetic-project/Cargo.toml")
        ]
    );
    let value = archive_exec("zip", &["zip", "-rq", "snapshot.zip", "src/a.rs"]);
    let archive = parse(&value).event.unwrap().archive.unwrap();
    assert_eq!(
        archive.output_path,
        Some(PathBuf::from("/private/tmp/synthetic-project/snapshot.zip"))
    );
    assert_eq!(archive.input_paths.len(), 1);
    let value = archive_exec("tar", &["tar", "-xf", "snapshot.tar"]);
    assert!(parse(&value).event.unwrap().archive.is_none());
}

#[test]
fn unsupported_archive_arguments_cannot_become_paths_or_leak() {
    let mut value = archive_exec(
        "zip",
        &["zip", "-P", "synthetic-secret", "snapshot.zip", "src"],
    );
    value["event"]["exec"]["env"] = json!(["SYNTHETIC_TOKEN=private-marker"]);
    let outcome = parse(&value);
    assert!(outcome.event.as_ref().unwrap().archive.is_none());
    assert!(
        outcome
            .issues
            .iter()
            .any(|issue| issue.code == "archive_arguments_incomplete")
    );
    let serialized = serde_json::to_string(&outcome).unwrap();
    assert!(!serialized.contains("synthetic-secret"));
    assert!(!serialized.contains("private-marker"));
    assert!(!serialized.contains("env"));
    let value = archive_exec(
        "tar",
        &[
            "tar",
            "-cf",
            "snapshot.tar",
            "--files-from",
            "synthetic-secret-list",
        ],
    );
    assert!(parse(&value).event.unwrap().archive.is_none());
}

#[test]
fn relative_archive_paths_require_non_truncated_cwd() {
    let mut value = archive_exec("tar", &["tar", "-cf", "/private/tmp/snapshot.tar", "src"]);
    value["event"]["exec"]["cwd"]["path_truncated"] = json!(true);
    assert!(parse(&value).event.unwrap().archive.is_none());
    value["event"]["exec"]["args"] = json!([
        "tar",
        "-cf",
        "/private/tmp/snapshot.tar",
        "/private/tmp/synthetic-project/src"
    ]);
    let archive = parse(&value).event.unwrap().archive.unwrap();
    assert!(archive.cwd.is_none());
}

#[test]
fn creates_new_destination_and_rejects_invalid_filename() {
    let mut value = rows().remove(7);
    value["event_type"] = json!(13);
    value["event"]["create"] = value["event"]["rename"].clone();
    let file = parse(&value).event.unwrap().file.unwrap();
    assert_eq!(
        file.path,
        PathBuf::from("/private/tmp/synthetic-snapshot.tar")
    );
    assert_eq!(file.inode, None);
    value["event"]["create"]["destination"]["new_path"]["filename"] = json!("../escape.tar");
    assert!(parse(&value).event.is_none());
}

#[test]
fn malformed_unknown_schema_and_auth_are_visible_without_raw_input() {
    let malformed = parse_line("{\"synthetic-private-value\":", "synthetic-run", 1);
    assert!(malformed.event.is_none());
    assert_eq!(malformed.issues[0].code, "malformed_json");
    assert!(
        !serde_json::to_string(&malformed)
            .unwrap()
            .contains("synthetic-private-value")
    );
    let too_long = parse_line(&"x".repeat(MAX_LINE_BYTES + 1), "synthetic-run", 1);
    assert_eq!(too_long.issues[0].code, "oversized_line");
    let mut value = rows().remove(0);
    value["schema_version"] = json!(2);
    assert_eq!(parse(&value).issues[0].code, "unsupported_schema");
    value["schema_version"] = json!(1);
    value["action_type"] = json!(0);
    assert_eq!(parse(&value).issues[0].code, "unexpected_action");
}
