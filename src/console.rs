//! 控制台的平台无关请求与唯一 SQLite 写入路径。

use crate::Result;
use crate::model::{Alert, now_ms};
use crate::rules::{RuleConfig, normalize_path};
use crate::storage::{
    AlertFilter, EventFilter, NotificationFilter, Storage, StoredEvent, StoredNotificationRecord,
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};

const PAGE_BYTES: usize = 1024 * 1024;

pub(crate) const CONSOLE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS console_settings (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS directory_state (
 path TEXT PRIMARY KEY NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
 FOREIGN KEY(path) REFERENCES monitored_directories(path) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS alert_metadata (
 alert_id TEXT PRIMARY KEY NOT NULL, is_read INTEGER NOT NULL DEFAULT 0,
 processed INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '',
 revision INTEGER NOT NULL DEFAULT 1, rule_version INTEGER NOT NULL DEFAULT 0,
 rule_snapshot TEXT,
 FOREIGN KEY(alert_id) REFERENCES alerts(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS alert_handling_history (
 id INTEGER PRIMARY KEY, alert_id TEXT NOT NULL, timestamp_ms INTEGER NOT NULL,
 is_read INTEGER NOT NULL, processed INTEGER NOT NULL, note TEXT NOT NULL,
 FOREIGN KEY(alert_id) REFERENCES alerts(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS alert_handling_alert ON alert_handling_history(alert_id,id);
INSERT OR IGNORE INTO alert_metadata(alert_id) SELECT id FROM alerts;
";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RulesSettings {
    pub version: u64,
    pub bulk_enabled: bool,
    pub archive_command_enabled: bool,
    pub archive_output_enabled: bool,
    pub bulk_file_threshold: usize,
    pub bulk_window_ms: i64,
    pub alert_merge_window_ms: i64,
    pub archive_correlation_window_ms: i64,
}

impl Default for RulesSettings {
    fn default() -> Self {
        Self::from_config(&RuleConfig::default())
    }
}

impl RulesSettings {
    pub fn from_config(config: &RuleConfig) -> Self {
        Self {
            version: 1,
            bulk_enabled: true,
            archive_command_enabled: true,
            archive_output_enabled: true,
            bulk_file_threshold: config.bulk_file_threshold,
            bulk_window_ms: config.bulk_window_ms,
            alert_merge_window_ms: config.alert_merge_window_ms,
            archive_correlation_window_ms: config.archive_correlation_window_ms,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if !(1..=512).contains(&self.bulk_file_threshold)
            || !(100..=3_600_000).contains(&self.bulk_window_ms)
            || !(100..=3_600_000).contains(&self.alert_merge_window_ms)
            || !(100..=3_600_000).contains(&self.archive_correlation_window_ms)
        {
            return Err(invalid("规则阈值须为1至512，时间须为100至3600000毫秒"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", content = "payload", rename_all = "snake_case")]
pub enum ConsoleRequest {
    Summary {
        since_ms: Option<i64>,
        until_ms: Option<i64>,
    },
    Directories,
    DirectorySet {
        path: PathBuf,
        enabled: bool,
    },
    DirectoryRemovePreview {
        path: PathBuf,
    },
    RulesGet,
    RulesSet {
        settings: RulesSettings,
    },
    EventsPage {
        filter: EventFilter,
        cursor: Option<String>,
        search: Option<String>,
        #[serde(default)]
        archive_only: bool,
    },
    AlertsPage {
        filter: AlertFilter,
        cursor: Option<String>,
        search: Option<String>,
        is_read: Option<bool>,
        processed: Option<bool>,
    },
    EventDetail {
        id: i64,
    },
    AlertDetail {
        id: String,
    },
    AlertUpdate {
        id: String,
        is_read: Option<bool>,
        processed: Option<bool>,
        note: Option<String>,
        expected_revision: Option<u64>,
    },
    RetentionGet,
    RetentionPreview {
        days: u32,
    },
    RetentionSet {
        days: u32,
        confirm: bool,
        preview_revision: Option<u64>,
    },
    ClearDetails {
        confirm: bool,
    },
    ClearCumulative {
        confirm: bool,
    },
    MonitoringSet {
        paused: bool,
    },
    RecordOperation {
        operation: String,
        outcome: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsoleDirectory {
    pub path: PathBuf,
    pub sources: Vec<String>,
    pub added_at_ms: i64,
    pub exists: bool,
    pub enabled: bool,
    pub effective: bool,
    pub excluded_by: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandlingRecord {
    pub timestamp_ms: i64,
    pub is_read: bool,
    pub processed: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlertEntry {
    pub alert: Alert,
    pub is_read: bool,
    pub processed: bool,
    pub note: String,
    pub revision: u64,
    pub rule_version: u64,
    pub rule_snapshot: Option<RulesSettings>,
    pub handling_history: Vec<HandlingRecord>,
    pub handling_history_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<StoredEvent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<Vec<StoredNotificationRecord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cursor {
    kind: String,
    upper: i64,
    time: i64,
    id: i64,
    signature: String,
    generation: u64,
}

fn invalid(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message))
}

fn path_text(path: &Path) -> Result<String> {
    normalize_path(path)
        .and_then(|path| path.to_str().map(str::to_owned))
        .ok_or_else(|| invalid("目录路径必须为有效绝对路径"))
}

fn optional_path(path: Option<&Path>) -> Result<Option<String>> {
    path.map(path_text).transpose()
}

fn search_text(search: Option<&str>) -> Result<Option<String>> {
    if search.is_some_and(|text| text.len() > 256) {
        return Err(invalid("搜索内容超过上限"));
    }
    Ok(search
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_lowercase))
}

pub(crate) fn record_alert_metadata(
    transaction: &Transaction<'_>,
    alert: &Alert,
    exists: bool,
    changed: bool,
) -> Result<()> {
    if !exists {
        let snapshot: Option<String> = transaction
            .query_row(
                "SELECT value FROM console_settings WHERE key='rules'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let settings = snapshot
            .as_deref()
            .map(serde_json::from_str::<RulesSettings>)
            .transpose()?;
        transaction.execute(
            "INSERT INTO alert_metadata(alert_id,rule_version,rule_snapshot) VALUES(?1,?2,?3)",
            params![
                alert.id,
                settings.as_ref().map_or(0, |value| value.version),
                snapshot
            ],
        )?;
    } else if changed {
        // 合并产生的新证据重新打开告警；备注与既有处理历史保持原样。
        transaction.execute(
            "UPDATE alert_metadata SET is_read=0,processed=0,revision=revision+1 WHERE alert_id=?1",
            params![alert.id],
        )?;
    }
    Ok(())
}

impl Storage {
    fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM console_settings WHERE key=?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn put_setting(&self, key: &str, value: &str) -> Result<()> {
        self.connection.execute("INSERT INTO console_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value])?;
        Ok(())
    }

    pub fn initialize_console_rules(&self, initial: RulesSettings) -> Result<RulesSettings> {
        if self.setting("rules")?.is_none() {
            initial.validate()?;
            self.put_setting("rules", &serde_json::to_string(&initial)?)?;
        }
        self.console_rules()
    }

    pub fn console_rules(&self) -> Result<RulesSettings> {
        self.setting("rules")?.map_or_else(
            || Ok(RulesSettings::default()),
            |text| Ok(serde_json::from_str(&text)?),
        )
    }

    pub fn save_console_rules(&mut self, mut settings: RulesSettings) -> Result<RulesSettings> {
        settings.validate()?;
        let current = self.console_rules()?;
        if settings.version != current.version {
            return Err(invalid("规则版本冲突，请刷新后重试"));
        }
        settings.version = current
            .version
            .checked_add(1)
            .ok_or_else(|| invalid("规则版本超过上限"))?;
        self.put_setting("rules", &serde_json::to_string(&settings)?)?;
        Ok(settings)
    }

    pub fn console_directories(&self) -> Result<Vec<ConsoleDirectory>> {
        let base = self.list_directories()?;
        let states: Vec<(String, bool)> = {
            let mut statement = self
                .connection
                .prepare("SELECT path,enabled FROM directory_state")?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        let exclusions: Vec<PathBuf> = states
            .iter()
            .filter(|(_, enabled)| !enabled)
            .map(|(path, _)| PathBuf::from(path))
            .collect();
        Ok(base
            .into_iter()
            .map(|directory| {
                let enabled = states
                    .iter()
                    .find(|(path, _)| Path::new(path) == directory.path)
                    .is_none_or(|(_, enabled)| *enabled);
                let excluded_by = exclusions
                    .iter()
                    .filter(|root| directory.path.starts_with(root))
                    .max_by_key(|root| root.components().count())
                    .cloned();
                ConsoleDirectory {
                    effective: enabled && directory.exists && excluded_by.is_none(),
                    enabled,
                    excluded_by,
                    path: directory.path,
                    sources: directory.sources,
                    added_at_ms: directory.added_at_ms,
                    exists: directory.exists,
                }
            })
            .collect())
    }

    pub fn excluded_directory_paths(&self) -> Result<Vec<PathBuf>> {
        Ok(self
            .console_directories()?
            .into_iter()
            .filter(|directory| !directory.enabled)
            .map(|directory| directory.path)
            .collect())
    }

    pub fn set_console_directory(
        &mut self,
        path: &Path,
        enabled: bool,
    ) -> Result<Vec<ConsoleDirectory>> {
        let path = path_text(path)?;
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM monitored_directories WHERE path=?1)",
            params![path],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(invalid("目录尚未配置"));
        }
        self.connection.execute("INSERT INTO directory_state(path,enabled) VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET enabled=excluded.enabled", params![path,enabled])?;
        self.console_directories()
    }

    pub fn console_directory_remove_preview(&self, path: &Path) -> Result<Value> {
        let path = PathBuf::from(path_text(path)?);
        let before = self.console_directories()?;
        let removed = before
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| invalid("目录尚未配置"))?;
        let exclusions: Vec<_> = before
            .iter()
            .filter(|entry| entry.path != path && !entry.enabled)
            .map(|entry| entry.path.clone())
            .collect();
        let enabled_ancestors: Vec<_> = before
            .iter()
            .filter(|entry| {
                entry.path != path && entry.enabled && entry.exists && path.starts_with(&entry.path)
            })
            .map(|entry| entry.path.clone())
            .collect();
        let affected_descendants: Vec<_> = before
            .iter()
            .filter(|entry| entry.path != path && entry.path.starts_with(&path))
            .map(|entry| entry.path.clone())
            .collect();
        let covered_by_exclusion = exclusions.iter().any(|root| path.starts_with(root));
        let mut after: Vec<_> = before
            .iter()
            .filter(|entry| entry.path != path)
            .cloned()
            .collect();
        for entry in &mut after {
            entry.excluded_by = exclusions
                .iter()
                .filter(|root| entry.path.starts_with(root))
                .max_by_key(|root| root.components().count())
                .cloned();
            entry.effective = entry.enabled && entry.exists && entry.excluded_by.is_none();
        }
        Ok(
            json!({"removed_path":path,"removes_exclusion":!removed.enabled,"coverage_may_expand":!removed.enabled && !covered_by_exclusion && (!enabled_ancestors.is_empty() || after.iter().any(|entry| entry.effective && entry.path.starts_with(&path))),"enabled_ancestors":enabled_ancestors,"affected_descendants":affected_descendants,"after":after}),
        )
    }

    pub fn monitoring_paused(&self) -> Result<bool> {
        Ok(self.setting("monitoring_paused")?.as_deref() == Some("true"))
    }
    pub fn set_monitoring_paused(&mut self, paused: bool) -> Result<()> {
        self.put_setting("monitoring_paused", if paused { "true" } else { "false" })
    }
    pub fn retention_days(&self) -> Result<u32> {
        self.setting("retention_days")?.map_or(Ok(30), |text| {
            text.parse().map_err(|_| invalid("保存期限数据无效"))
        })
    }

    pub fn console_counts(&self) -> Result<Value> {
        let mut counts = serde_json::Map::new();
        for (key, table) in [
            ("events", "events"),
            ("alerts", "alerts"),
            ("health_records", "health_records"),
            ("notifications", "notification_feedback"),
            ("handling_records", "alert_handling_history"),
        ] {
            let count: i64 =
                self.connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?;
            counts.insert(key.into(), json!(count));
        }
        Ok(Value::Object(counts))
    }

    pub fn console_retention(&self) -> Result<Value> {
        Ok(json!({"days":self.retention_days()?,"counts":self.console_counts()?}))
    }

    pub fn console_retention_preview(&self, days: u32, cutoff_ms: i64) -> Result<Value> {
        if !(1..=3650).contains(&days) {
            return Err(invalid("保存期限须为1至3650天"));
        }
        let mut counts = serde_json::Map::new();
        for (key, sql) in [
            (
                "events",
                "SELECT COUNT(*) FROM events WHERE received_timestamp_ms<?1",
            ),
            (
                "alerts",
                "SELECT COUNT(*) FROM alerts WHERE last_timestamp_ms<?1",
            ),
            (
                "health_records",
                "SELECT COUNT(*) FROM health_records WHERE observed_timestamp_ms<?1",
            ),
            (
                "notifications",
                "SELECT COUNT(*) FROM notification_feedback WHERE observed_timestamp_ms<?1 OR alert_id IN (SELECT id FROM alerts WHERE last_timestamp_ms<?1)",
            ),
            (
                "handling_records",
                "SELECT COUNT(*) FROM alert_handling_history WHERE alert_id IN (SELECT id FROM alerts WHERE last_timestamp_ms<?1)",
            ),
        ] {
            let count: u64 = self
                .connection
                .query_row(sql, params![cutoff_ms], |row| row.get(0))?;
            counts.insert(key.into(), json!(count));
        }
        Ok(json!({"days":days,"cutoff_ms":cutoff_ms,"counts":counts}))
    }

    pub(crate) fn set_console_retention_before(
        &mut self,
        days: u32,
        cutoff_ms: i64,
    ) -> Result<Value> {
        let generation = self.cursor_generation()?.saturating_add(1);
        let transaction = self.connection.transaction()?;
        transaction.execute("INSERT INTO console_settings(key,value) VALUES('retention_days',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![days.to_string()])?;
        crate::storage::prune_transaction(&transaction, cutoff_ms, generation)?;
        transaction.commit()?;
        self.console_retention()
    }

    pub fn set_console_retention(&mut self, days: u32, confirm: bool) -> Result<Value> {
        if !(1..=3650).contains(&days) {
            return Err(invalid("保存期限须为1至3650天"));
        }
        if days < self.retention_days()? && !confirm {
            return Err(invalid("缩短保存期限需要确认"));
        }
        self.set_console_retention_before(
            days,
            now_ms().saturating_sub(i64::from(days) * 86_400_000),
        )
    }

    pub fn clear_console_details(&mut self, confirm: bool) -> Result<Value> {
        if !confirm {
            return Err(invalid("清除明细需要确认"));
        }
        let counts = self.console_counts()?;
        let next_generation = self.cursor_generation()?.saturating_add(1);
        let transaction = self.connection.transaction()?;
        transaction
            .execute_batch("DELETE FROM events; DELETE FROM alerts; DELETE FROM health_records;")?;
        transaction.execute("INSERT INTO console_settings(key,value) VALUES('cursor_generation',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![next_generation.to_string()])?;
        transaction.commit()?;
        Ok(json!({"deleted":counts}))
    }

    pub(crate) fn cursor_generation(&self) -> Result<u64> {
        self.setting("cursor_generation")?.map_or(Ok(0), |text| {
            text.parse().map_err(|_| invalid("分页状态无效"))
        })
    }

    fn page_cursor(
        &self,
        kind: &str,
        cursor: Option<&str>,
        signature: String,
        table: &str,
    ) -> Result<Cursor> {
        let generation = self.cursor_generation()?;
        if let Some(text) = cursor {
            if text.len() > 4096 {
                return Err(invalid("分页游标超过上限"));
            }
            let parsed: Cursor = serde_json::from_str(text).map_err(|_| invalid("分页游标无效"))?;
            if parsed.kind != kind
                || parsed.signature != signature
                || parsed.generation != generation
            {
                return Err(invalid("筛选条件或记录已变化，请从首页重新查询"));
            }
            return Ok(parsed);
        }
        let upper = self.connection.query_row(
            &format!("SELECT COALESCE(MAX(rowid),0) FROM {table}"),
            [],
            |row| row.get(0),
        )?;
        Ok(Cursor {
            kind: kind.into(),
            upper,
            time: i64::MAX,
            id: i64::MAX,
            signature,
            generation,
        })
    }

    pub fn console_events_page(
        &self,
        filter: &EventFilter,
        cursor: Option<&str>,
        search: Option<&str>,
        archive_only: bool,
    ) -> Result<Page<StoredEvent>> {
        let directory = optional_path(filter.directory.as_deref())?;
        let file = optional_path(filter.file_path.as_deref())?;
        let search = search_text(search)?;
        let signature = serde_json::to_string(&(
            directory.clone(),
            filter.pid,
            filter.kind,
            file.clone(),
            filter.since_ms,
            filter.until_ms,
            search.clone(),
            archive_only,
        ))?;
        let mut cursor = self.page_cursor("events", cursor, signature, "events")?;
        let kind = filter
            .kind
            .map(|value| serde_json::to_value(value).unwrap_or(Value::Null))
            .and_then(|value| value.as_str().map(str::to_owned));
        let limit = filter.limit.clamp(1, 100);
        let condition = "e.id<=?1 AND (?2 IS NULL OR e.received_timestamp_ms>=?2) AND (?3 IS NULL OR e.received_timestamp_ms<?3) AND (?4 IS NULL OR e.pid=?4) AND (?5 IS NULL OR e.kind=?5) AND (?6 IS NULL OR EXISTS(SELECT 1 FROM event_directories d WHERE d.event_id=e.id AND d.directory_path=?6)) AND (?7 IS NULL OR e.file_path=?7 OR e.destination_path=?7) AND (?8 IS NULL OR instr(lower(coalesce(e.file_path,'')||' '||coalesce(e.destination_path,'')||' '||coalesce(json_extract(e.event_json,'$.process.executable'),'')||' '||coalesce(json_extract(e.event_json,'$.archive'),'')),?8)>0) AND (?9=0 OR json_extract(e.event_json,'$.archive') IS NOT NULL OR EXISTS(SELECT 1 FROM alerts a WHERE a.rule IN ('archive_command','archive_output') AND a.pid=e.pid AND a.pid_version IS e.pid_version AND coalesce(e.source_timestamp_ms,e.received_timestamp_ms)>=a.first_timestamp_ms AND coalesce(e.source_timestamp_ms,e.received_timestamp_ms)<=a.last_timestamp_ms AND substr(a.id,1,length(e.source_run_id)+1)=e.source_run_id||':' AND EXISTS(SELECT 1 FROM json_each(a.alert_json,'$.evidence_paths') p WHERE p.value=e.file_path OR p.value=e.destination_path)))";
        let total: u64 = self.connection.query_row(
            &format!("SELECT COUNT(*) FROM events e WHERE {condition}"),
            params![
                cursor.upper,
                filter.since_ms,
                filter.until_ms,
                filter.pid,
                kind,
                directory,
                file,
                search,
                archive_only
            ],
            |row| row.get(0),
        )?;
        let rows = {
            let mut statement = self.connection.prepare(&format!("SELECT e.id,e.received_timestamp_ms,e.event_json FROM events e WHERE {condition} AND (e.received_timestamp_ms<?10 OR (e.received_timestamp_ms=?10 AND e.id<?11)) ORDER BY e.received_timestamp_ms DESC,e.id DESC LIMIT ?12"))?;
            statement
                .query_map(
                    params![
                        cursor.upper,
                        filter.since_ms,
                        filter.until_ms,
                        filter.pid,
                        kind,
                        directory,
                        file,
                        search,
                        archive_only,
                        cursor.time,
                        cursor.id,
                        limit + 1
                    ],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut has_more = rows.len() > limit;
        let mut items = Vec::new();
        let mut bytes = 0;
        for (id, time, text) in rows.into_iter().take(limit) {
            let item = StoredEvent {
                id,
                event: serde_json::from_str(&text)?,
                directories: self.console_event_directories(id)?,
            };
            let size = serde_json::to_vec(&item)?.len();
            if !items.is_empty() && bytes + size > PAGE_BYTES {
                has_more = true;
                break;
            }
            bytes += size;
            items.push(item);
            cursor.time = time;
            cursor.id = id;
        }
        Ok(Page {
            items,
            total,
            next_cursor: if has_more {
                Some(serde_json::to_string(&cursor)?)
            } else {
                None
            },
        })
    }

    fn console_event_directories(&self, id: i64) -> Result<Vec<PathBuf>> {
        let mut statement = self.connection.prepare("SELECT directory_path FROM event_directories WHERE event_id=?1 ORDER BY directory_path")?;
        Ok(statement
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .map(|value| value.map(PathBuf::from))
            .collect::<std::result::Result<_, _>>()?)
    }

    pub fn console_alerts_page(
        &self,
        filter: &AlertFilter,
        cursor: Option<&str>,
        search: Option<&str>,
        is_read: Option<bool>,
        processed: Option<bool>,
    ) -> Result<Page<AlertEntry>> {
        let directory = optional_path(filter.directory.as_deref())?;
        let search = search_text(search)?;
        let rule = filter
            .rule
            .map(|value| serde_json::to_value(value).unwrap_or(Value::Null))
            .and_then(|value| value.as_str().map(str::to_owned));
        let signature = serde_json::to_string(&(
            directory.clone(),
            filter.pid,
            rule.clone(),
            filter.since_ms,
            filter.until_ms,
            search.clone(),
            is_read,
            processed,
        ))?;
        let mut cursor = self.page_cursor("alerts", cursor, signature, "alerts")?;
        let limit = filter.limit.clamp(1, 100);
        let condition = "a.rowid<=?1 AND (?2 IS NULL OR a.last_timestamp_ms>=?2) AND (?3 IS NULL OR a.last_timestamp_ms<?3) AND (?4 IS NULL OR a.pid=?4) AND (?5 IS NULL OR a.rule=?5) AND (?6 IS NULL OR EXISTS(SELECT 1 FROM alert_directories d WHERE d.alert_id=a.id AND d.directory_path=?6)) AND (?7 IS NULL OR instr(lower(a.alert_json||' '||m.note),?7)>0) AND (?8 IS NULL OR m.is_read=?8) AND (?9 IS NULL OR m.processed=?9)";
        let total = self.connection.query_row(&format!("SELECT COUNT(*) FROM alerts a JOIN alert_metadata m ON m.alert_id=a.id WHERE {condition}"), params![cursor.upper,filter.since_ms,filter.until_ms,filter.pid,rule,directory,search,is_read,processed], |row| row.get(0))?;
        let rows = {
            let mut statement = self.connection.prepare(&format!("SELECT a.rowid,a.id FROM alerts a JOIN alert_metadata m ON m.alert_id=a.id WHERE {condition} AND a.rowid<?10 ORDER BY a.rowid DESC LIMIT ?11"))?;
            statement
                .query_map(
                    params![
                        cursor.upper,
                        filter.since_ms,
                        filter.until_ms,
                        filter.pid,
                        rule,
                        directory,
                        search,
                        is_read,
                        processed,
                        cursor.id,
                        limit + 1
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut has_more = rows.len() > limit;
        let mut items = Vec::new();
        let mut bytes = 0;
        for (rowid, id) in rows.into_iter().take(limit) {
            let item = self.console_alert_entry(&id, false)?;
            let size = serde_json::to_vec(&item)?.len();
            if !items.is_empty() && bytes + size > PAGE_BYTES {
                has_more = true;
                break;
            }
            bytes += size;
            items.push(item);
            cursor.id = rowid;
        }
        Ok(Page {
            items,
            total,
            next_cursor: if has_more {
                Some(serde_json::to_string(&cursor)?)
            } else {
                None
            },
        })
    }

    pub fn console_alert_entry(&self, id: &str, detail: bool) -> Result<AlertEntry> {
        let (text,is_read,processed,note,revision,rule_version,snapshot): (String,bool,bool,String,u64,u64,Option<String>) = self.connection.query_row("SELECT a.alert_json,m.is_read,m.processed,m.note,m.revision,m.rule_version,m.rule_snapshot FROM alerts a JOIN alert_metadata m ON a.id=m.alert_id WHERE a.id=?1", params![id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?))).optional()?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "alert_unavailable"))?;
        let alert: Alert = serde_json::from_str(&text)?;
        let mut history = {
            let mut statement=self.connection.prepare("SELECT timestamp_ms,is_read,processed,note FROM alert_handling_history WHERE alert_id=?1 ORDER BY id DESC LIMIT 101")?;
            statement
                .query_map(params![id], |row| {
                    Ok(HandlingRecord {
                        timestamp_ms: row.get(0)?,
                        is_read: row.get(1)?,
                        processed: row.get(2)?,
                        note: row.get(3)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let handling_history_truncated = history.len() > 100;
        history.truncate(100);
        history.reverse();
        let mut entry = AlertEntry {
            alert,
            is_read,
            processed,
            note,
            revision,
            rule_version,
            rule_snapshot: snapshot
                .map(|text| serde_json::from_str(&text))
                .transpose()?,
            handling_history: history,
            handling_history_truncated,
            events: None,
            notifications: None,
        };
        if detail {
            // 关联依据为进程代际、触发窗口及路径；不宣称文件内容相同。
            let mut statement=self.connection.prepare("SELECT id,event_json FROM events WHERE pid=?1 AND pid_version IS ?2 AND (coalesce(source_timestamp_ms,received_timestamp_ms)>=?3 AND coalesce(source_timestamp_ms,received_timestamp_ms)<=?4) ORDER BY received_timestamp_ms DESC,id DESC LIMIT 100")?;
            let rows = statement
                .query_map(
                    params![
                        entry.alert.process.pid,
                        entry.alert.process.pid_version,
                        entry.alert.first_timestamp_ms,
                        entry.alert.last_timestamp_ms
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut events = Vec::new();
            for (id, text) in rows {
                let event: crate::model::ActivityEvent = serde_json::from_str(&text)?;
                if !entry
                    .alert
                    .id
                    .starts_with(&format!("{}:", event.source_run_id))
                {
                    continue;
                }
                let directories = self.console_event_directories(id)?;
                if directories
                    .iter()
                    .any(|path| entry.alert.roots.contains(path))
                {
                    events.push(StoredEvent {
                        id,
                        event,
                        directories,
                    });
                }
            }
            entry.events = Some(events);
            entry.notifications = Some(self.query_notifications(&NotificationFilter {
                alert_id: Some(id.into()),
                limit: 100,
                ..Default::default()
            })?);
        }
        Ok(entry)
    }

    pub fn console_event_detail(&self, id: i64) -> Result<Value> {
        let text: String = self.connection.query_row(
            "SELECT event_json FROM events WHERE id=?1",
            params![id],
            |row| row.get(0),
        )?;
        let event = StoredEvent {
            id,
            event: serde_json::from_str(&text)?,
            directories: self.console_event_directories(id)?,
        };
        let timestamp = event
            .event
            .source_timestamp_ms
            .unwrap_or(event.event.received_timestamp_ms);
        let ids = {
            let mut statement=self.connection.prepare("SELECT id FROM alerts WHERE pid=?1 AND pid_version IS ?2 AND first_timestamp_ms<=?3 AND last_timestamp_ms>=?3 ORDER BY last_timestamp_ms DESC LIMIT 100")?;
            statement
                .query_map(
                    params![
                        event.event.process.pid,
                        event.event.process.pid_version,
                        timestamp
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut alerts = Vec::new();
        for id in ids {
            let entry = self.console_alert_entry(&id, false)?;
            if entry
                .alert
                .id
                .starts_with(&format!("{}:", event.event.source_run_id))
                && event
                    .directories
                    .iter()
                    .any(|path| entry.alert.roots.contains(path))
            {
                alerts.push(entry);
            }
        }
        Ok(json!({"event":event,"alerts":alerts}))
    }

    pub fn console_alert_update(
        &mut self,
        id: &str,
        is_read: Option<bool>,
        processed: Option<bool>,
        note: Option<&str>,
        expected_revision: Option<u64>,
    ) -> Result<AlertEntry> {
        if note.is_some_and(|text| text.len() > 2048 || text.contains('\0')) {
            return Err(invalid("备注须少于2048字节且不能包含空字符"));
        }
        let current = self.console_alert_entry(id, false)?;
        if expected_revision.is_some_and(|value| value != current.revision) {
            return Err(invalid("告警版本冲突，请刷新后重试"));
        }
        let is_read = is_read.unwrap_or(current.is_read);
        let processed = processed.unwrap_or(current.processed);
        let note = note.unwrap_or(&current.note);
        let transaction = self.connection.transaction()?;
        transaction.execute("UPDATE alert_metadata SET is_read=?2,processed=?3,note=?4,revision=revision+1 WHERE alert_id=?1",params![id,is_read,processed,note])?;
        transaction.execute("INSERT INTO alert_handling_history(alert_id,timestamp_ms,is_read,processed,note) VALUES(?1,?2,?3,?4,?5)",params![id,now_ms(),is_read,processed,note])?;
        transaction.commit()?;
        self.console_alert_entry(id, true)
    }

    pub fn console_summary(&self, since_ms: Option<i64>, until_ms: Option<i64>) -> Result<Value> {
        let until = until_ms.unwrap_or_else(now_ms);
        let since = since_ms.unwrap_or_else(|| until.saturating_sub(86_400_000));
        if until <= since || until.saturating_sub(since) > 3650_i64 * 86_400_000 {
            return Err(invalid("统计时间范围无效"));
        }
        let width = (until.saturating_sub(since) / 72).max(60_000);
        let trend = {
            let mut statement=self.connection.prepare("SELECT ((received_timestamp_ms-?1)/?3)*?3+?1 AS bucket,SUM(kind='open'),SUM(kind='mmap'),SUM(json_extract(event_json,'$.archive') IS NOT NULL) FROM events WHERE received_timestamp_ms>=?1 AND received_timestamp_ms<?2 GROUP BY bucket ORDER BY bucket LIMIT 73")?;
            statement.query_map(params![since,until,width], |row| Ok(json!({"timestamp_ms":row.get::<_,i64>(0)?,"open":row.get::<_,u64>(1)?,"mmap":row.get::<_,u64>(2)?,"archive":row.get::<_,u64>(3)?})))?.collect::<std::result::Result<Vec<_>,_>>()?
        };
        let top_processes = {
            let mut statement=self.connection.prepare("SELECT pid,json_extract(event_json,'$.process.executable'),COUNT(*) AS count FROM events WHERE received_timestamp_ms>=?1 AND received_timestamp_ms<?2 GROUP BY pid,json_extract(event_json,'$.process.executable') ORDER BY count DESC,pid LIMIT 10")?;
            statement.query_map(params![since,until], |row| Ok(json!({"pid":row.get::<_,u32>(0)?,"executable":row.get::<_,Option<String>>(1)?,"count":row.get::<_,u64>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?
        };
        let top_files = {
            let mut statement=self.connection.prepare("SELECT file_path,COUNT(*) AS count FROM events WHERE received_timestamp_ms>=?1 AND received_timestamp_ms<?2 AND file_path IS NOT NULL GROUP BY file_path ORDER BY count DESC,file_path LIMIT 10")?;
            statement
                .query_map(params![since, until], |row| {
                    Ok(json!({"path":row.get::<_,String>(0)?,"count":row.get::<_,u64>(1)?}))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let unread: u64 = self.connection.query_row(
            "SELECT COUNT(*) FROM alert_metadata WHERE is_read=0",
            [],
            |row| row.get(0),
        )?;
        let pending: u64 = self.connection.query_row(
            "SELECT COUNT(*) FROM alert_metadata WHERE processed=0",
            [],
            |row| row.get(0),
        )?;
        Ok(
            json!({"stats":{"cumulative":self.cumulative_stats()?,"recent":self.recent_stats(since,until)?},"trend":trend,"top_processes":top_processes,"top_files":top_files,"unread_alerts":unread,"pending_alerts":pending,"since_ms":since,"until_ms":until}),
        )
    }
}
