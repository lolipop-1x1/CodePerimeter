# 活动关联、批量／归档规则与 SQLite

Status: ready-for-agent
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
