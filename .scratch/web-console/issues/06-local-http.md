# 本机 HTTP、CLI 与导出适配

Status: resolved
Type: task
Blocked by: 03, 04, 05

## 范围

按 api-contract.md 开发 axum 回环 HTTP、入口令牌与同源校验、受限宿主代理、后台授权操作、历史快照勾选导入、完整分页流式 JSON／CSV 及匿名模式、codeperimeter ui 与内嵌网页资源。可先按已固定契约并行开发；集成完成依赖03–05。

## 验证

实际 HTTP 鉴权、Origin／Host、体积边界、导出超过100条且无公式注入、宿主不可用明确状态、UI关闭与采集独立、普通用户权限和资源生命周期。

## Comments

- 2026-10-06：主 agent 负责 web.rs、main/lib、Cargo依赖及HTTP/CLI集成测试。
- 2026-10-06：生产 HTTP、普通用户宿主与 SQLite 的 4 项集成测试通过，含认证／同源隔离、完整分页导出、三个历史来源导入、宿主失联以及 CLI 后台复用。取消历史扫描仍保留并发配额、匿名身份关联和 CSV 公式防护回归通过；真实系统授权由07继续验收。
