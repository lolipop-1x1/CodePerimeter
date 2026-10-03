# 完整合成验收、说明与交付

Status: ready-for-agent
Type: task
Blocked by: 05

## Scope

所有权：`tests/end_to_end.rs`、合成发送器与验证脚本、README、`docs/validation/activity-monitor-mvp.md`、CI。必要运行修复单独回传，不覆盖他人模块。

## Acceptance

- 目录导入到系统采集、50／10 秒告警、通知和 SQLite 回查；外部 tar／zip、同进程落盘／内存压缩、正常操作与故障均有可复现入口。
- 组件测试、合成输入与本机 ES 实测分开；记录 OS、工具版本、权限、源时间、3 秒目标、性能与覆盖缺口。
- 未登录／重启／后台 FDA 未完成时如实记录，需要用户操作的验收明确列出，不以 fixture 或 plist 代替实测。
- Cargo fmt／clippy／tests 与必要本机运行通过，公开材料仅匿名场景。
- PR 写明本地规格和票据 Closing 引用，运行双轴 code-review 并修复后再 ready；清理实现 worktrees。
