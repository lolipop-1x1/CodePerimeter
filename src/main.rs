use clap::{Args, Parser, Subcommand, ValueEnum};
use codeperimeter::history::{
    self, DirectoryCandidate, DirectoryStatus, DiscoveryCounts, DiscoveryGap, HistoryOptions,
    ObservedVersion,
};
use codeperimeter::model::{AlertRule, EventKind, now_ms};
use codeperimeter::runtime::{self, ControlRequest, DirectoryImport, RuntimeOptions};
use codeperimeter::service::{self, CollectorOptions, OperationReport, ServicePlan};
use codeperimeter::storage::{
    AlertFilter, EventFilter, HealthFilter, NotificationFilter, NotificationOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HISTORY_PREVIEW_VERSION: u32 = 1;
const MAX_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;

type CliResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "codeperimeter",
    version,
    about = "CodePerimeter 本机文件活动观察工具"
)]
struct Cli {
    #[arg(long, global = true, value_name = "PATH")]
    host_socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Watch {
        #[command(subcommand)]
        command: WatchCommand,
    },
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    Events(EventQuery),
    Alerts(AlertQuery),
    Health(HealthQuery),
    Notifications(NotificationQuery),
    Stats {
        #[command(subcommand)]
        command: StatsCommand,
    },
    Status,
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    Collector(CollectorCommand),
    Daemon(DaemonCommand),
    Notify(NotifyCommand),
}

#[derive(Debug, Subcommand)]
enum WatchCommand {
    Add {
        #[arg(required = true, value_name = "DIRECTORY")]
        directories: Vec<PathBuf>,
    },
    Remove {
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,
    },
    List,
}

#[derive(Debug, Subcommand)]
enum HistoryCommand {
    Preview {
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        #[arg(long, value_name = "DIRECTORY")]
        codex_home: Option<PathBuf>,
        #[arg(long, value_name = "DIRECTORY")]
        claude_home: Option<PathBuf>,
        #[arg(long, value_name = "FILE")]
        zcode_db: Option<PathBuf>,
    },
    Import {
        #[arg(long, value_name = "FILE")]
        preview: PathBuf,
        #[arg(
            long,
            value_name = "INDEX",
            value_delimiter = ',',
            conflicts_with = "all_available"
        )]
        index: Vec<usize>,
        #[arg(long, conflicts_with = "index")]
        all_available: bool,
    },
}

#[derive(Debug, Args)]
struct EventQuery {
    #[arg(long)]
    directory: Option<PathBuf>,
    #[arg(long)]
    pid: Option<u32>,
    #[arg(long, value_enum)]
    kind: Option<EventKindArg>,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    since_ms: Option<i64>,
    #[arg(long)]
    until_ms: Option<i64>,
    #[arg(long, default_value_t = 100)]
    limit: usize,
}

#[derive(Debug, Args)]
struct AlertQuery {
    #[arg(long)]
    directory: Option<PathBuf>,
    #[arg(long)]
    pid: Option<u32>,
    #[arg(long, value_enum)]
    rule: Option<AlertRuleArg>,
    #[arg(long)]
    since_ms: Option<i64>,
    #[arg(long)]
    until_ms: Option<i64>,
    #[arg(long, default_value_t = 100)]
    limit: usize,
}

#[derive(Debug, Args)]
struct HealthQuery {
    #[arg(long)]
    source_run_id: Option<String>,
    #[arg(long)]
    component: Option<String>,
    #[arg(long)]
    code: Option<String>,
    #[arg(long)]
    since_ms: Option<i64>,
    #[arg(long)]
    until_ms: Option<i64>,
    #[arg(long, default_value_t = 100)]
    limit: usize,
}

#[derive(Debug, Args)]
struct NotificationQuery {
    #[arg(long)]
    alert_id: Option<String>,
    #[arg(long, value_enum)]
    outcome: Option<NotificationOutcomeArg>,
    #[arg(long)]
    since_ms: Option<i64>,
    #[arg(long)]
    until_ms: Option<i64>,
    #[arg(long, default_value_t = 100)]
    limit: usize,
}

#[derive(Debug, Subcommand)]
enum StatsCommand {
    Show {
        #[arg(long)]
        since_ms: Option<i64>,
        #[arg(long)]
        until_ms: Option<i64>,
    },
    ClearCumulative,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    Plan(UserOption),
    Install(UserOption),
    Start(UserOption),
    Stop(UserOption),
    Uninstall(UserOption),
}

#[derive(Debug, Args)]
struct UserOption {
    #[arg(long, value_name = "NAME")]
    user: String,
}

#[derive(Debug, Args)]
struct CollectorCommand {
    #[arg(long, value_name = "PATH")]
    socket: PathBuf,
    #[arg(long, value_name = "UID")]
    allowed_uid: u32,
}

#[derive(Debug, Args)]
struct DaemonCommand {
    #[arg(long, value_name = "PATH")]
    socket: PathBuf,
    #[arg(long, value_name = "PATH")]
    control_socket: PathBuf,
    #[arg(long, value_name = "PATH")]
    db: PathBuf,
    #[arg(long, default_value_t = 50)]
    bulk_file_threshold: usize,
    #[arg(long, default_value_t = 10_000)]
    bulk_window_ms: i64,
}

#[derive(Debug, Args)]
struct NotifyCommand {
    #[arg(long, value_name = "PATH")]
    control_socket: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EventKindArg {
    Open,
    Mmap,
    Create,
    Write,
    Close,
    Rename,
    Exec,
    Fork,
    Exit,
}

impl From<EventKindArg> for EventKind {
    fn from(value: EventKindArg) -> Self {
        match value {
            EventKindArg::Open => Self::Open,
            EventKindArg::Mmap => Self::Mmap,
            EventKindArg::Create => Self::Create,
            EventKindArg::Write => Self::Write,
            EventKindArg::Close => Self::Close,
            EventKindArg::Rename => Self::Rename,
            EventKindArg::Exec => Self::Exec,
            EventKindArg::Fork => Self::Fork,
            EventKindArg::Exit => Self::Exit,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AlertRuleArg {
    #[value(name = "bulk_file_access")]
    BulkFileAccess,
    #[value(name = "archive_command")]
    ArchiveCommand,
    #[value(name = "archive_output")]
    ArchiveOutput,
}

impl From<AlertRuleArg> for AlertRule {
    fn from(value: AlertRuleArg) -> Self {
        match value {
            AlertRuleArg::BulkFileAccess => Self::BulkFileAccess,
            AlertRuleArg::ArchiveCommand => Self::ArchiveCommand,
            AlertRuleArg::ArchiveOutput => Self::ArchiveOutput,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum NotificationOutcomeArg {
    Sent,
    Failed,
    Deferred,
    Acknowledged,
}

impl From<NotificationOutcomeArg> for NotificationOutcome {
    fn from(value: NotificationOutcomeArg) -> Self {
        match value {
            NotificationOutcomeArg::Sent => Self::Sent,
            NotificationOutcomeArg::Failed => Self::Failed,
            NotificationOutcomeArg::Deferred => Self::Deferred,
            NotificationOutcomeArg::Acknowledged => Self::Acknowledged,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryPreview {
    format_version: u32,
    generated_timestamp_ms: i64,
    candidates: Vec<IndexedCandidate>,
    counts: DiscoveryCounts,
    gaps: Vec<DiscoveryGap>,
    versions: Vec<ObservedVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedCandidate {
    index: usize,
    candidate: DirectoryCandidate,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return ExitCode::SUCCESS;
            }
            eprintln!("命令参数无效；运行 codeperimeter --help 查看用法。");
            return ExitCode::from(code as u8);
        }
    };

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("执行失败：{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> CliResult<()> {
    match cli.command {
        Command::Watch { command } => run_watch(command, &host_socket(cli.host_socket)?),
        Command::History { command } => run_history(command, cli.host_socket.as_deref()),
        Command::Events(query) => {
            let filter = EventFilter {
                directory: query.directory,
                pid: query.pid,
                kind: query.kind.map(Into::into),
                file_path: query.file,
                since_ms: query.since_ms,
                until_ms: query.until_ms,
                limit: query.limit,
            };
            send_request(
                &host_socket(cli.host_socket)?,
                ControlRequest::QueryEvents { filter },
            )
        }
        Command::Alerts(query) => {
            let filter = AlertFilter {
                directory: query.directory,
                pid: query.pid,
                rule: query.rule.map(Into::into),
                since_ms: query.since_ms,
                until_ms: query.until_ms,
                limit: query.limit,
            };
            send_request(
                &host_socket(cli.host_socket)?,
                ControlRequest::QueryAlerts { filter },
            )
        }
        Command::Health(query) => {
            let filter = HealthFilter {
                source_run_id: query.source_run_id,
                component: query.component,
                code: query.code,
                since_ms: query.since_ms,
                until_ms: query.until_ms,
                limit: query.limit,
            };
            send_request(
                &host_socket(cli.host_socket)?,
                ControlRequest::QueryHealth { filter },
            )
        }
        Command::Notifications(query) => {
            let filter = NotificationFilter {
                alert_id: query.alert_id,
                outcome: query.outcome.map(Into::into),
                since_ms: query.since_ms,
                until_ms: query.until_ms,
                limit: query.limit,
            };
            send_request(
                &host_socket(cli.host_socket)?,
                ControlRequest::QueryNotifications { filter },
            )
        }
        Command::Stats { command } => run_stats(command, &host_socket(cli.host_socket)?),
        Command::Status => send_request(&host_socket(cli.host_socket)?, ControlRequest::Status),
        Command::Service { command } => run_service(command),
        Command::Collector(command) => service::run_collector(CollectorOptions {
            socket_path: command.socket,
            allowed_uid: command.allowed_uid,
        }),
        Command::Daemon(command) => runtime::run_daemon(RuntimeOptions {
            collector_socket: command.socket,
            control_socket: command.control_socket,
            database_path: command.db,
            bulk_file_threshold: command.bulk_file_threshold,
            bulk_window_ms: command.bulk_window_ms,
        }),
        Command::Notify(command) => runtime::run_notify(&command.control_socket),
    }
}

fn run_watch(command: WatchCommand, socket: &Path) -> CliResult<()> {
    match command {
        WatchCommand::Add { directories } => {
            let entries = directories
                .into_iter()
                .map(|directory| {
                    let canonical = fs::canonicalize(&directory).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "監控目錄不存在或無法解析")
                    })?;
                    if !canonical.is_dir() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "監控路徑必須是目錄",
                        ));
                    }
                    Ok(DirectoryImport {
                        path: canonical,
                        sources: vec!["manual".into()],
                    })
                })
                .collect::<Result<Vec<_>, io::Error>>()?;
            send_request(socket, ControlRequest::AddDirectories { entries })
        }
        WatchCommand::Remove { directory } => {
            let path = if directory.exists() {
                fs::canonicalize(&directory)?
            } else if directory.is_absolute() {
                directory
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "移除不存在的目录时请提供此前配置的绝对路径",
                )
                .into());
            };
            send_request(socket, ControlRequest::RemoveDirectory { path })
        }
        WatchCommand::List => send_request(socket, ControlRequest::ListDirectories),
    }
}

fn run_history(command: HistoryCommand, socket: Option<&Path>) -> CliResult<()> {
    match command {
        HistoryCommand::Preview {
            output,
            codex_home,
            claude_home,
            zcode_db,
        } => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "找不到用户主目录"))?;
            let defaults = HistoryOptions::defaults(&home);
            let report = history::discover(&HistoryOptions {
                codex_home: codex_home.or(defaults.codex_home),
                claude_home: claude_home.or(defaults.claude_home),
                zcode_db: zcode_db.or(defaults.zcode_db),
            });
            let preview = HistoryPreview {
                format_version: HISTORY_PREVIEW_VERSION,
                generated_timestamp_ms: now_ms(),
                candidates: report
                    .candidates
                    .into_iter()
                    .enumerate()
                    .map(|(index, candidate)| IndexedCandidate { index, candidate })
                    .collect(),
                counts: report.counts,
                gaps: report.gaps,
                versions: report.versions,
            };
            let serialized = serde_json::to_vec_pretty(&preview)?;
            if let Some(path) = output {
                fs::write(&path, &serialized)?;
                print_json(&json!({
                    "ok": true,
                    "data": {
                        "preview_file": path,
                        "candidate_count": preview.candidates.len(),
                        "available_count": preview.candidates.iter()
                            .filter(|candidate| candidate.candidate.status == DirectoryStatus::Available)
                            .count()
                    }
                }))
            } else {
                println!("{}", String::from_utf8(serialized)?);
                Ok(())
            }
        }
        HistoryCommand::Import {
            preview,
            index,
            all_available,
        } => {
            if all_available && !index.is_empty() || !all_available && index.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "必须且只能指定 --all-available 或一个以上 --index",
                )
                .into());
            }
            let metadata = fs::metadata(&preview)?;
            if metadata.len() > MAX_PREVIEW_BYTES {
                return Err(
                    io::Error::new(io::ErrorKind::InvalidData, "预览快照超过 16 MiB 上限").into(),
                );
            }
            let bytes = fs::read(preview)?;
            let saved: HistoryPreview = serde_json::from_slice(&bytes)?;
            let entries = history_import_entries(&saved, &index, all_available)?;
            let socket = match socket {
                Some(path) => path.to_path_buf(),
                None => host_socket(None)?,
            };
            send_request(&socket, ControlRequest::AddDirectories { entries })
        }
    }
}

fn history_import_entries(
    preview: &HistoryPreview,
    requested_indices: &[usize],
    all_available: bool,
) -> CliResult<Vec<DirectoryImport>> {
    if preview.format_version != HISTORY_PREVIEW_VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "预览快照版本不受支持").into());
    }
    for (position, indexed) in preview.candidates.iter().enumerate() {
        if indexed.index != position {
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "预览快照候选序号不连续").into(),
            );
        }
    }
    let indices: BTreeSet<usize> = if all_available {
        preview
            .candidates
            .iter()
            .filter(|indexed| indexed.candidate.status == DirectoryStatus::Available)
            .map(|indexed| indexed.index)
            .collect()
    } else {
        requested_indices.iter().copied().collect()
    };
    if indices.is_empty() {
        return Err(
            io::Error::new(io::ErrorKind::InvalidInput, "快照中没有可导入的候选目录").into(),
        );
    }

    let mut selected = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for index in indices {
        let indexed = preview.candidates.get(index).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "候选序号超出预览快照范围")
        })?;
        let candidate = &indexed.candidate;
        if candidate.status != DirectoryStatus::Available {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "只能导入预览时标记为 available 的目录",
            )
            .into());
        }
        let path = candidate
            .canonical_path
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "可用候选缺少规范路径"))?;
        let current = fs::canonicalize(path).map_err(|_| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "预览目录当前不可用；请重新生成预览",
            )
        })?;
        if current.as_path() != path.as_path() || !current.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "预览目录已变化；请重新生成预览",
            )
            .into());
        }
        let mut candidate_sources = BTreeSet::new();
        for origin in &candidate.origins {
            let source = match origin.source {
                history::HistorySource::Codex => "codex",
                history::HistorySource::ClaudeCode => "claude_code",
                history::HistorySource::Zcode => "zcode",
                history::HistorySource::Manual => continue,
            };
            candidate_sources.insert(source.into());
        }
        if candidate_sources.is_empty() {
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "候选目录没有可导入的历史来源").into(),
            );
        }
        selected
            .entry(current)
            .or_default()
            .extend(candidate_sources);
    }
    Ok(selected
        .into_iter()
        .map(|(path, sources)| DirectoryImport {
            path,
            sources: sources.into_iter().collect(),
        })
        .collect())
}

fn run_stats(command: StatsCommand, socket: &Path) -> CliResult<()> {
    match command {
        StatsCommand::Show { since_ms, until_ms } => {
            send_request(socket, ControlRequest::Stats { since_ms, until_ms })
        }
        StatsCommand::ClearCumulative => send_request(socket, ControlRequest::ClearCumulativeStats),
    }
}

fn run_service(command: ServiceCommand) -> CliResult<()> {
    let user = match &command {
        ServiceCommand::Plan(options)
        | ServiceCommand::Install(options)
        | ServiceCommand::Start(options)
        | ServiceCommand::Stop(options)
        | ServiceCommand::Uninstall(options) => &options.user,
    };
    let source_binary = std::env::current_exe()?;
    let plan = ServicePlan::new(user, &source_binary)?;
    match command {
        ServiceCommand::Plan(_) => print_json(&json!({"ok": true, "data": plan})),
        ServiceCommand::Install(_) => run_operation(plan.install()?),
        ServiceCommand::Start(_) => run_operation(plan.start()?),
        ServiceCommand::Stop(_) => run_operation(plan.stop()?),
        ServiceCommand::Uninstall(_) => run_operation(plan.uninstall()?),
    }
}

fn run_operation(report: OperationReport) -> CliResult<()> {
    ensure_operation_succeeded(&report)?;
    print_json(&json!({
        "ok": true,
        "data_preserved": report.data_preserved,
        "steps": report.steps
    }))
}

fn ensure_operation_succeeded(report: &OperationReport) -> CliResult<()> {
    let failed: Vec<&str> = report
        .steps
        .iter()
        .filter(|step| !step.success)
        .map(|step| step.label.as_str())
        .collect();
    if failed.is_empty() {
        return Ok(());
    }
    Err(io::Error::other(format!("服务操作步骤失败：{}", failed.join(", "))).into())
}

fn host_socket(override_path: Option<PathBuf>) -> CliResult<PathBuf> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "找不到用户主目录"))?;
    Ok(home
        .join("Library/Application Support/CodePerimeter")
        .join("host.sock"))
}

fn send_request(socket: &Path, request: ControlRequest) -> CliResult<()> {
    let response = runtime::request_control(socket, request)?;
    if !response.ok {
        return Err(
            io::Error::other(response.error.unwrap_or_else(|| "宿主拒绝了请求".into())).into(),
        );
    }
    print_json(&json!({
        "ok": true,
        "data": response.data
    }))
}

fn print_json(value: &Value) -> CliResult<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeperimeter::service::OperationStep;

    #[test]
    fn failed_service_step_causes_cli_failure() {
        let report = OperationReport {
            steps: vec![
                OperationStep {
                    label: "collector".into(),
                    success: true,
                    message: "已启动".into(),
                },
                OperationStep {
                    label: "daemon".into(),
                    success: false,
                    message: "启动失败".into(),
                },
            ],
            data_preserved: true,
        };
        let error = ensure_operation_succeeded(&report).unwrap_err();
        assert!(error.to_string().contains("daemon"));
    }

    #[test]
    fn history_import_uses_saved_candidate_even_if_history_changes() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("saved-project");
        fs::create_dir(&project).unwrap();
        let canonical = fs::canonicalize(&project).unwrap();
        let preview = HistoryPreview {
            format_version: HISTORY_PREVIEW_VERSION,
            generated_timestamp_ms: 1,
            candidates: vec![IndexedCandidate {
                index: 0,
                candidate: DirectoryCandidate {
                    raw_paths: vec![project.clone()],
                    canonical_path: Some(canonical.clone()),
                    status: DirectoryStatus::Available,
                    origins: vec![history::DirectoryOrigin {
                        source: history::HistorySource::Codex,
                        history_file: None,
                        record_type: Some("session_meta".into()),
                        line: Some(1),
                        version: Some("synthetic".into()),
                        field: "cwd".into(),
                        occurrences: 1,
                    }],
                },
            }],
            counts: DiscoveryCounts::default(),
            gaps: Vec::new(),
            versions: Vec::new(),
        };
        // 新增历史来源后仍接受现有 v1 快照。
        let legacy_bytes = serde_json::to_vec(&preview).unwrap();
        let restored: HistoryPreview = serde_json::from_slice(&legacy_bytes).unwrap();
        let entries = history_import_entries(&restored, &[0], false).unwrap();
        assert_eq!(entries[0].path, canonical);
        assert_eq!(entries[0].sources, vec!["codex"]);
    }
}
