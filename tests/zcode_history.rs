use codeperimeter::history::{
    DirectoryStatus, DiscoveryGapKind, DiscoveryReport, HistoryOptions, HistorySource,
    MAX_HISTORY_LINE_BYTES, discover,
};
use rusqlite::{Connection, params};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

// 使用本机观察到的 session 列形状，内容全部为合成数据。
const SESSION_SCHEMA: &str = "CREATE TABLE session (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    workspace_id TEXT,
    directory TEXT NOT NULL,
    path TEXT,
    title TEXT NOT NULL,
    version TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL
);";

fn options(database: &Path) -> HistoryOptions {
    HistoryOptions {
        codex_home: None,
        claude_home: None,
        zcode_db: Some(database.into()),
    }
}

fn database(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(SESSION_SCHEMA).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE message (id TEXT, data TEXT);
            CREATE TABLE part (id TEXT, data TEXT);",
        )
        .unwrap();
    connection
}

fn insert_directory(connection: &Connection, id: &str, path: Option<&Path>) {
    connection
        .execute(
            "INSERT INTO session(id,directory,title,path,version,project_id,time_created,time_updated)
                VALUES (?1,?2,'PRIVATE_TITLE','PRIVATE_PATH','PRIVATE_VERSION','anonymous-project',1,1)",
            params![id, path.map(|path| path.to_str().unwrap())],
        )
        .unwrap();
}

fn has_gap(report: &DiscoveryReport, kind: DiscoveryGapKind) -> bool {
    report.gaps.iter().any(|gap| {
        gap.source == HistorySource::Zcode && gap.kind == kind && gap.first_line.is_none()
    })
}

#[test]
fn defaults_include_zcode_and_old_options_remain_readable() {
    let defaults = HistoryOptions::defaults(Path::new("/anonymous-home"));
    assert_eq!(
        defaults.zcode_db,
        Some(PathBuf::from("/anonymous-home/.zcode/cli/db/db.sqlite"))
    );
    let old: HistoryOptions =
        serde_json::from_str(r#"{"codex_home":null,"claude_home":null}"#).unwrap();
    assert!(old.zcode_db.is_none());
}

#[test]
fn extracts_only_directory_and_does_not_persist_private_metadata() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let path = temp.path().join("db.sqlite");
    let connection = database(&path);
    insert_directory(&connection, "PRIVATE_SESSION_ID", Some(&project));
    connection
        .execute_batch(
            "INSERT INTO message VALUES ('PRIVATE_MESSAGE_ID','PRIVATE_MESSAGE_BODY');
            INSERT INTO part VALUES ('PRIVATE_PART_ID','PRIVATE_TOOL_BODY');",
        )
        .unwrap();
    drop(connection);
    let before = fs::read(&path).unwrap();
    let report = discover(&options(&path));
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.counts.files_scanned, 1);
    assert_eq!(report.counts.records_scanned, 1);
    assert_eq!(report.counts.matched_metadata_records, 1);
    assert_eq!(report.candidates[0].status, DirectoryStatus::Available);
    let origin = &report.candidates[0].origins[0];
    assert_eq!(origin.source, HistorySource::Zcode);
    assert_eq!(origin.record_type.as_deref(), Some("session"));
    assert_eq!(origin.field, "directory");
    assert!(origin.line.is_none());
    assert!(origin.version.is_none());
    assert!(report.versions.is_empty());
    assert!(has_gap(
        &report,
        DiscoveryGapKind::DirectoryChangesNotValidated
    ));
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_"));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn committed_wal_rows_are_visible_without_checkpoint_or_database_writes() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let path = temp.path().join("db.sqlite");
    let connection = database(&path);
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    let database_before = fs::read(&path).unwrap();
    insert_directory(&connection, "anonymous-session", Some(&project));
    let wal_path = path.with_file_name("db.sqlite-wal");
    let wal_before = fs::read(&wal_path).unwrap();
    assert!(!wal_before.is_empty());
    let report = discover(&options(&path));
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].status, DirectoryStatus::Available);
    assert_eq!(fs::read(&path).unwrap(), database_before);
    assert_eq!(fs::read(&wal_path).unwrap(), wal_before);
    // 发现过程结束后，原应用连接仍然能正常追加会话。
    insert_directory(&connection, "anonymous-session-2", Some(&project));
    assert_eq!(
        discover(&options(&path)).candidates[0].origins[0].occurrences,
        2
    );
}

#[test]
fn null_empty_relative_missing_files_and_duplicates_keep_existing_contract() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let file = temp.path().join("file");
    fs::write(&file, "anonymous").unwrap();
    let path = temp.path().join("db.sqlite");
    // NULL 行模拟目录字段约束改变的 schema，仍必须明确报告缺失。
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(&SESSION_SCHEMA.replace("directory TEXT NOT NULL", "directory TEXT"))
        .unwrap();
    for (index, directory) in [
        None,
        Some(Path::new("")),
        Some(Path::new("relative-project")),
        Some(temp.path().join("missing").as_path()),
        Some(file.as_path()),
        Some(project.as_path()),
        Some(project.join(".").as_path()),
    ]
    .into_iter()
    .enumerate()
    {
        insert_directory(&connection, &format!("session-{index}"), directory);
    }
    let report = discover(&options(&path));
    assert_eq!(report.candidates.len(), 4);
    assert_eq!(report.counts.records_scanned, 7);
    assert_eq!(report.counts.missing_directories, 1);
    assert_eq!(report.counts.unresolved_directories, 1);
    assert_eq!(report.counts.non_directories, 1);
    let gap = report
        .gaps
        .iter()
        .find(|gap| gap.kind == DiscoveryGapKind::MissingDirectory)
        .unwrap();
    assert_eq!(gap.count, 2);
    let available = report
        .candidates
        .iter()
        .find(|candidate| candidate.status == DirectoryStatus::Available)
        .unwrap();
    // Path 比较会折叠末尾的“.”，同一写法不重复保存。
    assert_eq!(available.raw_paths.len(), 1);
    assert_eq!(available.origins[0].occurrences, 2);
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_"));
}

#[test]
fn same_directory_across_sources_retains_all_origins() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let path = temp.path().join("db.sqlite");
    insert_directory(&database(&path), "session", Some(&project));
    let codex = temp.path().join("codex");
    fs::create_dir_all(codex.join("sessions")).unwrap();
    fs::create_dir_all(codex.join("archived_sessions")).unwrap();
    fs::write(
        codex.join("sessions/session.jsonl"),
        format!(
            "{}\n",
            json!({"type":"session_meta","payload":{"cwd":project,"cli_version":"test"}})
        ),
    )
    .unwrap();
    let claude = temp.path().join("claude");
    fs::create_dir_all(claude.join("projects/anonymous-project")).unwrap();
    fs::write(
        claude.join("projects/anonymous-project/session.jsonl"),
        format!(
            "{}\n",
            json!({"type":"user","cwd":project,"version":"test"})
        ),
    )
    .unwrap();
    let report = discover(&HistoryOptions {
        codex_home: Some(codex),
        claude_home: Some(claude),
        zcode_db: Some(path),
    });
    assert_eq!(report.candidates.len(), 1);
    let sources: Vec<_> = report.candidates[0]
        .origins
        .iter()
        .map(|origin| origin.source)
        .collect();
    assert_eq!(
        sources,
        vec![
            HistorySource::Codex,
            HistorySource::ClaudeCode,
            HistorySource::Zcode
        ]
    );
}

#[test]
fn unsupported_schema_never_falls_back_to_path_views_or_generated_directory() {
    let temp = TempDir::new().unwrap();
    let schemas = [
        "CREATE TABLE other(directory TEXT);",
        "CREATE TABLE session(path TEXT); INSERT INTO session VALUES ('PRIVATE_PATH');",
        "CREATE TABLE session(directory INTEGER); INSERT INTO session VALUES (42);",
        "CREATE TABLE message(directory TEXT); CREATE VIEW session AS SELECT directory FROM message;",
        "CREATE TABLE session(title TEXT, directory TEXT GENERATED ALWAYS AS (title) VIRTUAL);",
    ];
    for (index, schema) in schemas.into_iter().enumerate() {
        let path = temp.path().join(format!("db-{index}.sqlite"));
        Connection::open(&path)
            .unwrap()
            .execute_batch(schema)
            .unwrap();
        let report = discover(&options(&path));
        assert!(report.candidates.is_empty());
        assert!(has_gap(&report, DiscoveryGapKind::UnsupportedFormat));
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_"));
    }
}

#[test]
fn invalid_and_oversized_directory_rows_do_not_hide_later_metadata() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let path = temp.path().join("db.sqlite");
    let connection = database(&path);
    connection
        .execute_batch(
            "INSERT INTO session(id,directory,project_id,title,version,time_created,time_updated)
                VALUES ('blob',x'505249564154455F','anonymous','PRIVATE_TITLE','test',1,1);
            INSERT INTO session(id,directory,project_id,title,version,time_created,time_updated)
                VALUES ('utf8',CAST(x'80' AS TEXT),'anonymous','PRIVATE_TITLE','test',1,1);",
        )
        .unwrap();
    insert_directory(
        &connection,
        "oversized",
        Some(Path::new(&"x".repeat(MAX_HISTORY_LINE_BYTES + 1))),
    );
    insert_directory(&connection, "valid", Some(&project));
    let report = discover(&options(&path));
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.counts.malformed_records, 2);
    assert_eq!(report.counts.oversized_records, 1);
    assert!(has_gap(&report, DiscoveryGapKind::MalformedRecord));
    assert!(has_gap(&report, DiscoveryGapKind::RecordTooLong));
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_"));
}

#[test]
fn missing_unreadable_and_corrupt_sources_are_explicit() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing.sqlite");
    let report = discover(&options(&missing));
    assert!(has_gap(&report, DiscoveryGapKind::MissingSource));
    assert!(!missing.exists());
    let report = discover(&options(temp.path()));
    assert!(has_gap(&report, DiscoveryGapKind::IoError));
    let corrupt = temp.path().join("corrupt.sqlite");
    fs::write(&corrupt, "PRIVATE_BODY is not sqlite").unwrap();
    let report = discover(&options(&corrupt));
    assert!(has_gap(&report, DiscoveryGapKind::IoError));
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_"));
}

#[test]
fn lock_wait_is_bounded_and_other_sources_still_return_candidates() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("db.sqlite");
    let connection = database(&path);
    connection.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let codex = temp.path().join("codex");
    fs::create_dir_all(codex.join("sessions")).unwrap();
    fs::create_dir_all(codex.join("archived_sessions")).unwrap();
    fs::write(
        codex.join("sessions/session.jsonl"),
        format!(
            "{}\n",
            json!({"type":"session_meta","payload":{"cwd":project,"cli_version":"test"}})
        ),
    )
    .unwrap();
    let started = Instant::now();
    let report = discover(&HistoryOptions {
        codex_home: Some(codex),
        claude_home: None,
        zcode_db: Some(path),
    });
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(has_gap(&report, DiscoveryGapKind::ReadTimedOut));
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].origins[0].source, HistorySource::Codex);
    assert_eq!(report.counts.io_errors, 1);
}

#[cfg(unix)]
#[test]
fn unreadable_database_and_directory_aliases_are_reported() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("db.sqlite");
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let alias = temp.path().join("alias");
    symlink(&project, &alias).unwrap();
    let connection = database(&path);
    insert_directory(&connection, "first", Some(&project));
    insert_directory(&connection, "second", Some(&alias));
    drop(connection);
    let report = discover(&options(&path));
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].raw_paths.len(), 2);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if unsafe { libc::geteuid() } != 0 {
        let report = discover(&options(&path));
        assert!(has_gap(&report, DiscoveryGapKind::IoError));
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(unix)]
#[test]
fn inaccessible_project_is_preserved_as_an_unavailable_candidate() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let temp = TempDir::new().unwrap();
    let parent = temp.path().join("inaccessible");
    let project = parent.join("project");
    fs::create_dir_all(&project).unwrap();
    let path = temp.path().join("db.sqlite");
    insert_directory(&database(&path), "session", Some(&project));
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o000)).unwrap();
    let report = discover(&options(&path));
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].status, DirectoryStatus::Inaccessible);
    assert_eq!(report.counts.inaccessible_directories, 1);
}
