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
    if !matches!(tool, "tar" | "bsdtar" | "zip") {
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
    let result = if tool == "zip" {
        zip_paths(&args[1..], cwd.as_deref())
    } else {
        tar_paths(&args[1..], cwd.as_deref())
    };
    match result {
        Ok(Some((input_paths, output_path))) => Some(ArchiveCommand {
            tool: tool.into(),
            input_paths,
            output_path,
            cwd,
        }),
        Ok(None) => None,
        Err(()) => {
            issues.push(issue(
                "archive_arguments_incomplete",
                "event.exec.args",
                "归档参数或目录格式超出支持范围，未推断路径",
            ));
            None
        }
    }
}

type ArchivePaths = (Vec<PathBuf>, Option<PathBuf>);

fn resolve_path(path: &str, cwd: Option<&Path>) -> Result<PathBuf, ()> {
    if path.is_empty() || path.contains('\0') {
        return Err(());
    }
    let path = Path::new(path);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.ok_or(())?.join(path)
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

fn tar_paths(args: &[&str], cwd: Option<&Path>) -> Result<Option<ArchivePaths>, ()> {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut input_cwd = cwd.map(Path::to_path_buf);
    let mut creating = false;
    let mut non_creating = false;
    let mut operands = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        if !operands && arg == "--" {
            operands = true;
        } else if !operands && matches!(arg, "--create" | "--append" | "--update") {
            creating = true;
        } else if !operands && matches!(arg, "--extract" | "--list" | "--get") {
            non_creating = true;
        } else if !operands && (arg == "--file" || arg == "--directory") {
            index += 1;
            let path = args.get(index).ok_or(())?;
            if arg == "--file" {
                output = if *path == "-" {
                    None
                } else {
                    Some(resolve_path(path, cwd)?)
                };
            } else {
                input_cwd = Some(resolve_path(path, input_cwd.as_deref())?);
            }
        } else if !operands && (arg.starts_with("--file=") || arg.starts_with("--directory=")) {
            let (flag, path) = arg.split_once('=').ok_or(())?;
            if flag == "--file" {
                output = if path == "-" {
                    None
                } else {
                    Some(resolve_path(path, cwd)?)
                };
            } else {
                input_cwd = Some(resolve_path(path, input_cwd.as_deref())?);
            }
        } else if !operands
            && matches!(
                arg,
                "--gzip" | "--bzip2" | "--xz" | "--zstd" | "--verbose" | "--auto-compress"
            )
        {
        } else if !operands
            && (arg.starts_with('-')
                || index == 0 && arg.chars().all(|c| "crutxazjJvpfCh".contains(c)))
        {
            if arg.starts_with("--") {
                return Err(());
            }
            let flags = arg.trim_start_matches('-');
            for (offset, flag) in flags.char_indices() {
                match flag {
                    'c' | 'r' | 'u' => creating = true,
                    't' | 'x' => non_creating = true,
                    'a' | 'z' | 'j' | 'J' | 'v' | 'p' | 'h' => {}
                    'f' | 'C' => {
                        let remainder = &flags[offset + flag.len_utf8()..];
                        let path = if remainder.is_empty() {
                            index += 1;
                            *args.get(index).ok_or(())?
                        } else {
                            remainder
                        };
                        if flag == 'f' {
                            output = if path == "-" {
                                None
                            } else {
                                Some(resolve_path(path, cwd)?)
                            };
                        } else {
                            input_cwd = Some(resolve_path(path, input_cwd.as_deref())?);
                        }
                        break;
                    }
                    _ => return Err(()),
                }
            }
        } else if arg.starts_with('@') {
            return Err(());
        } else {
            inputs.push(resolve_path(arg, input_cwd.as_deref())?);
        }
        index += 1;
    }
    if !creating || non_creating {
        return Ok(None);
    }
    Ok(Some((inputs, output)))
}

fn zip_paths(args: &[&str], cwd: Option<&Path>) -> Result<Option<ArchivePaths>, ()> {
    let mut paths = Vec::new();
    let mut operands = false;
    for arg in args {
        if !operands && *arg == "--" {
            operands = true;
        } else if !operands && arg.starts_with('-') {
            if arg
                .chars()
                .skip(1)
                .any(|flag| !"rqj09vXeyugD".contains(flag))
                || arg.len() < 2
            {
                return Err(());
            }
        } else {
            paths.push(resolve_path(arg, cwd)?);
        }
    }
    if paths.len() < 2 {
        return Err(());
    }
    let output = paths.remove(0);
    Ok(Some((paths, Some(output))))
}
