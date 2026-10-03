# 文件访问与压缩迹象监控 MVP：需求讨论

日期：2026-10-03
Status: claimed
Type: grilling

## Notes

- 2026-10-03：用户显式调用 `implement-spec`，进入实现；后续任务与完成证据见 [实现任务图](implementation-contract.md)，早期等待确认记录作为讨论历史保留。
- 本轮按 `grill-with-docs`，结合 `grilling` 与 `domain-modeling` 讨论最小 MVP。
- 用户明确要求先聚焦文件读取监控、压缩监控与及时发现。
- 已有项目上下文仍保留未经批准外传控制的完整产品目标；本轮讨论第一阶段的观察与发现交付。
- 需求讨论阶段完成官方资料与现有文档的只读核查；当前已进入实现，结果由实现票据和运行验收记录。
- 术语沿用根目录 `CONTEXT.md`：文件活动、工具读取、疑似打包、未知。文件活动不能直接等同于完整内容读取，疑似打包不能直接等同于外传。

## Decisions-so-far

- 当前迭代优先围绕文件访问、压缩／归档迹象和及时发现定义最小交付。
- macOS 首先验证；沿用用户选定目录、本地分析和必要事件记录的既有方向。
- Q1 选 A：观察选定目录内所有进程的文件活动，并显示来源身份；不将普通进程活动默认归为 AI 风险。术语已同步根目录 `CONTEXT.md`。
- Q2 选 A：第一份交付为本机运行的轻量验证版，接受命令行启动和必要授权；安装应用与菜单栏不列入本次最小交付。
- Q3 选 A：接受文件打开／映射等访问证据，明确标注事件类型，不承诺实际读取内容或字节量。
- Q5 选 A：采用终端记录与 macOS 系统通知；普通文件活动只记录，批量访问与疑似打包汇总提醒。
- Q4 补充确认选 A：把同进程读取后内存压缩列入必测，验收访问记录与批量提醒，不宣称确认压缩动作。
- Q6 选 A：疑似打包触发事件发生或批量门槛达到后 3 秒内生成终端告警并发出系统通知，作为待实测的初版目标。
- 已将确认范围与候选规则整理到 [最小规格](spec.md)，整体确认前不开始实施。
- 2026-10-03 开始 [技术讨论](technical-design.md)：已确认 Rust CLI 核心、仅采集器 root、SQLite、后台常驻与自动启动、50 文件／10 秒原型值。后台启动时机和采集入口仍在讨论。
- 技术轮进一步明确系统启动即监控、不依赖终端或桌面会话；优先验证 eslogger，进入桌面后补发未展示告警汇总。新增手动多个目录与历史候选批量导入，集合持久化，新会话不自动扩大范围；历史来源与故障细节见技术讨论。
- 技术轮 T1–T13 已逐项明确：首批 Codex／Claude Code CLI 分别适配，Codex 包含每轮目录变化，60 秒合并告警；SQLite 失败时继续发现并显示保存缺口。整体技术基线等待确认，未开始实现。

## 设计树

- 文件访问与压缩迹象监控 MVP
  - 监控对象（Q1，已确认 A）
    - 所有进程访问选定目录，显示进程身份。
  - 第一份可运行交付（Q2，已确认 A）
    - 本机轻量验证版，可接受命令行启动和必要授权。
  - 文件活动的证据口径（Q3，已确认 A）
    - 接受带读权限的打开、可读映射等文件访问证据，不承诺实际读取内容或字节量。
  - 压缩／归档的覆盖（Q4，已确认 A）
    - 覆盖外部归档命令与落盘归档迹象；纯内存归档只展示可见访问线索与缺口。
    - 同进程读取后内存压缩列入必测，按访问记录与批量提醒验收。
  - 提醒方式（Q5，已确认 A）
    - 普通活动进入终端记录，批量访问或疑似打包汇总提醒，采用终端与系统通知。
  - 及时发现（Q6，已确认 A）
    - 触发事件发生或批量门槛达到后 3 秒内生成终端告警并发出系统通知；尚未验证。
    - 系统通知发出与用户看到通知分别说明。
  - 最小验收与候选规则（已整理，待整体确认）
    - 合成读取、外部打包、内部落盘、内部内存压缩与正常操作对照，见 `spec.md`。

## Fog

- 已核查采集入口与权限的文档要求，尚未安装工具或申请系统授权，本机授权状态未知。
- 批量访问阈值提出 10 秒内 50 个不同文件的可调原型值；尚未通过样本校准，不作为正式默认或压缩识别保证。
- 用户已显式授权实现；采集完整性、实际通知、性能和误报仍未验证。

## 事实核查

- Apple 的 [open 事件](https://developer.apple.com/documentation/endpointsecurity/es_event_open_t)记录目标文件和打开标志；[mmap 事件](https://developer.apple.com/documentation/endpointsecurity/es_event_mmap_t)记录映射事实。这些事件不证明实际读取内容或字节量。
- [ES 事件类型](https://developer.apple.com/documentation/endpointsecurity/event-types)可支持进程执行与文件活动的关联；外部命令执行不证明成功打包，纯内存压缩没有可据此直接确认的通用系统事件。
- `eslogger` 可作为本机观察验证的候选，需要 root 与完全磁盘访问；其手册不将输出承诺为应用集成接口。Mac Monitor 官方包可作为另一验证候选，需系统扩展与完全磁盘访问授权。
- 验证入口尚未选定，资料核查不等于本机采集通过。事件完整性、丢失、延迟、性能和正常操作误报仍需实测。
- 内存压缩前的文件打开／映射若发生在监控期间，可成为访问线索；监控前已读入应用内存、复用已有句柄／映射或发生事件丢失时，不能保证出现新的打开／映射事件。
- 正常搜索、索引、构建也可能产生批量访问；只有访问证据时提醒应描述该活动，不宣称压缩或泄露已确认。

## 技术追问：监控与拦截

- 2026-10-03：用户询问监控是否可实现，以及能否发现压缩后拦截。本轮核对官方系统接口，未将拦截自动加入 MVP，也未运行拦截实验。
- [Apple ES 说明](https://developer.apple.com/videos/play/wwdc2020/10159/)区分异步 NOTIFY 与同步 AUTH：前者不会等待观察程序，后者可在支持的操作前允许或拒绝，并有响应期限。3 秒告警目标不能推导为发送前阻止。
- 外部归档程序可按策略在 `AUTH_EXEC` 拒绝启动，或在 `AUTH_OPEN` 拒绝其打开项目文件；这是程序执行／文件访问控制，不是通用压缩内容识别。
- 应用内部纯内存压缩没有本轮核实到的通用系统压缩授权事件；限制后续文件访问不能收回已经读入内存的内容。发现后暂停进程属于可能存在竞态的止损，不能保证压缩或外发尚未完成。
- [Network Extension](https://developer.apple.com/documentation/NetworkExtension/content-filter-providers)可作为后续受控外传的执行候选；已有连接、归属、执行时机与故障行为仍须独立验收。
- 自研 ES 控制组件须取得 [Apple ES entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.endpoint-security.client)，按发行路线完成签名与部署。系统采样工具的观察能力不等于本项目已经获得同步控制能力。
- 用户补充目前没有开发者签名。已只读核查本机 `eslogger` 手册与 `--list-events`，当前支持 `open`、`mmap`、`exec`、`create`、`write`、`rename` 等观察事件；尚未启动实际采集。
- [Apple eslogger 介绍](https://developer.apple.com/videos/play/wwdc2022/110345/)确认它随系统提供且已有 ES 授权；本机观察验证无需项目自己的开发者证书，但仍需要 root 与责任应用的完全磁盘访问授权。
- 本机手册明确 `eslogger` 不支持 AUTH 事件，排除自身进程组事件，并不承诺应用接口、性能或结构稳定性。因此可作为当前本机验证入口，不能据此宣称系统拦截或正式发布路线已成立。

## 开源复用追问：Mac Monitor

- 用户询问能否使用此前调研的开源项目，本轮按此前重点讨论的 Mac Monitor 核查。
- [官方 README](https://github.com/Brandon7CC/mac-monitor)提供已公证发行包，安装者批准系统扩展与完全磁盘访问即可使用官方采集能力，无需本项目自有开发者身份；本轮未安装或校验发行包。
- 官方工具可用于文件／进程活动采样与 JSON／JSONL 导出分析；项目范围聚合、批量规则、通知与 3 秒延迟仍需要自己的实现与实测。
- [社区构建说明](https://github.com/Brandon7CC/mac-monitor/wiki/Community-Development)将无作者证书的 adhoc 构建限制于关闭 SIP、放宽 AMFI 的虚拟机，并明确不得分发；fork 或修改扩展不会自动取得作者的签名身份。
- 已核查固定提交 `535933c07a071eeff81c11dfa4125a5911979d15`：ES 回调经内部 XPC 的 `surfaceEvent` 实时送到 Mac Monitor app，但 [XPC listener](https://github.com/Brandon7CC/mac-monitor/blob/535933c07a071eeff81c11dfa4125a5911979d15/ProjectSutro/SecurityExtension/XPC/RCXPC.swift#L84-L115)校验 Mac Monitor 的签名标识、证书与团队身份，不能直接作为 CodePerimeter 的通用第三方事件入口。
- 未核查当前主分支是否改变上述接口，未在本机安装或运行发行包。

## 开源命令行采集器候选

- 本轮进一步核查此前调研的 FileMonitor／ProcessMonitor。[Objective-See 官方产品页](https://objective-see.org/products/utilities.html)明确：它们以满足签名、公证要求的应用包分发，实际为 CLI 工具，必须保留应用包内的签名与 provisioning profile。
- 官方 CLI 向标准输出持续输出 JSON 事件，适合作为外围规则程序读取的候选；使用官方未修改发行包无需本项目自有开发者证书，仍须 root 和运行终端的完全磁盘访问授权。
- FileMonitor 提供文件活动，ProcessMonitor 提供进程执行、fork、退出及来源信息；批量规则、归档关联和通知仍由 CodePerimeter 实现。原始输出可能含完整参数与身份字段，持久化时按 MVP 隐私范围筛选。
- 修改或链接这些 ES 库进入自研采集程序仍需自己的 ES 运行条件。该候选仅支持观察路线，本轮未验证 mmap／读取标志覆盖、当前系统兼容、发行包实际签名、输出延迟或 3 秒提醒。
- 后续源码核查已确认上述覆盖缺口：FileMonitor CLI 不输出 OPEN 的 `fflag`，没有 mmap；ProcessMonitor 不能补齐。不能把这组官方工具直接视为满足完整访问口径，详见 [技术讨论](technical-design.md)。
