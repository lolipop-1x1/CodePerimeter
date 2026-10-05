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
        "operation_verified": True,
        "sidecar_execs": [sidecar_exec()],
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
        "operation_verified": True,
        "sidecar_execs": [sidecar_exec()],
    }
    barrier = {"pid": 41002, "pid_version": 2, "crossed": True}
    return case, execution, evidence, barrier


class ArchiveValidationSelfTest(unittest.TestCase):
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
                stop.assert_called_once_with(None, observer.process)
            self.assertEqual(observer.failure_codes, failures)

    def test_sidecar_start_preserves_the_authorized_terminal_session(self):
        observer = VALIDATOR.EsloggerSidecar()
        process_mock = mock.Mock(pid=PID)
        process_mock.poll.return_value = None
        with mock.patch.object(VALIDATOR.subprocess, "Popen", return_value=process_mock) as launch, \
                mock.patch.object(VALIDATOR.threading, "Thread"), \
                mock.patch.object(observer, "_verified_processes", return_value=[PID]), \
                mock.patch.object(VALIDATOR.MVP, "process_info",
                                  return_value=(0, PID - 1, PID - 1, "/usr/bin/eslogger")):
            observer.start()
        self.assertEqual(launch.call_args.args[0],
                         ["/usr/bin/sudo", "-n", "/usr/bin/eslogger", "exec"])
        self.assertEqual(launch.call_args.kwargs["stdin"], VALIDATOR.subprocess.DEVNULL)
        self.assertFalse(launch.call_args.kwargs.get("start_new_session", False))
        self.assertTrue(observer.root_identity_verified)

    @unittest.skipIf(os.geteuid() == 0, "无特权终端回归要求普通用户运行")
    def test_sidecar_session_inheritance_keeps_a_real_unprivileged_control_terminal(self):
        child, terminal = pty.fork()
        if child == 0:
            try:
                session = os.getsid(0)
                real_popen = VALIDATOR.subprocess.Popen
                probe_code = (
                    "import json, os, sys, time; "
                    "tty = os.open('/dev/tty', os.O_RDONLY); os.close(tty); "
                    "print(json.dumps({'same_session': os.getsid(0) == int(sys.argv[1]), "
                    "'control_terminal': True}), flush=True); time.sleep(0.1)"
                )

                def launch_probe(_command, **kwargs):
                    # 使用旁路的真实启动参数，仅将 sudo 换成无特权终端探针。
                    return real_popen([sys.executable, "-B", "-c", probe_code, str(session)],
                                      **kwargs)

                observer = VALIDATOR.EsloggerSidecar()
                with mock.patch.object(VALIDATOR.subprocess, "Popen", side_effect=launch_probe), \
                        mock.patch.object(VALIDATOR.threading, "Thread"), \
                        mock.patch.object(observer, "_verified_processes", return_value=[PID]), \
                        mock.patch.object(VALIDATOR.MVP, "process_info",
                                          return_value=(0, PID - 1, PID - 1, "/usr/bin/eslogger")):
                    observer.start()
                state = json.loads(observer.process.stdout.read())
                successful = (observer.process.wait(timeout=5) == 0
                              and state == {"same_session": True, "control_terminal": True})
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
                         "旁路启动必须继承 SID 与控制终端，stdin 仍为 DEVNULL")

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
            stop.assert_called_once_with(None, observer.process)
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
