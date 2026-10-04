#!/usr/bin/env python3
"""真实 macOS 验收入口：普通用户分析，root 仅采集；失败不会回退模拟事件。"""

import argparse
from collections import Counter
import hashlib
import json
import math
import mmap
import os
from pathlib import Path
import socket
import sqlite3
import stat
import subprocess
import sys
import tempfile
import re
import threading
import time

REPO = Path(__file__).resolve().parent.parent
SENDER = REPO / "scripts" / "synthetic_sender.py"
KINDS = ("open", "mmap", "exec", "fork", "exit", "create", "write", "rename", "close")


def save(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    path.chmod(0o600)


def control(path, operation, payload=None):
    request = {"operation": operation}
    if payload is not None:
        request["payload"] = payload
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(3)
        connection.connect(str(path))
        connection.sendall(json.dumps(request).encode() + b"\n")
        with connection.makefile("rb") as stream:
            line = stream.readline(8 * 1024 * 1024 + 1)
        if len(line) > 8 * 1024 * 1024:
            raise RuntimeError("控制响应超出验收上限")
    response = json.loads(line)
    if not response.get("ok"):
        raise RuntimeError(response.get("error") or "宿主拒绝请求")
    return response["data"]


class CollectorStartupError(RuntimeError):
    pass


class BridgeIdentityError(CollectorStartupError):
    def __init__(self, code):
        self.code = code
        super().__init__("采集桥接身份核验失败：" + code + "；拒绝控制未经本次启动确认的进程")


def startup_diagnostics(stream, codes):
    # 只保存白名单静态分类，原 stderr 持续有界排空，不保存原文或全系统事件。
    patterns = (("root 服务文件或父目录可被普通用户修改", "unsafe_root_path"),
                ("服务目录不是受信任的私有目录", "unsafe_run_directory"),
                ("拒绝替换非本服务的 socket", "untrusted_endpoint"),
                ("采集服务已运行", "endpoint_in_use"),
                ("password is required", "sudo_authorization_required"),
                ("a password is required", "sudo_authorization_required"),
                ("No such file or directory", "missing_install_path"),
                ("Permission denied", "permission_denied"))
    while True:
        chunk = stream.readline(4096)
        if not chunk:
            break
        line = chunk.decode("utf-8", errors="replace")
        code = next((code for text, code in patterns if text in line), "unclassified_stderr")
        if code not in codes and len(codes) < 8:
            codes.append(code)
    stream.close()


def wait_for_bridge(control_socket, launcher, collector, diagnostics=None, timeout=15):
    diagnostics = diagnostics if diagnostics is not None else {}
    diagnostics.update(result="waiting", reason="bridge_status_unavailable", status_received=False, run_id_present=False)
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        code = launcher.poll()
        if code is not None:
            diagnostics.update(result="failed", reason="collector_exited")
            raise CollectorStartupError(f"root collector 在连接前退出，退出码 {code}；见 root_startup 静态诊断")
        try:
            status = control(control_socket, "status")
        except (OSError, ValueError, RuntimeError):
            diagnostics["reason"] = "bridge_status_unavailable"
            time.sleep(.1)
            continue
        diagnostics.update(status_received=True, run_id_present=bool(status.get("collector_run_id")))
        if not status.get("collector_run_id"):
            diagnostics["reason"] = "bridge_run_id_missing"
            time.sleep(.1)
            continue
        try:
            identity = bridge_identity(status, launcher, collector)
        except BridgeIdentityError as error:
            diagnostics.update(result="failed", reason=error.code)
            raise
        if identity:
            diagnostics.update(result="verified", reason="root_pid_path_group_and_sudo_ancestry_verified")
            return identity
        diagnostics["reason"] = "collector_exited"
        time.sleep(.1)
    diagnostics["result"] = "timeout"
    raise TimeoutError("采集桥接等待超时：" + diagnostics["reason"] + "；没有使用 fixture")


def wait_for(callback, timeout=15, timeout_message="等待真实宿主／采集事件超时；没有使用 fixture"):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            result = callback()
            if result:
                return result
        except CollectorStartupError:
            raise
        except (OSError, ValueError, RuntimeError):
            pass
        time.sleep(0.1)
    raise TimeoutError(timeout_message)


def check_root_chain(path):
    if not path.is_absolute():
        raise RuntimeError("采集二进制必须是绝对路径")
    for candidate in [*reversed(path.parents), path]:
        value = candidate.lstat()
        if stat.S_ISLNK(value.st_mode) or value.st_uid != 0 or value.st_mode & 0o022:
            raise RuntimeError("采集二进制及父目录必须 root-owned 且普通用户不可写")
    # 系统目录中的 deny ACL 可以保留；拒绝父目录或文件中的额外写授权。
    for candidate in [*reversed(path.parents), path]:
        acl = subprocess.run(["/bin/ls", "-lde", str(candidate)], capture_output=True, text=True, check=True).stdout
        for entry in acl.splitlines()[1:]:
            if " allow " in entry and any(right in entry for right in ("write", "delete", "add_file", "add_subdirectory", "chown")):
                raise RuntimeError("采集二进制或父目录存在额外写 ACL；先按说明检查自有安装目录")


def trusted_collector(path):
    check_root_chain(path)
    if not stat.S_ISREG(path.lstat().st_mode):
        raise RuntimeError("采集二进制不是普通文件")


def preflight(binary, collector, collector_socket):
    if os.geteuid() == 0:
        raise RuntimeError("请以普通用户运行验收；root 只用于 collector")
    if sys.platform != "darwin":
        raise RuntimeError("真实验收仅支持 macOS，未回退合成输入")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("先 cargo build --release；未找到可执行的普通用户 CLI")
    trusted_collector(collector)
    if hashlib.sha256(binary.read_bytes()).digest() != hashlib.sha256(collector.read_bytes()).digest():
        raise RuntimeError("普通用户 CLI 与受保护 collector 二进制不同；先按说明同步版本")
    if subprocess.run(["/usr/bin/sudo", "-n", "/usr/bin/true"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        raise RuntimeError("当前终端没有 sudo 授权；先在自己的终端运行 sudo -v")
    if os.path.lexists(collector_socket):
        # 不连接已有端点，避免占用正在运行的单消费者桥接。
        raise RuntimeError("固定采集端点已存在（可能是现有服务或残留）；先检查并明确处理，验收不会连接或删除它")
    return {
        "os": subprocess.check_output(["/usr/bin/sw_vers", "-productVersion"], text=True).strip(),
        "python": sys.version.split()[0],
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "binary_version": subprocess.check_output([str(binary), "--version"], text=True).strip(),
        "eslogger": "/usr/bin/eslogger (系统版本同 OS)",
        "permission_model": "普通用户 daemon/notify，root collector；FDA 以真实事件核验",
    }


def sender(workspace, scenario, scope="inside"):
    result = subprocess.run([sys.executable, "-B", str(SENDER), "run", "--root", str(workspace),
                             "--scenario", scenario, "--output-scope", scope],
                            capture_output=True, text=True, timeout=45, start_new_session=True)
    rows = [json.loads(line) for line in result.stdout.splitlines()]
    if result.returncode or not rows or rows[-1].get("phase") != "completed":
        raise RuntimeError(f"合成操作 {scenario}/{scope} 失败，退出码 {result.returncode}")
    return rows


def matrix_worker(project):
    directory = project / ".event-matrix"
    directory.mkdir()
    first, second = directory / "first.txt", directory / "renamed.txt"
    with first.open("xb") as stream:
        stream.write(b"anonymous nine-event scenario")
        stream.flush()
    with first.open("rb") as stream:
        stream.read()
        with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as view:
            view[:]
    first.rename(second)
    child = os.fork()
    if child == 0:
        with second.open("rb") as stream:
            stream.read()
        os.execl("/usr/bin/true", "true")
    _, status = os.waitpid(child, 0)
    if status:
        raise RuntimeError("显式 fork／exec 合成操作失败")
    print(json.dumps({"source": "validation_worker", "scenario": "nine-events",
                      "pid": os.getpid(), "child_pid": child, "success": True}))


def load_evidence(database):
    # 仅从本轮匿名数据库读取；唯一写入方仍是普通用户 daemon。
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=1) as connection:
        schema = connection.execute("PRAGMA user_version").fetchone()[0]
        if schema != 3:
            raise RuntimeError("验收读取器只支持已锁定的 SQLite schema 3")
        events = [json.loads(row[0]) for row in connection.execute("SELECT event_json FROM events ORDER BY id")]
        alerts = [json.loads(row[0]) for row in connection.execute("SELECT alert_json FROM alerts ORDER BY rowid")]
        outbox = {row[0]: row[1] for row in connection.execute("SELECT alert_id,created_timestamp_ms FROM notification_outbox")}
        feedback = [{"alert_id": row[0], "observed_timestamp_ms": row[1], "outcome": row[2]}
                    for row in connection.execute("SELECT alert_id,observed_timestamp_ms,outcome FROM notification_feedback ORDER BY id")]
        health = [{"observed_timestamp_ms": row[0], "component": row[1], "code": row[2], "state": row[3],
                   "detail": row[4], "source": json.loads(row[5]) if row[5] else None}
                  for row in connection.execute("SELECT observed_timestamp_ms,component,code,state,detail,source_json FROM health_records ORDER BY id")]
    return {"sqlite_schema": schema, "events": events, "alerts": alerts, "outbox": outbox,
            "notifications": feedback, "health": health}


def process_key(event):
    identity = event["process"]
    return event["source_run_id"], identity["pid"], identity["pid_version"]


def latency_summary(values, missing):
    ordered = sorted(values)
    return {"sample_count": len(values), "missing_count": missing,
            "max_ms": max(values) if values else None,
            "p95_ms": ordered[math.ceil(len(ordered) * .95) - 1] if ordered else None,
            "within_3000_ms": bool(values) and missing == 0 and all(0 <= value <= 3000 for value in values)}


def triggers(evidence, project):
    windows, recent_reads, candidates = {}, {}, {}
    events = sorted(evidence["events"], key=lambda event: (event["source_timestamp_ms"] or 0, event["global_seq"] or 0))
    for event in events:
        timestamp = event["source_timestamp_ms"]
        if timestamp is None:
            continue
        key = process_key(event)
        file = event.get("file")
        if event["kind"] in ("open", "mmap") and file and file.get("readable") is True and not file["path_truncated"]:
            path = Path(file["path"])
            if project in path.parents and file.get("is_regular") is not False:
                recent_reads[key] = timestamp
                window = windows.setdefault(key, {})
                window = {identity: previous for identity, previous in window.items() if previous >= timestamp - 10000}
                identity = (file["device"], file["inode"]) if file["device"] is not None and file["inode"] is not None else file["path"]
                window[identity] = timestamp
                windows[key] = window
                if len(window) >= 50:
                    candidates.setdefault((key, "bulk_file_access"), []).append(timestamp)
        if event["kind"] == "exec" and event.get("archive"):
            candidates.setdefault((key, "archive_command"), []).append(timestamp)
        if event["kind"] in ("create", "write", "rename"):
            paths = [file["path"]] if file else []
            if event.get("destination"):
                paths.append(event["destination"])
            if key in recent_reads and 0 <= timestamp - recent_reads[key] <= 60000 and any(path.lower().endswith((".zip", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz")) for path in paths):
                candidates.setdefault((key, "archive_output"), []).append(timestamp)
    return candidates


def canonical_path(path):
    # 合成输出可能经 /var 或 /tmp 别名访问；比较的是同一落盘路径。
    return str(Path(path).resolve()) if path else None


def analyze(evidence, operations, project, initial_status, final_status):
    candidates = triggers(evidence, project)
    generation_delays, notification_delays, timing_rows = [], [], []
    missing_generation = missing_notification = 0
    for alert in evidence["alerts"]:
        keys = [key for key, rule in candidates if rule == alert["rule"] and key[1:] == (alert["process"]["pid"], alert["process"]["pid_version"])]
        possible = [value for key in keys for value in candidates[(key, alert["rule"])]
                    if alert["first_timestamp_ms"] <= value <= alert["last_timestamp_ms"]] if len(keys) == 1 else []
        generated = evidence["outbox"].get(alert["id"])
        trigger = min(possible) if possible else None
        sent = [row["observed_timestamp_ms"] for row in evidence["notifications"]
                if row["alert_id"] == alert["id"] and row["outcome"] == "sent"]
        notified = min(sent) if sent else None
        if trigger is None or generated is None:
            missing_generation += 1
        else:
            generation_delays.append(generated - trigger)
        if trigger is None or notified is None:
            missing_notification += 1
        else:
            notification_delays.append(notified - trigger)
        timing_rows.append({"alert_id": alert["id"], "rule": alert["rule"], "trigger_source_ms": trigger,
                            "generated_host_ms": generated, "notification_feedback_ms": notified,
                            "generation_delay_ms": generated - trigger if generated is not None and trigger is not None else None,
                            "notification_delay_ms": notified - trigger if notified is not None and trigger is not None else None})

    cases = []
    for operation in operations:
        rows = operation["metadata"]
        pids = {row["pid"] for row in rows if row.get("pid")}
        pids.update(row["child_pid"] for row in rows if row.get("child_pid"))
        events = [event for event in evidence["events"] if event["process"]["pid"] in pids]
        alerts = [alert for alert in evidence["alerts"] if alert["process"]["pid"] in pids]
        source_reads = {canonical_path(event["file"]["path"]) for event in events if event["kind"] in ("open", "mmap")
                        and event.get("file") and event["file"].get("readable") is True
                        and Path(canonical_path(event["file"]["path"])).parent == project / "src"}
        scenario = operation["scenario"]
        rules = Counter(alert["rule"] for alert in alerts)
        expected_files = 0 if scenario == "preloaded-memory" else 1 if scenario in ("read", "mmap", "repeat-read") else 55
        expected_outputs = {canonical_path(row["output_path"]) for row in rows if row.get("output_path")}
        observed_outputs, associated_outputs = set(), set()
        for output_event in events:
            if output_event["kind"] not in ("create", "write", "rename"):
                continue
            file = output_event.get("file")
            paths = {canonical_path(output_event.get("destination"))}
            if file and not file["path_truncated"]:
                paths.add(canonical_path(file["path"]))
            matches = paths & expected_outputs
            observed_outputs.update(matches)
            timestamp = output_event["source_timestamp_ms"]
            if not matches or timestamp is None:
                continue
            related_read = any(process_key(read) == process_key(output_event)
                               and read["kind"] in ("open", "mmap") and read.get("file")
                               and read["file"].get("readable") is True and not read["file"]["path_truncated"]
                               and canonical_path(read["file"]["path"]) in source_reads
                               and read["source_timestamp_ms"] is not None
                               and abs(timestamp - read["source_timestamp_ms"]) <= 60000 for read in events)
            related_alert = any(alert["rule"] == "archive_output"
                                and (alert["process"]["pid"], alert["process"]["pid_version"]) == process_key(output_event)[1:]
                                and canonical_path(project) in {canonical_path(root) for root in alert.get("roots", [])}
                                and alert["first_timestamp_ms"] <= timestamp <= alert["last_timestamp_ms"] for alert in alerts)
            # 外部工具可先创建输出再读源码；实际EXEC的输出参数和项目告警补充关联。
            related_command = scenario in ("tar", "zip") and any(
                command["kind"] == "exec" and command.get("archive")
                and process_key(command) == process_key(output_event)
                and canonical_path(command["archive"].get("output_path")) in matches
                and command["source_timestamp_ms"] is not None
                and 0 <= timestamp - command["source_timestamp_ms"] <= 60000
                and any(alert["rule"] == "archive_command"
                        and (alert["process"]["pid"], alert["process"]["pid_version"]) == process_key(command)[1:]
                        and canonical_path(project) in {canonical_path(root) for root in alert.get("roots", [])}
                        and alert["first_timestamp_ms"] <= command["source_timestamp_ms"] <= alert["last_timestamp_ms"]
                        for alert in alerts) for command in events)
            if related_read and (related_alert or related_command):
                associated_outputs.update(matches)
        missing_evidence = []
        if scenario == "nine-events":
            passed = any(event["kind"] == "rename" for event in events)
        else:
            passed = len(source_reads) == expected_files
            if expected_files == 55:
                passed &= rules["bulk_file_access"] >= 1
            if scenario in ("read", "mmap", "repeat-read", "preloaded-memory"):
                passed &= not rules["bulk_file_access"]
            if scenario in ("tar", "zip"):
                passed &= rules["archive_command"] >= 1
            if scenario in ("tar", "zip", "disk-archive"):
                if not expected_outputs:
                    missing_evidence.append("发送器未声明预期归档输出")
                if expected_outputs - observed_outputs:
                    missing_evidence.append("缺少预期归档输出的实际 create/write/rename 事件")
                if expected_outputs - associated_outputs:
                    missing_evidence.append("缺少归档输出与保护项目的告警关联证据")
                passed &= bool(expected_outputs) and expected_outputs == associated_outputs
            if scenario == "mmap":
                passed &= any(event["kind"] == "mmap" and event.get("file", {}).get("readable") is True for event in events)
        cases.append({"scenario": scenario, "output_scope": operation["scope"], "passed": bool(passed),
                      "expected_source_files": expected_files, "observed_source_files": len(source_reads),
                      "standard_event_count": len(events), "alerts_by_rule": dict(rules),
                      "expected_output_paths": sorted(expected_outputs),
                      "observed_output_paths": sorted(observed_outputs),
                      "project_associated_output_paths": sorted(associated_outputs),
                      "missing_evidence": missing_evidence,
                      "normal_workload": scenario in ("search", "index", "build")})
    delta = {kind: final_status["observed_events_by_kind"][kind] - initial_status["observed_events_by_kind"][kind] for kind in KINDS}
    return {"cases": cases, "nine_event_observed_delta": delta,
            "nine_event_aggregate_passed": all(value > 0 for value in delta.values()),
            "generation_latency": latency_summary(generation_delays, missing_generation),
            "notification_send_latency": latency_summary(notification_delays, missing_notification),
            "timing": timing_rows, "notification_display": "未验；sent 只表示发送命令接受",
            "normal_workload_notes": "搜索／索引／构建也可能触发批量访问；这是阈值校准样本，不代表恶意或确认压缩"}


def children(pid):
    result = subprocess.run(["/usr/bin/pgrep", "-P", str(pid)], capture_output=True, text=True)
    return [int(value) for value in result.stdout.split()] if result.returncode in (0, 1) else []


def performance(processes, stop, samples):
    while not stop.wait(.5):
        pids = {process.pid for process in processes if process.poll() is None}
        for _ in range(3):
            pids.update(child for pid in list(pids) for child in children(pid))
        if not pids:
            continue
        output = subprocess.run(["/bin/ps", "-o", "rss=,%cpu=", "-p", ",".join(map(str, pids))], capture_output=True, text=True).stdout
        values = [line.split() for line in output.splitlines()]
        if values:
            samples.append({"rss_kib_sum": sum(int(row[0]) for row in values), "ps_cpu_percent_sum": sum(float(row[1]) for row in values)})


def process_info(pid):
    result = subprocess.run(["/bin/ps", "-o", "uid=,ppid=,pgid=,comm=", "-p", str(pid)], capture_output=True, text=True)
    parts = result.stdout.strip().split(maxsplit=3)
    return (int(parts[0]), int(parts[1]), int(parts[2]), parts[3]) if len(parts) == 4 else None


def bridge_identity(status, launcher, collector):
    run_id = status.get("collector_run_id")
    if not run_id or launcher.poll() is not None:
        return None
    matched = re.fullmatch(r"eslogger-([1-9][0-9]*)-([0-9]+)", run_id)
    if not matched:
        raise BridgeIdentityError("invalid_run_id")
    pid = int(matched[1])
    info = process_info(pid)
    if not info:
        raise BridgeIdentityError("collector_process_missing")
    if info[0] != 0:
        raise BridgeIdentityError("collector_not_root")
    if info[2] != pid:
        raise BridgeIdentityError("collector_process_group_mismatch")
    if Path(info[3]) != collector:
        raise BridgeIdentityError("collector_executable_mismatch")
    parent = pid
    for _ in range(8):
        if parent == launcher.pid:
            return {"pid": pid, "pgid": pid, "collector": str(collector), "source_pids": children(pid)}
        current = process_info(parent)
        if not current or current[1] <= 1:
            break
        parent = current[1]
    raise BridgeIdentityError("collector_sudo_ancestry_mismatch")


def stop_root(bridge, launcher):
    if not launcher:
        return True
    if bridge and launcher.poll() is None:
        info = process_info(bridge["pid"])
        if not info or info[0] != 0 or info[2] != bridge["pgid"] or info[3] != bridge["collector"]:
            return False
        if subprocess.run(["/usr/bin/sudo", "-n", "/bin/kill", "-TERM", str(bridge["pid"])], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            return False
    elif launcher.poll() is None:
        # 尚未收到可信 run_id 时只请求本次 sudo 转发停止；不猜测 collector PID。
        if subprocess.run(["/usr/bin/sudo", "-n", "/bin/kill", "-TERM", str(launcher.pid)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            return False
    try:
        launcher.wait(timeout=10)
    except subprocess.TimeoutExpired:
        return False
    if bridge:
        for pid in bridge["source_pids"]:
            if process_info(pid) is not None:
                return False
    return True


def start_preloaded(workspace):
    process = subprocess.Popen([sys.executable, "-B", str(SENDER), "run", "--root", str(workspace), "--scenario", "preloaded-memory", "--release-timeout", "180"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, start_new_session=True)
    rows, ready = [], threading.Event()
    def read_rows():
        for line in process.stdout:
            row = json.loads(line)
            rows.append(row)
            if row["phase"] == "ready":
                ready.set()
    reader = threading.Thread(target=read_rows, daemon=True)
    reader.start()
    if not ready.wait(15):
        process.kill()
        process.wait()
        reader.join(timeout=3)
        raise TimeoutError("预加载没有在监控前完成")
    return process, rows, reader


def run(args, report, summary):
    summary["phase"] = "preflight"
    binary = args.binary.resolve()
    collector = args.collector_binary or Path(f"/Library/CodePerimeter/{os.getuid()}/codeperimeter")
    collector_socket = Path(f"/Library/CodePerimeter/{os.getuid()}/run/collector.sock")
    summary["environment"] = preflight(binary, collector, collector_socket)
    if args.preflight_only:
        summary["result"] = "preflight_passed_real_run_not_started"
        return 0
    summary["phase"] = "synthetic_project_prepare"
    workspace, host = report / "workspace", report / "host"
    host.mkdir(mode=0o700)
    control_socket, database = host / "host.sock", host / "events.sqlite"
    prepared = subprocess.run([sys.executable, "-B", str(SENDER), "prepare", "--root", str(workspace), "--files", "55"], capture_output=True, text=True, check=True, start_new_session=True)
    project = Path(json.loads(prepared.stdout)["project_root"]).resolve()
    preloaded, preload_rows, preload_reader = start_preloaded(workspace)
    daemon = notifier = root_process = None
    bridge = None
    stop_sampling, samples = threading.Event(), []
    operations = []
    startup_codes, startup_reader = [], None
    try:
        summary["phase"] = "host_startup"
        daemon = subprocess.Popen([str(binary), "daemon", "--socket", str(collector_socket), "--control-socket", str(control_socket), "--db", str(database)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        wait_for(lambda: control(control_socket, "status"))
        control(control_socket, "add_directories", {"entries": [{"path": str(project), "sources": ["manual"]}]})
        configured = control(control_socket, "list_directories")
        if len(configured) != 1 or Path(configured[0]["path"]) != project:
            raise RuntimeError("匿名监控配置不是唯一合成项目，拒绝继续")
        summary["phase"] = "collector_startup"
        root_process = subprocess.Popen(["/usr/bin/sudo", "-n", str(collector), "collector", "--socket", str(collector_socket), "--allowed-uid", str(os.getuid())], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        startup_reader = threading.Thread(target=startup_diagnostics, args=(root_process.stderr, startup_codes), daemon=True)
        startup_reader.start()
        notifier = subprocess.Popen([str(binary), "notify", "--control-socket", str(control_socket)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        monitor = threading.Thread(target=performance, args=([root_process, daemon, notifier], stop_sampling, samples), daemon=True)
        monitor.start()
        # 桥接 connected 不等于实际 ES 可读；先要求真实合成文件事件。
        summary["phase"] = "bridge_identity"
        summary["bridge_identity"] = {}
        bridge = wait_for_bridge(control_socket, root_process, collector, summary["bridge_identity"])
        initial = control(control_socket, "status")
        summary["phase"] = "first_synthetic_read"
        operations.append({"scenario": "read", "scope": "inside", "metadata": sender(workspace, "read")})
        read_pids = {row["pid"] for row in operations[0]["metadata"] if row.get("pid")}
        wait_for(lambda: any(event["process"]["pid"] in read_pids
                             and event["kind"] in ("open", "mmap")
                             and event["file"].get("readable") is True
                             and Path(event["file"]["path"]).parent == project / "src"
                             for event in load_evidence(database)["events"] if event.get("file")), 15,
                 "首条合成读取事件超时；身份核验与项目读取分别验收，没有使用 fixture")
        summary["real_source_confirmed"] = True
        summary["phase"] = "synthetic_scenario_matrix"
        preloaded.stdin.write("release\n")
        preloaded.stdin.flush()
        preloaded.wait(timeout=30)
        preload_reader.join(timeout=3)
        if preloaded.returncode or preload_rows[-1]["phase"] != "completed":
            raise RuntimeError("预加载释放后的纯内存压缩失败")
        operations.append({"scenario": "preloaded-memory", "scope": "memory", "metadata": preload_rows})
        matrix = subprocess.run([sys.executable, "-B", str(Path(__file__).resolve()), "--event-matrix-worker", str(project)], capture_output=True, text=True, timeout=15, check=True, start_new_session=True)
        operations.append({"scenario": "nine-events", "scope": "inside", "metadata": [json.loads(matrix.stdout)]})
        for scenario, scope in [("mmap", "inside"), ("repeat-read", "inside"), ("bulk-read", "inside"),
                                ("tar", "inside"), ("tar", "temporary"), ("zip", "inside"), ("zip", "temporary"),
                                ("disk-archive", "inside"), ("disk-archive", "temporary"), ("memory-archive", "inside"),
                                ("search", "inside"), ("index", "inside"), ("build", "inside")]:
            operations.append({"scenario": scenario, "scope": scope, "metadata": sender(workspace, scenario, scope)})
            try:
                wait_for(lambda: not control(control_socket, "pending_notifications", {"limit": 10000}), 7)
                operations[-1]["notification_queue_drained"] = True
            except TimeoutError:
                operations[-1]["notification_queue_drained"] = False
        summary["phase"] = "evidence_analysis"
        time.sleep(3)
        final_status = control(control_socket, "status")
        evidence = load_evidence(database)
        save(report / "operations.json", operations)
        save(report / "standard-evidence.json", evidence)
        summary["initial_status"], summary["final_status"] = initial, final_status
        summary["verification"] = analyze(evidence, operations, project, initial, final_status)
        summary["coverage"] = {
            "collector_dropped_lines": final_status["collector_dropped_lines"],
            "reader_dropped_frames": final_status["reader_dropped_frames"],
            "database_gap_events": final_status["database_gap_events"],
            "health_counts": dict(Counter(row["code"] for row in evidence["health"])),
            "degraded_health_records": sum(row["state"] in ("degraded", "gap", "failed", "error", "permission_denied", "coverage_gap") for row in evidence["health"]),
        }
        summary["phase"] = "controlled_disconnect"
        summary["collector_stopped"] = stop_root(bridge, root_process)
        time.sleep(1.5)
        fault_status = control(control_socket, "status")
        save(report / "fault-evidence.json", load_evidence(database))
        summary["controlled_disconnect"] = {"collector_state": fault_status["collector_state"], "state": fault_status["state"], "observed": fault_status["collector_state"] != final_status["collector_state"]}
        verification = summary["verification"]
        healthy = final_status["database_state"] == "ready" and not any(summary["coverage"][field] for field in ("collector_dropped_lines", "reader_dropped_frames", "database_gap_events", "degraded_health_records"))
        passed = healthy and summary["collector_stopped"] and summary["controlled_disconnect"]["observed"] and all(case["passed"] for case in verification["cases"]) and verification["nine_event_aggregate_passed"] and verification["generation_latency"]["within_3000_ms"] and verification["notification_send_latency"]["within_3000_ms"]
        summary["phase"] = "completed"
        summary["result"] = "real_run_passed_display_and_boot_pending" if passed else "real_run_failed_or_partial"
        return 0 if passed else 1
    finally:
        stop_sampling.set()
        summary["performance"] = {"sample_count": len(samples), "peak_rss_kib_sum": max((row["rss_kib_sum"] for row in samples), default=None), "peak_ps_cpu_percent_sum": max((row["ps_cpu_percent_sum"] for row in samples), default=None), "method": "后台角色及直接后代的RSS求和；ps %cpu是累计平均，无基线或性能达标结论"}
        if database.exists():
            try:
                save(report / "failure-or-final-evidence.json", load_evidence(database))
                summary["last_status"] = control(control_socket, "status")
                summary["source_observation"] = {
                    "system_events_observed": sum(summary["last_status"]["observed_events_by_kind"].values()) > 0,
                    "synthetic_project_read_confirmed": summary["real_source_confirmed"],
                    "method": "系统事件计数与指定合成进程可读文件事件分别报告",
                }
            except (OSError, RuntimeError, ValueError, sqlite3.Error):
                summary["evidence_read_gap"] = True
        if preloaded.poll() is None:
            preloaded.kill()
            preloaded.wait()
        for process in (notifier, daemon):
            if process and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        summary["root_cleanup_complete"] = stop_root(bridge, root_process)
        if startup_reader:
            startup_reader.join(timeout=3)
        summary["root_startup"] = {"exit_code": root_process.poll() if root_process else None,
                                   "diagnostic_codes": list(startup_codes),
                                   "stderr_complete": startup_reader is not None and not startup_reader.is_alive(),
                                   "method": "最多8种白名单静态分类；原stderr不落盘"}
        summary["artifacts_retained"] = "保留匿名证据与合成项目供回查；未删除用户数据或修改服务配置"
        save(report / "operations.json", operations)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=REPO / "target" / "release" / "codeperimeter")
    parser.add_argument("--collector-binary", type=Path)
    parser.add_argument("--report-dir", type=Path)
    parser.add_argument("--preflight-only", action="store_true")
    parser.add_argument("--event-matrix-worker", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    os.umask(0o077)
    if args.event_matrix_worker:
        matrix_worker(args.event_matrix_worker)
        return 0
    report = args.report_dir or Path(tempfile.mkdtemp(prefix="codeperimeter-validation-", dir="/private/tmp"))
    if report.is_symlink() or report.exists() and (not report.is_dir() or any(report.iterdir()) or report.stat().st_uid != os.getuid()):
        parser.error("report-dir 必须不存在或为空，拒绝覆盖已有证据")
    report.mkdir(mode=0o700, parents=True, exist_ok=True)
    report.chmod(0o700)
    report = report.resolve()
    summary = {"schema_version": 1, "started_timestamp_ms": time.time_ns() // 1_000_000,
               "result": "not_started", "real_source_confirmed": False,
               "pending": ["桌面通知实际到屏", "launchd安装/FDA", "注销/登录补发", "重启/未登录运行", "FileVault解锁前能力", "性能基线与阈值校准"]}
    try:
        code = run(args, report, summary)
    except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError, sqlite3.Error, KeyError, KeyboardInterrupt) as error:
        summary["result"] = "failed_no_fixture_fallback"
        summary["failure"] = {"phase": summary.get("phase", "not_started"), "code": getattr(error, "code", None), "type": type(error).__name__, "message": str(error).replace(str(REPO), "<repo>").replace(str(Path.home()), "<home>")}
        code = 2
    if summary.get("root_cleanup_complete") is False:
        summary["result"] = "failed_root_cleanup_incomplete"
        code = 2
    summary["finished_timestamp_ms"] = time.time_ns() // 1_000_000
    save(report / "summary.json", summary)
    print(json.dumps({"result": summary["result"], "summary": str(report / "summary.json")}, ensure_ascii=False))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
