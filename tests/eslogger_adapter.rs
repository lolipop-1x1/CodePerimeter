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
        &[
            "zip",
            "--unsupported",
            "synthetic-secret",
            "snapshot.zip",
            "src",
        ],
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
fn all_fourteen_names_parse_direct_inputs_and_their_output_shapes() {
    let cases: Vec<(&str, Vec<&str>, usize, usize)> = vec![
        (
            "tar",
            vec!["-czf", "snapshot.tar.gz", "src/a.rs", "src/b.rs"],
            2,
            1,
        ),
        (
            "bsdtar",
            vec!["-cf", "snapshot.tar", "src/a.rs", "src/b.rs"],
            2,
            1,
        ),
        (
            "gtar",
            vec!["--create", "--file=snapshot.tar", "src/a.rs", "src/b.rs"],
            2,
            1,
        ),
        (
            "zip",
            vec![
                "-ruT9",
                "-P",
                "SYNTHETIC_PASSWORD",
                "snapshot.zip",
                "src/a.rs",
                "src/b.rs",
            ],
            2,
            1,
        ),
        (
            "ditto",
            vec![
                "-c",
                "-k",
                "--keepParent",
                "--zlibCompressionLevel",
                "7",
                "src",
                "snapshot.zip",
            ],
            1,
            1,
        ),
        ("gzip", vec!["-k9", "src/a.rs", "src/b.rs"], 2, 2),
        (
            "pigz",
            vec!["-p", "2", "-b128", "-k9", "src/a.rs", "src/b.rs"],
            2,
            2,
        ),
        ("bzip2", vec!["-k9", "src/a.rs", "src/b.rs"], 2, 2),
        (
            "pbzip2",
            vec!["-p2", "-m100", "-b15", "-k9", "src/a.rs", "src/b.rs"],
            2,
            2,
        ),
        ("xz", vec!["-T2", "-e", "-k6", "src/a.rs", "src/b.rs"], 2, 2),
        (
            "zstd",
            vec![
                "-T2",
                "-r",
                "-3",
                "-o",
                "snapshot.zst",
                "src/a.rs",
                "src/b.rs",
            ],
            2,
            1,
        ),
        (
            "7z",
            vec![
                "a",
                "-r",
                "-mx=9",
                "-mmt=2",
                "-pSYNTHETIC_PASSWORD",
                "snapshot.7z",
                "src/a.rs",
                "src/b.rs",
            ],
            2,
            1,
        ),
        (
            "7zz",
            vec![
                "u",
                "-t7z",
                "-mhe=on",
                "snapshot.7z",
                "src/a.rs",
                "src/b.rs",
            ],
            2,
            1,
        ),
        (
            "rar",
            vec![
                "u",
                "-r",
                "-m5",
                "-pSYNTHETIC_PASSWORD",
                "snapshot.rar",
                "src/a.rs",
                "src/b.rs",
            ],
            2,
            1,
        ),
    ];
    for (tool, mut args, input_count, output_count) in cases {
        args.insert(0, tool);
        let outcome = parse(&archive_exec(tool, &args));
        assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        let archive = outcome.event.as_ref().unwrap().archive.as_ref().unwrap();
        assert_eq!(archive.tool, tool);
        assert_eq!(archive.input_paths.len(), input_count, "{tool}");
        assert_eq!(
            archive.output_paths.len() + usize::from(archive.output_path.is_some()),
            output_count,
            "{tool}"
        );
        if output_count > 1 {
            assert!(archive.output_path.is_none(), "{tool}");
            let suffix = match tool {
                "gzip" | "pigz" => ".gz",
                "bzip2" | "pbzip2" => ".bz2",
                "xz" => ".xz",
                _ => unreachable!(),
            };
            assert_eq!(
                archive.output_paths,
                ["src/a.rs", "src/b.rs"].map(|path| PathBuf::from(format!(
                    "/private/tmp/synthetic-project/{path}{suffix}"
                )))
            );
        }
        assert!(
            !serde_json::to_string(&outcome)
                .unwrap()
                .contains("SYNTHETIC_PASSWORD")
        );
    }
}

#[test]
fn reverse_modes_are_excluded_before_paths_lists_and_cwd_are_resolved() {
    let cases: Vec<(&str, Vec<Vec<&str>>)> = vec![
        (
            "tar",
            vec![vec!["-xf"], vec!["-tf"], vec!["--compare", "--file"]],
        ),
        ("bsdtar", vec![vec!["-xf"], vec!["-tf"]]),
        (
            "gtar",
            vec![
                vec!["--extract", "--file"],
                vec!["--list", "--file"],
                vec!["--delete", "--file"],
            ],
        ),
        (
            "zip",
            vec![vec!["-d"], vec!["-T"], vec!["-sf"], vec!["-F"], vec!["-U"]],
        ),
        ("ditto", vec![vec!["-x"], vec!["-k"]]),
        ("gzip", vec![vec!["-d"], vec!["-t"], vec!["-l"]]),
        (
            "pigz",
            vec![vec!["--decompress"], vec!["--test"], vec!["--list"]],
        ),
        ("bzip2", vec![vec!["-d"], vec!["-t"], vec!["-L"]]),
        ("pbzip2", vec![vec!["-d"], vec!["-t"], vec!["-V"]]),
        ("xz", vec![vec!["-d"], vec!["-t"], vec!["-l"]]),
        (
            "zstd",
            vec![
                vec!["-d"],
                vec!["--test"],
                vec!["-l"],
                vec!["-b1"],
                vec!["--train"],
            ],
        ),
        (
            "7z",
            vec![
                vec!["x"],
                vec!["e"],
                vec!["l"],
                vec!["t"],
                vec!["d"],
                vec!["rn"],
            ],
        ),
        (
            "7zz",
            vec![vec!["x"], vec!["e"], vec!["l"], vec!["t"], vec!["d"]],
        ),
        (
            "rar",
            vec![
                vec!["x"],
                vec!["e"],
                vec!["l"],
                vec!["t"],
                vec!["d"],
                vec!["p"],
            ],
        ),
    ];
    for (tool, modes) in cases {
        for mut args in modes {
            args.insert(0, tool);
            args.extend(["relative.archive", "@SYNTHETIC_PRIVATE_LIST"]);
            let mut value = archive_exec(tool, &args);
            value["event"]["exec"]["cwd"] = json!({"path_truncated": true});
            let outcome = parse(&value);
            assert!(outcome.event.as_ref().unwrap().archive.is_none(), "{tool}");
            assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        }
    }
}

#[test]
fn explicit_files_to_stdout_preserve_inputs_without_fabricating_output_paths() {
    for tool in [
        "tar", "bsdtar", "gtar", "zip", "ditto", "gzip", "pigz", "bzip2", "pbzip2", "xz", "zstd",
        "7z", "7zz",
    ] {
        let mut args = match tool {
            "tar" | "bsdtar" | "gtar" => vec!["-cf", "-", "--", "file name.rs", "-literal.rs"],
            "zip" => vec!["-q", "-", "--", "file name.rs", "-literal.rs"],
            "ditto" => vec!["-c", "-k", "file name.rs", "-"],
            "7z" | "7zz" => vec![
                "a",
                "-so",
                "-ttar",
                "snapshot.tar",
                "--",
                "file name.rs",
                "-literal.rs",
            ],
            _ => vec!["-c", "--", "file name.rs", "-literal.rs"],
        };
        args.insert(0, tool);
        let outcome = parse(&archive_exec(tool, &args));
        assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        let archive = outcome.event.unwrap().archive.unwrap();
        assert!(
            archive.input_paths.contains(&PathBuf::from(
                "/private/tmp/synthetic-project/file name.rs"
            )),
            "{tool}"
        );
        assert!(archive.output_path.is_none(), "{tool}");
        assert!(archive.output_paths.is_empty(), "{tool}");
    }
}

#[test]
fn pure_stdin_reports_unknown_project_source_without_turning_labels_into_paths() {
    for tool in [
        "zip", "gzip", "pigz", "bzip2", "pbzip2", "xz", "zstd", "7z", "7zz",
    ] {
        let mut args = match tool {
            "zip" => vec!["-", "-"],
            "7z" | "7zz" => vec![
                "a",
                "-siSYNTHETIC_STDIN_LABEL",
                "-so",
                "-ttar",
                "snapshot.tar",
            ],
            _ => vec!["-c", "-"],
        };
        args.insert(0, tool);
        let outcome = parse(&archive_exec(tool, &args));
        let archive = outcome.event.as_ref().unwrap().archive.as_ref().unwrap();
        assert!(archive.input_paths.is_empty(), "{tool}");
        assert!(archive.output_path.is_none(), "{tool}");
        assert!(archive.output_paths.is_empty(), "{tool}");
        assert!(
            outcome
                .issues
                .iter()
                .any(|issue| issue.code == "archive_input_source_unknown"),
            "{tool}"
        );
        assert!(
            !serde_json::to_string(&outcome)
                .unwrap()
                .contains("SYNTHETIC_STDIN_LABEL")
        );
    }
}

#[test]
fn unsupported_lists_unknown_parameters_and_missing_values_are_private_gaps() {
    for (tool, args, code) in [
        (
            "tar",
            vec!["-cf", "snapshot.tar", "-T", "SYNTHETIC_PRIVATE_LIST"],
            "archive_input_list_unsupported",
        ),
        (
            "zip",
            vec!["snapshot.zip", "-@"],
            "archive_input_list_unsupported",
        ),
        (
            "ditto",
            vec![
                "-c",
                "--bom",
                "SYNTHETIC_PRIVATE_LIST",
                "src",
                "snapshot.zip",
            ],
            "archive_input_list_unsupported",
        ),
        (
            "xz",
            vec!["--files=SYNTHETIC_PRIVATE_LIST"],
            "archive_input_list_unsupported",
        ),
        (
            "zstd",
            vec!["--filelist", "SYNTHETIC_PRIVATE_LIST"],
            "archive_input_list_unsupported",
        ),
        (
            "7z",
            vec!["a", "snapshot.7z", "@SYNTHETIC_PRIVATE_LIST"],
            "archive_input_list_unsupported",
        ),
        (
            "7zz",
            vec!["a", "snapshot.7z", "-i@SYNTHETIC_PRIVATE_LIST"],
            "archive_input_list_unsupported",
        ),
        (
            "rar",
            vec!["a", "snapshot.rar", "@SYNTHETIC_PRIVATE_LIST"],
            "archive_input_list_unsupported",
        ),
        ("pigz", vec!["-p"], "archive_arguments_incomplete"),
        ("zstd", vec!["-o"], "archive_arguments_incomplete"),
        ("ditto", vec!["-c", "src"], "archive_arguments_incomplete"),
    ] {
        let mut full_args = vec![tool];
        full_args.extend(args);
        let outcome = parse(&archive_exec(tool, &full_args));
        assert!(outcome.event.as_ref().unwrap().archive.is_none(), "{tool}");
        assert!(
            outcome.issues.iter().any(|issue| issue.code == code),
            "{tool}"
        );
        assert!(
            !serde_json::to_string(&outcome)
                .unwrap()
                .contains("SYNTHETIC_PRIVATE_LIST")
        );
    }
    for tool in [
        "tar", "bsdtar", "gtar", "zip", "ditto", "gzip", "pigz", "bzip2", "pbzip2", "xz", "zstd",
        "7z", "7zz", "rar",
    ] {
        let mut args = match tool {
            "tar" | "bsdtar" | "gtar" => vec!["-cf", "snapshot.tar", "src"],
            "zip" => vec!["snapshot.zip", "src"],
            "ditto" => vec!["-c", "src", "snapshot.zip"],
            "7z" | "7zz" | "rar" => vec!["a", "snapshot.archive", "src"],
            _ => vec!["src/a.rs"],
        };
        args.insert(0, tool);
        args.extend(["--unsupported", "SYNTHETIC_PRIVATE_VALUE"]);
        let outcome = parse(&archive_exec(tool, &args));
        assert!(outcome.event.as_ref().unwrap().archive.is_none(), "{tool}");
        assert!(
            outcome
                .issues
                .iter()
                .any(|issue| issue.code == "archive_arguments_incomplete"),
            "{tool}"
        );
        assert!(
            !serde_json::to_string(&outcome)
                .unwrap()
                .contains("SYNTHETIC_PRIVATE_VALUE")
        );
    }
}

#[test]
fn compression_output_options_are_not_inputs_and_recursive_outputs_are_not_invented() {
    for (tool, args, expected) in [
        ("gzip", vec!["-S", ".custom", "src/a.rs"], "src/a.rs.custom"),
        (
            "pigz",
            vec!["--suffix=.custom", "src/a.rs"],
            "src/a.rs.custom",
        ),
        (
            "xz",
            vec!["--format=lzma", "-T", "2", "src/a.rs"],
            "src/a.rs.lzma",
        ),
        (
            "zstd",
            vec!["-D", "SYNTHETIC_DICTIONARY", "-oresult.zst", "src/a.rs"],
            "result.zst",
        ),
        (
            "zip",
            vec!["old.zip", "src/a.rs", "--out=new.zip"],
            "new.zip",
        ),
    ] {
        let mut full_args = vec![tool];
        full_args.extend(args);
        let outcome = parse(&archive_exec(tool, &full_args));
        assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        let archive = outcome.event.unwrap().archive.unwrap();
        assert_eq!(
            archive.input_paths,
            [PathBuf::from("/private/tmp/synthetic-project/src/a.rs")],
            "{tool}"
        );
        assert_eq!(
            archive.output_path,
            Some(PathBuf::from(format!(
                "/private/tmp/synthetic-project/{expected}"
            ))),
            "{tool}"
        );
    }
    for tool in ["gzip", "pigz", "zstd"] {
        let outcome = parse(&archive_exec(tool, &[tool, "-r", "src"]));
        let archive = outcome.event.unwrap().archive.unwrap();
        assert_eq!(archive.input_paths.len(), 1);
        assert!(archive.output_path.is_none());
        assert!(archive.output_paths.is_empty());
    }
}

#[test]
fn mode_like_option_values_legacy_tar_and_zstd_output_order_follow_tool_grammar() {
    for (tool, args) in [
        ("tar", vec!["-cf", "snapshot.tar", "-T", "-x"]),
        (
            "tar",
            vec![
                "--create",
                "--file=snapshot.tar",
                "--files-from",
                "--extract",
            ],
        ),
        ("ditto", vec!["-c", "--bom", "-x", "src", "snapshot.zip"]),
        ("zstd", vec!["--filelist", "-d"]),
    ] {
        let mut full_args = vec![tool];
        full_args.extend(args);
        let outcome = parse(&archive_exec(tool, &full_args));
        assert!(outcome.event.as_ref().unwrap().archive.is_none());
        assert!(
            outcome
                .issues
                .iter()
                .any(|issue| issue.code == "archive_input_list_unsupported"),
            "{tool}"
        );
    }
    for tool in ["tar", "bsdtar", "gtar"] {
        let outcome = parse(&archive_exec(
            tool,
            &[tool, "cfp", "snapshot.tar", "src/a.rs"],
        ));
        assert!(outcome.issues.is_empty());
        let archive = outcome.event.unwrap().archive.unwrap();
        assert_eq!(
            archive.output_path,
            Some(PathBuf::from("/private/tmp/synthetic-project/snapshot.tar"))
        );
        assert_eq!(
            archive.input_paths,
            [PathBuf::from("/private/tmp/synthetic-project/src/a.rs")]
        );
    }
    for tool in ["tar", "bsdtar"] {
        let outcome = parse(&archive_exec(
            tool,
            &[tool, "-cf", "snapshot.tar", "-I", "-x", "src/a.rs"],
        ));
        assert!(outcome.event.as_ref().unwrap().archive.is_none());
        assert!(
            outcome
                .issues
                .iter()
                .any(|issue| issue.code == "archive_input_list_unsupported")
        );
    }
    let gtar = parse(&archive_exec(
        "gtar",
        &[
            "gtar",
            "-cf",
            "snapshot.tar",
            "-I",
            "SYNTHETIC_COMPRESS_PROGRAM",
            "-H",
            "ustar",
            "src/a.rs",
        ],
    ));
    assert!(gtar.issues.is_empty());
    assert_eq!(
        gtar.event
            .as_ref()
            .unwrap()
            .archive
            .as_ref()
            .unwrap()
            .input_paths,
        [PathBuf::from("/private/tmp/synthetic-project/src/a.rs")]
    );
    assert!(
        !serde_json::to_string(&gtar)
            .unwrap()
            .contains("SYNTHETIC_COMPRESS_PROGRAM")
    );
    let disk = parse(&archive_exec(
        "zstd",
        &["zstd", "-c", "-o", "snapshot.zst", "src/a.rs"],
    ));
    assert_eq!(
        disk.event.unwrap().archive.unwrap().output_path,
        Some(PathBuf::from("/private/tmp/synthetic-project/snapshot.zst"))
    );
    let stdout = parse(&archive_exec(
        "zstd",
        &["zstd", "-o", "snapshot.zst", "-c", "src/a.rs"],
    ));
    assert!(stdout.event.unwrap().archive.unwrap().output_path.is_none());
    for tool in ["7z", "7zz"] {
        let outcome = parse(&archive_exec(
            tool,
            &[
                tool,
                "-bd",
                "a",
                "-ttar",
                "-so",
                "-pSYNTHETIC_PASSWORD@VALUE",
                "snapshot.tar",
                "--",
                "@literal.rs",
            ],
        ));
        assert!(outcome.issues.is_empty());
        let archive = outcome.event.as_ref().unwrap().archive.as_ref().unwrap();
        assert_eq!(
            archive.input_paths,
            [PathBuf::from("/private/tmp/synthetic-project/@literal.rs")]
        );
        assert!(
            !serde_json::to_string(&outcome)
                .unwrap()
                .contains("SYNTHETIC_PASSWORD")
        );
        let invalid = parse(&archive_exec(
            tool,
            &[tool, "a", "-so", "snapshot.7z", "src/a.rs"],
        ));
        assert!(invalid.event.unwrap().archive.is_none());
        assert!(
            invalid
                .issues
                .iter()
                .any(|issue| issue.code == "archive_arguments_incomplete")
        );
    }
    let rar = parse(&archive_exec(
        "rar",
        &["rar", "a", "-siSYNTHETIC_STDIN_LABEL", "snapshot.rar"],
    ));
    assert!(
        rar.event
            .as_ref()
            .unwrap()
            .archive
            .as_ref()
            .unwrap()
            .input_paths
            .is_empty()
    );
    assert!(
        rar.issues
            .iter()
            .any(|issue| issue.code == "archive_input_source_unknown")
    );
    assert!(
        !serde_json::to_string(&rar)
            .unwrap()
            .contains("SYNTHETIC_STDIN_LABEL")
    );
    for tool in ["7z", "7zz", "rar"] {
        let unknown = parse(&archive_exec(
            tool,
            &[
                tool,
                "SYNTHETIC_UNKNOWN_MODE",
                "snapshot.archive",
                "src/a.rs",
            ],
        ));
        assert!(unknown.event.as_ref().unwrap().archive.is_none());
        assert!(
            unknown
                .issues
                .iter()
                .any(|issue| issue.code == "archive_arguments_incomplete")
        );
        assert!(
            !serde_json::to_string(&unknown)
                .unwrap()
                .contains("SYNTHETIC_UNKNOWN_MODE")
        );
    }
}

#[test]
fn compressor_operation_order_and_long_aliases_are_specific_to_each_tool() {
    for tool in ["bzip2", "xz", "zstd"] {
        let modes = if tool == "bzip2" {
            vec!["-d", "-t"]
        } else {
            vec!["-d", "-t", "-l"]
        };
        for reverse in modes {
            let compress = parse(&archive_exec(
                tool,
                &[tool, reverse, "-z", "-c", "src/a.rs"],
            ));
            assert!(compress.issues.is_empty(), "{tool}: {:?}", compress.issues);
            assert!(compress.event.unwrap().archive.is_some(), "{tool}");
            let reverse = parse(&archive_exec(tool, &[tool, "-z", reverse, "src/a.rs"]));
            assert!(reverse.issues.is_empty(), "{tool}: {:?}", reverse.issues);
            assert!(reverse.event.unwrap().archive.is_none(), "{tool}");
        }
        let long = parse(&archive_exec(
            tool,
            &[tool, "--decompress", "--compress", "--stdout", "src/a.rs"],
        ));
        assert!(long.issues.is_empty(), "{tool}: {:?}", long.issues);
        assert!(long.event.unwrap().archive.is_some(), "{tool}");
    }
    for tool in ["gzip", "pigz", "xz"] {
        let outcome = parse(&archive_exec(tool, &[tool, "--to-stdout", "src/a.rs"]));
        assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        let archive = outcome.event.unwrap().archive.unwrap();
        assert!(archive.output_path.is_none());
    }
    for (tool, option) in [
        ("gzip", "--compress"),
        ("pigz", "--compress"),
        ("bzip2", "--to-stdout"),
        ("zstd", "--to-stdout"),
        ("bzip2", "--list"),
        ("gzip", "--long-help"),
        ("zstd", "--long-help"),
        ("xz", "--license"),
        ("pbzip2", "--keep"),
    ] {
        let outcome = parse(&archive_exec(tool, &[tool, option, "src/a.rs"]));
        assert!(outcome.event.as_ref().unwrap().archive.is_none(), "{tool}");
        assert!(
            outcome
                .issues
                .iter()
                .any(|issue| issue.code == "archive_arguments_incomplete"),
            "{tool}"
        );
    }
    for args in [
        vec!["pbzip2", "-d", "-z", "src/a.rs"],
        vec!["pbzip2", "-z", "-t", "src/a.rs"],
    ] {
        let outcome = parse(&archive_exec("pbzip2", &args));
        assert!(outcome.event.as_ref().unwrap().archive.is_none());
        assert!(
            outcome
                .issues
                .iter()
                .any(|issue| issue.code == "archive_arguments_incomplete")
        );
    }
}

#[test]
fn tar_long_options_follow_the_confirmed_bsd_and_gnu_dialects() {
    for options in [
        vec!["--mac-metadata"],
        vec!["--no-mac-metadata"],
        vec!["--cd", "/private/tmp/synthetic-project"],
        vec!["--block-size", "20"],
    ] {
        for tool in ["tar", "bsdtar", "gtar"] {
            let mut args = vec![tool, "-cf", "snapshot.tar"];
            args.extend(options.iter().copied());
            args.push("src/a.rs");
            let outcome = parse(&archive_exec(tool, &args));
            if tool == "gtar" {
                assert!(outcome.event.as_ref().unwrap().archive.is_none());
                assert!(
                    outcome
                        .issues
                        .iter()
                        .any(|issue| issue.code == "archive_arguments_incomplete")
                );
            } else {
                assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
                assert_eq!(
                    outcome.event.unwrap().archive.unwrap().input_paths,
                    [PathBuf::from("/private/tmp/synthetic-project/src/a.rs")]
                );
            }
        }
    }
    for tool in ["tar", "bsdtar", "gtar"] {
        let outcome = parse(&archive_exec(
            tool,
            &[
                tool,
                "-cf",
                "snapshot.tar",
                "--blocking-factor",
                "20",
                "src/a.rs",
            ],
        ));
        assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        assert_eq!(
            outcome.event.unwrap().archive.unwrap().input_paths,
            [PathBuf::from("/private/tmp/synthetic-project/src/a.rs")]
        );
    }
}

#[test]
fn extensionless_archive_names_resolve_to_the_actual_default_output() {
    for (tool, mut args, extension) in [
        ("zip", vec!["pack", "src/a.rs"], "zip"),
        ("7z", vec!["a", "pack", "src/a.rs"], "7z"),
        ("7zz", vec!["a", "pack", "src/a.rs"], "7z"),
        ("rar", vec!["a", "pack", "src/a.rs"], "rar"),
    ] {
        args.insert(0, tool);
        let outcome = parse(&archive_exec(tool, &args));
        assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        assert_eq!(
            outcome.event.unwrap().archive.unwrap().output_path,
            Some(PathBuf::from(format!(
                "/private/tmp/synthetic-project/pack.{extension}"
            ))),
            "{tool}"
        );
    }
    for (tool, mut args, extension) in [
        ("zip", vec!["old.zip", "src/a.rs", "--out", "pack"], "zip"),
        ("7z", vec!["a", "-tzip", "pack", "src/a.rs"], "zip"),
        ("7zz", vec!["a", "-tzip", "pack", "src/a.rs"], "zip"),
        ("7z", vec!["a", "-ttar", "pack", "src/a.rs"], "tar"),
        ("7zz", vec!["a", "-ttar", "pack", "src/a.rs"], "tar"),
        ("7z", vec!["a", "-tgzip", "pack", "src/a.rs"], "gz"),
        ("7zz", vec!["a", "-tgzip", "pack", "src/a.rs"], "gz"),
        ("7z", vec!["a", "-tbzip2", "pack", "src/a.rs"], "bz2"),
        ("7zz", vec!["a", "-tbzip2", "pack", "src/a.rs"], "bz2"),
        ("7z", vec!["a", "-txz", "pack", "src/a.rs"], "xz"),
        ("7zz", vec!["a", "-txz", "pack", "src/a.rs"], "xz"),
    ] {
        args.insert(0, tool);
        let outcome = parse(&archive_exec(tool, &args));
        assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        assert_eq!(
            outcome.event.unwrap().archive.unwrap().output_path,
            Some(PathBuf::from(format!(
                "/private/tmp/synthetic-project/pack.{extension}"
            ))),
            "{tool}"
        );
    }
    for (tool, mut args) in [
        ("zip", vec!["pack.data", "src/a.rs"]),
        ("zip", vec!["old.zip", "src/a.rs", "--out=pack.data"]),
        ("7z", vec!["a", "pack.data", "src/a.rs"]),
        ("7zz", vec!["a", "-tzip", "pack.data", "src/a.rs"]),
        ("rar", vec!["a", "pack.data", "src/a.rs"]),
        ("rar", vec!["a", "pack.zip", "src/a.rs"]),
    ] {
        args.insert(0, tool);
        let output = if args.contains(&"pack.zip") {
            "pack.zip"
        } else {
            "pack.data"
        };
        let outcome = parse(&archive_exec(tool, &args));
        assert!(outcome.issues.is_empty(), "{tool}: {:?}", outcome.issues);
        assert_eq!(
            outcome.event.unwrap().archive.unwrap().output_path,
            Some(PathBuf::from(format!(
                "/private/tmp/synthetic-project/{output}"
            ))),
            "{tool}"
        );
    }
    for tool in ["7z", "7zz"] {
        let outcome = parse(&archive_exec(
            tool,
            &[tool, "a", "-tSYNTHETIC_UNKNOWN_FORMAT", "pack", "src/a.rs"],
        ));
        assert!(outcome.event.as_ref().unwrap().archive.is_none());
        assert!(
            outcome
                .issues
                .iter()
                .any(|issue| issue.code == "archive_arguments_incomplete")
        );
        assert!(
            !serde_json::to_string(&outcome)
                .unwrap()
                .contains("SYNTHETIC_UNKNOWN_FORMAT")
        );
    }
    for format in ["-afzip", "-afrar"] {
        let unsupported_rar = parse(&archive_exec(
            "rar",
            &["rar", "a", format, "pack", "src/a.rs"],
        ));
        assert!(unsupported_rar.event.as_ref().unwrap().archive.is_none());
        assert!(
            unsupported_rar
                .issues
                .iter()
                .any(|issue| issue.code == "archive_arguments_incomplete")
        );
    }
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
