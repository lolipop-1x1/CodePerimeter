//! 回环网页入口；数据读写仍通过普通用户宿主执行。

use crate::Result;
use crate::history::{self, DirectoryCandidate, DirectoryStatus, HistoryOptions};
use crate::native::{self, ServiceAction};
use crate::runtime::{self, ControlRequest, DirectoryImport};
use crate::service;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path as RoutePath, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc};
use tokio_stream::wrappers::ReceiverStream;

include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));
const MAX_BODY: usize = 256 * 1024;
const MAX_PREVIEW: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct WebOptions {
    pub host_socket: PathBuf,
    pub port: u16,
    pub foreground: bool,
    pub no_browser: bool,
    pub session_file: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub claude_home: Option<PathBuf>,
    pub zcode_db: Option<PathBuf>,
    pub alert_id: Option<String>,
    pub alerts: bool,
}

// 版本 2 才支持通知告警详情入口，更新后不复用旧页面服务。
const WEB_ENTRY_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Session {
    version: u32,
    pid: u32,
    origin: String,
    token: String,
    host_socket: PathBuf,
}

#[derive(Clone)]
struct AppState {
    session: Session,
    username: String,
    binary: PathBuf,
    history: HistoryOptions,
    native_enabled: bool,
    blocking: Arc<Semaphore>,
    previews: Arc<Mutex<BTreeMap<String, Vec<DirectoryCandidate>>>>,
    jobs: Arc<Mutex<BTreeMap<String, Value>>>,
    native_busy: Arc<Semaphore>,
    service_cache: Arc<Mutex<Option<(Instant, Value)>>>,
}

pub fn run(options: WebOptions) -> Result<()> {
    if options
        .alert_id
        .as_deref()
        .is_some_and(|id| !crate::model::valid_alert_id(id))
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "告警入口标识不合法").into());
    }
    let account = current_account()?;
    if account.uid == 0 {
        return Err(io::Error::other("网页控制台必须以普通用户运行").into());
    }
    let session_path = options.session_file.clone().unwrap_or_else(|| {
        account
            .home
            .join("Library/Application Support/CodePerimeter/ui-session.json")
    });
    ensure_private_parent(&session_path)?;
    if options.foreground {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(serve(options, account, session_path));
    }
    let lock = private_file(&session_path.with_extension("lock"), true)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::other("无法锁定网页入口").into());
    }
    if let Some(session) = read_session(&session_path)? {
        if session.host_socket == options.host_socket && probe_session(&session) {
            if !options.no_browser {
                open_browser(&session, options.alert_id.as_deref(), options.alerts)?;
            }
            println!("本机网页控制台已打开；关闭网页后监控继续运行。");
            return Ok(());
        }
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--host-socket")
        .arg(&options.host_socket)
        .args(["ui", "--foreground", "--no-browser", "--session-file"])
        .arg(&session_path)
        .arg("--port")
        .arg(options.port.to_string());
    for (flag, path) in [
        ("--codex-home", &options.codex_home),
        ("--claude-home", &options.claude_home),
        ("--zcode-db", &options.zcode_db),
    ] {
        if let Some(path) = path {
            command.arg(flag).arg(path);
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // 与调用终端分离；宿主与采集进程始终独立于网页入口。
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(session) = read_session(&session_path)? {
            if session.pid == child.id() && probe_session(&session) {
                if !options.no_browser {
                    open_browser(&session, options.alert_id.as_deref(), options.alerts)?;
                }
                println!("本机网页控制台已启动；关闭网页后监控继续运行。");
                return Ok(());
            }
        }
        if child.try_wait()?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(io::Error::other("网页控制台未能启动，请用 ui --foreground 查看状态").into())
}

fn current_account() -> io::Result<service::Account> {
    let uid = unsafe { libc::geteuid() };
    let mut record: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0u8; 65536];
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut record,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(io::Error::other("无法确认本机用户"));
    }
    let name = unsafe { CStr::from_ptr(record.pw_name) }
        .to_string_lossy()
        .into_owned();
    service::lookup_account(&name)
}

fn ensure_private_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("入口文件须位于私有目录"))?;
    if !parent.exists() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let meta = fs::symlink_metadata(parent)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other("入口文件目录必须属于当前用户且权限为0700"));
    }
    Ok(())
}

fn private_file(path: &Path, create: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
    {
        return Err(io::Error::other("入口文件权限或类型不安全"));
    }
    Ok(file)
}

fn read_session(path: &Path) -> io::Result<Option<Session>> {
    let mut file = match private_file(path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() > 16384 {
        return Err(io::Error::other("入口文件超限"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes).ok())
}

fn nonce() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn probe_session(session: &Session) -> bool {
    if session.version != WEB_ENTRY_VERSION || session.token.len() != 64 {
        return false;
    }
    let Some(address) = session
        .origin
        .strip_prefix("http://127.0.0.1:")
        .and_then(|port| port.parse::<u16>().ok())
    else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, address).into(),
        Duration::from_millis(300),
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    if write!(stream, "GET /api/ping HTTP/1.1\r\nHost: 127.0.0.1:{address}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n", session.token).is_err() { return false; }
    let mut bytes = Vec::new();
    if stream.take(16384).read_to_end(&mut bytes).is_err() {
        return false;
    }
    let response = String::from_utf8_lossy(&bytes);
    response.starts_with("HTTP/1.1 200") && response.contains(&format!("\"pid\":{}", session.pid))
}

fn browser_entry(session: &Session, alert_id: Option<&str>, alerts: bool) -> String {
    let target = if let Some(id) = alert_id {
        format!("?alert={id}")
    } else if alerts {
        "?view=alerts".into()
    } else {
        String::new()
    };
    format!("{}/{target}#token={}", session.origin, session.token)
}

fn open_browser(session: &Session, alert_id: Option<&str>, alerts: bool) -> io::Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(io::Error::other("请在本机浏览器打开私有入口文件中的地址"));
    }
    let status = Command::new("/usr/bin/open")
        .arg(browser_entry(session, alert_id, alerts))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("浏览器打开失败，入口仍在运行"))
    }
}

struct SessionGuard {
    path: PathBuf,
    pid: u32,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if read_session(&self.path)
            .ok()
            .flatten()
            .is_some_and(|s| s.pid == self.pid)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

async fn serve(
    options: WebOptions,
    account: service::Account,
    session_path: PathBuf,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, options.port)).await?;
    let port = listener.local_addr()?.port();
    let session = Session {
        version: WEB_ENTRY_VERSION,
        pid: std::process::id(),
        origin: format!("http://127.0.0.1:{port}"),
        token: nonce()?,
        host_socket: options.host_socket.clone(),
    };
    let defaults = HistoryOptions::defaults(&account.home);
    let overridden =
        options.codex_home.is_some() || options.claude_home.is_some() || options.zcode_db.is_some();
    let state = AppState {
        session: session.clone(),
        username: account.name,
        binary: std::env::current_exe()?,
        history: if overridden {
            HistoryOptions {
                codex_home: options.codex_home,
                claude_home: options.claude_home,
                zcode_db: options.zcode_db,
            }
        } else {
            defaults
        },
        // 独立宿主验收禁止修改机器上已安装的生产服务。
        native_enabled: options.host_socket
            == account
                .home
                .join("Library/Application Support/CodePerimeter/host.sock"),
        blocking: Arc::new(Semaphore::new(8)),
        previews: Arc::new(Mutex::new(BTreeMap::new())),
        jobs: Arc::new(Mutex::new(BTreeMap::new())),
        native_busy: Arc::new(Semaphore::new(1)),
        service_cache: Arc::new(Mutex::new(None)),
    };
    let app = Router::new()
        .route("/api/ping", get(ping))
        .route("/api/status", get(status))
        .route("/api/control", post(control))
        .route("/api/console", post(console))
        .route("/api/history/preview", post(history_preview))
        .route("/api/history/import", post(history_import))
        .route("/api/directory/pick", post(directory_pick))
        .route("/api/service", post(service_action))
        .route("/api/service/{id}", get(service_job))
        .route("/api/export", post(export))
        .fallback(asset)
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state);
    let temporary = session_path.with_extension(format!("{}.tmp", nonce()?));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer(&mut output, &session)?;
    output.sync_all()?;
    if session_path.exists() {
        let _ = private_file(&session_path, false)?;
    }
    fs::rename(&temporary, &session_path)?;
    let _guard = SessionGuard {
        path: session_path,
        pid: session.pid,
    };
    if !options.no_browser {
        open_browser(&session, options.alert_id.as_deref(), options.alerts)?;
    }
    println!("本机网页控制台已启动；入口令牌只保存在私有会话文件中。");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("信号监听失败");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn reply(data: Value) -> Response {
    Json(json!({"ok":true,"data":data,"error":null})).into_response()
}
fn failure(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"ok":false,"data":null,"error":code}))).into_response()
}
fn parse(body: &[u8]) -> std::result::Result<Value, ()> {
    serde_json::from_slice(body).map_err(|_| ())
}
fn equal_secret(expected: &[u8], actual: &[u8]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected
        .iter()
        .zip(actual)
        .fold(0u8, |value, (a, b)| value | (a ^ b))
        == 0
}

fn trusted_origin(headers: &axum::http::HeaderMap, expected: &str) -> bool {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    if headers.contains_key("sec-fetch-site") && site != Some("same-origin") {
        return false;
    }
    origin == Some(expected)
        // 某些本机浏览器环境去掉 Origin 端口；只接受浏览器确认的同源请求。
        || origin == Some("http://127.0.0.1") && site == Some("same-origin")
        || !headers.contains_key(header::ORIGIN) && site == Some("same-origin")
}

async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let host = state.session.origin.trim_start_matches("http://");
    if request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        != Some(host)
    {
        return failure(StatusCode::FORBIDDEN, "host_rejected");
    }
    if request.uri().path().starts_with("/api/") {
        // 静态资源没有用户数据；来源校验只保护读取数据与提交操作的 API。
        if (request.headers().contains_key(header::ORIGIN)
            || request.headers().contains_key("sec-fetch-site"))
            && !trusted_origin(request.headers(), &state.session.origin)
        {
            return failure(StatusCode::FORBIDDEN, "origin_rejected");
        }
        let bearer = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .unwrap_or("");
        if !equal_secret(state.session.token.as_bytes(), bearer.as_bytes()) {
            return failure(StatusCode::UNAUTHORIZED, "entry_expired");
        }
        if request.method() != axum::http::Method::GET {
            if !trusted_origin(request.headers(), &state.session.origin) {
                return failure(StatusCode::FORBIDDEN, "origin_required");
            }
            if request
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .map(|h| h.split(';').next())
                != Some(Some("application/json"))
            {
                return failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required");
            }
        }
    }
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
        ("cross-origin-resource-policy", "same-origin"),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
}

async fn ping(State(state): State<AppState>) -> Response {
    reply(json!({"pid":state.session.pid}))
}

async fn host_call(state: &AppState, value: Value) -> std::result::Result<Value, Response> {
    let request: ControlRequest = serde_json::from_value(value)
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "request_invalid"))?;
    let permit = state
        .blocking
        .clone()
        .try_acquire_owned()
        .map_err(|_| failure(StatusCode::TOO_MANY_REQUESTS, "host_busy"))?;
    let socket = state.session.host_socket.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        runtime::request_control(&socket, request)
    })
    .await;
    match result {
        Ok(Ok(response)) if response.ok => Ok(response.data.unwrap_or(Value::Null)),
        Ok(Ok(response)) if response.error.as_deref() == Some("alert_unavailable") => {
            Err(failure(StatusCode::NOT_FOUND, "alert_unavailable"))
        }
        Ok(Ok(_)) => Err(failure(StatusCode::BAD_REQUEST, "host_request_rejected")),
        _ => Err(failure(StatusCode::SERVICE_UNAVAILABLE, "host_unavailable")),
    }
}

async fn status(State(state): State<AppState>) -> Response {
    let host = host_call(&state, json!({"operation":"status"})).await;
    let cached = state
        .service_cache
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(at, _)| at.elapsed() < Duration::from_secs(2))
        .map(|(_, value)| value.clone());
    let service = if let Some(cached) = cached {
        cached
    } else {
        let username = state.username.clone();
        let binary = state.binary.clone();
        let permit = match state.blocking.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return failure(StatusCode::TOO_MANY_REQUESTS, "host_busy"),
        };
        let observed = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            native::service_status(&username, &binary)
        })
        .await;
        let value = match observed {
            Ok(Ok(value)) => serde_json::to_value(value).unwrap_or(Value::Null),
            _ => json!({"installed":false,"status_error":"service_status_unavailable"}),
        };
        *state.service_cache.lock().unwrap() = Some((Instant::now(), value.clone()));
        value
    };
    let (host, error) = match host {
        Ok(value) => (value, Value::Null),
        Err(_) => (Value::Null, json!("host_unavailable")),
    };
    reply(
        json!({"host":host,"host_error":error,"service":service,"platform":std::env::consts::OS,"service_actions_enabled":state.native_enabled && cfg!(target_os="macos")}),
    )
}

async fn control(State(state): State<AppState>, body: Bytes) -> Response {
    let mut value = match parse(&body) {
        Ok(value) => value,
        Err(()) => return failure(StatusCode::BAD_REQUEST, "request_invalid"),
    };
    let operation = value.get("operation").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        operation,
        "status"
            | "add_directories"
            | "remove_directory"
            | "list_directories"
            | "query_health"
            | "query_notifications"
            | "stats"
    ) {
        return failure(StatusCode::FORBIDDEN, "operation_rejected");
    }
    if operation == "add_directories" {
        let Some(entries) = value
            .pointer_mut("/payload/entries")
            .and_then(Value::as_array_mut)
        else {
            return failure(StatusCode::BAD_REQUEST, "request_invalid");
        };
        if entries.is_empty() || entries.len() > 1024 {
            return failure(StatusCode::BAD_REQUEST, "directory_limit");
        }
        for entry in entries {
            let Some(path) = entry
                .get("path")
                .and_then(Value::as_str)
                .and_then(|path| fs::canonicalize(path).ok())
                .filter(|path| path.is_dir())
            else {
                return failure(StatusCode::BAD_REQUEST, "directory_unavailable");
            };
            *entry = json!({"path":path,"sources":["manual"]});
        }
    }
    match host_call(&state, value).await {
        Ok(value) => reply(value),
        Err(response) => response,
    }
}

async fn console(State(state): State<AppState>, body: Bytes) -> Response {
    let value = match parse(&body) {
        Ok(value) => value,
        Err(()) => return failure(StatusCode::BAD_REQUEST, "request_invalid"),
    };
    if !matches!(
        value.get("action").and_then(Value::as_str),
        Some(
            "summary"
                | "directories"
                | "directory_set"
                | "directory_remove_preview"
                | "rules_get"
                | "rules_set"
                | "events_page"
                | "alerts_page"
                | "event_detail"
                | "alert_detail"
                | "alert_update"
                | "retention_get"
                | "retention_preview"
                | "retention_set"
                | "clear_details"
                | "clear_cumulative"
        )
    ) {
        return failure(StatusCode::FORBIDDEN, "operation_rejected");
    }
    match host_call(
        &state,
        json!({"operation":"console","payload":{"request":value}}),
    )
    .await
    {
        Ok(value) => reply(value),
        Err(response) => response,
    }
}

async fn history_preview(State(state): State<AppState>) -> Response {
    let Ok(permit) = state.blocking.clone().try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS, "host_busy");
    };
    let options = state.history.clone();
    let report = match tokio::task::spawn_blocking(move || {
        // 请求取消不终止阻塞扫描；配额由实际扫描生命周期持有。
        let _permit = permit;
        history::discover(&options)
    })
    .await
    {
        Ok(report) => report,
        Err(_) => return failure(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"),
    };
    let mut value = match serde_json::to_value(&report) {
        Ok(value) => value,
        Err(_) => return failure(StatusCode::INTERNAL_SERVER_ERROR, "history_unavailable"),
    };
    if !serde_json::to_vec(&value).is_ok_and(|bytes| bytes.len() <= MAX_PREVIEW) {
        return failure(StatusCode::PAYLOAD_TOO_LARGE, "history_preview_limit");
    }
    let id = match nonce() {
        Ok(id) => id,
        Err(_) => return failure(StatusCode::INTERNAL_SERVER_ERROR, "random_unavailable"),
    };
    let mut previews = state.previews.lock().unwrap();
    // 一次预览持有一份稳定快照；旧选择必须重新明确预览。
    previews.clear();
    previews.insert(id.clone(), report.candidates);
    value["preview_id"] = json!(id);
    reply(value)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportSelection {
    preview_id: String,
    indices: Vec<usize>,
}
async fn history_import(State(state): State<AppState>, body: Bytes) -> Response {
    let selection: ImportSelection = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "request_invalid"),
    };
    if selection.indices.is_empty() || selection.indices.len() > 1024 {
        return failure(StatusCode::BAD_REQUEST, "selection_invalid");
    }
    let entries = {
        let previews = state.previews.lock().unwrap();
        let Some(candidates) = previews.get(&selection.preview_id) else {
            return failure(StatusCode::CONFLICT, "preview_expired");
        };
        let mut seen = BTreeSet::new();
        let mut entries = Vec::new();
        for index in selection.indices {
            if !seen.insert(index) {
                continue;
            }
            let Some(candidate) = candidates
                .get(index)
                .filter(|c| c.status == DirectoryStatus::Available)
            else {
                return failure(StatusCode::BAD_REQUEST, "selection_unavailable");
            };
            let Some(path) = &candidate.canonical_path else {
                return failure(StatusCode::BAD_REQUEST, "selection_unavailable");
            };
            if !path.is_dir() || fs::canonicalize(path).ok().as_ref() != Some(path) {
                return failure(StatusCode::CONFLICT, "directory_changed");
            }
            let sources: BTreeSet<String> = candidate
                .origins
                .iter()
                .filter_map(|origin| {
                    serde_json::to_value(origin.source)
                        .ok()?
                        .as_str()
                        .map(str::to_owned)
                })
                .collect();
            entries.push(DirectoryImport {
                path: path.clone(),
                sources: sources.into_iter().collect(),
            });
        }
        entries
    };
    match host_call(
        &state,
        json!({"operation":"add_directories","payload":{"entries":entries}}),
    )
    .await
    {
        Ok(value) => reply(value),
        Err(response) => response,
    }
}

async fn directory_pick(State(state): State<AppState>) -> Response {
    if !cfg!(target_os = "macos") {
        return failure(StatusCode::BAD_REQUEST, "platform_unsupported");
    }
    let Ok(permit) = state.native_busy.clone().try_acquire_owned() else {
        return failure(StatusCode::CONFLICT, "native_operation_busy");
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        native::choose_directory()
    })
    .await
    {
        Ok(Ok(choice)) => reply(serde_json::to_value(choice).unwrap_or(Value::Null)),
        _ => failure(StatusCode::INTERNAL_SERVER_ERROR, "directory_picker_failed"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceRequest {
    operation: ServiceAction,
}
async fn service_action(State(state): State<AppState>, body: Bytes) -> Response {
    if !state.native_enabled || !cfg!(target_os = "macos") {
        return failure(StatusCode::FORBIDDEN, "service_actions_disabled");
    }
    let request: ServiceRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "request_invalid"),
    };
    let Ok(permit) = state.native_busy.clone().try_acquire_owned() else {
        return failure(StatusCode::CONFLICT, "native_operation_busy");
    };
    let id = match nonce() {
        Ok(id) => id,
        Err(_) => return failure(StatusCode::INTERNAL_SERVER_ERROR, "random_unavailable"),
    };
    let record = json!({"id":id,"operation":request.operation,"state":"running","error":null});
    {
        let mut jobs = state.jobs.lock().unwrap();
        if jobs.len() >= 64 {
            jobs.retain(|_, value| value["state"] == "running");
        }
        jobs.insert(id.clone(), record);
    }
    let worker_state = state.clone();
    let worker_id = id.clone();
    tokio::spawn(async move {
        let username = worker_state.username.clone();
        let binary = worker_state.binary.clone();
        let socket = worker_state.session.host_socket.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            native::run_service_action(request.operation, &username, &binary, &socket)
        })
        .await;
        let value = match result {
            Ok(Ok(result)) => {
                json!({"id":worker_id,"operation":request.operation,"state":result.state,"report":result.report,"error":result.error})
            }
            _ => {
                json!({"id":worker_id,"operation":request.operation,"state":"failed","error":"service_operation_failed"})
            }
        };
        worker_state.jobs.lock().unwrap().insert(worker_id, value);
        *worker_state.service_cache.lock().unwrap() = None;
    });
    reply(json!({"id":id,"state":"running"}))
}
async fn service_job(State(state): State<AppState>, RoutePath(id): RoutePath<String>) -> Response {
    match state.jobs.lock().unwrap().get(&id) {
        Some(value) => reply(value.clone()),
        None => failure(StatusCode::NOT_FOUND, "operation_unknown"),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportRequest {
    kind: String,
    #[serde(default = "empty_object")]
    filter: Value,
    search: Option<String>,
    #[serde(default)]
    archive_only: bool,
    format: String,
    anonymous: bool,
    is_read: Option<bool>,
    processed: Option<bool>,
}
fn empty_object() -> Value {
    json!({})
}
fn export_page_request(request: &ExportRequest, cursor: Option<&str>) -> Value {
    let mut filter = request.filter.clone();
    filter["limit"] = json!(100);
    let is_read = request
        .is_read
        .or_else(|| filter.get("is_read").and_then(Value::as_bool));
    let processed = request
        .processed
        .or_else(|| filter.get("processed").and_then(Value::as_bool));
    if let Some(fields) = filter.as_object_mut() {
        fields.remove("is_read");
        fields.remove("processed");
    }
    let mut payload = json!({"filter":filter,"cursor":cursor,"search":request.search});
    if request.kind == "events" {
        payload["archive_only"] = json!(request.archive_only);
    } else {
        payload["is_read"] = json!(is_read);
        payload["processed"] = json!(processed);
    }
    json!({"operation":"console","payload":{"request":{"action":if request.kind=="events" {"events_page"} else {"alerts_page"},"payload":payload}}})
}

async fn export(State(state): State<AppState>, body: Bytes) -> Response {
    let request: ExportRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "request_invalid"),
    };
    if !matches!(request.kind.as_str(), "events" | "alerts")
        || !matches!(request.format.as_str(), "json" | "csv")
        || !request.filter.is_object()
    {
        return failure(StatusCode::BAD_REQUEST, "export_invalid");
    }
    let first = match host_call(&state, export_page_request(&request, None)).await {
        Ok(page) => page,
        Err(response) => return response,
    };
    let Ok(permit) = state.blocking.clone().try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS, "host_busy");
    };
    let salt = match nonce() {
        Ok(salt) => salt,
        Err(_) => return failure(StatusCode::INTERNAL_SERVER_ERROR, "random_unavailable"),
    };
    let (sender, receiver) = mpsc::channel::<std::result::Result<Bytes, io::Error>>(2);
    let socket = state.session.host_socket.clone();
    let csv = request.format == "csv";
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let result = stream_export(&socket, &request, first, &salt, &sender);
        if result.is_err() {
            let _ = sender.blocking_send(Err(io::Error::other("export_incomplete")));
        }
    });
    let mut response = Response::new(Body::from_stream(ReceiverStream::new(receiver)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if csv {
            "text/csv; charset=utf-8"
        } else {
            "application/json; charset=utf-8"
        }),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(if csv {
            "attachment; filename=codeperimeter-records.csv"
        } else {
            "attachment; filename=codeperimeter-records.json"
        }),
    );
    response
}

fn stream_export(
    socket: &Path,
    request: &ExportRequest,
    mut page: Value,
    salt: &str,
    sender: &mpsc::Sender<std::result::Result<Bytes, io::Error>>,
) -> Result<()> {
    let send = |text: String| {
        sender
            .blocking_send(Ok(Bytes::from(text)))
            .map_err(|_| io::Error::other("export_cancelled"))
    };
    let csv = request.format == "csv";
    send(if csv {
        "id,kind,pid,executable,time_ms,evidence,record_json\r\n".into()
    } else {
        "[".into()
    })?;
    let mut first = true;
    let mut previous = None;
    loop {
        let items = page
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| io::Error::other("export_page_invalid"))?;
        let mut chunk = String::new();
        for item in items {
            if request.anonymous {
                anonymize(item, salt, None);
            }
            if csv {
                chunk.push_str(&csv_record(item, &request.kind));
            } else {
                if !first {
                    chunk.push(',');
                }
                first = false;
                chunk.push_str(&serde_json::to_string(item)?);
            }
        }
        send(chunk)?;
        let cursor = page
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(cursor) = cursor else { break };
        if previous.as_ref() == Some(&cursor) {
            return Err(io::Error::other("export_cursor_invalid").into());
        }
        let control: ControlRequest =
            serde_json::from_value(export_page_request(request, Some(&cursor)))?;
        let response = runtime::request_control(socket, control)?;
        if !response.ok {
            return Err(io::Error::other("export_incomplete").into());
        }
        page = response
            .data
            .ok_or_else(|| io::Error::other("export_page_invalid"))?;
        previous = Some(cursor);
    }
    if !csv {
        send("]".into())?;
    }
    Ok(())
}

fn alias(value: &str, salt: &str) -> String {
    let digest = Sha256::digest(format!("{salt}:{value}"));
    format!(
        "anon-{}",
        digest[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
fn anonymize(value: &mut Value, salt: &str, key: Option<&str>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "note" | "message" | "error" | "detail") {
                    *value = Value::Null;
                } else if matches!(
                    key.as_str(),
                    "pid" | "ppid" | "pid_version" | "uid" | "gid" | "device" | "inode"
                ) {
                    if let Some(number) = value.as_u64() {
                        let namespace = if matches!(key.as_str(), "pid" | "ppid") {
                            "process"
                        } else {
                            key.as_str()
                        };
                        let digest = Sha256::digest(format!("{salt}:{namespace}:{number}"));
                        let pseudonym = u32::from_be_bytes(digest[..4].try_into().unwrap()).max(1);
                        *value = json!(pseudonym);
                    }
                } else {
                    anonymize(value, salt, Some(key));
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                anonymize(item, salt, key);
            }
        }
        Value::String(text) => {
            let public = matches!(
                key,
                Some(
                    "kind" | "rule" | "tool" | "source_stream" | "outcome" | "state" | "operation"
                )
            );
            if !public {
                *text = alias(text, salt);
            }
        }
        _ => {}
    }
}
fn csv_cell(value: String) -> String {
    let prefix = if value.starts_with(['=', '+', '-', '@', '\t', '\r', '\n']) {
        "'"
    } else {
        ""
    };
    format!("\"{prefix}{}\"", value.replace('"', "\"\""))
}
fn csv_record(value: &Value, kind: &str) -> String {
    let event = if kind == "events" {
        &value["event"]
    } else {
        &value["alert"]
    };
    let evidence = if kind == "events" {
        json!({"file":event["file"],"destination":event["destination"],"archive":event["archive"],"directories":value["directories"]})
    } else {
        json!({"paths":event["evidence_paths"],"archive_output_paths":event["archive_output_paths"],"roots":event["roots"],"rule_version":value["rule_version"],"is_read":value["is_read"],"processed":value["processed"],"note":value["note"]})
    };
    [
        value.get("id").unwrap_or(&event["id"]).to_string(),
        event
            .get("kind")
            .unwrap_or(&event["rule"])
            .as_str()
            .unwrap_or("")
            .into(),
        event["process"]["pid"].to_string(),
        event["process"]["executable"].as_str().unwrap_or("").into(),
        event
            .get("received_timestamp_ms")
            .unwrap_or(&event["last_timestamp_ms"])
            .to_string(),
        evidence.to_string(),
        value.to_string(),
    ]
    .into_iter()
    .map(csv_cell)
    .collect::<Vec<_>>()
    .join(",")
        + "\r\n"
}

async fn asset(request: Request) -> Response {
    if request.method() != axum::http::Method::GET {
        return failure(StatusCode::METHOD_NOT_ALLOWED, "method_rejected");
    }
    let path = request.uri().path().trim_start_matches('/');
    if path.starts_with("api/") {
        return failure(StatusCode::NOT_FOUND, "route_unknown");
    }
    let asset = ASSETS.iter().find(|(name, _)| *name == path).or_else(|| {
        if !path.contains('.') {
            ASSETS.iter().find(|(name, _)| *name == "index.html")
        } else {
            None
        }
    });
    match asset {
        Some((name, bytes)) => {
            let mime = if name.ends_with(".html") {
                "text/html; charset=utf-8"
            } else if name.ends_with(".js") {
                "text/javascript; charset=utf-8"
            } else if name.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if name.ends_with(".svg") {
                "image/svg+xml"
            } else if name.ends_with(".woff2") {
                "font/woff2"
            } else {
                "application/octet-stream"
            };
            ([(header::CONTENT_TYPE, mime)], *bytes).into_response()
        }
        None => failure(StatusCode::NOT_FOUND, "web_assets_missing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_compatibility_requires_browser_same_origin_metadata() {
        let expected = "http://127.0.0.1:12345";
        for (origin, site, accepted) in [
            (Some(expected), None, true),
            (Some(expected), Some("same-origin"), true),
            (Some(expected), Some("cross-site"), false),
            (Some("http://127.0.0.1"), Some("same-origin"), true),
            (Some("http://127.0.0.1"), Some("same-site"), false),
            (Some("http://127.0.0.1"), None, false),
            (Some("http://127.0.0.1:54321"), Some("same-origin"), false),
            (Some("https://attacker.example"), Some("same-origin"), false),
            (None, Some("same-origin"), true),
            (None, None, false),
        ] {
            let mut headers = axum::http::HeaderMap::new();
            if let Some(origin) = origin {
                headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
            }
            if let Some(site) = site {
                headers.insert("sec-fetch-site", HeaderValue::from_str(site).unwrap());
            }
            assert_eq!(
                trusted_origin(&headers, expected),
                accepted,
                "origin={origin:?}, site={site:?}"
            );
        }
    }

    #[test]
    fn notification_entry_keeps_authentication_in_fragment_and_links_to_one_alert() {
        let session = Session {
            version: 1,
            pid: 1,
            origin: "http://127.0.0.1:12345".into(),
            token: "synthetic-token".into(),
            host_socket: "/private/tmp/synthetic-host.sock".into(),
        };
        assert_eq!(
            browser_entry(&session, Some("synthetic:1"), false),
            "http://127.0.0.1:12345/?alert=synthetic:1#token=synthetic-token"
        );
        assert_eq!(
            browser_entry(&session, None, true),
            "http://127.0.0.1:12345/?view=alerts#token=synthetic-token"
        );
        assert_eq!(
            browser_entry(&session, None, false),
            "http://127.0.0.1:12345/#token=synthetic-token"
        );
    }
    #[test]
    fn cancelled_history_request_holds_quota_until_queued_discovery_finishes() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (started, wait_started) = std::sync::mpsc::channel();
        let (finish, wait_finish) = std::sync::mpsc::channel();
        let gate = runtime.spawn_blocking(move || {
            started.send(()).unwrap();
            // 阻塞池只有一个工作线程，使实际生产扫描任务稳定排队。
            let _ = wait_finish.recv();
        });
        wait_started.recv_timeout(Duration::from_secs(2)).unwrap();
        let blocking = Arc::new(Semaphore::new(1));
        let state = AppState {
            session: Session {
                version: 1,
                pid: 1,
                origin: "http://127.0.0.1:1".into(),
                token: "synthetic-token".into(),
                host_socket: PathBuf::from("/private/tmp/synthetic/host.sock"),
            },
            username: "synthetic".into(),
            binary: PathBuf::from("/opt/synthetic/codeperimeter"),
            history: HistoryOptions {
                codex_home: None,
                claude_home: None,
                zcode_db: None,
            },
            native_enabled: false,
            blocking: blocking.clone(),
            previews: Arc::new(Mutex::new(BTreeMap::new())),
            jobs: Arc::new(Mutex::new(BTreeMap::new())),
            native_busy: Arc::new(Semaphore::new(1)),
            service_cache: Arc::new(Mutex::new(None)),
        };
        runtime.block_on(async {
            let handler = tokio::spawn(history_preview(State(state)));
            tokio::time::timeout(Duration::from_secs(2), async {
                while blocking.available_permits() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            handler.abort();
            assert!(handler.await.unwrap_err().is_cancelled());
            assert!(
                blocking.clone().try_acquire_owned().is_err(),
                "请求取消后，尚未完成的扫描仍应占用配额"
            );
            finish.send(()).unwrap();
            gate.await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while blocking.available_permits() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
        assert!(blocking.try_acquire_owned().is_ok());
    }
    #[test]
    fn anonymous_export_removes_identity_paths_and_notes() {
        let mut value = json!({"id":"row-1","kind":"open","process":{"pid":99,"executable":"/opt/sample/reader","signing_id":"example.reader"},"file":{"path":"/sample/project/source.txt"},"archive_output_paths":["/sample/output.zip"],"note":"private note","handling_history":[{"note":"private history"}]});
        anonymize(&mut value, "synthetic-salt", None);
        let text = value.to_string();
        assert!(
            !text.contains("/sample/")
                && !text.contains("private")
                && !text.contains("example.reader")
        );
        assert_eq!(value["kind"], "open");
        assert_eq!(
            value["archive_output_paths"][0],
            alias("/sample/output.zip", "synthetic-salt")
        );
        assert_ne!(value["process"]["pid"], 99);
        assert_eq!(value["id"], alias("row-1", "synthetic-salt"));
    }
    #[test]
    fn anonymous_process_links_remain_distinct_and_stable() {
        let mut first = json!({"pid":41,"ppid":40,"pid_version":5});
        let mut same = first.clone();
        let mut parent = json!({"pid":40,"ppid":1,"pid_version":3});
        anonymize(&mut first, "synthetic-salt", None);
        anonymize(&mut same, "synthetic-salt", None);
        anonymize(&mut parent, "synthetic-salt", None);
        assert_eq!(first, same);
        assert_eq!(first["ppid"], parent["pid"]);
        assert_ne!(first["pid"], parent["pid"]);
    }
    #[test]
    fn csv_quotes_and_disarms_formulas() {
        assert_eq!(csv_cell("=SUM(1,2)".into()), "\"'=SUM(1,2)\"");
        assert_eq!(csv_cell("a\"b".into()), "\"a\"\"b\"");
        assert_eq!(csv_cell("\n=1".into()), "\"'\n=1\"");
    }
}
