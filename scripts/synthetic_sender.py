#!/usr/bin/env python3
"""仅执行合成文件操作；输出不是系统事件或监控结果。"""

import argparse
import ast
import hashlib
import io
import json
import mmap
import os
from pathlib import Path
import py_compile
import selectors
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import uuid
import zipfile


MARKER = ".codeperimeter-synthetic.json"
TEMP_MARKER = ".codeperimeter-synthetic-owner"
SCENARIOS = (
    "read", "mmap", "repeat-read", "bulk-read", "tar", "zip",
    "disk-archive", "memory-archive", "preloaded-memory", "search", "index", "build",
)


def emit(phase, **fields):
    record = {
        "source": "synthetic_sender", "schema_version": 1, "phase": phase,
        "timestamp_ns": time.time_ns(), "monotonic_ns": time.monotonic_ns(),
        "pid": os.getpid(), "pgid": os.getpgrp(), **fields,
    }
    print(json.dumps(record, ensure_ascii=False, sort_keys=True), flush=True)


def fingerprint(path):
    value = path.lstat()
    if not stat.S_ISREG(value.st_mode):
        raise ValueError("合成文件不是普通文件，拒绝操作")
    return [value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns]


def sample(index):
    return (
        "# 匿名合成源码，仅用于文件活动验收。\n"
        f"def synthetic_value_{index}():\n    return {index}\n"
        f"synthetic_padding = {'x' * 1024!r}\n"
    ).encode("utf-8")


def source_paths(root, count):
    return [root / "project" / "src" / f"module_{index:03d}.py" for index in range(count)]


def save_manifest(root, manifest):
    temporary = root / (MARKER + ".tmp")
    with temporary.open("x", encoding="utf-8") as stream:
        json.dump(manifest, stream, sort_keys=True)
        stream.write("\n")
    temporary.replace(root / MARKER)


def prepare(root, count=55):
    if not 1 <= count <= 1000:
        raise ValueError("合成文件数量必须在 1 至 1000 之间")
    if root.is_symlink():
        raise ValueError("工作区不能是符号链接")
    root = root.resolve()
    if root.exists() and (not root.is_dir() or any(root.iterdir())):
        raise ValueError("prepare 只接受不存在或为空的合成工作区")
    root.mkdir(parents=True, exist_ok=True)
    for name in ("src", ".archives", ".build"):
        (root / "project" / name).mkdir(parents=True)
    paths = source_paths(root, count)
    for index, path in enumerate(paths):
        with path.open("xb") as stream:
            stream.write(sample(index))
    manifest = {
        "schema_version": 1, "kind": "codeperimeter-synthetic-only", "token": uuid.uuid4().hex,
        "count": count, "source_stats": [fingerprint(path) for path in paths],
        "outputs": [], "temporary_archives": [],
    }
    save_manifest(root, manifest)
    emit("prepared", root=str(root), project_root=str(root / "project"), file_count=count)
    return root


def load_workspace(root):
    if root.is_symlink():
        raise ValueError("工作区不能是符号链接")
    root = root.resolve(strict=True)
    marker = root / MARKER
    if fingerprint(marker)[2] > 1024 * 1024:
        raise ValueError("合成清单超限")
    with marker.open(encoding="utf-8") as stream:
        manifest = json.load(stream)
    if (manifest.get("schema_version") != 1
            or manifest.get("kind") != "codeperimeter-synthetic-only"
            or not isinstance(manifest.get("count"), int)
            or not 1 <= manifest["count"] <= 1000):
        raise ValueError("不是支持的合成工作区")
    if not isinstance(manifest.get("token"), str) or len(manifest["token"]) != 32:
        raise ValueError("合成工作区标识无效")
    if set(path.name for path in root.iterdir()) != {MARKER, "project"}:
        raise ValueError("工作区包含未登记文件，拒绝操作或清理")
    project = root / "project"
    for path in (project, project / "src", project / ".archives", project / ".build"):
        if path.is_symlink() or not path.is_dir():
            raise ValueError("合成目录已被替换，拒绝操作")
    if set(path.name for path in project.iterdir()) != {"src", ".archives", ".build"}:
        raise ValueError("合成项目包含未登记文件")
    paths = source_paths(root, manifest["count"])
    if set(path.name for path in (project / "src").iterdir()) != {path.name for path in paths}:
        raise ValueError("合成源码清单已经变化")
    if [fingerprint(path) for path in paths] != manifest["source_stats"]:
        raise ValueError("合成源码已被修改或替换，拒绝读取")
    registered = set()
    for output in manifest["outputs"]:
        relative = Path(output["path"])
        if (relative.parent not in (Path("project/.archives"), Path("project/.build"))
                or relative.name in ("", ".", "..")):
            raise ValueError("归档输出越过合成目录边界")
        path = root / relative
        registered.add(path)
        if path.exists() or path.is_symlink():
            actual = fingerprint(path)
            if output["stat"] is not None and actual != output["stat"]:
                raise ValueError("登记输出已被修改或替换")
    actual = set((project / ".archives").iterdir()) | set((project / ".build").iterdir())
    if actual - registered:
        raise ValueError("输出目录包含未登记文件")
    return root, manifest, paths


def register_output(root, manifest, scenario, scope, extension):
    name = f"{scenario}-{uuid.uuid4().hex}{extension}"
    if scope == "inside":
        path = root / "project" / ".archives" / name
        manifest["outputs"].append({"path": str(path.relative_to(root)), "stat": None})
    else:
        directory = Path(tempfile.mkdtemp(prefix="codeperimeter-synthetic-archives-"))
        (directory / TEMP_MARKER).write_text(manifest["token"], encoding="ascii")
        directory_stat = directory.stat()
        path = directory / name
        manifest["temporary_archives"].append({
            "directory": str(directory), "file": name,
            "directory_identity": [directory_stat.st_dev, directory_stat.st_ino], "stat": None,
        })
    save_manifest(root, manifest)
    return path


def record_output(root, manifest, path):
    for output in manifest["outputs"]:
        if root / output["path"] == path:
            output["stat"] = fingerprint(path)
    for output in manifest["temporary_archives"]:
        if Path(output["directory"]) / output["file"] == path:
            output["stat"] = fingerprint(path)
    save_manifest(root, manifest)


def read_source(path, index, operation="read"):
    started = time.time_ns()
    with path.open("rb") as stream:
        if operation == "mmap":
            with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as mapping:
                data = mapping[:]
        else:
            data = stream.read()
    if data != sample(index):
        raise ValueError("文件内容与匿名合成样本不符")
    emit("file_operation", operation=operation, path=str(path), unique_file_index=index + 1,
         operation_started_ns=started, operation_finished_ns=time.time_ns())
    return data


def memory_zip(paths, contents):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path, data in zip(paths, contents):
            archive.writestr("src/" + path.name, data)
    payload = buffer.getvalue()
    verify_zip(io.BytesIO(payload), len(paths))
    return payload


def verify_zip(path, count):
    with zipfile.ZipFile(path) as archive:
        members = {name for name in archive.namelist() if not name.endswith("/")}
        expected = {f"src/module_{index:03d}.py" for index in range(count)}
        if members != expected:
            raise ValueError("ZIP 归档成员不符合合成清单")
        for index in range(count):
            if archive.read(f"src/module_{index:03d}.py") != sample(index):
                raise ValueError("ZIP 归档内容校验失败")


def verify_tar(path, count):
    with tarfile.open(path, "r:gz") as archive:
        members = {member.name for member in archive.getmembers() if member.isfile()}
        expected = {f"src/module_{index:03d}.py" for index in range(count)}
        if members != expected:
            raise ValueError("tar 归档成员不符合合成清单")
        for index in range(count):
            with archive.extractfile(f"src/module_{index:03d}.py") as stream:
                if stream.read() != sample(index):
                    raise ValueError("tar 归档内容校验失败")


def wait_release(timeout):
    deadline = time.monotonic() + timeout
    pending = bytearray()
    with selectors.DefaultSelector() as selector:
        selector.register(sys.stdin.fileno(), selectors.EVENT_READ)
        while time.monotonic() < deadline:
            if not selector.select(max(0, deadline - time.monotonic())):
                break
            part = os.read(sys.stdin.fileno(), 128)
            if not part:
                raise ValueError("预加载等待期间 stdin 已关闭")
            pending.extend(part)
            if len(pending) > 128:
                raise ValueError("release 控制消息过长")
            if b"\n" in pending:
                if bytes(pending).strip() != b"release":
                    raise ValueError("预加载控制消息必须是 release")
                return
    raise TimeoutError("等待 release 超时")


def run_scenario(root, scenario, output_scope="inside", repeats=55, release_timeout=60):
    root, manifest, paths = load_workspace(root)
    if scenario not in SCENARIOS:
        raise ValueError("不支持的合成场景")
    if output_scope not in ("inside", "temporary") or not 1 <= repeats <= 10000:
        raise ValueError("场景参数不合法")
    emit("started", scenario=scenario, root=str(root), project_root=str(root / "project"),
         file_count=len(paths), output_scope=output_scope)
    if scenario in ("read", "mmap", "repeat-read"):
        for _ in range(repeats if scenario == "repeat-read" else 1):
            read_source(paths[0], 0, "mmap" if scenario == "mmap" else "read")
    elif scenario == "bulk-read":
        for index, path in enumerate(paths):
            read_source(path, index)
    elif scenario in ("tar", "zip"):
        output = register_output(root, manifest, scenario, output_scope, ".tar.gz" if scenario == "tar" else ".zip")
        tool = "/usr/bin/" + scenario
        tool_environment = None
        if scenario == "tar":
            args = [tool, "-czf", str(output), "-C", str(root / "project"), "src"]
            # 仅对子tar禁用AppleDouble附加成员；不修改父进程环境或放宽归档校验。
            tool_environment = {**os.environ, "COPYFILE_DISABLE": "1"}
        else:
            args = [tool, "-q", "-r", str(output), "src"]
        started = time.time_ns()
        child = subprocess.Popen(args, cwd=root / "project", stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=tool_environment)
        try:
            child_pgid = os.getpgid(child.pid)
        except ProcessLookupError:
            child_pgid = None
        emit("external_started", scenario=scenario, tool=tool, child_pid=child.pid,
             child_pgid=child_pgid, output_path=str(output), operation_started_ns=started)
        try:
            child.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            child.kill()
            child.communicate()
            raise
        if child.returncode:
            raise RuntimeError(f"合成 {scenario} 失败，退出码 {child.returncode}")
        # 不持久化工具输出，归档有效性由成员与内容独立验证。
        (verify_tar if scenario == "tar" else verify_zip)(output, len(paths))
        record_output(root, manifest, output)
        emit("archive_completed", scenario=scenario, output_path=str(output), child_pid=child.pid,
             valid_archive=True, member_count=len(paths), operation_started_ns=started,
             operation_finished_ns=time.time_ns())
    elif scenario == "disk-archive":
        output = register_output(root, manifest, scenario, output_scope, ".zip")
        started = time.time_ns()
        with zipfile.ZipFile(output, "x", compression=zipfile.ZIP_DEFLATED) as archive:
            for index, path in enumerate(paths):
                archive.writestr("src/" + path.name, read_source(path, index))
        verify_zip(output, len(paths))
        record_output(root, manifest, output)
        emit("archive_completed", scenario=scenario, output_path=str(output), valid_archive=True,
             member_count=len(paths), operation_started_ns=started, operation_finished_ns=time.time_ns())
    elif scenario in ("memory-archive", "preloaded-memory"):
        contents = [read_source(path, index) for index, path in enumerate(paths)]
        if scenario == "preloaded-memory":
            emit("ready", scenario=scenario, file_count=len(paths),
                 expected_source_reopens_after_release=0)
            wait_release(release_timeout)
            emit("released", scenario=scenario)
        started = time.time_ns()
        payload = memory_zip(paths, contents)
        emit("memory_archive_completed", scenario=scenario, archive_bytes=len(payload),
             archive_sha256=hashlib.sha256(payload).hexdigest(), member_count=len(paths),
             valid_archive=True, archive_on_disk=False,
             operation_started_ns=started, operation_finished_ns=time.time_ns())
    elif scenario in ("search", "index"):
        count = 0
        for index, path in enumerate(paths):
            contents = read_source(path, index).decode("utf-8")
            if scenario == "search":
                count += contents.count("synthetic_value_")
            else:
                count += sum(isinstance(node, ast.FunctionDef) for node in ast.walk(ast.parse(contents)))
        emit("normal_task_completed", scenario=scenario, matches_or_symbols=count)
    elif scenario == "build":
        for index, path in enumerate(paths):
            output = root / "project" / ".build" / (path.stem + ".pyc")
            if output.exists():
                # 重复构建仅替换此前登记的合成产物。
                output.unlink()
                manifest["outputs"] = [entry for entry in manifest["outputs"] if root / entry["path"] != output]
            manifest["outputs"].append({"path": str(output.relative_to(root)), "stat": None})
            save_manifest(root, manifest)
            started = time.time_ns()
            py_compile.compile(str(path), cfile=str(output), doraise=True)
            record_output(root, manifest, output)
            emit("file_operation", operation="build", path=str(path), unique_file_index=index + 1,
                 operation_started_ns=started, operation_finished_ns=time.time_ns())
        emit("normal_task_completed", scenario=scenario, compiled_files=len(paths))
    emit("completed", scenario=scenario, success=True)


def checked_temporary(output, token):
    directory = Path(output["directory"])
    if (not directory.is_absolute() or directory.is_symlink()
            or directory.parent.resolve() != Path(tempfile.gettempdir()).resolve()
            or not directory.name.startswith("codeperimeter-synthetic-archives-")):
        raise ValueError("临时归档目录边界无效")
    current = directory.stat()
    if [current.st_dev, current.st_ino] != output["directory_identity"]:
        raise ValueError("临时归档目录已被替换")
    marker = directory / TEMP_MARKER
    fingerprint(marker)
    if marker.read_text(encoding="ascii") != token:
        raise ValueError("临时归档所有权不符")
    name = output["file"]
    if Path(name).name != name:
        raise ValueError("临时归档文件路径越界")
    path = directory / name
    if set(item.name for item in directory.iterdir()) - {TEMP_MARKER, name}:
        raise ValueError("临时归档目录包含外部文件，拒绝清理")
    if path.exists() or path.is_symlink():
        actual = fingerprint(path)
        if output["stat"] is not None and actual != output["stat"]:
            raise ValueError("临时归档已被修改或替换")
    return directory, marker, path


def cleanup(root):
    root, manifest, paths = load_workspace(root)
    # 先校验全部临时目录，再删除任何内容，避免部分清理后才发现越界。
    temporary = [checked_temporary(output, manifest["token"]) for output in manifest["temporary_archives"]]
    for directory, marker, path in temporary:
        path.unlink(missing_ok=True)
        marker.unlink()
        directory.rmdir()
    for output in manifest["outputs"]:
        (root / output["path"]).unlink(missing_ok=True)
    for path in paths:
        path.unlink()
    for name in ("src", ".archives", ".build"):
        (root / "project" / name).rmdir()
    (root / "project").rmdir()
    (root / MARKER).unlink()
    root.rmdir()
    emit("cleaned", root=str(root))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare_parser = commands.add_parser("prepare", help="创建匿名合成工作区")
    prepare_parser.add_argument("--root", type=Path, required=True)
    prepare_parser.add_argument("--files", type=int, default=55)
    run_parser = commands.add_parser("run", help="执行独立文件操作场景")
    run_parser.add_argument("--root", type=Path, required=True)
    run_parser.add_argument("--scenario", choices=SCENARIOS, required=True)
    run_parser.add_argument("--output-scope", choices=("inside", "temporary"), default="inside")
    run_parser.add_argument("--repeats", type=int, default=55)
    run_parser.add_argument("--release-timeout", type=float, default=60)
    cleanup_parser = commands.add_parser("cleanup", help="校验后清理自己创建的合成产物")
    cleanup_parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        # eslogger 会排除自己的进程组，发送器默认建立独立会话／进程组。
        if os.getpgrp() != os.getpid():
            os.setsid()
        if args.command == "prepare":
            prepare(args.root, args.files)
        elif args.command == "cleanup":
            cleanup(args.root)
        else:
            if not 0 < args.release_timeout <= 3600:
                raise ValueError("release 超时必须在 0 至 3600 秒之间")
            run_scenario(args.root, args.scenario, args.output_scope, args.repeats, args.release_timeout)
    except (OSError, ValueError, RuntimeError, TimeoutError, KeyError, TypeError, subprocess.SubprocessError) as error:
        emit("error", error_type=type(error).__name__, message=str(error))
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
