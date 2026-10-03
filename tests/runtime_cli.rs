use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use tempfile::TempDir;

// 该替身只验证 CLI 与本机 IPC 的请求形状，不代表真实采集或通知验收。
fn start_ipc_stub(socket_path: &Path, responses: Vec<Value>) -> (Receiver<Value>, JoinHandle<()>) {
    let listener = UnixListener::bind(socket_path).unwrap();
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600)).unwrap();
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            sender
                .send(serde_json::from_str(&request_line).unwrap())
                .unwrap();
            serde_json::to_writer(&mut stream, &response).unwrap();
            stream.write_all(b"\n").unwrap();
            stream.flush().unwrap();
        }
    });
    (receiver, worker)
}

fn invoke(socket_path: &Path, args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codeperimeter"))
        .arg("--host-socket")
        .arg(socket_path)
        .args(args)
        .output()
        .unwrap()
}

fn invoke_without_socket(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codeperimeter"))
        .args(args)
        .output()
        .unwrap()
}

fn success_response(data: Value) -> Value {
    json!({"ok": true, "error": null, "data": data})
}

fn string_arg(value: &Path) -> String {
    value.to_string_lossy().into_owned()
}

#[test]
fn manual_directories_and_event_filters_use_host_ipc() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("host.sock");
    let first = temp.path().join("first-project");
    let second = temp.path().join("second-project");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let (requests, worker) = start_ipc_stub(
        &socket,
        vec![
            success_response(json!({"added": 2})),
            success_response(json!([])),
            success_response(json!({"removed": true})),
            success_response(json!([])),
        ],
    );

    let output = invoke(
        &socket,
        &[
            "watch".into(),
            "add".into(),
            string_arg(&first),
            string_arg(&second),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let add = requests.recv().unwrap();
    assert_eq!(add["operation"], "add_directories");
    let entries = add["payload"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["sources"], json!(["manual"]));
    assert_eq!(
        entries[0]["path"],
        fs::canonicalize(&first).unwrap().to_str().unwrap()
    );

    let output = invoke(&socket, &["watch".into(), "list".into()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(requests.recv().unwrap()["operation"], "list_directories");

    let output = invoke(
        &socket,
        &["watch".into(), "remove".into(), string_arg(&first)],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let remove = requests.recv().unwrap();
    assert_eq!(remove["operation"], "remove_directory");
    assert_eq!(
        remove["payload"]["path"],
        fs::canonicalize(&first).unwrap().to_str().unwrap()
    );

    let output = invoke(
        &socket,
        &[
            "events".into(),
            "--pid".into(),
            "231".into(),
            "--kind".into(),
            "open".into(),
            "--limit".into(),
            "2".into(),
            "--directory".into(),
            string_arg(temp.path()),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let query = requests.recv().unwrap();
    assert_eq!(query["operation"], "query_events");
    assert_eq!(query["payload"]["filter"]["pid"], 231);
    assert_eq!(query["payload"]["filter"]["kind"], "open");
    assert_eq!(query["payload"]["filter"]["limit"], 2);
    worker.join().unwrap();
}

#[test]
fn alert_health_notification_and_stats_queries_serialize_filters() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("host.sock");
    let (requests, worker) = start_ipc_stub(
        &socket,
        vec![
            success_response(json!([])),
            success_response(json!([])),
            success_response(json!([])),
            success_response(json!({})),
        ],
    );

    let output = invoke(
        &socket,
        &[
            "alerts".into(),
            "--pid".into(),
            "77".into(),
            "--rule".into(),
            "bulk_file_access".into(),
            "--limit".into(),
            "9".into(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let alerts = requests.recv().unwrap();
    assert_eq!(alerts["operation"], "query_alerts");
    assert_eq!(alerts["payload"]["filter"]["pid"], 77);
    assert_eq!(alerts["payload"]["filter"]["rule"], "bulk_file_access");
    assert_eq!(alerts["payload"]["filter"]["limit"], 9);

    let output = invoke(
        &socket,
        &[
            "health".into(),
            "--component".into(),
            "collector".into(),
            "--code".into(),
            "disconnected".into(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let health = requests.recv().unwrap();
    assert_eq!(health["operation"], "query_health");
    assert_eq!(health["payload"]["filter"]["component"], "collector");
    assert_eq!(health["payload"]["filter"]["code"], "disconnected");

    let output = invoke(
        &socket,
        &[
            "notifications".into(),
            "--alert-id".into(),
            "synthetic-alert".into(),
            "--outcome".into(),
            "failed".into(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let notifications = requests.recv().unwrap();
    assert_eq!(notifications["operation"], "query_notifications");
    assert_eq!(
        notifications["payload"]["filter"]["alert_id"],
        "synthetic-alert"
    );
    assert_eq!(notifications["payload"]["filter"]["outcome"], "failed");

    let output = invoke(
        &socket,
        &[
            "stats".into(),
            "show".into(),
            "--since-ms".into(),
            "100".into(),
            "--until-ms".into(),
            "200".into(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stats = requests.recv().unwrap();
    assert_eq!(stats["operation"], "stats");
    assert_eq!(stats["payload"]["since_ms"], 100);
    assert_eq!(stats["payload"]["until_ms"], 200);
    worker.join().unwrap();
}

#[test]
fn history_import_uses_the_saved_preview_after_new_history_arrives() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("host.sock");
    let codex_home = temp.path().join("codex");
    let claude_home = temp.path().join("claude");
    let old_project = temp.path().join("project-z");
    let newer_project = temp.path().join("project-a");
    fs::create_dir_all(&old_project).unwrap();
    fs::create_dir_all(&newer_project).unwrap();
    let history_file = codex_home.join("sessions/first.jsonl");
    fs::create_dir_all(history_file.parent().unwrap()).unwrap();
    fs::write(
        &history_file,
        format!(
            "{}\n{}\n",
            json!({
                "type": "session_meta",
                "payload": {"cwd": old_project, "cli_version": "synthetic"}
            }),
            json!({"type": "user", "message": "PRIVATE_CONTENT"})
        ),
    )
    .unwrap();

    let preview_file = temp.path().join("history-preview.json");
    let preview = invoke(
        &socket,
        &[
            "history".into(),
            "preview".into(),
            "--codex-home".into(),
            string_arg(&codex_home),
            "--claude-home".into(),
            string_arg(&claude_home),
            "--output".into(),
            string_arg(&preview_file),
        ],
    );
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let saved = fs::read_to_string(&preview_file).unwrap();
    assert!(!saved.contains("PRIVATE_CONTENT"));

    let later_history = codex_home.join("sessions/later.jsonl");
    fs::write(
        &later_history,
        format!(
            "{}\n",
            json!({
                "type": "session_meta",
                "payload": {"cwd": newer_project, "cli_version": "synthetic"}
            })
        ),
    )
    .unwrap();

    let (requests, worker) = start_ipc_stub(&socket, vec![success_response(json!({"added": 1}))]);
    let output = invoke(
        &socket,
        &[
            "history".into(),
            "import".into(),
            "--preview".into(),
            string_arg(&preview_file),
            "--index".into(),
            "0".into(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = requests.recv().unwrap();
    let entry = &request["payload"]["entries"][0];
    assert_eq!(
        entry["path"],
        fs::canonicalize(&old_project).unwrap().to_str().unwrap()
    );
    assert_eq!(entry["sources"], json!(["codex"]));
    assert_ne!(
        entry["path"],
        fs::canonicalize(&newer_project).unwrap().to_str().unwrap()
    );
    worker.join().unwrap();
}

#[test]
fn history_import_rejects_a_preview_directory_that_disappeared() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("missing-host.sock");
    let codex_home = temp.path().join("codex");
    let project = temp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let history_file = codex_home.join("sessions/session.jsonl");
    fs::create_dir_all(history_file.parent().unwrap()).unwrap();
    fs::write(
        &history_file,
        format!(
            "{}\n",
            json!({
                "type": "session_meta",
                "payload": {"cwd": project, "cli_version": "synthetic"}
            })
        ),
    )
    .unwrap();
    let preview_file = temp.path().join("preview.json");
    let preview = invoke(
        &socket,
        &[
            "history".into(),
            "preview".into(),
            "--codex-home".into(),
            string_arg(&codex_home),
            "--claude-home".into(),
            string_arg(&temp.path().join("empty-claude")),
            "--output".into(),
            string_arg(&preview_file),
        ],
    );
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    fs::remove_dir(&project).unwrap();

    let output = invoke(
        &socket,
        &[
            "history".into(),
            "import".into(),
            "--preview".into(),
            string_arg(&preview_file),
            "--all-available".into(),
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("预览目录当前不可用"));
}

#[test]
fn failed_host_response_and_invalid_arguments_return_failure_without_echoing_arguments() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("host.sock");
    let (requests, worker) = start_ipc_stub(
        &socket,
        vec![json!({
            "ok": false,
            "error": "控制请求被拒绝",
            "data": null
        })],
    );
    let output = invoke(&socket, &["status".into()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("控制请求被拒绝"));
    assert_eq!(requests.recv().unwrap()["operation"], "status");
    worker.join().unwrap();

    let invalid = invoke_without_socket(&["status", "--unexpected", "PRIVATE_ARGUMENT_SENTINEL"]);
    assert!(!invalid.status.success());
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&invalid.stdout),
        String::from_utf8_lossy(&invalid.stderr)
    );
    assert!(!diagnostic.contains("PRIVATE_ARGUMENT_SENTINEL"));
}

#[test]
fn stats_clear_uses_host_request_and_service_roles_keep_the_planned_argv() {
    let temp = TempDir::new().unwrap();
    let socket = temp.path().join("host.sock");
    let (requests, worker) = start_ipc_stub(&socket, vec![success_response(json!({}))]);
    let output = invoke(&socket, &["stats".into(), "clear-cumulative".into()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        requests.recv().unwrap()["operation"],
        "clear_cumulative_stats"
    );
    worker.join().unwrap();

    let collector = invoke_without_socket(&["collector", "--help"]);
    let collector_help = format!(
        "{}{}",
        String::from_utf8_lossy(&collector.stdout),
        String::from_utf8_lossy(&collector.stderr)
    );
    assert!(collector.status.success());
    assert!(collector_help.contains("--socket"));
    assert!(collector_help.contains("--allowed-uid"));

    let daemon = invoke_without_socket(&["daemon", "--help"]);
    let daemon_help = format!(
        "{}{}",
        String::from_utf8_lossy(&daemon.stdout),
        String::from_utf8_lossy(&daemon.stderr)
    );
    assert!(daemon.status.success());
    assert!(daemon_help.contains("--socket"));
    assert!(daemon_help.contains("--control-socket"));
    assert!(daemon_help.contains("--db"));
    assert!(daemon_help.contains("--bulk-file-threshold"));
    assert!(daemon_help.contains("--bulk-window-ms"));

    let notify = invoke_without_socket(&["notify", "--help"]);
    let notify_help = format!(
        "{}{}",
        String::from_utf8_lossy(&notify.stdout),
        String::from_utf8_lossy(&notify.stderr)
    );
    assert!(notify.status.success());
    assert!(notify_help.contains("--control-socket"));
}

#[test]
fn service_plan_exposes_the_exact_fixed_role_arguments_without_installing() {
    let Some(user) = std::env::var_os("USER") else {
        return;
    };
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let output = invoke_without_socket(&["service", "plan", "--user", user.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    let data = &plan["data"];
    let binary = data["installed_binary"].as_str().unwrap();
    let collector_socket = data["collector_socket"].as_str().unwrap();
    let control_socket = data["control_socket"].as_str().unwrap();
    let db_path = data["db_path"].as_str().unwrap();
    let uid = data["uid"].as_u64().unwrap().to_string();
    let jobs = data["jobs"].as_array().unwrap();
    let collector = jobs
        .iter()
        .find(|job| job["label"].as_str().unwrap().contains(".collector."))
        .unwrap();
    assert_eq!(
        collector["argv"],
        json!([
            binary,
            "collector",
            "--socket",
            collector_socket,
            "--allowed-uid",
            uid
        ])
    );
    let daemon = jobs
        .iter()
        .find(|job| job["label"].as_str().unwrap().contains(".daemon."))
        .unwrap();
    assert_eq!(
        daemon["argv"],
        json!([
            binary,
            "daemon",
            "--socket",
            collector_socket,
            "--control-socket",
            control_socket,
            "--db",
            db_path
        ])
    );
    let notify = jobs
        .iter()
        .find(|job| job["label"].as_str().unwrap().contains(".notify."))
        .unwrap();
    assert_eq!(
        notify["argv"],
        json!([binary, "notify", "--control-socket", control_socket])
    );
    assert!(notify.get("plist").is_some());
}

#[test]
fn host_socket_option_is_global_and_missing_socket_fails() {
    let temp = TempDir::new().unwrap();
    let missing_socket = temp.path().join("missing.sock");
    let output = invoke(&missing_socket, &["status".into()]);
    assert!(!output.status.success());
}
