# CodePerimeter · 代码门卫

macOS 本机项目文件活动观察工具。当前 MVP 用 Rust CLI 配置多个目录，记录文件打开／可读映射及来源进程，发现批量访问、外部归档命令和关联的归档输出，保存 SQLite 证据并发送系统通知。

文件打开／映射是访问证据，不能证明读完了文件。批量读取不能确认程序在内存中压缩，归档迹象也不能证明源码已经外传。外传拦截属于后续产品目标。

## 当前状态

CLI、窄事件适配、规则、SQLite、历史目录发现、权限分离和 launchd 管理已实现。组件测试和匿名用户流程通过；已有系统 eslogger 的真实字段探针。完整 Rust 采集链路、3 秒目标、桌面通知到屏、后台 FDA、注销和重启仍需按 [MVP 验收](docs/validation/activity-monitor-mvp.md) 实测，不能用测试或配置文件代替。

系统采集使用 macOS 自带 `/usr/bin/eslogger`，需要管理员权限与责任进程的完全磁盘访问。root 仅运行采集／转发；分析、SQLite 和通知使用普通用户。原始全系统 JSON、完整命令行、环境变量和文件正文不落盘。当前订阅 NOTIFY 事件，支持本机观察，不提供压缩或网络拦截。

## 构建与运行

需要 macOS、Rust 1.88 或以上；验收脚本另需 Python 3、系统 tar／zip。CI 固定 Rust 1.88.0，与 Cargo rust-version 和已验证基线一致，保留 fmt、严格 clippy 与测试检查。

```sh
cargo build --release --locked
./target/release/codeperimeter --help
./target/release/codeperimeter service plan --user "$(id -un)"
```

服务计划显示三角色、安装路径和数据位置。核对后显式安装并启动；install 只写入安装文件，start 才启用并加载服务，随后检查实际采集授权和运行状态：

```sh
sudo ./target/release/codeperimeter service install --user "$(id -un)"
sudo ./target/release/codeperimeter service start --user "$(id -un)"
./target/release/codeperimeter status
./target/release/codeperimeter watch add /absolute/path/project-a /absolute/path/project-b
./target/release/codeperimeter events --limit 50
./target/release/codeperimeter alerts --limit 50
./target/release/codeperimeter health --limit 100
./target/release/codeperimeter health --source-run-id "实际run_id" --limit 100
```

首次历史导入先生成快照，再选其中的目录。Codex 与 Claude Code 的元信息分别适配，新会话不会自动扩大范围：

```sh
./target/release/codeperimeter history preview --output candidates.json
./target/release/codeperimeter history import --preview candidates.json --index 0
./target/release/codeperimeter watch list
```

完整参数、查询、阈值、停止和卸载见 [CLI 使用说明](docs/usage.md)；三角色、root 二进制、FDA 边界见 [后台服务](docs/service.md)。停止／卸载保留数据库和目录配置。

## 验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -B -m unittest discover -s tests -p test_synthetic_sender.py -v
python3 -B scripts/validate-selftest.py
```

真实完整验收使用新建的匿名项目和独立普通用户数据库，不导入真实历史或触碰既有服务配置。先阅读 [运行步骤与证据口径](docs/validation/activity-monitor-mvp.md)，在自己的终端授权后显式运行：

```sh
sudo -v
python3 -B scripts/validate-prepare-collector.py --binary target/release/codeperimeter
python3 -B scripts/validate-mvp.py --binary target/release/codeperimeter
```

准备入口仅复制本账户的 root-owned 二进制，默认拒绝已有目标；无 launchd 安装且没有活动端点／进程的验收副本可按文档用 `--replace-sha256 <明确旧hash>` 安全替换。它不安装 launchd、不改变 FDA。完整入口保存本地 `summary.json` 路径，真实采集失败会退出并记录原因，不回退 fixture。执行现有服务停止、替换版本和 FDA 授权前，按验证文档检查当前状态。

## 项目文档

| 文档 | 用途 |
| --- | --- |
| [产品上下文](CONTEXT.md) | 已确认目标、范围和术语 |
| [MVP 规格](.scratch/activity-monitor-mvp/spec.md) | 当前观察版本范围与完成条件 |
| [技术基线](.scratch/activity-monitor-mvp/technical-design.md) | 已确认技术选择与覆盖边界 |
| [MVP 验收](docs/validation/activity-monitor-mvp.md) | 组件、真实事件、时延与后台验证 |
| [核心可行性](docs/research/feasibility.md) | 文件、打包、网络、加密、权限和分发 |
| [现有项目与复用](docs/research/existing-projects.md) | 开源参考和待核查项 |
| [整体架构](docs/architecture/technical-options.md) | 后续核心防护、接口和平台候选 |
| [核心验收方案](docs/validation/core-validation.md) | 发现、外传控制与接收证据 |
| [开发约定](AGENTS.md) | Agent 工作约定 |
