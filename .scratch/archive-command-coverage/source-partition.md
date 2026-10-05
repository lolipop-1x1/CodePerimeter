# 来源延迟修复边界

日期：2026-10-05。任务 03 的实现笔记，不扩大已确认的 14 个名称或 3 秒目标。

## 实证

8 项真实诊断只有 3 项及时通过。bzip2 标准输出在主流接收耗时 7308ms、exec 旁路 680ms；pbzip2 创建为 8309ms／465ms。来源 UTC 与执行窗口吻合；主链路最终保存全部 8 个 exec、对应告警与发送回执，无序号缺口或丢弃。共用高频来源的积压是优先候选，不能称为已经修复。

## 最小修改

- 单一可信 collector、socket 和运行 ID 保留。eslogger 分为 `exec` 与其余八类 `activity` 两路；不重复订阅、不删事件，不增加用户配置。
- fork／exit 留在 activity 路，与文件事件保持该路顺序，避免快速 exit 提前清空文件关联状态。未知 PID 代次只在同路使用降级的观察实例，不跨路猜关联。
- 平台无关模型增加固定 `SourceStream::{Combined, Exec, Activity}`；旧 JSON／帧默认 Combined。ActivityEvent 与 SourceContext 携带 stream。EsloggerAdapter::new 保留兼容入口，新增 new_with_stream；只在各自客户端内核验序号，并拒绝订阅不匹配的已知事件。
- 宿主按固定 stream 建立有界 Adapter 表，规则仍按共同 run＋PID／代次关联。内存与 SQLite 的序号去重包含 stream，Combined 保留旧 key；SQLite schema 3 不迁移。健康保守汇总，两路都须有已观察来源版本；任一路停止则整体停止并回收自有来源。
- 每路独立有界队列，公平转发；两路来源终止及所有 early Err 均回收本次子进程和读取线程。管理员、FDA、PGID、对端 UID 与原始事件不落盘约束不变。
- 宿主增加有界内存 exec 处理回执，仅含 run、stream、PID／代次，供已有可信控制连接按精确身份查询；在规则／持久化处理之后生成，不持久化、无路径或参数。ControlRequest::ExecReceipt 返回匹配的可选回执，用于证明主 exec 路已处理本轮负例。
- 负例先确认精确主 exec 回执，stdin 再确认本次精确来源缺口已持久化，随后启动独立文件屏障；不跨路比较 global_seq。两路均健康、无本进程项目告警才能通过。缺少回执／缺口保持失败。
- 8 项诊断与默认 92 项继续分开，真实验收沿用来源发生时间计算，所有新组件证据不能替代真实通过。

序号分客户端的依据：[Apple global_seq_num](https://developer.apple.com/documentation/endpointsecurity/es_message_t/global_seq_num?language=objc)。

## 所有权

- 共享模型／适配／存储／规则及相关测试由 foundation 子任务负责。
- Python 验收器与自测由 validation 子任务负责。
- collector／宿主／相应运行测试、文档、整体验证与本地提交由主任务负责。均不得覆盖他人改动或推送；最终完整真实验收通过后再 push。

## 组件验证

Rust 1.88／locked 完整回归 118 项通过，另 1 项原有信号 helper 忽略、由监督用例调用；归档裁决器 45 项、MVP 裁决器 41 项、合成发送器 9 项通过。fmt、严格 all-target clippy、4 个脚本 AST、shell 与 diff 检查通过；最低支持版本 release／locked 构建通过。新增内容的真实 home 路径、个人标识与非示例邮箱扫描通过。组件与普通用户 IPC 用例未启动 ES，仍需新版 8 项诊断及完整 92 项真实链路裁决。
