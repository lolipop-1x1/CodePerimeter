# 后台采集与服务管理

当前 MVP 使用 launchd：root collector 只启动系统 `/usr/bin/eslogger` 并转发内存流；普通用户 system daemon 分析、写入 SQLite；该账户 Aqua 桌面会话中的 notify agent 发送通知。system daemon 的 `UserName` 使分析在开机后即可运行，不依赖终端或桌面登录。

CLI 通过服务计划预览、安装、启动、停止和卸载。计划中的路径及三组 argv 是实际安装依据：

| 角色 | 启动参数 | 权限和生命周期 |
| --- | --- | --- |
| collector | `collector --socket <collector.sock> --allowed-uid <uid>` | root system LaunchDaemon |
| daemon | `daemon --socket <collector.sock> --control-socket <host.sock> --db <events.sqlite>` | `UserName` 普通用户 system LaunchDaemon |
| notify | `notify --control-socket <host.sock>` | 目标账户 `~/Library/LaunchAgents` 的 Aqua 代理 |

安装二进制为 `/Library/CodePerimeter/<uid>/codeperimeter`，由 root 拥有、0755；目录同样由 root 拥有且普通用户不可写。本服务管理的 root 目录／文件会清除继承 ACL，避免 POSIX 模式之外的普通用户写权限。system job 位于 `/Library/LaunchDaemons`，不长期执行用户可修改的 checkout 或 target 二进制。安装复制完成后再启动，更新需重新安装并重启 job。安装来源必须为目标账户拥有，或 root 拥有且普通用户可读的普通文件；不接收符号链接叶节点和组／其他用户可写的来源。

collector socket 固定为 `/var/run/codeperimeter-<uid>/collector.sock`。父目录 root-owned、0750，socket root-owned、0660、目标账户主组；服务仍逐连接检查真实 peer uid，仅接受配置账户。普通用户客户端同时检查路径权限和内核报告的 root peer，不能仅靠文件名相信来源。一个采集实例只向一个消费者传流。

分析数据目录为 `~/Library/Application Support/CodePerimeter`，安装时通过降权的系统命令创建并设为 0700。通知 plist 也通过普通账户写入；root 不直接在用户可改目录中执行写入，避免父路径符号链接竞态扩大 root 权限。控制 socket、SQLite 和其侧文件的私有权限由分析宿主维护。停止会持久禁用两个 system job，直到再次启动启用，避免下次开机自行恢复采集；当前桌面通知 job 同时卸载，未来会话中的通知代理只能等待已停用的宿主。卸载删除本服务 job、二进制与已知安装临时文件，默认保留目录配置和 SQLite 数据。

raw JSON 不进入临时文件、日志或 SQLite。stdout 通过有界队列和 Unix socket 传输，单行上限 1 MiB、队列最多 32 项；无消费者、慢连接和队列过载会丢弃事件并增加桥接丢弃计数，明确产生覆盖缺口。超长／非 UTF-8 行另报状态。采集实例 ID 与每秒心跳随 frame 传递；心跳只证明桥接存活，不证明 ES 源健康、没有漏事件或通知已展示。断流、源退出和新实例由宿主分别记录。

eslogger stderr 只瞬时分类成静态健康消息，不回显原始字段。源退出后短暂保留失败状态供宿主读取，再由 launchd 按节流策略重启；新源实例不能被当作无缺口连续采集。此进程不写 SQLite，也不持有文件规则或通知状态。

## 权限和验证边界

实际安装／管理需要管理员权限。API 在普通权限下返回明确错误，CLI 可先展示完整计划，再由用户运行管理员命令；不会修改 sudoers、SIP、AMFI、TCC 数据库或自动重启机器。

本机 `eslogger(1)` 手册要求 root 和责任进程的完全磁盘访问权限（FDA）；直接作为 LaunchDaemon 运行时，手册要求授权 eslogger 自身。当前 root codeperimeter 包装后再启动 eslogger 的链路仍需真实核验：终端和 eslogger 已授权不表示包装链自动获授权。若报告 `ES_NEW_CLIENT_RESULT_ERR_NOT_PERMITTED`，应按错误和实际后台责任进程检查权限，必要时在系统设置中给安装后的 `/Library/CodePerimeter/<uid>/codeperimeter` 授予 FDA。

无需用户自行创建 Apple 开发者证书的路线目前只作为本机 CLI 原型。既有系统工具的 ES 授权不会转成项目的 AUTH 拦截授权。当前安装、后台 FDA、注销／未登录、重启恢复和性能，必须通过真实运行证据验收；生成 plist、IPC 替身或解析样本通过不等于系统后台已经工作。

测试中的 `CollectorClient::connect_expected` 明确接受当前测试账户的替身服务；生产 `connect` 固定验证 root。测试帧使用 `test-only-run` 和 `test_mode`，不宣称已采到系统事件。匿名测试验证真实 Unix peer API、路径权限、边界／断流、有界队列、角色 argv 以及本机 `plutil` 语法。

依据：本机 macOS 15.6.1 的 `eslogger(1)`、`launchd.plist(5)`、`getpeereid(3)` 手册与 [Apple launchd 文档](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html)。核查日期：2026-10-03。
