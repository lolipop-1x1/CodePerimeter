# 核心目标的可行性与边界

整理日期：2026-10-03。研究依据为此前 2026-10-02 至 2026-10-03 的官方资料、源码核查和限定合成实验。本次为材料沉淀，未重新部署竞品、采集用户会话、监控真实客户端或测试系统拦截。

## 结论与证据等级

可以继续验证有边界的防护：观察项目活动，解释疑似收集／打包行为，并对确实纳管的外传路径实施批准与控制。任意客户端在任意加密、编码和协议下无感兼容且保证代码不外泄，没有已有证据支持。

| 等级 | 当前材料能支持什么 |
| --- | --- |
| 官方资料／源码核查 | API 及已检查项目声明或实现的能力，不证明本机效果 |
| 历史合成实验 | 指定发送器、网关和接收端的结果，不证明真实客户端已纳管 |
| 技术推论／建议 | 由能力边界得出的风险与候选路线，仍需验证 |
| 未验证 | 客户端覆盖、事件完整性、性能、误报、实际阻止和故障维持 |

## 文件访问能否监控

macOS Endpoint Security 提供文件打开、映射、写入和进程等事件，部分授权事件可决定是否允许操作。它可以为“哪个进程访问了哪些项目路径”提供系统证据。打开事件的路径与标志不证明读取了全部内容，也不是逐次 read() 或读取字节计量。

此前核查 Mac Monitor：支持打开／写入／关闭，但首次默认订阅未启用这些事件；需要启用相应 trace。事件来源、观察窗口、缓存、已有句柄、映射读取和丢事件都影响解释。尚未在本机验证其完整性。

FSEvents 面向文件系统变化，不是纯读取监控。普通目录 watcher 看到创建／修改不能推出全部读取；Hook 只覆盖经过该客户端工具入口的操作，内部库与后台行为另验。

参考：[ES](https://developer.apple.com/documentation/endpointsecurity)、[open 事件](https://developer.apple.com/documentation/endpointsecurity/es_event_open_t)、[事件类型](https://developer.apple.com/documentation/endpointsecurity/es_event_type_t)、[FSEvents](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/Introduction/Introduction.html)。

## 打包能否发现

可以关联外部 tar／zip 等命令、临时文件生成、归档文件访问和批量项目活动，形成可解释线索。没有通用系统事件直接表示“这个进程正在把整个项目压缩并准备上传”。

| 变体 | 可能的线索 | 主要缺口 |
| --- | --- | --- |
| 外部命令生成落盘归档 | 执行、文件创建／写入、路径及后续打开 | 命令可能用于正常交付；文件可能含其他内容 |
| 同进程库生成落盘归档 | 项目访问与文件生成 | 不会出现独立 tar／zip 进程 |
| 内存归档、压缩与加密 | 访问范围、进程活动、随后外联 | 无归档文件或后缀，缺少压缩／加密语义 |
| 小型高压缩快照 | 可能仍有大范围文件活动 | 上行很小，大流量阈值可能完全不触发 |
| 增量、逐文件或慢速分批 | 跨窗口累计范围与接收方 | 固定短窗口和单次体积规则可能漏检 |

批量搜索、索引和构建会产生相近活动。文件范围、时间、可信进程关联、接收方与正文线索应共同使用；风险关联仍不证明载荷内容同一。参考 [Apple Compression](https://developer.apple.com/documentation/Compression)。

## 网络控制和内容检查

系统连接控制、正文检查与接收证据分别解决不同问题：

| 外传路径 | 有条件可验证的行为 | 前提与限制 |
| --- | --- | --- |
| 独立未批准服务 | 在覆盖的连接上拒绝，无需解密项目快照 | 身份、子进程、已有连接与故障持续控制确实有效 |
| 获准服务的不同接口 | 按可见请求接口与格式检查 | 域名规则不足，需适用正文／HTTP 检查入口 |
| 合法模型请求中的大量明文源码 | 匹配项目范围、秘密与批量规则 | 实际请求经过检查，项目版本与解析覆盖明确 |
| 合法文本字段内的密文 | 批准门槛或风险暂停可以起作用 | 内容匹配无法可靠确定任意密文中是否为源码 |
| 大量访问后上传 | 提示关联风险并暂停后续发送 | 时序可能滞后；已发内容不能追回 |

普通 HTTPS 过滤能作流量允许／丢弃决定，不自动看到完整 HTTP 正文。代理配置或环境变量不保证客户端全部请求经过代理，直连、旧连接、helper、其他协议和重试都要验证。详见 [Apple 网络过滤说明](https://developer.apple.com/videos/play/wwdc2025/234/)、[NEFilterDataProvider](https://developer.apple.com/documentation/networkextension/nefilterdataprovider)。

连接放行不等于内容获准。网络字节包括协议、重复和非源码数据，不能换算成源码上传比例。阈值只能作为预算或风险规则，触发后阻止属于止损。

## 应用层加密的决定性边界

```mermaid
flowchart LR
    A[项目文件] --> B[归档或压缩]
    B --> C[应用层加密]
    C --> D[HTTPS]
    D --> E[适用的本机 TLS 检查]
    E --> F[正文仍可能是应用层密文]
```

TLS 检查解除传输层保护，不自动取得客户端加密快照的密钥。高熵、Base64 和体积异常不是源码内容证明。客户端若可读取全仓并向获准模型通道提交任意文本，仍具备逐步或编码披露的能力。

旧合成实验将 32 个文件压缩并加密成 335 字节，再放入合法模型文本字段。未批准时拒绝；明确批准该请求后，接收端收到正文并恢复全部文件。它证明批准门槛，而不是通用密文识别，详见 [历史实验](../lessons-learned.md)。

如需更强保证，可评估缩小可读范围、可信组件决定允许提交的内容，或受控启动／隔离等方案；它们改变访问或使用体验，不能自动替代已确认的产品入口。层次依据：[TLS 1.3](https://www.rfc-editor.org/rfc/rfc8446.html)、[Pipelock TLS 文档](https://github.com/luckyPipewrench/pipelock/blob/main/docs/guides/tls-interception.md)、[旁路边界](https://github.com/luckyPipewrench/pipelock/blob/main/docs/bypass-resistance.md)。

## 权限、签名与分发

| 角色／路线 | 历史核查结论 |
| --- | --- |
| 普通用户安装官方发行包 | 此前已核查的 Mac Monitor 官方包由作者签名、公证，用户不需要自己的开发者账号；仍按产品要求批准系统扩展等权限 |
| 本项目修改或新增 ES 客户端 | 发布者须取得相应 Apple entitlement 和签名／分发能力；不能继承原作者身份 |
| 原生网络扩展路线 | 按所选扩展类型和分发方式满足 Apple 授权、签名与配置要求，具体方案尚未验证 |
| 无自有证书的初步观察 | 可先使用官方签名工具采样，或评估系统诊断工具；工具可见性和所需权限另验 |
| 关闭系统保护的测试环境 | 属于受限开发实验，不作为普通用户安装路线 |

Mac Monitor 的 community 脚本说明无证书产物限于关闭 SIP 且配置特定启动参数的虚拟机。fork 并不消除 ES 授权与发布者职责；其 framework 也不是已经验证的独立 SDK。

来源：[Mac Monitor 安装与构建](https://github.com/Brandon7CC/mac-monitor/tree/535933c07a071eeff81c11dfa4125a5911979d15)、[ES entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.endpoint-security.client)、[NE entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.networking.networkextension)、[System Extensions](https://developer.apple.com/documentation/systemextensions/)、[Developer ID](https://developer.apple.com/developer-id/)。

## 是否像 DLP，是否值得做

定位接近面向 AI 开发流程的终端 DLP、应用出口控制和行为监测组合。文件指纹、内容匹配、目的地控制和事件审计已有成熟工程；“安全卫士”类别本身不是独有优势。

值得先验证的价值是项目范围解释、可解释的访问／打包／外传关联、及时有效的控制和正常开发兼容。已有资料没有完成用户访谈、付费意愿或市场规模验证，不能由有限竞品检索推导商业成立。

后续按 [核心验收方案](../validation/core-validation.md) 取得禁止载荷受控、正常任务完成和故障控制证据，再推进产品化。只观察到文件活动时，如实交付观察结论；完整拦截仍是需要证明的目标。
