#!/usr/bin/env python3
"""逐项验证归档命令的真实 eslogger 到通知证据链。"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import uuid


REPO = Path(__file__).resolve().parent.parent
VALIDATE_MVP_PATH = REPO / "scripts" / "validate-mvp.py"
TOOLS = (
    "tar", "bsdtar", "gtar", "zip", "ditto", "gzip", "pigz", "bzip2",
    "pbzip2", "xz", "zstd", "7z", "7zz", "rar",
)
UPDATE_TOOLS = {"tar", "bsdtar", "gtar", "zip", "7z", "7zz", "rar"}
STDOUT_TOOLS = set(TOOLS) - {"rar"}
STDIN_TOOLS = set(TOOLS) - {"tar", "bsdtar", "gtar", "ditto"}
REVERSE_MODES = {
    "tar": ("reverse_list", "reverse_extract"),
    "bsdtar": ("reverse_list", "reverse_extract"),
    "gtar": ("reverse_list", "reverse_extract"),
    "zip": ("reverse_list", "reverse_test"),
    "ditto": ("reverse_extract",),
    "gzip": ("reverse_list", "reverse_test", "reverse_decompress"),
    "pigz": ("reverse_list", "reverse_test", "reverse_decompress"),
    "bzip2": ("reverse_test", "reverse_decompress"),
    "pbzip2": ("reverse_test", "reverse_decompress"),
    "xz": ("reverse_list", "reverse_test", "reverse_decompress"),
    "zstd": ("reverse_list", "reverse_test", "reverse_decompress"),
    "7z": ("reverse_list", "reverse_test", "reverse_extract"),
    "7zz": ("reverse_list", "reverse_test", "reverse_extract"),
    "rar": ("reverse_list", "reverse_test", "reverse_extract"),
}
CONTENT_SENTINEL = b"SYNTHETIC_ARCHIVE_CONTENT_SENTINEL"
PASSWORD_SENTINEL = "SYNTHETIC_ARCHIVE_PASSWORD_SENTINEL"
ARCHIVE_STATIC_COVERAGE_CODES = {
    "archive_arguments_incomplete", "archive_input_list_unsupported",
    "archive_input_source_unknown",
}
PAYLOAD_BYTES = 1024 * 1024
REAL_CASE_TIMEOUT_SECONDS = 8
TOOL_TIMEOUT_SECONDS = 60
FAILURE_CODES = {
    "tool_missing", "version_probe_failed", "version_unavailable", "tool_start_failed",
    "tool_exit_nonzero", "output_missing", "stdout_empty", "exec_missing",
    "exec_identity_mismatch", "archive_metadata_mismatch", "archive_command_alert_missing",
    "archive_command_alert_wrong_scope", "notification_outbox_missing",
    "notification_feedback_missing", "generation_latency_negative",
    "notification_latency_negative", "generation_latency_over_3000ms",
    "notification_latency_over_3000ms", "reverse_exec_missing",
    "reverse_archive_command_alert", "unrelated_exec_missing", "unrelated_archive_metadata_missing",
    "unrelated_project_alert", "negative_fence_missing", "negative_fence_not_independent",
    "negative_fence_unhealthy", "collector_unhealthy", "collector_dropped_events",
    "database_gap", "unexpected_coverage_gap", "privacy_marker_persisted",
    "collector_cleanup_incomplete", "report_dir_inside_repository", "report_dir_unsafe",
    "update_seed_failed", "case_timeout",
    "run_as_root", "unsupported_platform", "binary_missing", "collector_mismatch",
    "sudo_authorization_required", "collector_endpoint_in_use", "collector_untrusted",
    "startup_timeout", "host_startup_failed", "collector_startup_failed",
    "sidecar_exec_ambiguous", "sidecar_exec_identity_missing", "sidecar_stream_malformed",
    "sidecar_stream_failed", "sidecar_stream_ended", "sidecar_sequence_missing",
    "sidecar_sequence_gap", "sidecar_sequence_regression", "sidecar_unexpected_event",
    "sidecar_not_root", "sidecar_root_unverified", "sidecar_es_client_denied",
    "sidecar_cleanup_incomplete", "reverse_output_empty", "reverse_extract_output_missing",
    "stdin_write_failed", "stdin_source_gap_missing", "sidecar_exit_nonzero", "interrupted",
}


def load_mvp_helpers():
    spec = importlib.util.spec_from_file_location("codeperimeter_validate_mvp", VALIDATE_MVP_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("既有 MVP 验收辅助模块不可用")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


try:
    MVP = load_mvp_helpers()
except KeyboardInterrupt:
    print(json.dumps({"result": "interrupted", "failure_code": "interrupted"}), file=sys.stderr)
    raise SystemExit(130)
except Exception:
    print(json.dumps({"result": "failed", "failure_code": "validation_startup_failed"}), file=sys.stderr)
    raise SystemExit(2)


@dataclass
class Case:
    tool: str
    mode: str
    argv: list
    cwd: Path
    project_root: Path
    inputs: tuple
    outputs: tuple
    positive: bool
    stdout_expected: bool = False
    fence_path: Path | None = None
    preparation_failure: str | None = None
    operation_artifact: Path | None = None
    stdin_payload: bytes | None = None


def is_reverse(mode):
    return mode.startswith("reverse_")


def is_negative(mode):
    return is_reverse(mode) or mode in ("unrelated", "stdin")


def is_within(path, root):
    try:
        Path(path).resolve(strict=False).relative_to(Path(root).resolve(strict=True))
        return True
    except (OSError, ValueError):
        return False


def checked_report_path(requested):
    if requested is None:
        path = Path(tempfile.mkdtemp(prefix="codeperimeter-archive-validation-", dir="/private/tmp"))
    else:
        path = Path(requested).expanduser()
        if is_within(path, REPO):
            raise ValueError("report_dir_inside_repository")
        if path.is_symlink() or path.exists() and (
            not path.is_dir() or any(path.iterdir()) or path.stat().st_uid != os.getuid()
        ):
            raise ValueError("report_dir_unsafe")
    if is_within(path, REPO):
        raise ValueError("report_dir_inside_repository")
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)
    resolved = path.resolve(strict=True)
    if is_within(resolved, REPO) or resolved.stat().st_uid != os.getuid():
        raise ValueError("report_dir_unsafe")
    return resolved


def check_external_binary(path):
    if path is None:
        return None
    value = Path(path).expanduser()
    if not value.is_absolute():
        value = Path.cwd() / value
    if is_within(value, REPO):
        raise ValueError("工具二进制必须位于项目目录之外")
    return value


def discover_tools(tools_dir, rar_binary):
    found = {}
    base = check_external_binary(tools_dir) if tools_dir else None
    rar_override = check_external_binary(rar_binary) if rar_binary else None
    for name in TOOLS:
        candidate = None
        if name == "rar" and rar_override is not None:
            candidate = rar_override
        elif base is not None and (base / name).is_file() and os.access(base / name, os.X_OK):
            candidate = base / name
        else:
            candidate = shutil.which(name)
        if candidate is not None:
            path = Path(candidate)
            if is_within(path, REPO):
                raise ValueError("工具二进制必须位于项目目录之外")
            if path.is_file() and os.access(path, os.X_OK):
                found[name] = path.absolute()
    return found


def version_probe(name, binary):
    if name == "ditto":
        result = subprocess.run(
            ["/usr/bin/sw_vers", "-productVersion"],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=8, check=False,
        )
        version = "macOS-" + result.stdout.decode("ascii", errors="ignore").strip()
        return version if result.returncode == 0 and version != "macOS-" else None, result.returncode
    args = ["--version"]
    if name == "zip":
        args = ["-v"]
    elif name in ("7z", "7zz"):
        args = ["i"]
    elif name == "rar":
        args = []
    result = subprocess.run(
        [str(binary), *args], stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=8, check=False,
    )
    output = result.stdout[:128 * 1024].decode("utf-8", errors="ignore")
    version_match = re.search(r"(?<!\d)(\d+(?:\.\d+){1,3}(?:[-+][0-9A-Za-z.-]+)?)", output)
    return (version_match.group(1) if version_match else None), result.returncode


def write_payload(path, case_token, index):
    path.parent.mkdir(parents=True, exist_ok=True)
    body = CONTENT_SENTINEL + b"\n" + case_token.encode("ascii") + bytes((48 + index,)) + b"\n"
    path.write_bytes(body + os.urandom(PAYLOAD_BYTES))
    path.chmod(0o600)


def make_pair(directory, tool, mode, token):
    paths = []
    for index in range(2):
        path = directory / f"synthetic {tool} {mode} {token} {index}.bin"
        write_payload(path, token, index)
        paths.append(path)
    return tuple(paths)


def tool_suffix(tool):
    return {
        "gzip": ".gz", "pigz": ".gz", "bzip2": ".bz2", "pbzip2": ".bz2",
        "xz": ".xz", "zstd": ".zst",
    }.get(tool, "")


def output_for(project, tool, mode, token, inputs):
    if tool in {"tar", "bsdtar", "gtar"}:
        extension = ".tar.gz" if mode != "update" else ".tar"
    elif tool in ("zip", "ditto"):
        extension = ".zip"
    elif tool in ("7z", "7zz"):
        extension = ".7z"
    elif tool == "rar":
        extension = ".rar"
    else:
        return Path(str(inputs[0]) + tool_suffix(tool))
    return project / ".archives" / f"{tool}-{mode}-{token}{extension}"


def expected_outputs(tool, inputs, single_output):
    suffix = tool_suffix(tool)
    if suffix:
        return tuple(Path(str(item) + suffix) for item in inputs)
    return (single_output,)


def descendants(pid):
    found = set()
    pending = [pid]
    while pending:
        parent = pending.pop()
        result = subprocess.run(
            ["/usr/bin/pgrep", "-P", str(parent)], stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, check=False,
        )
        if result.returncode in (0, 1):
            children = {int(item) for item in result.stdout.split() if item.isdigit()}
            new_children = children - found
            found.update(new_children)
            pending.extend(new_children)
    return found


class EsloggerSidecar:
    """短暂旁路观察本轮负例的 exec 身份，不保存原始事件。"""

    def __init__(self):
        self.process = None
        self.eslogger_pid = None
        self.eslogger_pgid = None
        self.stdout_reader = None
        self.stderr_reader = None
        self.lock = threading.Lock()
        self.active = None
        self.failure_codes = set()
        self.record_count = 0
        self.global_seq = None
        self.event_seq = None
        self.root_identity_verified = False
        self.stderr_es_denied = False
        self.stopping = False
        self.stopped = False
        self.cleanup_result = None

    @staticmethod
    def _ancestry_reaches(pid, launcher_pid):
        current = pid
        for _ in range(12):
            if current == launcher_pid:
                return True
            info = MVP.process_info(current)
            if not info or info[1] <= 1:
                return False
            current = info[1]
        return False

    def _verified_processes(self):
        if not self.process:
            return []
        pending = [self.process.pid]
        found = set()
        matches = []
        while pending:
            parent = pending.pop()
            if parent in found:
                continue
            found.add(parent)
            info = MVP.process_info(parent)
            if (info and info[0] == 0 and Path(info[3]) == Path("/usr/bin/eslogger")
                    and self._ancestry_reaches(parent, self.process.pid)):
                matches.append(parent)
            try:
                child_ids = MVP.children(parent)
            except (OSError, subprocess.SubprocessError):
                child_ids = []
            for child in child_ids:
                if child in found:
                    continue
                pending.append(child)
        return matches

    def start(self, timeout=8):
        try:
            self.process = subprocess.Popen(
                ["/usr/bin/sudo", "-n", "/usr/bin/eslogger", "exec"],
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                start_new_session=True,
            )
        except OSError as error:
            raise RuntimeError("sidecar_stream_failed") from error
        self.stdout_reader = threading.Thread(target=self._read_stdout, daemon=True)
        self.stderr_reader = threading.Thread(target=self._read_stderr, daemon=True)
        self.stdout_reader.start()
        self.stderr_reader.start()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                break
            matches = self._verified_processes()
            if len(matches) == 1:
                self.eslogger_pid = matches[0]
                self.eslogger_pgid = MVP.process_info(matches[0])[2]
                self.root_identity_verified = True
                return
            if len(matches) > 1:
                self.failure_codes.add("sidecar_root_unverified")
                raise RuntimeError("sidecar_root_unverified")
            time.sleep(0.05)
        if self.process.poll() is not None and self.stderr_reader:
            self.stderr_reader.join(timeout=0.2)
        if self.stderr_es_denied:
            self.failure_codes.add("sidecar_es_client_denied")
            raise RuntimeError("sidecar_es_client_denied")
        if not self.root_identity_verified:
            self.failure_codes.add("sidecar_root_unverified")
            raise RuntimeError("sidecar_root_unverified")

    def begin_case(self, tool):
        with self.lock:
            aliases = expected_tool_names(tool)
            self.active = {"aliases": aliases, "known_pids": set(), "candidates": [],
                           "overflow": False}

    def register_pids(self, pids):
        with self.lock:
            if self.active is not None:
                self.active["known_pids"].update(pids)

    def _record_exec(self, record):
        with self.lock:
            self.record_count += 1
            try:
                seq = record.get("seq_num")
                global_seq = record.get("global_seq_num")
            except AttributeError:
                self.failure_codes.add("sidecar_stream_malformed")
                return
            if not isinstance(seq, int) or seq < 0:
                self.failure_codes.add("sidecar_sequence_missing")
                return
            if not isinstance(global_seq, int) or global_seq < 0:
                self.failure_codes.add("sidecar_sequence_missing")
                return
            if self.event_seq is not None:
                if seq == self.event_seq:
                    self.failure_codes.add("sidecar_sequence_regression")
                elif seq != self.event_seq + 1:
                    self.failure_codes.add("sidecar_sequence_gap")
            if self.global_seq is not None and global_seq <= self.global_seq:
                self.failure_codes.add("sidecar_sequence_regression")
            self.event_seq = seq
            self.global_seq = global_seq
            if self.active is None:
                return
            try:
                event = record["event"]["exec"]
                target = event["target"]
                audit = target["audit_token"]
                target_pid = audit.get("pid")
                target_version = audit.get("pidversion")
                ppid = target.get("ppid")
                executable = target.get("executable") or {}
                path = executable.get("path")
                tool_name = Path(path).name.lower() if path and not executable.get("path_truncated") else None
                top_audit = (record.get("process") or {}).get("audit_token") or {}
                top_pid = top_audit.get("pid")
                top_version = top_audit.get("pidversion")
            except (KeyError, TypeError, AttributeError):
                self.failure_codes.add("sidecar_stream_malformed")
                return
            known = self.active["known_pids"]
            expected_name = tool_name in self.active["aliases"]
            belongs = target_pid in known or ppid in known
            if not expected_name and not belongs:
                return
            if len(self.active["candidates"]) >= 1024:
                self.active["overflow"] = True
                self.failure_codes.add("sidecar_exec_ambiguous")
                return
            self.active["candidates"].append({
                "target_pid": target_pid if isinstance(target_pid, int) else None,
                "target_pid_version": target_version if isinstance(target_version, int) else None,
                "ppid": ppid if isinstance(ppid, int) else None,
                "pre_exec_pid": top_pid if isinstance(top_pid, int) else None,
                "pre_exec_pid_version": top_version if isinstance(top_version, int) else None,
                "executable": tool_name,
            })

    def _read_stdout(self):
        try:
            while True:
                line = self.process.stdout.readline(1024 * 1024 + 1)
                if not line:
                    if not self.stopping:
                        self.failure_codes.add("sidecar_stream_ended")
                    return
                if len(line) > 1024 * 1024 or not line.endswith(b"\n"):
                    self.failure_codes.add("sidecar_stream_malformed")
                    continue
                try:
                    record = json.loads(line)
                except (ValueError, json.JSONDecodeError):
                    self.failure_codes.add("sidecar_stream_malformed")
                    continue
                if not isinstance(record, dict) or record.get("schema_version") != 1:
                    self.failure_codes.add("sidecar_stream_malformed")
                    continue
                if record.get("event_type") != 9:
                    self.failure_codes.add("sidecar_unexpected_event")
                    continue
                self._record_exec(record)
        except (OSError, ValueError):
            self.failure_codes.add("sidecar_stream_failed")

    def _read_stderr(self):
        try:
            while True:
                chunk = self.process.stderr.read(4096)
                if not chunk:
                    return
                text = chunk.decode("utf-8", errors="ignore").lower()
                if "not permitted" in text or "es_new_client_result_err_not_permitted" in text:
                    self.stderr_es_denied = True
        except OSError:
            self.failure_codes.add("sidecar_stream_failed")

    @staticmethod
    def _pid_chain(candidate, pids, candidates):
        chain = []
        target_pid = candidate.get("target_pid")
        if isinstance(target_pid, int):
            chain.append(target_pid)
        if target_pid in pids:
            return chain
        parents = {row.get("target_pid"): row.get("ppid") for row in candidates
                   if row.get("target_pid") is not None}
        current = candidate.get("ppid")
        seen = set()
        while current is not None and current not in seen:
            chain.append(current)
            if current in pids:
                return chain
            seen.add(current)
            current = parents.get(current)
        return chain

    @classmethod
    def _related(cls, candidate, pids, candidates):
        return bool(set(cls._pid_chain(candidate, pids, candidates)) & set(pids))

    def finish_case(self, pids, timeout=2.5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                active = self.active
                candidates = list(active["candidates"]) if active else []
                aliases = set(active["aliases"]) if active else set()
                pids = set(pids) | (set(active["known_pids"]) if active else set())
            matches = [row for row in candidates if row.get("executable") in aliases
                       and self._related(row, pids, candidates)]
            if matches or self.process.poll() is not None:
                break
            time.sleep(0.025)
        with self.lock:
            active = self.active
            candidates = list(active["candidates"]) if active else []
            aliases = set(active["aliases"]) if active else set()
            overflow = bool(active and active["overflow"])
            known = set(active["known_pids"]) if active else set()
            self.active = None
        pids = set(pids) | known
        related = [row for row in candidates if self._related(row, pids, candidates)]
        matches = [row for row in related if row.get("executable") in aliases]
        identity_missing = [row for row in related
                            if row.get("target_pid") is None or row.get("target_pid_version") is None
                            or row.get("executable") is None]
        failures = sorted(self.failure_codes)
        if overflow or len(matches) > 1:
            failure = "sidecar_exec_ambiguous"
        elif failures:
            failure = failures[0]
        elif identity_missing:
            failure = "sidecar_exec_identity_missing"
        elif len(matches) == 0:
            failure = None
        else:
            failure = None
        return {"execs": [dict(row, related_to_process=True,
                               related_pid_chain=self._pid_chain(row, pids, candidates))
                          for row in matches],
                "failure_code": failure,
                "record_count": self.record_count, "candidate_count": len(related)}

    def stop(self):
        if self.stopped:
            return self.cleanup_result
        if not self.process:
            self.stopped = True
            self.cleanup_result = True
            return self.cleanup_result
        cleanup_ok = True
        if not self.stopping and self.process.poll() is not None:
            self.failure_codes.add("sidecar_stream_ended")
            if self.process.poll() != 0:
                self.failure_codes.add("sidecar_exit_nonzero")
        self.stopping = True
        if self.process.poll() is None:
            matches = self._verified_processes()
            if not self.root_identity_verified:
                if len(matches) > 1:
                    cleanup_ok = False
                for pid in matches:
                    info = MVP.process_info(pid)
                    if (not info or info[0] != 0 or Path(info[3]) != Path("/usr/bin/eslogger")
                            or not self._ancestry_reaches(pid, self.process.pid)):
                        cleanup_ok = False
                        continue
                    if subprocess.run(
                        ["/usr/bin/sudo", "-n", "/bin/kill", "-TERM", str(pid)],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False,
                    ).returncode != 0:
                        cleanup_ok = False
                # sudo 已提升身份，普通用户不能直接 signal；只请求本轮 launcher 转发停止。
                cleanup_ok = MVP.stop_root(None, self.process) and cleanup_ok
            elif self.eslogger_pid not in matches or len(matches) != 1:
                cleanup_ok = False
                MVP.stop_root(None, self.process)
            else:
                info = MVP.process_info(self.eslogger_pid)
                if (not info or info[0] != 0 or info[2] != self.eslogger_pgid
                        or Path(info[3]) != Path("/usr/bin/eslogger")):
                    cleanup_ok = False
                elif subprocess.run(
                    ["/usr/bin/sudo", "-n", "/bin/kill", "-TERM", str(self.eslogger_pid)],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False,
                ).returncode != 0:
                    cleanup_ok = False
                else:
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline and MVP.process_info(self.eslogger_pid):
                        time.sleep(0.05)
                    if MVP.process_info(self.eslogger_pid):
                        info = MVP.process_info(self.eslogger_pid)
                        if (info and info[0] == 0 and info[2] == self.eslogger_pgid
                                and Path(info[3]) == Path("/usr/bin/eslogger")):
                            killed = subprocess.run(
                                ["/usr/bin/sudo", "-n", "/bin/kill", "-KILL", str(self.eslogger_pid)],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False,
                            ).returncode == 0
                            cleanup_ok = cleanup_ok and killed
                        else:
                            cleanup_ok = False
        try:
            self.process.wait(timeout=6)
        except subprocess.TimeoutExpired:
            cleanup_ok = False
        for reader in (self.stdout_reader, self.stderr_reader):
            if reader:
                reader.join(timeout=3)
                if reader.is_alive():
                    cleanup_ok = False
        if self.process.stdout:
            self.process.stdout.close()
        if self.process.stderr:
            self.process.stderr.close()
        if self.eslogger_pid and MVP.process_info(self.eslogger_pid):
            cleanup_ok = False
        self.stopped = True
        self.cleanup_result = cleanup_ok
        return self.cleanup_result

    def summary(self):
        return {"root_identity_verified": self.root_identity_verified,
                "record_count": self.record_count,
                "sequence_gap_or_regression": any(code in self.failure_codes for code in (
                    "sidecar_sequence_gap", "sidecar_sequence_regression")),
                "failure_codes": sorted(self.failure_codes)}


def execute(binary, argv, cwd, timeout=TOOL_TIMEOUT_SECONDS, capture_stdout=False, observer=None,
            stdin_payload=None):
    started = time.monotonic_ns()
    try:
        process = subprocess.Popen(
            [str(binary), *map(str, argv)], cwd=str(cwd),
            stdin=subprocess.PIPE if stdin_payload is not None else subprocess.DEVNULL,
            stdout=subprocess.PIPE if capture_stdout else subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, start_new_session=True,
        )
    except OSError:
        observation = observer.finish_case(set(), timeout=0) if observer else None
        return {"return_code": None, "pids": set(), "duration_ms": None, "stdout_bytes": 0,
                "failure_code": "tool_start_failed", "sidecar": observation}
    pids = {process.pid}
    if observer:
        observer.register_pids(pids)
    stdout_state = {"bytes": 0}
    stdout_reader = None
    if process.stdout:
        def drain_stdout():
            while True:
                chunk = process.stdout.read(64 * 1024)
                if not chunk:
                    return
                stdout_state["bytes"] += len(chunk)

        stdout_reader = threading.Thread(target=drain_stdout, daemon=True)
        stdout_reader.start()
    stdin_state = {"failed": False}
    stdin_writer = None
    if process.stdin:
        def provide_stdin():
            try:
                process.stdin.write(stdin_payload)
                process.stdin.close()
            except OSError:
                stdin_state["failed"] = True

        stdin_writer = threading.Thread(target=provide_stdin, daemon=True)
        stdin_writer.start()
    try:
        deadline = time.monotonic() + timeout
        while process.poll() is None and time.monotonic() < deadline:
            pids.update(descendants(process.pid))
            if observer:
                observer.register_pids(pids)
            time.sleep(0.002)
        failure = "case_timeout" if process.poll() is None else None
    finally:
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=2)
            except (OSError, subprocess.TimeoutExpired):
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except OSError:
                    pass
                process.wait()
        pids.update(descendants(process.pid))
        if observer:
            observer.register_pids(pids)
        for reader in (stdout_reader, stdin_writer):
            if reader:
                reader.join(timeout=3)
        for stream in (process.stdout, process.stdin):
            if stream:
                stream.close()
    if failure is None and stdin_state["failed"]:
        failure = "stdin_write_failed"
    result = {
        "return_code": process.returncode,
        "pids": pids,
        "duration_ms": round((time.monotonic_ns() - started) / 1_000_000),
        "stdout_bytes": stdout_state["bytes"],
        "failure_code": failure,
    }
    if observer:
        result["sidecar"] = observer.finish_case(pids)
        result["sidecar_execs"] = result["sidecar"]["execs"]
        if result["sidecar"].get("failure_code"):
            result["sidecar_failure_code"] = result["sidecar"]["failure_code"]
    return result


def seed_update_archives(binary, tool, project, seed_inputs, output, cwd):
    if tool in {"tar", "bsdtar", "gtar"}:
        argv = ["-cf", str(output), "-C", str(cwd), "--", *(item.name for item in seed_inputs)]
    elif tool == "zip":
        argv = ["-q", str(output), "--", *map(str, seed_inputs)]
    elif tool in ("7z", "7zz"):
        argv = ["a", "-p" + PASSWORD_SENTINEL, str(output), "--", *map(str, seed_inputs)]
    else:
        argv = ["a", "-idq", "-p" + PASSWORD_SENTINEL, str(output), "--", *map(str, seed_inputs)]
    result = execute(binary, argv, project, timeout=TOOL_TIMEOUT_SECONDS)
    return result["return_code"] == 0 and output.is_file() and output.stat().st_size > 0


def build_case(tool, mode, project, unrelated, source_sets, output_sets, fence_paths):
    src = project / "source files"
    if mode == "create":
        inputs = source_sets[(tool, "create")]
        output = output_sets[(tool, "create")]
        if tool in {"tar", "bsdtar", "gtar"}:
            argv = ["-czf", str(output), "-C", str(src), "--", *(item.name for item in inputs)]
        elif tool == "zip":
            argv = ["-q", "-r", str(output), "--", *map(str, inputs)]
        elif tool == "ditto":
            argv = ["-c", "-k", "--keepParent", str(inputs[0]), str(output)]
            inputs = (inputs[0],)
        elif tool in ("7z", "7zz"):
            argv = ["a", "-t7z", "-mx=1", str(output), "--", *map(str, inputs)]
        elif tool == "rar":
            argv = ["a", "-idq", "-p" + PASSWORD_SENTINEL, str(output), "--", *map(str, inputs)]
        else:
            argv = ["-k", "--", *map(str, inputs)]
        return Case(tool, mode, argv, project, project, inputs,
                    expected_outputs(tool, inputs, output), True)
    if mode == "update":
        inputs = source_sets[(tool, "update")]
        output = output_sets[(tool, "update")]
        if tool in {"tar", "bsdtar", "gtar"}:
            argv = ["-rf", str(output), "-C", str(src), "--", *(item.name for item in inputs)]
        elif tool == "zip":
            argv = ["-q", "-u", str(output), "--", *map(str, inputs)]
        elif tool in ("7z", "7zz"):
            argv = ["u", "-p" + PASSWORD_SENTINEL, str(output), "--", *map(str, inputs)]
        else:
            argv = ["u", "-idq", "-p" + PASSWORD_SENTINEL, str(output), "--", *map(str, inputs)]
        return Case(tool, mode, argv, project, project, inputs, (output,), True)
    if mode == "stdout":
        inputs = source_sets[(tool, "stdout")]
        if tool in {"tar", "bsdtar", "gtar"}:
            argv = ["-czf", "-", "-C", str(src), "--", *(item.name for item in inputs)]
        elif tool == "zip":
            argv = ["-q", "-", "--", *map(str, inputs)]
        elif tool == "ditto":
            argv = ["-c", "-k", str(inputs[0]), "-"]
            inputs = (inputs[0],)
        elif tool in ("7z", "7zz"):
            argv = ["a", "-so", "-ttar", "snapshot.tar", "--", *map(str, inputs)]
        else:
            argv = ["-c", "--", *map(str, inputs)]
        return Case(tool, mode, argv, project, project, inputs, (), True, stdout_expected=True)
    if mode == "stdin":
        output = output_sets[(tool, "stdin")]
        if tool in ("7z", "7zz", "rar"):
            argv = ["a", "-siSYNTHETIC_STREAM_MEMBER", str(output)]
            outputs, stdout_expected = (output,), False
        else:
            argv = [] if tool == "zip" else ["-c"]
            outputs, stdout_expected = (), True
        return Case(tool, mode, argv, project, project, (), outputs, False,
                    stdout_expected=stdout_expected, fence_path=fence_paths[(tool, mode)],
                    stdin_payload=CONTENT_SENTINEL + b"\n" + os.urandom(PAYLOAD_BYTES))
    if is_reverse(mode):
        archive = output_sets[(tool, "create")]
        destination = project / ".reverse" / f"{tool}-{mode}-{uuid.uuid4().hex[:8]}"
        if tool in {"tar", "bsdtar", "gtar"} and mode == "reverse_list":
            argv, stdout_expected = ["-tf", str(archive)], True
        elif tool in {"tar", "bsdtar", "gtar"} and mode == "reverse_extract":
            destination.mkdir(parents=True, exist_ok=False)
            argv, stdout_expected = ["-xzf", str(archive), "-C", str(destination)], False
        elif tool == "zip" and mode == "reverse_list":
            argv, stdout_expected = ["-sf", str(archive)], True
        elif tool == "zip" and mode == "reverse_test":
            argv, stdout_expected = ["-T", str(archive)], True
        elif tool == "ditto" and mode == "reverse_extract":
            destination.mkdir(parents=True, exist_ok=False)
            argv, stdout_expected = ["-x", "-k", str(archive), str(destination)], False
        elif tool in {"gzip", "pigz", "bzip2", "pbzip2", "xz", "zstd"}:
            if mode == "reverse_list":
                argv, stdout_expected = ["-l", "--", str(archive)], True
            elif mode == "reverse_test":
                argv, stdout_expected = ["-t", "--", str(archive)], False
            else:
                argv, stdout_expected = ["-d", "-c", "--", str(archive)], True
        elif tool in ("7z", "7zz") and mode == "reverse_list":
            argv, stdout_expected = ["l", str(archive)], True
        elif tool in ("7z", "7zz") and mode == "reverse_test":
            argv, stdout_expected = ["t", str(archive)], True
        elif tool in ("7z", "7zz") and mode == "reverse_extract":
            destination.mkdir(parents=True, exist_ok=False)
            argv, stdout_expected = ["x", str(archive), "-o" + str(destination), "-y"], False
        elif tool == "rar" and mode == "reverse_list":
            argv, stdout_expected = ["l", "-p" + PASSWORD_SENTINEL, str(archive)], True
        elif tool == "rar" and mode == "reverse_test":
            argv, stdout_expected = ["t", "-p" + PASSWORD_SENTINEL, str(archive)], True
        elif tool == "rar" and mode == "reverse_extract":
            destination.mkdir(parents=True, exist_ok=False)
            argv, stdout_expected = ["x", "-idq", "-p" + PASSWORD_SENTINEL,
                                     str(archive), str(destination) + "/"], False
        else:
            raise ValueError("未配置归档反向验收场景")
        return Case(tool, mode, argv, project, project, (archive,), (), False,
                    stdout_expected=stdout_expected, fence_path=fence_paths[(tool, mode)],
                    operation_artifact=destination if mode.endswith("extract") else None)
    if mode == "unrelated":
        inputs = source_sets[(tool, "unrelated")]
        output = output_sets[(tool, "unrelated")]
        if tool in {"tar", "bsdtar", "gtar"}:
            argv = ["-czf", str(output), "-C", str(unrelated / "source files"), "--",
                    *(item.name for item in inputs)]
        elif tool == "zip":
            argv = ["-q", "-r", str(output), "--", *map(str, inputs)]
        elif tool == "ditto":
            argv = ["-c", "-k", "--keepParent", str(inputs[0]), str(output)]
            inputs = (inputs[0],)
        elif tool in ("7z", "7zz"):
            argv = ["a", "-t7z", "-mx=1", str(output), "--", *map(str, inputs)]
        elif tool == "rar":
            argv = ["a", "-idq", "-p" + PASSWORD_SENTINEL, str(output), "--", *map(str, inputs)]
        else:
            argv = ["-k", "--", *map(str, inputs)]
        return Case(tool, mode, argv, unrelated, project, inputs,
                    expected_outputs(tool, inputs, output), False,
                    fence_path=fence_paths[(tool, mode)])
    raise ValueError("未知验收场景")


def prepare_workspace(report, tools):
    workspace = report / "workspace"
    project = workspace / "synthetic project"
    unrelated = workspace / "unrelated project"
    source_dir = project / "source files"
    unrelated_source = unrelated / "source files"
    archive_dir = project / ".archives"
    outside_archive_dir = unrelated / ".archives"
    seed_dir = workspace / "preseed"
    for directory in (source_dir, archive_dir, unrelated_source, outside_archive_dir, seed_dir):
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        directory.chmod(0o700)
    token = uuid.uuid4().hex[:12]
    source_sets, output_sets, fence_paths = {}, {}, {}
    seed_sets = {}
    for tool in TOOLS:
        for mode in ("create", "stdout", "unrelated"):
            base = unrelated_source if mode == "unrelated" else source_dir
            source_sets[(tool, mode)] = make_pair(base, tool, mode, token)
        if tool in UPDATE_TOOLS:
            source_sets[(tool, "update")] = make_pair(source_dir, tool, "update", token)
            seed_sets[tool] = make_pair(seed_dir, tool, "seed", token)
        output_sets[(tool, "create")] = output_for(project, tool, "create", token, source_sets[(tool, "create")])
        output_sets[(tool, "unrelated")] = output_for(unrelated, tool, "unrelated", token,
                                                       source_sets[(tool, "unrelated")])
        if tool in STDIN_TOOLS:
            output_sets[(tool, "stdin")] = output_for(unrelated, tool, "stdin", token,
                                                     source_sets[(tool, "unrelated")])
        if tool in UPDATE_TOOLS:
            update_output = output_for(project, tool, "update", token, source_sets[(tool, "update")])
            output_sets[(tool, "update")] = update_output
        for mode in (*REVERSE_MODES[tool], "unrelated", *(("stdin",) if tool in STDIN_TOOLS else ())):
            path = project / ".fences" / f"fence-{tool}-{mode}-{token}.bin"
            path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            write_payload(path, token, 0)
            fence_paths[(tool, mode)] = path
    for tool, binary in tools.items():
        if tool not in UPDATE_TOOLS:
            continue
        succeeded = seed_update_archives(binary, tool, project, seed_sets[tool],
                                         output_sets[(tool, "update")], seed_dir)
        output_sets[(tool, "update-seed-failed")] = not succeeded
    return workspace, project, unrelated, source_sets, output_sets, fence_paths


def all_cases(project, unrelated, source_sets, output_sets, fence_paths, tools):
    cases = []
    for tool in TOOLS:
        if tool not in tools:
            continue
        cases.append(build_case(tool, "create", project, unrelated, source_sets, output_sets, fence_paths))
        if tool in UPDATE_TOOLS:
            update_case = build_case(tool, "update", project, unrelated, source_sets, output_sets, fence_paths)
            if output_sets.get((tool, "update-seed-failed")):
                update_case.preparation_failure = "update_seed_failed"
            cases.append(update_case)
        if tool in STDOUT_TOOLS:
            cases.append(build_case(tool, "stdout", project, unrelated, source_sets, output_sets, fence_paths))
        if tool in STDIN_TOOLS:
            cases.append(build_case(tool, "stdin", project, unrelated, source_sets, output_sets, fence_paths))
        for mode in REVERSE_MODES[tool]:
            cases.append(build_case(tool, mode, project, unrelated, source_sets, output_sets, fence_paths))
        cases.append(build_case(tool, "unrelated", project, unrelated, source_sets, output_sets, fence_paths))
    return cases


def archive_outputs(archive):
    values = []
    if archive.get("output_path"):
        values.append(archive["output_path"])
    values.extend(archive.get("output_paths") or [])
    return {MVP.canonical_path(value) for value in values if value}


def event_process_key(event):
    identity = event.get("process") or {}
    if not event.get("source_run_id") or identity.get("pid") is None or identity.get("pid_version") is None:
        return None
    return event["source_run_id"], identity["pid"], identity["pid_version"]


def alert_process_key(alert, source_run_id):
    identity = alert.get("process") or {}
    if identity.get("pid") is None or identity.get("pid_version") is None:
        return None
    return source_run_id, identity["pid"], identity["pid_version"]


def executable_name(event):
    value = (event.get("process") or {}).get("executable")
    return Path(value).name.lower() if value else None


def exact_inputs(archive, expected):
    actual = {MVP.canonical_path(path) for path in archive.get("input_paths") or []}
    return actual == {MVP.canonical_path(path) for path in expected}


def matching_archive_exec(case, evidence, source_run_id, pids):
    matched = []
    actual_tool_names = set()
    expected_names = expected_tool_names(case.tool)
    for event in evidence["events"]:
        if event.get("kind") != "exec" or event_process_key(event) is None:
            continue
        key = event_process_key(event)
        if key[0] != source_run_id or key[1] not in pids:
            continue
        event_tool = executable_name(event)
        if event_tool in expected_names:
            actual_tool_names.add(event_tool)
        archive = event.get("archive")
        if not archive or archive.get("tool", "").lower() not in expected_names:
            continue
        if event_tool not in expected_names or archive.get("tool", "").lower() != event_tool:
            continue
        if exact_inputs(archive, case.inputs):
            matched.append(event)
    return matched, actual_tool_names


def matching_alerts(case, evidence, source_run_id, process_key):
    return [alert for alert in evidence["alerts"]
            if alert.get("rule") == "archive_command"
            and alert_process_key(alert, source_run_id) == process_key]


def analyze_positive(case, execution, evidence, source_run_id):
    result = {"passed": False, "failure_code": None, "evidence_event_count": 0,
              "archive_exec_count": 0, "archive_command_alert_count": 0,
              "notification_feedback_count": 0, "generation_latency_ms": None,
              "notification_latency_ms": None, "actual_executable": None}
    matched, names = matching_archive_exec(case, evidence, source_run_id, execution["pids"])
    result["actual_executable"] = sorted(names)[0] if names else None
    result["archive_exec_count"] = len(matched)
    if not matched:
        result["failure_code"] = "exec_missing" if not names else "archive_metadata_mismatch"
        return result
    if len(matched) != 1:
        result["failure_code"] = "archive_metadata_mismatch"
        return result
    event = matched[0]
    result["actual_executable"] = executable_name(event)
    result["evidence_event_count"] += 1
    archive = event["archive"]
    outputs = archive_outputs(archive)
    expected_outputs = {MVP.canonical_path(path) for path in case.outputs}
    if outputs != expected_outputs:
        result["failure_code"] = "archive_metadata_mismatch"
        return result
    key = event_process_key(event)
    if key is None:
        result["failure_code"] = "exec_identity_mismatch"
        return result
    alerts = matching_alerts(case, evidence, source_run_id, key)
    in_scope_alerts = [alert for alert in alerts
                       if MVP.canonical_path(case.project_root) in {
                           MVP.canonical_path(root) for root in alert.get("roots", [])
                       } and alert.get("first_timestamp_ms", 0)
                       <= event.get("source_timestamp_ms", -1)
                       <= alert.get("last_timestamp_ms", -1)]
    result["archive_command_alert_count"] = len(in_scope_alerts)
    if len(in_scope_alerts) != 1:
        result["failure_code"] = (
            "archive_command_alert_wrong_scope" if alerts else "archive_command_alert_missing"
        )
        return result
    alert = in_scope_alerts[0]
    outbox = evidence["outbox"].get(alert["id"])
    if outbox is None:
        result["failure_code"] = "notification_outbox_missing"
        return result
    sent = [item["observed_timestamp_ms"] for item in evidence["notifications"]
            if item.get("alert_id") == alert["id"] and item.get("outcome") == "sent"]
    result["notification_feedback_count"] = len(sent)
    if not sent:
        result["failure_code"] = "notification_feedback_missing"
        return result
    trigger = event.get("source_timestamp_ms")
    if trigger is None:
        result["failure_code"] = "archive_metadata_mismatch"
        return result
    generated = outbox
    notified = min(sent)
    generation_latency = generated - trigger
    notification_latency = notified - trigger
    result["generation_latency_ms"] = generation_latency
    result["notification_latency_ms"] = notification_latency
    if generation_latency < 0:
        result["failure_code"] = "generation_latency_negative"
    elif notification_latency < 0:
        result["failure_code"] = "notification_latency_negative"
    elif generation_latency > 3000:
        result["failure_code"] = "generation_latency_over_3000ms"
    elif notification_latency > 3000:
        result["failure_code"] = "notification_latency_over_3000ms"
    else:
        result["passed"] = True
    return result


def expected_tool_names(tool):
    if tool in ("tar", "bsdtar"):
        return {"tar", "bsdtar"}
    if tool in ("7z", "7zz"):
        return {"7z", "7zz"}
    return {tool}


def matching_sidecar_alerts(case, evidence, source_run_id, sidecar_exec):
    keys = set()
    for pid_field, version_field in (
        ("target_pid", "target_pid_version"),
        ("pre_exec_pid", "pre_exec_pid_version"),
    ):
        pid = sidecar_exec.get(pid_field)
        version = sidecar_exec.get(version_field)
        if isinstance(pid, int) and isinstance(version, int):
            keys.add((source_run_id, pid, version))
    project_root = MVP.canonical_path(case.project_root)
    return [alert for alert in evidence["alerts"]
            if alert_process_key(alert, source_run_id) in keys
            and project_root in {MVP.canonical_path(root) for root in alert.get("roots", [])}]


def analyze_negative(case, execution, evidence, source_run_id, barrier, healthy):
    result = {"passed": False, "failure_code": None, "evidence_event_count": 0,
              "archive_exec_count": 0, "archive_command_alert_count": 0,
              "notification_feedback_count": 0, "generation_latency_ms": None,
              "notification_latency_ms": None, "actual_executable": None,
              "sidecar_exec_count": 0,
              "barrier_crossed": False}
    if not execution.get("operation_verified"):
        result["failure_code"] = (
            execution.get("failure_code") or
            ("tool_exit_nonzero" if execution.get("return_code") != 0 else "output_missing")
        )
        return result
    process_events = [event for event in evidence["events"]
                      if event_process_key(event) is not None
                      and event_process_key(event)[0] == source_run_id
                      and event_process_key(event)[1] in execution["pids"]]
    result["evidence_event_count"] = len(process_events)
    sidecar = execution.get("sidecar") or {}
    if sidecar.get("failure_code"):
        result["failure_code"] = sidecar["failure_code"]
        return result
    execs = execution.get("sidecar_execs") or sidecar.get("execs") or []
    result["sidecar_exec_count"] = len(execs)
    if not execs:
        result["failure_code"] = "reverse_exec_missing" if is_reverse(case.mode) else "unrelated_exec_missing"
        return result
    if len(execs) != 1:
        result["failure_code"] = "sidecar_exec_ambiguous"
        return result
    sidecar_exec = execs[0]
    target_pid = sidecar_exec.get("target_pid")
    target_version = sidecar_exec.get("target_pid_version")
    actual_tool = sidecar_exec.get("executable")
    if (not isinstance(target_pid, int) or not isinstance(target_version, int)
            or not isinstance(actual_tool, str)):
        result["failure_code"] = "sidecar_exec_identity_missing"
        return result
    related_pids = set(sidecar_exec.get("related_pid_chain") or [])
    if (target_pid not in execution["pids"]
            and not related_pids.intersection(execution["pids"])):
        result["failure_code"] = "sidecar_exec_identity_missing"
        return result
    if actual_tool not in expected_tool_names(case.tool):
        result["failure_code"] = "sidecar_exec_identity_missing"
        return result
    result["actual_executable"] = actual_tool
    result["archive_exec_count"] = 1
    alerts = matching_sidecar_alerts(case, evidence, source_run_id, sidecar_exec)
    if is_reverse(case.mode):
        command_alerts = [alert for alert in alerts if alert.get("rule") == "archive_command"]
        result["archive_command_alert_count"] = len(command_alerts)
        if command_alerts:
            result["failure_code"] = "reverse_archive_command_alert"
            return result
    else:
        project_alerts = [alert for alert in alerts
                          if alert.get("rule") in ("archive_command", "archive_output")]
        result["archive_command_alert_count"] = len(project_alerts)
        if project_alerts:
            result["failure_code"] = "unrelated_project_alert"
            return result
    if barrier is None:
        result["failure_code"] = "negative_fence_missing"
        return result
    fence_key = (source_run_id, barrier.get("pid"), barrier.get("pid_version"))
    target_keys = {event_process_key(event) for event in process_events}
    sidecar_pids = {row.get("target_pid") for row in execs}
    if barrier.get("pid") in execution["pids"] or barrier.get("pid") in sidecar_pids or fence_key in target_keys:
        result["failure_code"] = "negative_fence_not_independent"
        return result
    if not barrier.get("crossed"):
        result["failure_code"] = "negative_fence_missing"
        return result
    if case.mode == "stdin":
        fence_seq = barrier.get("global_seq")
        gaps = [row for row in evidence["health"]
                if row.get("component") == "eslogger"
                and row.get("code") == "archive_input_source_unknown"
                and (row.get("source") or {}).get("run_id") == source_run_id
                and (row.get("source") or {}).get("pid") == target_pid
                and (row.get("source") or {}).get("pid_version") == target_version
                and isinstance((row.get("source") or {}).get("global_seq"), int)
                and isinstance(fence_seq, int)
                and 0 <= row["source"]["global_seq"] < fence_seq]
        if not gaps:
            result["failure_code"] = "stdin_source_gap_missing"
            return result
        result["source_unknown_gap_count"] = len(gaps)
    result["barrier_crossed"] = True
    if not healthy:
        result["failure_code"] = "negative_fence_unhealthy"
        return result
    result["passed"] = True
    return result


def healthy_status(status, evidence):
    if status.get("database_state") != "ready" or status.get("collector_state") != "connected":
        return False, "collector_unhealthy", {}
    if any(status.get(field, 0) for field in ("collector_dropped_lines", "reader_dropped_frames")):
        return False, "collector_dropped_events", {}
    if status.get("database_gap_events", 0):
        return False, "database_gap", {}
    issue_rows = [row for row in evidence["health"] if row.get("state") in (
        "degraded", "gap", "failed", "error", "permission_denied", "coverage_gap",
    )]
    static_coverage = {}
    unexpected = []
    for row in issue_rows:
        code = row.get("code")
        if row.get("component") == "eslogger" and code in ARCHIVE_STATIC_COVERAGE_CODES:
            static_coverage[code] = static_coverage.get(code, 0) + 1
        else:
            unexpected.append(row)
    if unexpected:
        return False, "unexpected_coverage_gap", static_coverage
    return True, None, static_coverage


def fence_worker(path):
    try:
        with Path(path).open("rb") as stream:
            stream.read(1)
        print(json.dumps({"pid": os.getpid(), "success": True}))
        return 0
    except OSError:
        print(json.dumps({"pid": os.getpid(), "success": False}))
        return 2


def run_fence(script, path, database, source_run_id, timeout=8):
    try:
        process = subprocess.run(
            [sys.executable, "-B", str(script), "--fence-worker", str(path)],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=10, check=False, start_new_session=True,
        )
        metadata = json.loads(process.stdout)
    except (OSError, subprocess.SubprocessError, ValueError, json.JSONDecodeError):
        return {"pid": None, "pid_version": None, "crossed": False}
    if process.returncode or not metadata.get("success"):
        return {"pid": metadata.get("pid"), "pid_version": None, "crossed": False}
    expected = MVP.canonical_path(path)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        evidence = MVP.load_evidence(database)
        for event in evidence["events"]:
            file = event.get("file") or {}
            identity = event.get("process") or {}
            if (event.get("source_run_id") == source_run_id
                    and identity.get("pid") == metadata["pid"]
                    and identity.get("pid_version") is not None
                    and event.get("kind") in ("open", "mmap")
                    and file.get("readable") is True and not file.get("path_truncated")
                    and MVP.canonical_path(file.get("path")) == expected
                    and event.get("global_seq") is not None):
                return {"pid": metadata["pid"], "pid_version": identity["pid_version"],
                        "global_seq": event["global_seq"], "crossed": True}
        time.sleep(0.05)
    return {"pid": metadata.get("pid"), "pid_version": None, "crossed": False}


def process_case(case, binary, database, source_run_id, script, sidecar=None):
    negative = is_negative(case.mode)
    if case.preparation_failure:
        execution = {"return_code": None, "pids": set(), "duration_ms": None,
                     "stdout_bytes": 0, "failure_code": case.preparation_failure}
        if negative:
            return execution, run_fence(script, case.fence_path, database, source_run_id)
        return execution, None
    if case.mode == "update" and not case.outputs[0].is_file():
        return {"return_code": None, "pids": set(), "duration_ms": None, "stdout_bytes": 0,
                "failure_code": "update_seed_failed"}, None
    observer = sidecar if negative else None
    if observer:
        observer.begin_case(case.tool)
    execution = execute(binary, case.argv, case.cwd, capture_stdout=case.stdout_expected,
                        observer=observer, stdin_payload=case.stdin_payload)
    if execution["failure_code"]:
        barrier = run_fence(script, case.fence_path, database, source_run_id) if negative else None
        return execution, barrier
    validate_operation(case, execution)
    if negative:
        barrier = run_fence(script, case.fence_path, database, source_run_id)
        return execution, barrier
    return execution, None


def validate_operation(case, execution):
    if execution.get("failure_code"):
        execution["operation_verified"] = False
        return
    if execution["return_code"] != 0:
        execution["failure_code"] = "tool_exit_nonzero"
    elif case.stdout_expected and execution["stdout_bytes"] == 0:
        execution["failure_code"] = "stdout_empty"
    elif case.outputs and any(
        not output.is_file() or output.stat().st_size == 0 for output in case.outputs
    ):
        execution["failure_code"] = "output_missing"
    elif case.operation_artifact and not any(
        item.is_file() and item.stat().st_size > 0
        for item in case.operation_artifact.rglob("*")
    ):
        execution["failure_code"] = "reverse_extract_output_missing"
    execution["operation_verified"] = execution.get("failure_code") is None


def case_summary(case, execution, result, version):
    failure = execution.get("failure_code") or result.get("failure_code")
    return {
        "mode": case.mode,
        "version": version,
        "return_code": execution.get("return_code"),
        "duration_ms": execution.get("duration_ms"),
        "stdout_bytes": execution.get("stdout_bytes", 0) if case.stdout_expected else None,
        "evidence_event_count": result.get("evidence_event_count", 0),
        "archive_exec_count": result.get("archive_exec_count", 0),
        "sidecar_exec_count": result.get("sidecar_exec_count", 0),
        "source_unknown_gap_count": result.get("source_unknown_gap_count", 0),
        "archive_command_alert_count": result.get("archive_command_alert_count", 0),
        "notification_feedback_count": result.get("notification_feedback_count", 0),
        "generation_latency_ms": result.get("generation_latency_ms"),
        "notification_latency_ms": result.get("notification_latency_ms"),
        "actual_executable": result.get("actual_executable"),
        "barrier_crossed": result.get("barrier_crossed", False),
        "passed": bool(result.get("passed")) and failure is None,
        "failure_code": failure,
    }


def save_summary(path, summary):
    path.write_text(json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    path.chmod(0o600)


def failure_code_for(error):
    message = str(error)
    if message in FAILURE_CODES:
        return message
    code = getattr(error, "code", None)
    if code in FAILURE_CODES:
        return code
    if isinstance(error, TimeoutError):
        return "startup_timeout"
    if "root-owned" in message or "受信任" in message or "可信" in message:
        return "collector_untrusted"
    if "sudo" in message:
        return "sudo_authorization_required"
    if "collector 与" in message or "二进制不同" in message:
        return "collector_mismatch"
    if "已存在" in message and "socket" in message:
        return "collector_endpoint_in_use"
    if "macOS" in message or "darwin" in message:
        return "unsupported_platform"
    if "普通用户 CLI" in message or "cargo build" in message:
        return "binary_missing"
    if isinstance(error, MVP.HostStartupError):
        return "host_startup_failed"
    if isinstance(error, MVP.CollectorStartupError):
        return "collector_startup_failed"
    if isinstance(error, PermissionError):
        return "permission_denied"
    if isinstance(error, FileNotFoundError):
        return "missing_install_path"
    return "tool_start_failed"


def summarize_tools(tools, version_rows, cases):
    return [{
        "tool": name,
        "version": version_rows[name]["version"],
        "version_return_code": version_rows[name]["version_return_code"],
        "available": name in tools,
        "cases": [row for row in cases if row["tool"] == name],
        "failure_code": (
            "tool_missing" if name not in tools else
            "version_probe_failed" if version_rows[name]["version_return_code"] not in (0, None) else
            "version_unavailable" if version_rows[name]["version"] is None else None
        ),
    } for name in TOOLS]


def exercise_only(tools, report, version_rows):
    _, project, unrelated, sources, outputs, fences = prepare_workspace(report, tools)
    cases = all_cases(project, unrelated, sources, outputs, fences, tools)
    summaries = []
    for case in cases:
        binary = tools[case.tool]
        if case.preparation_failure:
            execution = {"return_code": None, "pids": set(), "duration_ms": None,
                         "stdout_bytes": 0, "failure_code": case.preparation_failure}
        else:
            execution = execute(binary, case.argv, case.cwd, capture_stdout=case.stdout_expected,
                                stdin_payload=case.stdin_payload)
        validate_operation(case, execution)
        summaries.append({
            "tool": case.tool,
            "version": version_rows[case.tool]["version"],
            "mode": case.mode,
            "return_code": execution["return_code"],
            "evidence_event_count": 0,
            "duration_ms": execution["duration_ms"],
            "stdout_bytes": execution["stdout_bytes"] if case.stdout_expected else None,
            "operation_passed": execution["failure_code"] is None,
            "evidence_validation": "not_run",
            "failure_code": execution["failure_code"],
        })
        print(json.dumps({
            "case": len(summaries), "case_count": len(cases), "tool": case.tool,
            "mode": case.mode,
            "status": "operation_ok" if execution["failure_code"] is None else "failed",
            "failure_code": execution["failure_code"],
        }, ensure_ascii=False), flush=True)
    failures = any(row["failure_code"] for row in summaries)
    return {"mode": "exercise_only", "result": "exercise_partial" if failures else
            "exercise_completed_not_real_validation", "cases": summaries,
            "collector_evidence_count": 0}


def real_validation(args, report, tools, version_rows):
    summary = {"mode": "real_eslogger", "result": "not_started",
               "tools": summarize_tools(tools, version_rows, []), "cases": [],
               "failure_codes": [], "collector_health": None}
    save_summary(report / "summary.json", summary)
    binary = args.binary.resolve()
    collector = args.collector_binary or Path(f"/Library/CodePerimeter/{os.getuid()}/codeperimeter")
    collector_socket = Path(f"/Library/CodePerimeter/{os.getuid()}/run/collector.sock")
    preflight = MVP.preflight(binary, collector, collector_socket)
    summary["platform"] = {
        key: preflight[key]
        for key in ("os", "python", "binary_sha256", "binary_version")
        if key in preflight
    }
    summary["collector_preflight"] = "passed"
    missing = set(TOOLS) - set(tools)
    workspace, project, unrelated, sources, outputs, fences = prepare_workspace(report, tools)
    cases = all_cases(project, unrelated, sources, outputs, fences, tools)
    host_directory = report / "host"
    host_directory.mkdir(mode=0o700)
    database = host_directory / "events.sqlite"
    socket_parent = Path(tempfile.mkdtemp(prefix="cpa-socket-", dir="/private/tmp"))
    control_socket = socket_parent / "host.sock"
    daemon = notifier = root_process = None
    startup_reader = host_reader = None
    startup_codes, host_codes = [], []
    bridge = None
    sidecar = EsloggerSidecar()
    case_results = []
    try:
        daemon = subprocess.Popen(
            [str(binary), "daemon", "--socket", str(collector_socket), "--control-socket",
             str(control_socket), "--db", str(database)], stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE, start_new_session=True,
        )
        host_reader = threading.Thread(target=MVP.startup_diagnostics,
                                       args=(daemon.stderr, host_codes), daemon=True)
        host_reader.start()
        MVP.wait_for_host(control_socket, daemon)
        MVP.control(control_socket, "add_directories", {"entries": [{"path": str(project), "sources": ["manual"]}]})
        configured = MVP.control(control_socket, "list_directories")
        if len(configured) != 1 or Path(configured[0]["path"]) != project.resolve():
            raise RuntimeError("collector_unhealthy")
        root_process = subprocess.Popen(
            ["/usr/bin/sudo", "-n", str(collector), "collector", "--socket",
             str(collector_socket), "--allowed-uid", str(os.getuid())],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
        startup_reader = threading.Thread(target=MVP.startup_diagnostics,
                                          args=(root_process.stderr, startup_codes), daemon=True)
        startup_reader.start()
        notifier = subprocess.Popen(
            [str(binary), "notify", "--control-socket", str(control_socket)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
        )
        bridge = MVP.wait_for_bridge(control_socket, root_process, collector, {})
        source_status = MVP.control(control_socket, "status")
        summary["collector_protocol"] = {
            "schema_version": source_status.get("collector_schema_version"),
            "message_version": source_status.get("collector_message_version"),
        }
        source_run_id = source_status.get("collector_run_id")
        if not source_run_id:
            raise RuntimeError("collector_unhealthy")
        sidecar.start()
        summary["sidecar_started"] = sidecar.root_identity_verified
        for case_number, case in enumerate(cases, 1):
            tool = case.tool
            if version_rows[tool]["version"] is None or version_rows[tool]["version_return_code"] not in (0, None):
                version_failure = ("version_probe_failed" if version_rows[tool]["version_return_code"] not in
                                   (0, None) else "version_unavailable")
                case_results.append({
                    "tool": tool, "mode": case.mode, "version": version_rows[tool]["version"], "return_code": None,
                    "duration_ms": None, "evidence_event_count": 0, "archive_exec_count": 0,
                    "sidecar_exec_count": 0,
                    "archive_command_alert_count": 0, "notification_feedback_count": 0,
                    "generation_latency_ms": None, "notification_latency_ms": None,
                    "actual_executable": None, "barrier_crossed": False, "passed": False,
                    "failure_code": version_failure,
                })
                print(json.dumps({
                    "case": case_number, "case_count": len(cases), "tool": tool,
                    "mode": case.mode, "status": "failed", "failure_code": version_failure,
                }, ensure_ascii=False), flush=True)
                continue
            execution, barrier = process_case(case, tools[tool], database, source_run_id,
                                              Path(__file__).resolve(), sidecar=sidecar)
            deadline = time.monotonic() + REAL_CASE_TIMEOUT_SECONDS
            result = {"passed": False, "failure_code": None, "evidence_event_count": 0,
                      "archive_exec_count": 0, "archive_command_alert_count": 0,
                      "sidecar_exec_count": 0,
                      "notification_feedback_count": 0, "generation_latency_ms": None,
                      "notification_latency_ms": None, "actual_executable": None,
                      "barrier_crossed": False}
            if case.positive:
                while time.monotonic() < deadline:
                    evidence = MVP.load_evidence(database)
                    result = analyze_positive(case, execution, evidence, source_run_id)
                    if result["passed"] or result["failure_code"] not in (
                        "exec_missing", "archive_metadata_mismatch", "archive_command_alert_missing",
                        "notification_outbox_missing", "notification_feedback_missing",
                    ):
                        break
                    time.sleep(0.05)
            else:
                evidence = MVP.load_evidence(database)
                status = MVP.control(control_socket, "status")
                healthy, _, _ = healthy_status(status, evidence)
                result = analyze_negative(case, execution, evidence, source_run_id, barrier, healthy)
            if execution.get("failure_code"):
                result.update(passed=False, failure_code=execution["failure_code"])
            elif execution.get("return_code") != 0:
                result.update(passed=False, failure_code="tool_exit_nonzero")
            case_result = case_summary(case, execution, result, version_rows[tool]["version"])
            case_result["tool"] = tool
            case_results.append(case_result)
            summary["cases"] = case_results
            summary["tools"] = summarize_tools(tools, version_rows, case_results)
            save_summary(report / "summary.json", summary)
            print(json.dumps({
                "case": case_number, "case_count": len(cases), "tool": tool,
                "mode": case.mode,
                "status": "verified" if case_result["passed"] else "failed",
                "failure_code": case_result["failure_code"],
            }, ensure_ascii=False), flush=True)
        sidecar_cleanup_ok = sidecar.stop()
        summary["sidecar"] = sidecar.summary()
        summary["sidecar_cleanup_complete"] = sidecar_cleanup_ok
        if not sidecar_cleanup_ok:
            summary["failure_codes"].append("sidecar_cleanup_incomplete")
        summary["failure_codes"].extend(sidecar.failure_codes)
        final_evidence = MVP.load_evidence(database)
        final_status = MVP.control(control_socket, "status")
        healthy, health_code, static_coverage = healthy_status(final_status, final_evidence)
        serialized_evidence = json.dumps(final_evidence, ensure_ascii=False, sort_keys=True)
        privacy_ok = CONTENT_SENTINEL.decode("ascii") not in serialized_evidence and PASSWORD_SENTINEL not in serialized_evidence
        summary["collector_health"] = {
            "database_state": final_status.get("database_state"),
            "collector_state": final_status.get("collector_state"),
            "collector_dropped_lines": final_status.get("collector_dropped_lines"),
            "reader_dropped_frames": final_status.get("reader_dropped_frames"),
            "database_gap_events": final_status.get("database_gap_events"),
            "event_count": len(final_evidence["events"]),
            "alert_count": len(final_evidence["alerts"]),
            "notification_feedback_count": len(final_evidence["notifications"]),
            "privacy_markers_absent": privacy_ok,
            "healthy": healthy and privacy_ok,
            "failure_code": None if healthy and privacy_ok else (health_code or "privacy_marker_persisted"),
            "static_coverage_issues": static_coverage,
        }
        summary["tools"] = summarize_tools(tools, version_rows, case_results)
        summary["cases"] = case_results
        if not healthy:
            summary["failure_codes"].append(health_code or "collector_unhealthy")
        if not privacy_ok:
            summary["failure_codes"].append("privacy_marker_persisted")
        summary["result"] = "real_run_passed" if (
            not missing and healthy and privacy_ok and case_results
            and sidecar_cleanup_ok and not sidecar.failure_codes
            and all(row.get("passed") for row in case_results)
        ) else "real_run_failed_or_partial"
        return summary
    except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError, sqlite3.Error) as error:
        summary["failure_codes"].append(failure_code_for(error))
        summary["result"] = "real_run_failed_or_partial"
        summary["cases"] = case_results
        summary["tools"] = summarize_tools(tools, version_rows, case_results)
        return summary
    finally:
        try:
            sidecar_cleanup_ok = sidecar.stop()
        except (OSError, subprocess.SubprocessError):
            sidecar_cleanup_ok = False
        summary["sidecar"] = sidecar.summary()
        summary["sidecar_cleanup_complete"] = sidecar_cleanup_ok
        if not sidecar_cleanup_ok:
            summary["failure_codes"].append("sidecar_cleanup_incomplete")
        summary["failure_codes"].extend(sidecar.failure_codes)
        if sidecar.failure_codes or not sidecar_cleanup_ok:
            summary["result"] = "real_run_failed_or_partial"
        if notifier and notifier.poll() is None:
            notifier.terminate()
            try:
                notifier.wait(timeout=3)
            except subprocess.TimeoutExpired:
                notifier.kill()
                notifier.wait()
        if daemon and daemon.poll() is None:
            daemon.terminate()
            try:
                daemon.wait(timeout=3)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()
        if host_reader:
            host_reader.join(timeout=3)
        try:
            cleanup_ok = MVP.stop_root(bridge, root_process)
        except (OSError, subprocess.SubprocessError):
            cleanup_ok = False
        if startup_reader:
            startup_reader.join(timeout=3)
        summary["root_cleanup_complete"] = cleanup_ok
        summary["host_startup_diagnostic_codes"] = host_codes[:8]
        summary["root_startup_diagnostic_codes"] = startup_codes[:8]
        if not cleanup_ok:
            summary["failure_codes"].append("collector_cleanup_incomplete")
            summary["result"] = "failed_root_cleanup_incomplete"
        if database.exists():
            try:
                evidence = MVP.load_evidence(database)
                summary["retained_evidence_counts"] = {
                    "events": len(evidence["events"]), "alerts": len(evidence["alerts"]),
                    "notification_feedback": len(evidence["notifications"]),
                    "health_records": len(evidence["health"]),
                }
            except (OSError, ValueError, RuntimeError, sqlite3.Error):
                summary["failure_codes"].append("database_gap")
        if "socket_parent" in locals() and socket_parent.exists():
            try:
                socket_parent.rmdir()
            except OSError:
                pass
        summary["cases"] = case_results
        summary["tools"] = summarize_tools(tools, version_rows, case_results)
        summary["failure_codes"] = sorted(set(summary["failure_codes"] + [
            row["failure_code"] for row in case_results if row.get("failure_code")
        ] + [row["failure_code"] for row in summary["tools"] if row.get("failure_code")]))
        save_summary(report / "summary.json", summary)


class PrivateArgumentParser(argparse.ArgumentParser):
    def error(self, message):
        print(json.dumps({"result": "rejected", "failure_code": "invalid_arguments"}), file=sys.stderr)
        raise SystemExit(2)


def main():
    parser = PrivateArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=REPO / "target" / "release" / "codeperimeter")
    parser.add_argument("--collector-binary", type=Path)
    parser.add_argument("--tools-dir", type=Path,
                        help="外部工具目录；未找到的名称继续按 PATH 搜索，默认检测本机验收工具目录")
    parser.add_argument("--rar-binary", type=Path,
                        help="外部 RAR 试用二进制；不会复制到项目")
    parser.add_argument("--report-dir", type=Path,
                        help="项目目录外的空目录；省略时在 /private/tmp 创建私有目录")
    parser.add_argument("--exercise-only", action="store_true",
                        help="执行本机合成工具调用但不启动采集器，不构成监控验收")
    parser.add_argument("--fence-worker", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    os.umask(0o077)
    default_tools_dir = Path.home() / "Library" / "Application Support" / "CodePerimeter" / "validation-tools" / "bin"
    if args.tools_dir is None and default_tools_dir.is_dir():
        args.tools_dir = default_tools_dir
    if args.fence_worker:
        return fence_worker(args.fence_worker)
    try:
        report = checked_report_path(args.report_dir)
    except ValueError as error:
        code = str(error) if str(error) in FAILURE_CODES else "report_dir_unsafe"
        print(json.dumps({"result": "rejected", "failure_code": code}, ensure_ascii=False))
        return 2
    try:
        tools = discover_tools(args.tools_dir, args.rar_binary)
        version_rows = {}
        for name in TOOLS:
            if name not in tools:
                version_rows[name] = {"version": None, "version_return_code": None}
                continue
            try:
                version, return_code = version_probe(name, tools[name])
            except (OSError, subprocess.SubprocessError):
                version, return_code = None, None
            version_rows[name] = {"version": version, "version_return_code": return_code}
        if args.exercise_only:
            summary = exercise_only(tools, report, version_rows)
            summary["tool_inventory"] = [{
                "tool": name, "version": version_rows[name]["version"],
                "available": name in tools,
                "failure_code": (
                    "tool_missing" if name not in tools else
                    "version_probe_failed" if version_rows[name]["version_return_code"] not in (0, None) else
                    "version_unavailable" if version_rows[name]["version"] is None else None
                ),
            } for name in TOOLS]
            missing = len(tools) != len(TOOLS)
            failed_case = any(item.get("failure_code") for item in summary["cases"])
            failed_version = any(item.get("failure_code") for item in summary["tool_inventory"])
            summary["result"] = (
                "exercise_partial" if missing or failed_case or failed_version
                else "exercise_completed_not_real_validation"
            )
        else:
            summary = real_validation(args, report, tools, version_rows)
        summary.setdefault("failure_codes", [])
        summary["failure_codes"] = sorted(set(summary["failure_codes"] + [
            item["failure_code"] for item in summary.get("cases", []) if item.get("failure_code")
        ] + [item.get("failure_code") for item in summary.get("tool_inventory", [])
             if item.get("failure_code")]))
        save_summary(report / "summary.json", summary)
        print(json.dumps({"result": summary["result"], "tool_count": len(TOOLS),
                          "case_count": len(summary.get("cases", summary.get("tools", []))),
                          "failure_codes": summary["failure_codes"],
                          "summary": str(report / "summary.json")}, ensure_ascii=False))
        return 0 if summary["result"] in ("real_run_passed", "exercise_completed_not_real_validation") else 1
    except (OSError, ValueError, RuntimeError, TimeoutError, subprocess.SubprocessError,
            sqlite3.Error, KeyboardInterrupt) as error:
        interrupted = isinstance(error, KeyboardInterrupt)
        code = "interrupted" if interrupted else failure_code_for(error)
        try:
            summary = json.loads((report / "summary.json").read_text())
        except (OSError, ValueError, json.JSONDecodeError):
            summary = {
                "mode": "exercise_only" if args.exercise_only else "real_eslogger",
                "tools": [{"tool": name, "version": None, "available": False,
                           "cases": [], "failure_code": "not_started"} for name in TOOLS],
                "cases": [],
                "failure_codes": [],
            }
        summary["result"] = "interrupted" if interrupted else "failed_no_fixture_fallback"
        summary["failure_codes"] = sorted(set(
            summary.get("failure_codes", []) + [code]
        ))
        save_summary(report / "summary.json", summary)
        print(json.dumps({"result": summary["result"], "tool_count": len(TOOLS),
                          "case_count": len(summary.get("cases", [])),
                          "failure_codes": [code],
                          "summary": str(report / "summary.json")}, ensure_ascii=False))
        return 130 if interrupted else 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print(json.dumps({"result": "interrupted", "failure_code": "interrupted"}), file=sys.stderr)
        raise SystemExit(130)
    except Exception:
        print(json.dumps({"result": "failed", "failure_code": "validation_startup_failed"}), file=sys.stderr)
        raise SystemExit(2)
