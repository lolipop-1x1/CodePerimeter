use crate::Result;
use crate::model::{ActivityEvent, Alert, AlertRule, EventKind};
use crate::rules::normalize_path;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SCHEMA_VERSION: u32 = 1;
const DEFAULT_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
const SQLITE_BUSY_TIMEOUT_MS: u64 = 100;
const MAX_QUERY_LIMIT: usize = 10_000;
const MAX_DETAIL_CHARS: usize = 1_024;

const SCHEMA: &str = "
CREATE TABLE monitored_directories (
    path TEXT PRIMARY KEY NOT NULL,
    added_at_ms INTEGER NOT NULL
);
CREATE TABLE directory_sources (
    directory_path TEXT NOT NULL,
    source TEXT NOT NULL,
    added_at_ms INTEGER NOT NULL,
    PRIMARY KEY (directory_path, source),
    FOREIGN KEY (directory_path) REFERENCES monitored_directories(path) ON DELETE CASCADE
);
CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    source_run_id TEXT NOT NULL,
    source_timestamp_ms INTEGER,
    received_timestamp_ms INTEGER NOT NULL,
    global_seq INTEGER,
    event_seq INTEGER,
    sequence_key TEXT,
    kind TEXT NOT NULL,
    pid INTEGER NOT NULL,
    pid_version INTEGER,
    file_path TEXT,
    destination_path TEXT,
    event_json TEXT NOT NULL
);
CREATE UNIQUE INDEX events_source_sequence
    ON events (source_run_id, sequence_key)
    WHERE sequence_key IS NOT NULL;
CREATE INDEX events_received_time ON events (received_timestamp_ms DESC);
CREATE INDEX events_process_time ON events (pid, pid_version, received_timestamp_ms DESC);
CREATE INDEX events_file_time ON events (file_path, received_timestamp_ms DESC);
CREATE INDEX events_destination_time ON events (destination_path, received_timestamp_ms DESC);
CREATE TABLE event_directories (
    event_id INTEGER NOT NULL,
    directory_path TEXT NOT NULL,
    PRIMARY KEY (event_id, directory_path),
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX event_directories_path ON event_directories (directory_path, event_id);
CREATE TABLE alerts (
    id TEXT PRIMARY KEY NOT NULL,
    rule TEXT NOT NULL,
    pid INTEGER NOT NULL,
    pid_version INTEGER,
    first_timestamp_ms INTEGER NOT NULL,
    last_timestamp_ms INTEGER NOT NULL,
    alert_json TEXT NOT NULL
);
CREATE INDEX alerts_last_time ON alerts (last_timestamp_ms DESC);
CREATE INDEX alerts_process_time ON alerts (pid, pid_version, last_timestamp_ms DESC);
CREATE TABLE notification_outbox (
    alert_id TEXT PRIMARY KEY NOT NULL,
    created_timestamp_ms INTEGER NOT NULL,
    acknowledged_timestamp_ms INTEGER,
    FOREIGN KEY (alert_id) REFERENCES alerts(id) ON DELETE CASCADE
);
CREATE INDEX notification_outbox_pending
    ON notification_outbox (acknowledged_timestamp_ms, created_timestamp_ms);
CREATE TABLE alert_directories (
    alert_id TEXT NOT NULL,
    directory_path TEXT NOT NULL,
    PRIMARY KEY (alert_id, directory_path),
    FOREIGN KEY (alert_id) REFERENCES alerts(id) ON DELETE CASCADE
);
CREATE INDEX alert_directories_path ON alert_directories (directory_path, alert_id);
CREATE TABLE health_records (
    id INTEGER PRIMARY KEY,
    observed_timestamp_ms INTEGER NOT NULL,
    component TEXT NOT NULL,
    code TEXT NOT NULL,
    state TEXT NOT NULL,
    detail TEXT
);
CREATE INDEX health_time ON health_records (observed_timestamp_ms DESC);
CREATE TABLE notification_feedback (
    id INTEGER PRIMARY KEY,
    alert_id TEXT NOT NULL,
    observed_timestamp_ms INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    detail TEXT,
    FOREIGN KEY (alert_id) REFERENCES alerts(id) ON DELETE CASCADE
);
CREATE INDEX notification_time ON notification_feedback (observed_timestamp_ms DESC);
CREATE INDEX notification_alert ON notification_feedback (alert_id, observed_timestamp_ms DESC);
CREATE TABLE cumulative_statistics (
    key TEXT PRIMARY KEY NOT NULL,
    value INTEGER NOT NULL
);
";

#[derive(Debug)]
pub struct Storage {
    connection: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryConfig {
    pub path: PathBuf,
    pub sources: Vec<String>,
    pub added_at_ms: i64,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredEvent {
    pub id: i64,
    pub event: ActivityEvent,
    pub directories: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct EventFilter {
    pub directory: Option<PathBuf>,
    pub pid: Option<u32>,
    pub kind: Option<EventKind>,
    pub file_path: Option<PathBuf>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub limit: usize,
}

impl Default for EventFilter {
    fn default() -> Self {
        Self {
            directory: None,
            pid: None,
            kind: None,
            file_path: None,
            since_ms: None,
            until_ms: None,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AlertFilter {
    pub directory: Option<PathBuf>,
    pub pid: Option<u32>,
    pub rule: Option<AlertRule>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub limit: usize,
}

impl Default for AlertFilter {
    fn default() -> Self {
        Self {
            directory: None,
            pid: None,
            rule: None,
            since_ms: None,
            until_ms: None,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HealthFilter {
    pub component: Option<String>,
    pub code: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub limit: usize,
}

impl Default for HealthFilter {
    fn default() -> Self {
        Self {
            component: None,
            code: None,
            since_ms: None,
            until_ms: None,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthRecord {
    pub observed_timestamp_ms: i64,
    pub component: String,
    pub code: String,
    pub state: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredHealthRecord {
    pub id: i64,
    pub record: HealthRecord,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationOutcome {
    Sent,
    Failed,
    Deferred,
    Acknowledged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationRecord {
    pub alert_id: String,
    pub observed_timestamp_ms: i64,
    pub outcome: NotificationOutcome,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredNotificationRecord {
    pub id: i64,
    pub record: NotificationRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NotificationFilter {
    pub alert_id: Option<String>,
    pub outcome: Option<NotificationOutcome>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub limit: usize,
}

impl Default for NotificationFilter {
    fn default() -> Self {
        Self {
            alert_id: None,
            outcome: None,
            since_ms: None,
            until_ms: None,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlertWrite {
    #[default]
    Inserted,
    Updated,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CumulativeStats {
    pub events: u64,
    pub alerts: u64,
    pub alert_updates: u64,
    pub health_records: u64,
    pub notifications_sent: u64,
    pub notifications_failed: u64,
    pub notifications_deferred: u64,
    pub notifications_acknowledged: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RecentStats {
    pub events: u64,
    pub distinct_file_paths: u64,
    pub alerts: u64,
    pub health_records: u64,
    pub notifications_sent: u64,
    pub notifications_failed: u64,
    pub notifications_deferred: u64,
    pub notifications_acknowledged: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PruneSummary {
    pub events: usize,
    pub alerts: usize,
    pub health_records: usize,
    pub notifications: usize,
    pub outbox_entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingNotification {
    pub alert: Alert,
    pub created_timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationRuleCount {
    pub rule: AlertRule,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PendingNotificationSummary {
    pub count: u64,
    pub oldest_created_timestamp_ms: Option<i64>,
    pub by_rule: Vec<NotificationRuleCount>,
}

impl Storage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path != Path::new(":memory:")
            && let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;
        Self::initialize_schema(&mut connection)?;
        set_database_permissions(path)?;
        Ok(Self { connection })
    }

    pub fn schema_version(&self) -> Result<u32> {
        let version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        u32::try_from(version).map_err(|error| Box::new(error) as _)
    }

    pub fn add_directory(&mut self, path: &Path, source: &str, added_at_ms: i64) -> Result<bool> {
        let normalized = normalize_path(path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "监控目录必须是绝对路径。")
        })?;
        let source = source.trim();
        if source.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "目录来源不能为空。",
            )));
        }
        let path_text = path_to_string(&normalized)?;
        let transaction = self.connection.transaction()?;
        let inserted = transaction.execute(
            "INSERT INTO monitored_directories (path, added_at_ms)
             VALUES (?1, ?2)
             ON CONFLICT(path) DO NOTHING",
            params![path_text, added_at_ms],
        )?;
        transaction.execute(
            "INSERT INTO directory_sources (directory_path, source, added_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(directory_path, source) DO NOTHING",
            params![path_text, source, added_at_ms],
        )?;
        transaction.commit()?;
        Ok(inserted > 0)
    }

    pub fn remove_directory(&mut self, path: &Path) -> Result<bool> {
        let normalized = normalize_path(path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "监控目录必须是绝对路径。")
        })?;
        let path_text = path_to_string(&normalized)?;
        let transaction = self.connection.transaction()?;
        let removed = transaction.execute(
            "DELETE FROM monitored_directories WHERE path = ?1",
            params![path_text],
        )?;
        transaction.commit()?;
        Ok(removed > 0)
    }

    pub fn list_directories(&self) -> Result<Vec<DirectoryConfig>> {
        let paths = {
            let mut statement = self
                .connection
                .prepare("SELECT path, added_at_ms FROM monitored_directories ORDER BY path")?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut directories = Vec::with_capacity(paths.len());
        for (path, added_at_ms) in paths {
            let source_names = {
                let mut statement = self.connection.prepare(
                    "SELECT source FROM directory_sources
                     WHERE directory_path = ?1 ORDER BY source",
                )?;
                statement
                    .query_map(params![path], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            let path = PathBuf::from(path);
            directories.push(DirectoryConfig {
                exists: path.is_dir(),
                path,
                sources: source_names,
                added_at_ms,
            });
        }
        Ok(directories)
    }

    pub fn active_directory_paths(&self) -> Result<Vec<PathBuf>> {
        Ok(self
            .list_directories()?
            .into_iter()
            .filter(|directory| directory.exists)
            .map(|directory| directory.path)
            .collect())
    }

    pub fn record_event(
        &mut self,
        event: &ActivityEvent,
        matched_directories: &[PathBuf],
    ) -> Result<bool> {
        let event_json = serde_json::to_string(event)?;
        let sequence_key = sequence_key(event);
        let source_timestamp_ms = event.source_timestamp_ms;
        let global_seq = event.global_seq.map(u64_to_sql_integer).transpose()?;
        let event_seq = event.event_seq.map(u64_to_sql_integer).transpose()?;
        let file_path = event
            .file
            .as_ref()
            .map(|file| stored_path_to_string(&file.path))
            .transpose()?;
        let destination_path = event
            .destination
            .as_deref()
            .map(stored_path_to_string)
            .transpose()?;
        let kind = event_kind_name(event.kind);
        let transaction = self.connection.transaction()?;
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO events (
                source_run_id, source_timestamp_ms, received_timestamp_ms, global_seq,
                event_seq, sequence_key, kind, pid, pid_version, file_path,
                destination_path, event_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                event.source_run_id,
                source_timestamp_ms,
                event.received_timestamp_ms,
                global_seq,
                event_seq,
                sequence_key,
                kind,
                i64::from(event.process.pid),
                event.process.pid_version.map(i64::from),
                file_path,
                destination_path,
                event_json,
            ],
        )?;
        if inserted > 0 {
            let event_id = transaction.last_insert_rowid();
            for directory in normalize_directory_list(matched_directories)? {
                transaction.execute(
                    "INSERT OR IGNORE INTO event_directories (event_id, directory_path)
                     VALUES (?1, ?2)",
                    params![event_id, path_to_string(&directory)?],
                )?;
            }
            increment_stat(&transaction, "events", 1)?;
            increment_stat(&transaction, &format!("events.{kind}"), 1)?;
        }
        transaction.commit()?;
        Ok(inserted > 0)
    }

    pub fn record_alert(&mut self, alert: &Alert) -> Result<AlertWrite> {
        self.record_alert_at(alert, alert.last_timestamp_ms)
    }

    pub fn record_alert_at(
        &mut self,
        alert: &Alert,
        created_timestamp_ms: i64,
    ) -> Result<AlertWrite> {
        let mut persisted = alert.clone();
        persisted.is_new = false;
        let alert_json = serde_json::to_string(&persisted)?;
        let rule = alert_rule_name(alert.rule);
        let transaction = self.connection.transaction()?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM alerts WHERE id = ?1)",
            params![alert.id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO alerts (
                id, rule, pid, pid_version, first_timestamp_ms,
                last_timestamp_ms, alert_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                rule = excluded.rule,
                pid = excluded.pid,
                pid_version = excluded.pid_version,
                first_timestamp_ms = MIN(alerts.first_timestamp_ms, excluded.first_timestamp_ms),
                last_timestamp_ms = MAX(alerts.last_timestamp_ms, excluded.last_timestamp_ms),
                alert_json = excluded.alert_json",
            params![
                alert.id,
                rule,
                i64::from(alert.process.pid),
                alert.process.pid_version.map(i64::from),
                alert.first_timestamp_ms,
                alert.last_timestamp_ms,
                alert_json,
            ],
        )?;
        transaction.execute(
            "DELETE FROM alert_directories WHERE alert_id = ?1",
            params![alert.id],
        )?;
        for directory in normalize_directory_list(&alert.roots)? {
            transaction.execute(
                "INSERT OR IGNORE INTO alert_directories (alert_id, directory_path)
                 VALUES (?1, ?2)",
                params![alert.id, path_to_string(&directory)?],
            )?;
        }
        if exists {
            increment_stat(&transaction, "alert_updates", 1)?;
        } else {
            increment_stat(&transaction, "alerts", 1)?;
            increment_stat(&transaction, &format!("alerts.{rule}"), 1)?;
            if alert.is_new {
                transaction.execute(
                    "INSERT INTO notification_outbox (alert_id, created_timestamp_ms)
                     VALUES (?1, ?2)",
                    params![alert.id, created_timestamp_ms],
                )?;
            }
        }
        transaction.commit()?;
        Ok(if exists {
            AlertWrite::Updated
        } else {
            AlertWrite::Inserted
        })
    }

    pub fn record_health(&mut self, record: &HealthRecord) -> Result<i64> {
        let detail = record.detail.as_deref().map(bounded_detail);
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO health_records (
                observed_timestamp_ms, component, code, state, detail
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.observed_timestamp_ms,
                record.component,
                record.code,
                record.state,
                detail,
            ],
        )?;
        let id = transaction.last_insert_rowid();
        increment_stat(&transaction, "health_records", 1)?;
        transaction.commit()?;
        Ok(id)
    }

    pub fn record_notification(&mut self, record: &NotificationRecord) -> Result<i64> {
        let outcome = notification_outcome_name(record.outcome);
        let detail = record.detail.as_deref().map(bounded_detail);
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO notification_feedback (
                alert_id, observed_timestamp_ms, outcome, detail
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.alert_id,
                record.observed_timestamp_ms,
                outcome,
                detail,
            ],
        )?;
        let id = transaction.last_insert_rowid();
        increment_stat(&transaction, &format!("notifications.{outcome}"), 1)?;
        if matches!(
            record.outcome,
            NotificationOutcome::Sent | NotificationOutcome::Acknowledged
        ) {
            transaction.execute(
                "UPDATE notification_outbox
                 SET acknowledged_timestamp_ms = COALESCE(acknowledged_timestamp_ms, ?2)
                 WHERE alert_id = ?1",
                params![record.alert_id, record.observed_timestamp_ms],
            )?;
        }
        transaction.commit()?;
        Ok(id)
    }

    pub fn pending_notifications(&self, limit: usize) -> Result<Vec<PendingNotification>> {
        let rows = {
            let mut statement = self.connection.prepare(
                "SELECT a.alert_json, o.created_timestamp_ms
                 FROM notification_outbox o
                 JOIN alerts a ON a.id = o.alert_id
                 WHERE o.acknowledged_timestamp_ms IS NULL
                 ORDER BY o.created_timestamp_ms, o.alert_id
                 LIMIT ?1",
            )?;
            statement
                .query_map(params![query_limit(limit)], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|(json, created_timestamp_ms)| {
                let mut alert: Alert = serde_json::from_str(&json)?;
                alert.is_new = false;
                Ok(PendingNotification {
                    alert,
                    created_timestamp_ms,
                })
            })
            .collect()
    }

    pub fn pending_notification_summary(&self) -> Result<PendingNotificationSummary> {
        let (count, oldest): (i64, Option<i64>) = self.connection.query_row(
            "SELECT COUNT(*), MIN(created_timestamp_ms)
             FROM notification_outbox
             WHERE acknowledged_timestamp_ms IS NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let grouped = {
            let mut statement = self.connection.prepare(
                "SELECT a.rule, COUNT(*)
                 FROM notification_outbox o
                 JOIN alerts a ON a.id = o.alert_id
                 WHERE o.acknowledged_timestamp_ms IS NULL
                 GROUP BY a.rule
                 ORDER BY a.rule",
            )?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut by_rule = Vec::with_capacity(grouped.len());
        for (rule, count) in grouped {
            by_rule.push(NotificationRuleCount {
                rule: parse_alert_rule(&rule)?,
                count: nonnegative_u64(count),
            });
        }
        Ok(PendingNotificationSummary {
            count: nonnegative_u64(count),
            oldest_created_timestamp_ms: oldest,
            by_rule,
        })
    }

    pub fn acknowledge_notifications(
        &mut self,
        alert_ids: &[String],
        observed_timestamp_ms: i64,
    ) -> Result<usize> {
        let transaction = self.connection.transaction()?;
        let mut acknowledged = 0;
        for alert_id in alert_ids.iter().collect::<std::collections::BTreeSet<_>>() {
            let changed = transaction.execute(
                "UPDATE notification_outbox
                 SET acknowledged_timestamp_ms = ?2
                 WHERE alert_id = ?1 AND acknowledged_timestamp_ms IS NULL",
                params![alert_id, observed_timestamp_ms],
            )?;
            if changed > 0 {
                transaction.execute(
                    "INSERT INTO notification_feedback (
                        alert_id, observed_timestamp_ms, outcome, detail
                     ) VALUES (?1, ?2, 'acknowledged', NULL)",
                    params![alert_id, observed_timestamp_ms],
                )?;
                increment_stat(&transaction, "notifications.acknowledged", 1)?;
                acknowledged += changed;
            }
        }
        transaction.commit()?;
        Ok(acknowledged)
    }

    pub fn query_events(&self, filter: &EventFilter) -> Result<Vec<StoredEvent>> {
        let directory = normalize_optional_query_path(filter.directory.as_deref())?;
        let directory = directory.as_deref().map(path_to_string).transpose()?;
        let file_path = normalize_optional_query_path(filter.file_path.as_deref())?;
        let file_path = file_path.as_deref().map(path_to_string).transpose()?;
        let pid = filter.pid.map(i64::from);
        let kind = filter.kind.map(event_kind_name);
        let limit = query_limit(filter.limit);
        let rows = {
            let mut statement = self.connection.prepare(
                "SELECT e.id, e.event_json FROM events e
                 WHERE (?1 IS NULL OR e.received_timestamp_ms >= ?1)
                   AND (?2 IS NULL OR e.received_timestamp_ms < ?2)
                   AND (?3 IS NULL OR e.pid = ?3)
                   AND (?4 IS NULL OR e.kind = ?4)
                   AND (?5 IS NULL OR EXISTS (
                       SELECT 1 FROM event_directories d
                       WHERE d.event_id = e.id AND d.directory_path = ?5
                   ))
                   AND (?6 IS NULL OR e.file_path = ?6 OR e.destination_path = ?6)
                 ORDER BY e.received_timestamp_ms DESC, e.id DESC
                 LIMIT ?7",
            )?;
            statement
                .query_map(
                    params![
                        filter.since_ms,
                        filter.until_ms,
                        pid,
                        kind,
                        directory,
                        file_path,
                        limit,
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut events = Vec::with_capacity(rows.len());
        for (id, json) in rows {
            let event: ActivityEvent = serde_json::from_str(&json)?;
            events.push(StoredEvent {
                id,
                event,
                directories: self.event_directories(id)?,
            });
        }
        Ok(events)
    }

    pub fn query_alerts(&self, filter: &AlertFilter) -> Result<Vec<Alert>> {
        let directory = normalize_optional_query_path(filter.directory.as_deref())?;
        let directory = directory.as_deref().map(path_to_string).transpose()?;
        let pid = filter.pid.map(i64::from);
        let rule = filter.rule.map(alert_rule_name);
        let rows = {
            let mut statement = self.connection.prepare(
                "SELECT a.alert_json FROM alerts a
                 WHERE (?1 IS NULL OR a.last_timestamp_ms >= ?1)
                   AND (?2 IS NULL OR a.last_timestamp_ms < ?2)
                   AND (?3 IS NULL OR a.pid = ?3)
                   AND (?4 IS NULL OR a.rule = ?4)
                   AND (?5 IS NULL OR EXISTS (
                       SELECT 1 FROM alert_directories d
                       WHERE d.alert_id = a.id AND d.directory_path = ?5
                   ))
                 ORDER BY a.last_timestamp_ms DESC, a.id DESC
                 LIMIT ?6",
            )?;
            statement
                .query_map(
                    params![
                        filter.since_ms,
                        filter.until_ms,
                        pid,
                        rule,
                        directory,
                        query_limit(filter.limit),
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|json| {
                let mut alert: Alert = serde_json::from_str(&json)?;
                alert.is_new = false;
                Ok(alert)
            })
            .collect()
    }

    pub fn query_health(&self, filter: &HealthFilter) -> Result<Vec<StoredHealthRecord>> {
        let rows = {
            let mut statement = self.connection.prepare(
                "SELECT id, observed_timestamp_ms, component, code, state, detail
                 FROM health_records
                 WHERE (?1 IS NULL OR component = ?1)
                   AND (?2 IS NULL OR code = ?2)
                   AND (?3 IS NULL OR observed_timestamp_ms >= ?3)
                   AND (?4 IS NULL OR observed_timestamp_ms < ?4)
                 ORDER BY observed_timestamp_ms DESC, id DESC
                 LIMIT ?5",
            )?;
            statement
                .query_map(
                    params![
                        filter.component,
                        filter.code,
                        filter.since_ms,
                        filter.until_ms,
                        query_limit(filter.limit),
                    ],
                    |row| {
                        Ok(StoredHealthRecord {
                            id: row.get(0)?,
                            record: HealthRecord {
                                observed_timestamp_ms: row.get(1)?,
                                component: row.get(2)?,
                                code: row.get(3)?,
                                state: row.get(4)?,
                                detail: row.get(5)?,
                            },
                        })
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub fn query_notifications(
        &self,
        filter: &NotificationFilter,
    ) -> Result<Vec<StoredNotificationRecord>> {
        let outcome = filter.outcome.map(notification_outcome_name);
        let rows = {
            let mut statement = self.connection.prepare(
                "SELECT id, alert_id, observed_timestamp_ms, outcome, detail
                 FROM notification_feedback
                 WHERE (?1 IS NULL OR alert_id = ?1)
                   AND (?2 IS NULL OR outcome = ?2)
                   AND (?3 IS NULL OR observed_timestamp_ms >= ?3)
                   AND (?4 IS NULL OR observed_timestamp_ms < ?4)
                 ORDER BY observed_timestamp_ms DESC, id DESC
                 LIMIT ?5",
            )?;
            statement
                .query_map(
                    params![
                        filter.alert_id,
                        outcome,
                        filter.since_ms,
                        filter.until_ms,
                        query_limit(filter.limit),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|(id, alert_id, timestamp, outcome, detail)| {
                Ok(StoredNotificationRecord {
                    id,
                    record: NotificationRecord {
                        alert_id,
                        observed_timestamp_ms: timestamp,
                        outcome: parse_notification_outcome(&outcome)?,
                        detail,
                    },
                })
            })
            .collect()
    }

    pub fn cumulative_stats(&self) -> Result<CumulativeStats> {
        Ok(CumulativeStats {
            events: read_stat(&self.connection, "events")?,
            alerts: read_stat(&self.connection, "alerts")?,
            alert_updates: read_stat(&self.connection, "alert_updates")?,
            health_records: read_stat(&self.connection, "health_records")?,
            notifications_sent: read_stat(&self.connection, "notifications.sent")?,
            notifications_failed: read_stat(&self.connection, "notifications.failed")?,
            notifications_deferred: read_stat(&self.connection, "notifications.deferred")?,
            notifications_acknowledged: read_stat(&self.connection, "notifications.acknowledged")?,
        })
    }

    pub fn recent_stats(&self, since_ms: i64, until_ms: i64) -> Result<RecentStats> {
        let events: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM events
             WHERE received_timestamp_ms >= ?1 AND received_timestamp_ms < ?2",
            params![since_ms, until_ms],
            |row| row.get(0),
        )?;
        let distinct_file_paths: i64 = self.connection.query_row(
            "SELECT COUNT(DISTINCT file_path) FROM events
             WHERE received_timestamp_ms >= ?1 AND received_timestamp_ms < ?2
               AND file_path IS NOT NULL",
            params![since_ms, until_ms],
            |row| row.get(0),
        )?;
        let alerts: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM alerts
             WHERE last_timestamp_ms >= ?1 AND last_timestamp_ms < ?2",
            params![since_ms, until_ms],
            |row| row.get(0),
        )?;
        let health_records: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM health_records
             WHERE observed_timestamp_ms >= ?1 AND observed_timestamp_ms < ?2",
            params![since_ms, until_ms],
            |row| row.get(0),
        )?;
        let notifications = self.notification_counts(since_ms, until_ms)?;
        Ok(RecentStats {
            events: nonnegative_u64(events),
            distinct_file_paths: nonnegative_u64(distinct_file_paths),
            alerts: nonnegative_u64(alerts),
            health_records: nonnegative_u64(health_records),
            notifications_sent: notifications.0,
            notifications_failed: notifications.1,
            notifications_deferred: notifications.2,
            notifications_acknowledged: notifications.3,
        })
    }

    pub fn prune_expired(&mut self, now_ms: i64) -> Result<PruneSummary> {
        let cutoff = now_ms.saturating_sub(DEFAULT_RETENTION_MS);
        let transaction = self.connection.transaction()?;
        let events = transaction.execute(
            "DELETE FROM events WHERE received_timestamp_ms < ?1",
            params![cutoff],
        )?;
        let outbox_entries = transaction.execute(
            "DELETE FROM notification_outbox WHERE created_timestamp_ms < ?1",
            params![cutoff],
        )?;
        let alerts = transaction.execute(
            "DELETE FROM alerts WHERE last_timestamp_ms < ?1",
            params![cutoff],
        )?;
        let health_records = transaction.execute(
            "DELETE FROM health_records WHERE observed_timestamp_ms < ?1",
            params![cutoff],
        )?;
        let notifications = transaction.execute(
            "DELETE FROM notification_feedback WHERE observed_timestamp_ms < ?1",
            params![cutoff],
        )?;
        transaction.commit()?;
        Ok(PruneSummary {
            events,
            alerts,
            health_records,
            notifications,
            outbox_entries,
        })
    }

    pub fn clear_cumulative_stats(&mut self) -> Result<()> {
        self.connection
            .execute("DELETE FROM cumulative_statistics", [])?;
        Ok(())
    }

    fn initialize_schema(connection: &mut Connection) -> Result<()> {
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                let transaction = connection.transaction()?;
                transaction.execute_batch(SCHEMA)?;
                transaction.execute_batch("PRAGMA user_version = 1;")?;
                transaction.commit()?;
            }
            value if value == i64::from(SCHEMA_VERSION) => {}
            _ => {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("不支持的 SQLite schema 版本：{version}"),
                )));
            }
        }
        Ok(())
    }

    fn event_directories(&self, event_id: i64) -> Result<Vec<PathBuf>> {
        let mut statement = self.connection.prepare(
            "SELECT directory_path FROM event_directories
             WHERE event_id = ?1 ORDER BY directory_path",
        )?;
        statement
            .query_map(params![event_id], |row| row.get::<_, String>(0))?
            .map(|path| path.map(PathBuf::from))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| Box::new(error) as _)
    }

    fn notification_counts(&self, since_ms: i64, until_ms: i64) -> Result<(u64, u64, u64, u64)> {
        let mut statement = self.connection.prepare(
            "SELECT outcome, COUNT(*) FROM notification_feedback
             WHERE observed_timestamp_ms >= ?1 AND observed_timestamp_ms < ?2
             GROUP BY outcome",
        )?;
        let rows = statement
            .query_map(params![since_ms, until_ms], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut counts = (0, 0, 0, 0);
        for (outcome, count) in rows {
            match parse_notification_outcome(&outcome)? {
                NotificationOutcome::Sent => counts.0 = nonnegative_u64(count),
                NotificationOutcome::Failed => counts.1 = nonnegative_u64(count),
                NotificationOutcome::Deferred => counts.2 = nonnegative_u64(count),
                NotificationOutcome::Acknowledged => counts.3 = nonnegative_u64(count),
            }
        }
        Ok(counts)
    }
}

fn normalize_directory_list(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut normalized = Vec::with_capacity(paths.len());
    for path in paths {
        let path = normalize_path(path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "目录关联必须使用绝对路径。")
        })?;
        normalized.push(path);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn normalize_optional_query_path(path: Option<&Path>) -> Result<Option<PathBuf>> {
    match path {
        None => Ok(None),
        Some(path) => normalize_path(path).map(Some).ok_or_else(|| {
            Box::new(io::Error::new(
                io::ErrorKind::InvalidInput,
                "查询目录必须是绝对路径。",
            )) as _
        }),
    }
}

fn sequence_key(event: &ActivityEvent) -> Option<String> {
    event
        .global_seq
        .map(|sequence| format!("g:{sequence}"))
        .or_else(|| {
            event.event_seq.map(|sequence| {
                format!(
                    "e:{}:{}:{}:{sequence}",
                    event.process.pid,
                    event
                        .process
                        .pid_version
                        .map_or_else(|| "?".into(), |value| value.to_string()),
                    event_kind_name(event.kind)
                )
            })
        })
}

fn event_kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Open => "open",
        EventKind::Mmap => "mmap",
        EventKind::Create => "create",
        EventKind::Write => "write",
        EventKind::Close => "close",
        EventKind::Rename => "rename",
        EventKind::Exec => "exec",
        EventKind::Fork => "fork",
        EventKind::Exit => "exit",
    }
}

fn alert_rule_name(rule: AlertRule) -> &'static str {
    match rule {
        AlertRule::BulkFileAccess => "bulk_file_access",
        AlertRule::ArchiveCommand => "archive_command",
        AlertRule::ArchiveOutput => "archive_output",
    }
}

fn parse_alert_rule(value: &str) -> Result<AlertRule> {
    match value {
        "bulk_file_access" => Ok(AlertRule::BulkFileAccess),
        "archive_command" => Ok(AlertRule::ArchiveCommand),
        "archive_output" => Ok(AlertRule::ArchiveOutput),
        _ => Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            "数据库包含未知的告警规则。",
        ))),
    }
}

fn notification_outcome_name(outcome: NotificationOutcome) -> &'static str {
    match outcome {
        NotificationOutcome::Sent => "sent",
        NotificationOutcome::Failed => "failed",
        NotificationOutcome::Deferred => "deferred",
        NotificationOutcome::Acknowledged => "acknowledged",
    }
}

fn parse_notification_outcome(value: &str) -> Result<NotificationOutcome> {
    match value {
        "sent" => Ok(NotificationOutcome::Sent),
        "failed" => Ok(NotificationOutcome::Failed),
        "deferred" => Ok(NotificationOutcome::Deferred),
        "acknowledged" => Ok(NotificationOutcome::Acknowledged),
        _ => Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            "数据库包含未知的通知结果。",
        ))),
    }
}

fn u64_to_sql_integer(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|error| Box::new(error) as _)
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or_default()
}

fn path_to_string(path: &Path) -> Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            "路径不能表示为 UTF-8。",
        )) as _
    })
}

fn stored_path_to_string(path: &Path) -> Result<String> {
    match normalize_path(path) {
        Some(path) => path_to_string(&path),
        None => path_to_string(path),
    }
}

fn query_limit(limit: usize) -> i64 {
    if limit == 0 {
        100
    } else {
        limit.clamp(1, MAX_QUERY_LIMIT) as i64
    }
}

fn bounded_detail(detail: &str) -> String {
    detail.chars().take(MAX_DETAIL_CHARS).collect()
}

fn increment_stat(transaction: &Transaction<'_>, key: &str, amount: i64) -> Result<()> {
    transaction.execute(
        "INSERT INTO cumulative_statistics (key, value)
         VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = cumulative_statistics.value + excluded.value",
        params![key, amount],
    )?;
    Ok(())
}

fn read_stat(connection: &Connection, key: &str) -> Result<u64> {
    let value: Option<i64> = connection
        .query_row(
            "SELECT value FROM cumulative_statistics WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.map(nonnegative_u64).unwrap_or_default())
}

#[cfg(unix)]
fn set_database_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path == Path::new(":memory:") || !path.exists() {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_database_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
