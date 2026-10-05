#!/usr/bin/env python3
"""对归档验收裁决器执行反误通过自检。"""

import copy
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import pty
import select
import sqlite3
import sys
import tempfile
import time
import unittest
from unittest import mock


sys.dont_write_bytecode = True
SCRIPT_PATH = Path(__file__).with_name("validate-archive-commands.py")
SPEC = importlib.util.spec_from_file_location("codeperimeter_archive_validation", SCRIPT_PATH)
VALIDATOR = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VALIDATOR
SPEC.loader.exec_module(VALIDATOR)


SOURCE_RUN = "synthetic-run"
PROJECT = Path("/private/tmp/synthetic-project")
UNRELATED = Path("/private/tmp/unrelated-project")
INPUT = PROJECT / "source files" / "source one.bin"
OUTPUT = PROJECT / ".archives" / "source.zip"
FENCE = PROJECT / ".fences" / "independent-fence.bin"
PID = 41001
PID_VERSION = 17
ALERT_ID = "synthetic-alert"


def process(pid=PID, pid_version=PID_VERSION, executable="/usr/bin/zip"):
    return {"pid": pid, "pid_version": pid_version, "executable": executable}


def event(kind="exec", identity=None, archive=None, file=None):
    return {
        "source_run_id": SOURCE_RUN,
        "source_timestamp_ms": 1000,
        "received_timestamp_ms": 1005,
        "global_seq": 10,
        "kind": kind,
        "process": identity or process(),
        "archive": archive,
        "file": file,
    }


def archive_metadata(tool="zip", input_path=INPUT, output_path=OUTPUT):
    return {
        "tool": tool,
        "input_paths": [str(input_path)],
        "output_path": str(output_path) if output_path else None,
        "output_paths": [],
        "cwd": str(PROJECT),
    }


def command_alert(rule="archive_command", identity=None, roots=None):
    return {
        "id": ALERT_ID,
        "rule": rule,
        "process": identity or process(),
        "roots": [str(PROJECT)] if roots is None else [str(path) for path in roots],
        "first_timestamp_ms": 1000,
        "last_timestamp_ms": 1000,
    }


def sidecar_exec(pid=PID, pid_version=PID_VERSION, executable="zip", related_pids=None):
    return {
        "target_pid": pid,
        "target_pid_version": pid_version,
        "ppid": 40000,
        "pre_exec_pid": pid,
        "pre_exec_pid_version": pid_version - 1,
        "executable": executable,
        "related_pid_chain": [pid] if related_pids is None else list(related_pids),
    }


def main_exec_receipt(stream="combined"):
    return {"run_id": SOURCE_RUN, "pid": PID, "pid_version": PID_VERSION,
            "source_stream": stream}


def source_health(stream="combined"):
    return {stream: {"lines": 1, "parsed_events": 1, "skipped_lines": 0,
                     "lines_with_issues": 0, "last_received_timestamp_ms": 1005,
                     "schema_version": 1, "message_version": 9,
                     "global_sequence_available": True, "event_sequence_available": True,
                     "sequence_gaps": 0}}


def positive_case(tool="zip", mode="create", input_path=INPUT, output_path=OUTPUT):
    return VALIDATOR.Case(
        tool=tool,
        mode=mode,
        argv=[],
        cwd=PROJECT,
        project_root=PROJECT,
        inputs=(input_path,),
        outputs=(output_path,) if output_path else (),
        positive=True,
        stdout_expected=output_path is None,
    )


def positive_fixture(case=None):
    case = case or positive_case()
    actual_tool = case.tool
    if actual_tool == "7zz":
        actual_tool = "7z"
    actual_executable = "/usr/bin/" + actual_tool
    actual_inputs = case.inputs[0]
    actual_output = case.outputs[0] if case.outputs else None
    identity = process(executable=actual_executable)
    metadata = archive_metadata(actual_tool, actual_inputs, actual_output)
    evidence = {
        "events": [event(identity=copy.deepcopy(identity), archive=metadata)],
        "alerts": [command_alert(identity=copy.deepcopy(identity))],
        "outbox": {ALERT_ID: 1010},
        "notifications": [{
            "alert_id": ALERT_ID,
            "observed_timestamp_ms": 1020,
            "outcome": "sent",
        }],
        "health": [],
    }
    return case, evidence


def unrelated_case():
    return VALIDATOR.Case(
        tool="zip",
        mode="unrelated",
        argv=[],
        cwd=UNRELATED,
        project_root=PROJECT,
        inputs=(UNRELATED / "source one.bin",),
        outputs=(UNRELATED / ".archives" / "outside.zip",),
        positive=False,
        fence_path=FENCE,
    )


def unrelated_fixture():
    case = unrelated_case()
    evidence = {
        "events": [],
        "alerts": [],
        "outbox": {},
        "notifications": [],
        "health": [],
    }
    execution = {
        "pids": {PID},
        "return_code": 0,
        "failure_code": None,
        "operation_verified": True,
        "sidecar_execs": [sidecar_exec()],
        "main_exec_receipt": main_exec_receipt(),
        "main_exec_processed_before_fence": True,
    }
    barrier = {"pid": 41002, "pid_version": 2, "crossed": True}
    return case, execution, evidence, barrier


def reverse_fixture():
    case = unrelated_case()
    case.mode = "reverse_list"
    case.project_root = PROJECT
    case.cwd = PROJECT
    case.inputs = (OUTPUT,)
    case.outputs = ()
    evidence = {
        "events": [],
        "alerts": [],
        "outbox": {},
        "notifications": [],
        "health": [],
    }
    execution = {
        "pids": {PID},
        "return_code": 0,
        "failure_code": None,
        "operation_verified": True,
        "sidecar_execs": [sidecar_exec()],
        "main_exec_receipt": main_exec_receipt(),
        "main_exec_processed_before_fence": True,
    }
    barrier = {"pid": 41002, "pid_version": 2, "crossed": True}
    return case, execution, evidence, barrier


class ArchiveValidationSelfTest(unittest.TestCase):
    def test_positive_sidecar_is_always_used_for_exact_wrapper_identity(self):
        case = positive_case(mode="stdout", output_path=None)
        observer = mock.Mock()
        execution = {"pids": {PID}, "return_code": 0, "failure_code": None, "stdout_bytes": 1}
        with mock.patch.object(VALIDATOR, "execute", return_value=execution) as execute:
            VALIDATOR.process_case(case, Path("/usr/bin/synthetic"), None, SOURCE_RUN,
                                   SCRIPT_PATH, sidecar=observer)
        self.assertIs(execute.call_args.kwargs["observer"], observer)
        observer.begin_case.assert_called_once_with("zip")

    def test_positive_short_wrapper_uses_exact_sidecar_generation_without_relaxing_main_evidence(self):
        case, evidence = positive_fixture(positive_case(mode="stdout", output_path=None))
        launcher = PID - 1
        execution = {"pids": {launcher}, "sidecar_execs": [sidecar_exec(related_pids=[PID, launcher])]}
        self.assertTrue(VALIDATOR.analyze_positive(case, execution, evidence, SOURCE_RUN)["passed"])
        for replacement in (
            {"target_pid_version": PID_VERSION + 1},
            {"related_pid_chain": [PID, launcher + 9]},
            {"executable": "other"},
        ):
            invalid = copy.deepcopy(execution)
            invalid["sidecar_execs"][0].update(replacement)
            self.assertFalse(VALIDATOR.analyze_positive(case, invalid, evidence, SOURCE_RUN)["passed"])
        for replacement in (
            {"input_paths": [str(UNRELATED / "other.bin")]},
            {"output_path": str(OUTPUT)},
        ):
            invalid = copy.deepcopy(evidence)
            invalid["events"][0]["archive"].update(replacement)
            self.assertFalse(VALIDATOR.analyze_positive(case, execution, invalid, SOURCE_RUN)["passed"])
        late = copy.deepcopy(evidence)
        late["notifications"][0]["observed_timestamp_ms"] = 4001
        self.assertEqual(VALIDATOR.analyze_positive(case, execution, late, SOURCE_RUN)["failure_code"],
                         "notification_latency_over_3000ms")

    def test_real_short_lived_wrapper_child_can_match_after_descendant_polling_misses_it(self):
        with tempfile.TemporaryDirectory(prefix="codeperimeter-short-wrapper-", dir="/private/tmp") as directory:
            metadata = Path(directory) / "identity.json"
            program = ("import json, os, subprocess, sys; from pathlib import Path; "
                       "child = subprocess.Popen(['/usr/bin/true']); child.wait(); "
                       "Path(sys.argv[1]).write_text(json.dumps({'parent': os.getpid(), 'child': child.pid}))")
            observer = VALIDATOR.EsloggerSidecar()
            observer.process = mock.Mock()
            observer.process.poll.return_value = None
            observer.begin_case("zip")
            finish = observer.finish_case

            def observe(pids):
                identity = json.loads(metadata.read_text())
                observer._record_exec({"seq_num": 1, "global_seq_num": 1, "event": {"exec": {"target": {
                    "audit_token": {"pid": identity["child"], "pidversion": PID_VERSION},
                    "ppid": identity["parent"], "executable": {"path": "/usr/bin/zip"},
                }}}})
                return finish(pids, timeout=0)

            with mock.patch.object(VALIDATOR, "descendants", return_value=set()), \
                    mock.patch.object(observer, "finish_case", side_effect=observe):
                execution = VALIDATOR.execute(Path(sys.executable), ["-B", "-c", program, str(metadata)],
                                              Path(directory), observer=observer)
            identity = json.loads(metadata.read_text())
            self.assertEqual(execution["pids"], {identity["parent"]})
            self.assertEqual(execution["sidecar_execs"][0]["related_pid_chain"],
                             [identity["child"], identity["parent"]])
            case, evidence = positive_fixture(positive_case(mode="stdout", output_path=None))
            evidence["events"][0]["process"]["pid"] = identity["child"]
            evidence["alerts"][0]["process"]["pid"] = identity["child"]
            self.assertTrue(VALIDATOR.analyze_positive(case, execution, evidence, SOURCE_RUN)["passed"])
            self.assertFalse(VALIDATOR.analyze_positive(case, {"pids": execution["pids"]},
                                                       evidence, SOURCE_RUN)["passed"])
            for pid in identity.values():
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)

    def test_authorization_expiry_during_execute_still_reaps_owned_child(self):
        processes = []
        launch = VALIDATOR.subprocess.Popen

        def remember(*args, **kwargs):
            child = launch(*args, **kwargs)
            processes.append(child)
            return child

        authorization = VALIDATOR.MVP.SudoAuthorization()
        with mock.patch.object(VALIDATOR.subprocess, "Popen", side_effect=remember), \
                mock.patch.object(VALIDATOR, "descendants", return_value=set()), \
                mock.patch.object(VALIDATOR.MVP, "privileged_command_failure",
                                  return_value="sudo_authorization_required"), \
                self.assertRaisesRegex(RuntimeError, "^sudo_authorization_required$"):
            VALIDATOR.execute(Path("/bin/sleep"), ["1"], Path("/private/tmp"), authorization=authorization)
        self.assertEqual(len(processes), 1)
        self.assertIsNotNone(processes[0].poll())
        with self.assertRaises(ProcessLookupError):
            os.kill(processes[0].pid, 0)
        self.assertEqual(authorization.summary()["failure_counts"], {"sudo_authorization_required": 1})

    def test_case_boundary_authorization_failure_still_attempts_both_exact_cleanups(self):
        with tempfile.TemporaryDirectory(prefix="codeperimeter-authorization-cleanup-", dir="/private/tmp") as directory:
            report = Path(directory)
            project = report / "project"
            project.mkdir()
            binary = report / "synthetic"
            args = mock.Mock(binary=binary, collector_binary=binary, diagnose_source_latency=False)
            versions = {tool: {"version": "1.0", "version_return_code": 0} for tool in VALIDATOR.TOOLS}
            observer = mock.Mock(root_identity_verified=True, failure_codes=set())
            observer.stop.return_value = True
            observer.summary.return_value = {}
            child = mock.Mock(stderr=io.BytesIO(b""))
            child.poll.return_value = None
            bridge = {"pid": PID, "pgid": PID, "collector": str(binary), "source_pids": []}

            def control(_socket, operation, _payload=None):
                if operation == "list_directories":
                    return [{"path": str(project)}]
                return {"collector_run_id": SOURCE_RUN}

            with contextlib.ExitStack() as patches:
                for target, name, kwargs in (
                    (VALIDATOR.MVP, "preflight", {"return_value": {}}),
                    (VALIDATOR.MVP, "privileged_command_failure", {"side_effect": [
                        None, "sudo_authorization_required", "sudo_authorization_required",
                    ]}),
                    (VALIDATOR, "prepare_workspace", {"return_value": (report, project, project, {}, {}, {})}),
                    (VALIDATOR, "all_cases", {"return_value": [positive_case()]}),
                    (VALIDATOR, "EsloggerSidecar", {"return_value": observer}),
                    (VALIDATOR.subprocess, "Popen", {"return_value": child}),
                    (VALIDATOR.MVP, "control", {"side_effect": control}),
                    (VALIDATOR.MVP, "wait_for_host", {}),
                    (VALIDATOR.MVP, "wait_for_bridge", {"return_value": bridge}),
                    (VALIDATOR.threading, "Thread", {}),
                ):
                    patches.enter_context(mock.patch.object(target, name, **kwargs))
                stop = patches.enter_context(mock.patch.object(VALIDATOR.MVP, "stop_root", return_value=True))
                summary = VALIDATOR.real_validation(args, report, {tool: binary for tool in VALIDATOR.TOOLS}, versions)
            observer.stop.assert_called_once()
            stop.assert_called_once_with(bridge, child, {})
            self.assertTrue(summary["root_cleanup_complete"])
            self.assertTrue(summary["sidecar_cleanup_complete"])
            self.assertNotEqual(summary["result"], "real_run_passed")
            self.assertIn("sudo_authorization_required", summary["failure_codes"])
            self.assertEqual(summary["sudo_authorization"]["failure_counts"], {"sudo_authorization_required": 2})

    def test_failed_sidecar_term_does_not_close_live_readers_or_claim_capture_failure(self):
        observer = VALIDATOR.EsloggerSidecar()
        observer.root_identity_verified = True
        observer.eslogger_pid, observer.eslogger_pgid = PID, PID + 1
        observer.failure_codes.add("sidecar_sequence_gap")
        observer.process = mock.Mock(pid=PID)
        observer.process.poll.return_value = None
        observer.process.wait.side_effect = VALIDATOR.subprocess.TimeoutExpired("synthetic", 6)
        observer.process.stdout.close.side_effect = AssertionError("活跃 reader 的 close 会阻塞")
        observer.process.stderr.close.side_effect = AssertionError("活跃 reader 的 close 会阻塞")
        observer.stdout_reader, observer.stderr_reader = mock.Mock(), mock.Mock()
        observer.stdout_reader.is_alive.return_value = True
        observer.stderr_reader.is_alive.return_value = True
        with mock.patch.object(observer, "_verified_processes", return_value=[PID]), \
                mock.patch.object(VALIDATOR.MVP, "process_info", return_value=(0, PID - 1, PID + 1, "/usr/bin/eslogger")), \
                mock.patch.object(VALIDATOR.MVP, "privileged_command_failure", return_value="sudo_authorization_required"):
            self.assertFalse(observer.stop())
        observer.process.stdout.close.assert_not_called()
        observer.process.stderr.close.assert_not_called()
        self.assertEqual(observer.summary()["cleanup_diagnostic_counts"], {
            "term_sudo_authorization_required": 1, "launcher_wait_timeout": 1,
            "stdout_reader_still_running": 1, "stderr_reader_still_running": 1,
            "source_still_running": 1,
        })
        self.assertEqual(observer.failure_codes, {"sidecar_sequence_gap"})

    def test_reader_errors_after_stop_do_not_replace_preexisting_capture_failures(self):
        for stopping in (False, True):
            observer = VALIDATOR.EsloggerSidecar()
            observer.stopping = stopping
            observer.failure_codes.add("sidecar_sequence_gap")
            observer.process = mock.Mock()
            observer.process.poll.return_value = None
            observer.process.stdout.readline.side_effect = OSError("SYNTHETIC_PRIVATE_MARKER")
            observer.process.stderr.read.side_effect = OSError("SYNTHETIC_PRIVATE_MARKER")
            observer._read_stdout()
            observer._read_stderr()
            self.assertEqual("sidecar_stream_failed" in observer.failure_codes, not stopping)
            self.assertIn("sidecar_sequence_gap", observer.failure_codes)
            self.assertNotIn("SYNTHETIC_PRIVATE_MARKER", json.dumps(observer.summary()))

    def test_fence_uses_narrow_sql_and_waits_for_late_readable_evidence(self):
        with tempfile.TemporaryDirectory(prefix="codeperimeter-fence-query-", dir="/private/tmp") as directory:
            database = Path(directory) / "events.sqlite"
            with contextlib.closing(sqlite3.connect(database)) as connection, connection:
                connection.executescript("PRAGMA user_version=3; CREATE TABLE events "
                                         "(source_run_id TEXT,pid INTEGER,file_path TEXT,kind TEXT,event_json TEXT);")
            clock = [0.0]
            inserted = [False]
            arrival = [9]
            fence_event = event("open", identity=process(PID + 20, 23), file={
                "path": str(FENCE), "readable": True, "path_truncated": False,
            }) | {"source_stream": "activity", "source_timestamp_ms": 1000,
                  "received_timestamp_ms": 10000}

            def advance(seconds):
                clock[0] += seconds
                if clock[0] >= arrival[0] and not inserted[0]:
                    with contextlib.closing(sqlite3.connect(database)) as connection, connection:
                        connection.execute("INSERT INTO events VALUES(?,?,?,?,?)", (
                            SOURCE_RUN, PID + 20, str(FENCE), "open", json.dumps(fence_event),
                        ))
                    inserted[0] = True

            worker = mock.Mock(returncode=0, stdout=json.dumps({"pid": PID + 20, "success": True}).encode())
            with mock.patch.object(VALIDATOR.subprocess, "run", return_value=worker), \
                    mock.patch.object(VALIDATOR.MVP, "load_evidence", side_effect=AssertionError("禁止全量轮询")), \
                    mock.patch.object(VALIDATOR.time, "monotonic", side_effect=lambda: clock[0]), \
                    mock.patch.object(VALIDATOR.time, "sleep", side_effect=advance):
                result = VALIDATOR.run_fence(SCRIPT_PATH, FENCE, database, SOURCE_RUN)
            self.assertTrue(result["crossed"])
            self.assertIsNone(result["failure_code"])
            self.assertEqual(result["timeout_seconds"], 60)
            self.assertGreaterEqual(result["wait_duration_ms"], 9000)
            self.assertEqual(result["source_to_receive_ms"], 9000)
            with contextlib.closing(sqlite3.connect(database)) as connection, connection:
                connection.execute("DELETE FROM events")
            clock[0], inserted[0], arrival[0] = 0.0, False, 61
            with mock.patch.object(VALIDATOR.subprocess, "run", return_value=worker), \
                    mock.patch.object(VALIDATOR.time, "monotonic", side_effect=lambda: clock[0]), \
                    mock.patch.object(VALIDATOR.time, "sleep", side_effect=advance):
                result = VALIDATOR.run_fence(SCRIPT_PATH, FENCE, database, SOURCE_RUN)
            self.assertFalse(result["crossed"])
            self.assertEqual(result["failure_code"], "negative_fence_timeout")
            self.assertLess(result["wait_duration_ms"], 60100)

    def test_fence_timeout_and_invalid_evidence_are_distinct_and_anonymous(self):
        with tempfile.TemporaryDirectory(prefix="codeperimeter-fence-invalid-", dir="/private/tmp") as directory:
            database = Path(directory) / "events.sqlite"
            with contextlib.closing(sqlite3.connect(database)) as connection, connection:
                connection.executescript("PRAGMA user_version=3; CREATE TABLE events "
                                         "(source_run_id TEXT,pid INTEGER,file_path TEXT,kind TEXT,event_json TEXT);")
            worker = mock.Mock(returncode=0, stdout=json.dumps({"pid": PID + 20, "success": True}).encode())
            for replacement in (None, ("file", {"readable": False}), ("file", {"path_truncated": True}),
                                ("process", {"pid_version": None}), ("process", {"pid_version": True}),
                                ("event", {"source_stream": "exec"}), ("event", {"global_seq": None}),
                                ("event", {"global_seq": True}), ("event", {"source_run_id": "other-run"})):
                with contextlib.closing(sqlite3.connect(database)) as connection, connection:
                    connection.execute("DELETE FROM events")
                    if replacement:
                        invalid = event("open", identity=process(PID + 20, 23), file={
                            "path": str(FENCE), "readable": True, "path_truncated": False,
                        })
                        container, fields = replacement
                        (invalid if container == "event" else invalid[container]).update(fields)
                        connection.execute("INSERT INTO events VALUES(?,?,?,?,?)", (
                            SOURCE_RUN, PID + 20, str(FENCE), "open", json.dumps(invalid),
                        ))
                with mock.patch.object(VALIDATOR.subprocess, "run", return_value=worker):
                    result = VALIDATOR.run_fence(SCRIPT_PATH, FENCE, database, SOURCE_RUN, timeout=0)
                self.assertFalse(result["crossed"])
                self.assertEqual(result["failure_code"], "negative_fence_invalid" if replacement
                                 else "negative_fence_timeout")
                case, execution, evidence, _ = unrelated_fixture()
                analysis = VALIDATOR.analyze_negative(case, execution, evidence, SOURCE_RUN, result, True)
                summary = VALIDATOR.case_summary(case, execution, analysis, "synthetic-version")
                self.assertEqual(summary["fence_failure_code"], result["failure_code"])
                self.assertNotIn(str(FENCE), json.dumps(summary))
                self.assertNotIn(str(PID + 20), json.dumps(summary))

    def test_fence_closes_database_on_returns_and_exceptions(self):
        connect = sqlite3.connect
        worker = mock.Mock(returncode=0, stdout=json.dumps({"pid": PID + 20, "success": True}).encode())
        fence_event = event("open", identity=process(PID + 20, 23), file={
            "path": str(FENCE), "readable": True, "path_truncated": False,
        })
        with tempfile.TemporaryDirectory(prefix="codeperimeter-fence-close-", dir="/private/tmp") as directory:
            database = Path(directory) / "events.sqlite"
            for scenario in ("success", "timeout", "unsupported_schema", "query_failure", "authorization_failure"):
                with self.subTest(scenario=scenario):
                    with contextlib.closing(connect(database)) as connection, connection:
                        connection.executescript("DROP TABLE IF EXISTS events; PRAGMA user_version=3; "
                                                 "CREATE TABLE events (source_run_id TEXT,pid INTEGER,"
                                                 "file_path TEXT,kind TEXT,event_json TEXT);")
                        if scenario == "success":
                            connection.execute("INSERT INTO events VALUES(?,?,?,?,?)", (
                                SOURCE_RUN, PID + 20, str(FENCE), "open", json.dumps(fence_event),
                            ))
                        elif scenario == "unsupported_schema":
                            connection.execute("PRAGMA user_version=2")
                        elif scenario == "query_failure":
                            connection.execute("DROP TABLE events")
                    connections = []

                    def track_connection(*args, **kwargs):
                        opened = connect(*args, **kwargs)
                        connections.append(opened)
                        return opened

                    authorization = mock.Mock() if scenario == "authorization_failure" else None
                    if authorization:
                        authorization.require.side_effect = RuntimeError("synthetic_authorization_failure")
                    with mock.patch.object(VALIDATOR.subprocess, "run", return_value=worker), \
                            mock.patch.object(VALIDATOR.sqlite3, "connect", side_effect=track_connection):
                        if authorization:
                            with self.assertRaises(RuntimeError):
                                VALIDATOR.run_fence(SCRIPT_PATH, FENCE, database, SOURCE_RUN,
                                                    timeout=0, authorization=authorization)
                        else:
                            result = VALIDATOR.run_fence(SCRIPT_PATH, FENCE, database, SOURCE_RUN, timeout=0.1)
                            expected = {"success": None, "timeout": "negative_fence_timeout",
                                        "unsupported_schema": "negative_fence_query_failed",
                                        "query_failure": "negative_fence_query_failed"}[scenario]
                            self.assertEqual(result["failure_code"], expected)
                    self.assertEqual(len(connections), 1)
                    try:
                        with self.assertRaises(sqlite3.ProgrammingError):
                            connections[0].execute("SELECT 1")
                    finally:
                        connections[0].close()

    def test_negative_requires_exact_main_exec_receipt_before_the_file_fence(self):
        case, execution, evidence, barrier = unrelated_fixture()
        for field, value in (("main_exec_receipt", None),
                             ("main_exec_processed_before_fence", False),
                             ("main_exec_processed_before_fence", "SYNTHETIC_PRIVATE_MARKER")):
            wrong = dict(execution, **{field: value})
            result = VALIDATOR.analyze_negative(case, wrong, evidence, SOURCE_RUN, barrier, True)
            self.assertFalse(result["passed"])
            self.assertEqual(result["failure_code"], "main_exec_receipt_missing")
        for field, value in (("run_id", "synthetic-other-run"), ("pid", PID + 1),
                             ("pid_version", PID_VERSION + 1), ("source_stream", "activity"),
                             ("source_stream", None), ("pid", True), ("pid_version", True)):
            wrong = copy.deepcopy(execution)
            wrong["main_exec_receipt"][field] = value
            self.assertEqual(VALIDATOR.analyze_negative(
                case, wrong, evidence, SOURCE_RUN, barrier, True
            )["failure_code"], "main_exec_receipt_missing")
        missing_stream = copy.deepcopy(execution)
        missing_stream["main_exec_receipt"].pop("source_stream")
        self.assertEqual(VALIDATOR.analyze_negative(
            case, missing_stream, evidence, SOURCE_RUN, barrier, True
        )["failure_code"], "main_exec_receipt_missing")

    def test_split_stdin_requires_a_persisted_gap_confirmed_before_the_activity_fence(self):
        case, execution, evidence, barrier = unrelated_fixture()
        case.mode = "stdin"
        execution.update(main_exec_receipt=main_exec_receipt("exec"),
                         source_unknown_gap_before_fence=True)
        barrier.update(source_stream="activity", global_seq=1)
        gap = {"component": "eslogger", "code": "archive_input_source_unknown", "source": {
            "run_id": SOURCE_RUN, "pid": PID, "pid_version": PID_VERSION,
            "source_stream": "exec", "global_seq": 200,
        }}
        evidence["health"] = [gap]
        result = VALIDATOR.analyze_negative(case, execution, evidence, SOURCE_RUN, barrier, True)
        self.assertTrue(result["passed"], "不同客户端的序号不构成先后比较")
        self.assertEqual(result["source_unknown_gap_count"], 1)
        wrong_stream = dict(barrier, source_stream="exec")
        self.assertEqual(VALIDATOR.analyze_negative(
            case, execution, evidence, SOURCE_RUN, wrong_stream, True
        )["failure_code"], "negative_fence_missing")
        no_confirmation = dict(execution, source_unknown_gap_before_fence=False)
        self.assertEqual(VALIDATOR.analyze_negative(
            case, no_confirmation, evidence, SOURCE_RUN, barrier, True
        )["failure_code"], "stdin_source_gap_missing")
        for field, value in (("run_id", "synthetic-other-run"), ("pid", PID + 1),
                             ("pid_version", PID_VERSION + 1), ("source_stream", "activity"),
                             ("global_seq", None)):
            wrong = copy.deepcopy(evidence)
            wrong["health"][0]["source"][field] = value
            self.assertEqual(VALIDATOR.analyze_negative(
                case, execution, wrong, SOURCE_RUN, barrier, True
            )["failure_code"], "stdin_source_gap_missing")

    def test_negative_waits_for_main_receipt_and_stdin_persistence_before_starting_the_fence(self):
        case, execution, evidence, barrier = unrelated_fixture()
        case.mode = "stdin"
        execution.pop("main_exec_receipt")
        execution.pop("main_exec_processed_before_fence")
        receipt = main_exec_receipt("exec")
        gap = {"component": "eslogger", "code": "archive_input_source_unknown", "source": {
            **receipt, "global_seq": 200,
        }}
        calls = []

        def request(_socket, operation, payload):
            calls.append("receipt")
            self.assertEqual(operation, "exec_receipt")
            self.assertEqual(payload, {"run_id": SOURCE_RUN, "pid": PID, "pid_version": PID_VERSION})
            return None if len(calls) == 1 else receipt

        def fence(*_, **_kwargs):
            calls.append("fence")
            self.assertIs(execution["main_exec_processed_before_fence"], True)
            self.assertIs(execution["source_unknown_gap_before_fence"], True)
            return barrier

        with mock.patch.object(VALIDATOR, "execute", return_value=execution), \
                mock.patch.object(VALIDATOR, "validate_operation"), \
                mock.patch.object(VALIDATOR.MVP, "control", side_effect=request), \
                mock.patch.object(VALIDATOR.MVP, "load_evidence", side_effect=[
                    evidence, dict(evidence, health=[gap]),
                ]), \
                mock.patch.object(VALIDATOR.time, "sleep"), \
                mock.patch.object(VALIDATOR, "run_fence", side_effect=fence):
            actual, actual_barrier = VALIDATOR.process_case(
                case, Path("/usr/bin/zip"), Path("/anonymous.sqlite"), SOURCE_RUN, SCRIPT_PATH,
                control_socket=Path("/anonymous.sock"),
            )
        self.assertIs(actual, execution)
        self.assertIs(actual_barrier, barrier)
        self.assertEqual(calls, ["receipt", "receipt", "receipt", "fence"])
        summary = VALIDATOR.case_summary(case, execution, {"passed": True}, "1.0")
        self.assertTrue(summary["main_exec_processed_before_fence"])
        self.assertTrue(summary["source_unknown_gap_before_fence"])
        self.assertNotIn("main_exec_receipt", summary)
        self.assertNotIn(SOURCE_RUN, json.dumps(summary))

    def test_missing_main_receipt_never_runs_a_file_fence_or_echoes_control_errors(self):
        case, execution, _, _ = unrelated_fixture()
        execution.pop("main_exec_receipt")
        execution.pop("main_exec_processed_before_fence")
        with mock.patch.object(VALIDATOR.MVP, "control",
                               side_effect=RuntimeError("SYNTHETIC_PRIVATE_MARKER")), \
                mock.patch.object(VALIDATOR.time, "monotonic", side_effect=[0, 0, 9]), \
                mock.patch.object(VALIDATOR.time, "sleep"):
            failure = VALIDATOR.wait_for_negative_main_evidence(
                case, execution, Path("/anonymous.sqlite"), SOURCE_RUN, Path("/anonymous.sock")
            )
        self.assertEqual(failure, "main_exec_receipt_missing")
        self.assertNotIn("SYNTHETIC_PRIVATE_MARKER", json.dumps(execution, default=list))
        with mock.patch.object(VALIDATOR, "execute", return_value=execution), \
                mock.patch.object(VALIDATOR, "validate_operation"), \
                mock.patch.object(VALIDATOR, "wait_for_negative_main_evidence", return_value=failure), \
                mock.patch.object(VALIDATOR, "run_fence") as fence:
            _, barrier = VALIDATOR.process_case(
                case, Path("/usr/bin/zip"), None, SOURCE_RUN, SCRIPT_PATH,
                control_socket=Path("/anonymous.sock"),
            )
        self.assertIsNone(barrier)
        fence.assert_not_called()

    def test_source_latency_utc_parser_keeps_nine_digit_precision_and_rejects_unknown_time(self):
        self.assertEqual(VALIDATOR.parse_utc_timestamp_ns("1970-01-01T00:00:01.123456789Z"),
                         1_123_456_789)
        self.assertEqual(VALIDATOR.parse_utc_timestamp_ns("1970-01-01T00:00:01.1+00:00"),
                         1_100_000_000)
        for value in (None, True, "SYNTHETIC_PRIVATE_TIMESTAMP", "1970-01-01T00:00:01+01:00",
                      "1970-01-01T00:00:01.1234567890Z"):
            self.assertIsNone(VALIDATOR.parse_utc_timestamp_ns(value))

    def test_source_latency_pairing_requires_exact_pid_generation_and_executable(self):
        case, evidence = positive_fixture()
        observed = dict(sidecar_exec(), source_timestamp_ns=1_000_000_000,
                        received_timestamp_ns=1_004_000_000)
        execution = {"pids": {PID}, "sidecar_execs": [observed],
                     "started_timestamp_ns": 999_000_000, "finished_timestamp_ns": 1_002_000_000}
        result = VALIDATOR.source_latency_comparison(case, execution, evidence, SOURCE_RUN)
        self.assertTrue(result["complete"])
        self.assertTrue(result["source_timestamp_match"])
        self.assertEqual(result["main_minus_sidecar_receive_ms"], 1)
        self.assertEqual(result["sidecar_source_to_receive_ms"], 4)
        for field, value in (("target_pid", PID + 1), ("target_pid_version", PID_VERSION + 1),
                             ("executable", "gzip")):
            wrong = copy.deepcopy(execution)
            wrong["sidecar_execs"][0][field] = value
            result = VALIDATOR.source_latency_comparison(case, wrong, evidence, SOURCE_RUN)
            self.assertFalse(result["complete"])
            self.assertEqual(result["failure_code"], "source_latency_exec_pair_missing")
        wrong = dict(execution, sidecar_failure_code="SYNTHETIC_PRIVATE_FAILURE")
        result = VALIDATOR.source_latency_comparison(case, wrong, evidence, SOURCE_RUN)
        self.assertEqual(result["failure_code"], "source_latency_exec_pair_missing")
        self.assertNotIn("SYNTHETIC_PRIVATE_FAILURE", json.dumps(result))
        marker = "SYNTHETIC_PRIVATE_TIMESTAMP"
        for container, field in (("sidecar", "source_timestamp_ns"),
                                 ("sidecar", "received_timestamp_ns"),
                                 ("execution", "started_timestamp_ns"),
                                 ("event", "received_timestamp_ms")):
            for value in (None, True, marker):
                changed = copy.deepcopy(execution)
                changed_evidence = copy.deepcopy(evidence)
                target = (changed["sidecar_execs"][0] if container == "sidecar" else changed
                          if container == "execution" else changed_evidence["events"][0])
                target[field] = value
                result = VALIDATOR.source_latency_comparison(case, changed, changed_evidence, SOURCE_RUN)
                self.assertFalse(result["complete"])
                self.assertEqual(result["failure_code"], "source_latency_timestamp_missing")
                self.assertNotIn(marker, json.dumps(result))

    def test_source_latency_comparison_does_not_relax_main_latency_or_guess_time_window(self):
        case, evidence = positive_fixture()
        evidence["events"][0]["received_timestamp_ms"] = 5000
        evidence["outbox"][ALERT_ID] = 5001
        evidence["notifications"][0]["observed_timestamp_ms"] = 5200
        observed = dict(sidecar_exec(), source_timestamp_ns=1_000_000_000,
                        received_timestamp_ns=1_004_000_000)
        execution = {"pids": {PID}, "sidecar_execs": [observed],
                     "started_timestamp_ns": 999_000_000, "finished_timestamp_ns": 1_002_000_000}
        diagnostic = VALIDATOR.source_latency_comparison(case, execution, evidence, SOURCE_RUN)
        self.assertTrue(diagnostic["complete"])
        self.assertEqual(diagnostic["main_minus_sidecar_receive_ms"], 3996)
        self.assertEqual(VALIDATOR.analyze_positive(case, execution, evidence, SOURCE_RUN)["failure_code"],
                         "generation_latency_over_3000ms")
        execution["started_timestamp_ns"] = 1_001_000_000
        diagnostic = VALIDATOR.source_latency_comparison(case, execution, evidence, SOURCE_RUN)
        self.assertFalse(diagnostic["complete"])
        self.assertFalse(diagnostic["main_source_in_execution_window"])
        self.assertFalse(diagnostic["sidecar_source_in_execution_window"])
        self.assertEqual(diagnostic["failure_code"], "source_latency_execution_window_mismatch")
        execution["started_timestamp_ns"] = 999_000_000
        observed["source_timestamp_ns"] += 2_000_000
        diagnostic = VALIDATOR.source_latency_comparison(case, execution, evidence, SOURCE_RUN)
        self.assertFalse(diagnostic["source_timestamp_match"])
        self.assertFalse(diagnostic["complete"])

    def test_source_latency_subset_and_result_cannot_claim_full_validation(self):
        def build(tool, mode, *_):
            return positive_case(tool=tool, mode=mode)

        with mock.patch.object(VALIDATOR, "build_case", side_effect=build):
            cases = VALIDATOR.all_cases(PROJECT, UNRELATED, {}, {}, {}, dict.fromkeys(VALIDATOR.TOOLS))
        self.assertEqual(len(VALIDATOR.validation_cases(cases, False)), 92)
        subset = VALIDATOR.validation_cases(cases, True)
        self.assertEqual({(case.tool, case.mode) for case in subset}, {
            (tool, mode) for tool in ("ditto", "bzip2", "pbzip2", "zstd")
            for mode in ("create", "stdout")
        })
        self.assertEqual(len(subset), 8)
        self.assertEqual(VALIDATOR.validation_result(True, True), "source_latency_diagnostic_passed")
        self.assertEqual(VALIDATOR.validation_result(True, False),
                         "source_latency_diagnostic_failed_or_partial")
        self.assertEqual(VALIDATOR.validation_result(False, True), "real_run_passed")

    def test_incomplete_source_latency_diagnostic_cannot_mark_the_case_verified(self):
        case, evidence = positive_fixture()
        execution = {"pids": {PID}, "return_code": 0, "failure_code": None}
        result = VALIDATOR.analyze_positive(case, execution, evidence, SOURCE_RUN)
        self.assertTrue(result["passed"])
        result["source_latency_diagnostic"] = {"complete": False,
                                               "failure_code": "source_latency_exec_pair_missing"}
        summary = VALIDATOR.case_summary(case, execution, result, "1.0")
        self.assertFalse(summary["passed"])
        self.assertEqual(summary["failure_code"], "source_latency_exec_pair_missing")
        result.update(passed=False, failure_code="generation_latency_over_3000ms")
        summary = VALIDATOR.case_summary(case, execution, result, "1.0")
        self.assertEqual(summary["failure_code"], "generation_latency_over_3000ms")

    def test_source_latency_startup_failure_keeps_the_four_tool_diagnostic_scope(self):
        with tempfile.TemporaryDirectory(prefix="codeperimeter-diagnostic-selftest-",
                                         dir="/private/tmp") as temporary:
            report = Path(temporary) / "report"
            stdout = io.StringIO()
            with mock.patch.object(sys, "argv", ["validator", "--diagnose-source-latency",
                                                 "--report-dir", str(report)]), \
                    mock.patch.object(VALIDATOR, "discover_tools",
                                      side_effect=RuntimeError("SYNTHETIC_PRIVATE_FAILURE")), \
                    contextlib.redirect_stdout(stdout):
                self.assertEqual(VALIDATOR.main(), 2)
            summary = json.loads((report / "summary.json").read_text())
            self.assertEqual(summary["mode"], "source_latency_diagnostic")
            self.assertEqual(summary["result"], "source_latency_diagnostic_failed_or_partial")
            self.assertEqual({row["tool"] for row in summary["tools"]},
                             set(VALIDATOR.SOURCE_LATENCY_TOOLS))
            self.assertEqual(json.loads(stdout.getvalue())["tool_count"], 4)
            self.assertNotIn("SYNTHETIC_PRIVATE_FAILURE", stdout.getvalue())

    def test_positive_sidecar_timing_is_opt_in_and_only_retains_parsed_numeric_time(self):
        case = positive_case(mode="stdout", output_path=None)
        observer = mock.Mock()
        execution = {"pids": {PID}, "return_code": 0, "failure_code": None, "stdout_bytes": 1}
        for enabled in (False, True):
            observer.reset_mock()
            with mock.patch.object(VALIDATOR, "execute", return_value=dict(execution)) as execute:
                VALIDATOR.process_case(case, Path("/usr/bin/synthetic"), None, SOURCE_RUN,
                                       SCRIPT_PATH, sidecar=observer, observe_positive=enabled)
            self.assertIs(execute.call_args.kwargs["observer"], observer)
            if enabled:
                observer.begin_case.assert_called_once_with("zip", observe_timing=True)
            else:
                observer.begin_case.assert_called_once_with("zip")
        sidecar = VALIDATOR.EsloggerSidecar()
        sidecar.begin_case("zip", observe_timing=True)
        sidecar.register_pids({PID})
        record = {"seq_num": 1, "global_seq_num": 1, "time": "1970-01-01T00:00:01.123456789Z",
                  "event": {"exec": {"target": {
                      "audit_token": {"pid": PID, "pidversion": PID_VERSION}, "ppid": 40000,
                      "executable": {"path": "/usr/bin/zip"},
                  }}}}
        sidecar._record_exec(record, received_timestamp_ns=1_124_000_000)
        candidate = sidecar.active["candidates"][0]
        self.assertEqual(candidate["source_timestamp_ns"], 1_123_456_789)
        self.assertEqual(candidate["received_timestamp_ns"], 1_124_000_000)
        self.assertNotIn(record["time"], json.dumps(candidate))
        sidecar.active["candidates"].clear()
        record.update(seq_num=2, global_seq_num=2, time="SYNTHETIC_PRIVATE_TIMESTAMP")
        sidecar._record_exec(record, received_timestamp_ns=1_124_000_000)
        self.assertIsNone(sidecar.active["candidates"][0]["source_timestamp_ns"])
        self.assertNotIn("SYNTHETIC_PRIVATE_TIMESTAMP", json.dumps(sidecar.active["candidates"]))

    def test_runner_preparation_failures_and_interrupts_do_not_echo_private_arguments(self):
        runner = SCRIPT_PATH.with_name("run-archive-validation.sh").read_text()
        inline = runner.split("<<'PY'\n", 1)[1].split("\nPY\n", 1)[0]
        namespace = {"__name__": "runner_selftest"}
        exec(compile(inline, "synthetic-runner", "exec"), namespace)
        private_path = Path("/private/tmp/SYNTHETIC_PRIVATE_RUNNER/project")
        for error, expected in (
            (OSError("SYNTHETIC_PRIVATE_RUNNER", str(private_path)), 2),
            (VALIDATOR.subprocess.CalledProcessError(7, [str(private_path)]), 2),
            (KeyboardInterrupt(), 130),
        ):
            stderr = io.StringIO()
            with mock.patch.object(namespace["os"].path, "lexists", return_value=False), \
                    mock.patch.object(namespace["subprocess"], "run", side_effect=error), \
                    contextlib.redirect_stderr(stderr):
                self.assertEqual(namespace["prepare_collector"](private_path), expected)
            self.assertNotIn("SYNTHETIC_PRIVATE_RUNNER", stderr.getvalue())
            self.assertNotIn("Traceback", stderr.getvalue())
            self.assertEqual(json.loads(stderr.getvalue())["failure_code"],
                             "interrupted" if expected == 130 else "collector_prepare_failed")
        stderr = io.StringIO()
        with mock.patch.object(namespace["os"].path, "lexists", return_value=True), \
                mock.patch.object(namespace["importlib"].util, "spec_from_file_location",
                                  side_effect=OSError(str(private_path))), \
                contextlib.redirect_stderr(stderr):
            self.assertEqual(namespace["prepare_collector"](private_path), 2)
        self.assertNotIn("SYNTHETIC_PRIVATE_RUNNER", stderr.getvalue())
        validation = mock.Mock()
        validation.trusted_collector.side_effect = RuntimeError(str(private_path))
        stderr = io.StringIO()
        with mock.patch.object(namespace["os"].path, "lexists", return_value=True), \
                mock.patch.object(namespace["importlib"].util, "spec_from_file_location") as spec, \
                mock.patch.object(namespace["importlib"].util, "module_from_spec", return_value=validation), \
                mock.patch.object(namespace["subprocess"], "run") as run, \
                contextlib.redirect_stderr(stderr):
            self.assertEqual(namespace["prepare_collector"](private_path), 2)
            spec.return_value.loader.exec_module.assert_called_once_with(validation)
            validation.trusted_collector.assert_called_once()
            run.assert_not_called()
        self.assertEqual(json.loads(stderr.getvalue())["failure_code"], "collector_untrusted")
        self.assertNotIn("SYNTHETIC_PRIVATE_RUNNER", stderr.getvalue())

    def test_operation_validation_uses_one_failure_contract(self):
        case = positive_case(output_path=None)
        case.stdout_expected = True
        for execution, failure in (
            ({"return_code": 1, "stdout_bytes": 1, "failure_code": None}, "tool_exit_nonzero"),
            ({"return_code": 0, "stdout_bytes": 0, "failure_code": None}, "stdout_empty"),
            ({"return_code": 0, "stdout_bytes": 1, "failure_code": "case_timeout"}, "case_timeout"),
            ({"return_code": 0, "stdout_bytes": 1, "failure_code": None}, None),
        ):
            VALIDATOR.validate_operation(case, execution)
            self.assertEqual(execution["failure_code"], failure)
            self.assertEqual(execution["operation_verified"], failure is None)
        with tempfile.TemporaryDirectory(prefix="codeperimeter-operation-selftest-",
                                         dir="/private/tmp") as temporary:
            root = Path(temporary)
            case = positive_case(output_path=root / "missing.zip")
            execution = {"return_code": 0, "stdout_bytes": 0, "failure_code": None}
            VALIDATOR.validate_operation(case, execution)
            self.assertEqual(execution["failure_code"], "output_missing")
            case.outputs = ()
            case.operation_artifact = root
            execution["failure_code"] = None
            VALIDATOR.validate_operation(case, execution)
            self.assertEqual(execution["failure_code"], "reverse_extract_output_missing")

    def test_exercise_version_failures_cannot_return_success(self):
        for version_result in ((None, None), ("1.0", 7)):
            with tempfile.TemporaryDirectory(prefix="codeperimeter-version-selftest-",
                                             dir="/private/tmp") as temporary:
                report = Path(temporary) / "report"
                summary = {"result": "exercise_completed_not_real_validation", "cases": []}
                with mock.patch.object(sys, "argv", ["validator", "--exercise-only",
                                                     "--report-dir", str(report)]), \
                        mock.patch.object(VALIDATOR, "discover_tools",
                                          return_value={tool: Path("/usr/bin/synthetic")
                                                        for tool in VALIDATOR.TOOLS}), \
                        mock.patch.object(VALIDATOR, "version_probe", return_value=version_result), \
                        mock.patch.object(VALIDATOR, "exercise_only", return_value=summary), \
                        contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(VALIDATOR.main(), 1)
                self.assertEqual(json.loads((report / "summary.json").read_text())["result"],
                                 "exercise_partial")

    def test_unexpected_sidecar_exit_after_the_last_exec_remains_a_failure(self):
        class ExitedObserver:
            stdout = io.BytesIO(b"")
            stderr = io.BytesIO(b"")

            @staticmethod
            def poll():
                return 9

            @staticmethod
            def wait(timeout=None):
                return 9

        observer = VALIDATOR.EsloggerSidecar()
        observer.process = ExitedObserver()
        observer.root_identity_verified = True
        observer._read_stdout()
        observer.stop()
        self.assertIn("sidecar_stream_ended", observer.failure_codes)
        self.assertIn("sidecar_exit_nonzero", observer.failure_codes)
        self.assertTrue(observer.summary()["failure_codes"])

    def test_sidecar_repeated_stop_preserves_cleanup_independently_of_collection_failures(self):
        for cleanup_ok, failures in ((True, {"sidecar_sequence_gap"}), (False, set())):
            observer = VALIDATOR.EsloggerSidecar()
            observer.process = mock.Mock(pid=PID, stdout=None, stderr=None)
            observer.process.poll.return_value = None
            observer.failure_codes = failures.copy()
            with mock.patch.object(observer, "_verified_processes", return_value=[]), \
                    mock.patch.object(VALIDATOR.MVP, "stop_root", return_value=cleanup_ok) as stop:
                self.assertEqual(observer.stop(), cleanup_ok)
                self.assertEqual(observer.stop(), cleanup_ok)
                stop.assert_called_once_with(None, observer.process, observer.cleanup_diagnostic_counts)
            self.assertEqual(observer.failure_codes, failures)

    def test_sidecar_start_preserves_the_authorized_terminal_session(self):
        observer = VALIDATOR.EsloggerSidecar()
        process_mock = mock.Mock(pid=PID)
        process_mock.poll.return_value = None
        with mock.patch.object(VALIDATOR.subprocess, "Popen", return_value=process_mock) as launch, \
                mock.patch.object(VALIDATOR.threading, "Thread"), \
                mock.patch.object(observer, "_verified_processes", return_value=[PID]), \
                mock.patch.object(VALIDATOR.MVP, "process_info",
                                  return_value=(0, PID - 1, os.getpgrp() + 1, "/usr/bin/eslogger")):
            observer.start()
        self.assertEqual(launch.call_args.args[0],
                         ["/usr/bin/sudo", "-n", "/usr/bin/eslogger", "exec"])
        self.assertEqual(launch.call_args.kwargs["stdin"], VALIDATOR.subprocess.DEVNULL)
        self.assertFalse(launch.call_args.kwargs.get("start_new_session", False))
        self.assertEqual(launch.call_args.kwargs["process_group"], 0)
        self.assertTrue(observer.root_identity_verified)
        self.assertTrue(observer.summary()["isolated_process_group"])

    def test_sidecar_rejects_a_shared_process_group_or_missing_root_identity(self):
        for info, expected in (
            ((0, PID - 1, 9000, "/usr/bin/eslogger"), "sidecar_process_group_unverified"),
            (None, "sidecar_root_unverified"),
            ((501, PID - 1, 9001, "/usr/bin/eslogger"), "sidecar_root_unverified"),
            ((0, PID - 1, 9001, "/usr/bin/other"), "sidecar_root_unverified"),
        ):
            observer = VALIDATOR.EsloggerSidecar()
            target = mock.Mock(pid=PID)
            target.poll.return_value = None
            with mock.patch.object(VALIDATOR.subprocess, "Popen", return_value=target), \
                    mock.patch.object(VALIDATOR.threading, "Thread"), \
                    mock.patch.object(observer, "_verified_processes", return_value=[PID]), \
                    mock.patch.object(VALIDATOR.MVP, "process_info", return_value=info), \
                    mock.patch.object(VALIDATOR.os, "getpgrp", return_value=9000), \
                    mock.patch.object(VALIDATOR.subprocess, "run") as control:
                with self.assertRaisesRegex(RuntimeError, "^" + expected + "$"):
                    observer.start()
            control.assert_not_called()
            self.assertFalse(observer.root_identity_verified)
            self.assertFalse(observer.summary()["isolated_process_group"])
            self.assertIn(expected, observer.failure_codes)

    @unittest.skipIf(os.geteuid() == 0, "无特权终端回归要求普通用户运行")
    def test_sidecar_session_inheritance_keeps_a_real_unprivileged_control_terminal(self):
        child, terminal = pty.fork()
        if child == 0:
            try:
                session = os.getsid(0)
                group = os.getpgrp()
                real_popen = VALIDATOR.subprocess.Popen
                probe_code = (
                    "import json, os, sys, time; "
                    "tty = os.open('/dev/tty', os.O_RDONLY); os.close(tty); "
                    "print(json.dumps({'same_session': os.getsid(0) == int(sys.argv[1]), "
                    "'control_terminal': True, 'separate_group': os.getpgrp() == os.getpid() "
                    "and os.getpgrp() != int(sys.argv[2])}), flush=True); time.sleep(0.1)"
                )

                def launch_probe(_command, **kwargs):
                    # 使用旁路的真实启动参数，仅将 sudo 换成无特权终端探针。
                    return real_popen([sys.executable, "-B", "-c", probe_code, str(session), str(group)],
                                      **kwargs)

                observer = VALIDATOR.EsloggerSidecar()
                with mock.patch.object(VALIDATOR.subprocess, "Popen", side_effect=launch_probe), \
                        mock.patch.object(VALIDATOR.threading, "Thread"), \
                        mock.patch.object(observer, "_verified_processes", return_value=[PID]), \
                        mock.patch.object(VALIDATOR.MVP, "process_info",
                                          return_value=(0, PID - 1, group + 1, "/usr/bin/eslogger")):
                    observer.start()
                state = json.loads(observer.process.stdout.read())
                successful = (observer.process.wait(timeout=5) == 0
                              and state == {"same_session": True, "control_terminal": True,
                                            "separate_group": True})
                observer.process.stdout.close()
                observer.process.stderr.close()
            except Exception:
                successful = False
            os._exit(0 if successful else 1)
        deadline = time.monotonic() + 8
        status = None
        try:
            while time.monotonic() < deadline:
                finished, result = os.waitpid(child, os.WNOHANG)
                if finished:
                    status = result
                    break
                ready, _, _ = select.select([terminal], [], [], 0.05)
                if ready:
                    try:
                        os.read(terminal, 4096)
                    except OSError:
                        pass
            if status is None:
                os.kill(child, VALIDATOR.signal.SIGKILL)
                _, status = os.waitpid(child, 0)
        finally:
            os.close(terminal)
        self.assertEqual(os.waitstatus_to_exitcode(status), 0,
                         "旁路启动必须保留 SID／控制终端并隔离 PGID，stdin 仍为 DEVNULL")

    def test_old_python_is_rejected_before_runner_authorization_or_direct_startup(self):
        runner_path = SCRIPT_PATH.with_name("run-archive-validation.sh")
        runner = runner_path.read_text()
        self.assertIn("python_version_unsupported", runner)
        self.assertLess(runner.index("python_version_unsupported"), runner.index("sudo -v"))
        prefix = runner.split("printf '%s\\n' '请输入本机管理员密码", 1)[0]
        with tempfile.TemporaryDirectory(prefix="codeperimeter-version-selftest-",
                                         dir="/private/tmp") as temporary:
            fake_python = Path(temporary) / "python3"
            fake_python.write_text("#!/bin/sh\nexit 2\n")
            fake_python.chmod(0o700)
            environment = dict(os.environ, PATH=temporary + ":/usr/bin:/bin")
            execution = VALIDATOR.subprocess.run(
                ["/bin/sh", "-c", prefix, str(runner_path)], env=environment,
                stdin=VALIDATOR.subprocess.DEVNULL, stdout=VALIDATOR.subprocess.PIPE,
                stderr=VALIDATOR.subprocess.PIPE, check=False, timeout=5,
            )
        self.assertEqual(execution.returncode, 2)
        self.assertIn(b"python_version_unsupported", execution.stderr)
        self.assertNotIn("管理员密码", execution.stdout.decode())
        stderr = io.StringIO()
        with mock.patch.object(sys, "version_info", (3, 10, 0)), \
                contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as rejected:
            exec(compile(SCRIPT_PATH.read_text(), "synthetic-validator", "exec"),
                 {"__name__": "synthetic_validator", "__file__": str(SCRIPT_PATH)})
        self.assertEqual(rejected.exception.code, 2)
        self.assertEqual(json.loads(stderr.getvalue())["failure_code"],
                         "python_version_unsupported")

    def test_descendant_scans_are_limited_but_keep_short_process_identity_and_end_scan(self):
        for lifetime in (0.008, 0.2):
            clock = [0.0]
            scans = []
            target = mock.Mock(pid=PID, stdout=None, stdin=None, returncode=0)
            target.poll.side_effect = lambda: None if clock[0] < lifetime else 0

            def scan(pid):
                scans.append(clock[0])
                return {PID + 1}

            def advance(seconds):
                clock[0] += seconds

            with mock.patch.object(VALIDATOR.subprocess, "Popen", return_value=target), \
                    mock.patch.object(VALIDATOR, "descendants", side_effect=scan), \
                    mock.patch.object(VALIDATOR.time, "monotonic", side_effect=lambda: clock[0]), \
                    mock.patch.object(VALIDATOR.time, "monotonic_ns",
                                      side_effect=lambda: int(clock[0] * 1_000_000_000)), \
                    mock.patch.object(VALIDATOR.time, "sleep", side_effect=advance):
                result = VALIDATOR.execute(Path("/usr/bin/synthetic"), [], PROJECT)
            self.assertEqual(result["pids"], {PID, PID + 1})
            self.assertEqual(scans[0], 0)
            self.assertGreaterEqual(scans[-1], lifetime)
            self.assertLessEqual(len(scans), 2 if lifetime < 0.05 else 6)
            self.assertTrue(all(right - left >= 0.05 - 0.000001
                                for left, right in zip(scans[:-2], scans[1:-1])))
            self.assertLess(result["duration_ms"], lifetime * 1000 + 5)
            self.assertIsNone(result["failure_code"])

    def test_wrapper_negative_keeps_exact_exec_identity_and_parent_association(self):
        case, execution, evidence, barrier = unrelated_fixture()
        launcher = PID - 1
        record = {"seq_num": 1, "global_seq_num": 1, "event": {"exec": {"target": {
            "audit_token": {"pid": PID, "pidversion": PID_VERSION}, "ppid": launcher,
            "executable": {"path": "/usr/bin/zip", "path_truncated": False},
        }}}}
        for parent, passed in ((launcher, True), (launcher - 99, False)):
            observer = VALIDATOR.EsloggerSidecar()
            observer.process = mock.Mock()
            observer.process.poll.return_value = None
            observer.begin_case("zip")
            observer.register_pids({launcher})
            actual = copy.deepcopy(record)
            actual["event"]["exec"]["target"]["ppid"] = parent
            observer._record_exec(actual)
            observation = observer.finish_case({launcher}, timeout=0)
            execution.update(pids={launcher}, sidecar=observation,
                             sidecar_execs=observation["execs"])
            result = VALIDATOR.analyze_negative(
                case, execution, evidence, SOURCE_RUN, barrier, True
            )
            self.assertEqual(result["passed"], passed)
            if passed:
                self.assertEqual(observation["execs"][0]["target_pid_version"], PID_VERSION)
                self.assertEqual(observation["execs"][0]["related_pid_chain"], [PID, launcher])
                with_alert = dict(evidence, alerts=[command_alert()])
                self.assertEqual(VALIDATOR.analyze_negative(
                    case, execution, with_alert, SOURCE_RUN, barrier, True
                )["failure_code"], "unrelated_project_alert")
            else:
                self.assertEqual(result["failure_code"], "unrelated_exec_missing")

    def test_descendant_scan_limit_does_not_disable_owned_process_timeout_cleanup(self):
        result = VALIDATOR.execute(Path("/bin/sleep"), ["1"], Path("/private/tmp"), timeout=0.04)
        self.assertEqual(result["failure_code"], "case_timeout")
        self.assertNotEqual(result["return_code"], 0)
        self.assertLess(result["duration_ms"], 1000)
        for pid in result["pids"]:
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

    def test_sidecar_sequence_diagnostics_are_bounded_numeric_and_preserve_regression_failures(self):
        observer = VALIDATOR.EsloggerSidecar()
        observer._record_exec({"seq_num": 10, "global_seq_num": 100})
        for index in range(1, 11):
            observer._record_exec({"seq_num": 10 + index * 4,
                                   "global_seq_num": 100 + index * 10})
        summary = observer.summary()
        self.assertEqual(summary["sequence_gap_count"], 10)
        self.assertEqual(summary["sequence_missing_total"], 30)
        self.assertEqual(len(summary["sequence_gaps"]), 8)
        self.assertEqual(summary["sequence_gaps"][0], {
            "previous_event_seq": 10, "next_event_seq": 14,
            "previous_global_seq": 100, "next_global_seq": 110, "missing_events": 3,
        })
        self.assertTrue(all(type(value) is int for row in summary["sequence_gaps"]
                            for value in row.values()))
        observer._record_exec({"seq_num": 50, "global_seq_num": 201})
        observer._record_exec({"seq_num": 49, "global_seq_num": 202})
        observer._record_exec({"seq_num": True, "global_seq_num": 203})
        summary = observer.summary()
        self.assertEqual(summary["sequence_gap_count"], 10)
        self.assertEqual(summary["sequence_missing_total"], 30)
        self.assertEqual(summary["sequence_regression_count"], 2)
        self.assertIn("sidecar_sequence_regression", summary["failure_codes"])
        self.assertIn("sidecar_sequence_missing", summary["failure_codes"])

    def test_pipeline_timing_summary_keeps_only_known_numeric_fields(self):
        marker = "SYNTHETIC_PRIVATE_METRIC_MARKER"
        status = {"pipeline_timing": {
            "host_frames": True, "host_processing_total_us": 10,
            "host_processing_max_us": marker, "source_to_collector_receive_max_ms": 4234,
            "argv": marker, "collector": {
                "sampled_timestamp_ms": 1000, "source_lines": 12,
                "source_bytes": 999, "source_send_total_us": 15, "source_send_max_us": False,
                "bridge_line_write_attempts": 12, "bridge_write_total_us": 20,
                "bridge_write_max_us": 7, "pid": PID, "path": marker, "unknown": marker,
            },
        }}
        result = VALIDATOR.numeric_pipeline_timing(status)
        self.assertIsNone(result["host_frames"])
        self.assertEqual(result["host_processing_total_us"], 10)
        self.assertIsNone(result["host_processing_max_us"])
        self.assertEqual(result["source_to_collector_receive_max_ms"], 4234)
        self.assertEqual(result["collector"]["source_lines"], 12)
        self.assertIsNone(result["collector"]["source_send_max_us"])
        self.assertNotIn("pid", result["collector"])
        self.assertNotIn(marker, json.dumps(result))
        self.assertIsNone(VALIDATOR.numeric_pipeline_timing({}))
        self.assertIsNone(VALIDATOR.numeric_pipeline_timing({"pipeline_timing": marker}))
        status.update(database_state="ready", collector_state="connected", reader_dropped_frames=1)
        self.assertEqual(VALIDATOR.healthy_status(status, {"health": []})[:2],
                         (False, "collector_dropped_events"))

    def test_positive_latency_breakdown_preserves_the_source_based_three_second_gate(self):
        case, evidence = positive_fixture()
        execution = {"pids": {PID}, "return_code": 0, "duration_ms": 10}
        result = VALIDATOR.analyze_positive(case, execution, evidence, SOURCE_RUN)
        self.assertTrue(result["passed"])
        self.assertEqual(result["source_to_receive_ms"], 5)
        self.assertEqual(result["receive_to_outbox_ms"], 5)
        self.assertEqual(result["outbox_to_sent_ms"], 10)
        summary = VALIDATOR.case_summary(case, execution, result, "synthetic-version")
        self.assertEqual(summary["source_to_receive_ms"], 5)
        delayed = copy.deepcopy(evidence)
        delayed["events"][0]["received_timestamp_ms"] = 5000
        delayed["outbox"][ALERT_ID] = 5001
        delayed["notifications"][0]["observed_timestamp_ms"] = 5200
        result = VALIDATOR.analyze_positive(case, execution, delayed, SOURCE_RUN)
        self.assertEqual(result["source_to_receive_ms"], 4000)
        self.assertEqual(result["receive_to_outbox_ms"], 1)
        self.assertEqual(result["outbox_to_sent_ms"], 199)
        self.assertEqual(result["failure_code"], "generation_latency_over_3000ms")
        for received in (None, True, "SYNTHETIC_PRIVATE_METRIC_MARKER"):
            no_received = copy.deepcopy(evidence)
            no_received["events"][0]["received_timestamp_ms"] = received
            result = VALIDATOR.analyze_positive(case, execution, no_received, SOURCE_RUN)
            self.assertTrue(result["passed"])
            self.assertIsNone(result["source_to_receive_ms"])
            self.assertIsNone(result["receive_to_outbox_ms"])
            self.assertEqual(result["outbox_to_sent_ms"], 10)

    def test_sidecar_stderr_classifies_split_sudo_and_es_errors_without_raw_data(self):
        private_marker = b"SYNTHETIC_PRIVATE_PATH_MARKER"

        class ChunkStream:
            def __init__(self, chunks):
                self.chunks = iter(chunks)

            def read(self, size):
                return next(self.chunks, b"")

        scenarios = (
            ([b"sudo: a pass", b"word is requ", b"ired " + private_marker],
             ["sudo_authorization_required"]),
            ([b"ES_NEW_CLIENT_RESULT_ERR_NOT_", b"PERMITTED " + private_marker],
             ["es_client_denied"]),
            ([b"ES_NEW_CLIENT_RESULT_ERR_INTERNAL " + private_marker],
             ["source_startup_failed"]),
            ([b"unexpected " + private_marker], ["unclassified_stderr"]),
            ([], []),
        )
        for chunks, expected in scenarios:
            observer = VALIDATOR.EsloggerSidecar()
            observer.process = mock.Mock(stderr=ChunkStream(chunks))
            observer.process.poll.return_value = 1 if chunks else 0
            observer._read_stderr()
            summary = observer.summary()
            self.assertEqual(summary["diagnostic_codes"], expected)
            self.assertTrue(summary["stderr_complete"])
            self.assertEqual(summary["exit_code"], 1 if chunks else 0)
            self.assertNotIn(private_marker.decode(), json.dumps(summary))
            self.assertNotIn(private_marker, repr(vars(observer)).encode())

    def test_sidecar_failed_start_consumes_stderr_and_prioritizes_sudo_authorization(self):
        for stderr, code in (
            (b"sudo: a password is required\nES_NEW_CLIENT_RESULT_ERR_NOT_PERMITTED\n",
             "sidecar_sudo_authorization_required"),
            (b"ES_NEW_CLIENT_RESULT_ERR_NOT_PERMITTED\n", "sidecar_es_client_denied"),
            (b"SYNTHETIC_PRIVATE_PATH_MARKER\n", "sidecar_root_unverified"),
        ):
            observer = VALIDATOR.EsloggerSidecar()
            process_mock = mock.Mock(pid=PID, stdout=io.BytesIO(b""), stderr=io.BytesIO(stderr))
            process_mock.poll.return_value = 1
            process_mock.wait.return_value = 1
            with mock.patch.object(VALIDATOR.subprocess, "Popen", return_value=process_mock):
                with self.assertRaisesRegex(RuntimeError, "^" + code + "$"):
                    observer.start()
                self.assertTrue(observer.stop())
            summary = observer.summary()
            self.assertTrue(summary["stderr_complete"])
            self.assertIn(code, summary["failure_codes"])
            self.assertEqual(summary["exit_code"], 1)
            self.assertNotIn("SYNTHETIC_PRIVATE_PATH_MARKER", json.dumps(summary))

    def test_stdin_requires_this_main_stream_gap_identity_before_the_fence(self):
        case, execution, evidence, barrier = unrelated_fixture()
        case.mode = "stdin"
        case.cwd = PROJECT
        case.inputs = ()
        execution["source_unknown_gap_before_fence"] = True
        barrier["global_seq"] = 21
        gap = {"component": "eslogger", "code": "archive_input_source_unknown", "source": {
            "run_id": SOURCE_RUN, "pid": PID, "pid_version": PID_VERSION, "global_seq": 20,
        }}
        evidence["health"] = [gap]
        result = VALIDATOR.analyze_negative(case, execution, evidence, SOURCE_RUN, barrier, True)
        self.assertTrue(result["passed"])
        self.assertEqual(result["source_unknown_gap_count"], 1)
        for field, value in (("run_id", "synthetic-other-run"), ("pid", PID + 1),
                             ("pid_version", PID_VERSION + 1), ("global_seq", None),
                             ("global_seq", 21), ("global_seq", 22)):
            wrong = copy.deepcopy(evidence)
            wrong["health"][0]["source"][field] = value
            self.assertEqual(VALIDATOR.analyze_negative(
                case, execution, wrong, SOURCE_RUN, barrier, True
            )["failure_code"], "stdin_source_gap_missing")
        isolated = copy.deepcopy(evidence)
        isolated["health"][0]["source"] = {"run_id": SOURCE_RUN}
        self.assertEqual(VALIDATOR.analyze_negative(
            case, execution, isolated, SOURCE_RUN, barrier, True
        )["failure_code"], "stdin_source_gap_missing")
        no_gap = dict(evidence, health=[])
        self.assertEqual(VALIDATOR.analyze_negative(
            case, execution, no_gap, SOURCE_RUN, barrier, True
        )["failure_code"], "stdin_source_gap_missing")

    def test_supported_stdin_cases_have_bytes_and_no_explicit_project_paths(self):
        self.assertEqual(VALIDATOR.STDIN_TOOLS, {
            "zip", "gzip", "pigz", "bzip2", "pbzip2", "xz", "zstd", "7z", "7zz", "rar",
        })
        for tool in VALIDATOR.STDIN_TOOLS:
            output = UNRELATED / ".archives" / (tool + ".archive")
            case = VALIDATOR.build_case(tool, "stdin", PROJECT, UNRELATED, {},
                                        {(tool, "stdin"): output}, {(tool, "stdin"): FENCE})
            self.assertEqual(case.inputs, ())
            self.assertEqual(case.cwd, PROJECT)
            self.assertFalse(case.positive)
            self.assertIn(VALIDATOR.CONTENT_SENTINEL, case.stdin_payload)
            self.assertNotIn(str(PROJECT), " ".join(case.argv))
            self.assertTrue(all(not VALIDATOR.is_within(path, PROJECT) for path in case.outputs))
        with tempfile.TemporaryDirectory(prefix="codeperimeter-stdin-selftest-",
                                         dir="/private/tmp") as temporary:
            payload = VALIDATOR.CONTENT_SENTINEL + b"\nsynthetic stdin bytes"
            execution = VALIDATOR.execute(Path("/bin/cat"), [], Path(temporary),
                                          capture_stdout=True, stdin_payload=payload)
            self.assertEqual(execution["return_code"], 0)
            self.assertEqual(execution["stdout_bytes"], len(payload))
            self.assertIsNone(execution["failure_code"])

    def test_invalid_arguments_and_interrupts_use_fixed_output(self):
        stderr = io.StringIO()
        with mock.patch.object(sys, "argv", ["validator", "--SYNTHETIC_PRIVATE_UNKNOWN_ARGUMENT"]), \
                contextlib.redirect_stderr(stderr), self.assertRaises(SystemExit) as result:
            VALIDATOR.main()
        self.assertEqual(result.exception.code, 2)
        self.assertEqual(json.loads(stderr.getvalue())["failure_code"], "invalid_arguments")
        self.assertNotIn("SYNTHETIC_PRIVATE_UNKNOWN_ARGUMENT", stderr.getvalue())
        with tempfile.TemporaryDirectory(prefix="codeperimeter-interrupt-selftest-",
                                         dir="/private/tmp") as temporary:
            report = Path(temporary) / "report"
            stdout = io.StringIO()
            with mock.patch.object(sys, "argv", ["validator", "--exercise-only", "--report-dir", str(report)]), \
                    mock.patch.object(VALIDATOR, "discover_tools", side_effect=KeyboardInterrupt()), \
                    contextlib.redirect_stdout(stdout):
                self.assertEqual(VALIDATOR.main(), 130)
            self.assertEqual(json.loads(stdout.getvalue())["failure_codes"], ["interrupted"])
            self.assertNotIn("Traceback", stdout.getvalue())

    def test_positive_requires_a_real_exec_and_exact_archive_metadata(self):
        case, evidence = positive_fixture()
        execution = {"pids": {PID}}
        self.assertTrue(VALIDATOR.analyze_positive(case, execution, evidence, SOURCE_RUN)["passed"])

        no_exec = copy.deepcopy(evidence)
        no_exec["events"] = []
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, no_exec, SOURCE_RUN)["failure_code"],
            "exec_missing",
        )

        wrong_pid = copy.deepcopy(evidence)
        wrong_pid["events"][0]["process"]["pid"] += 1
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, wrong_pid, SOURCE_RUN)["failure_code"],
            "exec_missing",
        )

        wrong_inputs = copy.deepcopy(evidence)
        wrong_inputs["events"][0]["archive"]["input_paths"] = [str(UNRELATED / "stale.bin")]
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, wrong_inputs, SOURCE_RUN)["failure_code"],
            "archive_metadata_mismatch",
        )

    def test_command_alert_must_match_pid_generation_and_project_scope(self):
        case, evidence = positive_fixture()
        execution = {"pids": {PID}}
        wrong_pid = copy.deepcopy(evidence)
        wrong_pid["alerts"][0]["process"]["pid"] += 1
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, wrong_pid, SOURCE_RUN)["failure_code"],
            "archive_command_alert_missing",
        )

        wrong_generation = copy.deepcopy(evidence)
        wrong_generation["alerts"][0]["process"]["pid_version"] += 1
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, wrong_generation, SOURCE_RUN)["failure_code"],
            "archive_command_alert_missing",
        )

        output_only = copy.deepcopy(evidence)
        output_only["alerts"][0]["rule"] = "archive_output"
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, output_only, SOURCE_RUN)["failure_code"],
            "archive_command_alert_missing",
        )

        wrong_root = copy.deepcopy(evidence)
        wrong_root["alerts"][0]["roots"] = [str(UNRELATED)]
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, wrong_root, SOURCE_RUN)["failure_code"],
            "archive_command_alert_wrong_scope",
        )

    def test_stdout_requires_archive_command_evidence_without_a_disk_output(self):
        case = positive_case(mode="stdout", output_path=None)
        case, evidence = positive_fixture(case)
        result = VALIDATOR.analyze_positive(case, {"pids": {PID}}, evidence, SOURCE_RUN)
        self.assertTrue(result["passed"])
        evidence["events"][0]["archive"]["output_path"] = str(OUTPUT)
        self.assertEqual(
            VALIDATOR.analyze_positive(case, {"pids": {PID}}, evidence, SOURCE_RUN)["failure_code"],
            "archive_metadata_mismatch",
        )

    def test_notification_feedback_and_three_second_latency_are_mandatory(self):
        case, evidence = positive_fixture()
        execution = {"pids": {PID}}

        missing_feedback = copy.deepcopy(evidence)
        missing_feedback["notifications"] = []
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, missing_feedback, SOURCE_RUN)["failure_code"],
            "notification_feedback_missing",
        )

        late_generation = copy.deepcopy(evidence)
        late_generation["outbox"][ALERT_ID] = 4001
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, late_generation, SOURCE_RUN)["failure_code"],
            "generation_latency_over_3000ms",
        )

        late_notification = copy.deepcopy(evidence)
        late_notification["notifications"][0]["observed_timestamp_ms"] = 4001
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, late_notification, SOURCE_RUN)["failure_code"],
            "notification_latency_over_3000ms",
        )

        negative_generation = copy.deepcopy(evidence)
        negative_generation["outbox"][ALERT_ID] = 999
        self.assertEqual(
            VALIDATOR.analyze_positive(case, execution, negative_generation, SOURCE_RUN)["failure_code"],
            "generation_latency_negative",
        )

    def test_7zz_wrapper_child_must_keep_matching_generation_and_archive_evidence(self):
        case, evidence = positive_fixture(positive_case(tool="7zz"))
        evidence["events"][0]["archive"] = archive_metadata("7z", INPUT, OUTPUT)
        evidence["events"][0]["process"]["executable"] = "/opt/tools/7z"
        result = VALIDATOR.analyze_positive(case, {"pids": {PID}}, evidence, SOURCE_RUN)
        self.assertTrue(result["passed"])
        self.assertEqual(result["actual_executable"], "7z")

    def test_tar_symlink_identity_keeps_exact_process_metadata_and_gnu_separation(self):
        case, evidence = positive_fixture(positive_case(tool="tar"))
        evidence["events"][0]["process"]["executable"] = "/usr/bin/bsdtar"
        evidence["events"][0]["archive"]["tool"] = "bsdtar"
        result = VALIDATOR.analyze_positive(case, {"pids": {PID}}, evidence, SOURCE_RUN)
        self.assertTrue(result["passed"])
        self.assertEqual(result["actual_executable"], "bsdtar")
        wrong_generation = copy.deepcopy(evidence)
        wrong_generation["alerts"][0]["process"]["pid_version"] += 1
        self.assertEqual(VALIDATOR.analyze_positive(
            case, {"pids": {PID}}, wrong_generation, SOURCE_RUN
        )["failure_code"], "archive_command_alert_missing")
        wrong_inputs = copy.deepcopy(evidence)
        wrong_inputs["events"][0]["archive"]["input_paths"] = [str(UNRELATED / "wrong.bin")]
        self.assertEqual(VALIDATOR.analyze_positive(
            case, {"pids": {PID}}, wrong_inputs, SOURCE_RUN
        )["failure_code"], "archive_metadata_mismatch")
        wrong_tool = copy.deepcopy(evidence)
        wrong_tool["events"][0]["process"]["executable"] = "/opt/tools/gtar"
        wrong_tool["events"][0]["archive"]["tool"] = "gtar"
        self.assertFalse(VALIDATOR.analyze_positive(
            case, {"pids": {PID}}, wrong_tool, SOURCE_RUN
        )["passed"])
        self.assertEqual(VALIDATOR.expected_tool_names("gtar"), {"gtar"})
        observer = VALIDATOR.EsloggerSidecar()
        observer.begin_case("tar")
        self.assertEqual(observer.active["aliases"], {"tar", "bsdtar"})
        observer.register_pids({PID})
        observer._record_exec({"seq_num": 1, "global_seq_num": 1, "event": {"exec": {
            "target": {"audit_token": {"pid": PID, "pidversion": PID_VERSION},
                       "ppid": 40000, "executable": {"path": "/usr/bin/bsdtar", "path_truncated": False}},
        }}})
        observer.process = mock.Mock()
        observer.process.poll.return_value = None
        observed = observer.finish_case({PID}, timeout=0)
        self.assertEqual(observed["execs"][0]["executable"], "bsdtar")
        negative_case, execution, negative_evidence, barrier = reverse_fixture()
        negative_case.tool = "tar"
        execution["sidecar_execs"][0]["executable"] = "bsdtar"
        result = VALIDATOR.analyze_negative(
            negative_case, execution, negative_evidence, SOURCE_RUN, barrier, True
        )
        self.assertTrue(result["passed"])
        self.assertEqual(result["actual_executable"], "bsdtar")
        execution["sidecar_execs"][0]["executable"] = "gtar"
        self.assertEqual(VALIDATOR.analyze_negative(
            negative_case, execution, negative_evidence, SOURCE_RUN, barrier, True
        )["failure_code"], "sidecar_exec_identity_missing")

    def test_unrelated_operation_requires_sidecar_identity_and_an_independent_fence(self):
        case, execution, evidence, barrier = unrelated_fixture()
        result = VALIDATOR.analyze_negative(case, execution, evidence, SOURCE_RUN, barrier, True)
        self.assertTrue(result["passed"])
        self.assertTrue(result["barrier_crossed"])

        no_exec = copy.deepcopy(execution)
        no_exec["sidecar_execs"] = []
        self.assertEqual(
            VALIDATOR.analyze_negative(case, no_exec, evidence, SOURCE_RUN, barrier, True)["failure_code"],
            "unrelated_exec_missing",
        )
        wrong_pid = copy.deepcopy(execution)
        wrong_pid["sidecar_execs"] = [sidecar_exec(pid=PID + 9, related_pids=[PID + 9])]
        self.assertEqual(
            VALIDATOR.analyze_negative(case, wrong_pid, evidence, SOURCE_RUN, barrier, True)["failure_code"],
            "sidecar_exec_identity_missing",
        )
        unverified_operation = copy.deepcopy(execution)
        unverified_operation["operation_verified"] = False
        self.assertEqual(
            VALIDATOR.analyze_negative(
                case, unverified_operation, evidence, SOURCE_RUN, barrier, True
            )["failure_code"],
            "output_missing",
        )
        missing_generation = copy.deepcopy(execution)
        missing_generation["sidecar_execs"][0]["target_pid_version"] = None
        self.assertEqual(
            VALIDATOR.analyze_negative(
                case, missing_generation, evidence, SOURCE_RUN, barrier, True
            )["failure_code"],
            "sidecar_exec_identity_missing",
        )
        self.assertEqual(
            VALIDATOR.analyze_negative(case, execution, evidence, SOURCE_RUN, None, True)["failure_code"],
            "negative_fence_missing",
        )

        same_process_fence = {"pid": PID, "pid_version": PID_VERSION, "crossed": True}
        self.assertEqual(
            VALIDATOR.analyze_negative(
                case, execution, evidence, SOURCE_RUN, same_process_fence, True
            )["failure_code"],
            "negative_fence_not_independent",
        )
        self.assertEqual(
            VALIDATOR.analyze_negative(case, execution, evidence, SOURCE_RUN, barrier, False)["failure_code"],
            "negative_fence_unhealthy",
        )

        with_alert = copy.deepcopy(evidence)
        with_alert["alerts"] = [command_alert(identity=process(pid_version=PID_VERSION))]
        self.assertEqual(
            VALIDATOR.analyze_negative(case, execution, with_alert, SOURCE_RUN, barrier, True)["failure_code"],
            "unrelated_project_alert",
        )
        other_project_alert = copy.deepcopy(evidence)
        other_project_alert["alerts"] = [command_alert(
            identity=process(pid_version=PID_VERSION), roots=[UNRELATED]
        )]
        self.assertTrue(
            VALIDATOR.analyze_negative(
                case, execution, other_project_alert, SOURCE_RUN, barrier, True
            )["passed"]
        )
        wrong_generation_alert = copy.deepcopy(evidence)
        wrong_generation_alert["alerts"] = [command_alert(
            identity=process(pid_version=PID_VERSION + 99)
        )]
        self.assertTrue(
            VALIDATOR.analyze_negative(
                case, execution, wrong_generation_alert, SOURCE_RUN, barrier, True
            )["passed"]
        )
        broken_sidecar = copy.deepcopy(execution)
        broken_sidecar["sidecar"] = {"failure_code": "sidecar_sequence_gap"}
        self.assertEqual(
            VALIDATOR.analyze_negative(
                case, broken_sidecar, evidence, SOURCE_RUN, barrier, True
            )["failure_code"],
            "sidecar_sequence_gap",
        )

    def test_reverse_operation_requires_exec_evidence_and_no_project_archive_alert(self):
        case, execution, evidence, barrier = reverse_fixture()
        self.assertTrue(
            VALIDATOR.analyze_negative(case, execution, evidence, SOURCE_RUN, barrier, True)["passed"]
        )
        no_exec = copy.deepcopy(execution)
        no_exec["sidecar_execs"] = []
        self.assertEqual(
            VALIDATOR.analyze_negative(case, no_exec, evidence, SOURCE_RUN, barrier, True)["failure_code"],
            "reverse_exec_missing",
        )
        ambiguous = copy.deepcopy(execution)
        ambiguous["sidecar_execs"].append(sidecar_exec(pid=PID + 1, related_pids=[PID]))
        self.assertEqual(
            VALIDATOR.analyze_negative(case, ambiguous, evidence, SOURCE_RUN, barrier, True)["failure_code"],
            "sidecar_exec_ambiguous",
        )
        with_alert = copy.deepcopy(evidence)
        with_alert["alerts"] = [command_alert(identity=process(pid_version=PID_VERSION))]
        self.assertEqual(
            VALIDATOR.analyze_negative(case, execution, with_alert, SOURCE_RUN, barrier, True)["failure_code"],
            "reverse_archive_command_alert",
        )

    def test_static_archive_parse_gaps_are_separate_from_collector_health(self):
        status = {
            "database_state": "ready",
            "collector_state": "connected",
            "collector_dropped_lines": 0,
            "reader_dropped_frames": 0,
            "database_gap_events": 0,
            "collector_streams": source_health(),
            "collector_schema_version": 1,
            "collector_message_version": 9,
        }
        evidence = {"health": [{
            "component": "eslogger",
            "code": "archive_input_source_unknown",
            "state": "degraded",
        }]}
        healthy, code, issues = VALIDATOR.healthy_status(status, evidence)
        self.assertTrue(healthy)
        self.assertIsNone(code)
        self.assertEqual(issues, {"archive_input_source_unknown": 1})

        broken_status = dict(status, reader_dropped_frames=1)
        self.assertEqual(
            VALIDATOR.healthy_status(broken_status, evidence)[:2],
            (False, "collector_dropped_events"),
        )

    def test_sidecar_sequence_gaps_fail_and_normal_stop_does_not_fake_stream_end(self):
        sidecar = VALIDATOR.EsloggerSidecar()
        sidecar._record_exec({"seq_num": 8, "global_seq_num": 100})
        sidecar._record_exec({"seq_num": 10, "global_seq_num": 101})
        self.assertIn("sidecar_sequence_gap", sidecar.failure_codes)

        class RunningProcess:
            stdout = io.BytesIO(b"")

            @staticmethod
            def poll():
                return None

        normal_stop = VALIDATOR.EsloggerSidecar()
        normal_stop.process = RunningProcess()
        normal_stop.stopping = True
        normal_stop._read_stdout()
        self.assertNotIn("sidecar_stream_ended", normal_stop.failure_codes)

    def test_compressor_outputs_cover_each_input(self):
        inputs = (Path("/private/tmp/one.txt"), Path("/private/tmp/two.txt"))
        outputs = VALIDATOR.expected_outputs("gzip", inputs, inputs[0].with_suffix(".gz"))
        self.assertEqual(
            outputs,
            (Path("/private/tmp/one.txt.gz"), Path("/private/tmp/two.txt.gz")),
        )

    def test_sidecar_accepts_verified_eslogger_when_sudo_execs_in_place(self):
        observer = VALIDATOR.EsloggerSidecar()
        observer.process = mock.Mock(pid=PID)
        info = (0, 41000, PID, "/usr/bin/eslogger")
        with mock.patch.object(VALIDATOR.MVP, "children", return_value=[]), \
                mock.patch.object(VALIDATOR.MVP, "process_info", return_value=info):
            self.assertEqual(observer._verified_processes(), [PID])
        for invalid in ((501, 41000, PID, "/usr/bin/eslogger"),
                        (0, 41000, PID, "/usr/bin/other")):
            with mock.patch.object(VALIDATOR.MVP, "children", return_value=[]), \
                    mock.patch.object(VALIDATOR.MVP, "process_info", return_value=invalid):
                self.assertEqual(observer._verified_processes(), [])

    def test_unverified_sidecar_stop_uses_this_sudo_launcher(self):
        observer = VALIDATOR.EsloggerSidecar()
        observer.process = mock.Mock(pid=PID, stdout=None, stderr=None)
        observer.process.poll.return_value = None
        observer.process.terminate.side_effect = AssertionError("普通用户不能直接终止 root sudo")
        with mock.patch.object(observer, "_verified_processes", return_value=[]), \
                mock.patch.object(VALIDATOR.MVP, "stop_root", return_value=True) as stop:
            self.assertTrue(observer.stop())
            stop.assert_called_once_with(None, observer.process, observer.cleanup_diagnostic_counts)
        observer.process.terminate.assert_not_called()

    def test_report_directory_inside_repository_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "report_dir_inside_repository"):
            VALIDATOR.checked_report_path(VALIDATOR.REPO / ".scratch" / "forbidden-report")

        with tempfile.TemporaryDirectory(prefix="codeperimeter-archive-selftest-",
                                         dir="/private/tmp") as temporary:
            report = Path(temporary) / "outside-report"
            self.assertEqual(VALIDATOR.checked_report_path(report), report.resolve())


if __name__ == "__main__":
    unittest.main(verbosity=2)
