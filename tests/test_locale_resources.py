"""语言资源契约的负例与新增语言验证，仅使用合成文案。"""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "check_locales", Path(__file__).parent.parent / "scripts/check-locales.py"
)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class LanguageResourceContractTest(unittest.TestCase):
    def test_missing_key_and_parameter_mismatch_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            (root / "web/zh-CN.json").write_text('{"unrelated":"合成"}')
            with self.assertRaisesRegex(ValueError, "键不完整"):
                CHECK.validate(root)
            (root / "web/zh-CN.json").write_text('{"message_other":"合成 {{different}}"}')
            with self.assertRaisesRegex(ValueError, "命名参数不一致"):
                CHECK.validate(root)

    def test_extra_registered_language_uses_the_same_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            registry = json.loads((root / "languages.json").read_text())
            registry.append({"id": "fr", "name": "Synthetic"})
            (root / "languages.json").write_text(json.dumps(registry))
            for domain in ["web", "native"]:
                (root / domain / "fr.json").write_text('{"message_one":"Synthetic {{count}}","message_other":"Synthetic {{count}}"}')
            self.assertEqual(CHECK.validate(root), 6)
            (root / "native/fr.json").unlink()
            with self.assertRaises(OSError):
                CHECK.validate(root)

    def test_duplicate_keys_and_inconsistent_plural_parameters_are_rejected(self):
        with self.assertRaises(ValueError):
            json.loads('{"key":"first","key":"second"}', object_pairs_hook=CHECK.unique_object)
        with self.assertRaisesRegex(ValueError, "复数模板参数不一致"):
            CHECK.contract({"files_one": "{{count}}", "files_other": "{{different}}"})

    def test_source_reference_missing_from_both_languages_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory)
            self.fixture(project / "locales")
            (project / "web/src").mkdir(parents=True)
            (project / "src").mkdir()
            (project / "web/src/example.ts").write_text("t('missing.both.languages')")
            with self.assertRaisesRegex(ValueError, "源码引用缺少"):
                CHECK.validate_source_references(project)

    @staticmethod
    def fixture(root):
        root.mkdir(parents=True, exist_ok=True)
        (root / "languages.json").write_text(json.dumps([
            {"id": "en", "name": "English"},
            {"id": "zh-CN", "name": "简体中文"},
        ]))
        for domain in ["web", "native"]:
            (root / domain).mkdir()
            (root / domain / "en.json").write_text('{"message_one":"Synthetic {{count}}","message_other":"Synthetic {{count}}"}')
            (root / domain / "zh-CN.json").write_text('{"message_other":"合成 {{count}}"}')


if __name__ == "__main__":
    unittest.main()
