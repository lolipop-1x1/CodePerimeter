# ZCode 历史目录发现与导入：需求讨论

日期：2026-10-05
Status: resolved
Type: grilling

## Notes

- 用户调用 `grill-with-docs`，要求讨论 ZCode 兼容；本轮结合 `grilling` 与 `domain-modeling`，先确定兼容的含义。
- 沿用根目录 [产品上下文](../../CONTEXT.md) 的项目目录候选、历史目录来源和监控目录集合。历史来源与运行时进程归属分别定义。
- 规格见 [最小规格](spec.md)，实现任务见 [01 ZCode 历史目录适配](issues/01-zcode-history-adapter.md)，完成证据见 [验收记录](validation.md)。

## Decisions-so-far

- Q1 已确认 A：只增加 ZCode 历史项目目录发现与导入。导入后沿用现有所有进程的文件监控；实际运行 ZCode 的访问／打包验收和专属进程身份归属不纳入本轮。
- Q2 已确认 A：先导入明确保存的会话目录，显示会话中途目录变化尚未验证的覆盖缺口，不宣称完整还原所有历史工作目录。
- 沿用现有规则：先预览再显式选择；检查目录状态和规范路径；跨来源去重并保存来源；新会话不自动扩大已选集合。
- 只读取目录与必要来源元信息，不分析或保存聊天正文。公开文档与测试使用匿名场景和合成 SQLite 数据。
- 不新增 ADR：本轮沿用现有历史来源适配，未形成需要单独记录的高成本架构取舍。
- 2026-10-05：[实现任务](issues/01-zcode-history-adapter.md) resolved；组件回归、本机只读预览、release 构建及 [双轴审查](code-review.md)均通过。

## 设计树

- ZCode 兼容
  - 兼容范围（Q1，已确认 A）
    - 历史目录发现、预览、选择导入和来源持久化。
    - 复用系统采集与告警规则。
  - 目录覆盖（Q2，已确认 A）
    - 以明确保存的会话目录为依据。
    - 会话中途目录变化保留未验证状态。
  - 既有目录入口契约（沿用已确认规则）
    - 手动目录、跨来源去重、路径状态、导入后持久化。
    - 新历史不自动扩展监控范围。

## 事实核查

- 本机 `/Applications/ZCode.app/Contents/Info.plist`：ZCode 3.8.1，build 3.8.1.5310，bundle id `dev.zcode.app`；核查未启动应用。
- 本机 `~/.zcode/cli/db/db.sqlite` 的 `session` 表有 `directory`、`path`、`version`、`project_id`、`workspace_id` 等字段。只读 schema 与允许字段形状核查未输出真实项目路径，也未查询消息正文。
- 本机有 SQLite WAL。此次 `immutable=1` 抽样不能证明包含尚未 checkpoint 的最新记录；实现必须使用能读取 WAL 的正常只读快照，不能照搬该抽样方式。
- `directory` 是本轮目录候选的起点；`path` 与 `version` 的具体语义尚未证明，不将它们直接解释为工作目录或桌面应用版本。
- [官方存储说明](https://github.com/zai-org/ZCode/blob/main/NOTICE.md)与 [官方 README](https://github.com/zai-org/ZCode)说明运行形态和数据目录配置会影响存储位置；本机观察不是稳定公开历史 API 承诺。

## Fog

- 会话中途目录变化与远程工作区目录语义未验证；只能检查候选是否解析为本机目录，不凭历史记录推导执行主机或进程归属。
- 其他 ZCode 版本和自定义数据根目录尚未验证；默认位置之外通过显式数据库参数支持，不推导全版本兼容。
- 解析器、CLI、匿名组件、本机只读预览及独立源码审查已完成。目录发现结果不推导真实 ZCode 任务已经完成文件活动或压缩验收。

## Comments

- 2026-10-05：用户回复“1. A，2. A”，确认本轮兼容范围及目录覆盖边界。
- 2026-10-05：用户调用 `implement-spec`，按已确认主干约定在 `main` 实现；4 个可用本机候选仅预览，未加入真实监控配置。
