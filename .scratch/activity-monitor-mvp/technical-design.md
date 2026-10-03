# 文件活动监控 MVP：技术讨论

日期：2026-10-03
Status: claimed
Type: grilling

本轮使用 `grill-with-docs`，结合 `grilling` 与 `domain-modeling` 讨论具体技术路线。产品范围承接 [最小规格](spec.md)，已确认的产品选择不重复询问。用户 2026-10-03 显式调用 `implement-spec`，授权进入实现；本机验收结果由后续运行证据记录。

## Notes

- 当前交付为 macOS 本机 CLI 管理的后台观察版：选定目录、所有来源进程、文件打开／映射证据、批量访问与归档迹象、SQLite／终端回查和系统通知。
- 同进程读取后在内存压缩属于必测场景；按文件活动与批量提醒验收，不声称确认压缩运算。
- 用户目前没有自己的开发者签名；本轮优先讨论系统现成工具或作者已签名发行包作为事件来源，不通过修改系统保护来满足普通主机运行条件。
- 3 秒是待验证的告警与通知发送目标。采集器存在、源码支持和本机通过分别记录。
- 开发语言、进程拓扑、数据格式属于实现选择，记录在本文和架构文档，不写入产品术语表。
- 尚未形成需要 ADR 的稳定技术决策，不预建多平台实现或跨语言 ABI。

## Decisions-so-far

- T1 已确认 A：使用 Rust CLI，规则与关联继续作为后续产品核心；需要 Mac 原生界面或扩展时再考虑 Swift。
- T2 已确认 A：仅采集器使用 root，分析、记录和通知程序以普通用户运行。初始候选为管道接入，T4／T6 扩展为后台运行后改为最小权限本地 IPC，见后台拓扑。
- T3 已确认 B：第一版直接采用 SQLite，支持结构化回查；分析程序维护唯一写入路径，保留筛选后的事件和告警。
- T4 已确认 B：第一版包含后台常驻与自动启动；T6 已明确系统启动即运行。这个选择增加启动、重启、后台权限与用户会话验证。
- T5 已确认 A：批量规则初值为同一进程在滚动 10 秒内访问 50 个不同文件，达到门槛立即提醒，参数可调整；用正常任务样本校准，不证明压缩。
- T6 已按用户补充明确：监控是系统启动即运行的后台进程，不依赖终端或用户登录。桌面通知会话单独处理，不能让通知进程生命周期决定采集是否运行。
- T7 已确认 A：优先验证系统 eslogger，保持带读权限打开与可读映射的证据口径；开源工程用于源码参考与采样对照，不同时实现多个采集来源。
- T9 已确认 A：机器停留在 macOS 密码登录界面时，后台照常生成／保存告警；进入桌面后补发一条未展示告警汇总。有桌面会话时验收通知发送目标。
- T10 已确认 A：支持手动配置多个目录，以及从历史目录候选预览后按需批量导入；配置持久化，新会话不自动扩大监控范围。
- T8 已确认 A：SQLite 保存失败时，若采集与分析仍可运行，则继续实时提醒并明确显示证据保存缺口；不能宣称这段记录完整。采集本身停止另报故障。
- T11 已明确：第一版同时支持 Codex、Claude Code CLI 本机历史与手动目录。不同历史格式分别适配后输出统一项目目录候选；手动目录不依赖 Agent 格式。
- T12 已确认 A：Codex 同时提取会话初始目录与每轮目录变化；只分析目录元信息，不做正文内容分析。Claude Code 也提取实际记录中明确的 cwd 字段，不由文件夹名推断。
- T13 已确认 A：同一进程运行实例、同一规则在 60 秒内合并告警；首次即时通知，后续及时更新数量和证据，不额外弹窗。不同进程或规则分别提醒。
- 用户新增目录来源：从 Agent 历史会话提取项目／工作目录候选，支持一次批量加入监控；当前先通过 CLI 操作，后续再做界面。这不等于提前实现完整历史内容分析。
- 官方工具通过标准输出供外围规则程序读取，是待验证的复用方式；复用发行程序与复制／修改其 ES 源码是不同路线，后者不会继承作者的签名身份。

## 设计树与当前问题

- 事件来源与接入（事实核查中）
  - T7 已确认 A：优先验证系统 `eslogger`；Objective-See 工具与 Mac Monitor 用于参考与对照。
  - 核对实际输出的读权限标志、映射、身份、序号、丢失、分帧与后台授权条件，不能把工具列出事件类型当作采集通过。
  - 从实际来源收敛最小事件结构，记录不能提供的字段，不假设不同采集器同等精度。
- 程序语言与延续方式（T1，已确认 A）
  - Rust CLI，规则与关联继续作为后续核心；Mac 原生需求成立时再添加 Swift。
  - 实现建议与验收：最少依赖、模块职责、构建与测试入口见下文。
- 运行权限（T2，已确认 A）
  - 只有采集侧以 root 运行，普通用户程序经最小权限本地 IPC 分析、记录和通知。
  - 后台常驻与自动启动（T4，已确认 B）。
  - 启动时机（T6，已明确）：系统启动即运行，无人登录时也采集，不依赖终端。
  - 实现验证：后台权限、不同权限程序的事件传输、输入中断与自动恢复。
- 本地证据（T3，已确认 B）
  - 直接 SQLite，筛选后的事件和告警支持结构化回查；唯一写入路径归分析程序。
  - 不保存文件正文、完整命令行和环境变量，沿用已有隐私范围。
  - 保存失败（T8，已确认 A）：继续实时提醒并显示保存缺口，采集停止另报故障。
  - 实现建议与验收：必要字段、保留、回查与写入失败见下文。
- 活动关联与规则（采集事实收敛后讨论）
  - 进程运行身份、父子进程、目录边界、临时目录归档与事件乱序。
  - 滚动窗口、不同文件去重与正常索引／构建对照。
  - 批量原型值（T5，已确认 A）：50 个不同文件／滚动 10 秒，可调整，待正常样本校准。
  - 告警合并（T13，已确认 A）：同进程运行实例／同规则 60 秒内合并，首条即时通知，后续更新证据。
- 提醒、运行状态与验收（上述选择收敛后讨论）
  - 通知调用、发送失败与到屏展示分别记录。
  - 无 macOS 桌面会话时的告警（T9，已确认 A）：后台照常生成／保存；进入桌面后汇总补发。有桌面会话时验收通知发送。这里不指 Agent 账号或本程序登录。
  - 采集健康、丢失可见性、开销、延迟和真实合成场景验收。
- 目录配置与发现（用户新增需求）
  - 历史来源（T11，已明确）：Codex 与 Claude Code CLI；各自窄适配器只提取目录元信息，不分析会话正文。
  - 导入行为（T10，已确认 A）：手动多个目录与按需批量导入；新会话不自动扩大监控集合。
  - 目录覆盖（T12，已确认 A）：Codex 头部与每轮目录变化；Claude Code 实际记录中的 cwd。
  - 失效或无法解释的路径显示状态；存在的目录规范化去重，配置持久化。

## 本机事实快照

本轮仅只读检查，没有启动采集、申请 sudo 或完全磁盘访问权限。

- 系统：macOS 15.6.1，构建 24G90，arm64。
- Rust／Cargo 1.88.0 位于 `~/.cargo/bin`；当前 shell 的 PATH 找不到它们。Python 3.12.5，Apple Swift 5.3.2。
- `/Applications` 与 `~/Applications` 常见位置未找到 FileMonitor、ProcessMonitor 或 Mac Monitor；这不是全盘安装检查。
- `/usr/bin/eslogger --list-events` 成功，列出 `open`、`mmap`、`exec`、`fork`、`exit`、`create`、`write`、`rename` 等事件。
- 本机 `eslogger` 手册要求 root 与责任进程的完全磁盘访问权限；仅提供 NOTIFY，不提供 AUTH。输出 JSON Lines，排除与自身同进程组的进程，不承诺应用接口、schema 或数据完整性。
- 以上不证明本机采集已通过，完全磁盘访问权限、真实输出、事件完整性、性能与 3 秒目标仍未知。

## 资料与实现候选

- [Apple eslogger 介绍](https://developer.apple.com/videos/play/wwdc2022/110345/)：系统自带且已有 ES 授权，可观察事件、验证检测思路；不能由此推导项目已经具备授权拦截。
- [Objective-See 工具说明](https://objective-see.org/products/utilities.html)：FileMonitor／ProcessMonitor 是保留签名 App 包结构运行的 CLI，向标准输出提供 JSON；字段覆盖核查见下文。
- Rust 的 JSON 输入可评估 [serde_json](https://docs.rs/serde_json/latest/serde_json/)，通知可评估 [AppleScript 的 display notification](https://developer.apple.com/library/archive/documentation/LanguagesUtilities/Conceptual/MacAutomationScriptingGuide/DisplayNotifications.html)。这是建议，不是已验证的依赖或通知方案。

## 采集器源码核查：需要收紧的复用判断

核查日期为 2026-10-03，FileMonitor／ProcessMonitor 依据官方 `master` 分支，未取得不可变提交 SHA；源码与官网发行二进制的对应关系尚未核验。

| 候选 | 资料／源码支持的接入 | 影响当前 MVP 的缺口 |
| --- | --- | --- |
| 系统 eslogger | 本机支持 open、mmap、exec 等 NOTIFY；输出 JSONL，已有系统 ES 授权 | 未采集实际输出；后台 FDA、必要字段、schema 变化、丢失可见性和延迟仍须实测 |
| FileMonitor 官方 CLI | 文件 OPEN／CREATE／WRITE／CLOSE／RENAME 等，逐事件输出 JSON 行 | 不输出 OPEN 的 fflag；没有 MMAP；输出没有 ES 序号或稳定进程运行代际标识 |
| ProcessMonitor 官方 CLI | EXEC／FORK／EXIT 与进程信息，逐事件输出 JSON | 不能补足 FileMonitor 的 fflag／mmap；没有 ES 序号输出 |
| Mac Monitor 官方 App | 人工 trace 与 JSON／JSONL 导出，可用于事件对照 | 此前固定版本的实时 XPC 校验自家客户端身份；不是已验证的通用采集服务 |

- [FileMonitor main.m](https://github.com/objective-see/FileMonitor/blob/master/App/FileMonitor/main.m#L133-L187) 的订阅数组没有 mmap；默认输出一行 JSON，`-pretty` 会改变分帧，`-filter` 是路径尾缀匹配，不能代替项目目录边界过滤。
- [File.m 的 OPEN 分支](https://github.com/objective-see/FileMonitor/blob/master/Library/Source/File.m#L122-L127)仅提取路径；[JSON 序列化](https://github.com/objective-see/FileMonitor/blob/master/Library/Source/File.m#L198-L277)未保留 `fflag`。文件事件时间来自回调时的 `NSDate`，不能当作原始内核发生时间。
- [Apple OPEN fflag](https://developer.apple.com/documentation/endpointsecurity/es_event_open_t/fflag)使用 FREAD／FWRITE 标志；已知 OPEN 不能一律标成读打开。可读 mmap 同样需要实际保护标志。
- 两个 Objective-See 工具都是 NOTIFY 观察，不能由使用其发行包获得 AUTH 拦截能力。FileMonitor／ProcessMonitor 当前仓库许可证为 GPL-3.0，直接采用源码或随产品分发前须核对采用版本及许可要求。
- 上轮“官方现成工具可支撑监控 MVP”的判断应收紧：它们可支撑部分文件活动观察，不能直接满足已确认的读权限与可读映射口径。若保持该口径，应优先验证 eslogger 的真实输出，而不是默默降级证据。
- 已选择先验证 eslogger 这一来源取得真实闭环，不同时建设三个未验证的采集 Adapter。

## 后台运行事实与候选拓扑

- [Apple launchd 文档](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html)区分系统 LaunchDaemon 与用户 LaunchAgent：系统服务可在未登录时运行，用户 Agent 只在对应登录会话中运行。
- 本机 `man launchd.plist` 的 `UserName` 可使系统域服务以指定普通用户运行；所以“开机启动”不要求分析与 SQLite 程序一起成为 root。
- 本机 `man eslogger` 的 TCC AUTHORIZATION 明确：作为 launch daemon 运行时，需要给 `eslogger` 自身授予完全磁盘访问权限。没有核验本机当前授权，也未启动后台采集。
- `eslogger` 文档化输出为 stdout JSONL 或统一日志，没有现成 socket 输出。`StandardOutPath` 能写文件，不应把未经筛选的系统事件长期落盘作为桥接方式。
- 推荐的候选职责为：系统 root 采集与最小转发、系统域普通用户分析／SQLite、登录会话中的普通用户通知。IPC 需限定接收账户与核验对端，具体桥接还要验证；T2 的权限原则保持不变，终端管道不再被当作最终后台接入方案。
- 若采用转发器启动 eslogger，实际责任进程与 FDA 检查必须按最终启动链验证；直接作为 launch daemon 运行的手册结论不能自动推导所有包装方式都继承相同授权。
- MVP 本机安装候选为明确的 launchd job 和 CLI 管理；尚未选择 SMAppService 注册或安装包路线，不能声称无自有签名的注册／分发已验证。
- 需要新增的验收：未登录时后台运行、登录／注销不终止采集、进程重启与启动顺序、后台 FDA、IPC、通知会话与停止／卸载。

## 历史目录发现事实

Codex 依据官方源码／文档核查，没有读取本机 Codex 会话；Claude Code 另做少量本机字段抽样，见下文。Codex 来源为核查日的移动 `main` 分支，不是稳定历史 API；实现须固定参考版本并通过合成格式样本与所支持版本验证。

- [Codex SessionMeta](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs#L2902-L2951)明确包含 `cwd`、可选 `runtime_workspace_roots`、会话 ID、`cli_version` 与 `source`。这些目录是环境线索，不证明对应文件被访问。
- [官方 rollout 元数据读取](https://github.com/openai/codex/blob/main/codex-rs/rollout/src/list.rs#L1118-L1166)从头部读取会话元数据。来源目录包括 CODEX_HOME 下的 `sessions` 与 `archived_sessions`；[压缩读取](https://github.com/openai/codex/blob/main/codex-rs/rollout/src/compression.rs#L61-L66)支持 `.jsonl` 与 `.jsonl.zst`。
- 会话头部不覆盖中途切换的工作目录；每轮 `TurnContextItem` 另有 `cwd`／工作根目录。T12 已确认同时提取这些变更，逐记录处理支持的元信息类型，不能把只读头部称为全部历史目录覆盖。
- [Claude Code CLI 官方存档说明](https://code.claude.com/docs/en/sessions#where-transcripts-are-stored)说明项目文件夹名会替换非字母数字字符、可能截断并加哈希，还允许覆盖名称；不能从目录名可靠还原 cwd。JSONL 是可能随版本改变的内部格式，此说明也不等于 Desktop／VS Code 会话覆盖。
- 原先推荐首批只支持 Codex；用户 T11 已选择同时支持 Codex、Claude Code CLI 和手动目录，因此 Claude Code 的实际字段与所支持版本必须在交付前验证，不能靠文件夹名还原路径。
- 目录规范化建议：保留原始路径和来源，存在的绝对目录规范化后去重；失效、相对或无法解释的路径保留状态，不能按当前进程 cwd 静默补全。父子目录合并保护范围时仍保留各候选来源，避免把多个候选包装成新的更大父目录。

### Claude Code 当前字段证据

- 本机 CLI 版本为 2.1.247；未运行真实 Agent 任务。只读抽样 3 份历史 JSONL 的前 4 条记录，允许字段中均出现 `type`、`sessionId`，每份样本至少一条带 `cwd`、`version`、`timestamp`；cwd 为绝对路径形状。
- 抽样记录的版本与当前 CLI 版本不同，所以不能把这次检查写成“已验证 2.1.247 生成的历史”。采用前需按实际记录版本补齐匿名 fixtures，并报告缺字段／解析失败。
- 探针仅输出字段类型／形状，不输出真实路径、文件名、会话 ID 或正文；JSON 解码需要读入记录，正文未分析。产品适配器只提取目录与来源元信息，跳过其余数据，不持久化原始历史。
- 窄适配器从默认 `~/.claude/projects` 或显式指定的配置根目录发现受支持 JSONL，取实际 `cwd` 字段；缺字段标未知，不反解目录名。嵌套 agent 文件与其他客户端存储尚未验证，应列为覆盖缺口。
- [AgentsView 的 cwd 解析](https://github.com/kenn-io/agentsview/blob/main/internal/parser/claude.go#L318-L338)可作为字段参考，不直接复用会消费正文的完整解析器；[官方会话文档](https://code.claude.com/docs/en/sessions)仍是存储与格式变更约束来源。

## 最小实现建议，供整体核对

以下是承接已确认选择的实现建议，具体依赖版本、字段与后台桥接在真实原型中收敛，不把候选方案记录为已验证能力。

| 部分 | 建议 | 验证重点 |
| --- | --- | --- |
| Rust CLI 与服务程序 | 一个 Rust 工程，按实际职责组织事件接入、规则、SQLite、目录发现与查询 | 不预建多平台空实现、动态插件或 Rust／Swift ABI |
| 输入解析 | serde／serde_json；输入按 eslogger 的 JSONL 处理 | 原始字段缺失、未知事件、坏行、版本变化与有界缓冲 |
| SQLite | rusqlite；可用 bundled 固定随构建的 SQLite | 唯一写入方、事务、短批写、迁移、读查询与保存失败 |
| CLI 参数 | clap 管理目录、服务状态、查询和历史导入命令 | 参数错误、带空格路径、重复导入与幂等配置 |
| 事件传输 | 采集 stdout 仅在内存中转发；后台进程间使用受限的本地 IPC | 权限不同、对端身份、启动顺序、断开与重连，不保存原始全系统输出 |
| 服务生命周期 | launchd 管理采集和分析后台服务；桌面会话中的通知程序独立运行 | 重启、注销、开机、后台 FDA、受控停止与卸载 |
| 系统通知 | 优先验证用户会话中的 AppleScript 通知；需要更稳定身份时再讨论原生通知宿主 | 通知发送失败、权限、会话缺失与汇总补发，不把发送成功当作到屏 |
| 格式与场景测试 | 匿名 JSONL／会话元数据 fixtures，加本机合成项目操作 | 规则测试不替代真实 ES 采集；发送器须在 eslogger 排除的进程组之外 |

SQLite 查询按项目、时间、进程运行身份、文件和告警原因组织；配置、观察事实、告警、覆盖缺口及通知反馈保持区别。记录源时间与收到时间，不能用收到时间冒充内核发生时间。存在可靠运行代际字段时避免只按 PID 关联；没有时明确降级，不能补造身份或事件序号。

采集退出、IPC 中断、解析异常、已知丢失、数据库故障和通知失败分别记录状态。无活动不是健康证明；上游没有序号或心跳时，不能仅凭流量为空宣称“没有丢事件”。告警按进程运行身份与规则在 60 秒内合并，仍须通过正常搜索／索引／构建样本验证。

告警延迟按新告警的首次触发验收；合并期间后续证据及时更新，不要求每条追加活动重发通知。没有桌面会话时只验收后台生成／保存，进入桌面后再验汇总补发。数据库失败期间的告警明确为未保存；此时又无桌面会话或进程重启，不能保证补发证据完整。

## 事件契约与最小存储建议

从真实 eslogger 输出确认以下必要语义后再固定字段编码；来源不能提供的值记录未知，不生成假值。

- 采集来源、系统／工具版本、采集运行实例、事件类型、源时间和收到时间；原生序号存在时保留并检测相应缺口。
- 进程 PID、可用的运行代际身份、父进程、可执行路径与可用签名身份；运行身份缺失时不能仅凭同 PID 宣称是同一次进程运行。
- 文件路径、源／目的路径及截断状态；OPEN 的读权限、MMAP 的可读保护标志、可用文件身份。目录活动不冒充普通文件读取，路径边界不能用字符串前缀误把相邻目录纳入。
- 项目关联、归档工具／输出线索与规则原因；只保存识别工具和相关路径需要的元信息，不保存完整参数或环境变量。
- 观察、告警、保存状态、通知发送反馈与覆盖缺口；没有通知回执不能声称用户已看到。

SQLite 先使用必要的运行／健康、监控目录、观察事件、告警与来源记录；唯一写入方是普通权限的后台分析程序。CLI 和桌面通知通过同一本地服务查询／提交命令，不各自写一套规则或配置。数据库与侧文件限定本地账户权限，保留／清除沿用产品上下文。

只建立当前真实调用所需的事件来源和两种历史格式适配；不用一个通用历史解析器猜所有 Agent，也不创建未使用的扩展框架。

## 实施与验证顺序

1. 采集探针：用合成项目确认 eslogger 的真实字段、读打开／可读映射、进程身份与序号能力；未达到约定口径则记录原因并回到路线讨论。
2. 规则与 SQLite：匿名 fixtures 验证目录边界、文件去重、PID 复用、50／10 秒窗口、60 秒合并、保存失败与回查；本机合成操作验证真实事件与 3 秒目标。
3. 后台运行：验证 launchd、最小权限转发、普通用户分析、启动顺序、恢复、后台 FDA 与桌面通知；重启／未登录场景需取得真实运行证据，不能用配置文件存在代替。
4. 目录发现：分别验证 Codex 初始／每轮目录与 Claude Code 元字段、活动／归档及受支持压缩格式、去重、失效路径、版本缺口、重复导入与手动目录。
5. 完整 MVP：从批量导入目录到后台真实采集、告警、通知与 SQLite 回查；覆盖外部 tar／zip、同进程落盘／内存压缩、正常搜索／索引／构建、断流与故障。

2026-10-03 用户调用 `implement-spec` 后已进入实现，Cargo 基础构建通过；任务图见 [实现约定](implementation-contract.md)。用户运行短时合成采集探针时，eslogger 因责任进程缺少完全磁盘访问返回 `ES_NEW_CLIENT_RESULT_ERR_NOT_PERMITTED`；尚未取得真实事件。完成条件仍包含可复现运行证据，不能把权限错误时的零事件解释为无文件活动。

依赖依据：[serde_json](https://docs.rs/serde_json/latest/serde_json/)、[rusqlite 与 bundled](https://github.com/rusqlite/rusqlite#usage)、[clap](https://docs.rs/clap/latest/clap/)。实际实现版本固定在根目录 `Cargo.lock`。

## Fog

- T1–T13 已明确，已获用户显式实现授权；真实系统验收仍在推进。
- 未安装开源采集器、未核验其实际签名或当前系统运行效果。
- 具体后台 IPC、原生字段、依赖版本与格式兼容在真实原型中收敛；当前逐项选择不等于这些能力已通过。
- 日志保留要求沿用产品上下文；故障行为已确认，具体 schema 与性能须验证。
- 历史 cwd 是候选，不自动证明该路径仍存在或就是项目根目录；Claude Code 跨版本与嵌套记录覆盖仍有缺口。
