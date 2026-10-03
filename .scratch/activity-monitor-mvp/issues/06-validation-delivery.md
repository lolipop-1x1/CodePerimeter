# 完整合成验收、说明与交付

Status: claimed
Type: task
Blocked by: none (实现依赖与双轴审查已完成；真实授权运行与交付收尾由 root 统一协调)

## Scope

所有权：`tests/end_to_end.rs`、完整链路验证脚本（`scripts/validate-*`）、README、`docs/validation/activity-monitor-mvp.md`、CI。合成操作发送器由 08 提供；必要运行修复单独回传，不覆盖他人模块。

## Acceptance

- 目录导入到系统采集、50／10 秒告警、通知和 SQLite 回查；外部 tar／zip、同进程落盘／内存压缩、正常操作与故障均有可复现入口。
- 组件测试、合成输入与本机 ES 实测分开；记录 OS、工具版本、权限、源时间、3 秒目标、性能与覆盖缺口。
- 未登录／重启／后台 FDA 未完成时如实记录，需要用户操作的验收明确列出，不以 fixture 或 plist 代替实测。
- Cargo fmt／clippy／tests 与必要本机运行通过，公开材料仅匿名场景。
- PR 写明本地规格和票据 Closing 引用，运行双轴 code-review 并修复后再 ready；清理实现 worktrees。


## Answer

实现与两轮双轴源码定点复核已完成，PR此前已Ready。整票保持claimed：首跑失败后新源真实授权链路、桌面／后台验收与新head CI仍待；旧实现worktree归档受App保护，修复worktree暂保留，由主agent统一协调后续运行与清理。

- `tests/end_to_end.rs` 运行真实 CLI→普通用户 daemon→SQLite：手动目录、匿名 Codex／Claude 元信息合并、预览后新增历史不扩大范围、同库重启回查／移除；独立用例验证普通用户启动 collector 被拒绝。采集源刻意不可用，不冒充 ES。
- `scripts/validate-prepare-collector.py --binary target/release/codeperimeter` 只在用户终端显式 `sudo -v` 后准备 root-owned `/Library/CodePerimeter/<uid>/codeperimeter`，检查 root 父路径、写 ACL、来源权限和复制 SHA256，默认拒绝已有目标；仅无launchd安装的验收副本可明确旧hash安全替换，root发布时重查完整路径／ACL、旧新hash与无活动端点／进程／plist／loaded jobs；不创建 job、不改 FDA。
- `scripts/validate-mvp.py --binary target/release/codeperimeter` 创建本轮私有匿名目录／SQLite，root 仅 collector，固定 root peer/socket，保持 sudo TTY。已有端点直接拒绝且不连接、不删除，避免占用现有单消费者桥接。取得可信 run_id 后核验 PID／root／独立 PGID／受保护路径／本次 sudo 祖先，精确停止并核验自身 eslogger 子进程退出；无可信 run_id 只请求本次 sudo 转发停止，不猜测其他 PID。
- 入口要求本轮读取进程的实际可读 OPEN／MMAP 后继续，涵盖九类实际计数及目录内 rename、读取／映射／同文件重复／55 批量、tar／zip 两种输出位置、同进程落盘／内存、监控前预加载对照、搜索／索引／构建，以及受控断流。严格区分固定九类观察计数与落在项目范围的持久化事件。
- 标准 SQLite 只读回查用于独立裁决来源触发、outbox 首建和 NotificationRecord sent 时间；50 个不同文件／10 秒触发按进程代际和 dev/ino，归档按实际 exec／首次关联输出，60 秒合并后的 last 不作首次触发。报告 sample／missing／max／p95，负时延、超过 3 秒、数据缺口、断流不可见或不完整清理不算通过。`scripts/validate-selftest.py` 三个匿名判定器自测防止晚合并时间、重复读取和缺失／负时延被误判；这些不代表真实系统事件。
- 证据只保存匿名操作元数据、选定目录标准事件／告警、健康和通知结果；全系统 raw JSON、完整 args/env、文件正文不落盘。主脚本无 fixture fallback。保留合成产物供核查，不删除未知文件。性能仅给后台 RSS 求和与 ps 累计平均 %cpu，无基线不声称达标。
- README、验收文档、macOS CI 说明组件和真实验证层次；launchd/FDA、注销／补发、重启／未登录、FileVault 解锁前、桌面通知到屏明确待验，不自动 install/logout/reboot。

验证（2026-10-04）：fmt、严格 clippy、全部 Rust **64 passed／1 ignored helper（由监督用例显式调用）**；Python 发送器 **8 passed**；验收计时判定器 **3 passed**；全部 validate 脚本 AST、两个入口 `--help` 通过。`--preflight-only` 在无受保护副本的本机准确返回 2，私有 summary 为 `real_source_confirmed=false`／`failed_no_fixture_fallback`；未运行 root 准备或真实采集。

依赖为 05 `ac46fe6`、10 `dcda4db`／`0449640`，与集成 `5b93bc2` 的模块基线一致；07 生命周期修复另提交 `4fc976d`，已合入。真实 API／手动执行入口见 `docs/validation/activity-monitor-mvp.md`。真实探针字段参考 `real-probe.md` 与本机研究 handoff，仅作为字段来源，不用短时探针代表本票验收通过。


交付必要补修：恢复 01 初始化误覆盖的原 `.gitignore` 五行（`.DS_Store`、`.env`、`.env.*`、`!.env.example`、`*.log`），保留新 target／SQLite 四行。root 准备副本采用私有 UUID 暂存＋校验＋系统 `/bin/link` 原子非覆盖发布，finally 只删除本轮暂存链接。已用匿名普通用户目录验证该系统工具拒绝已有文件／目录、不写入已有内容，新目标与暂存同 inode；未把此工具测试当作 root 准备已执行。

最终 manifest：`.github/workflows/ci.yml`、本票、`.gitignore`、`README.md`、`docs/validation/activity-monitor-mvp.md`、`scripts/validate-mvp.py`、`scripts/validate-prepare-collector.py`、`scripts/validate-selftest.py`、`tests/end_to_end.rs`。07 生命周期修复另列其既有提交所有权。

- 2026-10-04审查补修：双轴固定点审查发现Standards 2项、Spec 4项，已逐项修复并添加组件回归；tar／zip两种输出位置现在独立要求实际输出事件和项目关联，读取器锁定schema3；install后明确start；地图同步resolved票据和当前待验口径。双轴定点复核已通过；当时完整系统验收、PR ready与清理尚未完成（PR随后已Ready，当前状态见本票末尾），整票保持claimed，详见 [审查记录](../code-review.md)。

- 2026-10-04定点复核完成：Standards S1／S2与Spec P1–P4均通过，各剩余可证缺陷0；原始2＋4统计保留在审查记录。修复来源`0c9a1c9`已合入`a08bcc2`，tree同为`bc683f579403766f5d4a80d7055ca0b90987ef2d`。release已由主agent构建并复制集成target/release，SHA256 `6c07108515587f5c9b78e46b578105ce23f9680f366948934f43b8c0e5927b13`；用户已取得真实终端匿名验收步骤；随后首跑实际失败，结果与补修见下方记录。源码冻结，本阶段不重复全套；真实root／FDA、3秒、到屏、后台与性能仍待验，当时PR ready和cleanup尚未报告完成，PR随后已Ready，清理限制见下方。

- 2026-10-04远端CI基线修正：run `37142828933` 的浮动stable安装Rust1.99.0，新增11条collapsible_if／function-casts-as-integer lint触发严格检查失败；本机验证基线Rust1.88.0。workflow固定1.88.0，fmt／clippy -D warnings／tests保持，源码与release不变。已完成配置解析与diff核对，远端重跑待结果，不报告CI通过；当时本轮真实root／FDA／3秒／后台等实际结果尚未收到，随后首跑结果见下方记录，06保持claimed。


- 2026-10-04真实首跑失败与启动补修：本地匿名证据 `/private/tmp/codeperimeter-validation-zxf_c66m/summary.json`／`failure-or-final-evidence.json` 实见run_id null、九类事件与持久化计数0、collector reconnecting，root清理完成；不能算实际ES或3秒通过。核对代码与本机目录：旧 `/var/run` canonicalize到root:daemon 0775的 `/private/var/run`，严格root路径检查会在建socket前拒绝。新固定端点 `/Library/CodePerimeter/<uid>/run/collector.sock` 先检查既有root安装目录，只创建root:目标组0750专用run，socket0660；不chmod系统目录或放宽校验。验收新增有界stderr白名单静态诊断／退出码及提前退出失败，不落盘原行。prepare新增显式旧hash替换无launchd验收副本：固定可信系统Python隔离执行，root发布时再次核验旧新hash、完整root路径／写ACL、新旧端点／控制端点、任何本服务进程和本UID plist／loaded jobs；未知项拒绝且不删除。普通权限 `/usr/bin/python3 -I -B -S` 可用为3.8.2；未运行管理员发布或修复版真实采集，下一轮root/FDA／3秒／到屏／后台仍待验。
- 先前head `2e18ef6` 的远端push／PR Component checks已SUCCESS（run `37144069732`／job `111264283915`；run `37144066116`／job `111264274209`）；启动补修新head仍需独立CI。十个旧实现worktree的App归档均被pinned task/workspace保护拒绝，保留workspace，未手工删除或变更固定状态；PR此前已Ready；全清理不声称完成。06保持claimed。

本次启动补修定点检查（普通用户uid501）：fmt --check、全部target check／严格clippy、git diff --check通过；Rust service_collector 11＋end_to_end 2通过；Python判定器／启动／安全替换17通过。3个validate脚本AST、两个入口help、固定系统Python root全链／ACL与 -I -B -S标准库可用性通过。root发布门禁测试仅在普通用户匿名目录替身中验证拒绝及原子替换，不代表管理员准备或ES系统通过。源码范围仅service固定路径；没有重复此前全套69项Rust，也没有执行sudo／FDA／安装／真实历史。


- 2026-10-04启动补修候选 `54bbb0d2a9b29d82cbb795b3203367b0e3402fa5` 两轴独立定点复核完成：Standards hard0／heuristic0、Spec可证缺陷0，独立新增8个反例通过。来源父提交为 `2e18ef6ce75ded164ee84489e93fce0c2a79a023`，候选tree `08c1bb432a447cc44cc56162e604d1b610ebe94f`；[审查记录](../code-review.md)保留原2＋4统计与本轮来源边界。主agentoffline release构建成功，外部产物 `/private/tmp/codeperimeter-integration-release/release/codeperimeter` SHA256 `987018ac8168f6ad6170d520637a574a8dc470eb49e3c3bdaafc43b33b44e17a`，service plan回读确认三角色新socket一致。此时集成target尚未复制，root副本仍为旧 `6c07108515587f5c9b78e46b578105ce23f9680f366948934f43b8c0e5927b13`，未替换、未启动新真实源；新head CI、完整root／FDA、九类、3秒、到屏／后台仍待，保持claimed。
- 当前交付状态：PR此前已Ready；十个旧实现worktree归档受App的pinned task/workspace保护拒绝，未手工删除或修改固定状态。修复worktree暂保留供后续实际问题处理，后续同样使用App正常归档流程；不声称全清理完成。此次只更新本票与审查记录，diff检查通过，不改源／脚本／测试，不重复测试。
