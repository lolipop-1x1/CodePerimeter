use codeperimeter::service::{
    CollectorClient, CollectorFrame, MAX_FRAME_BYTES, ServicePlan, checked_root_path, peer_uid,
    read_bounded_line, verify_peer_uid, write_frame,
};
use std::fs;
use std::io::{self, Cursor, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;
use tempfile::TempDir;

fn test_listener() -> (TempDir, UnixListener, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("collector.sock");
    let listener = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    (directory, listener, path)
}

#[test]
fn verifies_real_unix_peer_uid_in_both_directions() {
    let (client, server) = UnixStream::pair().unwrap();
    let uid = unsafe { libc::geteuid() };
    assert_eq!(peer_uid(&client).unwrap(), uid);
    assert_eq!(peer_uid(&server).unwrap(), uid);
    verify_peer_uid(&client, uid).unwrap();
    verify_peer_uid(&server, uid).unwrap();
    assert_eq!(
        verify_peer_uid(&server, uid.wrapping_add(1))
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn transports_explicit_test_frames_and_detects_eof() {
    let (_directory, listener, path) = test_listener();
    let uid = unsafe { libc::geteuid() };
    let frames = vec![
        CollectorFrame::Status {
            run_id: "test-only-run".into(),
            state: "test_mode".into(),
            message: "匿名替身，未启动 eslogger".into(),
            dropped_lines: 0,
        },
        CollectorFrame::Line {
            run_id: "test-only-run".into(),
            line: "{\"synthetic\":true}".into(),
            received_timestamp_ms: 100,
        },
        CollectorFrame::Heartbeat {
            run_id: "test-only-run".into(),
            dropped_lines: 2,
        },
        CollectorFrame::Status {
            run_id: "test-only-run".into(),
            state: "coverage_gap".into(),
            message: "合成缺口".into(),
            dropped_lines: 2,
        },
    ];
    let expected = frames.clone();
    let producer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        verify_peer_uid(&stream, uid).unwrap();
        for frame in &frames {
            write_frame(&mut stream, frame).unwrap();
        }
    });
    // expected UID 显式为本机用户；生产 connect 始终要求 root。
    let mut client = CollectorClient::connect_expected(&path, uid).unwrap();
    for frame in expected {
        assert_eq!(client.read_frame().unwrap(), frame);
    }
    assert_eq!(
        client.read_frame().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    producer.join().unwrap();
}

#[test]
fn production_client_requires_root_owner_and_peer() {
    let (_directory, _listener, path) = test_listener();
    if unsafe { libc::geteuid() } != 0 {
        assert!(
            matches!(CollectorClient::connect(&path), Err(error) if error.kind() == io::ErrorKind::PermissionDenied)
        );
    }
    assert!(
        matches!(CollectorClient::connect_expected(&path, unsafe { libc::geteuid() }.wrapping_add(1)), Err(error) if error.kind() == io::ErrorKind::PermissionDenied)
    );
}

#[test]
fn rejects_public_socket_symlink_and_mutable_parent() {
    let (directory, _listener, path) = test_listener();
    let uid = unsafe { libc::geteuid() };
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(CollectorClient::connect_expected(&path, uid).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let alias = directory.path().join("alias.sock");
    symlink(&path, &alias).unwrap();
    assert!(CollectorClient::connect_expected(&alias, uid).is_err());
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(CollectorClient::connect_expected(&path, uid).is_err());
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn skips_an_oversized_line_and_recovers_at_next_boundary() {
    let mut reader = Cursor::new(b"1234567890123\nnext\nlast".as_slice());
    assert_eq!(
        read_bounded_line(&mut reader, 8).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        read_bounded_line(&mut reader, 8).unwrap(),
        Some(b"next".to_vec())
    );
    assert_eq!(
        read_bounded_line(&mut reader, 8).unwrap(),
        Some(b"last".to_vec())
    );
    assert_eq!(read_bounded_line(&mut reader, 8).unwrap(), None);
}

#[test]
fn malformed_frames_do_not_echo_raw_values() {
    let (_directory, listener, path) = test_listener();
    let producer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .write_all(b"{\"synthetic-private-marker\":\n")
            .unwrap();
    });
    let mut client = CollectorClient::connect_expected(&path, unsafe { libc::geteuid() }).unwrap();
    let error = client.read_frame().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(!error.to_string().contains("synthetic-private-marker"));
    producer.join().unwrap();
}

#[test]
fn oversized_frames_are_rejected_and_reader_recovers() {
    let (_directory, listener, path) = test_listener();
    let producer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let oversized = vec![b'x'; MAX_FRAME_BYTES + 1];
        stream.write_all(&oversized).unwrap();
        stream.write_all(b"\n").unwrap();
        write_frame(
            &mut stream,
            &CollectorFrame::Heartbeat {
                run_id: "test-only-run".into(),
                dropped_lines: 0,
            },
        )
        .unwrap();
    });
    let mut client = CollectorClient::connect_expected(&path, unsafe { libc::geteuid() }).unwrap();
    assert_eq!(
        client.read_frame().unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(matches!(
        client.read_frame().unwrap(),
        CollectorFrame::Heartbeat { .. }
    ));
    producer.join().unwrap();
}

#[test]
fn service_plan_separates_root_daemon_and_desktop_roles() {
    let plan =
        ServicePlan::new("nobody", Path::new("/private/tmp/synthetic-codeperimeter")).unwrap();
    assert_eq!(plan.jobs.len(), 3);
    assert!(plan.installed_binary.starts_with("/Library/CodePerimeter"));
    assert_ne!(plan.installed_binary, plan.source_binary);
    assert_eq!(
        plan.collector_socket,
        plan.installed_binary
            .parent()
            .unwrap()
            .join("run/collector.sock")
    );
    assert!(!plan.collector_socket.starts_with("/var/run"));
    let collector = &plan.jobs[0];
    assert_eq!(collector.domain, "system");
    assert_eq!(collector.argv[1], "collector");
    assert_eq!(collector.argv[2], "--socket");
    assert_eq!(collector.argv[4], "--allowed-uid");
    assert!(!collector.plist.contains("<key>UserName</key>"));
    let daemon = &plan.jobs[1];
    assert_eq!(daemon.domain, "system");
    assert_eq!(daemon.argv[1], "daemon");
    assert!(
        daemon
            .plist
            .contains("<key>UserName</key><string>nobody</string>")
    );
    assert!(daemon.argv.iter().any(|arg| arg == "--control-socket"));
    assert!(daemon.argv.iter().any(|arg| arg == "--db"));
    let notification = &plan.jobs[2];
    assert!(notification.domain.starts_with("gui/"));
    assert_eq!(notification.argv[1], "notify");
    assert!(notification.plist.contains("<string>Aqua</string>"));
    for job in &plan.jobs {
        assert_eq!(job.argv[0], plan.installed_binary.to_string_lossy());
        assert!(job.plist.contains("<key>KeepAlive</key><true/>"));
        assert!(
            job.plist
                .contains("<key>StandardOutPath</key><string>/dev/null</string>")
        );
    }
}

#[test]
fn installation_rejects_tampered_plan_before_writing() {
    let mut plan =
        ServicePlan::new("nobody", Path::new("/private/tmp/synthetic-codeperimeter")).unwrap();
    plan.installed_binary = "/private/tmp/user-modifiable-executable".into();
    assert!(plan.install().is_err());
    assert!(ServicePlan::new("root", Path::new("/private/tmp/synthetic-codeperimeter")).is_err());
    assert!(ServicePlan::new("nobody", Path::new("relative-binary")).is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic-binary");
    fs::write(&path, "synthetic").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(checked_root_path(&path).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn mutable_system_run_directory_remains_rejected() {
    use std::os::unix::fs::MetadataExt;
    let path = fs::canonicalize("/var/run").unwrap();
    let metadata = fs::symlink_metadata(&path).unwrap();
    // 首跑机器为 root:daemon 0775；严格校验仍拒绝这一真实系统布局。
    if metadata.mode() & 0o022 != 0 {
        assert_eq!(
            checked_root_path(&path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    assert!(checked_root_path(Path::new("/Library")).is_ok());
}

#[cfg(target_os = "macos")]
#[test]
fn generated_plists_pass_native_plutil() {
    let plan =
        ServicePlan::new("nobody", Path::new("/private/tmp/synthetic-codeperimeter")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    for (index, job) in plan.jobs.iter().enumerate() {
        let path = directory.path().join(format!("job-{index}.plist"));
        fs::write(&path, &job.plist).unwrap();
        assert!(
            std::process::Command::new("/usr/bin/plutil")
                .args(["-lint", "--"])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }
}
