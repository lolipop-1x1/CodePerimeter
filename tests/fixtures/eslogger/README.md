# ES 格式测试样本

`events.jsonl` 为人工构造的匿名 schema 1 样本，覆盖本版九类 NOTIFY。路径、身份、时间和序号均为合成值；不含本机采集行、完整命令行或环境变量。归档参数测试仅在测试代码中构造最小工具／路径输入。

字段依据 Apple 本机 macOS 11.1 SDK `EndpointSecurity/ESTypes.h` 与 `ESMessage.h`，以及 [esl 原始字段声明](https://github.com/tstromberg/esl/blob/4d890d24a9aad6e7c848d6ce9f39c73d111004ab/pkg/eslogger/structs.go)；前者提供原生事件语义，后者提供 eslogger schema 1 的字段结构参考。核查日期为 2026-10-03。没有复制其解析实现。

这些样本验证适配逻辑。实现期间，本机 macOS 15.6.1 脱敏探针观察到 schema 1／message version 9 的 mmap.source／protection、close.target 和 EXEC target 的 pidversion 字段与结构一致；这不证明九类事件全部、后台 FDA、完整性或延迟。探针仅输出筛选后片段，不能用于序号连续性判断。未知 schema 会停止该行解析并报告缺口。
