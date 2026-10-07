# CodePerimeter

English · [简体中文](README.zh-CN.md)

<img src="web/public/codeperimeter.svg" alt="CodePerimeter shield icon" width="64" height="64" />

A local macOS tool for observing project file activity. Use the web console or Rust CLI to select multiple directories, record file opens and readable memory mappings with process identity, identify bulk access, external archive commands and related archive outputs, store evidence in SQLite, and receive system notifications.

File opens and mappings are access evidence; they do not prove that a file was read in full. Bulk access does not confirm compression in memory, and archive indicators do not prove that source code was transmitted. Preventing outbound transmission remains a future product goal.

## Current status

The local web console has seven pages: Overview, Monitored directories, File activity, Archive indicators, Alert center, Rule center, and Settings & diagnostics. The console runs independently of background monitoring, with data managed by the ordinary-user host. The web workflow has verified real administrator authorization, ES/FDA permissions, pause/resume, continued collection after the console exits, and notification display. Archive-output timeliness still has failing samples; restart behavior and summaries after entering the desktop remain unverified. Component/browser tests and real-system evidence are recorded separately in [Web console validation](docs/validation/web-console-validation.md) (Chinese). The earlier CLI validation below does not replace validation of the web workflow.

The CLI, narrow event adapters, rules, SQLite storage, history-based directory discovery, privilege separation, and launchd management are implemented. On 2026-10-05, macOS 15.6.1 validation of the real eslogger → Rust → SQLite/notification-submission pipeline passed all 16 synthetic scenarios. All 22 alerts were generated and submitted for notification within 3 seconds; the maxima were 2.255 and 2.541 seconds respectively, with no known drops or storage gaps in that run. On-screen notification display, background FDA, logout/login summaries, restart behavior, and sustained performance require separate validation in [MVP validation](docs/validation/activity-monitor-mvp.md) (Chinese). A `sent` receipt does not prove display, and a short successful run does not guarantee sustained coverage.

Archive-command recognition includes 14 names by default: tar, bsdtar, gtar, zip, ditto, gzip, pigz, bzip2, pbzip2, xz, zstd, 7z, 7zz, and rar. Users select monitored directories; no per-tool configuration is needed. Common direct paths and stdout modes supported by each tool are covered. Unknown arguments, list files, and stdin without project-source evidence remain coverage gaps. On 2026-10-06, all 92 real-collection scenarios passed: the 34 positive cases had maximum alert-generation/notification-feedback times of 1.373/1.717 seconds, and the 58 negative cases produced no project archive-command alerts. Health, privacy, and cleanup checks passed. File-activity barrier receipt still took about 20 seconds at worst, so timely command alerts do not establish timely file activity. See [Archive-command validation](docs/validation/archive-command-coverage.md) (Chinese) for versions and limits.

System collection uses the macOS-provided `/usr/bin/eslogger` and requires administrator privileges and Full Disk Access for the responsible process. Only collection and forwarding run as root; analysis, SQLite, and notifications run as an ordinary user. Raw system-wide JSON, complete command lines, environment variables, and file contents are not written to disk. The current version subscribes to NOTIFY events for local observation; it does not block compression or network traffic.

## Build and run

Building from source requires macOS, Rust 1.88 or later, Node.js 24, and the Swift compiler from Xcode Command Line Tools. Validation scripts additionally use Python 3 and the system tar/zip tools. Build the web assets before Rust. The web console and native notification helper are embedded in the final binary; Node.js, a Swift compiler, and a separate frontend server are not needed at runtime. CI pins Rust 1.88.0 and Node.js 24, checks web types, tests, and build-artifact consistency, and runs fmt, strict clippy, and Rust tests.

```sh
npm --prefix web ci --ignore-scripts --registry=https://registry.npmjs.org
npm --prefix web run build
cargo build --release --locked
./target/release/codeperimeter --help
./target/release/codeperimeter ui
```

Dependency installation disables lifecycle scripts, including Carbon installation telemetry. The console listens only on a local loopback address. `ui` starts or reuses the console as an ordinary user and opens the default browser. Closing the browser or the terminal used to start it does not stop background collection. If an entry URL stops working, run `codeperimeter ui` again; do not share a URL containing an entry credential.

On first use, install and enable the background service from Settings & diagnostics. Enter the administrator password only in the macOS authorization window; the web page does not receive it. Authorization can be canceled there. Successful installation or a loaded collection job does not prove that real event collection is healthy. Confirm Full Disk Access in System Settings; the console provides instructions and displays actual collection health. Locally built main binaries use ad-hoc signing, so updates may require restoring existing permissions and may leave older entries with the same name. These entries do not mean that duplicate monitoring processes are running. The console remains available when the host is missing or unavailable and shows installation/recovery actions; failed queries are not displayed as zero records.

System alerts use native local notifications with the CodePerimeter name and shield icon. Allow notifications in the macOS prompt when the notification helper first starts; later, adjust them in System Settings → Notifications → CodePerimeter. The notification helper does not need Full Disk Access, and disabling notifications does not stop collection or alert storage. System submission and actual display are verified separately. Notifications describe the behavior as bulk project-file access, a project compression/archive command, or a suspected compressed-file creation/modification. The body includes the program name, project name, and an associated short output filename when available. Clicking a notification or View details opens its alert; summaries and older notifications open the Alert center. The console starts automatically if needed. Clicking does not mark an alert read or resolved. Details explain when a record has expired, been cleared, or has not yet been saved. See the validation document above for native-click verification progress.

In Monitored directories, enter paths, use the native directory picker, or preview and select candidates from Codex, Claude Code, and ZCode history. Disabling a directory excludes its entire subtree and takes precedence over an enabled parent. Removal shows the scope change before confirmation. Directory configuration does not delete project files or original session files.

Pause monitoring pauses system collection while history queries and directory/rule management remain available. Resume reloads collection and reports actual health separately. Pause state is derived from both launchd state and host intent and is designed to persist across restarts; real restart behavior still requires validation. Uninstall preserves configuration and records by default. History queries are temporarily unavailable without the management host after uninstall; reinstall and enable the service to view them again.

| Page | Available actions |
| --- | --- |
| Overview | View service, collection, storage, notification, and coverage state separately; inspect trends and directory/process summaries |
| Monitored directories | Add paths, use the native picker, select imports from three history sources, enable/disable/remove directories |
| File activity | Filter live/history records, paginate fully, inspect processes and events, and open related alerts |
| Archive indicators | Review coverage of the 14 default commands, command/output indicators, and related evidence |
| Alert center | Mark read, resolve manually, add notes, review handling history; new evidence reopens alerts |
| Rule center | Toggle three built-in rules, change four global parameters, apply changes immediately, and interpret historical versions |
| Settings & diagnostics | Manage services, permissions and collection gaps, retention, complete exports, and separate detail/cumulative-statistics deletion |

Record exports follow the current filters, include every page, and support JSON/CSV. Anonymous sharing mode is the default: it replaces paths and process identities and removes free text such as handling notes. Full mode retains original fields and should be used locally as needed. Downloads are saved by the browser rather than written into the source directory. Detail records are retained for 30 days by default, with configurable retention; reducing retention or clearing data requires impact confirmation. Cumulative statistics are cleared separately, preserving directory and rule configuration.

For CLI use, preview the service plan before installation:

```sh
./target/release/codeperimeter service plan --user "$(id -un)"
```

The plan shows all three roles, installation paths, and data locations. After review, explicitly install and start the service. `install` writes installation files only; `start` enables and loads the service. Then check actual collection authorization and runtime state:

```sh
sudo ./target/release/codeperimeter service install --user "$(id -un)"
sudo ./target/release/codeperimeter service start --user "$(id -un)"
./target/release/codeperimeter status
./target/release/codeperimeter watch add /absolute/path/project-a /absolute/path/project-b
./target/release/codeperimeter events --limit 50
./target/release/codeperimeter alerts --limit 50
./target/release/codeperimeter health --limit 100
./target/release/codeperimeter health --source-run-id "SOURCE_RUN_ID" --limit 100
```

For a first history import, generate a snapshot and select directories from it. Codex, Claude Code, and ZCode directory metadata have separate adapters; new sessions do not automatically expand monitoring:

```sh
./target/release/codeperimeter history preview --output candidates.json
./target/release/codeperimeter history import --preview candidates.json --index 0
./target/release/codeperimeter watch list
```

ZCode reads explicitly stored session directories from `~/.zcode/cli/db/db.sqlite` by default; specify a different database with `history preview --zcode-db FILE`. The local format baseline is ZCode 3.8.1. Mid-session directory changes are unverified and appear as a coverage gap in the preview. Source labels identify where directories were discovered, not whether a runtime process belongs to ZCode.

See [CLI usage](docs/usage.en.md) for complete parameters, queries, thresholds, stopping, and uninstalling; see [Background service](docs/service.en.md) for the three roles, root-owned binary, and FDA boundaries. Stopping/uninstalling preserves the database and directory configuration.

## Language and display

Simplified Chinese and English are supported. First use follows the macOS preferred language: Chinese selects Simplified Chinese, English selects English, and unsupported languages fall back to English. The sidebar language menu offers Follow system / 简体中文 / English. The current Mac user's choice is saved and shared by the web console, subsequent system notifications, product-generated native prompts, and default CLI human messages. It persists after closing the console or restarting. Switching language does not restart collection or change monitored directories or rules.

Override the language for a single CLI invocation without changing the saved preference:

```sh
./target/release/codeperimeter --language en --help
./target/release/codeperimeter --language zh-CN ui
```

Dates and numbers follow the display language, using the local time zone and a 24-hour clock. Historical records get current-language explanations from stable codes, while original text remains available. Unrecognized older text, third-party text, and user notes retain their originals. Machine JSON fields, error codes, exported timestamps, paths, and process identities retain their values. Delivered notifications retain their original language. macOS controls the language of its own administrator and notification-permission windows.

Real background-notification, restart-persistence, and complete end-to-end localization results must come from this iteration's integrated validation record. Earlier single-language validation does not establish multilingual validation.

## Validation

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

Complete real validation uses newly created anonymous projects and an independent ordinary-user database, without importing real history or changing existing service configuration. Read [Steps and evidence boundaries](docs/validation/activity-monitor-mvp.md) (Chinese) first, then explicitly run in your own authorized terminal:

```sh
sudo -v
python3 -B scripts/validate-prepare-collector.py --binary target/release/codeperimeter
python3 -B scripts/validate-mvp.py --binary target/release/codeperimeter
```

Preparation only copies a root-owned binary for the current account and rejects an existing destination by default. A validation copy without launchd installation or active endpoints/processes can be replaced safely using `--replace-sha256 <EXPLICIT_OLD_HASH>` as described in the document. Preparation does not install launchd jobs or change FDA. The full entry point reports a local `summary.json` path; real collection failure exits and records the cause rather than falling back to fixtures. Before stopping an existing service, replacing a version, or changing FDA, check the current state as described in the validation document.

## Project documents

Engineering, research, and historical validation documents remain in Chinese.

| Document | Purpose |
| --- | --- |
| [Product context](CONTEXT.md) | Confirmed goals, scope, and terminology |
| [MVP specification](.scratch/activity-monitor-mvp/spec.md) | Current observation scope and completion criteria |
| [Technical baseline](.scratch/activity-monitor-mvp/technical-design.md) | Confirmed technical choices and coverage limits |
| [MVP validation](docs/validation/activity-monitor-mvp.md) | Components, real events, latency, and background validation |
| [Archive-command validation](docs/validation/archive-command-coverage.md) | Parameters, tools, and real collection for 14 command names |
| [Web console specification](.scratch/web-console/spec.md) | Confirmed web scope, operation semantics, and completion criteria |
| [Web console validation](docs/validation/web-console-validation.md) | Web entry point, browser/component evidence, and pending real-system checks |
| [Core feasibility](docs/research/feasibility.md) | Files, archives, network, encryption, permissions, and distribution |
| [Existing projects and reuse](docs/research/existing-projects.md) | Open-source references and pending checks |
| [Architecture](docs/architecture/technical-options.md) | Future protection, interfaces, and platform candidates |
| [Core validation plan](docs/validation/core-validation.md) | Detection, outbound control, and receiver evidence |
| [Development conventions](AGENTS.md) | Agent working conventions |
