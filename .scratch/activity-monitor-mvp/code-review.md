# 文件活动监控 MVP：固定点双轴审查与修复

日期：2026-10-04
Status: resolved
Type: task

固定比较点：`a8ecdf3e92aac93fca88734ac61c14ce8c9ac2a6`。审查HEAD：`725a263167d1fd9f65c93f4977477f17f032a82f`。以下保留原审查两轴的独立计数及修复证据。修复候选 `0c9a1c9442dc6d25339f01d2e1587aa30d0a9cca` 已通过两轴定点复核并合入 `a08bcc2f6269937e053392ca658bb4e6aa99d3a1`；两者tree均为 `bc683f579403766f5d4a80d7055ca0b90987ef2d`。本票仅关闭源码审查，PR此前已Ready；06仍claimed，实际系统验收待重跑，旧实现worktree归档受App保护，修复worktree暂保留。

## Standards

原审查：2项硬性违规，0项heuristic；最严重项为S1 [P2]。不与Spec轴合并计数。修复候选独立定点复核：S1／S2均通过，剩余硬性违规0、heuristic0，当前无最严重项。

| finding | 审查证据及要求 | 修复与针对性证据 |
| --- | --- | --- |
| S1 [P2] 来源版本和缺口结构丢失 | 审查HEAD的runtime:898仅保存issue.code和通用message；RuntimeStatus:124／ActivityEvent:60没有实际版本，storage:462仅序列化标准事件。AGENTS要求来源、版本与覆盖缺口，架构要求保留粒度、来源、版本及原因。 | 标准事件补可选来源schema/message版本，状态显示当前run实际版本；每run首次实见或版本变化写版本健康记录，结构化source保留run_id、field、missing_events和实际版本，detail保留静态解析原因。`health --source-run-id`回查，未知保持空。SQLite v3只新增health source_json，v1/v2在同一事务迁移；旧JSON缺版本仍可读。`source_versions_and_structured_gaps_are_queryable_by_run_after_restart`验证版本改变、unsupported schema、同版本不重复版本行、双序号缺口数和重启回查；v1/v2迁移测试验证旧事件／健康／通知／outbox与累计保留。raw、args/env/body均不持久化。 |
| S2 [P3] 已解决票据未同步地图 | 01–05／07–10为resolved，却未追加到map Decisions-so-far，违反issue-tracker的解决步骤。 | map逐组同步九张票据的完成结论与链接；Notes／Fog更新为实现已接通、系统字段探针部分证明、完整root／FDA／3秒／后台待验。06保持claimed；早期日期讨论和Comments保留历史口径。显式核对九个resolved票据和地图链接。 |

## Spec

原审查：4项可证缺陷，0项scope creep；最严重项为P1 [P1]。修复候选独立定点复核：P1–P4均通过，剩余可证缺陷0、scope creep0，当前无最严重项；复核另跑3项宿主定点测试与9项判定器自测通过。

| finding | 审查证据及规格要求 | 修复与针对性证据 |
| --- | --- | --- |
| P1 [P1] 查询客户端断开会结束宿主 | spec:13要求后台不依赖终端；runtime:640把响应写错误传播出daemon。原审查普通用户发送Status立即shutdown／close，实见exit1 Invalid argument。 | 控制连接的设置／读写错误局部结束，合法Stop仍生效；监听器本身错误维持原有处理。`disconnected_and_slow_control_clients_do_not_stop_host`实际建立Unix socket，发送Status／坏行后shutdown，再用正常Status及Stop证明宿主存活；不完整慢请求超时亦不结束宿主。 |
| P2 [P2] 30天保留没有宿主调用 | issues04:15承诺明细保留30天；storage:1010只被测试调用。原审查匿名库observed_timestamp_ms=1的健康记录重启仍可回查。 | 唯一普通用户宿主启动时与每小时调用prune_expired，失败显示retention_state与数据库降级，恢复重试；last_run可回查，累计不清零，过期明细记录追溯缺口。`host_prunes_at_start_and_periodically_preserves_statistics_and_reports_failure`验证启动清理、缩短测试周期的定期调用、匿名DELETE故障状态、恢复清理及累计保留。 |
| P3 [P2] 故障合并告警丢失登录补发 | spec:23要求进入桌面汇总补发；runtime:1062合并替换使首次is_new丢失，DB恢复不建outbox。原审查写锁下threshold2／3个不同文件产生alerts1／outbox0／memory_pending1，重启丢通知。 | deferred合并保留尚未保存的首次待通知事实；恢复成功后未发送告警交由持久outbox，移除对应内存投递；实时已发送事实抑制恢复排队并写Sent反馈。`merged_alert_recovers_outbox_across_restart_and_sent_alert_does_not_repeat`在真实匿名SQLite写锁中触发及合并，分别验证无notify时恢复＋重启汇总一次、故障时已发送恢复／重启不重发。保留原有通知接受后、Sent落库前崩溃可能重复的跨系统事务边界。 |
| P4 [P2] tar／zip缺归档输出仍可通过 | spec:50要求输出线索，validate-mvp:252仅检查archive_command。原判定器无任何输出事件仍pass。 | tar／zip项目内与临时输出独立要求发送器声明的预期路径、实际create/write/rename事件、同run/PID代际的实际项目读取及项目告警时间证据；外部命令先创建输出时，由实际EXEC的输出参数与项目ArchiveCommand补充关联；路径别名规范化。告警evidence_paths为有限样本，不代替实际输出事件。判定器新增四变体正例／缺输出反例、无关联、未声明、错误项目和PID代际反例；另验先创建输出正例及无输出路径样本时仍须实际输出的正反例，missing_evidence明确失败原因。 |

README和后台验收步骤补齐install后的显式service start；install只写安装文件，不bootstrap。当前规格和技术状态同步实现与待验边界，原型50／10秒与60秒口径保留。

## Answer

两轴原始2＋4项均已修复，独立定点复核各剩余0项可证缺陷，复核比较的是上述固定来源／候选tree。Standards与Spec结论分别保留，不将其中一轴或组件通过扩展成系统通过。主agent已构建release并复制至集成target/release，二进制SHA256为 `6c07108515587f5c9b78e46b578105ce23f9680f366948934f43b8c0e5927b13`；已向用户提供真实终端匿名运行步骤，随后首跑实际失败；结果与最小启动补修见下方记录。

## 验证与未完成事项

最终检查（普通用户uid501）：Cargo fmt、check全部目标、严格clippy全部目标与git diff --check通过；Rust全部目标69 passed／1 ignored helper（由监督信号用例显式调用）；Python发送器8 passed、判定器9 passed；3个validate脚本AST与两个验收入口--help通过。runtime_host的7项真实本地IPC测试和rules_storage的14项测试均实际执行。CLI既有过滤器测试补证source-run-id参数经IPC传递。

上述均为匿名组件、真实本地IPC和合成操作／判定器证据，不是实际ES防护验收。上述组件阶段未运行root准备／安装、未改FDA、未读取真实历史；随后由用户终端运行的首跑结果见下方；完整Rust root／FDA链、3秒生成／发送、通知到屏、launchd未登录／注销／重启、性能与持续完整性仍待06。未执行外传控制，也不报告控制成功。


## CI 工具链基线修正

2026-10-04远端 Component checks 首轮 `37142828933` 失败。主agent读取日志确认，浮动stable实际安装Rust 1.99.0，其新增的 `collapsible_if`／`function-casts-as-integer` 共11条lint被 `-D warnings` 阻断；源码编译不是本轮失败点。本机组件与release的已验证工具链为Rust 1.88.0，Cargo rust-version为1.88。

仅在workflow的工具链输入固定 `1.88.0`，fmt／严格clippy／tests和其他步骤保持；README与验收文档同步基线。源码、脚本、测试、Cargo文件均不改，原release SHA256 `6c07108515587f5c9b78e46b578105ce23f9680f366948934f43b8c0e5927b13` 仍有效。已做只读YAML解析、工具链与Cargo版本核对、原检查步骤保持及diff检查；远端重跑尚待结果，不能记为CI已通过。此配置修正不改变原Standards2／Spec4项的修复和两轴各0剩余结论，系统验收仍由06记录。


## 真实首跑启动失败补修

2026-10-04本轮已实际运行完整匿名入口，证据 `/private/tmp/codeperimeter-validation-zxf_c66m/summary.json`／`failure-or-final-evidence.json` 显示run_id为空、九类及筛选事件全0、collector reconnecting、root_cleanup_complete为true。核对代码与实际root:daemon 0775系统目录，旧固定端点的 `/var/run` canonicalize至 `/private/var/run` 后触发严格root路径拒绝，建socket前无法启动；原stderr未保存。这个启动缺陷是首跑新增证据，不改变此前固定点Standards2／Spec4的统计与已复核来源tree，各原finding仍已修复；不把旧复核的各0剩余扩展成新提交已完成独立复核或系统通过。

最小补修统一生产／验证端点为 `/Library/CodePerimeter/<uid>/run/collector.sock`，核验既有安装root全链后创建专用run；不弱化可写路径拒绝、不改系统目录。真实入口只保留最多8种静态启动stderr诊断码及退出码，提前退出立即失败，原文和全系统事件不落盘。已有验收副本必须显式指定旧SHA256，root发布时用固定可信隔离系统Python重新核验完整root路径／ACL、旧新hash、无新旧端点／控制端点、无codeperimeter进程以及本UID无plist／loaded jobs，才原子替换；默认仍拒绝覆盖，不安装job或删除未知端点。

先前CI pin head `2e18ef6` 的远端push／PR两条checks已SUCCESS（run `37144069732`／`37144066116`）；启动补修新head CI待跑。十个旧实现worktree的归档被App的pinned task/workspace保护拒绝，保留workspace；没有手工删除或修改固定状态，不报告清理全部完成。06仍claimed，修复版真实root/FDA、九类、3秒、通知到屏／后台与性能仍待用户终端重跑。

本次启动补修定点检查（普通用户uid501）：fmt --check、全部target check／严格clippy、git diff --check通过；Rust service_collector 11＋end_to_end 2通过；Python判定器／启动／安全替换17通过。3个validate脚本AST、两个入口help、固定系统Python root全链／ACL与 -I -B -S标准库可用性通过。root发布门禁测试仅在普通用户匿名目录替身中验证拒绝及原子替换，不代表管理员准备或ES系统通过。源码范围仅service固定路径；没有重复此前全套69项Rust，也没有执行sudo／FDA／安装／真实历史。


## 启动补修定点复核完成

2026-10-04候选 `54bbb0d2a9b29d82cbb795b3203367b0e3402fa5` 已完成两轴独立定点复核；来源父提交 `2e18ef6ce75ded164ee84489e93fce0c2a79a023`，候选tree为 `08c1bb432a447cc44cc56162e604d1b610ebe94f`。Standards：hard0／heuristic0；Spec：可证缺陷0，独立新增8个反例均通过。此结论限定在启动补修候选，不覆盖实际系统运行，原固定点Standards2／Spec4统计保持。

主agent已对该候选成功offline release构建，产物 `/private/tmp/codeperimeter-integration-release/release/codeperimeter` 的SHA256为 `987018ac8168f6ad6170d520637a574a8dc470eb49e3c3bdaafc43b33b44e17a`；service plan回读确认三角色使用同一新collector socket。此时尚未复制集成target，root副本 `/Library/CodePerimeter/501/codeperimeter` 仍为旧SHA256 `6c07108515587f5c9b78e46b578105ce23f9680f366948934f43b8c0e5927b13`，未执行替换或新源采集。新head远端CI、root／FDA、九类、3秒、到屏和后台仍待，06保持claimed。

PR此前已Ready，不再列为未完成项。十个旧实现worktree归档均被App的pinned task/workspace保护拒绝，未手工删除或修改固定状态；当前修复worktree暂保留供后续实测问题处理，后续清理仍按App正常归档流程，不报告全部清理完成。本次收尾仅更新本记录与06，diff检查通过，不修改源码／脚本／测试，也不重复组件测试。
