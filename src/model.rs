use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum SourceStream {
    #[default]
    Combined,
    Exec,
    Activity,
}

impl SourceStream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Combined => "combined",
            Self::Exec => "exec",
            Self::Activity => "activity",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub pid_version: Option<u32>,
    pub ppid: Option<u32>,
    pub executable: Option<PathBuf>,
    pub signing_id: Option<String>,
    pub team_id: Option<String>,
}

impl ProcessIdentity {
    pub fn key(&self, source_run: &str) -> String {
        format!(
            "{}:{}:{}",
            source_run,
            self.pid,
            self.pid_version
                .map_or_else(|| "unknown".into(), |v| v.to_string())
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEvidence {
    pub path: PathBuf,
    pub path_truncated: bool,
    pub device: Option<u64>,
    pub inode: Option<u64>,
    pub is_regular: Option<bool>,
    pub readable: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveCommand {
    pub tool: String,
    pub input_paths: Vec<PathBuf>,
    pub output_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_paths: Vec<PathBuf>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityEvent {
    pub source_run_id: String,
    #[serde(default)]
    pub source_stream: SourceStream,
    #[serde(default)]
    pub source_schema_version: Option<u64>,
    #[serde(default)]
    pub source_message_version: Option<u64>,
    pub source_timestamp_ms: Option<i64>,
    pub received_timestamp_ms: i64,
    pub global_seq: Option<u64>,
    pub event_seq: Option<u64>,
    pub kind: EventKind,
    pub process: ProcessIdentity,
    pub file: Option<FileEvidence>,
    pub destination: Option<PathBuf>,
    pub modified: Option<bool>,
    pub archive: Option<ArchiveCommand>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AlertRule {
    BulkFileAccess,
    ArchiveCommand,
    ArchiveOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Alert {
    pub id: String,
    pub rule: AlertRule,
    pub process: ProcessIdentity,
    pub roots: Vec<PathBuf>,
    pub first_timestamp_ms: i64,
    pub last_timestamp_ms: i64,
    pub unique_files: usize,
    pub activity_count: u64,
    pub evidence_paths: Vec<PathBuf>,
    pub is_new: bool,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
