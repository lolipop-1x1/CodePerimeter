use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use codeperimeter::history::{
    self, DirectoryCandidate, DirectoryStatus, DiscoveryCounts, DiscoveryGap, HistoryOptions,
    ObservedVersion,
};
use codeperimeter::i18n;
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
    #[arg(long, global = true, value_name = "LANGUAGE")]
    language: Option<String>,
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
    /// 启动或打开本机网页控制台；关闭浏览器不停止监控。
    Ui(UiCommand),
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
    Pause(UserOption),
    Resume(UserOption),
    Stop(UserOption),
    Uninstall(UserOption),
}

#[derive(Debug, Args)]
struct UserOption {
    #[arg(long, value_name = "NAME")]
    user: String,
}

#[derive(Debug, Args)]
struct UiCommand {
    /// 打开某条告警详情，入口令牌仍由本机控制台创建。
    #[arg(long, value_name = "ID", conflicts_with = "alerts")]
    alert_id: Option<String>,
    /// 打开告警中心，用于历史摘要或缺少标识的旧通知。
    #[arg(long)]
    alerts: bool,
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long)]
    foreground: bool,
    #[arg(long)]
    no_browser: bool,
    /// 私有入口文件；用于独立验收，不在终端打印令牌。
    #[arg(long, value_name = "FILE")]
    session_file: Option<PathBuf>,
    #[arg(long, value_name = "DIRECTORY")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "DIRECTORY")]
    claude_home: Option<PathBuf>,
    #[arg(long, value_name = "FILE")]
    zcode_db: Option<PathBuf>,
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
    let args: Vec<_> = std::env::args_os().collect();
    // 帮助也遵循临时选择；非法参数仅显示产品说明，不回显可能含隐私的参数值。
    let requested_language = language_argument(&args);
    let _ = i18n::try_set_cli_override(requested_language);
    let locale = i18n::current_locale();
    let command = localized_cli(&locale);
    let cli = match command
        .try_get_matches_from(args)
        .and_then(|matches| Cli::from_arg_matches(&matches))
    {
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
            eprintln!("{}", i18n::message(&locale, "cli.invalid_arguments", &[]));
            return ExitCode::from(code as u8);
        }
    };

    if i18n::try_set_cli_override(cli.language.clone()).is_err() {
        eprintln!("{}", i18n::message(&locale, "cli.invalid_arguments", &[]));
        return ExitCode::FAILURE;
    }

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let product_message = error
                .downcast_ref::<io::Error>()
                .and_then(io::Error::get_ref)
                .is_some_and(|inner| inner.is::<CliMessage>());
            let detail = if product_message {
                error.to_string()
            } else {
                i18n::message(
                    &i18n::current_locale(),
                    "cli.raw_error",
                    &[("error", error.to_string())],
                )
            };
            eprintln!(
                "{}",
                i18n::message(
                    &i18n::current_locale(),
                    "cli.execution_failed",
                    &[("error", detail)]
                )
            );
            ExitCode::FAILURE
        }
    }
}

fn language_argument(args: &[std::ffi::OsString]) -> Option<String> {
    let mut values = args.iter().skip(1);
    while let Some(value) = values.next() {
        let Some(value) = value.to_str() else {
            continue;
        };
        if value == "--" {
            break;
        }
        if value == "--language" {
            return values
                .next()
                .and_then(|value| value.to_str())
                .map(str::to_owned);
        }
        if let Some(value) = value.strip_prefix("--language=") {
            return Some(value.into());
        }
    }
    None
}

fn localized_cli(locale: &str) -> clap::Command {
    let values = std::iter::once("system".to_owned()).chain(i18n::supported_locales());
    let mut command = Cli::command().mut_arg("language", |arg| {
        arg.value_parser(clap::builder::PossibleValuesParser::new(values))
    });
    // 先生成 Clap 自带帮助与版本参数，再一同设置语言，保留 help 子命令行为。
    command.build();
    localize_command(command, locale, "")
}

fn localize_command(command: clap::Command, locale: &str, path: &str) -> clap::Command {
    let key = if path.is_empty() {
        "cli.about".into()
    } else if path.rsplit('.').next() == Some("help") {
        "cli.command.help".into()
    } else {
        format!("cli.command.{path}")
    };
    let mut command = command
        .about(i18n::message(locale, &key, &[]))
        .help_template(i18n::message(locale, "cli.help.template", &[]))
        .subcommand_help_heading(i18n::message(locale, "cli.help.commands", &[]))
        .subcommand_value_name(i18n::message(locale, "cli.help.command_name", &[]))
        .mut_args(|arg| {
            let id = arg.get_id().as_str();
            let key = match id {
                "help" => "cli.help.help".into(),
                "version" => "cli.help.version".into(),
                _ => format!("cli.arg.{id}"),
            };
            let mut help = i18n::message(locale, &key, &[]);
            if !arg.get_default_values().is_empty() {
                let value = arg
                    .get_default_values()
                    .iter()
                    .map(|value| value.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(", ");
                help.push_str(&format!(
                    " [{}]",
                    i18n::message(locale, "cli.help.default", &[("value", value)])
                ));
            }
            if let Some(values) = arg.get_value_parser().possible_values() {
                let values = values
                    .filter(|value| !value.is_hide_set())
                    .map(|value| value.get_name().to_owned())
                    .collect::<Vec<_>>()
                    .join(", ");
                if !values.is_empty() {
                    help.push_str(&format!(
                        " [{}]",
                        i18n::message(locale, "cli.help.values", &[("values", values)])
                    ));
                }
            }
            let heading = if arg.get_long().is_none() && arg.get_short().is_none() {
                "cli.help.arguments"
            } else {
                "cli.help.options"
            };
            arg.help(help)
                .long_help(None::<&str>)
                .help_heading(i18n::message(locale, heading, &[]))
                .hide_default_value(true)
                .hide_possible_values(true)
        })
        .mut_subcommands(|child| {
            let name = child.get_name();
            let path = if path.is_empty() {
                name.into()
            } else {
                format!("{path}.{name}")
            };
            localize_command(child, locale, &path)
        });
    let usage = command.render_usage().to_string();
    let usage = usage.strip_prefix("Usage: ").unwrap_or(&usage).replace(
        "[OPTIONS]",
        &format!("[{}]", i18n::message(locale, "cli.help.options_name", &[])),
    );
    command.override_usage(usage)
}

#[derive(Debug)]
struct CliMessage {
    key: &'static str,
    args: Vec<(&'static str, String)>,
}

impl std::fmt::Display for CliMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&i18n::message(
            &i18n::current_locale(),
            self.key,
            &self.args,
        ))
    }
}

impl Error for CliMessage {}

fn cli_message(key: &'static str) -> CliMessage {
    CliMessage { key, args: vec![] }
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
        Command::Ui(command) => codeperimeter::web::run(codeperimeter::web::WebOptions {
            host_socket: host_socket(cli.host_socket)?,
            port: command.port,
            foreground: command.foreground,
            no_browser: command.no_browser,
            session_file: command.session_file,
            codex_home: command.codex_home,
            claude_home: command.claude_home,
            zcode_db: command.zcode_db,
            alert_id: command.alert_id,
            alerts: command.alerts,
        }),
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
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            cli_message("cli.error.directory_unavailable"),
                        )
                    })?;
                    if !canonical.is_dir() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            cli_message("cli.error.directory_required"),
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
                    cli_message("cli.error.remove_absolute"),
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
            let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    cli_message("cli.error.home_unavailable"),
                )
            })?;
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
                    cli_message("cli.error.import_selection"),
                )
                .into());
            }
            let metadata = fs::metadata(&preview)?;
            if metadata.len() > MAX_PREVIEW_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    cli_message("cli.error.preview_oversized"),
                )
                .into());
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            cli_message("cli.error.preview_version"),
        )
        .into());
    }
    for (position, indexed) in preview.candidates.iter().enumerate() {
        if indexed.index != position {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                cli_message("cli.error.preview_indices"),
            )
            .into());
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            cli_message("cli.error.preview_empty"),
        )
        .into());
    }

    let mut selected = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for index in indices {
        let indexed = preview.candidates.get(index).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                cli_message("cli.error.preview_index_invalid"),
            )
        })?;
        let candidate = &indexed.candidate;
        if candidate.status != DirectoryStatus::Available {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                cli_message("cli.error.preview_available_only"),
            )
            .into());
        }
        let path = candidate.canonical_path.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                cli_message("cli.error.preview_path_missing"),
            )
        })?;
        let current = fs::canonicalize(path).map_err(|_| {
            io::Error::new(
                io::ErrorKind::NotFound,
                cli_message("cli.error.preview_directory_unavailable"),
            )
        })?;
        if current.as_path() != path.as_path() || !current.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                cli_message("cli.error.preview_directory_changed"),
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                cli_message("cli.error.preview_source_missing"),
            )
            .into());
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
        | ServiceCommand::Pause(options)
        | ServiceCommand::Resume(options)
        | ServiceCommand::Stop(options)
        | ServiceCommand::Uninstall(options) => &options.user,
    };
    let source_binary = std::env::current_exe()?;
    let plan = ServicePlan::new(user, &source_binary)?;
    match command {
        ServiceCommand::Plan(_) => print_json(&json!({"ok": true, "data": plan})),
        ServiceCommand::Install(_) => run_operation(plan.install()?),
        ServiceCommand::Start(_) => run_operation(plan.start()?),
        ServiceCommand::Pause(_) => run_operation(plan.pause()?),
        ServiceCommand::Resume(_) => run_operation(plan.resume()?),
        ServiceCommand::Stop(_) => run_operation(plan.stop()?),
        ServiceCommand::Uninstall(_) => run_operation(plan.uninstall()?),
    }
}

fn run_operation(report: OperationReport) -> CliResult<()> {
    let outcome = ensure_operation_succeeded(&report);
    print_json(&json!({
        "ok": outcome.is_ok(),
        "data_preserved": report.data_preserved,
        "steps": report.steps
    }))?;
    outcome
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
    Err(io::Error::other(CliMessage {
        key: "cli.error.service_steps",
        args: vec![("steps", failed.join(", "))],
    })
    .into())
}

fn host_socket(override_path: Option<PathBuf>) -> CliResult<PathBuf> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            cli_message("cli.error.home_unavailable"),
        )
    })?;
    Ok(home
        .join("Library/Application Support/CodePerimeter")
        .join("host.sock"))
}

fn send_request(socket: &Path, request: ControlRequest) -> CliResult<()> {
    let response = runtime::request_control(socket, request)?;
    if !response.ok {
        return Err(io::Error::other(
            response
                .error
                .unwrap_or_else(|| cli_message("cli.error.host_rejected").to_string()),
        )
        .into());
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
    fn notification_alert_identifier_is_always_one_option_value() {
        let cli = Cli::try_parse_from(["codeperimeter", "ui", "--alert-id=--help"]).unwrap();
        let Command::Ui(command) = cli.command else {
            panic!("应解析为网页入口")
        };
        assert_eq!(command.alert_id.as_deref(), Some("--help"));
    }

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
        let error = run_operation(report).unwrap_err();
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
