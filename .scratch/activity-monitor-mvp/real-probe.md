# 本机短时采集探针

日期：2026-10-03
Status: claimed
Type: task

## 已取得的证据

- macOS 15.6.1（24G90）、arm64；系统 `/usr/bin/eslogger`，实见 `schema_version=1`、消息 `version=9`。
- 用户在终端完成管理员授权和完全磁盘访问后，运行独立合成发送器；发送器与采集器分属不同进程组。
- 4.3 秒短时采样中，7244 条全系统输入均为可解析 JSON；筛选保留 12 条合成目录相关脱敏记录。达到上限后主动停止采集（SIGTERM），不是源自行失败。
- 样本实见 `mmap.source` 的合成普通文件路径、`protection=1`、文件 stat、`close.target`、进程 `audit_token.pid/pidversion`、EXEC target 代际变化、RFC3339 纳秒源时间以及全局／分类序号。
- OPEN 实见 `fflag` 含 FREAD；临时探针脱敏 whitelist 误删 `open.file`，已经修正。完整 OPEN 路径口径仍待下一次完整链路验收，不能由不完整脱敏记录推断源缺字段。

## 未完成的验收

- 此样本经过筛选且有条数上限，不能用于连续流完整性或内核丢失判断。
- 尚未据此验证 Rust 分析、SQLite、归档关联、告警／通知 3 秒目标、后台运行、重启／未登录、性能或展示回执。
- 早期缺 FDA 时 `ES_NEW_CLIENT_RESULT_ERR_NOT_PERMITTED`、0 事件是采集未启动；不能报告无文件活动。
- 原始系统流未落盘，公开文档不包含真实项目路径、正文、完整参数或环境变量。
