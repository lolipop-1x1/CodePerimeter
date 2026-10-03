#!/usr/bin/env python3
"""验收判定器的匿名自测；不启动采集，不能作为真实 ES 或通知验收。"""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("validation", Path(__file__).with_name("validate-mvp.py"))
validation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(validation)
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


if __name__ == "__main__":
    unittest.main(verbosity=2)
