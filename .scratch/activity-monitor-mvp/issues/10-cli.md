# CLI 配置、查询与服务管理入口

Status: ready-for-agent
Type: task
Blocked by: 01、05 的公开控制契约

## Scope

从 05 拆出独立入口实现；05 负责普通用户宿主、控制 IPC 与通知运行。所有权：`src/main.rs`、`docs/usage.md`、`tests/runtime_cli.rs`。使用 05 的公开 API，不重复实现规则、Storage 写入或后台运行。

## Acceptance

- CLI 通过宿主控制接口完成手动多目录、历史候选快照预览／选择／批量导入、事件／告警／健康／通知查询与统计；新历史不能改变已经预览选择的范围。
- 三个后台角色 argv 与 ServicePlan 完全一致；配置与查询使用普通权限。root 仅执行 collector／服务管理，失败的 OperationReport 步骤必须使命令失败。
- JSON 输出可供验收脚本使用；不回显 raw JSON、历史正文或完整命令行，不直接另开 SQLite 写入路径。
- CLI 参数、失败退出、历史快照导入与必要请求字段有实际程序入口验证；IPC 替身明确标注，不称为真实采集或通知达标。
- 中文使用说明包含构建、后台管理、目录选择、回查、权限与覆盖边界，最终真实运行证据由 06 记录。
