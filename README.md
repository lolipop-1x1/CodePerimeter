# CodePerimeter · 代码门卫

<img src="web/public/codeperimeter.svg" alt="CodePerimeter 盾牌图标" width="64" height="64" />

macOS 本机项目文件活动观察工具。通过本机网页控制台或 Rust CLI 配置多个目录，记录文件打开／可读映射及来源进程，发现批量访问、外部归档命令和关联的归档输出，保存 SQLite 证据并发送系统通知。

文件打开／映射是访问证据，不能证明读完了文件。批量读取不能确认程序在内存中压缩，归档迹象也不能证明源码已经外传。外传拦截属于后续产品目标。

## 当前状态

本机网页控制台提供概览、监控目录、文件活动、归档迹象、告警中心、规则中心、设置与诊断七个页面。网页和后台监控独立，数据仍由普通用户宿主统一管理。网页链路已验证实际管理员授权、ES／FDA、暂停／恢复、控制台退出后继续采集和通知到屏；归档输出及时性仍有失败样本，重启及进入桌面补发待验证。组件与浏览器测试、真实系统证据分别记录在 [网页控制台验收](docs/validation/web-console-validation.md)。下述既有 CLI 验收不能替代新增网页链路验收。

CLI、窄事件适配、规则、SQLite、历史目录发现、权限分离和 launchd 管理已实现。2026-10-05 在 macOS 15.6.1 完成真实 eslogger → Rust → SQLite／通知发送验收：16 个合成场景全部通过，22 条告警生成与通知发送均在 3 秒内，最大分别为 2.255 秒和 2.541 秒，本轮已知丢弃及保存缺口为 0。桌面通知到屏、后台 FDA、注销／登录补发、重启和持续性能仍待按 [MVP 验收](docs/validation/activity-monitor-mvp.md) 单独验证；sent 回执不代表到屏，短时通过不保证长期覆盖。

归档命令解析已扩展为默认识别 tar、bsdtar、gtar、zip、ditto、gzip、pigz、bzip2、pbzip2、xz、zstd、7z、7zz、rar 共 14 个名称。用户只选择监控目录，无需逐工具配置。支持常见直接路径及工具允许的标准输出模式；未知参数、列表文件与缺少项目来源的标准输入保留覆盖缺口。2026-10-06 全部 92 个真实采集场景通过，34 正例告警生成／发送反馈最大 1.373／1.717 秒，58 负例无项目归档命令告警；健康、隐私和清理通过。文件活动屏障接收仍有最长约 20 秒延迟，不能由命令告警时效推导文件活动及时。版本与覆盖边界见 [归档命令验收](docs/validation/archive-command-coverage.md)。

系统采集使用 macOS 自带 `/usr/bin/eslogger`，需要管理员权限与责任进程的完全磁盘访问。root 仅运行采集／转发；分析、SQLite 和通知使用普通用户。原始全系统 JSON、完整命令行、环境变量和文件正文不落盘。当前订阅 NOTIFY 事件，支持本机观察，不提供压缩或网络拦截。

## 构建与运行

源码构建需要 macOS、Rust 1.88 或以上、Node.js 24 及 Xcode Command Line Tools 中的 Swift；验收脚本另需 Python 3、系统 tar／zip。先构建网页资源，再构建 Rust，网页与原生通知助手会嵌入最终二进制，运行时无需 Node.js、Swift 编译器或独立前端服务。CI 固定 Rust 1.88.0 和 Node.js 24，检查网页类型、测试、构建产物一致性，并保留 fmt、严格 clippy 与 Rust 测试。

```sh
npm --prefix web ci --ignore-scripts --registry=https://registry.npmjs.org
npm --prefix web run build
cargo build --release --locked
./target/release/codeperimeter --help
./target/release/codeperimeter ui
```

依赖安装禁用生命周期脚本，包括 Carbon 安装遥测。页面仅监听本机回环地址；`ui` 以普通用户启动或复用控制台，并打开默认浏览器。关闭浏览器或启动命令所在的终端，不会停止后台采集。入口失效时再次运行 `codeperimeter ui`，不要复制带入口凭证的地址分享。

首次使用，在“设置与诊断”中安装并启用后台服务。管理员密码只在 macOS 系统授权窗口输入，网页不接收；可以在系统窗口取消。安装成功或采集任务已加载不表示真实事件采集正常。完全磁盘访问需要在系统设置中确认，网页提供指引并展示实际采集健康。本机构建的主程序采用 ad-hoc 签名，更新后可能需要重新授予既有权限，并可能保留同名历史项；这些项不表示监控进程重复运行。未安装或宿主不可用时，网页仍可打开并展示安装／恢复入口，查询失败不会显示成零记录。

系统告警使用 CodePerimeter 名称和盾牌图标的原生本地通知。首次启动通知程序时，在 macOS 弹窗中允许通知；之后可在“系统设置 → 通知 → CodePerimeter”调整。通知程序不需要完全磁盘访问权限，关闭通知不影响采集和告警记录。系统接受发送与用户实际看到分别验证。通知按行为显示“大量访问项目文件”“启动项目压缩／打包命令”或“发现疑似压缩文件生成／修改”，正文附程序名、项目名及已关联的短输出文件名。点击通知或“查看详情”打开对应告警；摘要和旧通知打开告警中心。控制台未运行时自动启动，点击不自动标为已读／已处理。记录过期、清除或尚未保存时，详情会说明不可用。原生点击的现场验证进度见上述验收文档。

在“监控目录”中输入路径、使用原生目录选择，或者预览 Codex／Claude Code／ZCode 候选后勾选导入。停用一个目录会排除其整个子树，优先于启用的父目录；移除前显示范围变化。目录配置操作不会删除项目文件或原始会话文件。

“暂停监控”只暂停系统采集，历史查询与目录／规则管理继续可用；恢复操作重新加载采集，并单独显示实际健康。暂停状态按 launchd 与宿主意图交叉判断，设计为跨重启保留，真实重启行为仍需验收。卸载默认保留配置与记录；卸载后没有管理宿主时，历史查询暂不可用，重新安装并启用后可继续查看。

| 页面 | 可以执行的操作 |
| --- | --- |
| 概览 | 分别查看服务、采集、保存、通知和覆盖状态，查看趋势及目录／进程汇总 |
| 监控目录 | 手动添加、原生选择、三个历史来源勾选导入，启用／停用／移除 |
| 文件活动 | 实时与历史筛选、完整分页、进程与事件详情、关联告警 |
| 归档迹象 | 查看默认 14 个命令的覆盖说明、命令／输出线索和关联证据 |
| 告警中心 | 已读、人工处理、备注及处理历史；新证据重新打开告警 |
| 规则中心 | 三类内置规则独立开关、四项全局参数、即时生效及历史版本解释 |
| 设置与诊断 | 服务操作、权限与采集缺口、保留期、完整导出、分别清除明细／累计统计 |

记录页导出遵循当前筛选范围，包含全部分页，支持 JSON／CSV。默认匿名分享模式替换路径与进程身份，并删除处理备注等自由文本；完整模式保留原字段，只应在本机按需使用。下载由浏览器保存，数据不写入源码目录。明细默认保留 30 天，可设置保留期；缩短保留期和清除数据前需要确认影响。累计统计单独清除，目录与规则配置保留。

如果使用 CLI，可先查看服务计划，再安装和启动：

```sh
./target/release/codeperimeter service plan --user "$(id -un)"
```

服务计划显示三角色、安装路径和数据位置。核对后显式安装并启动；install 只写入安装文件，start 才启用并加载服务，随后检查实际采集授权和运行状态：

```sh
sudo ./target/release/codeperimeter service install --user "$(id -un)"
sudo ./target/release/codeperimeter service start --user "$(id -un)"
./target/release/codeperimeter status
./target/release/codeperimeter watch add /absolute/path/project-a /absolute/path/project-b
./target/release/codeperimeter events --limit 50
./target/release/codeperimeter alerts --limit 50
./target/release/codeperimeter health --limit 100
./target/release/codeperimeter health --source-run-id "实际run_id" --limit 100
```

首次历史导入先生成快照，再选其中的目录。Codex、Claude Code 与 ZCode 的目录元信息分别适配，新会话不会自动扩大范围：

```sh
./target/release/codeperimeter history preview --output candidates.json
./target/release/codeperimeter history import --preview candidates.json --index 0
./target/release/codeperimeter watch list
```

ZCode 默认只读 `~/.zcode/cli/db/db.sqlite` 中明确保存的会话目录，也可用 `history preview --zcode-db FILE` 指定数据库。本机格式基线为 ZCode 3.8.1；会话中途目录变化未验证，预览会显示覆盖缺口。来源标签说明目录出处，不表示运行时进程已归属到 ZCode。

完整参数、查询、阈值、停止和卸载见 [CLI 使用说明](docs/usage.md)；三角色、root 二进制、FDA 边界见 [后台服务](docs/service.md)。停止／卸载保留数据库和目录配置。

## 验证

```sh
npm --prefix web ci --ignore-scripts --registry=https://registry.npmjs.org
npm --prefix web run typecheck
npm --prefix web test
npm --prefix web run build
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -B -m unittest discover -s tests -p test_synthetic_sender.py -v
python3 -B scripts/validate-selftest.py
python3 -B scripts/validate-archive-selftest.py
```

真实完整验收使用新建的匿名项目和独立普通用户数据库，不导入真实历史或触碰既有服务配置。先阅读 [运行步骤与证据口径](docs/validation/activity-monitor-mvp.md)，在自己的终端授权后显式运行：

```sh
sudo -v
python3 -B scripts/validate-prepare-collector.py --binary target/release/codeperimeter
python3 -B scripts/validate-mvp.py --binary target/release/codeperimeter
```

准备入口仅复制本账户的 root-owned 二进制，默认拒绝已有目标；无 launchd 安装且没有活动端点／进程的验收副本可按文档用 `--replace-sha256 <明确旧hash>` 安全替换。它不安装 launchd、不改变 FDA。完整入口保存本地 `summary.json` 路径，真实采集失败会退出并记录原因，不回退 fixture。执行现有服务停止、替换版本和 FDA 授权前，按验证文档检查当前状态。

## 项目文档

| 文档 | 用途 |
| --- | --- |
| [产品上下文](CONTEXT.md) | 已确认目标、范围和术语 |
| [MVP 规格](.scratch/activity-monitor-mvp/spec.md) | 当前观察版本范围与完成条件 |
| [技术基线](.scratch/activity-monitor-mvp/technical-design.md) | 已确认技术选择与覆盖边界 |
| [MVP 验收](docs/validation/activity-monitor-mvp.md) | 组件、真实事件、时延与后台验证 |
| [归档命令验收](docs/validation/archive-command-coverage.md) | 14 个命令的参数、工具与真实采集验证 |
| [网页控制台规格](.scratch/web-console/spec.md) | 已确认网页范围、操作语义与完成条件 |
| [网页控制台验收](docs/validation/web-console-validation.md) | 网页使用入口、浏览器／组件证据与真实系统待验项 |
| [核心可行性](docs/research/feasibility.md) | 文件、打包、网络、加密、权限和分发 |
| [现有项目与复用](docs/research/existing-projects.md) | 开源参考和待核查项 |
| [整体架构](docs/architecture/technical-options.md) | 后续核心防护、接口和平台候选 |
| [核心验收方案](docs/validation/core-validation.md) | 发现、外传控制与接收证据 |
| [开发约定](AGENTS.md) | Agent 工作约定 |
