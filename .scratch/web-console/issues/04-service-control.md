# 系统授权及采集独立生命周期

Status: resolved
Type: task
Blocked by: 01

## 范围

按 spec.md 和 ../api-contract.md 实现原生管理员窗口、目录选择、实际服务状态、仅暂停／恢复采集、卸载保留数据。普通用户网页不接收密码、不执行任意命令，后台与桌面通知状态分别报告。

## 验证

参数转义／操作白名单、取消／拒绝／失败静态分类、三角色计划与暂停不关闭宿主、重启时保留暂停意图。真实 macOS 授权另验，组件替身不算系统通过。

## Comments

- 2026-10-06：服务 worker 负责 src/service.rs、src/native.rs 及服务测试；不运行 sudo／安装或改FDA，不修改 main/lib/Cargo。
- 2026-10-06：已实现受限 ServiceAction、原生管理员与目录选择脚本、安装／三角色 launchd 状态、采集独立 pause／resume。暂停只 disable／bootout collector；恢复只 enable／bootstrap／kickstart collector。原有 stop 保留停止全部角色的行为。
- 2026-10-06：原生端仅在 root 操作成功后请求普通用户宿主持久化 monitoring_set；缺失或不一致 ACK 报 monitoring_state_unconfirmed。状态只有在 launchd 与宿主意图一致时显示已暂停。无宿主期间安装／卸载结果可由网页任务展示，操作健康记录无法写入时不虚构已保存。
- 2026-10-06：脚本不接受密码；参数经 shell 和 AppleScript 两层字面转义，系统错误转静态码。真实系统编译发现沙箱不自动加载标准术语，已显式引用固定 StandardAdditions.osax 术语，未向另一应用发送管理员命令。
- 2026-10-06：已用匿名组件验证系统脚本编译、字面转义、静态取消／拒绝／失败分类、状态交叉核对和来源二进制校验；本机授权对话框、FDA、真正 launchd 暂停／恢复及重启验收仍由集成阶段执行，不以组件替身宣称通过。
- 2026-10-06：主工程原生组件 7 项、collector 生命周期 4 项、公开服务接口 3 项均通过。测试采用匿名路径；原生脚本仅编译不执行，暂停／恢复用受控 launchctl 替身；真实普通用户 Unix socket 验证持久化 ACK 必须与请求暂停状态一致。

## Answer

已完成服务／原生适配代码及 14 项主工程组件回归；暂停采集不会关闭宿主或通知，launchd disabled 与宿主持久化意图交叉确认，失败／拒绝／取消均不能报告成功。真实管理员窗口、FDA、采集链路、重启及通知到屏属于 07 端到端验收，尚未执行。
