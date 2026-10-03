//! 系统采集桥接与 launchd 管理；这里不解析事件、不写 SQLite。

use crate::Result;
use crate::eslogger::{MAX_LINE_BYTES, SUBSCRIBED_EVENTS};
use crate::model::now_ms;
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

pub const MAX_FRAME_BYTES: usize = MAX_LINE_BYTES * 6 + 4096;
const QUEUE_CAPACITY: usize = 32;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorOptions {
    pub socket_path: PathBuf,
    pub allowed_uid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CollectorFrame {
    Line {
        run_id: String,
        line: String,
        received_timestamp_ms: i64,
    },
    Heartbeat {
        run_id: String,
        dropped_lines: u64,
    },
    Status {
        run_id: String,
        state: String,
        message: String,
        dropped_lines: u64,
    },
}

/// 读取直到换行；超限行继续排空但不继续分配内存。
pub fn read_bounded_line(reader: &mut impl BufRead, maximum: usize) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if oversized {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "输入行超过上限"));
            }
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(bytes))
            };
        }
        let end = available.iter().position(|byte| *byte == b'\n');
        let count = end.map_or(available.len(), |position| position + 1);
        if !oversized {
            if bytes.len().saturating_add(count) > maximum {
                oversized = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(&available[..count]);
            }
        }
        reader.consume(count);
        if end.is_some() {
            if oversized {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "输入行超过上限"));
            }
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return Ok(Some(bytes));
        }
    }
}

pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    #[cfg(target_os = "macos")]
    {
        let mut uid = 0;
        let mut gid = 0;
        // getpeereid 返回由内核绑定到此 Unix 连接的有效身份。
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
    #[cfg(target_os = "linux")]
    {
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut _ as *mut _,
                &mut length,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(credentials.uid)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = stream;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "此平台未实现 Unix peer 身份核验",
        ))
    }
}

pub fn verify_peer_uid(stream: &UnixStream, expected_uid: u32) -> io::Result<()> {
    if peer_uid(stream)? != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Unix socket 对端身份不符",
        ));
    }
    Ok(())
}

pub struct CollectorClient {
    reader: BufReader<UnixStream>,
}

impl CollectorClient {
    pub fn connect(socket_path: &Path) -> io::Result<Self> {
        Self::connect_expected(socket_path, 0)
    }

    /// 显式替身测试入口；生产宿主必须调用 connect，不能把用户身份当成 root。
    pub fn connect_expected(socket_path: &Path, expected_uid: u32) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(socket_path)?;
        let parent = fs::symlink_metadata(socket_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "socket 路径没有父目录")
        })?)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != expected_uid
            || metadata.mode() & 0o007 != 0
            || !parent.is_dir()
            || parent.uid() != expected_uid
            || parent.mode() & 0o022 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "采集 socket 路径或权限不可信",
            ));
        }
        let stream = UnixStream::connect(socket_path)?;
        verify_peer_uid(&stream, expected_uid)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    pub fn read_frame(&mut self) -> io::Result<CollectorFrame> {
        let bytes = read_bounded_line(&mut self.reader, MAX_FRAME_BYTES + 1)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "采集连接已断开"))?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "采集 frame 超过上限",
            ));
        }
        let frame: CollectorFrame = serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "采集 frame 格式不受支持"))?;
        if let CollectorFrame::Line { line, .. } = &frame {
            if line.len() > MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "采集事件超过上限",
                ));
            }
        }
        Ok(frame)
    }
}

pub fn write_frame(stream: &mut UnixStream, frame: &CollectorFrame) -> io::Result<()> {
    let bytes = serde_json::to_vec(frame).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "采集 frame 超过上限",
        ));
    }
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")
}

fn require_root() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "此操作需要管理员权限",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

pub fn lookup_account(username: &str) -> io::Result<Account> {
    let name = CString::new(username)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "账户名称不合法"))?;
    let mut record: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0u8; 64 * 1024];
    let status = unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            &mut record,
            buffer.as_mut_ptr() as *mut _,
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status));
    }
    if result.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "目标本地账户不存在",
        ));
    }
    Ok(Account {
        name: unsafe { CStr::from_ptr(record.pw_name) }
            .to_string_lossy()
            .into_owned(),
        uid: record.pw_uid,
        gid: record.pw_gid,
        home: PathBuf::from(
            unsafe { CStr::from_ptr(record.pw_dir) }
                .to_string_lossy()
                .into_owned(),
        ),
    })
}

fn account_for_uid(uid: u32) -> io::Result<Account> {
    let mut record: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0u8; 64 * 1024];
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut record,
            buffer.as_mut_ptr() as *mut _,
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "目标 UID 没有本地账户",
        ));
    }
    lookup_account(
        unsafe { CStr::from_ptr(record.pw_name) }
            .to_str()
            .map_err(io::Error::other)?,
    )
}

fn set_owner(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "路径包含 NUL"))?;
    if unsafe { libc::chown(path.as_ptr(), uid, gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn checked_directory(path: &Path, uid: u32, gid: u32, mode: u32) -> io::Result<()> {
    let created = match fs::create_dir(path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error),
    };
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || !created && (metadata.uid() != uid || metadata.mode() & 0o022 != 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "服务目录不是受信任的私有目录",
        ));
    }
    set_owner(path, uid, gid)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    if uid == 0 {
        clear_managed_acl(path)?;
    }
    Ok(())
}

fn bind_collector(socket_path: &Path, allowed_uid: u32) -> io::Result<UnixListener> {
    let account = account_for_uid(allowed_uid)?;
    if account.uid == 0 || !socket_path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "采集接收账户必须为普通用户且 socket 路径必须绝对",
        ));
    }
    let expected = collector_socket_path(allowed_uid);
    if socket_path != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "root 采集 socket 必须使用固定服务目录",
        ));
    }
    let parent = socket_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket 缺少父目录"))?;
    // 先核验既有安装目录；不修改系统 /var/run 的权限。
    checked_root_path(
        parent
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket 缺少安装目录"))?,
    )?;
    checked_directory(parent, 0, account.gid, 0o750)?;
    if let Ok(metadata) = fs::symlink_metadata(socket_path) {
        if !metadata.file_type().is_socket() || metadata.uid() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "拒绝替换非本服务的 socket",
            ));
        }
        if UnixStream::connect(socket_path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, "采集服务已运行"));
        }
        fs::remove_file(socket_path)?;
    }
    let listener = UnixListener::bind(socket_path)?;
    set_owner(socket_path, 0, account.gid)?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o660))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

enum SourceItem {
    Line(String, i64),
    Status(&'static str, &'static str),
    End,
}

fn produce_stdout(
    stdout: impl Read + Send + 'static,
    sender: mpsc::SyncSender<SourceItem>,
    drops: Arc<AtomicU64>,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let item = match read_bounded_line(&mut reader, MAX_LINE_BYTES + 1) {
                Ok(Some(bytes)) => match String::from_utf8(bytes) {
                    Ok(line) if line.len() <= MAX_LINE_BYTES => SourceItem::Line(line, now_ms()),
                    Ok(_) => SourceItem::Status("oversized_line", "采集行超过上限，已跳过"),
                    Err(_) => SourceItem::Status("invalid_line", "采集输出不是 UTF-8，已跳过"),
                },
                Ok(None) => break,
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    SourceItem::Status("oversized_line", "采集行超过上限，已跳过")
                }
                Err(_) => {
                    let _ = sender.try_send(SourceItem::Status("source_error", "读取采集输出失败"));
                    break;
                }
            };
            if sender.try_send(item).is_err() {
                drops.fetch_add(1, Ordering::Relaxed);
            }
        }
        // 生命周期标记使用有界阻塞发送，避免队列满时丢掉“源已停止”。
        let _ = sender.send(SourceItem::End);
    });
}

fn produce_stderr(stderr: impl Read + Send + 'static, sender: mpsc::SyncSender<SourceItem>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        while let Ok(Some(bytes)) = read_bounded_line(&mut reader, 16 * 1024) {
            let text = String::from_utf8_lossy(&bytes);
            let item = if text.contains("ERR_NOT_PERMITTED") || text.contains("Full Disk Access") {
                SourceItem::Status(
                    "permission_denied",
                    "eslogger 缺少完全磁盘访问授权；须核验后台责任进程 FDA",
                )
            } else {
                SourceItem::Status(
                    "source_diagnostic",
                    "eslogger 报告诊断信息；原始 stderr 未保存",
                )
            };
            let _ = sender.try_send(item);
        }
    });
}

fn deliver(client: &mut Option<UnixStream>, frame: &CollectorFrame) -> bool {
    if let Some(stream) = client {
        if write_frame(stream, frame).is_ok() {
            return true;
        }
        *client = None;
    }
    false
}

static COLLECTOR_STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn collector_stop_signal(_signal: libc::c_int) {
    // 信号处理器只设置无锁原子标志；子进程回收在正常循环中执行。
    COLLECTOR_STOP.store(true, Ordering::Relaxed);
}

struct CollectorSignals {
    previous: Vec<(libc::c_int, libc::sighandler_t)>,
}

impl CollectorSignals {
    fn install() -> io::Result<Self> {
        // sudo 完成同 TTY 认证后才隔离采集组；eslogger 继承本组。
        if unsafe { libc::getpgrp() } != unsafe { libc::getpid() }
            && unsafe { libc::setpgid(0, 0) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        COLLECTOR_STOP.store(false, Ordering::Relaxed);
        let mut guard = Self {
            previous: Vec::new(),
        };
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let previous =
                unsafe { libc::signal(signal, collector_stop_signal as libc::sighandler_t) };
            if previous == libc::SIG_ERR {
                return Err(io::Error::last_os_error());
            }
            guard.previous.push((signal, previous));
        }
        Ok(guard)
    }
}

impl Drop for CollectorSignals {
    fn drop(&mut self) {
        for (signal, previous) in &self.previous {
            unsafe { libc::signal(*signal, *previous) };
        }
    }
}

struct CollectorSource {
    child: Child,
}

impl CollectorSource {
    fn stop(&mut self) -> io::Result<ExitStatus> {
        if self.child.try_wait()?.is_none() {
            let _ = self.child.kill();
        }
        self.child.wait()
    }
}

impl Drop for CollectorSource {
    fn drop(&mut self) {
        // 正常停止和每一条 early Err 都只回收本次启动的来源，不按名称杀进程。
        let _ = self.stop();
    }
}

pub fn run_collector(options: CollectorOptions) -> Result<()> {
    require_root()?;
    let _signals = CollectorSignals::install()?;
    let listener = bind_collector(&options.socket_path, options.allowed_uid)?;
    let run_id = format!("eslogger-{}-{}", std::process::id(), now_ms());
    let mut source = CollectorSource {
        child: Command::new("/usr/bin/eslogger")
            .args(SUBSCRIBED_EVENTS)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()?,
    };
    let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
    let drops = Arc::new(AtomicU64::new(0));
    produce_stdout(
        source
            .child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("采集 stdout 未建立"))?,
        sender.clone(),
        Arc::clone(&drops),
    );
    produce_stderr(
        source
            .child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("采集 stderr 未建立"))?,
        sender.clone(),
    );
    drop(sender);
    let mut client = None;
    let mut last_heartbeat = Instant::now();
    let mut previous_drops = 0;
    let mut last_diagnostic = None;
    while !COLLECTOR_STOP.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if verify_peer_uid(&stream, options.allowed_uid).is_ok() && client.is_none() {
                    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                    let frame = CollectorFrame::Status {
                        run_id: run_id.clone(),
                        state: "connected".into(),
                        message: "采集桥接已连接；ES 事件完整性仍由宿主核验".into(),
                        dropped_lines: drops.load(Ordering::Relaxed),
                    };
                    if write_frame(&mut stream, &frame).is_ok() {
                        client = Some(stream);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(SourceItem::Line(line, received_timestamp_ms)) => {
                if !deliver(
                    &mut client,
                    &CollectorFrame::Line {
                        run_id: run_id.clone(),
                        line,
                        received_timestamp_ms,
                    },
                ) {
                    drops.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(SourceItem::Status(state, message)) => {
                last_diagnostic = Some((state, message));
                deliver(
                    &mut client,
                    &CollectorFrame::Status {
                        run_id: run_id.clone(),
                        state: state.into(),
                        message: message.into(),
                        dropped_lines: drops.load(Ordering::Relaxed),
                    },
                );
            }
            Ok(SourceItem::End) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let dropped_lines = drops.load(Ordering::Relaxed);
        if dropped_lines != previous_drops {
            deliver(
                &mut client,
                &CollectorFrame::Status {
                    run_id: run_id.clone(),
                    state: "coverage_gap".into(),
                    message: "无消费者、发送失败或队列过载导致桥接事件丢弃".into(),
                    dropped_lines,
                },
            );
            previous_drops = dropped_lines;
        }
        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            deliver(
                &mut client,
                &CollectorFrame::Heartbeat {
                    run_id: run_id.clone(),
                    dropped_lines,
                },
            );
            last_heartbeat = Instant::now();
        }
    }
    let status = source.stop()?;
    let stopped = CollectorFrame::Status {
        run_id: run_id.clone(),
        state: "stopped".into(),
        message: format!(
            "eslogger 已退出，exit_code={}",
            status
                .code()
                .map_or_else(|| "signal".into(), |code| code.to_string())
        ),
        dropped_lines: drops.load(Ordering::Relaxed),
    };
    deliver(&mut client, &stopped);
    if COLLECTOR_STOP.load(Ordering::Relaxed) {
        let _ = fs::remove_file(&options.socket_path);
        return Ok(());
    }
    // 退出前保留短暂状态窗口，让启动顺序较后的普通用户宿主看到 FDA／源故障。
    let diagnostic_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < diagnostic_deadline && !COLLECTOR_STOP.load(Ordering::Relaxed) {
        for item in receiver.try_iter() {
            if let SourceItem::Status(state, message) = item {
                last_diagnostic = Some((state, message));
                deliver(
                    &mut client,
                    &CollectorFrame::Status {
                        run_id: run_id.clone(),
                        state: state.into(),
                        message: message.into(),
                        dropped_lines: drops.load(Ordering::Relaxed),
                    },
                );
            }
        }
        if let Ok((mut stream, _)) = listener.accept() {
            if verify_peer_uid(&stream, options.allowed_uid).is_ok() {
                stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                if let Some((state, message)) = last_diagnostic {
                    let _ = write_frame(
                        &mut stream,
                        &CollectorFrame::Status {
                            run_id: run_id.clone(),
                            state: state.into(),
                            message: message.into(),
                            dropped_lines: drops.load(Ordering::Relaxed),
                        },
                    );
                }
                let _ = write_frame(&mut stream, &stopped);
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_file(&options.socket_path);
    if COLLECTOR_STOP.load(Ordering::Relaxed) {
        Ok(())
    } else {
        Err(io::Error::other("eslogger 采集已停止；launchd 可重启新的采集实例").into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceJob {
    pub label: String,
    pub domain: String,
    pub plist_path: PathBuf,
    pub argv: Vec<String>,
    pub plist: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePlan {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub source_binary: PathBuf,
    pub installed_binary: PathBuf,
    pub data_dir: PathBuf,
    pub collector_socket: PathBuf,
    pub control_socket: PathBuf,
    pub db_path: PathBuf,
    pub jobs: Vec<ServiceJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationStep {
    pub label: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationReport {
    pub steps: Vec<OperationStep>,
    pub data_preserved: bool,
}

pub fn collector_socket_path(uid: u32) -> PathBuf {
    PathBuf::from(format!("/Library/CodePerimeter/{uid}/run/collector.sock"))
}

impl ServicePlan {
    pub fn new(username: &str, source_binary: &Path) -> Result<Self> {
        let account = lookup_account(username)?;
        if account.uid == 0 || !account.home.is_absolute() || !source_binary.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "安装目标必须是有绝对 home 路径的普通账户，二进制路径必须绝对",
            )
            .into());
        }
        let installed_binary = PathBuf::from(format!(
            "/Library/CodePerimeter/{}/codeperimeter",
            account.uid
        ));
        let data_dir = account
            .home
            .join("Library/Application Support/CodePerimeter");
        let collector_socket = collector_socket_path(account.uid);
        let control_socket = data_dir.join("host.sock");
        let db_path = data_dir.join("events.sqlite");
        let binary = installed_binary.to_string_lossy().into_owned();
        let uid = account.uid.to_string();
        let roles = [
            (
                "collector",
                vec![
                    binary.clone(),
                    "collector".into(),
                    "--socket".into(),
                    collector_socket.to_string_lossy().into_owned(),
                    "--allowed-uid".into(),
                    uid.clone(),
                ],
                None,
            ),
            (
                "daemon",
                vec![
                    binary.clone(),
                    "daemon".into(),
                    "--socket".into(),
                    collector_socket.to_string_lossy().into_owned(),
                    "--control-socket".into(),
                    control_socket.to_string_lossy().into_owned(),
                    "--db".into(),
                    db_path.to_string_lossy().into_owned(),
                ],
                Some(account.name.as_str()),
            ),
            (
                "notify",
                vec![
                    binary,
                    "notify".into(),
                    "--control-socket".into(),
                    control_socket.to_string_lossy().into_owned(),
                ],
                None,
            ),
        ];
        let jobs = roles
            .into_iter()
            .map(|(role, argv, user)| {
                let label = format!("com.codeperimeter.{role}.{uid}");
                let is_agent = role == "notify";
                let plist_path = if is_agent {
                    account
                        .home
                        .join("Library/LaunchAgents")
                        .join(format!("{label}.plist"))
                } else {
                    PathBuf::from("/Library/LaunchDaemons").join(format!("{label}.plist"))
                };
                let plist = render_plist(&label, &argv, user, is_agent);
                ServiceJob {
                    label,
                    domain: if is_agent {
                        format!("gui/{uid}")
                    } else {
                        "system".into()
                    },
                    plist_path,
                    argv,
                    plist,
                }
            })
            .collect();
        Ok(Self {
            username: account.name,
            uid: account.uid,
            gid: account.gid,
            source_binary: source_binary.to_path_buf(),
            installed_binary,
            data_dir,
            collector_socket,
            control_socket,
            db_path,
            jobs,
        })
    }

    fn validate(&self) -> Result<Account> {
        let expected = Self::new(&self.username, &self.source_binary)?;
        if self.uid != expected.uid
            || self.gid != expected.gid
            || self.installed_binary != expected.installed_binary
            || self.data_dir != expected.data_dir
            || self.collector_socket != expected.collector_socket
            || self.control_socket != expected.control_socket
            || self.db_path != expected.db_path
            || serde_json::to_value(&self.jobs)? != serde_json::to_value(&expected.jobs)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "服务计划与本地账户／固定安装布局不符",
            )
            .into());
        }
        Ok(lookup_account(&self.username)?)
    }

    pub fn install(&self) -> Result<OperationReport> {
        require_root()?;
        let account = self.validate()?;
        checked_root_path(Path::new("/Library"))?;
        checked_directory(Path::new("/Library/CodePerimeter"), 0, 0, 0o755)?;
        let install_dir = self
            .installed_binary
            .parent()
            .ok_or_else(|| io::Error::other("安装目录缺失"))?;
        checked_directory(install_dir, 0, 0, 0o755)?;
        let mut source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.source_binary)?;
        let source_metadata = source.metadata()?;
        if !source_metadata.is_file()
            || source_metadata.mode() & 0o022 != 0
            || source_metadata.uid() != account.uid
                && !(source_metadata.uid() == 0 && source_metadata.mode() & 0o004 != 0)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "安装来源必须是普通二进制文件",
            )
            .into());
        }
        let temporary = install_dir.join("codeperimeter.new");
        write_installed_file(&temporary, &mut source, 0, 0, 0o755)?;
        if let Ok(metadata) = fs::symlink_metadata(&self.installed_binary) {
            if !metadata.is_file() || metadata.uid() != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "拒绝替换非本服务的安装二进制",
                )
                .into());
            }
        }
        fs::rename(&temporary, &self.installed_binary)?;
        checked_root_path(&self.installed_binary)?;
        checked_root_path(Path::new("/Library/LaunchDaemons"))?;
        ensure_user_directories(&account, &self.data_dir)?;
        let agent_dir = account.home.join("Library/LaunchAgents");
        ensure_user_directories(&account, &agent_dir)?;
        for job in &self.jobs {
            if job.domain.starts_with("gui/") {
                // 用户可改父目录下的写入降权执行，避免符号链接竞态扩大 root 写权限。
                let staging = install_dir.join("notify.plist");
                write_installed_file(&staging, &mut job.plist.as_bytes(), 0, 0, 0o644)?;
                user_command(
                    &account,
                    "/usr/bin/install",
                    &[
                        "-m",
                        "644",
                        &staging.to_string_lossy(),
                        &job.plist_path.to_string_lossy(),
                    ],
                )?;
                fs::remove_file(staging)?;
            } else {
                write_installed_file(&job.plist_path, &mut job.plist.as_bytes(), 0, 0, 0o644)?;
            }
        }
        Ok(OperationReport {
            steps: vec![OperationStep {
                label: "install".into(),
                success: true,
                message: "已安装 root 拥有的二进制与三份 job；FDA 和后台运行需另行核验".into(),
            }],
            data_preserved: true,
        })
    }

    pub fn start(&self) -> Result<OperationReport> {
        require_root()?;
        self.validate()?;
        checked_root_path(&self.installed_binary)?;
        let mut steps = Vec::new();
        for job in &self.jobs {
            let target = format!("{}/{}", job.domain, job.label);
            if !launchctl_success(&["print", &job.domain])? && job.domain.starts_with("gui/") {
                steps.push(OperationStep {
                    label: job.label.clone(),
                    success: true,
                    message: "当前没有对应 macOS 桌面会话；通知 job 将在该账户进入桌面时启动"
                        .into(),
                });
                continue;
            }
            let loaded = launchctl_success(&["print", &target])?;
            let mut success = launchctl_success(&["enable", &target])?;
            if success && !loaded {
                success = launchctl_success(&[
                    "bootstrap",
                    &job.domain,
                    &job.plist_path.to_string_lossy(),
                ])?;
            }
            if success {
                success = launchctl_success(&["kickstart", "-k", &target])?;
            }
            steps.push(OperationStep {
                label: job.label.clone(),
                success,
                message: if success {
                    "已请求 launchd 启动；需通过宿主状态与真实事件核验采集".into()
                } else {
                    "launchd 启动请求失败，不能报告运行生效".into()
                },
            });
        }
        Ok(OperationReport {
            steps,
            data_preserved: true,
        })
    }

    pub fn stop(&self) -> Result<OperationReport> {
        require_root()?;
        self.validate()?;
        let mut steps = Vec::new();
        for job in self.jobs.iter().rev() {
            let target = format!("{}/{}", job.domain, job.label);
            let loaded = launchctl_success(&["print", &target])?;
            // system job 的关闭状态持久化，避免停止后在下次开机意外恢复采集。
            let disabled = job.domain != "system" || launchctl_success(&["disable", &target])?;
            let success = disabled && (!loaded || launchctl_success(&["bootout", &target])?);
            steps.push(OperationStep {
                label: job.label.clone(),
                success,
                message: if success {
                    "job 已停止或当前未加载；保留证据数据".into()
                } else {
                    "停止请求失败".into()
                },
            });
        }
        Ok(OperationReport {
            steps,
            data_preserved: true,
        })
    }

    pub fn uninstall(&self) -> Result<OperationReport> {
        let mut report = self.stop()?;
        if report.steps.iter().any(|step| !step.success) {
            return Err(io::Error::other("部分 job 停止失败，保留安装文件以便恢复").into());
        }
        let account = self.validate()?;
        for job in &self.jobs {
            if job.domain.starts_with("gui/") {
                user_command(
                    &account,
                    "/bin/rm",
                    &["-f", "--", &job.plist_path.to_string_lossy()],
                )?;
                continue;
            }
            remove_root_file(&job.plist_path)?;
        }
        remove_root_file(&self.installed_binary)?;
        if let Some(dir) = self.installed_binary.parent() {
            remove_root_file(&dir.join("codeperimeter.new"))?;
            remove_root_file(&dir.join("notify.plist"))?;
            let _ = fs::remove_dir(dir);
        }
        report.steps.push(OperationStep {
            label: "uninstall".into(),
            success: true,
            message: "已删除本服务的 job 与二进制；SQLite 和配置保留".into(),
        });
        Ok(report)
    }
}

fn remove_root_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.uid() == 0 => {
            checked_root_path(
                path.parent()
                    .ok_or_else(|| io::Error::other("卸载路径缺少父目录"))?,
            )?;
            fs::remove_file(path)?;
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "卸载路径不是 root 拥有的本服务普通文件",
            )
            .into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn launchctl_success(args: &[&str]) -> io::Result<bool> {
    Command::new("/bin/launchctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
}

pub fn checked_root_path(path: &Path) -> io::Result<()> {
    let mut current = Some(path);
    while let Some(path) = current {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "root 服务文件或父目录可被普通用户修改",
            ));
        }
        current = path.parent();
    }
    Ok(())
}

fn write_installed_file(
    path: &Path,
    reader: &mut impl Read,
    uid: u32,
    gid: u32,
    mode: u32,
) -> io::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.uid() != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "拒绝写入非本服务的普通文件",
            ));
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    io::copy(reader, &mut file)?;
    file.sync_all()?;
    set_owner(path, uid, gid)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    if uid == 0 {
        clear_managed_acl(path)?;
    }
    Ok(())
}

fn clear_managed_acl(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if !Command::new("/bin/chmod")
            .arg("-N")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success()
        {
            return Err(io::Error::other("清除本服务 root 文件的继承 ACL 失败"));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
    }
    Ok(())
}

fn ensure_user_directories(account: &Account, target: &Path) -> io::Result<()> {
    let relative = target
        .strip_prefix(&account.home)
        .map_err(io::Error::other)?;
    let home = fs::symlink_metadata(&account.home)?;
    if !home.is_dir() || home.file_type().is_symlink() || home.uid() != account.uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "账户 home 目录不可验证",
        ));
    }
    user_command(
        account,
        "/bin/mkdir",
        &["-p", "--", &target.to_string_lossy()],
    )?;
    let mut path = account.home.clone();
    for component in relative.components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != account.uid
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "用户服务目录身份或类型不符",
            ));
        }
    }
    if target.ends_with("CodePerimeter") {
        user_command(account, "/bin/chmod", &["700", &target.to_string_lossy()])?;
    }
    Ok(())
}

fn user_command(account: &Account, program: &str, args: &[&str]) -> io::Result<()> {
    // sudo 在已有 root 调用中只负责切换普通账户及组，不申请或保存密码。
    let status = Command::new("/usr/bin/sudo")
        .args(["-n", "-u", &account.name, "--", program])
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(io::Error::other("普通用户目录／通知 job 操作失败"));
    }
    Ok(())
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_plist(label: &str, argv: &[String], username: Option<&str>, agent: bool) -> String {
    let arguments = argv
        .iter()
        .map(|arg| format!("<string>{}</string>", xml(arg)))
        .collect::<Vec<_>>()
        .join("\n");
    let user = username
        .map(|name| format!("<key>UserName</key><string>{}</string>\n", xml(name)))
        .unwrap_or_default();
    let session = if agent {
        "<key>LimitLoadToSessionType</key><string>Aqua</string>\n"
    } else {
        ""
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array>\n{}\n</array>\n{}{}<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>ThrottleInterval</key><integer>5</integer>\n<key>ProcessType</key><string>Background</string>\n<key>Umask</key><integer>63</integer>\n<key>StandardOutPath</key><string>/dev/null</string>\n<key>StandardErrorPath</key><string>/dev/null</string>\n</dict></plist>\n",
        xml(label),
        arguments,
        user,
        session
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    #[ignore = "由受控子进程信号测试调用，不在测试宿主中改信号处理器"]
    fn collector_signal_worker() {
        assert_eq!(
            std::env::var("CODEPERIMETER_SERVICE_SIGNAL_WORKER").as_deref(),
            Ok("yes")
        );
        let _signals = CollectorSignals::install().unwrap();
        let mut source = CollectorSource {
            child: Command::new("/bin/sleep").arg("30").spawn().unwrap(),
        };
        println!(
            "COLLECTOR_READY {} {} {}",
            std::process::id(),
            unsafe { libc::getpgrp() },
            source.child.id()
        );
        io::stdout().flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !COLLECTOR_STOP.load(Ordering::Relaxed) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(COLLECTOR_STOP.load(Ordering::Relaxed));
        source.stop().unwrap();
    }

    #[test]
    fn collector_signals_isolate_group_and_reap_owned_source() {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let mut worker = CollectorSource {
                child: Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "service::tests::collector_signal_worker",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("CODEPERIMETER_SERVICE_SIGNAL_WORKER", "yes")
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap(),
            };
            let stdout = worker.child.stdout.take().unwrap();
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                for line in BufReader::new(stdout)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    if let Some(values) = line.strip_prefix("COLLECTOR_READY ") {
                        let _ = sender.send(values.to_owned());
                    }
                }
            });
            let values = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
            let pids: Vec<libc::pid_t> = values
                .split_whitespace()
                .map(|value| value.parse().unwrap())
                .collect();
            assert_eq!(pids[0], worker.child.id() as libc::pid_t);
            assert_eq!(pids[0], pids[1]);
            assert_ne!(pids[1], unsafe { libc::getpgrp() });
            assert_eq!(unsafe { libc::kill(pids[0], signal) }, 0);
            let deadline = Instant::now() + Duration::from_secs(3);
            let status = loop {
                if let Some(status) = worker.child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    // 失败时仍只清理本次测试的已记录来源 PID。
                    unsafe { libc::kill(pids[2], libc::SIGKILL) };
                    panic!("collector 信号停止超时");
                }
                thread::sleep(Duration::from_millis(10));
            };
            assert!(status.success());
            assert_eq!(unsafe { libc::kill(pids[2], 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        }
    }

    #[test]
    fn owned_source_is_reaped_when_an_error_returns_early() {
        let mut pid = 0;
        let result = (|| -> io::Result<()> {
            let source = CollectorSource {
                child: Command::new("/bin/sleep").arg("30").spawn()?,
            };
            pid = source.child.id() as libc::pid_t;
            Err(io::Error::other("合成 early Err"))
        })();
        assert!(result.is_err());
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn source_queue_is_bounded_and_reports_discarded_events() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let drops = Arc::new(AtomicU64::new(0));
        produce_stdout(
            Cursor::new(b"one\ntwo\nthree\nfour\n".to_vec()),
            sender,
            Arc::clone(&drops),
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while drops.load(Ordering::Relaxed) < 2 && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            SourceItem::Line(_, _)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            SourceItem::Line(_, _)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            SourceItem::End
        ));
    }

    #[test]
    fn plist_strings_are_escaped_without_shell_evaluation() {
        let plist = render_plist(
            "example&label",
            &["/root-owned/<binary>".into(), "literal$(value)".into()],
            Some("example&user"),
            false,
        );
        assert!(plist.contains("example&amp;label"));
        assert!(plist.contains("&lt;binary&gt;"));
        assert!(plist.contains("example&amp;user"));
        assert!(plist.contains("literal$(value)"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn removes_extended_write_acl_from_managed_files() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("synthetic-executable");
        fs::write(&file, "synthetic").unwrap();
        assert!(
            Command::new("/bin/chmod")
                .args(["+a", "everyone allow write"])
                .arg(&file)
                .status()
                .unwrap()
                .success()
        );
        let before = Command::new("/bin/ls")
            .arg("-le")
            .arg(&file)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&before.stdout).contains("allow write"));
        clear_managed_acl(&file).unwrap();
        let after = Command::new("/bin/ls")
            .arg("-le")
            .arg(&file)
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&after.stdout).contains("allow write"));
    }
}
