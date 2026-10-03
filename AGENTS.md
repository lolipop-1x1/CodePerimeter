# 开发约定

- 使用中文回答，代码注释使用中文。
- 开始产品或实现工作前读取 [CONTEXT.md](CONTEXT.md)；任务范围和术语以该文档为准。
- 涉及系统接入、模块 Interface 或平台迁移时读取 [整体架构与技术候选](docs/architecture/technical-options.md)。
- 涉及文件、打包、加密、权限或签名判断时读取 [核心可行性调研](docs/research/feasibility.md)；涉及现有工程复用时读取 [现有项目与复用](docs/research/existing-projects.md)。
- 定义完成标准或报告防护结果时读取 [核心验收方案](docs/validation/core-validation.md)；修改地图或历史分析时读取 [产品形态与指标](docs/design/product-and-metrics.md)。

## 实施原则

- 首先验证 Mac 核心发现与外传控制，依据实证推进后续能力。
- 修改前说明假设、范围和可验证的完成条件；范围取舍由用户确认，日常实现选择在授权范围内推进。
- 使用最少的必要设计，保持修改可追溯到当前需求；保留他人改动。
- 共享规则使用平台无关的模型，系统与客户端差异通过对应 Adapter 表达。
- 结果注明来源、版本、覆盖缺口和未验证项；组件测试、告警和策略决定各自报告，实际防护以执行与接收证据判断。
- 公开文档和测试使用匿名场景与合成数据；提交显式文件清单，推送后核验远端。

## Agent skills

### Issue tracker

任务和规格使用本地 Markdown，存放在 `.scratch/<feature>/`。参见 [本地任务跟踪](docs/agents/issue-tracker.md)。

### Triage labels

采用五个默认分诊标签。参见 [分诊标签](docs/agents/triage-labels.md)。

### Domain docs

采用 single-context：根目录 `CONTEXT.md` 与 `docs/adr/`。参见 [领域文档约定](docs/agents/domain.md)。候选方案记录在架构文档；形成明确技术决策后再增加相关 ADR。
