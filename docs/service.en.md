# Background collection and service management

English · [简体中文](service.md) · [Project home](../README.md)

The current MVP uses launchd. A root collector starts only the system `/usr/bin/eslogger` and forwards an in-memory stream. An ordinary-user system daemon analyzes events and writes SQLite. A notify agent sends notifications in that account's Aqua desktop session. The system daemon's `UserName` allows analysis to start after boot without relying on a terminal or desktop login.

Use the CLI to preview a service plan, install, start, stop, and uninstall. The plan's paths and three argv sets are the actual installation inputs:

| Role | Arguments | Privileges and lifecycle |
| --- | --- | --- |
| collector | `collector --socket <collector.sock> --allowed-uid <uid>` | Root system LaunchDaemon |
| daemon | `daemon --socket <collector.sock> --control-socket <host.sock> --db <events.sqlite>` | Ordinary-user system LaunchDaemon using `UserName` |
| notify | `notify --control-socket <host.sock>` | Aqua agent in the target account's `~/Library/LaunchAgents` |

The installed binary is `/Library/CodePerimeter/<uid>/codeperimeter`, owned by root with mode 0755. Its directory is also root-owned and not writable by ordinary users. Root directories/files managed by this service have inherited ACLs removed to prevent ordinary-user writes beyond POSIX modes. System jobs live in `/Library/LaunchDaemons`; they do not keep executing user-editable checkout or target binaries. Copy installation files before starting. Updating requires reinstalling and restarting the jobs. The source must be a regular file owned by the target account, or a root-owned regular file readable by ordinary users. Symbolic-link leaves and sources writable by group/others are rejected.

The collector socket is fixed at `/Library/CodePerimeter/<uid>/run/collector.sock`. The entire root parent chain of an existing protected installation is checked before a dedicated run directory is created; system directories such as `/var/run` are not modified. The run directory is root-owned with mode 0750, and the socket is root-owned with mode 0660 and the target account's primary group. The service still checks the actual peer uid on every connection and accepts only the configured account. Ordinary-user clients verify path permissions and the kernel-reported root peer rather than trusting a filename alone. One collection instance streams to one consumer only.

Analysis data lives in `~/Library/Application Support/CodePerimeter`, created with mode 0700 by a privilege-dropped system command during installation. The notification plist is also written as the ordinary account. Root does not write directly into user-editable directories, avoiding privilege expansion through parent-path symlink races. The analysis host maintains private permissions for the control socket, SQLite, and its side files. Stop persistently disables both system jobs until start enables them again, avoiding automatic collection after the next boot. It also unloads the current desktop notification job; a notification agent in a future session can only wait for the disabled host. Uninstall removes this service's jobs, binary, and known temporary installation files, while preserving directory configuration and SQLite data by default.

Raw JSON does not enter temporary files, logs, or SQLite. Stdout passes through a bounded queue and Unix socket: a line is limited to 1 MiB, and the queue holds at most 32 items. No consumer, slow connections, and queue overload cause events to be dropped and the bridge-drop counter to increase, explicitly creating a coverage gap. Oversized/non-UTF-8 lines are reported separately. Collection-instance IDs and per-second heartbeats accompany frames. A heartbeat proves only bridge liveness, not a healthy ES source, complete event delivery, or notification display. The host records stream termination, source exit, and new instances separately.

eslogger stderr is classified briefly into static health messages without displaying raw fields. On source exit, failure state is retained briefly for the host to read before launchd restarts the job under its throttling policy. A new source instance cannot be treated as gap-free continuous collection. This process does not write SQLite or own file-rule/notification state.

## Language and system windows

The web console, subsequent notifications, product-generated native prompts, and default CLI human messages share the current Mac user's persistent language preference. Simplified Chinese and English are supported; Follow system is the default, and unsupported languages fall back to English. Notifications are generated in the background, so they continue using the saved preference after the console closes. Delivered notifications retain their original language. Use `--language en` or `--language zh-CN` for a temporary CLI override without changing the saved preference.

macOS controls the language of its own administrator authorization, notification-permission, and System Settings windows. The product cannot force those windows to use its language. Language choice does not change collector privileges, monitoring scope, or event semantics, and switching does not restart collection. Original records, user notes, machine fields, and error codes are not rewritten. The console provides current-language explanations from stable codes; unrecognized older text and third-party text retain their originals.

This iteration's bilingual background notifications, cross-interface consistency, and restart persistence must be reported from integrated real validation. Earlier service validation does not replace multilingual validation. See [Language and display](usage.en.md#language-and-display) for the menu and command examples.

## Permissions and validation limits

Real installation/management requires administrator privileges. The API returns a clear error under ordinary privileges. The CLI can show the full plan before the user runs administrator commands. It does not change sudoers, SIP, AMFI, or the TCC database, or restart the machine automatically.

The local `eslogger(1)` manual requires root and Full Disk Access (FDA) for the responsible process. When running directly as a LaunchDaemon, it requires authorizing eslogger itself. The chain where root codeperimeter wraps and launches eslogger still needs real verification: authorizing a terminal and eslogger does not automatically authorize the wrapper chain. If `ES_NEW_CLIENT_RESULT_ERR_NOT_PERMITTED` is reported, check permissions against the error and the actual background responsible process. If needed, grant FDA to the installed `/Library/CodePerimeter/<uid>/codeperimeter` in System Settings.

The route without a user-created Apple developer certificate is currently a local CLI prototype only. Existing system tools' ES authorization does not become the project's AUTH interception entitlement. Installation, background FDA, logout/pre-login behavior, restart recovery, and performance must be validated with real runtime evidence. Generated plists, IPC substitutes, or successful parsing of samples do not prove that the system background service works.

In tests, `CollectorClient::connect_expected` explicitly accepts a substitute service for the current test account; production `connect` always checks for root. Test frames use `test-only-run` and `test_mode` and do not claim real system collection. Anonymous tests verify the real Unix peer API, path permissions, bounds/disconnection, bounded queues, role argv, and local `plutil` syntax.

Sources: local macOS 15.6.1 manuals for `eslogger(1)`, `launchd.plist(5)`, and `getpeereid(3)`, and [Apple launchd documentation](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html). Checked on 2026-10-03.
