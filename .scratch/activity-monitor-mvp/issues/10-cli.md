# CLI 配置、查询与服务管理入口

Status: claimed
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

## Answer

### CLI 与宿主契约

- 所有普通配置、查询、历史导入和累计统计清除通过 runtime::request_control 使用宿主控制 socket；默认地址为用户 Library/Application Support/CodePerimeter/host.sock，可由全局 --host-socket PATH 覆盖。CLI 不打开 SQLite。
- watch add 支持一次传入多个目录；watch remove/list 分别移除或查询配置。手动加入时发送规范绝对路径和 manual 来源。
- history preview 将发现结果保存为版本化 JSON 快照，含稳定数组序号、目录与来源元数据、版本及发现缺口，不包含会话正文。history import --preview FILE --index ... 或 --all-available 只按该快照选择；只允许快照标记为 available 的目录，并在导入前确认路径仍指向同一规范目录。路径变化或失效时整次导入失败，不重扫新历史、不按新候选重新解释序号。
- events、alerts、health、notifications、status 和 stats show 构造对应 ControlRequest。stats clear-cumulative 发送 ClearCumulativeStats，不直接改库、不清除明细。
- 服务操作 argv 沿用 07 的 collector、daemon、notify 角色参数。daemon 可显式调整批量阈值和窗口，默认 50 个文件／10000 毫秒；ServicePlan 不传可选参数时使用默认值。service plan --user NAME 输出完整 ServicePlan，供安装前核对安装源、UID、路径、argv 与 plist。install/start/stop/uninstall 逐项检查 OperationReport.steps.success，任一步失败即命令失败。
- daemon 调用普通用户 runtime；collector 调用 07 collector API；notify 调用通知运行器。不存在合成采集器或伪通知命令行开关。

### 验证与边界

- tests/runtime_cli.rs 通过实际 codeperimeter 二进制检查参数解析、失败退出、IPC 请求字段、多个目录、查询过滤、累计清除、历史快照固定选择及失效路径拒绝。测试 socket 是同 UID 本机 IPC 替身，只验证 CLI 契约，不代表真实采集或通知通过。
- service plan 测试核对三个 launchd job 的 argv 与对应 socket／数据库路径；帮助测试核对角色 CLI 参数名。src/main.rs 单元测试验证 OperationReport 有失败步骤时返回错误，不执行 launchd 安装或启动。
- 真实 macOS root collector、FDA、后台运行、通知到屏、3 秒目标和重启恢复由票据 06 单独验收；本文档不将 CLI IPC 测试称为这些能力的证据。
- 2026-10-04：cargo fmt --all -- --check 通过；cargo test --offline --locked --all-targets 为 57 passed、0 failed。cargo clippy --offline --locked --all-targets 可完成，当前只报告同期 05 runtime.rs 的 3 个 lint；全量 -D warnings 待 05 owner 修复后复验。CLI 本身未产生 clippy 诊断。
