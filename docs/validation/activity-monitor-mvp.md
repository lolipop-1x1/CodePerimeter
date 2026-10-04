# 文件活动监控 MVP 验收

这是可复现入口和证据口径。组件通过、真实系统事件、告警生成、通知发送和用户看到通知分别记录，不能互相代替。当前交付是观察版，不执行外传拦截。

## 已验证与待验证

传输补修源码为 `46dd68f`：有界队列使用背压保序，保留读写半帧，可靠传递连接状态和错误分类，报告停滞及覆盖缺口，停止回收自身线程。两轴独立复核剩余0项。交付基线 `2f38c95` 的 PR／push CI均成功，日志核实 Rust79通过／1信号helper由监督用例调用、Python发送器8／判定器20通过，fmt与严格clippy通过；这些是组件证据。

第三次真实运行已使用受保护的 `a67ecbff53ad0b44c7883af6c9286d5130df4f239f2a0742b9b4aecc37ab3f14` 二进制。macOS15.6.1、Python3.12.12、eslogger schema1／message9，root身份与桥接核验通过，指定合成进程的普通源码文件可读OPEN已持久化，`real_source_confirmed=true`。本轮观察3,680条系统事件、保存5个open和5个close，本地collector／reader丢弃及SQLite保存缺口均0，root清理完成。运行仅2.765秒：这不证明长期完整性、性能或3秒告警达标。

该轮仍判 `failed_no_fixture_fallback`，阶段为 `synthetic_scenario_matrix`：九类事件操作留下 `.event-matrix`，后续mmap发送器在严格项目清单检查时退出2，映射尚未执行。普通用户按原顺序已复现此测试脚本冲突。完整九类项目覆盖、批量／归档告警、3秒生成／发送、到屏与后台仍待完整重跑；不回退fixture，也不放宽事件、健康或3000ms判定。

2026-10-05 操作端补修 `d4cdc40`：九类worker成功后只删除已知重命名文件和自建空目录，既有目录与未知文件继续拒绝清理。完整普通用户顺序进一步复现macOS tar默认AppleDouble附加成员导致严格清单失败；仅对子tar设置 `COPYFILE_DISABLE=1`，保持父环境、argv及严格55个源码成员和全部正文校验。此开关以本机bsdtar3.5.3／libarchive3.7.4实际create为依据，不宣称所有版本兼容；PAX扩展属性仍可能保留。Python3.12.12验收自测24项、发送器9项通过，包含原始完整场景顺序、预加载释放、项目内／临时输出、正常操作、严格cleanup与未知文件保留。冻结候选 `d4cdc40` 的两轴独立复核：Standards hard0／heuristic0，Spec可证缺陷0／scope creep0；独立验证预加载后源码路径暂时缺失仍能完成内存压缩、完整场景顺序、未知符号链接保留与严格清理。这是操作端回归，不代替完整真实系统重跑；本轮远端CI结果以PR检查回读为准。

2026-10-04 集成基线 `5b93bc2`：62 项 Rust 测试通过、1 项信号 worker 由监督用例显式调用；8 项 Python 发送器测试通过，fmt 与严格 clippy 通过。06 验证分支在此基础上运行全部 64 项 Rust 测试通过、fmt／严格 clippy 通过；8 项 Python 发送器与 3 项验收计时判定器自测通过。新增两个实际 CLI→普通用户宿主→SQLite 用户流程／采集权限拒绝测试。它们刻意没有可用采集源，验证手动目录、固定历史快照导入、配置不随新历史扩张、宿主重启回查和权限边界。

2026-10-04固定点审查补修：Rust全部目标69 passed／1 ignored监督helper、Python发送器8 passed、判定器9 passed；fmt、check与严格clippy通过。新增连接断开／读超时、宿主启动与定期保留故障、锁恢复后重启通知、来源版本／缺口回查与v1／v2迁移回归。以上是组件证据，完整系统待验项保持不变，详见 [审查记录](../../.scratch/activity-monitor-mvp/code-review.md)。

| 层次 | 当前证据 | 结论边界 |
| --- | --- | --- |
| Adapter／规则／SQLite／IPC／CLI | 匿名 fixtures、真实本地 IPC、持久化回查、故障注入、信号子进程 | 组件契约通过；替身不代表 root ES |
| 系统字段探针 | [本机探针](../../.scratch/activity-monitor-mvp/real-probe.md) | eslogger 真实字段已见；短时探针不代表持续链路 |
| 完整匿名真实运行 | `scripts/validate-mvp.py` | 第三轮已通过指定合成普通文件可读OPEN门禁；后续发送器因测试目录残留退出，完整场景仍待重跑 |
| 3 秒生成／通知发送 | 实际标准事件、outbox、通知反馈的独立时间 | 待真实运行裁决 |
| 通知到屏、后台 FDA、登录补发和重启 | 下文独立步骤 | 尚未验收 |

原生输入参考 macOS 15.6.1 的 eslogger schema 1／message 9 和 Apple SDK 字段；实际脚本会报告本机 OS、Python、二进制版本及 SHA256。SQLite 读取器锁定 schema 3，发现不兼容立即失败。宿主状态回显当前 run_id 的实际 schema／message 版本；标准事件保存对应版本，健康查询的 source 保留 run_id、版本、field 和 missing_events。每个采集运行首次实见或版本变更才新增版本健康记录；用 `health --source-run-id <run_id>` 回查版本变化和缺口。旧 v1／v2 数据原子迁移，旧事件缺版本时保持未知。

CI 固定 Rust 1.88.0，与 Cargo rust-version 和本机已验证基线一致；fmt、clippy `-D warnings` 与测试步骤保持。工具链升级需要独立验证，不把浮动 stable 的新增 lint 当作功能测试结论。

## 真实运行前

用普通用户在仓库根目录构建。不要将整个脚本以 root 运行：

```sh
cargo build --release --locked
./target/release/codeperimeter service plan --user "$(id -un)"
```

固定采集端点为 `/Library/CodePerimeter/<uid>/run/collector.sock`，一个 collector 只服务一个分析消费者。已有固定 socket 时，入口不连接它并拒绝继续，包括可能的残留端点；先查看计划、状态和已有目录，明确决定是否执行 `sudo ./target/release/codeperimeter service stop --user "$(id -un)"`。脚本不会代替用户停止已有服务或修改其数据库。

新环境先在同一终端完成 sudo 授权，然后显式准备受保护副本：

```sh
sudo -v
python3 -B scripts/validate-prepare-collector.py --binary target/release/codeperimeter
```

准备入口检查来源普通文件和权限，检查 root 父路径／ACL，先复制到 root 私有暂存文件，清除新创建自有目录／文件的继承 ACL 并核对 SHA256，再用系统 link 原子发布为 `/Library/CodePerimeter/<uid>/codeperimeter`；发布也拒绝已有目标，包括并发创建。默认拒绝已有目标。仅对无 launchd 安装的验收副本，可明确指定 `--replace-sha256`：普通用户先核对旧hash与完整root路径／ACL；发布时固定 `/usr/bin/python3 -I -B -S` 再核验旧／新SHA256、root属主与完整路径／ACL、旧／新采集端点和控制端点、任何codeperimeter进程、该UID的三份plist与loaded jobs，全部静止才原子替换。系统Python不可用或任一检查不符就失败，不降级、不删除未知端点。已有服务安装应按服务管理流程处理，本入口不是通用更新器。此入口只准备副本，不创建 job、不运行 collector、不变更 FDA。

本机第三轮已将受保护验收副本更新为SHA256 `a67ecbff53ad0b44c7883af6c9286d5130df4f239f2a0742b9b4aecc37ab3f14`，并真实启动采集。此次补修仅涉及Python合成操作脚本；二进制未变且副本一致时，不需重复准备或替换，在同一终端继续运行：

```sh
sudo -v
python3 -B scripts/validate-mvp.py --binary target/release/codeperimeter
```

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
- 归档命令取对应真实 EXEC 的来源时间；归档输出取已有项目读取关联后的第一条实际输出事件。tar／zip 的项目内和临时输出均必须匹配发送器声明的预期输出路径、实际 create／write／rename 事件及项目关联（同run/PID代际的实际项目读配合ArchiveOutput，或先创建输出时的实际EXEC输出参数与项目ArchiveCommand）；仅归档命令告警不能通过。最终合并告警的 last_timestamp_ms 不能当作首条触发时间。
- 生成时间取宿主实际保存的 outbox.created_timestamp_ms；通知取 NotificationRecord.observed_timestamp_ms 且 outcome=sent。三者独立报告，摘要包含 sample_count、missing_count、max 和 p95。
- 缺失、负时延、超过 3000ms、字段／序号缺口、桥接／宿主丢弃、数据库缺口或不完整清理明确判失败／部分通过。无真实源不会回退 fixture。
- sent 只表示通知命令接受，不等于用户看到弹窗。脚本结束仍将桌面展示和系统后台验收列为待验。

标准输出仅给结果与 `summary.json` 路径。结果目录包含私有权限的匿名操作元数据、筛选后的标准证据与健康状态；collector启动stderr持续有界排空，只在 `root_startup` 保留最多8种白名单静态诊断码、退出码与排空完整性；提前退出立即失败，不等待通用超时。完整系统 raw JSON、原stderr、完整 args／env 和文件正文不落盘。采样以后台角色及后代 RSS 求和、ps 累计平均 %cpu 报告，无性能基线时不宣称开销达标。脚本保留合成产物和证据，方便核查；目录外归档仍按发送器清单登记，不自动删除未知文件。

退出码：0 代表本轮脚本定义的真实观察／发送检查通过（仍有到屏和后台待验），1 代表实际场景失败／部分通过，2 代表前置条件、源连接或执行失败。`--preflight-only` 的 0 只表示前置检查通过。

前置失败已本机检查：未准备受保护副本时返回 2，摘要 `real_source_confirmed=false`／`failed_no_fixture_fallback`，只留下私有摘要，没有启动 daemon／collector／发送器。这不是实际 ES 运行。

2026-10-04完整入口首跑已实际执行，证据目录 `/private/tmp/codeperimeter-validation-zxf_c66m`：`summary.json` 和 `failure-or-final-evidence.json` 显示run_id为空、九类计数及筛选事件均0、collector reconnecting，`root_cleanup_complete=true`。本机代码和权限核对定位：旧固定路径经canonicalize进入root:daemon 0775的 `/private/var/run`，严格root路径校验在创建socket前拒绝；首跑原stderr未保存，不能把该失败报告成FDA或ES事件通过。现改为安装目录内专用run路径，严格权限拒绝不变；下一轮实际root／FDA、九类完整事件、3秒、到屏及后台仍待验。

CI固定基线的先前head `2e18ef6` 已由远端push／PR两条 Component checks确认SUCCESS（run `37144069732`／`37144066116`）；本次启动修复新head仍需单独CI。十个旧实现worktree的归档被App以pinned task/workspace保护拒绝，未手工删除或修改固定状态，不报告全部清理完成。

## 后台和故障的独立验收

完整入口是短时前台编排，不能证明 launchd 生命周期。先预览安装计划，再由用户明确安装并启动；install 本身不加载 job：

```sh
./target/release/codeperimeter service plan --user "$(id -un)"
sudo ./target/release/codeperimeter service install --user "$(id -un)"
sudo ./target/release/codeperimeter service start --user "$(id -un)"
./target/release/codeperimeter status
```

核对 system collector 为 root，system daemon 为指定普通用户，notify 为该用户 Aqua 会话，检查独立后台 FDA；关闭终端后重新执行匿名操作，回查相同证据。用户明确注销／登录后检查后台分析继续、待通知仅补一条汇总；用户明确重启后核验新 collector run_id、恢复采集与缺口记录。不能把 plist／launchctl 成功当作事件已采集。FileVault 解锁前和进入 macOS 登录窗口后的阶段应分别记录，本项目没有证明解锁前执行能力。

宿主启动时及每小时清理30天以前明细，累计统计保留；`retention_state`／`retention_last_run_ms` 回显执行与失败，明细过期健康记录表示追溯缺口。清理失败时显示数据库降级，恢复后重试。

源断开在真实入口中受控验证；数据库写入失败、超长／错误 schema、序号缺口、通知失败和大批补发已有独立组件故障注入。真运行遇到这些故障要保留实际状态，不把组件注入当成本机故障通过。公开报告只使用匿名场景和摘要。
