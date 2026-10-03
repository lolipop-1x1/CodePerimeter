# ES 标准事件到规则与证据的跨模块契约

Status: resolved
Type: task
Blocked by: 02, 04

## Scope

从 06 拆出只依赖已完成模块的合成输入契约验证。所有权：`tests/core_pipeline.rs`、本票据。不要修改实现、Cargo、README或其他测试；发现实际问题报告主 agent 协调。

## Acceptance

- 少量必要用例真正串联 ES 原生形状 JSON → adapter → RuleEngine → Storage →查询/待通知，不手工绕过adapter构造 ActivityEvent，不重复各模块单元测试。
- 证明读打开／可读映射去重、50／10秒门槛、告警60秒合并与SQLite/outbox一致；范围外或非读活动不伪造不同源码读取。
- 证明外部归档相关路径/输出跨模块语义正确，最终持久化不含原始args/env/未知正文标记；保持操作证据与压缩内容判定区别。
- 输入为匿名合成 JSON，格式参考本机 schema1/message9 与 Apple SDK；不将此测试声明为真实ES、后台、通知展示或3秒本机验收。
- 两到三个必要跨模块场景足够，fmt/clippy/tests通过，票据Answer记录证据与未验收项。

## Answer

新增 `tests/core_pipeline.rs`，从匿名 schema 1／message 9 的原生形状 JSON 经 `EsloggerAdapter::parse_line`、默认 `RuleEngine`、`Storage` 写入，最终查询 SQLite 和 outbox。没有手工构造 `ActivityEvent`，没有修改实现或依赖。

三个契约场景：

- 可读 OPEN 与 MMAP 按 dev／ino 去重；两个保护根合计 49 个不同文件时不告警，第 50 个立即产生 `BulkFileAccess`。同文件两次访问保留访问频次；只写 OPEN、不可读 MMAP 与前缀相似的目录外路径不补造读取数。默认 10 秒窗口中的第二批读取在 60 秒内更新原告警和原 outbox，超过合并窗口产生新告警与第二项 outbox；查询中的不可读事件仍准确保留 `readable=false`。
- 来源时间跨过 10 秒时，旧的 49 个文件过期，即使接收时间相邻也不累计告警；新的窗口达到 50 才触发。告警首末时间使用来源时间，outbox 创建时间使用接收时间，持久化和待通知查询一致。
- 原生 tar EXEC 用 target 的 PID 代际和 `-C`／相对输出解析出两个项目输入与临时目录输出；EXEC 前的另一代际读取不混入。后续同代际 OPEN／MMAP 与目录外 WRITE 关联为 `ArchiveOutput`，追加 WRITE 更新证据并保留两项不同规则的通知队列；另一进程只有同路径输出不产生关联。关闭并重开 SQLite 后查询路径、归档类型、进程身份和数量一致；事件／告警／outbox DTO 和数据库字节均不含完整参数、环境变量与未知正文标记。

验证（2026-10-03）：

- `cargo test --test core_pipeline`：3 passed，0 failed。
- `cargo fmt --check`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过。

来源：字段与接口沿用 [02 Answer](02-eslogger-adapter.md#answer)、[04 Answer](04-rules-sqlite.md#answer)；输入参考本机 schema 1／message 9 形状及 02 的 Apple SDK 字段依据，使用匿名进程和临时合成文件。测试只验证文件操作证据与模块契约，没有执行 tar 压缩，也不证明读了多少正文或发生压缩运算。真实 ES 采集、FDA／launchd、3 秒告警／通知发送和桌面展示验收仍归 06。
