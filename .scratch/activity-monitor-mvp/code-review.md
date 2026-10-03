# 文件活动监控 MVP：固定点双轴审查与修复

日期：2026-10-04
Status: claimed
Type: task

固定比较点：`a8ecdf3e92aac93fca88734ac61c14ce8c9ac2a6`。审查HEAD：`725a263167d1fd9f65c93f4977477f17f032a82f`。以下保留独立审查返回的两轴计数及可证缺陷，修复候选在 `codex/activity-monitor-mvp-review-fixes`。主agent仍需对候选定点复核、合并、PR ready和清理；06保持claimed。

## Standards

原审查：2项硬性违规，0项heuristic；最严重项为S1 [P2]。不与Spec轴合并计数。

| finding | 审查证据及要求 | 修复与针对性证据 |
| --- | --- | --- |
| S1 [P2] 来源版本和缺口结构丢失 | 审查HEAD的runtime:898仅保存issue.code和通用message；RuntimeStatus:124／ActivityEvent:60没有实际版本，storage:462仅序列化标准事件。AGENTS要求来源、版本与覆盖缺口，架构要求保留粒度、来源、版本及原因。 | 标准事件补可选来源schema/message版本，状态显示当前run实际版本；每run首次实见或版本变化写版本健康记录，结构化source保留run_id、field、missing_events和实际版本，detail保留静态解析原因。`health --source-run-id`回查，未知保持空。SQLite v3只新增health source_json，v1/v2在同一事务迁移；旧JSON缺版本仍可读。`source_versions_and_structured_gaps_are_queryable_by_run_after_restart`验证版本改变、unsupported schema、同版本不重复版本行、双序号缺口数和重启回查；v1/v2迁移测试验证旧事件／健康／通知／outbox与累计保留。raw、args/env/body均不持久化。 |
| S2 [P3] 已解决票据未同步地图 | 01–05／07–10为resolved，却未追加到map Decisions-so-far，违反issue-tracker的解决步骤。 | map逐组同步九张票据的完成结论与链接；Notes／Fog更新为实现已接通、系统字段探针部分证明、完整root／FDA／3秒／后台待验。06保持claimed；早期日期讨论和Comments保留历史口径。显式核对九个resolved票据和地图链接。 |

## Spec

原审查：4项可证缺陷，0项scope creep；最严重项为P1 [P1]。

| finding | 审查证据及规格要求 | 修复与针对性证据 |
| --- | --- | --- |
| P1 [P1] 查询客户端断开会结束宿主 | spec:13要求后台不依赖终端；runtime:640把响应写错误传播出daemon。原审查普通用户发送Status立即shutdown／close，实见exit1 Invalid argument。 | 控制连接的设置／读写错误局部结束，合法Stop仍生效；监听器本身错误维持原有处理。`disconnected_and_slow_control_clients_do_not_stop_host`实际建立Unix socket，发送Status／坏行后shutdown，再用正常Status及Stop证明宿主存活；不完整慢请求超时亦不结束宿主。 |
| P2 [P2] 30天保留没有宿主调用 | issues04:15承诺明细保留30天；storage:1010只被测试调用。原审查匿名库observed_timestamp_ms=1的健康记录重启仍可回查。 | 唯一普通用户宿主启动时与每小时调用prune_expired，失败显示retention_state与数据库降级，恢复重试；last_run可回查，累计不清零，过期明细记录追溯缺口。`host_prunes_at_start_and_periodically_preserves_statistics_and_reports_failure`验证启动清理、缩短测试周期的定期调用、匿名DELETE故障状态、恢复清理及累计保留。 |
| P3 [P2] 故障合并告警丢失登录补发 | spec:23要求进入桌面汇总补发；runtime:1062合并替换使首次is_new丢失，DB恢复不建outbox。原审查写锁下threshold2／3个不同文件产生alerts1／outbox0／memory_pending1，重启丢通知。 | deferred合并保留尚未保存的首次待通知事实；恢复成功后未发送告警交由持久outbox，移除对应内存投递；实时已发送事实抑制恢复排队并写Sent反馈。`merged_alert_recovers_outbox_across_restart_and_sent_alert_does_not_repeat`在真实匿名SQLite写锁中触发及合并，分别验证无notify时恢复＋重启汇总一次、故障时已发送恢复／重启不重发。保留原有通知接受后、Sent落库前崩溃可能重复的跨系统事务边界。 |
| P4 [P2] tar／zip缺归档输出仍可通过 | spec:50要求输出线索，validate-mvp:252仅检查archive_command。原判定器无任何输出事件仍pass。 | tar／zip项目内与临时输出独立要求发送器声明的预期路径、实际create/write/rename事件、同run/PID代际的实际项目读取及项目告警时间证据；外部命令先创建输出时，由实际EXEC的输出参数与项目ArchiveCommand补充关联；路径别名规范化。告警evidence_paths为有限样本，不代替实际输出事件。判定器新增四变体正例／缺输出反例、无关联、未声明、错误项目和PID代际反例；另验先创建输出正例及无输出路径样本时仍须实际输出的正反例，missing_evidence明确失败原因。 |

README和后台验收步骤补齐install后的显式service start；install只写安装文件，不bootstrap。当前规格和技术状态同步实现与待验边界，原型50／10秒与60秒口径保留。

## 验证与未完成事项

最终检查（普通用户uid501）：Cargo fmt、check全部目标、严格clippy全部目标与git diff --check通过；Rust全部目标69 passed／1 ignored helper（由监督信号用例显式调用）；Python发送器8 passed、判定器9 passed；3个validate脚本AST与两个验收入口--help通过。runtime_host的7项真实本地IPC测试和rules_storage的14项测试均实际执行。CLI既有过滤器测试补证source-run-id参数经IPC传递。

上述均为匿名组件、真实本地IPC和合成操作／判定器证据，不是实际ES防护验收。未运行root准备／安装、未改FDA、未读取真实历史；完整Rust root／FDA链、3秒生成／发送、通知到屏、launchd未登录／注销／重启、性能与持续完整性仍待06。未执行外传控制，也不报告控制成功。
