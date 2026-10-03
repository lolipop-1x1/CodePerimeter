# 活动关联、批量／归档规则与 SQLite

Status: resolved
Type: task
Blocked by: 01

## Scope

所有权：`src/rules.rs`、`src/storage.rs`、`tests/rules_storage.rs`。基于标准模型输入，不持有 root 或采集进程，不做历史扫描。

## Acceptance

- 多目录严格路径边界；文件去重与 PID 代际；滚动 10 秒／50 个文件可配置；同实例同规则 60 秒合并，新告警即时返回。
- 外部归档命令与文件活动关联，不按任意命令或后缀宣称项目已打包；进程读目录后关联项目内／临时目录归档输出。
- SQLite 版本化 schema、目录配置幂等、事件／告警／健康／通知反馈与查询、30 天明细保留及累计统计。
- 只有宿主调用写入；保存失败返回错误不吞掉，不阻止规则返回实时告警；不保存完整 args／env／文件正文。
- 意义测试覆盖正常批量活动、重复、PID 复用、乱序、目录边界、窗口／合并、归档输出、查询、保留与数据库错误。

## Comments

API 由本任务明确并写在 Answer，供运行宿主接入。[spec](../spec.md) 为需求来源。
- 2026-10-03：实现滚动源时间窗口、路径边界、PID 代际降级、归档活动关联、SQLite 单写调用接口、待通知 outbox、查询与保留；匿名合成集成测试通过。

## Answer

### 规则接口

- `RuleEngine::new(roots: Vec<PathBuf>, config: RuleConfig) -> Result<RuleEngine>` 初始化已规范化的保护目录；`replace_roots(&mut self, roots) -> Result<()>` 替换目录并清空旧进程关联状态。
- `process(&mut self, event: &ActivityEvent) -> RuleOutput` 返回 `alerts: Vec<Alert>`、`health: Vec<RuleHealth>` 和用于事件落库的 `matched_directories: Vec<PathBuf>`。`Alert.is_new` 只表示新告警；后续合并结果为 false。
- 默认批量参数为 10 秒、50 个不同文件；文件打开／映射必须有 `readable == Some(true)` 证据。重复事件提高活动频次，不增加不同文件数。默认最多缓存 256 个进程、每进程 512 个文件／访问频次；淘汰会输出健康记录。来源时间缺失时回退接收时间并输出 `source_timestamp_missing`；时间水位只增不减，旧于窗口的乱序事件不回退窗口并输出 `source_timestamp_out_of_order`。
- `BulkFileAccess` 的 first 是当前批量窗口中保留的最早读事件时间，last 是触发窗口的最大来源时间；`ArchiveCommand`／`ArchiveOutput` 的 first 是参与关联的最早读取证据时间（没有读取证据时等于候选事件时间），last 是命令／输出来源时间。60 秒合并保留 first 的最小值、last 的最大值；批量告警的 `unique_files` 与 `activity_count` 表示合并期间观察到的峰值窗口，不是跨整段累加的原始事件总量。来源时间缺失时这些值使用降级时间；通知反馈使用宿主实际观察时间。
- PID 代际缺失时，告警保留 `pid_version: None`，只按本进程观察到的 Exec 切分临时关联段，并输出 `process_generation_unknown`；不把该分段表示为精确 OS 进程实例。
- 外部归档命令只识别有限常见工具，并须与保护目录输入／工作目录／输出或同进程近期项目读取相关；输出后缀只作为线索。`ArchiveOutput` 与 `BulkFileAccess` 均不证明发生压缩。

### SQLite 宿主接口

`Storage::open(path) -> Result<Storage>` 建立 schema 版本 3，原子迁移 v1 通知队列与 v2 健康来源字段并保留既有证据；SQLite busy timeout 为 100ms，避免锁等待占用实时告警预算。所有修改方法都需要 `&mut self`；运行宿主由普通用户后台进程持有唯一 `Storage` 实例，CLI 与通知会话通过宿主请求查询或提交，不直接打开第二条写入路径。SQLite 写入失败原样返回给宿主，由宿主标记保存缺口；规则输出在写入前生成。公开查询 DTO、filter、通知结果和统计类型支持 Serde JSON IPC。

- 目录配置：`add_directory(&Path, &str, i64) -> Result<bool>`、`remove_directory(&Path) -> Result<bool>`、`list_directories() -> Result<Vec<DirectoryConfig>>`、`active_directory_paths() -> Result<Vec<PathBuf>>`。路径幂等，来源按目录去重；移除配置不删除历史事件。
- 事件：`record_event(&ActivityEvent, &[PathBuf]) -> Result<bool>`；具有源序号时按采集运行实例和序号幂等，无序号事件不伪造去重键。`query_events(&EventFilter) -> Result<Vec<StoredEvent>>` 支持目录、PID、类型、规范化精确文件／目的路径和接收时间范围。
- 告警：`record_alert_at(&Alert, created_timestamp_ms) -> Result<AlertWrite>`；宿主传 `now_ms()` 表示实际生成／入队时刻，来源触发时刻留在 `Alert.first_timestamp_ms`／`last_timestamp_ms`。旧签名便利入口 `record_alert(&Alert) -> Result<AlertWrite>` 以告警来源 last 时间作为创建时间，仅供无接收时刻的场景。仅首次插入且 `alert.is_new == true` 时在同一事务创建一条待通知记录；同 ID 的合并更新不会再次排队。`query_alerts(&AlertFilter) -> Result<Vec<Alert>>` 支持目录、PID、规则、告警来源时间范围。
- 健康：`record_health(&HealthRecord) -> Result<i64>` 与 `query_health(&HealthFilter) -> Result<Vec<StoredHealthRecord>>`；规则侧 `RuleHealth` 由宿主映射为 `component = "rules"` 的记录。健康行与累计计数在单个事务内写入。
- 通知：`pending_notifications(limit) -> Result<Vec<PendingNotification>>` 按排队时间读取详情；`pending_notification_summary() -> Result<PendingNotificationSummary>` 返回未确认数、最早排队时间和按规则汇总。宿主可在桌面会话不可用期间积累记录，恢复后汇总发送。
- 通知反馈：`record_notification(&NotificationRecord) -> Result<i64>` 原子保存反馈和累计统计；`Sent` 表示系统通知 API 接受请求并确认该告警队列项，不证明已到屏；`Failed`、`Deferred` 保持待处理。`acknowledge_notifications(&[String], i64) -> Result<usize>` 对仍待处理项幂等确认并保存 acknowledged 反馈。`query_notifications(&NotificationFilter) -> Result<Vec<StoredNotificationRecord>>` 回查尝试及确认记录。
- 积压摘要快照：`PendingNotificationSummary.latest_sequence: Option<i64>` 是同一 SQLite 读快照中未确认成员的最大队列序号，无 pending 时为 `None`。`acknowledge_pending_notifications_through(sequence: i64, observed_timestamp_ms: i64) -> Result<usize>` 用 SQL 在一个事务插入截止序号及以前仍待处理成员的 acknowledged 反馈、更新队列和累计统计，不将全部 ID 拉入内存。第一次桌面恢复可先汇总发送，再用快照序号确认；同毫秒或墙钟回拨以后新入队的成员仍有更大序号，不会被旧摘要确认。重试返回 0，不增加反馈或统计。
- outbox 的 `sequence INTEGER PRIMARY KEY AUTOINCREMENT` 在详情清理后不复用；v1 升级按原队列 rowid 顺序分配序号并保留排队／确认时间。所有其他证据表不变，迁移与 schema 版本提升在同一事务内。
- 统计与保留：`recent_stats(i64, i64) -> Result<RecentStats>` 返回接收／反馈时间区间统计；`cumulative_stats() -> Result<CumulativeStats>` 返回跨明细清理累计值；`prune_expired(i64) -> Result<PruneSummary>` 删除 30 天以前的事件、告警、健康、通知反馈与待通知明细，不删除累计值；`clear_cumulative_stats() -> Result<()>` 显式清零累计统计。

### 验证

- 2026-10-03：`rustfmt --check --edition 2024 src/rules.rs src/storage.rs` 通过；`cargo test --locked` 通过（10 个集成测试）；`cargo clippy --locked --all-targets -- -D warnings` 通过。
- 2026-10-03：补充 outbox 单调快照确认，13 项 rules/storage 测试通过：10,001 条 pending 批量确认，摘要后同毫秒／更早时间新告警保留，幂等重试、详情清理不复用序号、更新故障回滚反馈／队列／统计，以及 v1 迁移保留全部既有证据与 pending。

### 宿主处理顺序

1. 先调用 `RuleEngine::process`，将 `RuleOutput.matched_directories` 与事件一起传给 `record_event`；逐项持久化规则健康状态。写入失败由宿主记录保存缺口，不能丢弃已生成的实时告警；待通知写入失败时，宿主仍对内存中的新告警尝试即时通知，但该条不能在重启后从 outbox 恢复。
2. 对每条告警调用 `record_alert_at(alert, now_ms())`。新告警在 SQLite 事务中进入待通知队列，通知工作循环立即读取并发送；桌面会话恢复时读取同一队列并按规则汇总。数据库短暂失败后的写重试由宿主节流，避免每条后续事件都等待 SQLite 锁。
3. 通知 API 接受请求后记录 `NotificationOutcome::Sent`；发送失败或无桌面会话时分别记录 `Failed`／`Deferred`，待处理项保留供后续重试或汇总。通知调用与 SQLite 确认不能形成跨系统事务；若进程在系统接受通知后、保存 Sent 前退出，恢复时可能重复发送。

详细回查使用各自 filter：`EventFilter`、`AlertFilter`、`HealthFilter`、`NotificationFilter`；健康查询支持 `source_run_id`，source保留版本、缺失字段与丢事件数；通知反馈及队列状态均是观察结果，不表示用户已看到通知或策略已拦截操作。

- 2026-10-04审查补修：来源版本／结构化缺口与v3迁移、控制连接错误隔离、宿主启动及每小时明细清理、故障合并告警的持久通知恢复和已发送去重均已补齐；针对性组件证据见 [双轴审查](../code-review.md)。系统root／FDA与后台验收仍由06记录。
