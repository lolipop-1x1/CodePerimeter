//! 生产 HTTP／普通用户宿主／SQLite 联调；采集输入为明确标注的合成 ES 帧。
use codeperimeter::model::{SourceStream, now_ms};
use codeperimeter::runtime::{
    ControlRequest, DirectoryImport, RuntimeOptions, request_control,
    run_daemon_with_expected_collector_uid,
};
use codeperimeter::service::{CollectorFrame, write_frame};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tempfile::{Builder, TempDir};

struct Rig {
    temp: TempDir,
    host_socket: PathBuf,
    ui: Child,
    source: mpsc::Sender<CollectorFrame>,
    stop: Arc<AtomicBool>,
    collector_thread: Option<JoinHandle<()>>,
    host_thread: Option<JoinHandle<codeperimeter::Result<()>>>,
    session: Value,
    project: PathBuf,
    claude_project: PathBuf,
    zcode_project: PathBuf,
}
fn wait(check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(check(), "独立验收组件未就绪");
}
impl Rig {
    fn new() -> Self {
        let temp = Builder::new()
            .prefix("codeperimeter-web-fixture-")
            .tempdir_in("/private/tmp")
            .unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = temp.path();
        let project = root.join("synthetic-project");
        fs::create_dir(&project).unwrap();
        for index in 0..130 {
            fs::write(
                project.join(format!("source-{index:03}.txt")),
                "synthetic data",
            )
            .unwrap();
        }
        let codex = root.join("synthetic-codex");
        fs::create_dir_all(codex.join("sessions")).unwrap();
        fs::write(
            codex.join("sessions/session.jsonl"),
            format!(
                "{}\n",
                json!({"type":"session_meta","payload":{"cwd":project,"cli_version":"0.1.0"}})
            ),
        )
        .unwrap();
        let claude_project = root.join("synthetic-claude-project");
        let zcode_project = root.join("synthetic-zcode-project");
        fs::create_dir(&claude_project).unwrap();
        fs::create_dir(&zcode_project).unwrap();
        let claude = root.join("synthetic-claude");
        fs::create_dir_all(claude.join("projects/synthetic-project")).unwrap();
        fs::write(claude.join("projects/synthetic-project/session.jsonl"), format!("{}\n", json!({"type":"user","sessionId":"synthetic-claude-session","cwd":claude_project,"version":"2.1.247","message":{"content":"synthetic history"}}))).unwrap();
        let zcode = root.join("synthetic-zcode.sqlite");
        let connection = Connection::open(&zcode).unwrap();
        connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, workspace_id TEXT, directory TEXT NOT NULL, path TEXT, title TEXT NOT NULL, version TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL);").unwrap();
        connection.execute("INSERT INTO session (id,project_id,directory,title,version,time_created,time_updated) VALUES ('synthetic-session','synthetic-project',?1,'synthetic history','synthetic-version',1,1)", params![zcode_project.to_string_lossy()]).unwrap();
        drop(connection);
        fs::set_permissions(&zcode, fs::Permissions::from_mode(0o600)).unwrap();
        let collector = root.join("collector.sock");
        let listener = UnixListener::bind(&collector).unwrap();
        fs::set_permissions(&collector, fs::Permissions::from_mode(0o600)).unwrap();
        let (source, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let collector_stop = stop.clone();
        listener.set_nonblocking(true).unwrap();
        let collector_thread = thread::spawn(move || {
            while !collector_stop.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(20));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                while !collector_stop.load(Ordering::Relaxed) {
                    let frame = receiver
                        .recv_timeout(Duration::from_millis(250))
                        .unwrap_or_else(|_| CollectorFrame::Heartbeat {
                            run_id: "synthetic-web-run".into(),
                            dropped_lines: 0,
                        });
                    if write_frame(&mut stream, &frame).is_err() {
                        break;
                    }
                }
            }
        });
        let host_socket = root.join("host.sock");
        let options = RuntimeOptions {
            collector_socket: collector,
            control_socket: host_socket.clone(),
            database_path: root.join("events.sqlite"),
            bulk_file_threshold: 50,
            bulk_window_ms: 10000,
        };
        let uid = unsafe { libc::geteuid() };
        let host_thread =
            thread::spawn(move || run_daemon_with_expected_collector_uid(options, uid));
        wait(|| host_socket.exists());
        let response = request_control(
            &host_socket,
            ControlRequest::AddDirectories {
                entries: vec![DirectoryImport {
                    path: project.clone(),
                    sources: vec!["manual".into()],
                }],
            },
        )
        .unwrap();
        assert!(response.ok);
        let session_file = root.join("ui.json");
        let ui = Command::new(env!("CARGO_BIN_EXE_codeperimeter"))
            .arg("--host-socket")
            .arg(&host_socket)
            .args(["ui", "--foreground", "--no-browser", "--session-file"])
            .arg(&session_file)
            .arg("--codex-home")
            .arg(codex)
            .arg("--claude-home")
            .arg(claude)
            .arg("--zcode-db")
            .arg(zcode)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait(|| session_file.exists());
        let session: Value = serde_json::from_slice(&fs::read(&session_file).unwrap()).unwrap();
        let rig = Self {
            temp,
            host_socket,
            ui,
            source,
            stop,
            collector_thread: Some(collector_thread),
            host_thread: Some(host_thread),
            session,
            project,
            claude_project,
            zcode_project,
        };
        wait(|| rig.http("GET", "/api/ping", None, None, None).0 == 200);
        rig
    }
    fn http(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        host: Option<&str>,
        origin: Option<&str>,
    ) -> (u16, Vec<u8>) {
        http(&self.session, method, path, body, host, origin, true)
    }
    fn console(&self, action: &str, payload: Value) -> Value {
        let (status, body) = self.http(
            "POST",
            "/api/console",
            Some(
                if matches!(action, "rules_get" | "directories" | "retention_get") {
                    json!({"action":action})
                } else {
                    json!({"action":action,"payload":payload})
                },
            ),
            None,
            None,
        );
        assert_eq!(status, 200, "控制台请求未成功：{action}");
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["ok"], true, "{action}");
        value["data"].clone()
    }
    fn send_open(&self, index: usize, sequence: u64) {
        self.source
            .send(open_frame(
                &self.project.join(format!("source-{index:03}.txt")),
                sequence,
            ))
            .unwrap();
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.ui.kill();
        let _ = self.ui.wait();
        let _ = request_control(&self.host_socket, ControlRequest::Stop);
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.host_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.collector_thread.take() {
            let _ = thread.join();
        }
    }
}
fn open_frame(path: &Path, sequence: u64) -> CollectorFrame {
    let metadata = fs::metadata(path).unwrap();
    CollectorFrame::Line {source_stream:SourceStream::Combined,run_id:"synthetic-web-run".into(),received_timestamp_ms:now_ms(),line:json!({
        "schema_version":1,"version":9,"action_type":1,"event_type":10,"seq_num":sequence,"global_seq_num":sequence,
        "time":chrono::DateTime::from_timestamp_millis(now_ms()).unwrap().to_rfc3339(),
        "process":{"audit_token":{"pid":12345,"pidversion":1},"ppid":1,"executable":{"path":"/usr/bin/synthetic-reader","path_truncated":false}},
        "event":{"open":{"fflag":1,"file":{"path":path,"path_truncated":false,"stat":{"st_mode":metadata.mode(),"st_dev":metadata.dev(),"st_ino":metadata.ino()}}}}
    }).to_string()}
}
fn http(
    session: &Value,
    method: &str,
    path: &str,
    body: Option<Value>,
    host: Option<&str>,
    origin: Option<&str>,
    auth: bool,
) -> (u16, Vec<u8>) {
    http_with_metadata(session, method, path, body, (host, origin), None, auth)
}
fn http_with_metadata(
    session: &Value,
    method: &str,
    path: &str,
    body: Option<Value>,
    authority: (Option<&str>, Option<&str>),
    fetch_site: Option<&str>,
    auth: bool,
) -> (u16, Vec<u8>) {
    let (host, origin) = authority;
    let address = session["origin"]
        .as_str()
        .unwrap()
        .trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let text = body.map(|v| v.to_string()).unwrap_or_default();
    write!(stream,"{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nOrigin: {}\r\n",host.unwrap_or(address),origin.unwrap_or(session["origin"].as_str().unwrap())).unwrap();
    if let Some(fetch_site) = fetch_site {
        write!(stream, "Sec-Fetch-Site: {fetch_site}\r\n").unwrap();
    }
    if auth {
        write!(
            stream,
            "Authorization: Bearer {}\r\n",
            session["token"].as_str().unwrap()
        )
        .unwrap();
    }
    write!(stream, "Content-Length: {}\r\n\r\n{text}", text.len()).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let header = String::from_utf8_lossy(&response[..split]);
    let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut body = response[split + 4..].to_vec();
    if header.to_lowercase().contains("transfer-encoding: chunked") {
        let mut decoded = Vec::new();
        let mut offset = 0;
        loop {
            let end = body[offset..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .unwrap()
                + offset;
            let size = usize::from_str_radix(std::str::from_utf8(&body[offset..end]).unwrap(), 16)
                .unwrap();
            offset = end + 2;
            if size == 0 {
                break;
            }
            decoded.extend_from_slice(&body[offset..offset + size]);
            offset += size + 2;
        }
        body = decoded;
    }
    (status, body)
}

#[test]
fn loopback_http_enforces_credentials_origin_and_internal_operation_boundary() {
    let rig = Rig::new();
    let (status, html) = http(
        &rig.session,
        "GET",
        "/",
        None,
        None,
        Some("http://127.0.0.1"),
        false,
    );
    assert_eq!(status, 200);
    let html = String::from_utf8(html).unwrap();
    for attribute in ["src=\"", "href=\""] {
        for path in html
            .split(attribute)
            .skip(1)
            .filter_map(|part| part.split('"').next())
        {
            if path.starts_with("/assets/") {
                assert_eq!(
                    http(
                        &rig.session,
                        "GET",
                        path,
                        None,
                        None,
                        Some("http://127.0.0.1"),
                        false
                    )
                    .0,
                    200,
                );
            }
        }
    }
    assert_eq!(
        http(
            &rig.session,
            "GET",
            "/",
            None,
            Some("attacker.example"),
            None,
            false
        )
        .0,
        403
    );
    assert_eq!(
        http(&rig.session, "GET", "/api/status", None, None, None, false).0,
        401
    );
    assert_eq!(
        rig.http("GET", "/api/status", None, Some("attacker.example"), None)
            .0,
        403
    );
    assert_eq!(
        rig.http(
            "GET",
            "/api/status",
            None,
            None,
            Some("https://attacker.example")
        )
        .0,
        403
    );
    for action in ["monitoring_set", "record_operation"] {
        assert_eq!(
            rig.http(
                "POST",
                "/api/console",
                Some(json!({"action":action,"payload":{"paused":true}})),
                None,
                None
            )
            .0,
            403
        );
    }
    assert_eq!(
        rig.http(
            "POST",
            "/api/control",
            Some(json!({"operation":"stop"})),
            None,
            None
        )
        .0,
        403
    );
    assert_eq!(
        rig.http(
            "POST",
            "/api/service",
            Some(json!({"operation":"install"})),
            None,
            None
        )
        .0,
        403
    );
    assert_eq!(rig.http("GET", "/", None, None, None).0, 200);
    assert_eq!(rig.http("GET", "/api/unknown", None, None, None).0, 404);
}

#[test]
fn portless_browser_origin_requires_same_origin_metadata_and_credentials() {
    let rig = Rig::new();
    for (origin, fetch_site, auth, expected) in [
        ("http://127.0.0.1", Some("same-origin"), true, 200),
        ("http://127.0.0.1", Some("same-origin"), false, 401),
        ("http://127.0.0.1", None, true, 403),
        ("http://127.0.0.1", Some("same-site"), true, 403),
        ("http://127.0.0.1", Some("cross-site"), true, 403),
        ("http://127.0.0.1:1", Some("same-origin"), true, 403),
        ("https://attacker.example", Some("same-origin"), true, 403),
    ] {
        for (method, path, body) in [
            ("GET", "/api/status", None),
            ("POST", "/api/console", Some(json!({"action":"rules_get"}))),
        ] {
            assert_eq!(
                http_with_metadata(
                    &rig.session,
                    method,
                    path,
                    body,
                    (None, Some(origin)),
                    fetch_site,
                    auth,
                )
                .0,
                expected,
                "{method} 的来源检查错误：{origin} / {fetch_site:?}",
            );
        }
    }
}

#[test]
fn actual_host_pagination_export_rules_and_handling_are_connected() {
    let rig = Rig::new();
    for index in 0..125 {
        rig.send_open(index, index as u64 + 1);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let count =
            rig.console("events_page", json!({"filter":{},"archive_only":false}))["total"].as_u64();
        if count == Some(125) {
            break;
        }
        if Instant::now() > deadline {
            let response = request_control(&rig.host_socket, ControlRequest::Status).unwrap();
            let status = response.data.unwrap();
            panic!(
                "合成事件数量未匹配：count={count:?},observed={},persisted={},filtered={},db_state={}",
                status["observed_events_by_kind"],
                status["persisted_events_by_kind"],
                status["filtered_events_by_kind"],
                status["database_state"]
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    let first = rig.console("events_page", json!({"filter":{"limit":100}}));
    assert_eq!(first["items"].as_array().unwrap().len(), 100);
    let second = rig.console(
        "events_page",
        json!({"filter":{"limit":100},"cursor":first["next_cursor"]}),
    );
    assert_eq!(second["items"].as_array().unwrap().len(), 25);
    let (status, body) = rig.http(
        "POST",
        "/api/export",
        Some(json!({"kind":"events","filter":{},"format":"json","anonymous":false})),
        None,
        None,
    );
    assert_eq!(status, 200);
    let exported: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(exported.len(), 125);
    let (_, body) = rig.http(
        "POST",
        "/api/export",
        Some(json!({"kind":"events","filter":{},"format":"json","anonymous":true})),
        None,
        None,
    );
    assert_eq!(
        serde_json::from_slice::<Vec<Value>>(&body).unwrap().len(),
        125
    );
    assert!(!String::from_utf8_lossy(&body).contains(rig.project.to_str().unwrap()));
    let (_, body) = rig.http(
        "POST",
        "/api/export",
        Some(json!({"kind":"events","filter":{},"format":"csv","anonymous":false})),
        None,
        None,
    );
    assert_eq!(String::from_utf8(body).unwrap().lines().count(), 126);
    let alerts = rig.console("alerts_page", json!({"filter":{}}));
    assert!(!alerts["items"].as_array().unwrap().is_empty());
    let alert = &alerts["items"][0];
    let id = alert["alert"]["id"].clone();
    let (missing_status, missing_body) = rig.http(
        "POST",
        "/api/console",
        Some(json!({"action":"alert_detail","payload":{"id":"synthetic-missing"}})),
        None,
        None,
    );
    assert_eq!(missing_status, 404);
    assert_eq!(
        serde_json::from_slice::<Value>(&missing_body).unwrap()["error"],
        "alert_unavailable"
    );
    let processed=rig.console("alert_update",json!({"id":id,"is_read":true,"processed":true,"note":"synthetic note","expected_revision":alert["revision"]}));
    assert_eq!(processed["processed"], true);
    rig.send_open(125, 126);
    wait(|| rig.console("alert_detail", json!({"id":id}))["processed"] == false);
    let detail = rig.console("alert_detail", json!({"id":id}));
    assert_eq!(detail["handling_history"].as_array().unwrap().len(), 1);
    let mut settings = rig.console("rules_get", json!({}));
    settings["bulk_enabled"] = json!(false);
    let version = settings["version"].as_u64().unwrap();
    let saved = rig.console("rules_set", json!({"settings":settings}));
    assert_eq!(saved["version"], version + 1);
    let old = rig.console("alert_detail", json!({"id":id}));
    assert_eq!(old["rule_version"], version);
    let retention = rig.console("retention_get", json!({}));
    assert_eq!(retention["days"], 30);
    assert_eq!(
        rig.http(
            "POST",
            "/api/console",
            Some(json!({"action":"clear_details","payload":{"confirm":false}})),
            None,
            None
        )
        .0,
        400
    );
    rig.console("clear_details", json!({"confirm":true}));
    assert_eq!(rig.console("events_page", json!({"filter":{}}))["total"], 0);
    assert_eq!(rig.console("rules_get", json!({}))["version"], version + 1);
}

#[test]
fn history_snapshot_selection_and_host_unavailable_are_explicit() {
    let rig = Rig::new();
    let (_, body) = rig.http("POST", "/api/history/preview", Some(json!({})), None, None);
    let preview: Value = serde_json::from_slice(&body).unwrap();
    let candidates = preview["data"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 3);
    for source in ["codex", "claude_code", "zcode"] {
        assert!(candidates.iter().any(|entry| {
            entry["origins"]
                .as_array()
                .unwrap()
                .iter()
                .any(|origin| origin["source"] == source)
        }));
    }
    let (_, body) = rig.http(
        "POST",
        "/api/history/import",
        Some(json!({"preview_id":preview["data"]["preview_id"],"indices":[0,1,2]})),
        None,
        None,
    );
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["ok"], true);
    let directories = rig.console("directories", json!({}));
    let entries = directories.as_array().unwrap();
    assert_eq!(entries.len(), 3);
    for (path, source) in [
        (&rig.project, "codex"),
        (&rig.claude_project, "claude_code"),
        (&rig.zcode_project, "zcode"),
    ] {
        let entry = entries
            .iter()
            .find(|entry| entry["path"].as_str() == path.to_str())
            .expect("所选合成候选必须进入宿主配置");
        assert!(
            entry["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == source)
        );
    }
    assert_eq!(
        rig.http(
            "POST",
            "/api/history/import",
            Some(json!({"preview_id":"expired","indices":[0]})),
            None,
            None
        )
        .0,
        409
    );
    request_control(&rig.host_socket, ControlRequest::Stop).unwrap();
    wait(|| !rig.host_socket.exists());
    let (_, body) = rig.http("GET", "/api/status", None, None, None);
    let status: Value = serde_json::from_slice(&body).unwrap();
    assert!(status["data"]["host"].is_null());
    assert_eq!(status["data"]["host_error"], "host_unavailable");
    assert_eq!(
        rig.http(
            "POST",
            "/api/console",
            Some(json!({"action":"directories"})),
            None,
            None
        )
        .0,
        503
    );
}

#[test]
fn cli_detaches_reuses_private_entry_and_closing_ui_keeps_host_running() {
    let rig = Rig::new();
    let entry = rig.temp.path().join("secondary-ui.json");
    let open = || {
        Command::new(env!("CARGO_BIN_EXE_codeperimeter"))
            .arg("--host-socket")
            .arg(&rig.host_socket)
            .args(["ui", "--no-browser", "--session-file"])
            .arg(&entry)
            .output()
            .unwrap()
    };
    let first = open();
    assert!(first.status.success());
    let session: Value = serde_json::from_slice(&fs::read(&entry).unwrap()).unwrap();
    assert!(!String::from_utf8_lossy(&first.stdout).contains(session["token"].as_str().unwrap()));
    let second = open();
    assert!(second.status.success());
    let repeated: Value = serde_json::from_slice(&fs::read(&entry).unwrap()).unwrap();
    assert_eq!(session["pid"], repeated["pid"]);
    let mut legacy = repeated.clone();
    legacy["version"] = json!(1);
    fs::write(&entry, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(open().status.success());
    let upgraded: Value = serde_json::from_slice(&fs::read(&entry).unwrap()).unwrap();
    assert_ne!(
        upgraded["pid"], session["pid"],
        "旧入口不能处理通知详情，需启动新版页面"
    );
    let pid = session["pid"].as_u64().unwrap() as libc::pid_t;
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    assert!(entry.exists(), "旧服务退出不能移除新版入口");
    let new_pid = upgraded["pid"].as_u64().unwrap() as libc::pid_t;
    assert_eq!(unsafe { libc::kill(new_pid, libc::SIGTERM) }, 0);
    wait(|| !entry.exists());
    rig.send_open(0, 1);
    wait(|| rig.console("events_page", json!({"filter":{}}))["total"] == 1);
    assert!(
        request_control(&rig.host_socket, ControlRequest::Status)
            .unwrap()
            .ok
    );
}

#[test]
#[ignore = "仅供真实浏览器验收；独立私有fixture，无真实ES或系统服务修改"]
fn browser_fixture() {
    let rig = Rig::new();
    let destination =
        std::env::var_os("CODEPERIMETER_BROWSER_FIXTURE").expect("请指定项目外私有验收目录");
    let destination = PathBuf::from(destination);
    let metadata = fs::symlink_metadata(&destination).unwrap();
    assert!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0
    );
    fs::write(destination.join("browser.json"),json!({"session":rig.session,"project":rig.project,"claude_project":rig.claude_project,"zcode_project":rig.zcode_project,"root":rig.temp.path(),"host_socket":rig.host_socket}).to_string()).unwrap();
    fs::set_permissions(
        destination.join("browser.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    for index in 0..125 {
        rig.send_open(index, index as u64 + 1);
    }
    let deadline = Instant::now() + Duration::from_secs(1800);
    let mut sequence = 126;
    while Instant::now() < deadline && !destination.join("stop").exists() {
        if destination.join("inject").exists() {
            fs::remove_file(destination.join("inject")).unwrap();
            for index in 0..50 {
                rig.send_open(index, sequence);
                sequence += 1;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}
