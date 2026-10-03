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

## 选择保护目录

可以一次添加多个本地目录。加入时路径会解析为规范路径：

~~~sh
codeperimeter watch add ~/work/project-a ~/work/project-b
codeperimeter watch list
codeperimeter watch remove ~/work/project-a
~~~

移除已不存在的目录时，提供它此前登记的绝对路径。移除配置不会删除已有活动、告警或统计记录。

历史发现只读取 Codex 与 Claude Code CLI 记录中的目录元信息，不导入或保存会话正文。先生成 JSON 预览快照，再按快照中的零起始候选序号导入：

~~~sh
codeperimeter history preview --output ~/Desktop/codeperimeter-history.json
codeperimeter history import --preview ~/Desktop/codeperimeter-history.json --index 0 --index 2
~~~

也可以使用 --all-available 导入该快照里预览时标记为 available 的全部目录：

~~~sh
codeperimeter history import --preview ~/Desktop/codeperimeter-history.json --all-available
~~~

导入只使用快照中记录的候选与来源；命令会重新核对路径仍解析到同一规范目录。目录失效或快照格式不匹配时，操作失败，请重新预览。新的会话不会自动扩大已选目录集合。预览文件只含目录、来源、版本和发现缺口等元信息，建议仍按本地敏感文件保管。

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

安装服务、系统采集和完全磁盘访问权限需要按 macOS 提示完成必要授权。本仓库的 CLI 命令不会修改 SIP、AMFI、sudoers，也不会自动重启机器。service install/start 返回 launchd 操作结果；操作成功不等于 FDA 已授予、采集器已取得有效事件、系统通知已到屏或 3 秒目标已经通过。完整安装、开机运行、注销恢复和真实事件验收需要单独记录。

当前监控只表达系统实际提供的文件事件与可归属进程身份。已在内存中的内容在没有新文件访问事件时不可见；事件丢失、权限不足、采集断开、数据库故障和通知失败应从状态或健康记录查看。批量访问提醒描述访问汇总，不判定其已经压缩或泄露。此观察版不执行外传拦截，也不检查 HTTPS 正文。
