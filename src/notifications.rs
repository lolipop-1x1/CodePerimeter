//! 普通用户的原生通知应用：显式授权、有界请求与系统提交回执。

use crate::Result;
use crate::runtime::NotificationTarget;
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const INFO: &[u8] = include_bytes!("../native/notifications/Info.plist");
const ICON: &[u8] = include_bytes!("../native/notifications/CodePerimeter.icns");
const MAX_REQUEST: usize = 16 * 1024;
const MAX_REPLY: usize = 2048;
const MAX_BINARY: usize = 16 * 1024 * 1024;
const EXECUTABLE: &str = "Contents/MacOS/CodePerimeterNotifications";
const SIGNATURE: &str = "Contents/_CodeSignature/CodeResources";
static INSTALL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationStatus {
    NotDetermined,
    Denied,
    Authorized,
    Provisional,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct NotificationStatus {
    pub authorization: AuthorizationStatus,
    pub alerts_enabled: bool,
}

#[derive(Serialize)]
struct Request<'a> {
    version: u8,
    operation: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<&'a NotificationTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u8,
    ok: bool,
    code: String,
    authorization: Option<AuthorizationStatus>,
    alerts_enabled: Option<bool>,
}

pub struct NativeNotificationSender {
    executable: PathBuf,
}

impl NativeNotificationSender {
    /// 部署与注册固定 .app，不调用通知授权，也不申请完全磁盘访问。
    pub fn new(helper_binary: &[u8]) -> Result<Self> {
        if helper_binary.is_empty() || helper_binary.len() > MAX_BINARY {
            return Err(failure("notification_binary_invalid"));
        }
        let (uid, home) = user_home()?;
        let mut directory = home;
        checked_directory(&directory, uid)?;
        for component in [
            "Library",
            "Application Support",
            "CodePerimeter",
            "Notifications",
        ] {
            directory.push(component);
            create_private_directory(&directory, uid)?;
        }
        let application = install_bundle(&directory, helper_binary, uid)?;
        let mut register = Command::new(
            "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
        );
        register.arg("-f").arg(&application);
        fixed_command(&mut register, "notification_registration_failed")?;
        Ok(Self {
            executable: application.join(EXECUTABLE),
        })
    }

    /// 只有通知会话明确要求时调用；拒绝、关闭提醒或超时均返回失败。
    pub fn request_authorization(&self) -> Result<()> {
        self.request("authorize", None, None, None, Duration::from_secs(65))?;
        Ok(())
    }

    /// 状态查询不会触发权限窗口。
    pub fn status(&self) -> Result<NotificationStatus> {
        let reply = self.request("status", None, None, None, Duration::from_secs(6))?;
        Ok(NotificationStatus {
            authorization: reply
                .authorization
                .ok_or_else(|| failure("notification_reply_invalid"))?,
            alerts_enabled: reply
                .alerts_enabled
                .ok_or_else(|| failure("notification_reply_invalid"))?,
        })
    }

    /// 成功仅表示 UNUserNotificationCenter 接受提交，不等同于到屏。
    pub fn send(&self, title: &str, body: &str) -> Result<()> {
        self.send_to(title, body, &NotificationTarget::Alerts)
    }

    pub fn send_to(&self, title: &str, body: &str, target: &NotificationTarget) -> Result<()> {
        if title.is_empty() || title.len() > 512 || body.is_empty() || body.len() > 8192 {
            return Err(failure("notification_request_invalid"));
        }
        if let NotificationTarget::Alert { id } = target
            && !crate::model::valid_alert_id(id)
        {
            return Err(failure("notification_target_invalid"));
        }
        self.request(
            "send",
            Some(title),
            Some(body),
            Some(target),
            Duration::from_secs(6),
        )?;
        Ok(())
    }

    fn request(
        &self,
        operation: &str,
        title: Option<&str>,
        body: Option<&str>,
        target: Option<&NotificationTarget>,
        timeout: Duration,
    ) -> Result<Reply> {
        let bytes = serde_json::to_vec(&Request {
            version: 1,
            operation,
            title,
            body,
            target,
        })?;
        if bytes.len() > MAX_REQUEST {
            return Err(failure("notification_request_invalid"));
        }
        let deadline = Instant::now() + timeout;
        let mut child = Command::new(&self.executable)
            .arg("--request-stdin")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| failure("notification_helper_unavailable"))?;
        let mut input = child.stdin.take().expect("已创建 stdin 管道");
        let output = child.stdout.take().expect("已创建 stdout 管道");
        // 两端各自有界；超时回收子进程后关闭管道并 join，不遗留写线程。
        let writer = thread::spawn(move || input.write_all(&bytes));
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            output
                .take((MAX_REPLY + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        });
        let mut status = wait_child(
            &mut child,
            deadline.saturating_duration_since(Instant::now()),
        );
        while status.is_ok() && (!writer.is_finished() || !reader.is_finished()) {
            if Instant::now() >= deadline {
                stop_child(&mut child);
                status = Err(failure("notification_timeout"));
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let written = writer.join();
        let received = reader.join();
        let status = status?;
        written
            .map_err(|_| failure("notification_io_failed"))?
            .map_err(|_| failure("notification_io_failed"))?;
        let received = received
            .map_err(|_| failure("notification_io_failed"))?
            .map_err(|_| failure("notification_io_failed"))?;
        parse_reply(&received, status.success(), operation)
    }
}

impl crate::runtime::NotificationSender for NativeNotificationSender {
    fn send(&mut self, title: &str, body: &str, target: &NotificationTarget) -> Result<()> {
        self.send_to(title, body, target)
    }
}

fn failure(code: &'static str) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::other(code).into()
}

fn parse_reply(bytes: &[u8], successful_exit: bool, operation: &str) -> Result<Reply> {
    if bytes.len() > MAX_REPLY {
        return Err(failure("notification_reply_invalid"));
    }
    let reply: Reply =
        serde_json::from_slice(bytes).map_err(|_| failure("notification_reply_invalid"))?;
    if reply.version != 1 {
        return Err(failure("notification_reply_invalid"));
    }
    if !reply.ok {
        return Err(failure(match reply.code.as_str() {
            "permission_denied" => "notification_permission_denied",
            "authorization_required" => "notification_authorization_required",
            "alerts_disabled" => "notification_alerts_disabled",
            "timed_out" => "notification_timeout",
            _ => "notification_submission_failed",
        }));
    }
    let expected = match operation {
        "authorize" => "authorized",
        "status" => "status",
        "send" => "accepted",
        _ => return Err(failure("notification_request_invalid")),
    };
    if !successful_exit || reply.code != expected {
        return Err(failure("notification_reply_invalid"));
    }
    if operation == "authorize"
        && (reply.authorization != Some(AuthorizationStatus::Authorized)
            || reply.alerts_enabled != Some(true))
    {
        return Err(failure("notification_reply_invalid"));
    }
    Ok(reply)
}

fn wait_child(child: &mut Child, timeout: Duration) -> Result<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                stop_child(child);
                let _ = child.wait();
                return Err(failure("notification_timeout"));
            }
            Err(_) => {
                stop_child(child);
                let _ = child.wait();
                return Err(failure("notification_helper_failed"));
            }
        }
    }
}

fn stop_child(child: &mut Child) {
    // 仅作用于本次建立的独立进程组，避免子进程继承管道导致 join 卡住。
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
}

fn fixed_command(command: &mut Command, error: &'static str) -> Result<()> {
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| failure(error))?;
    if wait_child(&mut child, Duration::from_secs(15))?.success() {
        Ok(())
    } else {
        Err(failure(error))
    }
}

fn user_home() -> Result<(u32, PathBuf)> {
    let uid = unsafe { libc::geteuid() };
    if uid == 0 {
        return Err(failure("notification_requires_user_session"));
    }
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut found = std::ptr::null_mut();
    let mut buffer = vec![0u8; 16 * 1024];
    let result = unsafe {
        libc::getpwuid_r(
            uid,
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if result != 0 || found.is_null() {
        return Err(failure("notification_user_unavailable"));
    }
    let entry = unsafe { entry.assume_init() };
    if entry.pw_dir.is_null() {
        return Err(failure("notification_user_unavailable"));
    }
    let home = PathBuf::from(std::ffi::OsStr::from_bytes(unsafe {
        CStr::from_ptr(entry.pw_dir).to_bytes()
    }));
    if !home.is_absolute() {
        return Err(failure("notification_user_unavailable"));
    }
    Ok((uid, home))
}

fn checked_directory(path: &Path, uid: u32) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| failure("notification_directory_unavailable"))?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(failure("notification_directory_untrusted"));
    }
    Ok(())
}

fn create_private_directory(path: &Path, uid: u32) -> Result<()> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(failure("notification_directory_unavailable")),
    }
    checked_directory(path, uid)
}

fn write_private(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if executable { 0o700 } else { 0o600 })
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|_| failure("notification_deployment_failed"))?;
    Ok(())
}

struct StagedBundle {
    path: PathBuf,
    cleanup: bool,
}

impl Drop for StagedBundle {
    fn drop(&mut self) {
        // 交换回来的旧包不进入无条件清理，未知用户内容必须保留。
        if self.cleanup {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn install_bundle(directory: &Path, binary: &[u8], uid: u32) -> Result<PathBuf> {
    let application = directory.join("CodePerimeter.app");
    if application.try_exists()? {
        checked_bundle(&application, uid)?;
    } else if fs::symlink_metadata(&application).is_ok() {
        return Err(failure("notification_bundle_untrusted"));
    }
    let stage = directory.join(format!(
        ".notification-install-{}-{}",
        std::process::id(),
        INSTALL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    DirBuilder::new().mode(0o700).create(&stage)?;
    let mut stage = StagedBundle {
        path: stage,
        cleanup: true,
    };
    for component in ["Contents", "Contents/MacOS", "Contents/Resources"] {
        create_private_directory(&stage.path.join(component), uid)?;
    }
    write_private(&stage.path.join("Contents/Info.plist"), INFO, false)?;
    write_private(&stage.path.join(EXECUTABLE), binary, true)?;
    write_private(
        &stage.path.join("Contents/Resources/CodePerimeter.icns"),
        ICON,
        false,
    )?;
    let mut sign = Command::new("/usr/bin/codesign");
    sign.args([
        "--force",
        "--sign",
        "-",
        "--identifier",
        "com.codeperimeter.notifications",
        "--requirements",
        "=designated => identifier \"com.codeperimeter.notifications\"",
    ])
    .arg(&stage.path);
    fixed_command(&mut sign, "notification_signing_failed")?;
    verify_signature(&stage.path)?;
    replace_bundle(&mut stage, &application, uid)?;
    Ok(application)
}

fn replace_bundle(stage: &mut StagedBundle, application: &Path, uid: u32) -> Result<()> {
    if application.try_exists()? {
        // 签署期间旧包可能变化，交换前重新确认范围与签名。
        checked_bundle(application, uid)?;
        // 签名会改变 Mach-O，比较已签名资源，不能与未签名嵌入字节直接比。
        if bundles_equal(application, &stage.path)? {
            return Ok(());
        }
        swap_bundle(&stage.path, application)?;
        stage.cleanup = false;
        cleanup_replaced_bundle(stage, application, uid)?;
    } else {
        // 不存在检查之后若出现其他目录，不覆盖它；首次安装也使用原子排他移动。
        let from = CString::new(stage.path.as_os_str().as_bytes())?;
        let to = CString::new(application.as_os_str().as_bytes())?;
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } != 0 {
            return Err(failure("notification_deployment_failed"));
        }
    }
    Ok(())
}

fn swap_bundle(first: &Path, second: &Path) -> Result<()> {
    let from = CString::new(first.as_os_str().as_bytes())?;
    let to = CString::new(second.as_os_str().as_bytes())?;
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(failure("notification_deployment_failed"));
    }
    Ok(())
}

fn cleanup_replaced_bundle(stage: &StagedBundle, application: &Path, uid: u32) -> Result<()> {
    if checked_bundle(&stage.path, uid).is_err() {
        // 未知内容回到原位置；回退失败时仍保留两份目录，不删除任何一份。
        if checked_bundle(application, uid).is_ok() {
            let _ = swap_bundle(&stage.path, application);
        }
        return Err(failure("notification_bundle_changed"));
    }
    fs::remove_dir_all(&stage.path).map_err(|_| failure("notification_cleanup_failed"))?;
    Ok(())
}

fn verify_signature(path: &Path) -> Result<()> {
    let mut verify = Command::new("/usr/bin/codesign");
    verify.args(["--verify", "--strict"]).arg(path);
    fixed_command(&mut verify, "notification_signature_invalid")
}

fn checked_bundle(application: &Path, uid: u32) -> Result<()> {
    for (relative, expected) in [
        ("", &["Contents"][..]),
        (
            "Contents",
            &["Info.plist", "MacOS", "Resources", "_CodeSignature"][..],
        ),
        ("Contents/MacOS", &["CodePerimeterNotifications"][..]),
        ("Contents/Resources", &["CodePerimeter.icns"][..]),
        ("Contents/_CodeSignature", &["CodeResources"][..]),
    ] {
        let directory = application.join(relative);
        checked_directory(&directory, uid)?;
        let entries = fs::read_dir(&directory)?.collect::<io::Result<Vec<_>>>()?;
        if entries.len() != expected.len() {
            return Err(failure("notification_bundle_untrusted"));
        }
        for entry in entries {
            if !expected.iter().any(|name| entry.file_name() == *name) {
                return Err(failure("notification_bundle_untrusted"));
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink()
                || metadata.uid() != uid
                || metadata.mode() & 0o022 != 0
                || (!metadata.is_dir() && !metadata.is_file())
            {
                return Err(failure("notification_bundle_untrusted"));
            }
        }
    }
    if bounded_file(&application.join("Contents/Info.plist"))? != INFO {
        return Err(failure("notification_bundle_untrusted"));
    }
    verify_signature(application)
}

fn bounded_file(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((MAX_BINARY + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BINARY {
        return Err(failure("notification_bundle_untrusted"));
    }
    Ok(bytes)
}

fn bundles_equal(first: &Path, second: &Path) -> Result<bool> {
    for relative in [
        "Contents/Info.plist",
        EXECUTABLE,
        "Contents/Resources/CodePerimeter.icns",
        SIGNATURE,
    ] {
        if bounded_file(&first.join(relative))? != bounded_file(&second.join(relative))? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn fixture(directory: &Path, source: &str) -> NativeNotificationSender {
        let executable = directory.join("anonymous-helper");
        write_private(&executable, source.as_bytes(), true).unwrap();
        NativeNotificationSender { executable }
    }

    #[test]
    fn request_uses_stdin_and_accepts_only_submission_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let sender = fixture(
            directory.path(),
            "#!/bin/sh\n[ \"$#\" -eq 1 ] && [ \"$1\" = --request-stdin ] || exit 3\nIFS= read -r request\ncase \"$request\" in *'\"title\":\"匿名告警\"'*'\"body\":\"合成证据\"'*) ;; *) exit 4;; esac\nprintf '%s\\n' '{\"version\":1,\"ok\":true,\"code\":\"accepted\"}'\n",
        );
        sender.send("匿名告警", "合成证据").unwrap();
        assert!(sender.send("", "合成证据").is_err());
        assert!(sender.send("匿名告警", &"x".repeat(8193)).is_err());
        // JSON 转义后的总量也有界，不只检查原始字符串字节。
        assert!(sender.send("匿名告警", &"\u{0001}".repeat(8192)).is_err());
    }

    #[test]
    fn alert_navigation_is_bounded_stdin_metadata_without_web_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let sender = fixture(
            directory.path(),
            "#!/bin/sh\nIFS= read -r request\ncase \"$request\" in *'\"target\":{\"kind\":\"alert\",\"id\":\"synthetic-run:1:v1:archive_output:engine:1\"}'*) ;; *) exit 4;; esac\ncase \"$request\" in *token*|*http*|*launcher*) exit 5;; esac\nprintf '%s\\n' '{\"version\":1,\"ok\":true,\"code\":\"accepted\"}'\n",
        );
        sender
            .send_to(
                "合成告警",
                "点击查看详情",
                &NotificationTarget::Alert {
                    id: "synthetic-run:1:v1:archive_output:engine:1".into(),
                },
            )
            .unwrap();
        for id in [
            "",
            "https://attacker.example",
            "../other",
            "bad\nidentifier",
            &"x".repeat(513),
        ] {
            assert_eq!(
                sender
                    .send_to(
                        "合成告警",
                        "点击查看详情",
                        &NotificationTarget::Alert { id: id.into() }
                    )
                    .unwrap_err()
                    .to_string(),
                "notification_target_invalid"
            );
        }
    }

    #[test]
    fn denial_and_untrusted_replies_are_failures_without_raw_error() {
        let denied = br#"{"version":1,"ok":false,"code":"permission_denied"}"#;
        assert_eq!(
            parse_reply(denied, false, "send").unwrap_err().to_string(),
            "notification_permission_denied"
        );
        for reply in [
            br#"{"version":1,"ok":false,"code":"anonymous_private_detail"}"#.as_slice(),
            br#"{"version":2,"ok":true,"code":"accepted"}"#,
            br#"{"version":1,"ok":true,"code":"status"}"#,
            br#"{"version":1,"ok":true,"code":"accepted","extra":"anonymous"}"#,
            b"malformed",
        ] {
            let error = parse_reply(reply, true, "send").unwrap_err().to_string();
            assert!(!error.contains("anonymous_private_detail"));
        }
        assert!(
            parse_reply(
                br#"{"version":1,"ok":true,"code":"accepted"}"#,
                false,
                "send"
            )
            .is_err()
        );
        assert!(parse_reply(&vec![b'x'; MAX_REPLY + 1], true, "send").is_err());
        assert!(parse_reply(
            br#"{"version":1,"ok":true,"code":"authorized","authorization":"denied","alerts_enabled":true}"#,
            true,
            "authorize"
        )
        .is_err());
    }

    #[test]
    fn status_does_not_request_authorization_and_timeout_reaps_owned_group() {
        let directory = tempfile::tempdir().unwrap();
        let sender = fixture(
            directory.path(),
            "#!/bin/sh\nIFS= read -r request\ncase \"$request\" in *'\"operation\":\"status\"'*) ;; *) exit 2;; esac\nprintf '%s\\n' '{\"version\":1,\"ok\":true,\"code\":\"status\",\"authorization\":\"not_determined\",\"alerts_enabled\":false}'\n",
        );
        assert_eq!(
            sender.status().unwrap(),
            NotificationStatus {
                authorization: AuthorizationStatus::NotDetermined,
                alerts_enabled: false,
            }
        );
        let stalled_directory = tempfile::tempdir().unwrap();
        let stalled = fixture(
            stalled_directory.path(),
            "#!/bin/sh\n/bin/sleep 30 &\nwait\n",
        );
        let started = Instant::now();
        let error = stalled
            .request("status", None, None, None, Duration::from_millis(100))
            .unwrap_err();
        assert_eq!(error.to_string(), "notification_timeout");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn private_directories_reject_links_foreign_owner_and_shared_write() {
        let directory = tempfile::tempdir().unwrap();
        let uid = unsafe { libc::geteuid() };
        let private = directory.path().join("private");
        create_private_directory(&private, uid).unwrap();
        assert!(checked_directory(&private, uid.wrapping_add(1)).is_err());
        let link = directory.path().join("linked");
        symlink(&private, &link).unwrap();
        assert!(create_private_directory(&link, uid).is_err());
        fs::set_permissions(&private, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(create_private_directory(&private, uid).is_err());
    }

    #[test]
    fn accepted_parent_exit_does_not_bypass_pipe_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let sender = fixture(
            directory.path(),
            "#!/bin/sh\nIFS= read -r request\n/bin/sleep 30 &\nprintf '%s\\n' '{\"version\":1,\"ok\":true,\"code\":\"accepted\"}'\nexit 0\n",
        );
        let started = Instant::now();
        let error = sender
            .request(
                "send",
                Some("匿名告警"),
                Some("合成证据"),
                None,
                Duration::from_millis(100),
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "notification_timeout");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn changed_bundle_is_preserved_before_exchange_and_before_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let uid = unsafe { libc::geteuid() };
        let binary = fs::read("/usr/bin/true").unwrap();
        let application = install_bundle(directory.path(), &binary, uid).unwrap();
        let stage_parent = directory.path().join("anonymous-stage-parent");
        create_private_directory(&stage_parent, uid).unwrap();
        let replacement = fs::read("/usr/bin/false").unwrap();
        let staged = install_bundle(&stage_parent, &replacement, uid).unwrap();
        let mut stage = StagedBundle {
            path: staged,
            cleanup: true,
        };
        checked_bundle(&application, uid).unwrap();
        // 模拟最初核验与签署完成之间添加用户文件。
        let unknown = application.join("anonymous-user-file");
        write_private(&unknown, b"preserve-before-swap", false).unwrap();
        assert!(replace_bundle(&mut stage, &application, uid).is_err());
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve-before-swap");
        fs::remove_file(&unknown).unwrap();
        swap_bundle(&stage.path, &application).unwrap();
        stage.cleanup = false;
        // 模拟交换之后旧目录又出现未知内容，不能让 Drop 删除它。
        write_private(
            &stage.path.join("anonymous-user-file"),
            b"preserve-before-cleanup",
            false,
        )
        .unwrap();
        assert_eq!(
            cleanup_replaced_bundle(&stage, &application, uid)
                .unwrap_err()
                .to_string(),
            "notification_bundle_changed"
        );
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve-before-cleanup");
        let retained = stage.path.clone();
        drop(stage);
        assert!(retained.exists());
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve-before-cleanup");
    }

    #[test]
    fn bundle_install_is_private_reusable_and_refuses_unknown_content() {
        // 仅在临时目录签署 /usr/bin/true 的副本；不启动应用、不注册、不授权。
        let directory = tempfile::tempdir().unwrap();
        let uid = unsafe { libc::geteuid() };
        let binary = fs::read("/usr/bin/true").unwrap();
        let application = install_bundle(directory.path(), &binary, uid).unwrap();
        checked_bundle(&application, uid).unwrap();
        let first = bounded_file(&application.join(EXECUTABLE)).unwrap();
        assert_eq!(
            install_bundle(directory.path(), &binary, uid).unwrap(),
            application
        );
        assert_eq!(bounded_file(&application.join(EXECUTABLE)).unwrap(), first);
        let different_binary = fs::read("/usr/bin/false").unwrap();
        install_bundle(directory.path(), &different_binary, uid).unwrap();
        checked_bundle(&application, uid).unwrap();
        assert_ne!(bounded_file(&application.join(EXECUTABLE)).unwrap(), first);
        let unknown = application.join("anonymous-user-file");
        write_private(&unknown, b"preserve", false).unwrap();
        assert!(install_bundle(directory.path(), &binary, uid).is_err());
        assert_eq!(fs::read(unknown).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
