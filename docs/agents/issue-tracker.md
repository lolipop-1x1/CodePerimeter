# Issue tracker：本地 Markdown

本仓库的任务和规格以 Markdown 文件保存在 `.scratch/` 中。

## 文件约定

- 每个功能一个目录：`.scratch/<feature-slug>/`。
- 规格文件：`.scratch/<feature-slug>/spec.md`。
- 每个任务一个文件：`.scratch/<feature-slug>/issues/<NN>-<slug>.md`。
  从 `01` 开始编号，不将多个任务合并到一个文件。
- 分诊状态写在任务文件顶部附近的 `Status:` 行中，
  状态名称参见 `docs/agents/triage-labels.md`。
- 评论和讨论记录追加到文件底部的 `## Comments` 下。

## 发布到任务跟踪器

在 `.scratch/<feature-slug>/` 下创建对应文件，必要时创建目录。

## 获取相关任务

读取用户指定的任务文件；用户通常会提供路径或任务编号。

## Wayfinder 操作

- 地图：`.scratch/<effort>/map.md`，
  保存 Notes、Decisions-so-far 和 Fog。
- 子任务：`.scratch/<effort>/issues/<NN>-<slug>.md`，
  从 `01` 开始编号，在正文中记录待解决的问题。
- 类型：使用 `Type:` 行记录
  `research`、`prototype`、`grilling` 或 `task`。
- 阻塞关系：在顶部附近使用 `Blocked by: NN, NN`。
  所有阻塞任务均为 `resolved` 后，当前任务才可推进。
- 下一项：按编号选择尚未解决、未认领且无阻塞的任务。
- 认领：开始工作前写入 `Status: claimed` 并保存。
- 解决：在 `## Answer` 下追加结论，设置 `Status: resolved`，
  再将上下文摘要和任务链接追加到地图的 Decisions-so-far。
