# Domain docs

本仓库采用 single-context 领域文档布局。

## 探索代码前读取

- 根目录 `CONTEXT.md`：领域术语和上下文。
- `docs/adr/`：与当前工作范围相关的架构决策。

文件不存在时静默继续，不提前建议创建。
由 `domain-modeling` 在术语或决策得到明确结论后按需创建。

## 文件布局

- `CONTEXT.md`
- `docs/adr/<NNNN>-<decision-slug>.md`

## 使用领域词汇

任务标题、重构建议、假设和测试名称中的领域概念，
使用 `CONTEXT.md` 定义的术语，避免使用其明确排除的同义词。

需要的概念未被收录时，先判断是否偏离项目已有用语；
若确有缺口，记录下来供 `domain-modeling` 后续处理。

## 明确指出 ADR 冲突

输出与已有 ADR 冲突时，明确指出冲突的 ADR 和重新讨论的原因，
不要静默覆盖既有决策。
