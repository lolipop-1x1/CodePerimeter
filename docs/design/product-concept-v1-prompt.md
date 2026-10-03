# Agent Guardian 产品概念图提示词

生成方式：内置 imagegen。用途：产品形态讨论与预览；图中为合成项目和演示数据，不代表已实现或验证的防护。

```text
Use case: ui-mockup
Asset type: A single high-fidelity product concept image for discussing Agent Guardian, a macOS agent egress protection app. This is a preview mockup, not a screenshot of implemented software.
Primary request: Give a Chinese developer a concrete feel for a quiet background security companion: a native macOS menu-bar utility, an expandable code-map workspace, and a clear intervention when suspicious encrypted project upload occurs.

Composition: wide landscape, 16:10, crisp readable product design presentation. One dominant large macOS desktop app window taking about 75 percent of the composition, plus two smaller detached UI surfaces on the right: a menu-bar popover above and a risk intervention panel below. Do not overlap or obscure the main code map. Show the app surfaces at near front-on angle, no perspective distortion, no device photography, no hands. Restrained plain background and subtle natural window shadows. The right-hand risk panel should be large enough for readable text. Designed like a polished practical native developer tool, not a marketing landing page. No giant outside titles.

Style: coherent modern macOS dark appearance, refined graphite surfaces, subtle rounded corners and separators, excellent typography and alignment, SF-style Chinese sans-serif, highly legible restrained blue for known source disclosure, neutral gray for unobserved source, amber diagonal hatching for unknown evidence, amber risk indicators. Calm rather than alarmist. Small native red/yellow/green window controls. Thin functional line icons. No neon hacker aesthetics, no shield mascot, no security scores or fake speedometer, no antivirus scan button. All screens use the same design system.

Main app window:
App name "Agent Guardian"; top subtle status "演示概念". Sidebar includes "保护概览", selected "代码地图", "活动记录", "历史导入", "保护策略"; project "demo-project".
Header "代码地图"; compact scope filters "终端 Agent" and "模型服务 A"; subtitle "当前源码快照 · 自有文本源码". Secondary caption "账号未知 · 本次运行已批准".
Three clearly separated metrics: "已确认外发" with "18.6%", "新增范围" with "2.4%", "重复发送" with "4 次". These are example data for one recipient and one project snapshot, not a leakage score.
Segmented views "外发范围", "文件活动", "写入热点", with "外发范围" active. Show a very prominent real treemap-like atlas, organized into contiguous directory regions named "src", "api", "core", "ui", "tests", each holding numerous varied-size rectangular file tiles (around 45). Mostly neutral tiles; a coherent minority of blue partially filled rectangles for known sent source; a few amber hatched segments for unknown evidence. Some visible simple file names: "auth.ts", "client.ts", "policy.ts", "parser.ts", "main.ts", "map.ts". The map is an understandable code atlas, not arbitrary decorative boxes or a geographic globe. One selected tile has a blue outline.
Below the map a compact legend with exact labels "已确认外发", "未记录", "无法判断"; a small caption "未记录不代表未外发".
A narrow inspector integrated into the main window for selected "auth.ts": "已确认外发 36%", "重复发送 4 次", "打开事件 12", "写入事件 3", a very small source excerpt with colored range highlighting, and a provenance tag "受控出口记录". Opening events are not labeled true reads.
Bottom faint text "演示数据 · 索引与分析在本机完成". Historical import is visibly accessible in the sidebar but is not mixed into confirmed sent statistics.

Detached macOS menu-bar popover:
Tiny shield outline menu-bar glyph, popover title "项目保护", a calm green dot with "已纳管 2 个客户端", an amber row "1 处覆盖缺口", project "demo-project", a button "打开代码地图". Do not state universal/full protection.

Detached risk panel:
Prominent restrained amber icon and heading "外传已暂停".
Title "疑似项目快照上传".
Sender/recipient "桌面 Agent → 服务 B".
Evidence rows "短时访问 184 个文件", "随后发起加密载荷上传".
Distinguish uncertainty with clear text "内容不可检查，源码范围未知"; tiny explanation "行为关联提示风险，不等于确认泄露".
A status strip "本次待发载荷未转发" for this synthetic pre-send-block scenario. Never treat blocked payload as uploaded on the code map, and never show an encrypted archive coverage percentage.
Actions: prominent "保持阻止", secondary "查看证据", quieter tertiary "临时放行…". Small footer "临时放行需确认目的地、时限与上行额度". No permanent whitelist and no automatic allow.
Constraints: all UI copy in simplified Chinese, exact provided labels, no unrelated logos or product names, no actual private incident identities, no claimed encrypted-content decryption, no fake full protection claims, no extra security subscriptions, premium paywalls or marketing slogans. This image should make day-to-day use and the critical interruption legible at first glance. Render text carefully; prioritize main hierarchy and the code map over excessive tiny decorations.
```
