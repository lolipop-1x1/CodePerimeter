# CodePerimeter 命令行使用说明

当前交付是 macOS 本机文件活动观察版。它记录选定目录内可见的文件打开、映射等系统事件，展示来源进程，并汇总批量访问或疑似打包迹象。文件打开或映射是访问证据，不代表读完了文件；批量访问和归档迹象也不证明内容已经压缩或外传。

## 构建与命令帮助

在仓库根目录执行：

~~~sh
cargo build --release
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
~~~

二进制位于 target/release/codeperimeter。运行 codeperimeter --help 或 codeperimeter <命令> --help 查看参数。查询和目录配置命令通过普通用户宿主的 Unix socket 工作；可以在全局参数中指定 --host-socket PATH。默认位置为当前用户的 ~/Library/Application Support/CodePerimeter/host.sock。

## 逐项归档命令验收

先运行脚本裁决器的无采集自检，再可选地做本机工具演练：

~~~sh
python3 -B scripts/validate-archive-selftest.py
python3 -B scripts/validate-archive-commands.py --exercise-only
~~~

无采集演练会对合成文件实际调用本机可用的 tar、bsdtar、gtar、zip、ditto、gzip、pigz、bzip2、pbzip2、xz、zstd、7z、7zz 和 rar，并覆盖创建／更新、支持的标准输出、纯标准输入、列表／测试／解包／解压与无关目录场景。纯标准输入仅用于支持该模式的 10 个工具，不给 tar／ditto 伪造支持。演练只核对工具自身的执行结果，不能证明 CodePerimeter 收到了系统事件或生成提醒；版本探针缺失、失败或工具操作失败都会返回失败。缺失工具默认从 ~/Library/Application Support/CodePerimeter/validation-tools/bin 与 PATH 查找；RAR 可通过 --rar-binary PATH 显式指定项目外的试用二进制。

完整验收需要 Python 3.11 或以上、macOS 系统采集授权与 sudo 授权。Python 版本要求仅用于验收脚本，监控程序仍是 Rust 二进制；入口会在管理员授权前检查版本。在仓库根目录运行：

~~~sh
sh scripts/run-archive-validation.sh
~~~

脚本只为本次验收准备受保护的 collector 副本，不安装或启动 launchd 服务。准备失败和取消使用固定分类，不回显原始异常或命令。root 仅运行 collector 与短暂的系统事件旁路；归档工具、daemon、SQLite 和通知代理以当前普通用户身份运行。负例的旁路只在内存中保留与本次合成进程关联的 exec 身份，用来确认反向、无关目录及纯标准输入操作确实执行；观察器意外结束也会使验收失败。纯标准输入在保护目录内启动，要求主 SQLite 的来源未知缺口匹配实际 PID／代次且早于独立读取屏障，已有其他场景的缺口不能替代。项目事件、告警、读取屏障与采集健康仍由主 SQLite 证据核对。每个工具与模式会输出一行脱敏进度，结束时给出项目外 summary.json 路径。汇总中记录工具版本、返回码、证据计数、延迟和静态失败代码，不保存命令参数、原始系统事件或真实 executable 路径。

可用 --report-dir PATH 指定项目目录外的空目录保存本机证据；脚本会拒绝项目内路径并限制目录权限。验收失败也会保留合成目录、SQLite 和匿名摘要以便回查。组件自检或无采集工具演练通过，均不替代完整系统采集验收。

独立采集旁路保留授权终端的会话和控制终端，并使用独立进程组，避免 eslogger 抑制测试程序的同组活动。序号缺口仍使验收失败；摘要只增加有界的数值序号诊断、现有链路耗时指标及告警延迟拆分，不保存原始事件。子进程扫描限制频率以减少测试脚本自身负载；3 秒门槛仍从系统事件发生时计算，不扣除采集延迟。

若需定位来源到接收的超时，可执行以下诊断入口。它只运行 ditto、bzip2、pbzip2、zstd 的创建与标准输出共 8 个正向场景，比较完整采集与独立 exec 旁路对同次执行的到达时间，并检查来源时间是否处于实际执行窗口。新增诊断字段只保存数字、布尔值和静态分类。该模式的结果与完整验收分开，8 项通过也不能判定全部 14 个名称验收完成；普通监控及默认 92 项验收入口保持原有范围。

~~~sh
sh scripts/run-archive-validation.sh --diagnose-source-latency
~~~

## 选择保护目录

可以一次添加多个本地目录。加入时路径会解析为规范路径：

~~~sh
codeperimeter watch add ~/work/project-a ~/work/project-b
codeperimeter watch list
codeperimeter watch remove ~/work/project-a
~~~

移除已不存在的目录时，提供它此前登记的绝对路径。移除配置不会删除已有活动、告警或统计记录。

历史发现读取 Codex、Claude Code CLI 记录与 ZCode 会话库中的目录元信息，不导入或保存会话正文。先生成 JSON 预览快照，再按快照中的零起始候选序号导入：

~~~sh
codeperimeter history preview --output ~/Desktop/codeperimeter-history.json
codeperimeter history import --preview ~/Desktop/codeperimeter-history.json --index 0 --index 2
~~~

也可以使用 --all-available 导入该快照里预览时标记为 available 的全部目录：

~~~sh
codeperimeter history import --preview ~/Desktop/codeperimeter-history.json --all-available
~~~

导入只使用快照中记录的候选与来源；命令会重新核对路径仍解析到同一规范目录。目录失效或快照格式不匹配时，操作失败，请重新预览。新的会话不会自动扩大已选目录集合。预览文件只含目录、来源、版本和发现缺口等元信息，建议仍按本地敏感文件保管。

ZCode 默认数据库是 `~/.zcode/cli/db/db.sqlite`，自定义位置可显式指定；该参数不改变 Codex 与 Claude Code 的默认发现位置：

~~~sh
codeperimeter history preview --zcode-db /absolute/path/db.sqlite --output candidates.json
~~~

ZCode 适配只读普通 `session` 表中明确保存的 `directory`，能读取已提交的 WAL 记录，不迁移或写入原库；不查询会话 ID、标题、正文，也不借用语义未验证的 `path` 字段。支持格式依据本机 ZCode 3.8.1 的字段形状，不承诺所有版本。预览始终显示会话中途目录变化未验证的缺口；数据库缺失、schema 不支持、读取失败或锁等待超时分别报告。历史来源标签不证明运行时进程归属，也不证明远程路径对应本机执行。当前程序继续接受原有 v1 快照，旧程序不承诺读取含 `zcode` 的新快照。

## 回查活动和告警

事件、告警、健康、通知反馈和统计均通过宿主接口查询，输出 JSON：

~~~sh
codeperimeter status
codeperimeter events --directory ~/work/project-a --pid 1234 --kind open --limit 50
codeperimeter alerts --rule bulk_file_access --since-ms 1790900000000
codeperimeter health --limit 100
codeperimeter notifications --outcome failed
codeperimeter stats show
codeperimeter stats show --since-ms 1790900000000 --until-ms 1790986400000
~~~

stats clear-cumulative 通过宿主显式清除累计统计，不删除事件、告警、健康记录或通知明细：

~~~sh
codeperimeter stats clear-cumulative
~~~

时间参数使用 Unix 毫秒时间戳。省略统计区间时，宿主按最近 24 小时返回近期统计，同时提供可用的累计值。

## 后台服务

服务计划只读输出普通用户身份、安装与数据路径、三种 launchd 角色参数和 plist，供安装前核对：

~~~sh
codeperimeter service plan --user "$USER"
~~~

安装、启动、停止和卸载需要管理员权限，并显式指定普通用户：

~~~sh
sudo ./target/release/codeperimeter service install --user "$USER"
sudo ./target/release/codeperimeter service start --user "$USER"
sudo ./target/release/codeperimeter service stop --user "$USER"
sudo ./target/release/codeperimeter service uninstall --user "$USER"
~~~

卸载会保留本地数据库和目录配置。后台内部角色分别是 collector、daemon 和 notify，它们的参数由服务计划生成，通常由 launchd 启动：

~~~text
collector --socket PATH --allowed-uid UID
daemon --socket PATH --control-socket PATH --db PATH [--bulk-file-threshold N] [--bulk-window-ms MS]
notify --control-socket PATH
~~~

daemon 的批量访问默认阈值为 50 个不同文件，滚动窗口为 10000 毫秒；可通过这两个参数调整，status 会回显实际配置。ServicePlan 生成的 daemon argv 不指定这两个可选参数，因此采用以上默认值。不要以 root 身份启动 daemon；它是普通用户宿主，也是 SQLite 的唯一写入者。root collector 只负责启动系统 eslogger 并通过受限 socket 转发事件。配置、查询、历史导入和通知代理不另开数据库写入通道。计划中的参数只描述本服务生成的固定角色，不采集或回显观察到的进程完整命令行或环境变量。

## 权限与覆盖边界

### 归档与压缩命令

选定目录后，程序默认自动识别 tar、bsdtar、gtar、zip、ditto、gzip、pigz、bzip2、pbzip2、xz、zstd、7z、7zz、rar；无需逐工具配置。常见直接路径、适用的压缩级别与输出选项，以及明确输入到标准输出的模式均使用同一事件链路。解压、列表、测试等非压缩操作不产生对应的归档命令告警。

列表文件、未知参数和缺少可关联项目证据的标准输入通过健康记录报告缺口，不猜测项目来源。归档命令和输出仍表示操作线索，不能证明成功压缩或外传。参数与版本范围见 [归档命令验收](validation/archive-command-coverage.md)。

入口见 [逐项归档命令验收](#逐项归档命令验收)，版本、场景与待验结果见 [归档命令验收记录](validation/archive-command-coverage.md)。全部 14 名称的真实系统结果尚待记录。

### 系统接入

安装服务、系统采集和完全磁盘访问权限需要按 macOS 提示完成必要授权。本仓库的 CLI 命令不会修改 SIP、AMFI、sudoers，也不会自动重启机器。service install/start 返回 launchd 操作结果；操作成功不等于 FDA 已授予、采集器已取得有效事件、系统通知已到屏或 3 秒目标已经通过。完整安装、开机运行、注销恢复和真实事件验收需要单独记录。

当前监控只表达系统实际提供的文件事件与可归属进程身份。已在内存中的内容在没有新文件访问事件时不可见；事件丢失、权限不足、采集断开、数据库故障和通知失败应从状态或健康记录查看。批量访问提醒描述访问汇总，不判定其已经压缩或泄露。此观察版不执行外传拦截，也不检查 HTTPS 正文。

来源版本与结构化缺口可用 `codeperimeter health --source-run-id <status中的collector_run_id> --limit 100` 回查。标准事件的来源版本为空表示旧数据或来源未知，不能推定为当前支持版本。
