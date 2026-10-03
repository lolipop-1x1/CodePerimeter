# root 采集桥接与 launchd 安装管理

Status: resolved
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

## Answer

- `CollectorOptions { socket_path: PathBuf, allowed_uid: u32 }`；`run_collector(options) -> crate::Result<()>` 固定启动 `/usr/bin/eslogger`，root 负责最小内存桥接。
- `CollectorClient::connect(&Path) -> io::Result<Self>` 验证 root socket／peer；`read_frame(&mut self) -> io::Result<CollectorFrame>` 返回 Line、Heartbeat、Status。原始行只在内存，健康 message 不回显原始 stderr／args。
- `ServicePlan::new(username: &str, source_binary: &Path) -> crate::Result<Self>` 生成预览；`install/start/stop/uninstall` 提供对应操作。安装二进制在 `/Library/CodePerimeter/<uid>/codeperimeter`，root-owned、0755，根采集 job 不从用户可写 checkout 运行。
- 角色 argv：`collector --socket <collector.sock> --allowed-uid <uid>`；`daemon --socket <collector.sock> --control-socket <host.sock> --db <events.sqlite>`；`notify --control-socket <host.sock>`。05 与此保持一致。
- collector socket 父目录 root-owned、0750，socket root-owned、0660、目标账户主组，仍逐连接核验 allowed_uid；分析／控制 socket 和 SQLite 目录由普通用户维护私有权限。无桌面会话时普通用户 system daemon 仍运行，通知 agent 等待对应账户 Aqua 会话。
- FDA 按最终后台责任进程启动链单独验收；probe 中终端授权成功不推导 launchd 包装链已授权。安装不修改 TCC、SIP、AMFI、sudoers，也不自动重启。
- 具体返回类型：`ServicePlan` 提供 username／uid／gid、source_binary／installed_binary、data_dir／collector_socket／control_socket／db_path 与 jobs。每个 job 带 label／domain／plist_path／argv／plist；操作返回 `OperationReport { steps: Vec<OperationStep { label, success, message }>, data_preserved: bool }`。宿主必须检查各 step 的 success，不把 launchd 接受启动请求当作真实采集成立。未进入对应桌面时通知代理记录待会话启动，不影响 system job 启动。
- `CollectorFrame` 为 serde 内部 tag `type` 的 Line `{run_id,line,received_timestamp_ms}`、Heartbeat `{run_id,dropped_lines}`、Status `{run_id,state,message,dropped_lines}`。状态涵盖 connected、permission_denied、coverage_gap、oversized_line、invalid_line、source_error、source_diagnostic、stopped；心跳只表示桥接存活，宿主必须保留源失败／缺口。源 stdout 结束保证发出结束标记，失败状态短暂保留给稍后启动的宿主，再交 launchd 重启，新实例 ID 不冒充连续采集。
- 同时提供 `read_bounded_line(&mut impl BufRead, maximum: usize) -> io::Result<Option<Vec<u8>>>`、`peer_uid(&UnixStream) -> io::Result<u32>`、`verify_peer_uid(&UnixStream, expected_uid: u32) -> io::Result<()>`。`CollectorClient::connect_expected(path, expected_uid)` 仅用于显式替身测试，生产使用固定 root 身份的 `connect`。Frame 最大 6 MiB＋4096 字节（覆盖 JSON 转义），原始行 1 MiB；队列 32 项，不写 raw 输出。
- root 采集 socket 固定于 `/Library/CodePerimeter/<uid>/run/collector.sock`，拒绝任意 root socket 目标；macOS 用 getpeereid 双向验证内核 peer 身份。安装自有 root 文件／目录清除继承 ACL；用户路径的 mkdir／chmod／通知文件复制和卸载通过普通账户运行系统命令，root 不追随用户可改父路径写入。两个 system job 停止时持久禁用，再次启动显式 enable；卸载保留所有 SQLite／目录配置。
- 验证：3 个 unit＋10 个集成测试通过，包括实际 Unix peer uid、身份拒绝、权限／symlink／可写父目录、替身帧传输与 EOF、超长帧边界恢复、bounded queue 过载、计划篡改、三角色 argv、本机 3 份 plutil 检查与实际扩展 ACL 清除；`cargo fmt --check` 和 clippy `--all-targets -- -D warnings` 通过。替身帧明确 test-only-run／test_mode，不冒充真实 ES 或 root 采集。
- 本机事实依据：macOS 15.6.1 的 eslogger／launchd.plist／getpeereid 手册，核查日期 2026-10-03。最终安装、后台 FDA、注销／未登录、开机／重启恢复和性能尚待票据 06 的真实验收；未在本票据中执行系统安装或后台授权变更。


### 06 接入发现的临时采集生命周期修复（2026-10-04）

`run_collector` 在 sudo 同 TTY 认证完成后建立自己的 PGID（已有自有组时保留），eslogger 继承该组。SIGINT／SIGTERM 处理器只设置无锁退出标志；主循环随后发送 stopped，kill＋wait 自己启动的 eslogger，并删除自己的 socket，受控停止返回成功。来源子进程由作用域 guard 管理，其他 early Err 同样回收该子进程，不按进程名或共享组清理。无新增依赖或公开 API；可信 `run_id=eslogger-<collector_pid>-<timestamp_ms>` 可供临时验收入口取得 collector PID，再核验 root 身份、自有 PGID、二进制与本次启动的父子关系。

新增独立测试子进程验证 SIGINT／SIGTERM 下 PGID=PID、与测试宿主组不同、退出成功且已回收明确的 sleep 替身；另测 early Err 也回收来源。信号 worker 标为 ignored，只由监督测试显式启动，不在并行测试宿主中改变信号处理器。结果：service unit 5 passed／1 helper ignored（其中监督测试实际调用两个信号 worker），service_collector 10 passed；clippy all-targets 无警告。该测试验证生命周期机制，未以 root 启动 eslogger；真实临时链路清理由 06 实测核验。
