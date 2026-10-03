// 真实 CLI／普通用户宿主／SQLite 用户流程；采集源刻意不可用，不代表 ES 实测。
use codeperimeter::runtime::{ControlRequest, request_control};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct Host {
    process: Child,
    socket: PathBuf,
}

impl Host {
    fn start(directory: &Path) -> Self {
        let socket = directory.join("host.sock");
        let mut host = Self {
            process: Command::new(env!("CARGO_BIN_EXE_codeperimeter"))
                .args(["daemon", "--socket"])
                .arg(directory.join("deliberately-unavailable-collector.sock"))
                .arg("--control-socket")
                .arg(&socket)
                .arg("--db")
                .arg(directory.join("events.sqlite"))
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
            socket,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while request_control(&host.socket, ControlRequest::Status).is_err() {
            if let Some(status) = host.process.try_wait().unwrap() {
                let mut error = String::new();
                host.process
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut error)
                    .unwrap();
                panic!("真实 CLI 宿主提前退出 {status}: {error}");
            }
            assert!(Instant::now() < deadline, "宿主控制 socket 未启动");
            thread::sleep(Duration::from_millis(20));
        }
        host
    }

    fn cli(&self, args: &[&str]) -> Value {
        let result = Command::new(env!("CARGO_BIN_EXE_codeperimeter"))
            .arg("--host-socket")
            .arg(&self.socket)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let response: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(response["ok"], true);
        response["data"].clone()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = request_control(&self.socket, ControlRequest::Stop);
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.process.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn cli_imports_a_fixed_history_snapshot_and_reopens_the_same_user_database() {
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "此用户流程必须以普通用户运行"
    );
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let manual = directory.path().join("manual-project");
    let history = directory.path().join("history-project");
    let later = directory.path().join("later-project");
    for root in [&manual, &history, &later] {
        fs::create_dir(root).unwrap();
    }
    let codex = directory.path().join("codex");
    let claude = directory.path().join("claude");
    fs::create_dir_all(codex.join("sessions")).unwrap();
    fs::create_dir_all(claude.join("projects/synthetic")).unwrap();
    let session = codex.join("sessions/synthetic.jsonl");
    fs::write(
        &session,
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","payload":{"cwd":history,"cli_version":"0.111.0"}}),
            json!({"type":"response_item","payload":{"text":"SYNTHETIC_PRIVATE_CONVERSATION"}})
        ),
    )
    .unwrap();
    fs::write(claude.join("projects/synthetic/synthetic.jsonl"), format!("{}\n", json!({"type":"user","cwd":history,"version":"2.1.0","message":{"content":"SYNTHETIC_PRIVATE_CONVERSATION"}}))).unwrap();
    let host = Host::start(directory.path());
    host.cli(&["watch", "add", manual.to_str().unwrap()]);
    let preview = directory.path().join("preview.json");
    host.cli(&[
        "history",
        "preview",
        "--codex-home",
        codex.to_str().unwrap(),
        "--claude-home",
        claude.to_str().unwrap(),
        "--output",
        preview.to_str().unwrap(),
    ]);
    let snapshot = fs::read_to_string(&preview).unwrap();
    assert!(!snapshot.contains("SYNTHETIC_PRIVATE_CONVERSATION"));
    let saved: Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(saved["candidates"].as_array().unwrap().len(), 1);
    // 预览后新增会话目录，导入必须仍使用旧快照，不重扫并自动扩大范围。
    let mut changed = fs::read_to_string(&session).unwrap();
    changed.push_str(&format!(
        "{}\n",
        json!({"type":"turn_context","payload":{"cwd":later}})
    ));
    fs::write(session, changed).unwrap();
    host.cli(&[
        "history",
        "import",
        "--preview",
        preview.to_str().unwrap(),
        "--all-available",
    ]);
    let configured = host.cli(&["watch", "list"]);
    let roots = configured.as_array().unwrap();
    assert_eq!(roots.len(), 2);
    assert!(
        roots
            .iter()
            .all(|item| item["path"] != later.to_str().unwrap())
    );
    let imported = roots
        .iter()
        .find(|item| item["path"] == history.canonicalize().unwrap().to_str().unwrap())
        .unwrap();
    assert_eq!(imported["sources"].as_array().unwrap().len(), 2);
    assert!(host.cli(&["events"]).as_array().unwrap().is_empty());
    assert!(host.cli(&["alerts"]).as_array().unwrap().is_empty());
    assert_eq!(host.cli(&["stats", "show"])["cumulative"]["events"], 0);
    let status = host.cli(&["status"]);
    assert_eq!(status["database_state"], "ready");
    assert_eq!(status["bulk_file_threshold"], 50);
    assert_eq!(status["bulk_window_ms"], 10000);
    assert!(status["collector_run_id"].is_null());
    drop(host);
    let reopened = Host::start(directory.path());
    assert_eq!(reopened.cli(&["watch", "list"]), configured);
    reopened.cli(&["watch", "remove", manual.to_str().unwrap()]);
    assert_eq!(
        reopened.cli(&["watch", "list"]).as_array().unwrap().len(),
        1
    );
    assert!(
        reopened
            .cli(&["notifications"])
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn normal_user_cannot_start_the_privileged_collector_role() {
    assert_ne!(unsafe { libc::geteuid() }, 0);
    let result = Command::new(env!("CARGO_BIN_EXE_codeperimeter"))
        .args([
            "collector",
            "--socket",
            "/var/run/codeperimeter-test/collector.sock",
            "--allowed-uid",
        ])
        .arg(unsafe { libc::geteuid() }.to_string())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("管理员权限"));
}
