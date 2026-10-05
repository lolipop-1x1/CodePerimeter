#!/usr/bin/env python3
"""验收判定器的匿名自测；不启动采集，不能作为真实 ES 或通知验收。"""
from contextlib import ExitStack
import hashlib
import importlib.util
import io
import itertools
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("validation", Path(__file__).with_name("validate-mvp.py"))
validation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(validation)
prepare_spec = importlib.util.spec_from_file_location("prepare", Path(__file__).with_name("validate-prepare-collector.py"))
prepare = importlib.util.module_from_spec(prepare_spec)
prepare_spec.loader.exec_module(prepare)
PROJECT = Path("/private/tmp/anonymous-validation/project")
PROCESS = {"pid": 100, "pid_version": 4}


def event(timestamp, kind="open", index=0, **extra):
    value = {"source_run_id": "anonymous-run", "source_timestamp_ms": timestamp,
             "global_seq": timestamp, "kind": kind, "process": PROCESS,
             "file": {"path": str(PROJECT / "src" / f"file-{index}.txt"),
                      "path_truncated": False, "readable": True, "is_regular": True,
                      "device": 7, "inode": index + 1}, "destination": None}
    value.update(extra)
    return value


def evidence(events, rule, first, last, generated, notified):
    return {"events": events,
            "alerts": [{"id": "anonymous-alert", "rule": rule, "process": PROCESS,
                        "first_timestamp_ms": first, "last_timestamp_ms": last}],
            "outbox": {} if generated is None else {"anonymous-alert": generated},
            "notifications": [] if notified is None else [
                {"alert_id": "anonymous-alert", "observed_timestamp_ms": notified, "outcome": "sent"}]}


def analyze(value):
    status = {"observed_events_by_kind": {kind: 0 for kind in validation.KINDS}}
    return validation.analyze(value, [], PROJECT, status, status)


class ValidationTimingTests(unittest.TestCase):
    def test_fiftieth_distinct_source_is_trigger_even_when_last_alert_is_later(self):
        events = []
        for index in range(55):
            timestamp = 1000 + index * 10
            events.extend([event(timestamp, index=index), event(timestamp + 1, "mmap", index)])
        report = analyze(evidence(events, "bulk_file_access", 1000, 5000, 1600, 1750))
        self.assertEqual(report["timing"][0]["trigger_source_ms"], 1490)
        self.assertEqual(report["generation_latency"]["max_ms"], 110)
        self.assertEqual(report["notification_send_latency"]["max_ms"], 260)
        repeated = [event(1000 + index, index=0) for index in range(55)]
        self.assertFalse(validation.triggers({"events": repeated}, PROJECT))

    def test_archive_uses_first_output_after_reading_not_final_merge_timestamp(self):
        output = event(850, "create")["file"] | {"path": str(PROJECT / "result.zip"), "readable": None}
        events = [event(850, "create", file=output), event(900),
                  event(1100, "write", file=output), event(1250, "write", file=output)]
        report = analyze(evidence(events, "archive_output", 900, 5000, 1200, 1300))
        self.assertEqual(report["timing"][0]["trigger_source_ms"], 1100)
        self.assertEqual(report["generation_latency"]["max_ms"], 100)
        self.assertTrue(report["notification_send_latency"]["within_3000_ms"])

    def test_missing_and_negative_feedback_cannot_pass(self):
        command = event(1000, "exec", file=None, archive={"tool": "tar"})
        missing = analyze(evidence([command], "archive_command", 1000, 1000, None, None))
        self.assertEqual(missing["generation_latency"]["missing_count"], 1)
        self.assertEqual(missing["notification_send_latency"]["missing_count"], 1)
        self.assertFalse(missing["generation_latency"]["within_3000_ms"])
        negative = analyze(evidence([command], "archive_command", 1000, 1000, 800, 900))
        self.assertFalse(negative["generation_latency"]["within_3000_ms"])
        self.assertFalse(negative["notification_send_latency"]["within_3000_ms"])
        self.assertFalse(validation.latency_summary([3001], 0)["within_3000_ms"])


class ValidationArchiveOutputTests(unittest.TestCase):
    def case(self, scenario, scope, output_event=True, associated=True, declared=True, source_generation=4, project_root=PROJECT, output_before_read=False, command_output=True):
        output = str(PROJECT / "result.zip" if scope == "inside" else Path("/private/tmp/anonymous-output/result.zip"))
        events = [event(1000 + index, index=index, process=PROCESS | {"pid_version": source_generation}) for index in range(55)]
        events.append(event(900, "exec", file=None, archive={"tool": scenario, "output_path": output if command_output else None}))
        if output_event:
            events.append(event(950 if output_before_read else 1100, "write", file=event(1100)["file"] | {"path": output, "readable": None}))
        alerts = [{"id": rule, "rule": rule, "process": PROCESS,
                   "first_timestamp_ms": 900, "last_timestamp_ms": 1200,
                   "roots": [str(project_root)], "evidence_paths": []}
                  for rule in ("bulk_file_access", "archive_command")]
        if associated:
            alerts.append(alerts[0] | {"id": "output", "rule": "archive_output"})
        value = {"events": events, "alerts": alerts, "outbox": {}, "notifications": []}
        operation = {"scenario": scenario, "scope": scope,
                     "metadata": [{"pid": PROCESS["pid"]} | ({"output_path": output} if declared else {})]}
        status = {"observed_events_by_kind": {kind: 0 for kind in validation.KINDS}}
        return validation.analyze(value, [operation], PROJECT, status, status)["cases"][0]

    def test_tar_and_zip_require_actual_output_in_both_locations(self):
        for scenario in ("tar", "zip"):
            for scope in ("inside", "temporary"):
                with self.subTest(scenario=scenario, scope=scope):
                    self.assertTrue(self.case(scenario, scope)["passed"])
                    missing = self.case(scenario, scope, output_event=False)
                    self.assertFalse(missing["passed"])
                    self.assertTrue(missing["missing_evidence"])

    def test_output_without_project_association_cannot_pass(self):
        missing = self.case("zip", "temporary", associated=False, command_output=False)
        self.assertFalse(missing["passed"])
        self.assertTrue(missing["observed_output_paths"])
        self.assertFalse(missing["project_associated_output_paths"])

    def test_output_before_read_uses_actual_command_project_association(self):
        for scenario in ("tar", "zip"):
            self.assertTrue(self.case(scenario, "temporary", associated=False, output_before_read=True)["passed"])
            self.assertFalse(self.case(scenario, "temporary", associated=False, output_before_read=True,
                                       command_output=False)["passed"])

    def test_project_or_process_generation_mismatch_cannot_pass(self):
        self.assertFalse(self.case("tar", "temporary", source_generation=8)["passed"])
        self.assertFalse(self.case("zip", "inside", project_root=PROJECT.parent / "another-project")["passed"])

    def test_limited_alert_path_sample_does_not_replace_actual_output(self):
        # case的告警路径样本均为空，仍以真实输出与项目读取／告警验证。
        self.assertTrue(self.case("zip", "temporary", command_output=False)["passed"])
        self.assertFalse(self.case("zip", "temporary", output_event=False, command_output=False)["passed"])

    def test_undeclared_output_cannot_pass(self):
        missing = self.case("tar", "inside", declared=False)
        self.assertFalse(missing["passed"])
        self.assertIn("发送器未声明预期归档输出", missing["missing_evidence"])


class ValidationCompletionTests(unittest.TestCase):
    def setUp(self):
        self.status = {"observed_events_by_kind": {kind: 0 for kind in validation.KINDS}}
        self.fence = {"pid": 999, "path": str(PROJECT / ".validation-fence-anonymous")}
        self.operation = {"scenario": "read", "scope": "inside", "metadata": [{"pid": 100}]}
        self.base = {"events": [], "alerts": [], "outbox": {}, "notifications": []}
        self.fence_event = event(1100, process={"pid": 999, "pid_version": 5},
                                 file=event(1100)["file"] | {"path": self.fence["path"]})

    def wait(self, snapshots, pending=None, timeout=.01):
        with patch.object(validation, "load_evidence", side_effect=snapshots), patch.object(validation, "control", return_value=pending or []) as control:
            result, snapshot = validation.wait_for_completion(Path("/anonymous.sqlite"), Path("/anonymous.sock"),
                                                              [self.operation], PROJECT, self.status, self.fence, timeout)
        return result, snapshot, control

    def test_empty_outbox_before_event_progress_cannot_complete_and_retains_failure(self):
        # 旧逐场景条件此时为真；但来源访问与业务证据都还没有进入数据库。
        self.assertFalse(self.base["outbox"])
        result, snapshot, control = self.wait([self.base], timeout=0)
        self.assertFalse(result["completed"])
        self.assertFalse(result["source_fence_crossed"])
        self.assertEqual(result["pending_cases"], [{"scenario": "read", "output_scope": "inside"}])
        self.assertEqual(snapshot, self.base)
        control.assert_not_called()
        self.assertIn("deadline_timestamp_ms", result)

    def test_fence_alone_does_not_replace_expected_business_evidence(self):
        result, _, control = self.wait([self.base | {"events": [self.fence_event]}], timeout=0)
        self.assertTrue(result["source_fence_crossed"])
        self.assertFalse(result["expected_evidence_complete"])
        self.assertFalse(result["completed"])
        control.assert_not_called()

    def test_real_fence_requires_read_flag_matching_pid_path_and_sequence(self):
        for replacement in ({"global_seq": None}, {"kind": "close"}, {"process": PROCESS},
                            {"file": self.fence_event["file"] | {"readable": False}},
                            {"file": self.fence_event["file"] | {"path_truncated": True}},
                            {"file": self.fence_event["file"] | {"path": str(PROJECT / "other")}}):
            with self.subTest(replacement=replacement):
                snapshot = self.base | {"events": [event(1000), self.fence_event | replacement]}
                self.assertFalse(validation.completion_state(snapshot, [self.operation], PROJECT, self.status, self.fence)["source_fence_crossed"])

    def test_lagged_stream_waits_then_uses_feedback_after_drain_without_relaxing_latency(self):
        ready = self.base | {"events": [event(1000), self.fence_event]}
        final = ready | {"notifications": [{"alert_id": "anonymous", "outcome": "sent", "observed_timestamp_ms": 4100}]}
        result, snapshot, control = self.wait([self.base, ready, final], timeout=1)
        self.assertTrue(result["completed"])
        self.assertTrue(result["notifications_drained"])
        self.assertEqual(snapshot, final)
        control.assert_called_once()
        case = validation.analyze(snapshot, [self.operation], PROJECT, self.status, self.status)["cases"][0]
        self.assertEqual(case["standard_event_count"], 1, "独立 fence PID 不增加业务读取数")
        self.assertFalse(validation.latency_summary([3100], 0)["within_3000_ms"])

    def test_feedback_read_after_deadline_preserves_evidence_but_cannot_complete(self):
        ready = self.base | {"events": [event(1000), self.fence_event]}
        with patch.object(validation.time, "monotonic", side_effect=[0, .5, 1.5]):
            result, snapshot, _ = self.wait([ready, ready], timeout=1)
        self.assertTrue(result["source_fence_crossed"])
        self.assertTrue(result["notifications_drained"])
        self.assertFalse(result["completed"])
        self.assertEqual(snapshot, ready)

    def test_unfinished_notifications_cannot_complete_even_after_source_fence(self):
        ready = self.base | {"events": [event(1000), self.fence_event]}
        for pending, completed, clock in (([{"alert_id": "anonymous"}], False, [0, .1, 1.1]),
                                           ([], True, [0, .1, .2])):
            with self.subTest(pending=pending), patch.object(validation.time, "monotonic", side_effect=clock):
                snapshots = [ready, ready] if completed else [ready]
                result, _, control = self.wait(snapshots, pending=pending, timeout=1)
            control.assert_called_once_with(Path("/anonymous.sock"), "pending_notifications", {"limit": 10000})
            self.assertTrue(result["source_fence_crossed"])
            self.assertTrue(result["expected_evidence_complete"])
            self.assertEqual(result["notifications_drained"], completed)
            self.assertEqual(result["completed"], completed)

    def test_fence_worker_is_independent_and_refuses_existing_file(self):
        with tempfile.TemporaryDirectory(prefix="codeperimeter-fence-test-", dir="/private/tmp") as directory:
            path = Path(directory) / ".validation-fence-anonymous"
            command = [sys.executable, "-B", validation.__file__, "--source-fence-worker", str(path)]
            result = subprocess.run(command, capture_output=True, text=True, timeout=15, start_new_session=True)
            self.assertEqual(result.returncode, 0)
            metadata = json.loads(result.stdout)
            self.assertNotEqual(metadata["pid"], os.getpid())
            self.assertEqual(metadata["path"], str(path))
            self.assertTrue(metadata["success"])
            self.assertTrue(path.is_file())
            result = subprocess.run(command, capture_output=True, text=True, timeout=15, start_new_session=True)
            self.assertNotEqual(result.returncode, 0)


class ValidationStartupTests(unittest.TestCase):
    def test_startup_diagnostics_are_bounded_static_and_never_echo_raw(self):
        stream = io.BytesIO(("执行失败：root 服务文件或父目录可被普通用户修改\n"
                             + "anonymous-secret=/unknown/private/path " * 10000 + "\n"
                             + "sudo: a password is required\n").encode())
        codes = []
        validation.startup_diagnostics(stream, codes)
        self.assertEqual(codes, ["unsafe_root_path", "unclassified_stderr", "sudo_authorization_required"])
        self.assertTrue(stream.closed)
        self.assertLessEqual(len(codes), 8)
        self.assertNotIn("anonymous-secret", str(codes))
        self.assertNotIn("/unknown/private/path", str(codes))

    def test_exited_collector_fails_before_timeout_without_control_request(self):
        with patch.object(validation, "control") as control:
            with self.assertRaisesRegex(validation.CollectorStartupError, "退出码 7"):
                validation.wait_for_bridge(Path("/anonymous.sock"), SimpleNamespace(poll=lambda: 7), Path("/anonymous"))
            control.assert_not_called()


class ValidationHostLifecycleTests(unittest.TestCase):
    def exercise_run(self, mode):
        with tempfile.TemporaryDirectory(prefix="cpv-report-test-", dir="/private/tmp") as directory:
            report = Path(directory) / ("persistent-report-" + "x" * 110)
            report.mkdir(mode=0o700)
            project = report / "workspace" / "project"
            args = SimpleNamespace(binary=Path("/anonymous/codeperimeter"), collector_binary=None,
                                   preflight_only=mode == "preflight_only")
            summary = {"real_source_confirmed": False}
            launches, runtime_paths, statuses = [], [], []
            binding = socket.socket(socket.AF_UNIX)
            self.addCleanup(binding.close)
            class Process:
                def __init__(self, code=None, diagnostic=b""):
                    self.returncode, self.pid = code, 123
                    self.stderr, self.stdin = io.BytesIO(diagnostic), io.StringIO()
                def poll(self):
                    return self.returncode
                def wait(self, timeout=None):
                    self.returncode = self.returncode or 0
                    return self.returncode
                def terminate(self):
                    self.returncode = 0
                    binding.close()
                kill = terminate
            def launch(command, **kwargs):
                launches.append(command)
                if command[1] == "daemon":
                    self.assertEqual(kwargs["stderr"], subprocess.PIPE)
                    path = Path(command[command.index("--control-socket") + 1])
                    runtime_paths.append(path)
                    self.assertLess(len(os.fsencode(path)), 104)
                    self.assertEqual(path.parent.stat().st_mode & 0o777, 0o700)
                    self.assertEqual(Path(command[command.index("--db") + 1]), report / "host" / "events.sqlite")
                    if mode == "early_exit":
                        return Process(7, b"path must be shorter than SUN_LEN /unknown-private-marker\n")
                    binding.bind(str(path))
                    return Process()
                if command[1] == "notify":
                    self.assertEqual(Path(command[command.index("--control-socket") + 1]), runtime_paths[0])
                return Process()
            def invoke(command, **kwargs):
                if "prepare" in command:
                    if mode == "prepare_error":
                        raise RuntimeError("anonymous prepare failure")
                    project.mkdir(parents=True)
                    return SimpleNamespace(stdout=json.dumps({"project_root": str(project)}))
                return SimpleNamespace(stdout=json.dumps({"pid": 999, "path": str(project / ".fence")}))
            status = {"observed_events_by_kind": {kind: 0 for kind in validation.KINDS},
                      "collector_dropped_lines": 0, "reader_dropped_frames": 0,
                      "database_gap_events": 0, "database_state": "ready", "state": "ready"}
            def request(path, operation, payload=None):
                statuses.append(operation)
                self.assertEqual(path, runtime_paths[0])
                if mode == "interrupt":
                    raise KeyboardInterrupt()
                if operation == "list_directories":
                    return [{"path": str(project)}]
                return status | {"collector_state": "connected" if len(statuses) < 6 else "disconnected"}
            events = {"events": [event(1000, file=event(1000)["file"] | {"path": str(project / "src/file.txt")})], "health": []}
            ticks = itertools.count()
            allocation = validation.tempfile.mkdtemp
            def allocate(*arguments, **kwargs):
                path = allocation(*arguments, **kwargs)
                if mode in ("prepare_error", "preflight_error", "preflight_only"):
                    runtime_paths.append(Path(path) / "host.sock")
                return path
            with ExitStack() as patches:
                for name, kwargs in {
                    "preflight": {"side_effect": RuntimeError("anonymous preflight failure")} if mode == "preflight_error" else {"return_value": {}},
                    "start_preloaded": {"return_value": (Process(0), [{"phase": "completed"}], SimpleNamespace(join=lambda timeout: None))},
                    "control": {"side_effect": request}, "load_evidence": {"return_value": events},
                    "sender": {"return_value": [{"pid": 100}]},
                    "wait_for_bridge": {"return_value": {}}, "performance": {"return_value": None},
                    "stop_root": {"return_value": True},
                    "wait_for_completion": {"return_value": ({"completed": True}, events)},
                    "analyze": {"return_value": {"cases": [{"passed": True}], "nine_event_aggregate_passed": True,
                                                       "generation_latency": {"within_3000_ms": True},
                                                       "notification_send_latency": {"within_3000_ms": True}}},
                }.items():
                    patches.enter_context(patch.object(validation, name, **kwargs))
                patches.enter_context(patch.object(validation.subprocess, "Popen", side_effect=launch))
                patches.enter_context(patch.object(validation.subprocess, "run", side_effect=invoke))
                patches.enter_context(patch.object(validation.tempfile, "mkdtemp", side_effect=allocate))
                patches.enter_context(patch.object(validation.time, "sleep"))
                patches.enter_context(patch.object(validation.time, "monotonic", side_effect=lambda: next(ticks)))
                if mode == "early_exit":
                    with self.assertRaisesRegex(RuntimeError, "普通用户 daemon.*退出码 7"):
                        validation.run(args, report, summary)
                elif mode == "interrupt":
                    with self.assertRaises(KeyboardInterrupt):
                        validation.run(args, report, summary)
                elif mode == "preflight_error":
                    with self.assertRaisesRegex(RuntimeError, "anonymous preflight failure"):
                        validation.run(args, report, summary)
                elif mode == "prepare_error":
                    with self.assertRaisesRegex(RuntimeError, "anonymous prepare failure"):
                        validation.run(args, report, summary)
                else:
                    self.assertEqual(validation.run(args, report, summary), 0)
            self.assertTrue(runtime_paths)
            self.assertTrue(all(not path.parent.exists() for path in runtime_paths))
            self.assertTrue(report.is_dir())
            if mode == "normal":
                self.assertTrue((report / "operations.json").is_file())
                self.assertTrue(project.is_dir())
                collector = next(command for command in launches if command[0] == "/usr/bin/sudo")
                self.assertEqual(collector[collector.index("--socket") + 1],
                                 f"/Library/CodePerimeter/{os.getuid()}/run/collector.sock")
            else:
                self.assertFalse(any(command[0] == "/usr/bin/sudo" for command in launches))
            if mode in ("preflight_only", "preflight_error", "prepare_error"):
                self.assertFalse(launches)
            if mode == "early_exit":
                self.assertFalse(statuses)
                self.assertEqual(summary["phase"], "host_startup")
                self.assertEqual(summary["host_startup"]["exit_code"], 7)
                self.assertEqual(summary["host_startup"]["diagnostic_codes"], ["socket_path_too_long"])
                self.assertTrue(summary["host_startup"]["stderr_complete"])
                self.assertNotIn("unknown-private-marker", str(summary))

    def test_long_persistent_report_uses_short_private_socket_and_cleans_after_success(self):
        self.exercise_run("normal")

    def test_host_early_exit_is_diagnosed_before_any_root_launch_and_cleans_runtime(self):
        self.exercise_run("early_exit")

    def test_keyboard_interrupt_cleans_runtime_without_starting_root(self):
        self.exercise_run("interrupt")

    def test_prepare_failure_also_cleans_runtime(self):
        self.exercise_run("prepare_error")

    def test_preflight_failure_also_cleans_runtime(self):
        self.exercise_run("preflight_error")

    def test_preflight_only_cleans_runtime_without_starting_any_process(self):
        self.exercise_run("preflight_only")


class ValidationBridgeIdentityTests(unittest.TestCase):
    def setUp(self):
        self.collector = Path("/Library/CodePerimeter/501/codeperimeter")
        self.launcher = SimpleNamespace(pid=100, poll=lambda: None)
        self.status = {"collector_run_id": "eslogger-200-1791123401112"}
        self.processes = {200: (0, 150, 200, str(self.collector)), 150: (0, 100, 150, "/usr/bin/sudo")}

    def identity(self, status=None):
        with patch.object(validation, "process_info", side_effect=lambda pid: self.processes.get(pid)), patch.object(validation, "children", return_value=[201]):
            return validation.bridge_identity(status or self.status, self.launcher, self.collector)

    def test_exact_root_path_group_and_this_sudo_ancestry_are_required(self):
        self.assertEqual(self.identity()["pid"], 200)
        for code, process in (
            ("collector_not_root", (501, 150, 200, str(self.collector))),
            ("collector_process_group_mismatch", (0, 150, 150, str(self.collector))),
            ("collector_executable_mismatch", (0, 150, 200, "/private/tmp/unknown-secret")),
            ("collector_sudo_ancestry_mismatch", (0, 1, 200, str(self.collector))),
            ("collector_process_missing", None),
        ):
            with self.subTest(code=code):
                self.processes[200] = process
                with self.assertRaises(validation.BridgeIdentityError) as result:
                    self.identity()
                self.assertEqual(result.exception.code, code)
                self.assertNotIn("unknown-secret", str(result.exception))
        with self.assertRaisesRegex(validation.BridgeIdentityError, "invalid_run_id"):
            self.identity({"collector_run_id": "unknown-private-marker"})

    def test_wait_never_swallows_identity_rejection_into_timeout(self):
        diagnostics = {}
        with patch.object(validation, "control", return_value=self.status), patch.object(validation, "bridge_identity", side_effect=validation.BridgeIdentityError("collector_not_root")):
            with self.assertRaises(validation.BridgeIdentityError):
                validation.wait_for_bridge(Path("/anonymous.sock"), self.launcher, self.collector, diagnostics)
        self.assertEqual(diagnostics["reason"], "collector_not_root")
        self.assertEqual(diagnostics["result"], "failed")

    def test_missing_run_id_and_unavailable_status_have_distinct_anonymous_diagnostics(self):
        for value, code in (({}, "bridge_run_id_missing"), (OSError("unknown-private-marker"), "bridge_status_unavailable")):
            diagnostics = {}
            kwargs = {"side_effect": value} if isinstance(value, OSError) else {"return_value": value}
            with patch.object(validation, "control", **kwargs):
                with self.assertRaisesRegex(TimeoutError, code):
                    validation.wait_for_bridge(Path("/anonymous.sock"), self.launcher, self.collector, diagnostics, timeout=.01)
            self.assertEqual(diagnostics["reason"], code)
            self.assertNotIn("unknown-private-marker", str(diagnostics))


class ValidationMatrixWorkflowTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="codeperimeter-matrix-test-", dir="/private/tmp")
        self.addCleanup(temporary.cleanup)
        self.workspace = Path(temporary.name) / "workspace"
        self.sender_cli("prepare", "--root", str(self.workspace), "--files", "55")
        self.project = self.workspace / "project"

    def sender_cli(self, *arguments):
        result = subprocess.run([sys.executable, "-B", str(validation.SENDER), *arguments],
                                capture_output=True, text=True, timeout=45, start_new_session=True)
        self.assertEqual(result.returncode, 0, "匿名发送器 CLI 未成功完成")
        return [json.loads(line) for line in result.stdout.splitlines()]

    def matrix(self):
        return subprocess.run([sys.executable, "-B", validation.__file__, "--event-matrix-worker", str(self.project)],
                              capture_output=True, text=True, timeout=15, start_new_session=True)

    def test_successful_matrix_allows_following_mmap_and_strict_cleanup(self):
        self.assertEqual(self.matrix().returncode, 0)
        rows = validation.sender(self.workspace, "mmap")
        self.assertEqual(rows[-1]["phase"], "completed")
        self.assertFalse((self.project / ".event-matrix").exists())
        self.sender_cli("cleanup", "--root", str(self.workspace))
        self.assertFalse(self.workspace.exists())

    def test_complete_original_scenario_order_and_preloaded_release_can_finish(self):
        preloaded, rows, reader = validation.start_preloaded(self.workspace)
        try:
            self.assertTrue(any(row["phase"] == "ready" for row in rows))
            self.assertEqual(validation.sender(self.workspace, "read")[-1]["phase"], "completed")
            preloaded.stdin.write("release\n")
            preloaded.stdin.flush()
            preloaded.wait(timeout=30)
            reader.join(timeout=3)
            self.assertFalse(reader.is_alive())
            self.assertEqual(preloaded.returncode, 0)
            self.assertEqual(rows[-1]["phase"], "completed")
            self.assertTrue(any(row["phase"] == "released" for row in rows))
            self.assertEqual(self.matrix().returncode, 0)
            temporary_archives = []
            for scenario, scope in [("mmap", "inside"), ("repeat-read", "inside"), ("bulk-read", "inside"),
                                    ("tar", "inside"), ("tar", "temporary"), ("zip", "inside"), ("zip", "temporary"),
                                    ("disk-archive", "inside"), ("disk-archive", "temporary"), ("memory-archive", "inside"),
                                    ("search", "inside"), ("index", "inside"), ("build", "inside")]:
                with self.subTest(scenario=scenario, scope=scope):
                    result = validation.sender(self.workspace, scenario, scope)
                    self.assertEqual(result[-1]["phase"], "completed")
                    if scope == "temporary":
                        temporary_archives.extend(Path(row["output_path"]).parent for row in result if row["phase"] == "archive_completed")
            self.sender_cli("cleanup", "--root", str(self.workspace))
            self.assertFalse(self.workspace.exists())
            self.assertTrue(temporary_archives)
            self.assertTrue(all(not directory.exists() for directory in temporary_archives))
        finally:
            if preloaded.poll() is None:
                preloaded.kill()
                preloaded.wait()
            reader.join(timeout=3)
            preloaded.stdin.close()
            preloaded.stdout.close()

    def test_existing_matrix_directory_and_its_unknown_file_are_preserved(self):
        directory = self.project / ".event-matrix"
        directory.mkdir()
        unknown = directory / "unknown-owned-by-another-task.txt"
        unknown.write_bytes(b"anonymous external artifact")
        self.assertNotEqual(self.matrix().returncode, 0)
        self.assertEqual(unknown.read_bytes(), b"anonymous external artifact")
        self.assertEqual(set(directory.iterdir()), {unknown})

    def test_unknown_file_added_to_owned_directory_is_never_deleted(self):
        # 仅替换fork/exec为匿名等待结果；其余创建/读取/mmap/rename和清理执行真实文件操作。
        unknown = self.project / ".event-matrix" / "unknown.txt"
        def completed_child(pid, options):
            unknown.write_bytes(b"anonymous external artifact")
            return pid, 0
        with patch.object(validation.os, "fork", return_value=123), patch.object(validation.os, "waitpid", side_effect=completed_child):
            with self.assertRaises(OSError):
                validation.matrix_worker(self.project)
        self.assertEqual(unknown.read_bytes(), b"anonymous external artifact")


class ProtectedReplacementTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.namespace = {"__name__": "test_only"}
        exec(prepare.ROOT_REPLACE_CODE, self.namespace)
        self.namespace["INSTALL_ROOT"] = self.root / "CodePerimeter"
        self.uid = os.getuid()
        self.directory = self.namespace["INSTALL_ROOT"] / str(self.uid)
        self.directory.mkdir(parents=True)
        self.destination = self.directory / "codeperimeter"
        self.staged = self.directory / (".codeperimeter-prepare-" + "a" * 32)
        self.destination.write_bytes(b"anonymous-old")
        self.staged.write_bytes(b"anonymous-new")
        self.old = hashlib.sha256(self.destination.read_bytes()).hexdigest()
        self.new = hashlib.sha256(self.staged.read_bytes()).hexdigest()

    def tearDown(self):
        self.temporary.cleanup()

    def replace(self, old=None, staged=None):
        # 测试只写普通用户匿名目录；root路径/身份的替身不代表实际管理员发布。
        with patch.dict(self.namespace, {"check_chain": lambda path: None, "ensure_quiet": lambda uid, directory: None}), patch.object(os, "geteuid", return_value=0):
            self.namespace["replace"](self.destination, staged or self.staged, old or self.old, self.new, self.uid)

    def test_exact_old_hash_allows_atomic_replacement_but_mismatch_preserves_old(self):
        with self.assertRaisesRegex(RuntimeError, "SHA256"):
            self.replace(old="b" * 64)
        self.assertEqual(self.destination.read_bytes(), b"anonymous-old")
        self.assertTrue(self.staged.exists())
        self.replace()
        self.assertEqual(self.destination.read_bytes(), b"anonymous-new")
        self.assertFalse(self.staged.exists())

    def test_root_publication_rechecks_trust_and_quiet_before_replacing(self):
        for gate, message in (("check_chain", "root链拒绝"), ("ensure_quiet", "活动端点拒绝")):
            def rejected(*args):
                raise RuntimeError(message)
            with patch.dict(self.namespace, {"check_chain": lambda path: None, "ensure_quiet": lambda uid, directory: None,
                                             gate: rejected}), patch.object(os, "geteuid", return_value=0):
                with self.assertRaisesRegex(RuntimeError, message):
                    self.namespace["replace"](self.destination, self.staged, self.old, self.new, self.uid)
            self.assertEqual(self.destination.read_bytes(), b"anonymous-old")
            self.assertTrue(self.staged.exists())

    def test_unknown_staging_and_symlink_target_are_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "固定路径"):
            self.replace(staged=self.directory / "unknown-file")
        other = self.directory / "unknown-data"
        other.write_bytes(b"anonymous-old")
        self.destination.unlink()
        self.destination.symlink_to(other)
        with self.assertRaisesRegex(RuntimeError, "SHA256"):
            self.replace()
        self.assertEqual(other.read_bytes(), b"anonymous-old")

    def quiet(self, comm="", loaded=False):
        def command(args, **kwargs):
            return SimpleNamespace(returncode=0 if loaded and args[0] == "/bin/launchctl" else 1,
                                   stdout=comm if args[0] == "/bin/ps" else "")
        with patch.object(self.namespace["pwd"], "getpwuid", return_value=SimpleNamespace(pw_dir=str(self.root / "home"))), patch.object(self.namespace["subprocess"], "run", side_effect=command):
            self.namespace["ensure_quiet"](self.uid, self.directory)

    def test_new_legacy_and_control_endpoints_refuse_replacement_without_cleanup(self):
        for path in (self.directory / "run/collector.sock", Path(f"/var/run/codeperimeter-{self.uid}/collector.sock"),
                     self.root / "home/Library/Application Support/CodePerimeter/host.sock"):
            with self.subTest(endpoint=path), patch.object(os.path, "lexists", side_effect=lambda candidate: candidate == path):
                with self.assertRaisesRegex(RuntimeError, "端点"):
                    self.quiet()
        self.assertEqual(self.destination.read_bytes(), b"anonymous-old")

    def test_installed_or_loaded_jobs_and_any_active_codeperimeter_are_rejected(self):
        with patch.object(os.path, "lexists", side_effect=lambda candidate: str(candidate).endswith(f"com.codeperimeter.collector.{self.uid}.plist")):
            with self.assertRaisesRegex(RuntimeError, "已安装或加载"):
                self.quiet()
        with self.assertRaisesRegex(RuntimeError, "已安装或加载"):
            self.quiet(loaded=True)
        with self.assertRaisesRegex(RuntimeError, "进程"):
            self.quiet(comm="/anonymous/bin/codeperimeter\n")
        self.quiet(comm="/usr/bin/unrelated\n")

    def test_root_guard_rechecks_mode_and_write_acl(self):
        with patch.object(Path, "lstat", return_value=SimpleNamespace(st_uid=0, st_mode=stat.S_IFDIR | 0o775)):
            with self.assertRaisesRegex(RuntimeError, "不可写"):
                self.namespace["check_chain"](Path("/private/var/run"))
        with patch.object(Path, "lstat", return_value=SimpleNamespace(st_uid=0, st_mode=stat.S_IFDIR | 0o755)), patch.object(self.namespace["subprocess"], "run", return_value=SimpleNamespace(stdout="anonymous\n 0: group:staff allow add_file\n")):
            with self.assertRaisesRegex(RuntimeError, "写ACL"):
                self.namespace["check_chain"](Path("/Library/CodePerimeter"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
