# 可复现合成项目操作发送器

Status: resolved
Type: task
Blocked by: 01

## Scope

从 06 拆出独立操作端。所有权：`scripts/synthetic_sender.py`、`tests/test_synthetic_sender.py`。不接入模拟事件、不解析 ES、不改 Rust 或其他文档。用户选定合成目录，生成匿名源码样本，发送器可按独立场景运行；供 06 完整链路验收调用。

## Acceptance

- 单文件 read／可读 mmap／重复 read、55 个文件批量读取、外部 tar／zip（目录内与外部临时输出）、同进程落盘归档、同进程纯内存压缩、监控开始前预加载对照、正常搜索／索引／构建样本均可独立执行。
- 真正创建有效归档；纯内存场景不落归档，预加载对照有明确 ready／release 协调，释放后不重新打开源码。所有进程与输出时间、PID、预期活动作为操作侧元数据供验收，不称为观察或告警结果。
- 默认独立进程组／可由父级隔离，避免 eslogger 自身进程组排除；不读取真实源码，不产生上传，不修改外部用户文件，只管理合成目录与自己创建的临时归档。
- 所有产物公开数据为合成；保存最小场景元数据，不读取／持久化系统原始事件。发送器自测检查 zip/tar 内容、mmap／memory/预加载状态、正常任务完成、清理路径边界。
- 仅依赖 Python 标准库与 macOS现有 tar/zip，使用说明写入票据 Answer；完整事件／时延验收由 06 完成，不能宣称发送器测试证明监控有效。

## Answer

操作端为 `scripts/synthetic_sender.py`，标准输出为 JSONL。示例：

```sh
python3 -B scripts/synthetic_sender.py prepare --root /private/tmp/codeperimeter-synthetic-demo --files 55
python3 -B scripts/synthetic_sender.py run --root /private/tmp/codeperimeter-synthetic-demo --scenario bulk-read
python3 -B scripts/synthetic_sender.py run --root /private/tmp/codeperimeter-synthetic-demo --scenario tar --output-scope temporary
python3 -B scripts/synthetic_sender.py cleanup --root /private/tmp/codeperimeter-synthetic-demo
```

- `--root` 是合成工作区。prepare 在其 `project/src/` 生成匿名源码；**06 应监控 prepare 输出的 `project_root`，即 root/project**。工作区清单在项目范围之外，校验用 lstat 身份／大小／mtime，不为了校验提前读取源码。只接受不存在或为空的工作区，后续不读真实项目。
- `run --scenario` 支持 `read`、`mmap`、`repeat-read`、`bulk-read`、`tar`、`zip`、`disk-archive`、`memory-archive`、`preloaded-memory`、`search`、`index`、`build`。默认 55 个不同源码文件；重复读取默认 55 次同一文件，可用 `--repeats` 调整。prepare 文件数允许 1–1000，以便阈值校准。
- 外部 tar／zip 真正运行 `/usr/bin/tar`／`/usr/bin/zip`；同进程落盘 ZIP 用 Python zipfile；纯内存 ZIP 只在 BytesIO 压缩。归档均校验完整成员和合成内容。`--output-scope inside|temporary` 控制落盘归档位置：项目内 `.archives/`，或新创建并登记的外部临时目录。没有上传代码。
- 预加载对照：以 stdin/stdout 管道启动 `preloaded-memory`，持续读取 JSONL 到 `phase=ready`，此时源码已进入内存；06 可启动监控，再向 stdin 写入 `release\n`。之后 `released`→`memory_archive_completed`→`completed`，不重新打开源码。`--release-timeout` 默认 60 秒，范围 0–3600 秒；错误、超时或 stdin 关闭均退出 2。
- CLI 默认 `setsid` 隔离进程组；父级也可用 `Popen(start_new_session=True)`，此时保持已有独立组。每条元数据带操作侧 PID、PGID、墙钟／单调纳秒时间；`file_operation` 带路径、索引、操作起止时间。`external_started` 带真实 child_pid（短命进程 PGID 获取失败保留未知），归档完成带输出路径与内容校验结果。
- 输出来源固定 `synthetic_sender`、schema 1；它描述发送器实际做了什么，不是 ES 事件、告警或通知证据。不能由其自测推导监控有效、覆盖完整或 3 秒目标达标。完整链路须由 06 对真实监控记录独立裁决。
- 清理逐项验证源码和输出身份、不跟随被替换的符号链接、拒绝未登记文件。外部临时归档同时核验临时根、目录身份、所有权标记与内容清单，只删除自己登记的文件；发现用户新增文件先拒绝整次清理。工作区不支持并发操作。
- Python 调用入口：`prepare(root: Path, count=55) -> Path`、`run_scenario(root: Path, scenario: str, output_scope="inside", repeats=55, release_timeout=60) -> None`、`cleanup(root: Path) -> None`；这些入口输出同一 JSONL，进程组隔离由 CLI 或父级完成。
- 自测：`python3 -B -m unittest discover -s tests -p test_synthetic_sender.py -v`，8 项测试通过（2026-10-03，Python 3.12.5／本机 tar、zip）。覆盖真实归档双位置、成员内容、可读映射／重复与批量、纯内存无落盘、ready 后让源码路径失效仍成功压缩、正常任务／重复构建、独立进程组与清理安全边界。
