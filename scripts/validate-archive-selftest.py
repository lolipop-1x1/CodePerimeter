#!/usr/bin/env python3
"""对归档验收裁决器执行反误通过自检。"""

import copy
import importlib.util
import io
from pathlib import Path
import sys
import tempfile
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
