use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

/// 超长记录跳过并报告缺口，下一条元信息仍能继续提取。
pub const MAX_HISTORY_LINE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum HistorySource {
    Codex,
    ClaudeCode,
    Manual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryStatus {
    Available,
    Missing,
    Unresolved,
    Inaccessible,
    NotDirectory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryOrigin {
    pub source: HistorySource,
    pub history_file: Option<PathBuf>,
    pub record_type: Option<String>,
    pub line: Option<u64>,
    pub version: Option<String>,
    pub field: String,
    pub occurrences: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryCandidate {
    pub raw_paths: Vec<PathBuf>,
    pub canonical_path: Option<PathBuf>,
    pub status: DirectoryStatus,
    pub origins: Vec<DirectoryOrigin>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryGapKind {
    MissingSource,
    IoError,
    MalformedRecord,
    IncompleteRecord,
    RecordTooLong,
    UnsupportedFormat,
    MissingDirectory,
    MissingVersion,
    VersionNotValidated,
    SkippedSymlink,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryGap {
    pub source: HistorySource,
    pub path: PathBuf,
    pub first_line: Option<u64>,
    pub kind: DiscoveryGapKind,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservedVersion {
    pub source: HistorySource,
    pub version: String,
    /// 字段形状测试不等于整版历史格式兼容认证。
    pub compatibility_validated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryCounts {
    pub files_scanned: u64,
    pub records_scanned: u64,
    pub matched_metadata_records: u64,
    pub ignored_records: u64,
    pub missing_directories: u64,
    pub unresolved_directories: u64,
    pub inaccessible_directories: u64,
    pub non_directories: u64,
    pub malformed_records: u64,
    pub unsupported_records: u64,
    pub incomplete_records: u64,
    pub oversized_records: u64,
    pub io_errors: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryReport {
    pub candidates: Vec<DirectoryCandidate>,
    pub counts: DiscoveryCounts,
    pub gaps: Vec<DiscoveryGap>,
    pub versions: Vec<ObservedVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryOptions {
    /// Codex 配置根目录；扫描其 sessions 与 archived_sessions。
    pub codex_home: Option<PathBuf>,
    /// Claude Code CLI 配置根目录；扫描其 projects。
    pub claude_home: Option<PathBuf>,
}

impl HistoryOptions {
    pub fn defaults(home: &Path) -> Self {
        Self {
            codex_home: Some(home.join(".codex")),
            claude_home: Some(home.join(".claude")),
        }
    }
}

/// 仅提取明确的目录元字段，候选不会自动变为监控目录。
pub fn discover(options: &HistoryOptions) -> DiscoveryReport {
    let mut report = DiscoveryReport::default();
    if let Some(root) = &options.codex_home {
        scan_tree(&root.join("sessions"), HistorySource::Codex, &mut report);
        scan_tree(
            &root.join("archived_sessions"),
            HistorySource::Codex,
            &mut report,
        );
    }
    if let Some(root) = &options.claude_home {
        scan_tree(
            &root.join("projects"),
            HistorySource::ClaudeCode,
            &mut report,
        );
    }
    finish_report(&mut report);
    report
}

/// 供手动多个目录使用。相对路径不借用宿主当前目录补全。
pub fn manual_directories(paths: &[PathBuf]) -> DiscoveryReport {
    let mut report = DiscoveryReport::default();
    for path in paths {
        add_candidate(
            &mut report,
            path.clone(),
            DirectoryOrigin {
                source: HistorySource::Manual,
                history_file: None,
                record_type: None,
                line: None,
                version: None,
                field: "manual".into(),
                occurrences: 1,
            },
        );
    }
    finish_report(&mut report);
    report
}

fn scan_tree(root: &Path, source: HistorySource, report: &mut DiscoveryReport) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                let kind = if error.kind() == io::ErrorKind::NotFound {
                    DiscoveryGapKind::MissingSource
                } else {
                    report.counts.io_errors += 1;
                    DiscoveryGapKind::IoError
                };
                add_gap(report, source, &directory, None, kind);
                continue;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    report.counts.io_errors += 1;
                    add_gap(report, source, &directory, None, DiscoveryGapKind::IoError);
                    continue;
                }
            };
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_symlink() => {
                    add_gap(
                        report,
                        source,
                        &path,
                        None,
                        DiscoveryGapKind::SkippedSymlink,
                    );
                }
                Ok(kind) if kind.is_dir() => children.push(path),
                Ok(kind) if kind.is_file() && is_history_file(&path, source) => {
                    children.push(path);
                }
                Ok(_) => {}
                Err(_) => {
                    report.counts.io_errors += 1;
                    add_gap(report, source, &path, None, DiscoveryGapKind::IoError);
                }
            }
        }
        children.sort();
        for child in children.into_iter().rev() {
            if is_history_file(&child, source) && child.is_file() {
                scan_file(&child, source, report);
            } else {
                pending.push(child);
            }
        }
    }
}

fn is_history_file(path: &Path, source: HistorySource) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.ends_with(".jsonl")
                || (source == HistorySource::Codex && name.ends_with(".jsonl.zst"))
        })
}

fn scan_file(path: &Path, source: HistorySource, report: &mut DiscoveryReport) {
    let reader: io::Result<Box<dyn Read>> = File::open(path).and_then(|file| {
        if path.extension().is_some_and(|extension| extension == "zst") {
            let mut decoder = zstd::stream::read::Decoder::new(file)?;
            // 限制压缩帧声明的窗口，避免小文件诱导无界解码内存。
            decoder.window_log_max(24)?;
            Ok(Box::new(decoder) as Box<dyn Read>)
        } else {
            Ok(Box::new(file) as Box<dyn Read>)
        }
    });
    let reader = match reader {
        Ok(reader) => reader,
        Err(_) => {
            report.counts.io_errors += 1;
            add_gap(report, source, path, None, DiscoveryGapKind::IoError);
            return;
        }
    };
    report.counts.files_scanned += 1;
    let mut reader = BufReader::new(reader);
    let mut line_number = 0;
    let mut codex_version = None;
    loop {
        let line = match bounded_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(_) => {
                report.counts.io_errors += 1;
                add_gap(
                    report,
                    source,
                    path,
                    Some(line_number + 1),
                    DiscoveryGapKind::IoError,
                );
                break;
            }
        };
        line_number += 1;
        if line.oversized {
            report.counts.records_scanned += 1;
            report.counts.oversized_records += 1;
            add_gap(
                report,
                source,
                path,
                Some(line_number),
                DiscoveryGapKind::RecordTooLong,
            );
            continue;
        }
        if line.bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        report.counts.records_scanned += 1;
        let result = match source {
            HistorySource::Codex => {
                parse_codex(&line.bytes, path, line_number, &mut codex_version, report)
            }
            HistorySource::ClaudeCode => parse_claude(&line.bytes, path, line_number, report),
            HistorySource::Manual => unreachable!(),
        };
        if let Err(error) = result {
            let kind = if !line.terminated && error.is_eof() {
                report.counts.incomplete_records += 1;
                DiscoveryGapKind::IncompleteRecord
            } else {
                report.counts.malformed_records += 1;
                DiscoveryGapKind::MalformedRecord
            };
            // 不保存 JSON 错误的原始值，避免错误描述带入会话内容。
            add_gap(report, source, path, Some(line_number), kind);
        }
    }
}

struct HistoryLine {
    bytes: Vec<u8>,
    terminated: bool,
    oversized: bool,
}

fn bounded_line(reader: &mut impl BufRead) -> io::Result<Option<HistoryLine>> {
    let mut line = HistoryLine {
        bytes: Vec::new(),
        terminated: false,
        oversized: false,
    };
    let mut consumed = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(consumed.then_some(line));
        }
        consumed = true;
        let length = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1);
        let take = length.unwrap_or(buffer.len());
        if !line.oversized {
            if line.bytes.len() + take <= MAX_HISTORY_LINE_BYTES {
                line.bytes.extend_from_slice(&buffer[..take]);
            } else {
                line.oversized = true;
                line.bytes.clear();
            }
        }
        reader.consume(take);
        if length.is_some() {
            line.terminated = true;
            return Ok(Some(line));
        }
    }
}

#[derive(Deserialize)]
struct RecordHeader {
    #[serde(rename = "type")]
    record_type: Option<String>,
}

#[derive(Deserialize)]
struct CodexEnvelope {
    payload: Option<CodexMetadata>,
}

#[derive(Deserialize)]
struct CodexMetadata {
    cwd: Option<PathBuf>,
    runtime_workspace_roots: Option<Vec<PathBuf>>,
    workspace_roots: Option<Vec<PathBuf>>,
    cli_version: Option<String>,
}

fn parse_codex(
    bytes: &[u8],
    path: &Path,
    line: u64,
    version: &mut Option<String>,
    report: &mut DiscoveryReport,
) -> serde_json::Result<()> {
    // serde 忽略非元信息字段，不构造消息、工具参数或正文的数据模型。
    let record: RecordHeader = serde_json::from_slice(bytes)?;
    let Some(record_type) = record.record_type else {
        unsupported(report, HistorySource::Codex, path, line);
        return Ok(());
    };
    if record_type != "session_meta" && record_type != "turn_context" {
        if matches!(
            record_type.as_str(),
            "response_item" | "event_msg" | "compacted" | "world_state"
        ) {
            report.counts.ignored_records += 1;
        } else {
            unsupported(report, HistorySource::Codex, path, line);
        }
        return Ok(());
    }
    let record: CodexEnvelope = serde_json::from_slice(bytes)?;
    let Some(metadata) = record.payload else {
        unsupported(report, HistorySource::Codex, path, line);
        return Ok(());
    };
    report.counts.matched_metadata_records += 1;
    if record_type == "session_meta" {
        *version = metadata.cli_version;
    }
    observe_version(report, HistorySource::Codex, path, line, version.as_deref());
    let mut fields = Vec::new();
    if let Some(cwd) = metadata.cwd {
        fields.push(("cwd", cwd));
    }
    let roots = if record_type == "session_meta" {
        metadata
            .runtime_workspace_roots
            .map(|roots| ("runtime_workspace_roots", roots))
    } else {
        metadata
            .workspace_roots
            .map(|roots| ("workspace_roots", roots))
    };
    if let Some((field, roots)) = roots {
        fields.extend(roots.into_iter().map(|root| (field, root)));
    }
    add_fields(
        report,
        HistorySource::Codex,
        path,
        line,
        &record_type,
        version.clone(),
        fields,
    );
    Ok(())
}

#[derive(Deserialize)]
struct ClaudeMetadata {
    #[serde(rename = "type")]
    record_type: Option<String>,
    cwd: Option<PathBuf>,
    version: Option<String>,
}

fn parse_claude(
    bytes: &[u8],
    path: &Path,
    line: u64,
    report: &mut DiscoveryReport,
) -> serde_json::Result<()> {
    let record: ClaudeMetadata = serde_json::from_slice(bytes)?;
    let Some(record_type) = record.record_type else {
        unsupported(report, HistorySource::ClaudeCode, path, line);
        return Ok(());
    };
    let Some(cwd) = record.cwd else {
        report.counts.ignored_records += 1;
        add_gap(
            report,
            HistorySource::ClaudeCode,
            path,
            Some(line),
            DiscoveryGapKind::MissingDirectory,
        );
        return Ok(());
    };
    report.counts.matched_metadata_records += 1;
    observe_version(
        report,
        HistorySource::ClaudeCode,
        path,
        line,
        record.version.as_deref(),
    );
    add_fields(
        report,
        HistorySource::ClaudeCode,
        path,
        line,
        &record_type,
        record.version,
        vec![("cwd", cwd)],
    );
    Ok(())
}

fn add_fields(
    report: &mut DiscoveryReport,
    source: HistorySource,
    path: &Path,
    line: u64,
    record_type: &str,
    version: Option<String>,
    fields: Vec<(&str, PathBuf)>,
) {
    if fields.is_empty() {
        add_gap(
            report,
            source,
            path,
            Some(line),
            DiscoveryGapKind::MissingDirectory,
        );
    }
    for (field, directory) in fields {
        add_candidate(
            report,
            directory,
            DirectoryOrigin {
                source,
                history_file: Some(path.to_path_buf()),
                record_type: Some(record_type.to_owned()),
                line: Some(line),
                version: version.clone(),
                field: field.into(),
                occurrences: 1,
            },
        );
    }
}

fn observe_version(
    report: &mut DiscoveryReport,
    source: HistorySource,
    path: &Path,
    line: u64,
    version: Option<&str>,
) {
    match version.filter(|version| !version.is_empty()) {
        Some(version) => {
            if !report
                .versions
                .iter()
                .any(|seen| seen.source == source && seen.version == version)
            {
                report.versions.push(ObservedVersion {
                    source,
                    version: version.into(),
                    compatibility_validated: false,
                });
            }
            add_gap(
                report,
                source,
                path,
                Some(line),
                DiscoveryGapKind::VersionNotValidated,
            );
        }
        None => add_gap(
            report,
            source,
            path,
            Some(line),
            DiscoveryGapKind::MissingVersion,
        ),
    }
}

fn unsupported(report: &mut DiscoveryReport, source: HistorySource, path: &Path, line: u64) {
    report.counts.unsupported_records += 1;
    add_gap(
        report,
        source,
        path,
        Some(line),
        DiscoveryGapKind::UnsupportedFormat,
    );
}

fn add_gap(
    report: &mut DiscoveryReport,
    source: HistorySource,
    path: &Path,
    line: Option<u64>,
    kind: DiscoveryGapKind,
) {
    if let Some(gap) = report
        .gaps
        .iter_mut()
        .find(|gap| gap.source == source && gap.path == path && gap.kind == kind)
    {
        gap.count += 1;
    } else {
        report.gaps.push(DiscoveryGap {
            source,
            path: path.into(),
            first_line: line,
            kind,
            count: 1,
        });
    }
}

fn add_candidate(report: &mut DiscoveryReport, raw_path: PathBuf, origin: DirectoryOrigin) {
    let (status, canonical_path) = normalize_directory(&raw_path);
    let key = canonical_path.as_ref().unwrap_or(&raw_path);
    if let Some(candidate) = report.candidates.iter_mut().find(|candidate| {
        candidate.status == status
            && candidate
                .canonical_path
                .as_ref()
                .unwrap_or(&candidate.raw_paths[0])
                == key
    }) {
        if !candidate.raw_paths.contains(&raw_path) {
            candidate.raw_paths.push(raw_path);
        }
        if let Some(existing) = candidate.origins.iter_mut().find(|existing| {
            existing.source == origin.source
                && existing.history_file == origin.history_file
                && existing.record_type == origin.record_type
                && existing.version == origin.version
                && existing.field == origin.field
        }) {
            existing.occurrences += 1;
        } else {
            candidate.origins.push(origin);
        }
    } else {
        report.candidates.push(DirectoryCandidate {
            raw_paths: vec![raw_path],
            canonical_path,
            status,
            origins: vec![origin],
        });
    }
}

fn normalize_directory(path: &Path) -> (DirectoryStatus, Option<PathBuf>) {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return (DirectoryStatus::Unresolved, None);
    }
    match fs::canonicalize(path) {
        Ok(canonical) => match fs::metadata(&canonical) {
            Ok(metadata) if metadata.is_dir() => (DirectoryStatus::Available, Some(canonical)),
            Ok(_) => (DirectoryStatus::NotDirectory, None),
            Err(_) => (DirectoryStatus::Inaccessible, None),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => (DirectoryStatus::Missing, None),
        Err(_) => (DirectoryStatus::Inaccessible, None),
    }
}

fn finish_report(report: &mut DiscoveryReport) {
    report.candidates.sort_by(|left, right| {
        left.canonical_path
            .as_ref()
            .unwrap_or(&left.raw_paths[0])
            .cmp(right.canonical_path.as_ref().unwrap_or(&right.raw_paths[0]))
    });
    report.counts.missing_directories = 0;
    report.counts.unresolved_directories = 0;
    report.counts.inaccessible_directories = 0;
    report.counts.non_directories = 0;
    for candidate in &report.candidates {
        match candidate.status {
            DirectoryStatus::Available => {}
            DirectoryStatus::Missing => report.counts.missing_directories += 1,
            DirectoryStatus::Unresolved => report.counts.unresolved_directories += 1,
            DirectoryStatus::Inaccessible => report.counts.inaccessible_directories += 1,
            DirectoryStatus::NotDirectory => report.counts.non_directories += 1,
        }
    }
    report
        .versions
        .sort_by(|left, right| (left.source, &left.version).cmp(&(right.source, &right.version)));
}
