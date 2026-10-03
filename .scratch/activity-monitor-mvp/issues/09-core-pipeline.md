# ES 标准事件到规则与证据的跨模块契约

Status: ready-for-agent
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
