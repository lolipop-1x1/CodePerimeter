# 现有项目、竞品与复用判断

整理日期：2026-10-03。以下为此前 2026-10-02 至 2026-10-03 调研的摘要，依据官方 README、文档、少量关键源码和问题单。能力与成熟度按该次快照解释；本次迁移未更新联网核查，未独立部署竞品或验证本机效果。

## 与核心目标最相关的工程

许可证只记录旧材料实际核查到的值；“待核查”不表示没有许可证。实际抽取、链接或分发前应核对采用版本与随附依赖。

| 项目／官方来源 | 历史材料核查到的能力 | 对源码外传目标的边界 | 复用方向／许可记录 |
| --- | --- | --- | --- |
| [Mac Monitor](https://github.com/Brandon7CC/mac-monitor) | ES 文件／进程事件、关联、过滤及 JSON／JSONL 导出 | 打开不等于实际读取量；没有项目归档语义、HTTP 正文检查与外传控制闭环 | 官方包先采样，源码参考事件映射与关联；BSD-3-Clause |
| [FileMonitor](https://github.com/objective-see/FileMonitor)／[ProcessMonitor](https://github.com/objective-see/ProcessMonitor) | 文件通知及执行／fork／退出采集工程 | 不能直接得出归档内容或上传结论，仍需 ES 运行条件与其他控制路径 | 采集代码参考；旧材料记录 GPL-3.0 |
| [MacAudit](https://github.com/navjotk8690/macaudit) | 整理部分 fs_usage 活动，扫描新归档文件并生成事件 | 后缀、大小阈值、轮询间隔和目录排除可能漏掉短时、小包及内存归档；不阻止上传 | 轻量观察规则参考；MIT |
| [LuLu](https://objective-see.org/products/lulu.html) | 出站连接提示、进程／目的地规则和控制 | 连接放行不等于内容安全，正文、已有连接和真实覆盖分别核验 | 原生外联控制、安装和交互参考；许可采用前核查 |
| [Sanctuary](https://github.com/Hardener-ai/sanctuary) | Mac Agent 分类、敏感目录、审计和菜单栏产品方向 | 核查时 README 为 v0.1 pre-launch，正式签名生产包与完整 ES 执行控制尚未作为已发布能力证明 | 相邻产品形态与采集取舍；AGPL-3.0 |
| [AgentWall](https://github.com/agentwall/agentwall) | MCP 代理／OpenClaw Hook 工具策略与敏感访问后续网络调用关联 | 经过工具入口的活动不覆盖任意客户端内部直读／直传，公开资料明确部分正文和间接数据流限制 | 工具入口策略和解释参考；Apache-2.0 |
| [Pipelock](https://github.com/luckyPipewrench/pipelock) | 出口代理、适用 TLS 检查、正文 DLP、秘密及流量规则 | 请求须经过检查入口；TLS 不解除应用层加密，预算不等于源码覆盖比例 | 请求检查和旁路／资源边界参考；许可采用前核查 |
| [Chopi](https://github.com/danra/chopi) | Mac Seatbelt、主机代理规则及仓库推送白名单 | 包装启动与已确认原入口不同，工作目录和模型主机获准仍可能允许源码经模型请求披露 | 隔离与出口范围取舍参考；许可采用前核查 |
| [Ash](https://ashell.dev/) | 官网描述 ES／NE 文件、网络及进程限制，并展示 ash run | 公开受控启动流程不证明透明覆盖任意已运行桌面客户端，权限限制不等于正文内容检查 | 系统防护底座与使用成本参考；许可采用前核查 |
| [Lasso](https://www.lasso.security/use-cases/ai-coding-assistants) | 官网描述多客户端、工具调用、MCP、运行治理与审计 | 广泛支持声明尚未独立验证，不能推导 Hook 外的客户端内部行为都受控 | 直接竞争方向，需按客户端实际入口比较；许可／条款未核查 |
| [Microsoft Purview Endpoint DLP](https://learn.microsoft.com/en-us/purview/endpoint-dlp-learn-about) | 官方资料列 Mac／Windows 文件活动、受限应用及浏览器／目的域上传控制 | 支持受平台、文件类型和活动影响，不推导任意 Agent API 正文都可检查 | 企业 DLP 能力与范围定义参考；许可／条款未核查 |
| [Forcepoint DLP](https://help.forcepoint.com/dlp/10.4.0/dlphelp/b35abf4d-a2b7-4ffb-8e15-15d0b474d391.html) | 文件指纹、完整匹配与文档片段相似性 | 内容匹配已有基础，不等同于按源码版本、会话、接收方累计范围的产品能力 | 避免把指纹／片段匹配当成算法首创；许可／条款未核查 |

## Mac Monitor 的具体复用结论

旧核查固定到提交 `535933c07a071eeff81c11dfa4125a5911979d15`。它是开源 Mac 安全研究与取证工具，不是已经完成本项目目标的防泄漏引擎。

- 支持 `NOTIFY_OPEN`、`NOTIFY_WRITE`、`NOTIFY_CLOSE`，首次默认订阅未启用；打开模型保留文件和 fflag，不包含实际读取字节。
- 进程关联可帮助整理外部归档迹象，但同进程内存压缩不会以 tar／zip 子进程出现。
- UIPC socket 事件描述本地 IPC，不能当作 HTTP 正文或互联网上传统计。
- `SutroESFramework` target 与应用 XPC、事件模型、Core Data 和 UI 集成，未被确认是可直接替换 UI 的稳定独立 SDK。
- 官方签名发行包可先用于采样；本项目修改版 ES 客户端仍需自己的发布者授权和签名路线。
- BSD-3-Clause 的声明保留及名称背书限制须遵守，随附第三方依赖分别核查；未决定整仓 fork。

固定来源：[README](https://github.com/Brandon7CC/mac-monitor/blob/535933c07a071eeff81c11dfa4125a5911979d15/README.md)、[订阅定义](https://github.com/Brandon7CC/mac-monitor/blob/535933c07a071eeff81c11dfa4125a5911979d15/ProjectSutro/SutroESFramework/Events/Models/EventType/EventSubscriptions.swift)、[LICENSE](https://github.com/Brandon7CC/mac-monitor/blob/535933c07a071eeff81c11dfa4125a5911979d15/LICENSE)、[社区构建脚本](https://github.com/Brandon7CC/mac-monitor/blob/535933c07a071eeff81c11dfa4125a5911979d15/ProjectSutro/Scripts/community-sign.sh)。

建议顺序为官方工具采样、验证事件和项目规则、评估最小采集代码复用，再建设项目策略与出口控制。fork 的价值取决于真实需要和耦合成本，不能省去控制闭环。

## 历史分析与代码地图的相邻工程

此前也调研过 Agent 会话分析。它们适合参考解析、统计和查询，当前不作为新的独立产品方向或核心防护替代。

| 项目 | 已检查材料中的相关能力 | 证据边界 |
| --- | --- | --- |
| [workspace-heatmap](https://github.com/fulf/workspace-heatmap) | 历史挖掘、文件／目录热度、CLI 和 HTML 报告 | Read 工具参数不保证成功读取，未读文档集合有格式范围；不等于系统全部读取或外发 |
| [claude-code-devtools](https://github.com/takattowo/claude-code-devtools) | Claude 历史时间线、文件 Read／Write／Edit／错误计数和回放 | 核查时其他客户端部分 Adapter 在路线图；工具调用计数和错误分别解释 |
| [agentsview](https://github.com/kenn-io/agentsview) | 多客户端档案、检索、Web／CLI、聚合统计及实验性 Recall | 部分分析为实验性；可选模型分析可能发送会话派生内容，“本地档案”不等于所有功能离线 |
| [coding_agent_session_search](https://github.com/Dicklesworthstone/coding_agent_session_search) | 多客户端统一历史检索、CLI／TUI、导出和分析 | 核查时标 alpha，覆盖类术语不一定指源码覆盖，采用前核查许可和具体格式 |

该表承接此前《Coding Agent 历史分析：GitHub 项目调研》的 2026-10-03 材料，属于相邻能力研究。关键源码／文档来源为 [workspace-heatmap 历史解析](https://github.com/fulf/workspace-heatmap/blob/main/src/mine.mjs#L18-L27)、[devtools 文件热度](https://github.com/takattowo/claude-code-devtools/blob/main/packages/server/src/routes/heatmap.ts#L14-L38)、[agentsview Recall](https://github.com/kenn-io/agentsview/blob/main/docs/recall.md#L6-L33) 和 [cass README](https://github.com/Dicklesworthstone/coding_agent_session_search/blob/main/README.md)。这些仍是历史资料核查，不是本次运行验收。

文件热度、历史检索和 Web／CLI 入口已有直接相邻项目。地图的价值需要来自项目范围、来源回溯和清晰证据，不由“做一个热图”推导差异。

## 有价值但仍待验证的差异

1. 让用户看懂一个 AI 应用在选定项目中的访问范围、疑似汇集和后续外联。
2. 用可信归属串联事件，并说明为什么暂停、哪些内容仍未知。
3. 提供及时、范围明确且实际有效的外传控制，同时保持正常开发可用。
4. 将文件活动、会话可见和受控出口证据放在同一项目视图中，保留不同证据等级。

尚未完成竞品运行对照、用户访谈、付费意愿或留存验证。星标数、问题反应数、README 承诺及有限搜索中未发现某功能，都不能用作市场成立或技术独有的证据。

## 采用前仍需取得的材料

固定采用版本、许可证与依赖清单，实际构建／运行、权限和分发条件，以及与本项目合成场景一致的效果证据。对不透明客户端、已有连接、Hook 外行为与加密正文的限制分别核验。

核心可行性见 [feasibility.md](feasibility.md)，复用 Interface 与平台分离见 [整体架构](../architecture/technical-options.md)。
