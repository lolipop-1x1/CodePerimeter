//! 本机网页的受限原生适配：系统授权、目录选择及 launchd 状态。

use crate::Result;
use crate::runtime::{ControlRequest, request_control};
use crate::service::{OperationReport, ServicePlan};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CANCELLED: &str = "__CODEPERIMETER_CANCELLED__";
const DENIED: &str = "__CODEPERIMETER_DENIED__";
const FAILED: &str = "__CODEPERIMETER_FAILED__";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    Install,
    Start,
    Pause,
    Resume,
    Uninstall,
}

impl ServiceAction {
    fn command(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Start => "start",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Uninstall => "uninstall",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobStatus {
    pub loaded: bool,
    pub running: bool,
    pub disabled: Option<bool>,
    pub last_exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceStatus {
    pub uid: u32,
    pub installed: bool,
    pub installed_binary_trusted: bool,
    pub collector: JobStatus,
    pub analyzer: JobStatus,
    pub notification: JobStatus,
    pub paused: Option<bool>,
    pub status_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceActionResult {
    pub state: String,
    pub report: Option<OperationReport>,
    pub error: Option<String>,
}

impl ServiceActionResult {
    fn failure(code: &str) -> Self {
        Self {
            state: "failed".into(),
            report: None,
            error: Some(code.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryChoice {
    pub path: Option<PathBuf>,
    pub cancelled: bool,
}

/// 状态来自当前 launchd 与宿主；读取失败不以“正常”或“已暂停”替代。
pub fn service_status(username: &str, source_binary: &Path) -> Result<ServiceStatus> {
    let plan = ServicePlan::new(username, source_binary)
        .map_err(|_| io::Error::other("service_account_invalid"))?;
    let installed =
        plan.installed_binary.is_file() && plan.jobs.iter().all(|job| job.plist_path.is_file());
    let mut status = ServiceStatus {
        uid: plan.uid,
        installed,
        installed_binary_trusted: crate::service::checked_root_path(&plan.installed_binary).is_ok(),
        collector: JobStatus::default(),
        analyzer: JobStatus::default(),
        notification: JobStatus::default(),
        paused: None,
        status_error: None,
    };
    if !cfg!(target_os = "macos") {
        status.status_error = Some("platform_unsupported".into());
        return Ok(status);
    }
    let disabled = launchctl_output(&["print-disabled", "system"]);
    for job in &plan.jobs {
        let target = format!("{}/{}", job.domain, job.label);
        let mut observed = match launchctl_output(&["print", &target]) {
            Ok((true, text)) => parse_job_status(&text),
            Ok((false, _)) => JobStatus::default(),
            Err(_) => {
                status.status_error = Some("launchd_status_unavailable".into());
                JobStatus::default()
            }
        };
        if job.domain == "system" {
            observed.disabled = match &disabled {
                Ok((true, text)) => parse_disabled(text, &job.label),
                _ => {
                    status.status_error = Some("launchd_disabled_unavailable".into());
                    None
                }
            };
        }
        if job.argv.get(1).map(String::as_str) == Some("collector") {
            status.collector = observed;
        } else if job.argv.get(1).map(String::as_str) == Some("daemon") {
            status.analyzer = observed;
        } else {
            status.notification = observed;
        }
    }
    if status.status_error.is_some() {
        return Ok(status);
    }
    if let Ok(response) = request_control(&plan.control_socket, ControlRequest::Status) {
        let host_paused = response
            .data
            .as_ref()
            .and_then(|data| data.get("monitoring_paused"))
            .and_then(|value| value.as_bool());
        status.paused = reconcile_pause(
            &status.collector,
            response.ok.then_some(host_paused).flatten(),
        );
    }
    Ok(status)
}

fn reconcile_pause(collector: &JobStatus, host_paused: Option<bool>) -> Option<bool> {
    match (collector.disabled, collector.loaded, host_paused) {
        (Some(true), false, Some(true)) => Some(true),
        (Some(false), true, Some(false)) => Some(false),
        _ => None,
    }
}

fn parse_job_status(text: &str) -> JobStatus {
    let mut status = JobStatus {
        loaded: true,
        ..JobStatus::default()
    };
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("pid = ") {
            status.running = value.parse::<u32>().is_ok_and(|pid| pid > 0);
        }
        if let Some(value) = line.strip_prefix("last exit code = ") {
            status.last_exit_code = value.parse().ok();
        }
    }
    status
}

fn parse_disabled(text: &str, label: &str) -> Option<bool> {
    let prefix = format!("\"{label}\" => ");
    match text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(&prefix))
    {
        Some(value) => match value.trim() {
            "true" | "disabled" => Some(true),
            "false" | "enabled" => Some(false),
            _ => None,
        },
        // 未列入 disabled 覆盖表的任务使用默认启用状态。
        None => Some(false),
    }
}

fn launchctl_output(args: &[&str]) -> io::Result<(bool, String)> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if output.stdout.len() > 1024 * 1024 {
        return Err(io::Error::other("launchd_status_oversized"));
    }
    // 113 明确表示任务不存在；其他非零结果保留查询失败，不能推断任务已停止。
    let job_missing = args.first() == Some(&"print") && output.status.code() == Some(113);
    if !(output.status.success() || job_missing) {
        return Err(io::Error::other("launchd_status_unavailable"));
    }
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

/// 固定白名单动作通过系统授权执行；调用参数不含密码或自由形式 shell 命令。
pub fn run_service_action(
    action: ServiceAction,
    username: &str,
    source_binary: &Path,
    control_socket: &Path,
) -> Result<ServiceActionResult> {
    if !cfg!(target_os = "macos") {
        return Ok(ServiceActionResult::failure("platform_unsupported"));
    }
    let plan = match ServicePlan::new(username, source_binary) {
        Ok(plan) => plan,
        Err(_) => return Ok(ServiceActionResult::failure("service_account_invalid")),
    };
    if plan.uid != unsafe { libc::geteuid() } || plan.control_socket != control_socket {
        return Ok(ServiceActionResult::failure("service_account_mismatch"));
    }
    let binary = match validate_source_binary(source_binary, plan.uid) {
        Ok(binary) => binary,
        Err(_) => return Ok(ServiceActionResult::failure("service_binary_untrusted")),
    };
    let script = admin_script(action, &plan.username, &binary)?;
    let output = match run_osascript(&script) {
        Ok(output) => output,
        Err(_) => {
            return Ok(ServiceActionResult::failure(
                "native_authorization_unavailable",
            ));
        }
    };
    let mut result = parse_admin_result(&output);
    if result.state != "succeeded" {
        record_operation(control_socket, action, &result.state);
        return Ok(result);
    }
    if matches!(
        action,
        ServiceAction::Pause | ServiceAction::Resume | ServiceAction::Start
    ) {
        let paused = action == ServiceAction::Pause;
        if !set_monitoring(control_socket, paused, true) {
            result.state = "failed".into();
            result.error = Some("monitoring_state_unconfirmed".into());
        }
    }
    record_operation(control_socket, action, &result.state);
    Ok(result)
}

fn validate_source_binary(path: &Path, uid: u32) -> io::Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !path.is_absolute()
        || !metadata.is_file()
        || metadata.mode() & 0o022 != 0
        || metadata.mode() & 0o111 == 0
        || metadata.uid() != uid && metadata.uid() != 0
    {
        return Err(io::Error::other("service_binary_untrusted"));
    }
    fs::canonicalize(path)
}

fn set_monitoring(path: &Path, paused: bool, await_host: bool) -> bool {
    let request: ControlRequest = match serde_json::from_value(json!({
        "operation": "console",
        "payload": {"request": {"action": "monitoring_set", "payload": {"paused": paused}}}
    })) {
        Ok(request) => request,
        Err(_) => return false,
    };
    let deadline = Instant::now() + Duration::from_secs(if await_host { 5 } else { 0 });
    loop {
        if let Ok(response) = request_control(path, request.clone()) {
            return response.ok
                && response
                    .data
                    .as_ref()
                    .and_then(|data| data.get("paused"))
                    .and_then(|value| value.as_bool())
                    == Some(paused);
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn record_operation(path: &Path, action: ServiceAction, outcome: &str) {
    let request = serde_json::from_value::<ControlRequest>(json!({
        "operation": "console",
        "payload": {"request": {"action": "record_operation", "payload": {
            "operation": action.command(), "outcome": outcome
        }}}
    }));
    if let Ok(request) = request {
        let _ = request_control(path, request);
    }
}

fn shell_argument(value: &str) -> io::Result<String> {
    if value.contains('\0') {
        return Err(io::Error::other("native_argument_invalid"));
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn applescript_string(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

fn admin_script(action: ServiceAction, username: &str, binary: &Path) -> io::Result<String> {
    let binary = binary
        .to_str()
        .ok_or_else(|| io::Error::other("native_argument_invalid"))?;
    let command = [binary, "service", action.command(), "--user", username]
        .into_iter()
        .map(shell_argument)
        .collect::<io::Result<Vec<_>>>()?
        .join(" ");
    // 部分失败仍返回结构化步骤；系统 stderr 不带回网页，退出失败由报告判定。
    let command = format!("{command} 2>/dev/null || :");
    Ok(standard_additions_terms(&format!(
        "try\nreturn do shell script {} with administrator privileges without altering line endings\non error number errorNumber\nif errorNumber is -128 or errorNumber is -60006 then\nreturn \"{CANCELLED}\"\nelse if errorNumber is -1743 or errorNumber is -60005 then\nreturn \"{DENIED}\"\nelse\nreturn \"{FAILED}\"\nend if\nend try",
        applescript_string(&command)
    )))
}

fn parse_admin_result(output: &str) -> ServiceActionResult {
    match output.trim() {
        CANCELLED => ServiceActionResult {
            state: "cancelled".into(),
            report: None,
            error: None,
        },
        DENIED => ServiceActionResult::failure("native_authorization_denied"),
        FAILED => ServiceActionResult::failure("service_command_failed"),
        text => {
            let value = serde_json::from_str::<serde_json::Value>(text).ok();
            let ok = value.as_ref().and_then(|value| value.get("ok")?.as_bool());
            let report = value.as_ref().and_then(|value| {
                serde_json::from_value::<OperationReport>(json!({
                    "steps": value.get("steps")?,
                    "data_preserved": value.get("data_preserved")?
                }))
                .ok()
            });
            match report {
                Some(report)
                    if ok == Some(true)
                        && !report.steps.is_empty()
                        && report.steps.iter().all(|step| step.success) =>
                {
                    ServiceActionResult {
                        state: "succeeded".into(),
                        report: Some(report),
                        error: None,
                    }
                }
                Some(report)
                    if ok == Some(false) && report.steps.iter().any(|step| !step.success) =>
                {
                    ServiceActionResult {
                        state: "failed".into(),
                        report: Some(report),
                        error: Some("service_steps_failed".into()),
                    }
                }
                _ => ServiceActionResult::failure("service_result_invalid"),
            }
        }
    }
}

fn run_osascript(script: &str) -> io::Result<String> {
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 {
        return Err(io::Error::other("native_dialog_failed"));
    }
    String::from_utf8(output.stdout).map_err(|_| io::Error::other("native_result_invalid"))
}

/// 目录选择由系统窗口完成；取消不制造一个空路径配置。
pub fn choose_directory() -> Result<DirectoryChoice> {
    if !cfg!(target_os = "macos") {
        return Err(io::Error::other("platform_unsupported").into());
    }
    let output = run_osascript(&directory_script())
        .map_err(|_| io::Error::other("directory_dialog_unavailable"))?;
    parse_directory_result(&output)
}

fn directory_script() -> String {
    directory_script_for_locale(&crate::i18n::current_locale())
}

fn directory_script_for_locale(locale: &str) -> String {
    standard_additions_terms(&format!(
        "try\nset chosenFolder to choose folder with prompt {}\nreturn \"__CODEPERIMETER_DIRECTORY__\" & linefeed & POSIX path of chosenFolder\non error number errorNumber\nif errorNumber is -128 then\nreturn \"{CANCELLED}\"\nelse\nreturn \"{FAILED}\"\nend if\nend try",
        applescript_string(&crate::i18n::message(
            locale,
            "native.directory_prompt",
            &[]
        ))
    ))
}

fn standard_additions_terms(script: &str) -> String {
    // 仅显式加载系统术语，不把管理员命令发送给另一应用。
    format!(
        "using terms from application \"/System/Library/ScriptingAdditions/StandardAdditions.osax\"\n{script}\nend using terms from"
    )
}

fn parse_directory_result(output: &str) -> Result<DirectoryChoice> {
    if output.trim() == CANCELLED {
        return Ok(DirectoryChoice {
            path: None,
            cancelled: true,
        });
    }
    let path = output
        .strip_prefix("__CODEPERIMETER_DIRECTORY__\n")
        .and_then(|path| path.strip_suffix('\n'))
        .ok_or_else(|| io::Error::other("directory_dialog_failed"))?;
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_dir() {
        return Err(io::Error::other("directory_selection_invalid").into());
    }
    Ok(DirectoryChoice {
        path: Some(path),
        cancelled: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    #[test]
    fn authorization_quotes_literal_arguments_and_never_takes_password() {
        let script = admin_script(
            ServiceAction::Pause,
            "synthetic'account",
            Path::new("/synthetic/bin/a'\"$(marker);x"),
        )
        .unwrap();
        assert!(script.contains("administrator privileges"));
        assert!(!script.contains(" password "));
        assert!(!script.contains("user name"));
        assert!(script.contains("'pause'"));
        assert_eq!(shell_argument("a'b").unwrap(), "'a'\"'\"'b'");
        assert!(shell_argument("invalid\0argument").is_err());
        let literal = "quote'$(printf injected);\n\"value\"";
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s' {}", shell_argument(literal).unwrap()))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), literal);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_scripts_compile_without_opening_or_authorizing_dialogs() {
        let directory = tempfile::tempdir().unwrap();
        let scripts = [
            admin_script(
                ServiceAction::Pause,
                "synthetic-account",
                Path::new("/synthetic/bin/a'\"$(marker);x"),
            )
            .unwrap(),
            directory_script_for_locale("zh-CN"),
            directory_script_for_locale("en"),
        ];
        for (index, script) in scripts.iter().enumerate() {
            let status = Command::new("/usr/bin/osacompile")
                .arg("-o")
                .arg(directory.path().join(format!("script-{index}.scpt")))
                .arg("-e")
                .arg(script)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "原生脚本语法必须通过系统编译器");
        }
    }

    #[test]
    fn directory_prompt_follows_the_product_language() {
        let chinese = directory_script_for_locale("zh-CN");
        let english = directory_script_for_locale("en");
        assert!(chinese.contains("选择需要监控的目录"));
        assert!(english.contains("Choose a directory to monitor"));
        assert!(!english.contains("选择需要监控的目录"));
        assert!(chinese.contains(CANCELLED) && english.contains(CANCELLED));
    }

    #[test]
    fn authorization_outcomes_are_static_and_partial_steps_never_succeed() {
        assert_eq!(parse_admin_result(CANCELLED).state, "cancelled");
        assert_eq!(
            parse_admin_result(DENIED).error.as_deref(),
            Some("native_authorization_denied")
        );
        assert_eq!(
            parse_admin_result(FAILED).error.as_deref(),
            Some("service_command_failed")
        );
        let output = r#"{"ok":true,"data_preserved":true,"steps":[{"label":"collector","success":false,"message":"合成失败"}]}"#;
        assert_eq!(parse_admin_result(output).state, "failed");
        let failed = output.replace("\"ok\":true", "\"ok\":false");
        let partial = parse_admin_result(&failed);
        assert_eq!(partial.error.as_deref(), Some("service_steps_failed"));
        assert_eq!(partial.report.unwrap().steps[0].message, "合成失败");
        let result = parse_admin_result("synthetic-private-marker");
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("synthetic-private-marker")
        );
        let success = output.replace("\"success\":false", "\"success\":true");
        assert_eq!(parse_admin_result(&success).state, "succeeded");
    }

    #[test]
    fn launchd_loaded_running_and_pause_are_independent() {
        let job = parse_job_status("service = {\nstate = waiting\npid = 41\nlast exit code = 7\n}");
        assert!(job.loaded && job.running);
        assert_eq!(job.last_exit_code, Some(7));
        assert_eq!(
            parse_disabled(
                "\"com.codeperimeter.collector.1\" => true",
                "com.codeperimeter.collector.1"
            ),
            Some(true)
        );
        assert_eq!(
            parse_disabled(
                "\"com.codeperimeter.collector.1\" => false",
                "com.codeperimeter.collector.1"
            ),
            Some(false)
        );
        assert_eq!(
            parse_disabled("\"collector\" => disabled", "collector"),
            Some(true)
        );
        assert_eq!(
            parse_disabled("\"collector\" => enabled", "collector"),
            Some(false)
        );
        assert_eq!(
            parse_disabled("\"collector\" => unknown", "collector"),
            None
        );
        assert_eq!(parse_disabled("", "collector"), Some(false));
        let mut paused = JobStatus {
            disabled: Some(true),
            ..JobStatus::default()
        };
        assert_eq!(reconcile_pause(&paused, Some(true)), Some(true));
        assert_eq!(reconcile_pause(&paused, None), None);
        assert_eq!(reconcile_pause(&paused, Some(false)), None);
        paused.loaded = true;
        assert_eq!(reconcile_pause(&paused, Some(true)), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_query_failure_is_not_an_unloaded_job() {
        assert!(launchctl_output(&["synthetic-invalid-command"]).is_err());
        let target = format!(
            "system/com.codeperimeter.synthetic-missing.{}",
            std::process::id()
        );
        assert!(!launchctl_output(&["print", &target]).unwrap().0);
    }

    #[test]
    fn selection_cancel_and_errors_do_not_echo_private_output() {
        assert!(parse_directory_result(CANCELLED).unwrap().cancelled);
        let error = parse_directory_result("synthetic-private-marker").unwrap_err();
        assert_eq!(error.to_string(), "directory_dialog_failed");
        let directory = tempfile::tempdir().unwrap();
        let output = format!(
            "__CODEPERIMETER_DIRECTORY__\n{}\n",
            directory.path().display()
        );
        assert_eq!(
            parse_directory_result(&output).unwrap().path.unwrap(),
            directory.path()
        );
    }

    #[test]
    fn source_binary_rejects_symlink_and_other_write_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("synthetic-binary");
        fs::write(&binary, "synthetic").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let uid = unsafe { libc::geteuid() };
        assert!(validate_source_binary(&binary, uid).is_ok());
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o775)).unwrap();
        assert!(validate_source_binary(&binary, uid).is_err());
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink(&binary, &alias).unwrap();
        assert!(validate_source_binary(&alias, uid).is_err());
    }

    #[test]
    fn monitoring_coordination_requires_the_exact_persisted_acknowledgement() {
        for acknowledged in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
            let socket = directory.path().join("host.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                let request: ControlRequest = serde_json::from_str(&request).unwrap();
                assert!(matches!(
                    request,
                    ControlRequest::Console {
                        request: crate::console::ConsoleRequest::MonitoringSet { paused: true }
                    }
                ));
                writeln!(
                    stream,
                    "{}",
                    json!({"ok":true,"error":null,"data":{"paused":acknowledged}})
                )
                .unwrap();
            });
            assert_eq!(set_monitoring(&socket, true, false), acknowledged);
            server.join().unwrap();
        }
        let directory = tempfile::tempdir().unwrap();
        assert!(!set_monitoring(
            &directory.path().join("missing.sock"),
            true,
            false
        ));
    }

    #[test]
    fn monitoring_coordination_retries_a_transient_disconnect() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let socket = directory.path().join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let server = thread::spawn(move || {
            // 第一条连接在任务切换期间断开；不得把随后正确的 ACK 丢掉。
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            writeln!(
                stream,
                "{}",
                json!({"ok":true,"error":null,"data":{"paused":true}})
            )
            .unwrap();
        });
        assert!(set_monitoring(&socket, true, true));
        server.join().unwrap();
    }
}
