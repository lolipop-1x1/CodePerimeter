#!/usr/bin/env python3
"""验收判定器的匿名自测；不启动采集，不能作为真实 ES 或通知验收。"""
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import stat
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
