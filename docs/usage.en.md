# CodePerimeter CLI usage

English · [简体中文](usage.md) · [Project home](../README.md)

The current release observes local macOS file activity. It records visible system events such as file opens and mappings in selected directories, identifies the originating process, and summarizes bulk access or suspected archive activity. An open or mapping is access evidence, not proof that a file was read in full; bulk access and archive indicators do not prove successful compression or outbound transmission.

## Build and command help

Run from the repository root:

~~~sh
npm --prefix web ci --ignore-scripts --registry=https://registry.npmjs.org
npm --prefix web run build
cargo build --release --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
~~~

`web/dist/` is untracked generated output. Generate it after cloning the source, before building Rust. Rebuild both the web assets and Rust after changing the web source.

The binary is at `target/release/codeperimeter`. Use `codeperimeter --help` or `codeperimeter <COMMAND> --help` for parameters. Query and directory-configuration commands use the ordinary-user host's Unix socket. Override it with the global `--host-socket PATH` option. Its default location is `~/Library/Application Support/CodePerimeter/host.sock` for the current user.

## Language and display

Simplified Chinese and English are supported. First use follows the macOS preferred language; unsupported languages fall back to English. The web sidebar's Follow system / 简体中文 / English menu saves one preference for the current Mac user. Subsequent notifications, product-generated native prompts, and default CLI human messages share it, including after the console closes or the machine restarts.

The global CLI `--language` option selects a language for one invocation without changing the saved preference:

~~~sh
codeperimeter --language en --help
codeperimeter --language zh-CN history --help
~~~

Help and product human messages follow the selected language. JSON fields, error codes, timestamps, paths, process identities, and original records retain their values. The web console interprets historical event/status codes in the current language while retaining original text. Unrecognized older messages, third-party text, and user notes retain their originals. Displayed dates and numbers follow the selected language, local time zone, and 24-hour clock. macOS controls the language of its own authorization windows; delivered notifications are not rewritten retroactively.

Switching language does not restart collection, expand monitoring, or rewrite historical evidence. Multilingual end-to-end behavior across the web console, CLI, and background notifications, along with restart persistence, must be established by this iteration's integrated validation record.

## Validate individual archive commands

First run the validation script's self-test without collection, then optionally exercise local tools:

~~~sh
python3 -B scripts/validate-archive-selftest.py
python3 -B scripts/validate-archive-commands.py --exercise-only
~~~

The exercise calls locally available tar, bsdtar, gtar, zip, ditto, gzip, pigz, bzip2, pbzip2, xz, zstd, 7z, 7zz, and rar on synthetic files. It covers creation/update, supported stdout modes, stdin-only input, listing/testing/unpacking/decompression, and unrelated directories. Stdin-only cases run only for the 10 tools that support that mode; support is not fabricated for tar or ditto. The exercise checks tool execution only. It does not prove that CodePerimeter received system events or generated notifications. Missing/failed version probes and failed tool operations make the exercise fail. Missing tools are searched for in `~/Library/Application Support/CodePerimeter/validation-tools/bin` and `PATH` by default. Use `--rar-binary PATH` to specify a trial RAR binary outside the project.

Complete validation requires Python 3.11 or later, macOS system-collection authorization, and sudo authorization. This Python requirement applies only to validation scripts; the monitor remains a Rust binary. The entry point checks Python before administrator authorization. Run from the repository root:

~~~sh
sh scripts/run-archive-validation.sh
~~~

The script prepares a protected collector copy for this validation run only. It does not install or start launchd services. Preparation failures and cancellation use fixed categories without exposing raw exceptions or commands. Only the collector and a short-lived independent system-event observer run as root. Archive tools, the daemon, SQLite, and the notification agent run as the current ordinary user. For negative cases, the independent observer retains only exec identities associated with this run's synthetic processes in memory, confirming that reverse, unrelated-directory, and stdin-only operations executed. Unexpected observer termination makes validation fail. Stdin-only commands start inside a protected directory and require the main SQLite unknown-source gap to match the actual PID/generation and precede an independent read barrier; a gap from another scenario cannot substitute. Project events, alerts, read barriers, and collection health are still checked against the main SQLite evidence. Each tool/mode prints a sanitized progress line, followed by a `summary.json` path outside the project. The summary contains tool versions, return codes, evidence counts, latency, and static failure codes, but no command arguments, raw system events, or real executable paths.

Use `--report-dir PATH` to select an empty directory outside the project for local evidence. The script rejects project-internal paths and restricts directory permissions. On failure it preserves the synthetic directory, SQLite database, and anonymous summary for inspection. Passing component self-tests or a tool exercise does not replace full system-collection validation.

The independent collection observer preserves the authorized terminal session and controlling terminal, using a separate process group so that eslogger does not suppress same-group test activity. Sequence gaps still fail validation. The summary adds only bounded numeric sequence diagnostics, existing pipeline timing metrics, and alert-latency breakdowns; it does not save raw events. Child-process scans are rate-limited to reduce the validation script's own load. The 3-second threshold is still measured from the system event time, without subtracting collection latency.

The main collector routes exec and the other eight file/process event types through two eslogger sources with a shared run identity; fork, exit, and file activity remain ordered on the same stream. `status` reports separate exec/activity versions, counts, and sequence health in `collector_streams`. If either stream stops, the whole collector stops; the remaining stream cannot represent complete monitoring. Older single-stream records remain readable, and sequences are compared only within their own source. Negative cases first use a trusted control connection to confirm that the main exec stream processed the exact process identity. Stdin-only cases also confirm that their unknown-source gap was saved before starting the independent file barrier. Receipts are bounded, memory-only, and contain no paths or arguments. The full 92-scenario run passed on 2026-10-06; file-activity timeliness and sustained performance remain separate checks.

To diagnose source-to-receipt timeouts, run the entry point below. It runs only the eight positive creation/stdout cases for ditto, bzip2, pbzip2, and zstd, comparing arrival times for the same execution in full collection and an independent exec observer. It also checks that source times fall within the actual execution window. Additional diagnostic fields contain only numbers, booleans, and static classifications. These results are separate from complete validation: eight passing cases do not establish validation of all 14 names. Ordinary monitoring and the default 92-case entry point retain their existing scope.

~~~sh
sh scripts/run-archive-validation.sh --diagnose-source-latency
~~~

## Select protected directories

Add several local directories at once. Paths are canonicalized when added:

~~~sh
codeperimeter watch add ~/work/project-a ~/work/project-b
codeperimeter watch list
codeperimeter watch remove ~/work/project-a
~~~

To remove a directory that no longer exists, supply its previously registered absolute path. Removing configuration does not delete existing activity, alerts, or statistics.

History discovery reads directory metadata from Codex and Claude Code CLI records and the ZCode session database. It does not import or save session contents. Generate a JSON preview snapshot, then import using the snapshot's zero-based candidate indexes:

~~~sh
codeperimeter history preview --output ~/Desktop/codeperimeter-history.json
codeperimeter history import --preview ~/Desktop/codeperimeter-history.json --index 0 --index 2
~~~

Use `--all-available` to import every directory marked `available` when the snapshot was previewed:

~~~sh
codeperimeter history import --preview ~/Desktop/codeperimeter-history.json --all-available
~~~

Import uses only the candidates and sources saved in the snapshot, and rechecks that paths resolve to the same canonical directories. If a directory is no longer valid or the snapshot format does not match, the operation fails; preview again. New sessions do not automatically expand the selected directory set. Preview files contain only metadata such as directories, sources, versions, and discovery gaps, but should still be treated as local sensitive files.

The default ZCode database is `~/.zcode/cli/db/db.sqlite`. Specify a custom location explicitly; this does not change default discovery locations for Codex or Claude Code:

~~~sh
codeperimeter history preview --zcode-db /absolute/path/db.sqlite --output candidates.json
~~~

The ZCode adapter reads only explicitly saved `directory` values in the ordinary `session` table and supports committed WAL records. It does not migrate or write to the source database, query session IDs, titles, or content, or reuse the semantically unverified `path` field. Supported formatting is based on the local ZCode 3.8.1 field shape, without promising compatibility with every version. Previews always report that mid-session directory changes are unverified. Missing databases, unsupported schemas, read failures, and lock timeouts are reported separately. A history-source label does not establish runtime process attribution or prove that a remote path corresponds to local execution. The program still accepts existing v1 snapshots; older programs are not guaranteed to read new snapshots containing `zcode`.

## Query activity and alerts

Events, alerts, health, notification feedback, and statistics are queried through the host interface and returned as JSON:

~~~sh
codeperimeter status
codeperimeter events --directory ~/work/project-a --pid 1234 --kind open --limit 50
codeperimeter alerts --rule bulk_file_access --since-ms 1790900000000
codeperimeter health --limit 100
codeperimeter notifications --outcome failed
codeperimeter stats show
codeperimeter stats show --since-ms 1790900000000 --until-ms 1790986400000
~~~

`stats clear-cumulative` explicitly clears cumulative statistics through the host without deleting events, alerts, health records, or notification details:

~~~sh
codeperimeter stats clear-cumulative
~~~

Time parameters use Unix timestamps in milliseconds. Without a statistics interval, the host returns recent statistics for the past 24 hours together with available cumulative values.

## Background service

The service plan is read-only. It shows the ordinary-user identity, installation/data paths, arguments, and plists for the three launchd roles for review before installation:

~~~sh
codeperimeter service plan --user "$USER"
~~~

Installation, start, stop, and uninstall require administrator privileges and an explicit ordinary user:

~~~sh
sudo ./target/release/codeperimeter service install --user "$USER"
sudo ./target/release/codeperimeter service start --user "$USER"
sudo ./target/release/codeperimeter service stop --user "$USER"
sudo ./target/release/codeperimeter service uninstall --user "$USER"
~~~

Uninstall preserves the local database and directory configuration. Internal background roles are collector, daemon, and notify; their arguments are generated by the service plan and they are normally started by launchd:

~~~text
collector --socket PATH --allowed-uid UID
daemon --socket PATH --control-socket PATH --db PATH [--bulk-file-threshold N] [--bulk-window-ms MS]
notify --control-socket PATH
~~~

The daemon defaults to 50 distinct files in a rolling 10000-millisecond window. Change these with the two optional parameters; `status` reports the actual configuration. `ServicePlan` does not put those optional parameters in the daemon argv, so it uses the defaults. Do not start the daemon as root: it is the ordinary-user host and the sole SQLite writer. The root collector only starts system eslogger and forwards events through a restricted socket. Configuration, queries, history import, and the notification agent do not open additional database-writing channels. Plan arguments describe the fixed service roles only; the system does not collect or display observed processes' complete command lines or environment variables.

## Permissions and coverage limits

### Archive and compression commands

After directories are selected, the program recognizes tar, bsdtar, gtar, zip, ditto, gzip, pigz, bzip2, pbzip2, xz, zstd, 7z, 7zz, and rar automatically by default. No per-tool configuration is needed. Common direct paths, applicable compression-level/output options, and explicit-input-to-stdout modes use the same event pipeline. Non-compression operations such as decompression, listing, and testing do not produce corresponding archive-command alerts.

List files, unknown arguments, and stdin without attributable project evidence produce health-record coverage gaps rather than guessed project origins. Archive commands and outputs remain activity indicators; they do not prove successful compression or outbound transmission. See [Archive-command validation](validation/archive-command-coverage.md) (Chinese) for argument and version coverage.

See [Validate individual archive commands](#validate-individual-archive-commands) for the entry point and [Archive-command validation results](validation/archive-command-coverage.md) (Chinese) for versions and scenarios. On 2026-10-06, all 14 names/92 real scenarios passed, with maximum alert-generation/submission-feedback latency of 1373/1717 ms. File-activity receipt, notification display, and background lifecycle limits are documented separately there.

### System integration

Complete required macOS authorization for service installation, system collection, and Full Disk Access. The repository's CLI does not modify SIP, AMFI, or sudoers, or restart the machine automatically. `service install/start` returns launchd operation results; success does not prove that FDA was granted, the collector received valid events, notifications appeared on screen, or the 3-second target passed. Full installation, startup before login, logout recovery, and real-event validation require separate records. See [Background service](service.en.md) for details.

Monitoring represents only file events and attributable process identities actually provided by the system. Contents already in memory are invisible without new file-access events. Use status/health records to inspect event loss, missing permissions, collection disconnects, database faults, and notification failures. Bulk-access alerts describe access summaries rather than confirmed compression or disclosure. This observation release does not block outbound transmission or inspect HTTPS bodies.

Query source versions and structured gaps with `codeperimeter health --source-run-id <COLLECTOR_RUN_ID_FROM_STATUS> --limit 100`. A missing source version on a standard event indicates older data or an unknown source, not an assumed currently supported version.
