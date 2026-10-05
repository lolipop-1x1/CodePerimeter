# 归档与压缩命令覆盖：需求讨论

日期：2026-10-05
Status: resolved
Type: grilling

## Notes

- 用户调用 `grill-with-docs`，希望支持更多压缩命令；本轮先讨论范围，未修改监控实现。
- 将“归档命令迹象”与“归档输出迹象”分别定义，沿用风险线索不等于操作成功或外传的证据口径。
- 讨论任务见 [01 支持范围与验收](issues/01-scope-and-validation.md)。

## Decisions-so-far

- 沿用已确认的观察 MVP：识别与保护目录关联的活动、记录与提醒；本轮未提出新的拦截能力。
- 沿用匿名文档与合成验证数据约束，不记录真实个人路径、历史正文或完整命令行。
- Q1 已确认 A：补齐 gtar、ditto、gzip、pigz、bzip2、pbzip2、xz、zstd、7z、7zz、rar 共 11 个名称，与已有 tar、bsdtar、zip 合计 14 个。
- 用户明确：选择监控目录后，全部受支持名称默认自动识别；无需选择压缩方式或逐工具配置。该名单不表示任意未知工具或内部库压缩都能直接识别。
- Q2 已确认 A：同时识别明确项目输入文件到标准输出的模式；纯标准输入且没有项目证据时报告覆盖缺口，不推断项目来源。本轮不增加跨进程管道数据追踪。
- Q3 已确认 B：准备本机缺失工具，所有 14 个名称均要求逐一真实采集验收；不能把未执行的工具视为通过。
- Q4 已确认 A：先覆盖常见直接路径、适用的递归／压缩级别／输出模式；列表文件与未知参数报告缺口，不读取列表补全输入。
- Q5 已确认 A：采用官方 RAR 试用包，仅用于本机合成验收，不随开源项目分发；试用期与系统执行限制按实际结果记录。
- Q6 已确认 A：共享理解达成，[最小规格](spec.md)定稿；后续任务见 [02 命令解析与规则](issues/02-command-parsers-and-rules.md)、[03 工具准备与真实验收](issues/03-tools-and-real-validation.md)。
- 实施进展：[02](issues/02-command-parsers-and-rules.md) 已 resolved，默认 14 名称解析与多输出兼容模型完成，组件回归通过；[03](issues/03-tools-and-real-validation.md) 已 claimed，全部工具准备完成，正在补齐真实采集验收证据。全部规格尚未 resolved。

## 设计树

- 扩展压缩命令监控
  - 新增名称范围（Q1，已确认 A）
    - 补齐现有规则名单中缺少解析的 11 个名称。
    - 全部 14 个名称默认自动识别，无逐工具选择入口。
  - 输出覆盖（Q2，已确认 A）
    - 落盘模式与明确项目输入文件到标准输出的模式。
    - 纯标准输入缺少项目证据时不猜测来源。
  - 验收安排（Q3，已确认 B）
    - 准备缺失工具，14 个名称逐一真实采集验收。
    - RAR 准备方式（Q5，已确认 A）：官方试用包用于本机合成验收，不随项目分发。
  - 列表文件覆盖（Q4，已确认 A）
    - 先覆盖常见直接路径形式，列表文件／未知参数报告缺口。
  - 最终共享理解（Q6，已确认 A）
    - 最小规格作为后续实现依据。

## 事实核查（讨论时基线）

- [实时事件解析](../../src/eslogger.rs)：目前仅为 tar、bsdtar、zip 生成归档命令元信息；tar 与 bsdtar 共用解析。
- [规则](../../src/rules.rs)：工具名单另含 gtar、ditto、gzip、pigz、bzip2、pbzip2、xz、zstd、7z、7zz、rar，但名单本身不启用实时参数解析。
- 同文件中的归档后缀规则覆盖常见 tar、zip、7z、rar、gzip、bzip2、xz、zstd 输出；触发需满足项目与同进程活动关联，后缀本身不足以告警。
- 本机只读核查：ditto、gzip、bzip2、xz、zstd、7z 可用；gtar、7zz、pigz、pbzip2、rar 未发现。这不是新增命令的系统验收。
- bsdtar 名称可用；7z 入口为 shell 包装脚本，最终 Mach-O 子进程名称仍为 7z。真实验收必须使用最终系统事件中的可执行身份，不仅看调用时的命令名。
- [XZ 官方手册](https://tukaani.org/xz/man/xz.1.html)明确区分压缩、解压、测试、列表与标准输出模式；[zstd 官方命令行手册](https://github.com/facebook/zstd/blob/dev/programs/zstd.1.md)为后续参数核查的一手来源。具体支持参数须在实现规格中逐工具列出。
- 缺失的开放工具可用 Homebrew 官方配方准备：[gnu-tar](https://formulae.brew.sh/formula/gnu-tar)、[sevenzip](https://formulae.brew.sh/formula/sevenzip)、[pigz](https://formulae.brew.sh/formula/pigz)、[pbzip2](https://formulae.brew.sh/formula/pbzip2)；本机 Homebrew 可用，本轮未安装。
- [RAR cask](https://formulae.brew.sh/cask/rar)当前因 Gatekeeper 检查失败被禁用；[官方命令行下载](https://www.rarlab.com/download.htm)仍提供 macOS 试用包。[RAR EULA](https://www.rarlab.com/license.htm)规定最多 40 天试用，持续使用需要许可；不得将其单独二进制嵌入本项目分发。

## Fog

- 标准输入与跨进程管道：压缩进程可能没有明确文件输入或项目读取证据；不能仅凭压缩程序名称推断载荷来自项目。
- 现有模型只有工具、输入、单个可选输出与工作目录；多输入压缩可能产生多个输出，须在范围确认后评估最小必要变化。
- 未识别的参数、文件列表、别名和工具版本差异需要明确覆盖缺口，不能猜测路径或把解压当作压缩。
- 不新增 ADR：当前是既有观察链路的覆盖讨论，尚未形成高成本架构取舍。

## Comments

- 2026-10-05：用户确认 Q1 A、Q2 A，并强调全部受支持命令应自动监控，无需用户指定压缩方式。
- 2026-10-05：用户确认 Q3 B、Q4 A；要求准备缺失工具逐一实测，列表文件先保留覆盖缺口。
- 2026-10-05：用户确认 Q5 A，接受官方 RAR 试用包用于本机合成验收；最终共享理解待确认。
- 2026-10-05：Q6 最终规格确认已发出；未修改监控实现或安装测试工具。
- 2026-10-05：用户确认 Q6 A，需求讨论完成，最小规格定稿；实现与真实验收仍待后续任务执行。
