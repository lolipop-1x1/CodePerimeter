"""合成操作端自测；这些结果不证明系统监控成立。"""

import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from contextlib import redirect_stdout
import zipfile


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "synthetic_sender.py"
SPEC = importlib.util.spec_from_file_location("synthetic_sender", SCRIPT)
SENDER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SENDER)


class SyntheticSenderTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="codeperimeter-sender-test-")
        self.root = (Path(self.temporary.name) / "workspace").resolve()
        self.call("prepare")

    def tearDown(self):
        if self.root.exists():
            with redirect_stdout(io.StringIO()):
                SENDER.cleanup(self.root)
        self.temporary.cleanup()

    def call(self, command, *args):
        result = subprocess.run(
            [sys.executable, "-B", str(SCRIPT), command, "--root", str(self.root), *args],
            text=True, capture_output=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        records = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertTrue(all(record["source"] == "synthetic_sender" for record in records))
        self.assertTrue(all(record["pid"] == record["pgid"] for record in records))
        self.assertTrue(all(record["pgid"] != os.getpgrp() for record in records))
        return records

    def run_sample(self, scenario, *args):
        records = self.call("run", "--scenario", scenario, *args)
        self.assertEqual(records[-1]["phase"], "completed")
        self.assertTrue(records[-1]["success"])
        return records

    def test_read_mapping_repeat_and_batch_use_expected_paths(self):
        for scenario, expected_count in (("read", 1), ("mmap", 1), ("repeat-read", 55), ("bulk-read", 55)):
            with self.subTest(scenario=scenario):
                records = self.run_sample(scenario)
                operations = [record for record in records if record["phase"] == "file_operation"]
                self.assertEqual(len(operations), expected_count)
                self.assertEqual(len({record["path"] for record in operations}), 55 if scenario == "bulk-read" else 1)
                self.assertTrue(all(record["operation_started_ns"] <= record["operation_finished_ns"] for record in operations))
                if scenario == "mmap":
                    self.assertEqual(operations[0]["operation"], "mmap")

    def test_external_tar_zip_and_in_process_disk_archives_are_valid(self):
        for scenario in ("tar", "zip", "disk-archive"):
            for scope in ("inside", "temporary"):
                with self.subTest(scenario=scenario, scope=scope):
                    records = self.run_sample(scenario, "--output-scope", scope)
                    completed = next(record for record in records if record["phase"] == "archive_completed")
                    output = Path(completed["output_path"])
                    self.assertTrue(completed["valid_archive"])
                    self.assertEqual(completed["member_count"], 55)
                    self.assertEqual(output.is_relative_to(self.root / "project"), scope == "inside")
                    if scenario == "tar":
                        with tarfile.open(output) as archive:
                            self.assertEqual(len([member for member in archive.getmembers() if member.isfile()]), 55)
                            self.assertEqual(archive.extractfile("src/module_000.py").read(), SENDER.sample(0))
                    else:
                        with zipfile.ZipFile(output) as archive:
                            self.assertEqual(archive.read("src/module_054.py"), SENDER.sample(54))
                    external = [record for record in records if record["phase"] == "external_started"]
                    self.assertEqual(len(external), 0 if scenario == "disk-archive" else 1)
                    if external:
                        self.assertNotEqual(external[0]["child_pid"], external[0]["pid"])

    def test_memory_compression_has_no_disk_archive(self):
        before = set(self.root.rglob("*"))
        records = self.run_sample("memory-archive")
        completed = next(record for record in records if record["phase"] == "memory_archive_completed")
        self.assertFalse(completed["archive_on_disk"])
        self.assertEqual(completed["member_count"], 55)
        self.assertGreater(completed["archive_bytes"], 0)
        self.assertEqual(set(self.root.rglob("*")), before)
        self.assertEqual(len([record for record in records if record["phase"] == "file_operation"]), 55)

    def test_preloaded_release_does_not_reopen_sources(self):
        child = subprocess.Popen(
            [sys.executable, "-B", str(SCRIPT), "run", "--root", str(self.root),
             "--scenario", "preloaded-memory", "--release-timeout", "10"],
            text=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        records = []
        moved = self.root / "temporarily-unavailable-sources"
        try:
            while True:
                line = child.stdout.readline()
                self.assertTrue(line, "发送器没有进入 ready 状态")
                record = json.loads(line)
                records.append(record)
                if record["phase"] == "ready":
                    break
            self.assertEqual(records[-1]["expected_source_reopens_after_release"], 0)
            # ready 以后让原路径不可用；压缩成功证明该阶段未重新打开源码。
            (self.root / "project" / "src").rename(moved)
            remaining, errors = child.communicate("release\n", timeout=10)
            self.assertEqual(child.returncode, 0, errors + remaining)
            after_release = [json.loads(line) for line in remaining.splitlines()]
            self.assertFalse(any(record["phase"] == "file_operation" for record in after_release))
            self.assertTrue(any(record["phase"] == "memory_archive_completed" for record in after_release))
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate()
            if moved.exists():
                moved.rename(self.root / "project" / "src")

    def test_normal_search_index_and_repeated_build_finish(self):
        for scenario in ("search", "index", "build", "build"):
            records = self.run_sample(scenario)
            completed = next(record for record in records if record["phase"] == "normal_task_completed")
            self.assertEqual(completed.get("matches_or_symbols", completed.get("compiled_files")), 55)
        products = list((self.root / "project" / ".build").glob("*.pyc"))
        self.assertEqual(len(products), 55)
        self.assertTrue(all(path.read_bytes().startswith(importlib.util.MAGIC_NUMBER) for path in products))

    def test_prepare_refuses_existing_user_data_and_cleanup_refuses_extra_files(self):
        untouched = Path(self.temporary.name) / "real-project"
        untouched.mkdir()
        private = untouched / "important.txt"
        private.write_text("synthetic stand-in for a user file", encoding="utf-8")
        with redirect_stdout(io.StringIO()), self.assertRaises(ValueError):
            SENDER.prepare(untouched)
        self.assertTrue(private.exists())
        extra = self.root / "project" / "user-added.txt"
        extra.write_text("user-added synthetic stand-in", encoding="utf-8")
        with self.assertRaises(ValueError):
            SENDER.cleanup(self.root)
        self.assertTrue(extra.exists())
        extra.unlink()

    def test_source_replacement_and_symlink_are_rejected(self):
        source = self.root / "project" / "src" / "module_000.py"
        replacement = source.with_suffix(".backup")
        source.rename(replacement)
        source.symlink_to(replacement)
        with self.assertRaises(ValueError):
            SENDER.load_workspace(self.root)
        source.unlink()
        replacement.rename(source)

    def test_temporary_archive_cleanup_stays_within_owned_files(self):
        records = self.run_sample("zip", "--output-scope", "temporary")
        output = Path(next(record["output_path"] for record in records if record["phase"] == "archive_completed"))
        foreign = output.parent / "foreign.txt"
        foreign.write_text("synthetic user-added file", encoding="utf-8")
        with self.assertRaises(ValueError):
            SENDER.cleanup(self.root)
        self.assertTrue(output.exists())
        self.assertTrue(foreign.exists())
        foreign.unlink()
        self.call("cleanup")
        self.assertFalse(output.parent.exists())
        self.assertFalse(self.root.exists())


if __name__ == "__main__":
    unittest.main()
