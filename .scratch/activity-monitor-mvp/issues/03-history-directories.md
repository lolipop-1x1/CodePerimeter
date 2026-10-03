# Codex／Claude Code 目录元信息适配

Status: ready-for-agent
Type: task
Blocked by: 01

## Scope

所有权：`src/history.rs`、`tests/fixtures/history/`、`tests/history_directories.rs`。模块自有可序列化候选／来源／扫描报告类型，供 CLI 调用，不读写 SQLite。

## Acceptance

- 分别适配 Codex session_meta／turn_context 的 cwd／workspace roots 和 Claude Code 顶层 cwd，支持活动／归档及 Codex jsonl.zst。
- 默认来源及显式来源根目录可用；只提取目录／来源／版本元信息，不保存对话正文或原始历史。
- 保留 missing／unresolved／malformed／unsupported 计数，路径存在时规范化去重，保留来源，不能反解 Claude 文件夹名或扩大父目录范围。
- 公开测试仅合成数据；覆盖同会话切目录、压缩、重复、旧／未知字段、超长或部分追加记录、失效路径和文件权限异常。

## Comments

依据 [技术基线](../technical-design.md) 的历史字段核查；手动目录通过相同规范化入口，但不依赖 Agent 格式。
