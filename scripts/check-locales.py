#!/usr/bin/env python3
"""检查离线语言资源的完整句子、复数键和命名参数契约。"""

import json
import re
import sys
from pathlib import Path

PLURAL = re.compile(r"_(zero|one|two|few|many|other)$")
PARAMETER = re.compile(r"\{\{\s*([\w.]+)(?:\s*,[^}]*)?\s*\}\}")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"重复语言键：{key}")
        result[key] = value
    return result


def read_json(path):
    return json.loads(path.read_text(), object_pairs_hook=unique_object)


def contract(catalog):
    groups = {}
    for key, template in catalog.items():
        if not isinstance(template, str) or not template.strip():
            raise ValueError(f"文案必须为非空字符串：{key}")
        base = PLURAL.sub("", key)
        parameters = set(PARAMETER.findall(template))
        if base in groups and groups[base] != parameters:
            raise ValueError(f"复数模板参数不一致：{base}")
        groups[base] = parameters
    return groups


def validate(root):
    registry = read_json(root / "languages.json")
    identifiers = [language["id"] for language in registry]
    if len(identifiers) != len(set(identifiers)) or "en" not in identifiers:
        raise ValueError("语言登记重复或缺少英文回退")
    if not all(re.fullmatch(r"[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8})*", name) for name in identifiers):
        raise ValueError("语言标识无效")
    if not all(isinstance(language.get("name"), str) and language["name"].strip() for language in registry):
        raise ValueError("语言名称不能为空")
    count = 0
    for domain in ["web", "native"]:
        baseline = contract(read_json(root / domain / "en.json"))
        for identifier in identifiers:
            current = contract(read_json(root / domain / f"{identifier}.json"))
            if current.keys() != baseline.keys():
                missing = sorted(baseline.keys() - current.keys())
                extra = sorted(current.keys() - baseline.keys())
                raise ValueError(f"{domain}/{identifier} 键不完整：missing={missing}, extra={extra}")
            inconsistent = [key for key in baseline if baseline[key] != current[key]]
            if inconsistent:
                raise ValueError(f"{domain}/{identifier} 命名参数不一致：{inconsistent}")
            count += len(current)
    return count


def validate_source_references(project):
    root = project / "locales"
    web = contract(read_json(root / "web/en.json"))
    native = contract(read_json(root / "native/en.json"))
    problems = []
    for path in sorted((project / "web/src").glob("*")):
        if path.suffix not in [".ts", ".tsx"]:
            continue
        for match in re.finditer(r"\bt\(\s*['\"]([^'\"]+)['\"]", path.read_text()):
            key = PLURAL.sub("", match.group(1))
            if key not in web:
                problems.append(f"{path.relative_to(project)}: {key}")
    for path in sorted((project / "src").glob("*.rs")):
        production = path.read_text().split("#[cfg(test)]")[0]
        for key in re.findall(r'"((?:cli|notification|native|web)\.[a-zA-Z0-9_.-]+)"', production):
            if PLURAL.sub("", key) not in native:
                problems.append(f"{path.relative_to(project)}: {key}")
    if problems:
        raise ValueError(f"源码引用缺少英文回退资源：{sorted(set(problems))}")


if __name__ == "__main__":
    try:
        project = Path(__file__).resolve().parent.parent
        count = validate(project / "locales")
        validate_source_references(project)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"语言资源检查失败：{error}", file=sys.stderr)
        sys.exit(1)
    print(f"语言资源检查通过：{count} 个语义文案")
