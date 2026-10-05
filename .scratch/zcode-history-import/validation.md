# ZCode 历史目录适配验收

日期：2026-10-05

## 支持基线与结果

本轮按 [最小规格](spec.md)实现目录发现、预览与导入。来源标签为 `zcode`；默认数据库为 `~/.zcode/cli/db/db.sqlite`，可用 `--zcode-db FILE` 指定其他位置。

- 本机 ZCode 3.8.1，build 3.8.1.5310；`session` 是普通表，`directory` 为非生成 TEXT 字段。
- Rust 1.88.0；沿用 Cargo.lock 与已有 rusqlite 依赖，不增加依赖。
- 普通用户只读预览读取 9 条会话目录记录，归并为 4 个可用候选；出处均为 `session.directory`，无伪造行号和应用版本。
- 预览包含 `directory_changes_not_validated` 缺口。没有导入真实候选、运行 ZCode 任务、安装后台服务或管理员采集。
- 原始本机预览仅在已有忽略的本地验收目录保存，目录权限 0700、文件权限 0600；公开记录只保留匿名摘要。

## 组件与用户流程

| 验证 | 实际结果 |
| --- | --- |
| 目录组件 | `cargo test --locked --test history_directories --test zcode_history`，19 项通过，其中 ZCode 新增 11 项 |
| 完整 Rust 回归 | `cargo test --locked`，92 项通过、1 项忽略 |
| CLI 与宿主 | 真实普通用户 CLI → 本机 IPC → 独立 SQLite：三来源同目录去重，重复导入稳定，重启后来源与配置保留 |
| 范围稳定 | 预览之后新增 Codex 与 ZCode 会话目录，导入仍只使用旧快照 |
| WAL 与只读 | 已提交且未 checkpoint 的合成记录可见，发现前后主库／WAL 字节不变，原连接仍能追加会话 |
| 隐私与异常 | 合成标题、正文、会话 ID、`path`、`version` 不进入报告；缺表／字段、视图／生成目录、非文本／过长值、失效路径和锁超时有明确结果 |
| 既有快照 | 既有 v1 来源快照仍可反序列化和导入；旧程序不承诺读取新增来源／缺口 |
| 静态检查 | `cargo fmt --check`、`cargo clippy --locked --all-targets -- -D warnings`、`git diff --check` 通过 |
| 可运行产物 | `cargo build --release --locked` 通过，release CLI 帮助显示 `--zcode-db FILE` |

测试宿主使用匿名项目与独立数据库，采集源刻意不可用；这些结果不是 Endpoint Security 或通知到屏验收。沙箱内本地 IPC 首跑因系统权限限制退出；正常本机权限下相关测试及完整回归通过。

SQLite 采用正常只读事务，schema 与目录行处于同一快照，锁等待上限 250 ms；不对原库执行迁移、写入或主动 checkpoint。读取 WAL 的依据见 [SQLite 官方说明](https://www.sqlite.org/wal.html)。

## 未验证项

- 会话中途所有目录变化、远程工作区执行主机与专属运行时进程归属。
- 其他 ZCode 版本、不同 schema 与完整历史内容分析。
- 真实 ZCode 任务的文件活动、打包、通知和外传控制；本轮目录导入不新增这些能力的验收结论。
- 独立双轴源码审查与最终主干交付尚待完成。
