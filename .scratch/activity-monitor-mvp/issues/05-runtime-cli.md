# 后台生命周期、最小权限 IPC、CLI 与通知

Status: ready-for-agent
Type: task
Blocked by: 02, 03, 04, 07

## Scope

所有权：`src/main.rs`、`src/runtime.rs`、`tests/runtime_cli.rs`、`docs/usage.md`。采集桥接与安装管理由 07 提供；不得修改已完成模块，需 API 调整通过主 agent 协调。

## Acceptance

- Rust CLI 配置多个目录、历史发现／预览／批量导入、事件／告警／状态查询与服务管理。
- launchd 系统后台开机运行，root 仅采集／最小转发，普通用户宿主唯一 SQLite 写入，桌面通知会话独立；IPC 验证对端且原始输出仅内存存在。
- 输入断开、错误、序号缺口、过载、SQLite 与通知失败显示状态，恢复不伪装连续完整。
- 新告警及时发通知，60 秒合并不重发；无桌面会话持久化待展示，进入桌面后汇总，SQLite 保存失败仍可实时提醒。
- 安装、启动、停止、卸载可执行且清单明确；用户数据默认保留；不修改 sudoers、SIP 或 AMFI，不自动重启机器。
- 测试覆盖 CLI 到服务到 SQLite 真实 IPC、访问控制、异常／恢复；系统 FDA 与真实重启另验。
