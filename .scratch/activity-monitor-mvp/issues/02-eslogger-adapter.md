# eslogger 事件接入与覆盖状态

Status: ready-for-agent
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
