# root 采集桥接与 launchd 安装管理

Status: ready-for-agent
Type: task
Blocked by: 01

## Scope

从 05 拆出独立任务。所有权：`src/service.rs`、`tests/service_collector.rs`、`docs/service.md` 及必要的 `scripts/service-*`。模块只负责权限分离、采集流桥接和服务安装，不写 SQLite、不分析规则、不存原始事件。不要修改共享 Cargo/lib/model。

## Acceptance

- root 启动系统 eslogger，stdout 仅内存转发给受限 Unix socket 的指定普通用户；双向检查 peer uid，路径权限可核验。
- 传输包含 collector run id、原始行、心跳／状态，原始行有界，断流／重启／过载可见；无客户端时不积压原始事件到磁盘或无限内存。
- launchd 三角色：root collector、UserName 普通用户 system daemon、该用户桌面 notification agent。安装参数与生成的 CLI argv 和 05 明确约定。
- 可预览安装计划、安装／启动／停止／卸载；只管理自己的 job／安装目录，默认保留证据数据。二进制与 root job root-owned，不能从用户可改二进制长期以 root 执行。
- 不修改 SIP、AMFI、sudoers，不自动重启。安装与运行需要 root／FDA，实际授权、后台／开机验证分别报告。
- 有意义测试覆盖 socket 身份／边界、传输、超长／断流、plist／argv、路径权限；无 root 时的替身测试不得冒充 root 真实采集。

## 接入约定

提供可供 main 直接调用的 CollectorOptions、ServicePlan／安装操作 API 与客户端读取 frame 的 API，具体签名写 Answer。守护角色建议 CLI 名称为 `collector`、`daemon`、`notify`；05 必须使用生成 plist 中的同一名称／参数。客户端先验证 root 服务身份，原始 JSON 在普通用户宿主中调用 eslogger adapter 后筛选保存。

外部研究笔记见 `/private/tmp/codeperimeter-mvp-research/runtime-notes.md`。真实样本探针已取得 schema1/message9，但完整后台采集仍待验。
