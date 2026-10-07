# 多语言实施图

## Notes

规格已确认；直接在 main 实施。

## Decisions-so-far

- 首批简体中文与英文，同一用户统一偏好；README 默认英文。
- 取消语言贡献功能和指南；集中资源保留后续扩展能力。

- 统一用户偏好使用私有独立配置，保持证据数据库职责；网页使用 i18next，Rust 使用 CLDR 复数，通知语言按次快照。
- [语言核心](issues/01-language-core.md)、[网页](issues/02-web.md)、[原生与 CLI](issues/03-native-cli.md)、[用户文档](issues/04-user-docs.md) 已完成。
- 隔离端到端与两轴代码审查通过；[验收记录](validation.md) 区分代码验证和真实后台验证。

## Fog

- 真实后台双语通知到屏和实际 Mac 重启仍待复验；当前后台未更新。用户已追加授权将本轮多语言改动提交并推送到 main。
