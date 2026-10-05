//! eslogger schema 1 的窄适配器。原始行和参数只在本次解析期间存在。

use crate::model::{ActivityEvent, ArchiveCommand, EventKind, FileEvidence, ProcessIdentity};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

pub const MAX_LINE_BYTES: usize = 1024 * 1024;
pub const SUBSCRIBED_EVENTS: &[&str] = &[
    "open", "mmap", "exec", "fork", "exit", "create", "write", "rename", "close",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoverageIssue {
    pub code: String,
    pub field: Option<String>,
    pub message: String,
    pub missing_events: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseOutcome {
    pub event: Option<ActivityEvent>,
    pub issues: Vec<CoverageIssue>,
    pub schema_version: Option<u64>,
    pub message_version: Option<u64>,
    pub event_type: Option<u64>,
    pub global_seq: Option<u64>,
    pub event_seq: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdapterHealth {
    pub lines: u64,
    pub parsed_events: u64,
    pub skipped_lines: u64,
    pub lines_with_issues: u64,
    pub last_received_timestamp_ms: Option<i64>,
    pub schema_version: Option<u64>,
    pub message_version: Option<u64>,
    pub global_sequence_available: bool,
    pub event_sequence_available: bool,
    pub sequence_gaps: u64,
}

pub struct EsloggerAdapter {
    source_run_id: String,
    global_seq: Option<u64>,
    event_seq: HashMap<u64, u64>,
    health: AdapterHealth,
}

impl EsloggerAdapter {
    /// 采集器重启必须创建新适配器和新的 source_run_id。
    pub fn new(source_run_id: impl Into<String>) -> Self {
        Self {
            source_run_id: source_run_id.into(),
            global_seq: None,
            event_seq: HashMap::new(),
            health: AdapterHealth::default(),
        }
    }

    /// 必须在目录筛选之前传入每一行，否则序号检查会把筛选当成丢失。
    pub fn parse_line(&mut self, line: &str, received_timestamp_ms: i64) -> ParseOutcome {
        let mut outcome = parse_line(line, &self.source_run_id, received_timestamp_ms);
        if outcome.schema_version == Some(1) {
            if let Some(seq) = outcome.global_seq {
                check_sequence(
                    &mut self.global_seq,
                    seq,
                    "global_seq_num",
                    &mut outcome.issues,
                );
            }
            if let (Some(kind), Some(seq)) = (outcome.event_type, outcome.event_seq) {
                // 只维护订阅的九类序号，未知类型不能使状态表无界增长。
                if event_kind(kind).is_some() {
                    let mut previous = self.event_seq.get(&kind).copied();
                    check_sequence(&mut previous, seq, "seq_num", &mut outcome.issues);
                    if let Some(previous) = previous {
                        self.event_seq.insert(kind, previous);
                    }
                }
            }
            self.health.global_sequence_available = outcome.global_seq.is_some();
            self.health.event_sequence_available = outcome.event_seq.is_some();
        }
        self.health.lines += 1;
        self.health.parsed_events += u64::from(outcome.event.is_some());
        self.health.skipped_lines += u64::from(outcome.event.is_none());
        self.health.lines_with_issues += u64::from(!outcome.issues.is_empty());
        self.health.sequence_gaps += outcome
            .issues
            .iter()
            .filter(|issue| issue.code == "sequence_gap")
            .count() as u64;
        self.health.last_received_timestamp_ms = Some(received_timestamp_ms);
        self.health.schema_version = outcome.schema_version;
        self.health.message_version = outcome.message_version;
        outcome
    }

    pub fn health(&self) -> &AdapterHealth {
        &self.health
    }
}

fn check_sequence(
    previous: &mut Option<u64>,
    next: u64,
    field: &str,
    issues: &mut Vec<CoverageIssue>,
) {
    if let Some(last) = *previous {
        if next > last.saturating_add(1) {
            let mut issue = issue("sequence_gap", field, "观察到事件序号间隙，覆盖存在缺口");
            issue.missing_events = Some(next - last - 1);
            issues.push(issue);
        } else if next <= last {
            issues.push(issue(
                "sequence_regression",
                field,
                "事件序号重复或回退，需检查采集实例与顺序",
            ));
            return;
        }
    }
    *previous = Some(next);
}

fn issue(code: &str, field: &str, message: &str) -> CoverageIssue {
    CoverageIssue {
        code: code.into(),
        field: Some(field.into()),
        message: message.into(),
        missing_events: None,
    }
}

fn missing(issues: &mut Vec<CoverageIssue>, field: &str) {
    issues.push(issue(
        "missing_or_invalid_field",
        field,
        "必要字段缺失或类型不受支持，不能视为完整证据",
    ));
}

/// 无状态格式解析；连续采集使用 EsloggerAdapter 以启用序号检查。
pub fn parse_line(line: &str, source_run_id: &str, received_timestamp_ms: i64) -> ParseOutcome {
    let mut outcome = ParseOutcome {
        event: None,
        issues: Vec::new(),
        schema_version: None,
        message_version: None,
        event_type: None,
        global_seq: None,
        event_seq: None,
    };
    if line.len() > MAX_LINE_BYTES {
        outcome.issues.push(issue(
            "oversized_line",
            "line",
            "事件行超过解析上限，已跳过",
        ));
        return outcome;
    }
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            // 不返回 JSON 错误原文，避免把字段值或原始参数写进健康日志。
            outcome.issues.push(issue(
                "malformed_json",
                "line",
                "事件行不是受支持的 JSON 对象，已跳过",
            ));
            return outcome;
        }
    };
    outcome.schema_version = value["schema_version"].as_u64();
    outcome.message_version = value["version"].as_u64();
    outcome.event_type = value["event_type"].as_u64();
    outcome.global_seq = value["global_seq_num"].as_u64();
    outcome.event_seq = value["seq_num"].as_u64();
    if outcome.schema_version != Some(1) {
        outcome.issues.push(issue(
            "unsupported_schema",
            "schema_version",
            "仅支持已声明的 eslogger schema 1，已跳过",
        ));
        return outcome;
    }
    if value["action_type"].as_u64() != Some(1) {
        outcome.issues.push(issue(
            "unexpected_action",
            "action_type",
            "仅接收 NOTIFY 观察事件，已跳过",
        ));
        return outcome;
    }
    let Some((name, kind)) = outcome.event_type.and_then(event_kind) else {
        outcome.issues.push(issue(
            "unsupported_event",
            "event_type",
            "事件类型不在本版监控范围，已跳过",
        ));
        return outcome;
    };
    let payload = &value["event"][name];
    if !payload.is_object() {
        missing(&mut outcome.issues, "event.payload");
        return outcome;
    }
    if outcome.message_version.is_none() {
        missing(&mut outcome.issues, "version");
    }
    let process_value = match kind {
        EventKind::Exec => &payload["target"],
        EventKind::Fork => &payload["child"],
        _ => &value["process"],
    };
    let Some(process) = process_identity(process_value, &mut outcome.issues) else {
        return outcome;
    };
    let source_timestamp_ms = value["time"]
        .as_str()
        .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
        .map(|time| time.timestamp_millis());
    if source_timestamp_ms.is_none() {
        missing(&mut outcome.issues, "time");
    }
    let global_seq = value["global_seq_num"].as_u64();
    let event_seq = value["seq_num"].as_u64();
    if global_seq.is_none() || event_seq.is_none() {
        outcome.issues.push(issue(
            "sequence_unavailable",
            "sequence",
            "序号不完整，丢事件检查覆盖未知",
        ));
    }
    let file = match kind {
        EventKind::Open => {
            let mut file = file_evidence(&payload["file"], &mut outcome.issues);
            let flags = payload["fflag"].as_i64();
            let allowed = value["action"]["result"]["result"]["flags"].as_u64();
            if let Some(file) = &mut file {
                file.readable =
                    flags.map(|flags| flags & 1 != 0 && allowed.is_none_or(|mask| mask & 1 != 0));
            }
            if flags.is_none() {
                missing(&mut outcome.issues, "event.open.fflag");
            }
            file
        }
        EventKind::Mmap => {
            let mut file = file_evidence(&payload["source"], &mut outcome.issues);
            let protection = payload["protection"].as_i64();
            let allowed = value["action"]["result"]["result"]["auth"].as_u64();
            if let Some(file) = &mut file {
                file.readable =
                    protection.map(|flags| flags & 1 != 0 && allowed.is_none_or(|auth| auth == 0));
            }
            if protection.is_none() {
                missing(&mut outcome.issues, "event.mmap.protection");
            }
            file
        }
        EventKind::Create => destination_evidence(payload, &mut outcome.issues),
        EventKind::Write | EventKind::Close => {
            file_evidence(&payload["target"], &mut outcome.issues)
        }
        EventKind::Rename => file_evidence(&payload["source"], &mut outcome.issues),
        _ => None,
    };
    if matches!(
        kind,
        EventKind::Open
            | EventKind::Mmap
            | EventKind::Create
            | EventKind::Write
            | EventKind::Close
            | EventKind::Rename
    ) && file.is_none()
    {
        return outcome;
    }
    let destination = if kind == EventKind::Rename {
        destination_evidence(payload, &mut outcome.issues).and_then(|file| {
            if file.path_truncated {
                None
            } else {
                Some(file.path)
            }
        })
    } else {
        None
    };
    let modified = if kind == EventKind::Close {
        let modified = payload["modified"].as_bool();
        if modified.is_none() {
            missing(&mut outcome.issues, "event.close.modified");
        }
        modified
    } else {
        None
    };
    let archive = if kind == EventKind::Exec {
        archive_command(payload, &process, &mut outcome.issues)
    } else {
        None
    };
    outcome.event = Some(ActivityEvent {
        source_run_id: source_run_id.into(),
        source_schema_version: outcome.schema_version,
        source_message_version: outcome.message_version,
        source_timestamp_ms,
        received_timestamp_ms,
        global_seq,
        event_seq,
        kind,
        process,
        file,
        destination,
        modified,
        archive,
    });
    outcome
}

fn event_kind(event_type: u64) -> Option<(&'static str, EventKind)> {
    // Apple ESTypes.h 中的 NOTIFY 枚举值；不能仅根据 payload 名字接受 AUTH。
    Some(match event_type {
        9 => ("exec", EventKind::Exec),
        10 => ("open", EventKind::Open),
        11 => ("fork", EventKind::Fork),
        12 => ("close", EventKind::Close),
        13 => ("create", EventKind::Create),
        15 => ("exit", EventKind::Exit),
        20 => ("mmap", EventKind::Mmap),
        25 => ("rename", EventKind::Rename),
        33 => ("write", EventKind::Write),
        _ => return None,
    })
}

fn process_identity(value: &Value, issues: &mut Vec<CoverageIssue>) -> Option<ProcessIdentity> {
    let Some(pid) = value["audit_token"]["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
    else {
        missing(issues, "process.audit_token.pid");
        return None;
    };
    let pid_version = value["audit_token"]["pidversion"]
        .as_u64()
        .and_then(|version| u32::try_from(version).ok());
    if pid_version.is_none() {
        issues.push(issue(
            "process_generation_unavailable",
            "process.audit_token.pidversion",
            "只有 PID，无法精确区分运行实例",
        ));
    }
    let executable = if value["executable"]["path_truncated"].as_bool() == Some(false) {
        let path = path_string(&value["executable"]["path"]);
        if path.is_none() {
            missing(issues, "process.executable.path");
        }
        path
    } else {
        issues.push(issue(
            "process_executable_unavailable",
            "process.executable",
            "可执行路径缺失或截断，身份覆盖不完整",
        ));
        None
    };
    let ppid = value["ppid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok());
    Some(ProcessIdentity {
        pid,
        pid_version,
        ppid,
        executable,
        signing_id: value["signing_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
        team_id: value["team_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
    })
}

fn path_string(value: &Value) -> Option<PathBuf> {
    value
        .as_str()
        .filter(|path| !path.is_empty() && !path.contains('\0'))
        .map(PathBuf::from)
}

fn file_evidence(value: &Value, issues: &mut Vec<CoverageIssue>) -> Option<FileEvidence> {
    let Some(path) = path_string(&value["path"]) else {
        missing(issues, "file.path");
        return None;
    };
    if !path.is_absolute() {
        issues.push(issue(
            "relative_file_path",
            "file.path",
            "系统文件路径不是绝对路径，目录关联未知",
        ));
    }
    let path_truncated = value["path_truncated"].as_bool().unwrap_or_else(|| {
        missing(issues, "file.path_truncated");
        true
    });
    if path_truncated {
        issues.push(issue(
            "path_truncated",
            "file.path",
            "文件路径截断或完整性未知，不能作为完整目录关联",
        ));
    }
    let stat = &value["stat"];
    let is_regular = stat["st_mode"]
        .as_u64()
        .map(|mode| mode & 0o170000 == 0o100000);
    if is_regular.is_none() {
        missing(issues, "file.stat.st_mode");
    }
    Some(FileEvidence {
        path,
        path_truncated,
        device: stat["st_dev"].as_u64(),
        inode: stat["st_ino"].as_u64(),
        is_regular,
        readable: None,
    })
}

fn destination_evidence(payload: &Value, issues: &mut Vec<CoverageIssue>) -> Option<FileEvidence> {
    match payload["destination_type"].as_u64() {
        Some(0) => file_evidence(&payload["destination"]["existing_file"], issues),
        Some(1) => {
            let mut dir = file_evidence(&payload["destination"]["new_path"]["dir"], issues)?;
            let Some(filename) = payload["destination"]["new_path"]["filename"]
                .as_str()
                .filter(|name| {
                    !name.is_empty()
                        && !name.contains('/')
                        && !name.contains('\0')
                        && *name != "."
                        && *name != ".."
                })
            else {
                missing(issues, "destination.new_path.filename");
                return None;
            };
            dir.path.push(filename);
            dir.device = None;
            dir.inode = None;
            dir.is_regular = None;
            Some(dir)
        }
        _ => {
            missing(issues, "destination_type");
            None
        }
    }
}

fn archive_command(
    payload: &Value,
    process: &ProcessIdentity,
    issues: &mut Vec<CoverageIssue>,
) -> Option<ArchiveCommand> {
    let tool = process.executable.as_ref()?.file_name()?.to_str()?;
    if !matches!(
        tool,
        "tar"
            | "bsdtar"
            | "gtar"
            | "zip"
            | "ditto"
            | "gzip"
            | "pigz"
            | "bzip2"
            | "pbzip2"
            | "xz"
            | "zstd"
            | "7z"
            | "7zz"
            | "rar"
    ) {
        return None;
    }
    let Some(raw_args) = payload["args"].as_array() else {
        missing(issues, "event.exec.args");
        return None;
    };
    let args: Option<Vec<&str>> = raw_args.iter().map(Value::as_str).collect();
    let Some(args) = args.filter(|args| !args.is_empty() && args.len() <= 4096) else {
        missing(issues, "event.exec.args");
        return None;
    };
    let cwd = if payload["cwd"]["path_truncated"].as_bool() == Some(false) {
        path_string(&payload["cwd"]["path"]).filter(|path| path.is_absolute())
    } else {
        None
    };
    let result = match tool {
        "tar" | "bsdtar" | "gtar" => tar_paths(tool, &args[1..], cwd.as_deref()),
        "zip" => zip_paths(&args[1..], cwd.as_deref()),
        "ditto" => ditto_paths(&args[1..], cwd.as_deref()),
        "7z" | "7zz" | "rar" => archive_tool_paths(tool, &args[1..], cwd.as_deref()),
        _ => compressor_paths(tool, &args[1..], cwd.as_deref()),
    };
    match result {
        Ok(Some((input_paths, mut output_paths))) => {
            if input_paths.is_empty() {
                issues.push(issue(
                    "archive_input_source_unknown",
                    "event.exec.args",
                    "命令未提供可解析的直接输入；标准输入或隐式输入的项目来源未知",
                ));
            }
            let output_path = if output_paths.len() == 1 {
                output_paths.pop()
            } else {
                None
            };
            Some(ArchiveCommand {
                tool: tool.into(),
                input_paths,
                output_path,
                output_paths,
                cwd,
            })
        }
        Ok(None) => None,
        Err(error) => {
            let (code, message) = match error {
                ArchiveParseError::Arguments => (
                    "archive_arguments_incomplete",
                    "归档参数或目录格式超出支持范围，未推断路径",
                ),
                ArchiveParseError::InputList => (
                    "archive_input_list_unsupported",
                    "命令使用列表文件或间接输入，未读取列表或推断路径",
                ),
            };
            issues.push(issue(code, "event.exec.args", message));
            None
        }
    }
}

type ArchivePaths = (Vec<PathBuf>, Vec<PathBuf>);

#[derive(Clone, Copy)]
enum ArchiveParseError {
    Arguments,
    InputList,
}

type ArchiveResult = Result<Option<ArchivePaths>, ArchiveParseError>;

fn resolve_path(path: &str, cwd: Option<&Path>) -> Result<PathBuf, ArchiveParseError> {
    if path.is_empty() || path.contains('\0') {
        return Err(ArchiveParseError::Arguments);
    }
    let path = Path::new(path);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.ok_or(ArchiveParseError::Arguments)?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn option_value<'a>(
    args: &[&'a str],
    index: &mut usize,
    attached: Option<&'a str>,
) -> Result<&'a str, ArchiveParseError> {
    let value = if let Some(value) = attached {
        value
    } else {
        *index += 1;
        args.get(*index)
            .copied()
            .ok_or(ArchiveParseError::Arguments)?
    };
    if value.is_empty() || value.contains('\0') {
        return Err(ArchiveParseError::Arguments);
    }
    Ok(value)
}

fn direct_paths(inputs: &[&str], outputs: &[&str], cwd: Option<&Path>) -> ArchiveResult {
    Ok(Some((
        inputs
            .iter()
            .filter(|input| **input != "-")
            .map(|input| resolve_path(input, cwd))
            .collect::<Result<_, _>>()?,
        outputs
            .iter()
            .filter(|output| **output != "-")
            .map(|output| resolve_path(output, cwd))
            .collect::<Result<_, _>>()?,
    )))
}

fn add_archive_extension(outputs: &mut [PathBuf], extension: &str) {
    for output in outputs {
        if output.extension().is_none() {
            output.set_extension(extension);
        }
    }
}

fn tar_paths(tool: &str, args: &[&str], cwd: Option<&Path>) -> ArchiveResult {
    // 先判定操作，再解析路径；反向操作不受缺失 cwd 或未知路径格式影响。
    let mut operations = Vec::new();
    let mut outputs = Vec::new();
    let mut creating = false;
    let mut non_creating = false;
    let mut operands = false;
    let mut problem = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        if !operands && arg == "--" {
            operands = true;
        } else if !operands && arg.starts_with("--") {
            let (flag, attached) = arg
                .split_once('=')
                .map_or((arg, None), |(flag, value)| (flag, Some(value)));
            match flag {
                "--create" | "--append" | "--update" => creating = true,
                "--extract" | "--list" | "--get" | "--delete" | "--compare" | "--diff"
                | "--help" | "--version" => non_creating = true,
                "--file" | "--directory" | "--cd" if flag != "--cd" || tool != "gtar" => {
                    match option_value(args, &mut index, attached) {
                        Ok(value) if flag == "--file" => outputs.push(value),
                        Ok(value) => operations.push((true, value)),
                        Err(error) => problem = Some(error),
                    }
                }
                "--files-from" | "--exclude-from" => {
                    let _ = option_value(args, &mut index, attached);
                    problem = Some(ArchiveParseError::InputList);
                }
                "--options" | "--block-size" if tool != "gtar" => {
                    if let Err(error) = option_value(args, &mut index, attached) {
                        problem = Some(error);
                    }
                }
                "--format" | "--exclude" | "--blocking-factor" | "--use-compress-program" => {
                    if let Err(error) = option_value(args, &mut index, attached) {
                        problem = Some(error);
                    }
                }
                "--gzip" | "--bzip2" | "--xz" | "--zstd" | "--verbose" | "--auto-compress"
                | "--no-recursion" | "--recursion" | "--dereference" | "--numeric-owner"
                | "--exclude-vcs" | "--acls" | "--no-acls" | "--xattrs" | "--no-xattrs"
                    if attached.is_none() => {}
                "--mac-metadata" | "--no-mac-metadata" if attached.is_none() && tool != "gtar" => {}
                _ => problem = Some(ArchiveParseError::Arguments),
            }
        } else if !operands
            && (arg.starts_with('-')
                || index == 0 && arg.chars().all(|c| "crutxazjJvpfChHTXIbd".contains(c)))
        {
            let legacy = !arg.starts_with('-');
            let flags = arg.trim_start_matches('-');
            for (offset, flag) in flags.char_indices() {
                match flag {
                    'c' | 'r' | 'u' => creating = true,
                    't' | 'x' | 'd' => non_creating = true,
                    'a' | 'z' | 'j' | 'J' | 'v' | 'p' | 'h' => {}
                    'H' if tool != "gtar" => {}
                    'T' | 'X' | 'I' if flag != 'I' || tool != "gtar" => {
                        let remainder = &flags[offset + 1..];
                        let _ = option_value(
                            args,
                            &mut index,
                            (!legacy && !remainder.is_empty()).then_some(remainder),
                        );
                        problem = Some(ArchiveParseError::InputList);
                        if !legacy {
                            break;
                        }
                    }
                    'f' | 'C' | 'b' | 'I' | 'H' => {
                        let remainder = &flags[offset + flag.len_utf8()..];
                        match option_value(
                            args,
                            &mut index,
                            (!legacy && !remainder.is_empty()).then_some(remainder),
                        ) {
                            Ok(value) if flag == 'f' => outputs.push(value),
                            Ok(value) if flag == 'C' => operations.push((true, value)),
                            Ok(_) => {}
                            Err(error) => problem = Some(error),
                        }
                        if !legacy {
                            break;
                        }
                    }
                    _ => problem = Some(ArchiveParseError::Arguments),
                }
            }
        } else if arg.starts_with('@') {
            problem = Some(ArchiveParseError::InputList);
        } else {
            operations.push((false, arg));
        }
        index += 1;
    }
    if !creating || non_creating {
        return Ok(None);
    }
    if let Some(error) = problem {
        return Err(error);
    }
    if outputs.len() > 1 {
        return Err(ArchiveParseError::Arguments);
    }
    let mut inputs = Vec::new();
    let mut input_cwd = cwd.map(Path::to_path_buf);
    for (directory, path) in operations {
        let path = resolve_path(path, input_cwd.as_deref())?;
        if directory {
            input_cwd = Some(path);
        } else {
            inputs.push(path);
        }
    }
    let (_, outputs) = direct_paths(&[], &outputs, cwd)?.unwrap();
    Ok(Some((inputs, outputs)))
}

fn zip_paths(args: &[&str], cwd: Option<&Path>) -> ArchiveResult {
    let mut paths = Vec::new();
    let mut operands = false;
    let mut non_creating = false;
    let mut testing = false;
    let mut output = None;
    let mut problem = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        if !operands && arg == "--" {
            operands = true;
        } else if !operands && arg.starts_with("--") {
            let (flag, attached) = arg
                .split_once('=')
                .map_or((arg, None), |(flag, value)| (flag, Some(value)));
            match flag {
                "--test" => testing = true,
                "--delete" | "--show-files" | "--help" | "--version" | "--copy"
                | "--adjust-sfx" | "--fix" | "--fixfix" => non_creating = true,
                "--out" | "--password" | "--compression-method" => {
                    match option_value(args, &mut index, attached) {
                        Ok(value) if flag == "--out" => output = Some(value),
                        Ok(_) => {}
                        Err(error) => problem = Some(error),
                    }
                }
                "--recurse-paths" | "--quiet" | "--junk-paths" | "--update" | "--freshen"
                | "--encrypt"
                    if attached.is_none() => {}
                _ => problem = Some(ArchiveParseError::Arguments),
            }
        } else if !operands && arg != "-" && arg.starts_with('-') {
            if matches!(arg, "-sf" | "-su" | "-sc" | "-h2" | "-FF") {
                non_creating = true;
            } else if arg == "-@" {
                problem = Some(ArchiveParseError::InputList);
            } else {
                let flags = &arg[1..];
                for (offset, flag) in flags.char_indices() {
                    match flag {
                        'T' => testing = true,
                        'd' | 'h' | 'U' | 'A' | 'F' => non_creating = true,
                        'r'
                        | 'q'
                        | 'j'
                        | '0'..='9'
                        | 'v'
                        | 'X'
                        | 'e'
                        | 'y'
                        | 'u'
                        | 'f'
                        | 'g'
                        | 'D' => {}
                        'P' | 'Z' => {
                            let remainder = &flags[offset + 1..];
                            if let Err(error) = option_value(
                                args,
                                &mut index,
                                (!remainder.is_empty()).then_some(remainder),
                            ) {
                                problem = Some(error);
                            }
                            break;
                        }
                        _ => problem = Some(ArchiveParseError::Arguments),
                    }
                }
            }
        } else if !operands && arg.starts_with('@') {
            problem = Some(ArchiveParseError::InputList);
        } else {
            paths.push(arg);
        }
        index += 1;
    }
    if non_creating || testing && paths.len() <= 1 {
        return Ok(None);
    }
    if let Some(error) = problem {
        return Err(error);
    }
    // 无参数 zip 是 stdin 到 stdout；不从 cwd 推导管道中的项目来源。
    if paths.is_empty() {
        return direct_paths(&[], &[], cwd);
    }
    let archive = paths.remove(0);
    let (inputs, mut outputs) = direct_paths(&paths, &[output.unwrap_or(archive)], cwd)?.unwrap();
    add_archive_extension(&mut outputs, "zip");
    Ok(Some((inputs, outputs)))
}

fn ditto_paths(args: &[&str], cwd: Option<&Path>) -> ArchiveResult {
    let mut creating = false;
    let mut non_creating = false;
    let mut paths = Vec::new();
    let mut operands = false;
    let mut problem = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        match arg {
            "--" if !operands => operands = true,
            "-c" if !operands => creating = true,
            "-x" | "-h" | "--help" if !operands => non_creating = true,
            "--bom" if !operands => {
                let _ = option_value(args, &mut index, None);
                problem = Some(ArchiveParseError::InputList);
            }
            "--zlibCompressionLevel" | "--arch" if !operands => {
                if let Err(error) = option_value(args, &mut index, None) {
                    problem = Some(error);
                }
            }
            "-k"
            | "-z"
            | "-j"
            | "-v"
            | "-V"
            | "-X"
            | "--keepParent"
            | "--sequesterRsrc"
            | "--rsrc"
            | "--norsrc"
            | "--extattr"
            | "--noextattr"
            | "--acl"
            | "--noacl"
            | "--qtn"
            | "--noqtn"
            | "--nocache"
            | "--hfsCompression"
            | "--nohfsCompression"
            | "--preserveHFSCompression"
            | "--nopreserveHFSCompression"
                if !operands => {}
            _ if !operands && arg != "-" && arg.starts_with('-') => {
                problem = Some(ArchiveParseError::Arguments);
            }
            _ => paths.push(arg),
        }
        index += 1;
    }
    if !creating || non_creating {
        return Ok(None);
    }
    if let Some(error) = problem {
        return Err(error);
    }
    if paths.len() != 2 {
        return Err(ArchiveParseError::Arguments);
    }
    direct_paths(&paths[..1], &paths[1..], cwd)
}

fn set_compressor_mode(
    tool: &str,
    mode: &mut Option<bool>,
    conflict: &mut bool,
    compressing: bool,
) {
    // 已核对的实现按最后操作模式生效；未核对的组合保留缺口。
    if tool == "pbzip2" && mode.is_some_and(|previous| previous != compressing) {
        *conflict = true;
    }
    *mode = Some(compressing);
}

fn compressor_paths(tool: &str, args: &[&str], cwd: Option<&Path>) -> ArchiveResult {
    let gzip = matches!(tool, "gzip" | "pigz");
    let bzip = matches!(tool, "bzip2" | "pbzip2");
    let mut inputs = Vec::new();
    let mut output = None;
    let mut suffix = match tool {
        "gzip" | "pigz" => ".gz",
        "bzip2" | "pbzip2" => ".bz2",
        "xz" => ".xz",
        _ => ".zst",
    };
    let mut stdout = false;
    let mut recursive = false;
    let mut operands = false;
    let mut non_creating = false;
    let mut mode = None;
    let mut mode_conflict = false;
    let mut problem = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        if !operands && arg == "--" {
            operands = true;
        } else if !operands && arg.starts_with("--") {
            // pbzip2 的长选项别名尚未取得对应版本证据，不借用 bzip2 的白名单。
            if tool == "pbzip2" {
                problem = Some(ArchiveParseError::Arguments);
                index += 1;
                continue;
            }
            let (flag, attached) = arg
                .split_once('=')
                .map_or((arg, None), |(flag, value)| (flag, Some(value)));
            match flag {
                "--decompress" | "--test" if attached.is_none() => {
                    set_compressor_mode(tool, &mut mode, &mut mode_conflict, false);
                }
                "--uncompress" | "--list" if !bzip && attached.is_none() => {
                    set_compressor_mode(tool, &mut mode, &mut mode_conflict, false);
                }
                "--compress" if !gzip && attached.is_none() => {
                    set_compressor_mode(tool, &mut mode, &mut mode_conflict, true);
                }
                "--help" | "--version" if attached.is_none() => non_creating = true,
                "--long-help" if tool == "xz" && attached.is_none() => non_creating = true,
                "--license" if (gzip || bzip) && attached.is_none() => non_creating = true,
                "--train" | "--train-cover" | "--train-fastcover" | "--train-legacy"
                    if tool == "zstd" =>
                {
                    non_creating = true
                }
                "--stdout" if attached.is_none() => {
                    stdout = true;
                    output = None;
                }
                "--to-stdout" if (gzip || tool == "xz") && attached.is_none() => {
                    stdout = true;
                    output = None;
                }
                "--recursive" if gzip && attached.is_none() => recursive = true,
                "--files" | "--files0" if tool == "xz" => {
                    problem = Some(ArchiveParseError::InputList);
                }
                "--filelist" if tool == "zstd" => {
                    let _ = option_value(args, &mut index, attached);
                    problem = Some(ArchiveParseError::InputList);
                }
                "--suffix" if gzip || tool == "xz" => {
                    match option_value(args, &mut index, attached) {
                        Ok(value) if !value.contains('/') => suffix = value,
                        _ => problem = Some(ArchiveParseError::Arguments),
                    }
                }
                "--format" if tool == "xz" || tool == "zstd" => {
                    match option_value(args, &mut index, attached) {
                        Ok("xz") => suffix = ".xz",
                        Ok("lzma") => suffix = ".lzma",
                        Ok("zstd") if tool == "zstd" => suffix = ".zst",
                        Ok("gzip") if tool == "zstd" => suffix = ".gz",
                        Ok("lz4") if tool == "zstd" => suffix = ".lz4",
                        _ => problem = Some(ArchiveParseError::Arguments),
                    }
                }
                "--threads" | "--check" | "--memlimit-compress" if tool == "xz" => {
                    if let Err(error) = option_value(args, &mut index, attached) {
                        problem = Some(error);
                    }
                }
                "--processes" | "--blocksize" if tool == "pigz" => {
                    if let Err(error) = option_value(args, &mut index, attached) {
                        problem = Some(error);
                    }
                }
                "--fast" | "--long" if tool == "zstd" => {}
                "--keep" | "--force" | "--quiet" | "--verbose" if attached.is_none() => {}
                "--fast" | "--best" if (gzip || bzip) && attached.is_none() => {}
                "--no-name" | "--name" if gzip && attached.is_none() => {}
                "--extreme" if tool == "xz" && attached.is_none() => {}
                "--rm" | "--ultra" | "--single-thread" | "--no-progress" | "--progress"
                | "--rsyncable" | "--no-check" | "--check" | "--no-dictID"
                    if tool == "zstd" && attached.is_none() => {}
                _ => problem = Some(ArchiveParseError::Arguments),
            }
        } else if !operands && arg != "-" && arg.starts_with('-') {
            let flags = &arg[1..];
            // zstd 的级别及线程数可超过一位，整段数字不是多个独立选项。
            if tool == "zstd" && flags.bytes().all(|byte| byte.is_ascii_digit()) {
                index += 1;
                continue;
            }
            for (offset, flag) in flags.char_indices() {
                match flag {
                    'd' | 't' => {
                        set_compressor_mode(tool, &mut mode, &mut mode_conflict, false);
                    }
                    'l' if !bzip => {
                        set_compressor_mode(tool, &mut mode, &mut mode_conflict, false);
                    }
                    'h' | 'V' => non_creating = true,
                    'L' if bzip || gzip => non_creating = true,
                    'H' if tool == "xz" || tool == "zstd" => non_creating = true,
                    'b' if tool == "zstd" => {
                        non_creating = true;
                        break;
                    }
                    'c' => {
                        stdout = true;
                        output = None;
                    }
                    'r' if gzip || tool == "zstd" => recursive = true,
                    'r' if tool == "pbzip2" => {}
                    '1'..='9' | 'k' | 'f' | 'q' | 'v' => {}
                    '0' if tool == "pigz" || tool == "xz" => {}
                    'z' if bzip || tool == "xz" || tool == "zstd" => {
                        set_compressor_mode(tool, &mut mode, &mut mode_conflict, true);
                    }
                    'z' if tool == "pigz" => suffix = ".zz",
                    'n' | 'N' if gzip => {}
                    'e' if tool == "xz" => {}
                    's' if bzip => {}
                    'a' | 'i' if tool == "pigz" => {}
                    'S' if gzip || tool == "xz" => {
                        let remainder = &flags[offset + 1..];
                        match option_value(
                            args,
                            &mut index,
                            (!remainder.is_empty()).then_some(remainder),
                        ) {
                            Ok(value) if !value.contains('/') => suffix = value,
                            _ => problem = Some(ArchiveParseError::Arguments),
                        }
                        break;
                    }
                    'o' if tool == "zstd" => {
                        let remainder = &flags[offset + 1..];
                        match option_value(
                            args,
                            &mut index,
                            (!remainder.is_empty()).then_some(remainder),
                        ) {
                            Ok(value) => {
                                output = Some(value);
                                stdout = false;
                            }
                            Err(error) => problem = Some(error),
                        }
                        break;
                    }
                    'p' | 'b' | 'm' if tool == "pbzip2" => {
                        let remainder = &flags[offset + 1..];
                        if remainder.is_empty()
                            || !remainder.bytes().all(|byte| byte.is_ascii_digit())
                        {
                            problem = Some(ArchiveParseError::Arguments);
                        }
                        break;
                    }
                    'p' | 'b' if tool == "pigz" => {
                        let remainder = &flags[offset + 1..];
                        if let Err(error) = option_value(
                            args,
                            &mut index,
                            (!remainder.is_empty()).then_some(remainder),
                        ) {
                            problem = Some(error);
                        }
                        break;
                    }
                    'T' if tool == "xz" || tool == "zstd" => {
                        let remainder = &flags[offset + 1..];
                        if tool == "zstd" && remainder.is_empty() {
                            problem = Some(ArchiveParseError::Arguments);
                            break;
                        }
                        if let Err(error) = option_value(
                            args,
                            &mut index,
                            (!remainder.is_empty()).then_some(remainder),
                        ) {
                            problem = Some(error);
                        }
                        break;
                    }
                    'D' if tool == "zstd" => {
                        let remainder = &flags[offset + 1..];
                        if let Err(error) = option_value(
                            args,
                            &mut index,
                            (!remainder.is_empty()).then_some(remainder),
                        ) {
                            problem = Some(error);
                        }
                        break;
                    }
                    _ => problem = Some(ArchiveParseError::Arguments),
                }
            }
        } else if !operands && arg.starts_with('@') {
            problem = Some(ArchiveParseError::InputList);
        } else {
            inputs.push(arg);
        }
        index += 1;
    }
    if non_creating {
        return Ok(None);
    }
    if mode_conflict {
        return Err(ArchiveParseError::Arguments);
    }
    if mode == Some(false) {
        return Ok(None);
    }
    if let Some(error) = problem {
        return Err(error);
    }
    let (inputs, _) = direct_paths(&inputs, &[], cwd)?.unwrap();
    let outputs = if stdout {
        Vec::new()
    } else if let Some(output) = output {
        direct_paths(&[], &[output], cwd)?.unwrap().1
    } else if recursive {
        // 未展开目录，不能给目录伪造一个带后缀的输出文件。
        Vec::new()
    } else {
        inputs
            .iter()
            .map(|input| {
                let mut path = input.as_os_str().to_os_string();
                path.push(suffix);
                PathBuf::from(path)
            })
            .collect()
    };
    Ok(Some((inputs, outputs)))
}

fn archive_tool_paths(tool: &str, args: &[&str], cwd: Option<&Path>) -> ArchiveResult {
    let Some(command_index) = args.iter().position(|arg| !arg.starts_with('-')) else {
        return Err(ArchiveParseError::Arguments);
    };
    let command = args[command_index];
    let rar = tool == "rar";
    if !(matches!(command, "a" | "u") || rar && command == "f") {
        let non_creating = if rar {
            matches!(
                command,
                "c" | "ch"
                    | "cw"
                    | "d"
                    | "e"
                    | "k"
                    | "l"
                    | "lb"
                    | "lt"
                    | "ltb"
                    | "p"
                    | "r"
                    | "rc"
                    | "rr"
                    | "rv"
                    | "s"
                    | "t"
                    | "v"
                    | "vb"
                    | "vt"
                    | "vtb"
                    | "x"
            )
        } else {
            matches!(
                command,
                "b" | "d" | "e" | "h" | "i" | "l" | "rn" | "t" | "x"
            )
        };
        return if non_creating {
            Ok(None)
        } else {
            Err(ArchiveParseError::Arguments)
        };
    }
    let mut paths = Vec::new();
    let mut stdout = false;
    let mut stdin = false;
    let mut format = None;
    let mut operands = false;
    let mut problem = None;
    for (index, arg) in args.iter().enumerate() {
        if index == command_index {
            continue;
        }
        if !operands && *arg == "--" {
            operands = true;
        } else if !operands
            && (arg.starts_with('@')
                || arg.contains('@')
                    && (arg.starts_with("-i")
                        || arg.starts_with("-x")
                        || arg.starts_with("-ai")
                        || arg.starts_with("-ax")))
        {
            problem = Some(ArchiveParseError::InputList);
        } else if !operands && arg.starts_with('-') {
            if !rar && *arg == "-so" {
                stdout = true;
            } else if !rar && arg.starts_with("-t") && arg.len() > 2 {
                format = Some(&arg[2..]);
            } else if arg.starts_with("-si") {
                // stdin 后的名字是归档内标签，不能当作本机项目路径。
                stdin = true;
            } else if arg.starts_with("-p") || rar && arg.starts_with("-hp") {
                // 密码仅影响工具行为，不进入标准事件。
            } else if matches!(*arg, "-r" | "-r0" | "-r-" | "-y")
                || !rar && seven_zip_switch(arg)
                || rar && rar_switch(arg)
            {
            } else {
                problem = Some(ArchiveParseError::Arguments);
            }
        } else {
            paths.push(*arg);
        }
    }
    if let Some(error) = problem {
        return Err(error);
    }
    if paths.is_empty() {
        return Err(ArchiveParseError::Arguments);
    }
    let output = paths.remove(0);
    if stdin {
        // 已分别核对 7-Zip 与 RAR：-si 使用流成员，不把磁盘位置参数作为输入。
        paths.clear();
    }
    if stdout {
        if command != "a" {
            return Err(ArchiveParseError::Arguments);
        }
        let format = format.or_else(|| match Path::new(output).extension()?.to_str()? {
            "tar" => Some("tar"),
            "gz" => Some("gzip"),
            "bz2" => Some("bzip2"),
            "xz" => Some("xz"),
            _ => None,
        });
        if !matches!(format, Some("tar" | "gzip" | "bzip2" | "xz")) {
            return Err(ArchiveParseError::Arguments);
        }
    }
    let extension = if rar {
        "rar"
    } else {
        match format.unwrap_or("7z") {
            "7z" => "7z",
            "zip" => "zip",
            "tar" => "tar",
            "gzip" => "gz",
            "bzip2" => "bz2",
            "xz" => "xz",
            _ => return Err(ArchiveParseError::Arguments),
        }
    };
    let outputs = [output];
    let (inputs, mut outputs) =
        direct_paths(&paths, if stdout { &[] } else { &outputs }, cwd)?.unwrap();
    add_archive_extension(&mut outputs, extension);
    Ok(Some((inputs, outputs)))
}

fn seven_zip_switch(arg: &str) -> bool {
    matches!(
        arg,
        "-bd"
            | "-bt"
            | "-sdel"
            | "-spd"
            | "-spf"
            | "-ssw"
            | "-snh"
            | "-snl"
            | "-stl"
            | "-sfx"
            | "-ssc"
            | "-ssc-"
    ) || arg.starts_with("-t") && arg.len() > 2
        || arg.starts_with("-mx") && arg[3..].trim_start_matches('=').parse::<u8>().is_ok()
        || arg.starts_with("-mmt") && arg[4..].trim_start_matches('=').parse::<u32>().is_ok()
        || matches!(arg, "-mhe=on" | "-mhe=off")
        || arg.starts_with("-bb") && matches!(&arg[3..], "0" | "1" | "2" | "3")
        || arg.starts_with("-w")
        || arg.starts_with("-x!")
}

fn rar_switch(arg: &str) -> bool {
    matches!(
        arg,
        "-ep" | "-ep1" | "-ep2" | "-ep3" | "-idq" | "-inul" | "-cfg-" | "-s" | "-s-" | "-k"
    ) || arg.starts_with("-m") && arg[2..].parse::<u8>().is_ok()
        || arg.starts_with("-mt") && arg[3..].parse::<u32>().is_ok()
        || arg.starts_with("-w")
        || arg.starts_with("-x") && arg.len() > 2
}
