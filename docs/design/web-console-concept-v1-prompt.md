# 本机网页控制台概念图提示词

日期：2026-10-06
生成方式：内置 image_gen 工具，生成新图；未使用 CLI 或外部 API 脚本。
用途：已确认的网页控制台规格的视觉概念，不是运行截图或功能验收。

设计参数：DESIGN_VARIANCE 3、MOTION_INTENSITY 1、VISUAL_DENSITY 6。静态概念图不表达实际动效。

信息组织参考 [Carbon 数据表指南](https://www.carbondesignsystem.com/building-blocks/core/components/data-table/guidelines)，不表示已经安装或采用 Carbon 组件库。生成图仅使用匿名路径与合成数据。

## 最终生成提示词

```text
Use case: ui-mockup
Asset type: CodePerimeter local web console product concept, project design documentation.
Primary request: Create one high-fidelity, screenshot-like desktop web interface concept for CodePerimeter, a macOS local file-activity and archive-evidence monitor. Use crisp, legible Simplified Chinese UI typography. Wide landscape 16:10 composition at high resolution. Flat front view, the application fills the frame with a slim neutral browser-style top bar. All visible data is synthetic and anonymous.

Design language: calm professional developer tool, Carbon-informed information organization with a broad data-table area and contextual detail pane. This is design inspiration, not an official Carbon screenshot. Off-white cool neutral surfaces, dark graphite text, subdued jade green #276A5A as the single brand accent. Muted amber is reserved for a real semantic coverage-warning label. No purple gradients, neon, glass, photography, device mockups, oversize headings, decorative textures or marketing hero. Consistent compact spacing, 6px control corners and 8px panel corners; restrained thin separators. Clear Chinese sans-serif similar to Noto Sans SC / IBM Plex Sans, monospace for paths, times and process IDs. DESIGN_VARIANCE 3, MOTION_INTENSITY 1, VISUAL_DENSITY 6.

Composition:
- Fixed narrow left navigation, roughly 15% of width, light gray within the same light theme. Wordmark exactly "CodePerimeter", supporting text "本机监控控制台". Seven links exactly "概览", "监控目录", "文件活动", "归档迹象", "告警中心", "规则中心", "设置与诊断". Select "概览" using a muted jade fill. Minimal consistent outline icons.
- Main header: "监控概览", secondary "文件活动与归档线索", small visible label "示例数据". Top right outlined action "暂停监控", and local scope label "仅本机访问".
- A compact health strip distinctly separates "系统采集：运行中", "证据保存：正常", "通知：已发送，到屏待确认", "覆盖状态：有缺口". One restrained amber coverage badge.
- A horizontal plain metric band, using typography and spacing rather than four floating cards: "监控目录" value "4", "文件活动" value "2,486", "归档迹象" value "18", "待处理告警" value "5". All figures are illustrative sample data.
- Main workspace below: large central area about two thirds of the remaining width, and one contextual alert-detail pane about one third. Central upper region contains "文件活动趋势", scope selector "最近 24 小时", a modest readable time-series line chart with actual axis labels, green solid "打开" series and neutral dashed "可读映射" series. Synthetic irregular activity peaks, no percentages, read-byte volumes or exfiltration metrics.
- Central lower region: "最近活动" table with functional toolbar: search "搜索文件或进程", filters "全部目录" and "全部类型", and "导出". Table columns exactly "时间", "进程", "事件", "文件". Six comfortably readable rows with anonymous relative project paths. Example rows:
"14:32:18" | "tar" | "归档命令迹象" | "project-a"
"14:32:17" | "python3" | "可读映射" | "project-a/src/main.rs"
"14:32:16" | "rg" | "只读打开" | "project-b/src/lib.rs"
"14:31:54" | "zstd" | "归档命令迹象" | "project-b/docs/guide.md"
"14:31:42" | "gzip" | "归档命令迹象" | "project-a/docs/guide.md"
"14:31:28" | "python3" | "只读打开" | "project-a/src/lib.rs"
Select the tar row with a faint jade background. Include unobtrusive pagination beneath the table, and a useful "查看全部活动" link.
- Right contextual pane heading "告警详情". Main title "归档命令迹象", status "待处理". Fields: "进程" value "tar (4208)"; "监控目录" value "/workspace/project-a"; "规则版本" value "3"; "通知反馈" value "已发送，到屏待确认". A compact chronological evidence list uses "命令执行", "项目文件关联", "告警生成" with simple time labels. Section "关联文件" lists exactly "src/main.rs", "src/lib.rs", "docs/guide.md". Short evidence explanation exactly "命令执行是线索，不代表压缩成功或外传。". Below: labeled note field "处理备注", secondary button "标记已读", primary jade button "标记已处理", and link "查看规则".
- A small functional footer, legible and understated: "概念设计，示例数据". Additional evidence note exactly "打开与映射不代表读完文件。".

Constraints: All words must be correctly spelled in Simplified Chinese, match the specified labels, and remain legible. Carefully align the table headers, data columns, chart and detail fields on a precise grid. Use comfortable text sizes and contrast. Paths only use the synthetic /workspace/project-a and project-b examples. No real names, home directories, emails, company identities, URLs, accounts, passwords, tokens, avatars or source-code content. Do not show any interception switch, confirmed compression badge, blocked upload metric, code map, model-chat transcript, developer-signing setup or login form. No em dash or en dash characters. Maintain one consistent light theme. This is a proposed UI concept, never claim it is an implemented or tested live product.
```
