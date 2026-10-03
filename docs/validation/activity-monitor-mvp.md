# 文件活动监控 MVP 验收

这是可复现入口和证据口径。组件通过、真实系统事件、告警生成、通知发送和用户看到通知分别记录，不能互相代替。当前交付是观察版，不执行外传拦截。

## 已验证与待验证

2026-10-04 集成基线 `5b93bc2`：62 项 Rust 测试通过、1 项信号 worker 由监督用例显式调用；8 项 Python 发送器测试通过，fmt 与严格 clippy 通过。06 验证分支在此基础上运行全部 64 项 Rust 测试通过、fmt／严格 clippy 通过；8 项 Python 发送器与 3 项验收计时判定器自测通过。新增两个实际 CLI→普通用户宿主→SQLite 用户流程／采集权限拒绝测试。它们刻意没有可用采集源，验证手动目录、固定历史快照导入、配置不随新历史扩张、宿主重启回查和权限边界。

| 层次 | 当前证据 | 结论边界 |
| --- | --- | --- |
| Adapter／规则／SQLite／IPC／CLI | 匿名 fixtures、真实本地 IPC、持久化回查、故障注入、信号子进程 | 组件契约通过；替身不代表 root ES |
| 系统字段探针 | [本机探针](../../.scratch/activity-monitor-mvp/real-probe.md) | eslogger 真实字段已见；短时探针不代表持续链路 |
| 完整匿名真实运行 | `scripts/validate-mvp.py` | 已准备，尚未获本轮真实运行证据 |
| 3 秒生成／通知发送 | 实际标准事件、outbox、通知反馈的独立时间 | 待真实运行裁决 |
| 通知到屏、后台 FDA、登录补发和重启 | 下文独立步骤 | 尚未验收 |

原生输入参考 macOS 15.6.1 的 eslogger schema 1／message 9 和 Apple SDK 字段；实际脚本会报告本机 OS、Python、二进制版本及 SHA256。SQLite 读取器锁定 schema 2，发现不兼容立即失败。

## 真实运行前

用普通用户在仓库根目录构建。不要将整个脚本以 root 运行：

```sh
cargo build --release --locked
./target/release/codeperimeter service plan --user "$(id -un)"
```

固定采集端点为 `/var/run/codeperimeter-<uid>/collector.sock`，一个 collector 只服务一个分析消费者。已有固定 socket 时，入口不连接它并拒绝继续，包括可能的残留端点；先查看计划、状态和已有目录，明确决定是否执行 `sudo ./target/release/codeperimeter service stop --user "$(id -un)"`。脚本不会代替用户停止已有服务或修改其数据库。

新环境先在同一终端完成 sudo 授权，然后显式准备受保护副本：

```sh
sudo -v
python3 -B scripts/validate-prepare-collector.py --binary target/release/codeperimeter
```

准备入口检查来源普通文件和权限，检查 root 父路径／ACL，先复制到 root 私有暂存文件，清除新创建自有目录／文件的继承 ACL 并核对 SHA256，再用系统 link 原子发布为 `/Library/CodePerimeter/<uid>/codeperimeter`；发布也拒绝已有目标，包括并发创建。已有目标一律拒绝覆盖；已有安装应先按服务管理流程核对版本，不能为了验收覆盖使用中的 root 二进制。此入口只准备副本，不创建 job、不运行 collector、不变更 FDA。

这条观察路线使用系统 eslogger 的已有 ES 授权，不需要为本项目申请自有 ES 开发者签名。责任进程仍需要 FDA：终端／eslogger 探针成功不推导包装二进制或 launchd 已授权。实际错误包含 `permission_denied` 时，到系统设置检查责任进程；必要时给上述受保护 codeperimeter 副本和 eslogger 授予完全磁盘访问，再重试。不改 TCC 数据库、SIP、AMFI 或 sudoers。

## 单次完整真实入口

```sh
python3 -B scripts/validate-mvp.py --binary target/release/codeperimeter
```

可先用 `--preflight-only` 检查二进制、root 路径、版本一致、当前终端 sudo 与端点占用；它不启动监控，也不尝试连接已有固定采集端点。可用 `--report-dir /private/tmp/一个新的空目录` 指定本地结果目录，已有非空目录会被拒绝。

入口按以下顺序执行：

1. 创建 55 个匿名源码文件；启动预加载对照，收到 ready 后才开始监控。
2. 以普通用户启动独立 daemon／SQLite／控制 socket，只添加本轮 `workspace/project`。root collector 固定 root peer 和端点，sudo 保留当前 TTY 认证；collector 自己建立 PGID。notify 和每个发送器使用独立进程组。
3. 必须先观察到真实合成文件事件才继续；释放已预加载的进程，之后在内存中压缩，不重新打开源码。
4. 显式 create/write/open/mmap/close/rename/fork/exec/exit；执行单文件读取、可读 mmap、55 次同文件读取、55 个不同文件批量、tar／zip 项目内和临时输出、同进程落盘／内存归档，以及正常搜索／索引／构建。
5. 从本轮匿名 SQLite 只读回查标准事件、告警、outbox 首建和通知反馈。每场景等待通知队列收敛，记录缺失与超时，不借操作端元数据补造监控证据。
6. 从认证 run_id 取得本次 collector PID，核验 root、自有 PGID、受保护执行路径和本次 sudo 父子关系后精准停止；核验其已登记子进程退出。保存受控断流状态，结束普通用户进程。不会按名称全局 kill、自动注销或重启。

九类 `observed_events_by_kind` 计数反映宿主收到的真实、未筛选标准事件；同时检查目录内标准事件，包括 rename。exec／fork／exit 无项目路径时按范围过滤，只保留固定计数；计数有增长不证明每个生命周期都归属于保护项目。订阅九类本身不算九类已经实见。

内存归档验收访问记录和批量告警，提醒始终写“批量文件访问”。预加载对照期望释放后没有新源码读取与批量提醒，不能以没有告警证明没有压缩。正常搜索／索引／构建可能同样触发默认 50／10 秒门槛；结果用于误报和阈值校准，操作须正常完成。

## 时间和结果

- 批量触发取同进程代际、实际可读 OPEN／MMAP 中滚动 10 秒内第 50 个不同文件的来源时间，按 dev／ino 或路径去重。
- 归档命令取对应真实 EXEC 的来源时间；归档输出取已有项目读取关联后的第一条实际输出事件。最终合并告警的 last_timestamp_ms 不能当作首条触发时间。
- 生成时间取宿主实际保存的 outbox.created_timestamp_ms；通知取 NotificationRecord.observed_timestamp_ms 且 outcome=sent。三者独立报告，摘要包含 sample_count、missing_count、max 和 p95。
- 缺失、负时延、超过 3000ms、字段／序号缺口、桥接／宿主丢弃、数据库缺口或不完整清理明确判失败／部分通过。无真实源不会回退 fixture。
- sent 只表示通知命令接受，不等于用户看到弹窗。脚本结束仍将桌面展示和系统后台验收列为待验。

标准输出仅给结果与 `summary.json` 路径。结果目录包含私有权限的匿名操作元数据、筛选后的标准证据与健康状态；完整系统 raw JSON、完整 args／env 和文件正文不落盘。采样以后台角色及后代 RSS 求和、ps 累计平均 %cpu 报告，无性能基线时不宣称开销达标。脚本保留合成产物和证据，方便核查；目录外归档仍按发送器清单登记，不自动删除未知文件。

退出码：0 代表本轮脚本定义的真实观察／发送检查通过（仍有到屏和后台待验），1 代表实际场景失败／部分通过，2 代表前置条件、源连接或执行失败。`--preflight-only` 的 0 只表示前置检查通过。

前置失败已本机检查：未准备受保护副本时返回 2，摘要 `real_source_confirmed=false`／`failed_no_fixture_fallback`，只留下私有摘要，没有启动 daemon／collector／发送器。这不是实际 ES 运行。

## 后台和故障的独立验收

完整入口是短时前台编排，不能证明 launchd 生命周期。先预览安装计划，再由用户明确安装：

```sh
./target/release/codeperimeter service plan --user "$(id -un)"
sudo ./target/release/codeperimeter service install --user "$(id -un)"
./target/release/codeperimeter status
```

核对 system collector 为 root，system daemon 为指定普通用户，notify 为该用户 Aqua 会话，检查独立后台 FDA；关闭终端后重新执行匿名操作，回查相同证据。用户明确注销／登录后检查后台分析继续、待通知仅补一条汇总；用户明确重启后核验新 collector run_id、恢复采集与缺口记录。不能把 plist／launchctl 成功当作事件已采集。FileVault 解锁前和进入 macOS 登录窗口后的阶段应分别记录，本项目没有证明解锁前执行能力。

源断开在真实入口中受控验证；数据库写入失败、超长／错误 schema、序号缺口、通知失败和大批补发已有独立组件故障注入。真运行遇到这些故障要保留实际状态，不把组件注入当成本机故障通过。公开报告只使用匿名场景和摘要。
