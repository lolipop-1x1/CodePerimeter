# 后台生命周期、最小权限 IPC、CLI 与通知

Status: resolved
Type: task
Blocked by: 02, 03, 04, 07

## Scope

本票据实现所有权经拆分调整为 `src/runtime.rs`、`tests/runtime_host.rs`；CLI、使用文档与 CLI 集成测试由独立票据负责。采集桥接与安装管理由 07 提供；不得修改已完成模块，需 API 调整通过主 agent 协调。

## Acceptance

- Rust CLI 配置多个目录、历史发现／预览／批量导入、事件／告警／状态查询与服务管理。
- launchd 系统后台开机运行，root 仅采集／最小转发，普通用户宿主唯一 SQLite 写入，桌面通知会话独立；IPC 验证对端且原始输出仅内存存在。
- 输入断开、错误、序号缺口、过载、SQLite 与通知失败显示状态，恢复不伪装连续完整。
- 新告警及时发通知，60 秒合并不重发；无桌面会话持久化待展示，进入桌面后汇总，SQLite 保存失败仍可实时提醒。
- 安装、启动、停止、卸载可执行且清单明确；用户数据默认保留；不修改 sudoers、SIP 或 AMFI，不自动重启机器。
- 测试覆盖 CLI 到服务到 SQLite 真实 IPC、访问控制、异常／恢复；系统 FDA 与真实重启另验。

## Answer

### Runtime 接口

- `RuntimeOptions { collector_socket, control_socket, database_path, bulk_file_threshold, bulk_window_ms }`。批量访问门槛默认 50 个不同文件／10,000 毫秒，可由宿主调用方传入并由状态查询回显。
- `ControlRequest` 使用 Serde 内部标签 `operation`、内容字段 `payload`，枚举名为 snake_case；`ControlResponse { ok, error, data }` 的查询结果直接序列化对应 DTO。稳定请求与 JSON 字段见 `/private/tmp/codeperimeter-mvp-research/runtime-api.md`。
- 请求字段固定为：`AddDirectories { entries: Vec<DirectoryImport { path, sources }> }`、`RemoveDirectory { path }`、`QueryEvents { filter: EventFilter }`、`QueryAlerts { filter: AlertFilter }`、`QueryHealth { filter: HealthFilter }`、`QueryNotifications { filter: NotificationFilter }`、`Stats { since_ms, until_ms }`、`PendingNotifications { limit }`、`NotifyPoll { session_id }`、`NotifySummaryResult { session_id, sequence, success }`、`NotifyAlertResult { session_id, alert_id, success }`，以及无 payload 的 `Status`、`ListDirectories`、`ClearCumulativeStats`、`Stop`。默认区间统计省略起止时间时覆盖最近 24 小时。
- `Status.data` 是 `RuntimeStatus`：`state, uid, bulk_file_threshold, bulk_window_ms, collector_state, collector_run_id, collector_dropped_lines, reader_dropped_frames, database_state, database_gap_events, database_error, last_event_received_ms, observed_events_by_kind, persisted_events_by_kind, filtered_events_by_kind, duplicate_events, memory_pending_notifications, memory_dropped_notifications, notify_session_active, notify_session_last_seen_ms`。通知轮询 `data.kind` 为 `summary { summary, sequence }`、`alert { alert, persisted }` 或 `idle { database_state, memory_pending }`。
- `request_control(path, request) -> Result<ControlResponse>` 只连接当前用户拥有的私有 Unix socket，验证对端 uid，并限制请求、响应大小及 I/O 超时。后台通过 `run_daemon(options) -> Result<()>` 启动；生产入口拒绝 root 身份，在打开数据库前完成检查，并只接受 root collector peer。`run_daemon_with_expected_collector_uid` 是隐藏的测试入口，允许真实本机 IPC 测试使用同 uid 合成采集器。
- 普通用户后台是唯一 `Storage` 写入者。采集帧经有界队列进入事件去重、目录范围过滤和规则分析；9 类事件分别提供 observed／persisted／filtered 计数。队列容量 64、去重键缓存 8,192、内存待通知最多 256；SQLite 错误设置保存缺口并按 5 秒冷却重试，不中断规则分析。
- `NotifyPoll` 对新的桌面通知会话先生成一条 outbox 快照汇总；按返回的 `sequence` 确认，不会误确认快照之后的告警。随后逐项投递实时告警，发送失败保留并冷却重试。`notify_once_with_sender(...)` 与 `notify_burst_with_sender(..., maximum) -> Result<usize>` 提供本机集成测试的 sender 注入；生产 `run_notify(control_socket)` 每轮最多连续发送 8 条，只有空闲时才短暂等待。系统通知使用固定 AppleScript 及 argv 文本参数，不把标题／正文拼入脚本。

### 验证与边界

- `tests/runtime_host.rs` 使用本机 Unix socket、合成 collector 与临时 SQLite，覆盖目录控制、50 文件批量告警、60 秒合并、历史 outbox 摘要和实时通知、三条告警突发连续发送、事件范围过滤、采集器重连／代际状态、心跳保留权限诊断，以及 SQLite 写锁期间继续分析和内存通知、解锁后的恢复写入。
- 2026-10-04 验证：`cargo fmt --all -- --check`、`cargo test --locked --all-targets`（50 项）及 `cargo clippy --locked --all-targets -- -D warnings` 通过；本次测试以 uid 501 运行，runtime_host 三项均实际执行。测试在进程为 root 时会主动跳过；本机 root／FDA 后台采集链、真实通知到屏和 3 秒端到端时延仍由 06 的真实运行验收给结论。发送器替身只证明宿主投递／反馈路径，不证明 macOS 展示通知。

- 2026-10-04审查补修：来源版本／结构化缺口与v3迁移、控制连接错误隔离、宿主启动及每小时明细清理、故障合并告警的持久通知恢复和已发送去重均已补齐；针对性组件证据见 [双轴审查](../code-review.md)。系统root／FDA与后台验收仍由06记录。
