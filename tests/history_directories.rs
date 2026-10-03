use codeperimeter::history::{
    DirectoryStatus, DiscoveryGapKind, HistoryOptions, HistorySource, MAX_HISTORY_LINE_BYTES,
    discover, manual_directories,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn fixture(name: &str, first: &Path, second: &Path) -> String {
    let source = match name {
        "codex" => include_str!("fixtures/history/codex.jsonl"),
        "claude" => include_str!("fixtures/history/claude.jsonl"),
        _ => unreachable!(),
    };
    let escape = |path: &Path| {
        let encoded = serde_json::to_string(&path.to_string_lossy()).unwrap();
        encoded[1..encoded.len() - 1].to_owned()
    };
    source
        .replace("@PROJECT_A@", &escape(first))
        .replace("@PROJECT_B@", &escape(second))
}

fn write(path: &Path, contents: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn codex_initial_and_turn_roots_keep_sources_and_skip_contents() {
    let temp = TempDir::new().unwrap();
    let first = temp.path().join("project A");
    let second = temp.path().join("project-B");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let root = temp.path().join("codex");
    write(
        &root.join("sessions/2026/10/03/session.jsonl"),
        fixture("codex", &first, &second),
    );
    fs::create_dir_all(root.join("archived_sessions")).unwrap();
    let report = discover(&HistoryOptions {
        codex_home: Some(root),
        claude_home: None,
    });
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(report.counts.files_scanned, 1);
    assert_eq!(report.counts.matched_metadata_records, 3);
    assert_eq!(report.counts.ignored_records, 1);
    assert!(
        report
            .candidates
            .iter()
            .all(|candidate| candidate.status == DirectoryStatus::Available)
    );
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.canonical_path == Some(fs::canonicalize(&second).unwrap()))
        .unwrap();
    assert!(
        candidate
            .origins
            .iter()
            .any(|origin| origin.field == "runtime_workspace_roots")
    );
    let turn_origin = candidate
        .origins
        .iter()
        .find(|origin| origin.record_type.as_deref() == Some("turn_context"))
        .unwrap();
    assert_eq!(turn_origin.occurrences, 2);
    assert_eq!(turn_origin.version.as_deref(), Some("0.114.0"));
    let saved = serde_json::to_string(&report).unwrap();
    assert!(!saved.contains("PRIVATE_CONTENT"));
    assert!(!saved.contains("/unrelated/body/path"));
    assert!(
        report
            .versions
            .iter()
            .all(|version| !version.compatibility_validated)
    );
}

#[test]
fn codex_compressed_archives_and_active_files_are_deduplicated() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let root = temp.path().join("codex");
    let record = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"cwd":project,"cli_version":"0.114.0"}})
    );
    write(&root.join("sessions/live.jsonl"), &record);
    let compressed = zstd::stream::encode_all(record.as_bytes(), 1).unwrap();
    write(&root.join("archived_sessions/old.jsonl.zst"), compressed);
    let report = discover(&HistoryOptions {
        codex_home: Some(root),
        claude_home: None,
    });
    assert_eq!(report.counts.files_scanned, 2);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].origins.len(), 2);
}

#[test]
fn claude_reads_actual_cwd_and_never_decodes_project_folder_names() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("actual-project");
    fs::create_dir(&project).unwrap();
    let root = temp.path().join("claude");
    write(
        &root.join("projects/-fake-path-with-hash/session.jsonl"),
        fixture("claude", &project, &project),
    );
    write(
        &root.join("projects/-fake-only-name/without-cwd.jsonl"),
        "{\"type\":\"user\",\"version\":\"2.1.247\"}\n",
    );
    let report = discover(&HistoryOptions {
        codex_home: None,
        claude_home: Some(root),
    });
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(report.counts.unresolved_directories, 1);
    assert!(!report.candidates.iter().any(|candidate| {
        candidate
            .raw_paths
            .iter()
            .any(|path| path.to_string_lossy().contains("fake-path"))
    }));
    assert!(
        report
            .gaps
            .iter()
            .any(|gap| gap.kind == DiscoveryGapKind::MissingDirectory)
    );
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("PRIVATE_CONTENT")
    );
}

#[test]
fn bad_unknown_oversized_and_partial_records_do_not_hide_later_metadata() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let root = temp.path().join("codex");
    let contents = format!(
        "not-json\n{{\"type\":\"new_meta\",\"payload\":{{}}}}\n{}\n{}\n{{\"type\":\"turn_context\",\"payload\":",
        "x".repeat(MAX_HISTORY_LINE_BYTES + 32),
        json!({"type":"session_meta","payload":{"cwd":project}})
    );
    write(&root.join("sessions/session.jsonl"), contents);
    fs::create_dir_all(root.join("archived_sessions")).unwrap();
    let report = discover(&HistoryOptions {
        codex_home: Some(root),
        claude_home: None,
    });
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.counts.malformed_records, 1);
    assert_eq!(report.counts.unsupported_records, 1);
    assert_eq!(report.counts.oversized_records, 1);
    assert_eq!(report.counts.incomplete_records, 1);
    assert!(
        report
            .gaps
            .iter()
            .any(|gap| gap.kind == DiscoveryGapKind::MissingVersion)
    );
}

#[test]
fn missing_relative_and_file_candidates_preserve_parent_and_child() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    let child = project.join("child");
    fs::create_dir_all(&child).unwrap();
    let file = temp.path().join("regular-file");
    fs::write(&file, "synthetic").unwrap();
    let missing = temp.path().join("missing");
    let report = manual_directories(&[
        project.clone(),
        child,
        project.join("."),
        missing,
        "relative".into(),
        file,
    ]);
    assert_eq!(report.candidates.len(), 5);
    assert_eq!(report.counts.missing_directories, 1);
    assert_eq!(report.counts.unresolved_directories, 1);
    assert_eq!(report.counts.non_directories, 1);
    // 保留父子项目，不能凭共同父目录扩大范围。
    assert_eq!(
        report
            .candidates
            .iter()
            .filter(|candidate| candidate.status == DirectoryStatus::Available)
            .count(),
        2
    );
    assert!(report.candidates.iter().all(|candidate| {
        candidate
            .origins
            .iter()
            .all(|origin| origin.source == HistorySource::Manual)
    }));
}

#[cfg(unix)]
#[test]
fn directory_aliases_are_deduplicated_without_losing_raw_paths() {
    use std::os::unix::fs::symlink;
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let alias = temp.path().join("alias");
    symlink(&project, &alias).unwrap();
    let report = manual_directories(&[project, alias]);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].raw_paths.len(), 2);
    assert_eq!(report.candidates[0].origins[0].occurrences, 2);
}

#[test]
fn missing_sources_and_corrupt_compression_are_visible() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("codex");
    write(&root.join("sessions/bad.jsonl.zst"), "not-zstd");
    let report = discover(&HistoryOptions {
        codex_home: Some(root),
        claude_home: Some(temp.path().join("absent-claude")),
    });
    assert!(
        report
            .gaps
            .iter()
            .any(|gap| gap.kind == DiscoveryGapKind::MissingSource)
    );
    assert_eq!(report.counts.io_errors, 1);
}

#[cfg(unix)]
#[test]
fn unreadable_files_and_symlink_history_are_reported() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("codex");
    let file = root.join("sessions/unreadable.jsonl");
    write(&file, "{\"type\":\"session_meta\",\"payload\":{}}\n");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    symlink(&file, root.join("sessions/symlink.jsonl")).unwrap();
    let report = discover(&HistoryOptions {
        codex_home: Some(root),
        claude_home: None,
    });
    if unsafe { libc::geteuid() } != 0 {
        assert_eq!(report.counts.io_errors, 1);
    }
    assert!(
        report
            .gaps
            .iter()
            .any(|gap| gap.kind == DiscoveryGapKind::SkippedSymlink)
    );
    fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
}
