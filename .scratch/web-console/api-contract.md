# 网页接口与并行实施契约

日期：2026-10-06。实施依据：spec.md；这些是实现选择，不改变已确认范围。

## 技术与职责

- 主 agent：src/web.rs、src/main.rs、src/lib.rs、Cargo.toml／Cargo.lock、HTTP 与 CLI 测试、集成和交付。
- 核心 worker：src/console.rs、src/model.rs、src/rules.rs、src/storage.rs、src/runtime.rs 及核心相关 Rust 测试；唯一数据库写入方仍是宿主。
- 服务 worker：src/service.rs、src/native.rs、tests/web_service.rs 及服务相关测试；系统授权、原生目录选择、采集单独暂停与恢复。
- 前端 worker：web/ 内源代码、构建产物、npm 锁文件、页面组件与浏览器测试；不改 Rust 或根级构建配置。
- 各 worker 共享 main 工作区、按文件所有权协作，不创建分支／PR／独立 worktree，不提交、不推送、不覆盖他人修改。
- Rust HTTP 使用 axum 0.8 与 Tokio；前端 React／TypeScript，Carbon 正式组件，Vite 构建后嵌入二进制。运行时不需要 Node。

## 通用响应与认证

所有 JSON API 返回 {ok:boolean,data:any,error:string|null}。错误使用静态中文或代码，不回显原始 stderr、SQL、argv 或完整未过滤事件。

控制台 URL 的 fragment 带本次入口令牌；前端读取后从地址栏移除，并仅在当前标签页 sessionStorage 保留以支持刷新；401 时删除，不使用 localStorage，使用 Authorization: Bearer 发请求。所有 API 要求令牌；检查 Host 和 Origin，仅接受本机同源 JSON 请求，不允许跨域。静态网页可显示入口失效引导。服务端不记录令牌。浏览器测试从项目外的私有会话文件读取认证，不回显令牌或私人路径。

## HTTP 契约

- GET /api/status -> data:{host: RuntimeStatus|null,host_error:string|null,service:ServiceStatus,platform:string,service_actions_enabled:boolean}。
- POST /api/control -> 既有 ControlRequest；允许 status、目录增删列、查询健康／通知、stats，禁止 notify ACK／stop 等非网页操作。
- POST /api/console -> body:{action,payload?}；转为 ControlRequest::Console {request:ConsoleRequest}。新操作如下。
- POST /api/history/preview -> 默认三种本机来源；data:{preview_id,candidates,gaps,counts,versions}，快照仅留内存；支持独立测试启动指定匿名历史来源（CLI 参数，不在普通网页暴露）。
- POST /api/history/import -> {preview_id,indices:number[]}，仅导入快照中可用候选；不得重新扫描扩大选择。
- POST /api/directory/pick -> 系统原生目录选择；data:{path:string|null,cancelled:boolean}。
- POST /api/service -> {operation:install|start|pause|resume|uninstall}；立即 data:{id,state:"running"}。
- GET /api/service/{id} -> data:{id,operation,state:running|succeeded|cancelled|failed,report?,error?}；授权异步，不阻塞查询。
- POST /api/export -> {kind:events|alerts,filter:{},search?:string,archive_only?:boolean,format:json|csv,anonymous:boolean}，下载当前筛选的全部分页，不只当前100条。流式、有界，不阻塞采集。导出文件名固定，CSV 防公式注入，匿名模式替换身份／路径、删除备注等自由文本。

## 核心 ConsoleRequest 契约

serde 使用 action／payload 的内部标签结构；payload 的以下参数均为对象。旧 QueryEvents／QueryAlerts 响应和公开模型尽量保持兼容。

- summary:{since_ms?:number,until_ms?:number} -> {stats:{cumulative,recent},trend:[{timestamp_ms,open,mmap,archive}],top_processes:[{pid,executable,count}],top_files:[{path,count}],unread_alerts:number,pending_alerts:number}。
- directories -> 数组，每项 {path,sources,added_at_ms,exists,enabled,effective,excluded_by:string|null}。
- directory_set:{path:string,enabled:boolean} -> 目录状态列表；路径需已配置，停用形成全子树优先排除。
- rules_get -> {version:number,bulk_enabled:boolean,archive_command_enabled:boolean,archive_output_enabled:boolean,bulk_file_threshold:number,bulk_window_ms:number,alert_merge_window_ms:number,archive_correlation_window_ms:number}。
- rules_set:{settings:上项对象} -> 保存后的同形对象；校验有界参数，成功保存后才更新引擎，清旧窗口，版本增加，老版本告警不再合并。
- events_page:{filter:旧EventFilter,cursor?:string,search?:string,archive_only?:boolean} -> {items:StoredEvent[],next_cursor:string|null,total:number}；稳定有界分页，不限制全部结果为100条。
- alerts_page:{filter:旧AlertFilter,cursor?:string,search?:string,is_read?:boolean,processed?:boolean} -> {items:AlertEntry[],next_cursor:string|null,total:number}。
- event_detail:{id:number} -> {event:StoredEvent,alerts:AlertEntry[]}，关联线索不宣称内容同一。
- alert_detail:{id:string} -> AlertEntry，增加 events:StoredEvent[]、notifications:StoredNotificationRecord[]。
- alert_update:{id:string,is_read?:boolean,processed?:boolean,note?:string,expected_revision?:number} -> AlertEntry；旧版本更新拒绝，保留操作历史，新增合并证据重置未读／待处理。
- directory_remove_preview:{path:string} -> {removed_path,removes_exclusion,coverage_may_expand,enabled_ancestors,affected_descendants,after}，预览移除配置后的范围变化。
- retention_preview:{days:number} -> {days,cutoff_ms,counts,preview_revision:number}，固定截点、60秒有效。
- retention_get -> {days:number,counts:{events,alerts,health_records,notifications,handling_records}}。
- retention_set:{days:number,confirm:boolean,preview_revision?:number} -> retention_get 同形；缩短需确认。
- clear_details:{confirm:boolean} -> {deleted:计数}；保留配置／规则／累计，清关联处理与旧规则窗口。
- clear_cumulative:{confirm:boolean} -> {cleared:true}。
- monitoring_set:{paused:boolean} -> {paused:boolean}；供服务适配协调使用，HTTP /api/console 不允许直接调用此操作。宿主持久化意图、暂停期间保留 IPC。
- record_operation:{operation:string,outcome:string} -> {recorded:true}；静态白名单操作码写健康记录。

AlertEntry={alert:原Alert,is_read:boolean,processed:boolean,note:string,revision:number,rule_version:number,rule_snapshot:RulesSettings|null,handling_history:[{timestamp_ms,is_read,processed,note}]}。新迁移中旧告警规则版本明确未知，不用当前配置冒充旧配置。旧 CLI 查询与数据写入契约继续工作。

## 服务／原生 Rust API

服务 worker 导出 native 模块中的 enum ServiceAction 与函数 service_status(username,source_binary)、run_service_action(action,username,source_binary,control_socket)、choose_directory()，返回可 Serialize 的状态／结果。具体接口为 service_status(&str,&Path)、run_service_action(ServiceAction,&str,&Path,&Path)、choose_directory()。ServiceStatus 包含 uid、installed、installed_binary_trusted、collector／analyzer／notification（loaded／running／disabled／last_exit_code）、paused 与 status_error；授权结果包含 state、report 与静态 error。

service::ServicePlan 增加 pause／resume：只控制 collector，不卸载 analyzer／notify。暂停状态由 launchd disabled 及宿主持久化意图共同判断；不把权限拒绝或采集断流报告成成功暂停。恢复实际加载后才报告启用，源健康仍单独判断。原生授权不收密码，仅固定程序／参数；系统 stderr 静态分类。原有 stop 仍是停止全部角色。

## 测试边界

- 真实浏览器 + 生产 HTTP + 普通用户生产宿主 + 匿名采集替身，是应用端到端组件证据，不冒充 ES／FDA。
- 另做实际管理员授权／eslogger／后台与通知链路；任何缺权或重启尚未执行保留待验证。
- 匿名 fixture、截图、导出和测试会话放项目外受限临时目录；只有脱敏摘要进入项目。

## Comments

- 2026-10-06：已确认整版需求，用户要求开始开发及端到端测试。接口契约用于减少并行文件冲突；发现与现有模型不兼容时先协调契约，不另造第二个数据库写入方。
