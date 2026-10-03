# eslogger 事件接入与覆盖状态

Status: resolved
Type: task
Blocked by: 01

## Scope

所有权：`src/eslogger.rs`、`tests/fixtures/eslogger/`、`tests/eslogger_adapter.rs`。使用 `src/model.rs`；必要模型变更通过主 agent 协调。

## Acceptance

- 解析真实 eslogger JSONL 的 OPEN／MMAP／EXEC／FORK／EXIT／CREATE／WRITE／CLOSE／RENAME，保留身份、时间、序号、截断／缺字段。
- OPEN 只按 FREAD，MMAP 只按 PROT_READ 标可读；不能以事件名冒充读取。
- 只临时解析 tar／zip 等参数，输出关联路径，原始 args／env 不进入标准模型、数据库或 fixtures。
- 坏行、未知事件、关键字段缺失、全局或分类序号间隙产生可见覆盖状态；序号不存在时说明未知。
- 用匿名格式 fixtures 验证 PID 代际、路径、读写差别、归档参数与 malformed 输入；真实采集仍独立验收。

## Comments

依据 [技术基线](../technical-design.md) 与 [spec](../spec.md)。不要安装服务或扩大真实数据持久化。

## Answer

- 实现 `EsloggerAdapter::new(source_run_id: impl Into<String>)`、`parse_line(&mut self, line: &str, received_timestamp_ms: i64) -> ParseOutcome`、`health(&self) -> &AdapterHealth`；采集重启创建新适配器和新来源运行 ID。另提供无状态 `parse_line(line: &str, source_run_id: &str, received_timestamp_ms: i64) -> ParseOutcome`。
- `ParseOutcome` 包含 `event: Option<ActivityEvent>`、`issues: Vec<CoverageIssue>`、schema／message version、event_type、原始 global／分类序号；`CoverageIssue` 只包含静态 code／field／message 与可选缺失数量，不返回原始行或 JSON 错误中的值。宿主应在目录筛选前输入每一行并保存／展示 issues；合成样本的全局与分类间隙不能相加当成实际丢失数量。
- 支持 schema 1 的九类 NOTIFY。OPEN 使用 FREAD，MMAP 使用 protection 的 PROT_READ；返回未知状态而非补造值，存在拒绝结果时不会标为可读。EXEC 使用 target 的执行后身份，FORK 使用 child 的身份与其父进程。CREATE／RENAME 解析现有目标和新路径；截断目标不用于完整重命名关联。
- `MAX_LINE_BYTES` 为 1 MiB，宿主读流也必须采用该上限，避免先无界读入后才调用解析器；`SUBSCRIBED_EVENTS` 提供九类订阅名称。序号状态只维护订阅类型，未知事件仍推进全局序号并报告缺口。重复／回退报告覆盖异常，标准事件仍保留供宿主判断，不能仅靠序号异常行宣称完整统计。
- tar／bsdtar 支持常见创建、追加、更新、`-f`／`-C`、压缩选项与路径；zip 支持常见创建选项。参数仅瞬时解析，标准模型只输出工具、输入／输出路径和 cwd；提取失败明确报告 `archive_arguments_incomplete`。提取列表、密码、未知含值选项等不猜路径；tar 列表／解包不产生归档命令迹象。未承诺识别所有归档工具和复杂选项。
- 验证：10 个匿名集成测试通过，涵盖九类事件、读写标志、代际、目录和文件身份、缺字段、截断、序号间隙／回退／重启、坏行、未知 schema／AUTH、归档参数与敏感字段不持久化；`cargo clippy --locked --all-targets -- -D warnings` 通过。
- 来源：Apple 本机 macOS 11.1 SDK ESMessage／ESTypes／fcntl 头文件与 [esl schema 字段参考](https://github.com/tstromberg/esl/blob/4d890d24a9aad6e7c848d6ce9f39c73d111004ab/pkg/eslogger/structs.go)，核查日期 2026-10-03。本机 macOS 15.6.1 探针实见 schema 1／message version 9 的 mmap、close、EXEC 字段一致；探针当时错误脱敏删除 OPEN.file，正在由集成任务补取。九类真实采集、完整流序号、后台 FDA、3 秒和开销验收继续由票据 06 记录，组件测试不替代运行证据。
