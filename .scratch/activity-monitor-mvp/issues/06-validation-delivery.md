# 完整合成验收、说明与交付

Status: in-progress
Type: task
Blocked by: none (实现依赖已接入；真实授权运行与最终审查待 root 统一协调)

## Scope

所有权：`tests/end_to_end.rs`、完整链路验证脚本（`scripts/validate-*`）、README、`docs/validation/activity-monitor-mvp.md`、CI。合成操作发送器由 08 提供；必要运行修复单独回传，不覆盖他人模块。

## Acceptance

- 目录导入到系统采集、50／10 秒告警、通知和 SQLite 回查；外部 tar／zip、同进程落盘／内存压缩、正常操作与故障均有可复现入口。
- 组件测试、合成输入与本机 ES 实测分开；记录 OS、工具版本、权限、源时间、3 秒目标、性能与覆盖缺口。
- 未登录／重启／后台 FDA 未完成时如实记录，需要用户操作的验收明确列出，不以 fixture 或 plist 代替实测。
- Cargo fmt／clippy／tests 与必要本机运行通过，公开材料仅匿名场景。
- PR 写明本地规格和票据 Closing 引用，运行双轴 code-review 并修复后再 ready；清理实现 worktrees。


## Answer

实现已准备，整票尚未 resolved：真实授权链路、桌面／后台验收、双轴 review、PR ready 与 worktree 清理由主 agent 统一完成。

- `tests/end_to_end.rs` 运行真实 CLI→普通用户 daemon→SQLite：手动目录、匿名 Codex／Claude 元信息合并、预览后新增历史不扩大范围、同库重启回查／移除；独立用例验证普通用户启动 collector 被拒绝。采集源刻意不可用，不冒充 ES。
- `scripts/validate-prepare-collector.py --binary target/release/codeperimeter` 只在用户终端显式 `sudo -v` 后准备 root-owned `/Library/CodePerimeter/<uid>/codeperimeter`，检查 root 父路径、写 ACL、来源权限和复制 SHA256，已有目标一律拒绝覆盖；不创建 job、不改 FDA。
- `scripts/validate-mvp.py --binary target/release/codeperimeter` 创建本轮私有匿名目录／SQLite，root 仅 collector，固定 root peer/socket，保持 sudo TTY。已有端点直接拒绝且不连接、不删除，避免占用现有单消费者桥接。取得可信 run_id 后核验 PID／root／独立 PGID／受保护路径／本次 sudo 祖先，精确停止并核验自身 eslogger 子进程退出；无可信 run_id 只请求本次 sudo 转发停止，不猜测其他 PID。
- 入口要求本轮读取进程的实际可读 OPEN／MMAP 后继续，涵盖九类实际计数及目录内 rename、读取／映射／同文件重复／55 批量、tar／zip 两种输出位置、同进程落盘／内存、监控前预加载对照、搜索／索引／构建，以及受控断流。严格区分固定九类观察计数与落在项目范围的持久化事件。
- 标准 SQLite 只读回查用于独立裁决来源触发、outbox 首建和 NotificationRecord sent 时间；50 个不同文件／10 秒触发按进程代际和 dev/ino，归档按实际 exec／首次关联输出，60 秒合并后的 last 不作首次触发。报告 sample／missing／max／p95，负时延、超过 3 秒、数据缺口、断流不可见或不完整清理不算通过。`scripts/validate-selftest.py` 三个匿名判定器自测防止晚合并时间、重复读取和缺失／负时延被误判；这些不代表真实系统事件。
- 证据只保存匿名操作元数据、选定目录标准事件／告警、健康和通知结果；全系统 raw JSON、完整 args/env、文件正文不落盘。主脚本无 fixture fallback。保留合成产物供核查，不删除未知文件。性能仅给后台 RSS 求和与 ps 累计平均 %cpu，无基线不声称达标。
- README、验收文档、macOS CI 说明组件和真实验证层次；launchd/FDA、注销／补发、重启／未登录、FileVault 解锁前、桌面通知到屏明确待验，不自动 install/logout/reboot。

验证（2026-10-04）：fmt、严格 clippy、全部 Rust **64 passed／1 ignored helper（由监督用例显式调用）**；Python 发送器 **8 passed**；验收计时判定器 **3 passed**；全部 validate 脚本 AST、两个入口 `--help` 通过。`--preflight-only` 在无受保护副本的本机准确返回 2，私有 summary 为 `real_source_confirmed=false`／`failed_no_fixture_fallback`；未运行 root 准备或真实采集。

依赖为 05 `ac46fe6`、10 `dcda4db`／`0449640`，与集成 `5b93bc2` 的模块基线一致；07 生命周期修复另提交 `4fc976d`，已合入。真实 API／手动执行入口见 `docs/validation/activity-monitor-mvp.md`。真实探针字段参考 `real-probe.md` 与本机研究 handoff，仅作为字段来源，不用短时探针代表本票验收通过。
