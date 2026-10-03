# Codex／Claude Code 目录元信息适配

Status: resolved
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

## Answer

- `history::discover(&HistoryOptions) -> DiscoveryReport`：`codex_home` 指配置根目录，扫描 `sessions` 与 `archived_sessions` 的 `.jsonl`／`.jsonl.zst`；`claude_home` 指 CLI 配置根目录，递归扫描 `projects` 中 `.jsonl`。任一来源设为 `None` 可只扫描另一来源。
- `HistoryOptions::defaults(home: &Path) -> HistoryOptions` 提供 `~/.codex`／`~/.claude` 默认值；环境变量或 CLI 显式根目录由调用者设置，不借用历史目录名猜路径。
- `history::manual_directories(paths: &[PathBuf]) -> DiscoveryReport`：只接受可解释的绝对目录，现存目录 canonical 去重；保留原始路径、父子目录和来源。相对、失效、文件及权限错误分别返回状态，不扩展到共同父目录。
- `DirectoryCandidate` 的 `canonical_path`＋`Available` 可供用户预览后导入；`raw_paths`、`origins` 保留来源。相同文件／元字段／版本的来源合并 `occurrences`，`line` 保留首次位置，避免按聊天条数堆积来源。
- Codex 提取头部 `cwd`／`runtime_workspace_roots` 与每轮 `cwd`／`workspace_roots`，沿用头部 `cli_version`；Claude Code 提取顶层 `cwd`／`version`。serde 跳过非元字段，报告不保存原始 JSON、会话 ID、正文或工具参数。
- `DiscoveryReport` 包含 `counts`、按文件与原因合并的 `gaps`、实见 `versions`。所有 `compatibility_validated` 均为 `false`：这版验证明确字段形状，尚无整版历史兼容认证；未知记录、缺字段、无版本、损坏／超长／未完成追加、缺失来源、IO 和符号链接跳过分别可见。
- 行缓冲最多保留 1 MiB；超长行丢弃到换行后继续。zstd 解码窗口上限为 16 MiB，超限记录读取缺口。末尾尚未写完的 JSON 计为 `incomplete_records`，不隐藏前面的有效元信息。历史文件符号链接不跟随，目录候选自身允许规范化符号链接。
- 合成测试覆盖两格式、切目录、压缩归档、正文不进入报告、重复来源、坏行／未知记录／超长／部分追加、路径状态、目录别名、缺失来源与权限异常。fixtures 中版本值为合成场景元信息，不能当作对应客户端的真实任务验收。
- 字段参考：[Codex 官方协议源码](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs#L3078-L3093)、[Claude Code CLI 存档说明](https://code.claude.com/docs/en/sessions)。范围不包括 Claude Desktop／VS Code／云会话；递归读取 CLI 根目录中可识别字段，不保证未来版本元信息位置不变。
