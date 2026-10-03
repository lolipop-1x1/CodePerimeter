#!/usr/bin/env python3
"""显式准备本账户的受保护 collector 副本；拒绝覆盖，不创建 launchd job。"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import uuid

REPO = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=REPO / "target/release/codeperimeter")
    args = parser.parse_args()
    try:
        if sys.platform != "darwin" or os.geteuid() == 0:
            raise RuntimeError("请在 macOS 以普通用户运行，先在自己的终端 sudo -v")
        source = args.binary.absolute()
        value = source.lstat()
        if not stat.S_ISREG(value.st_mode) or value.st_uid not in (0, os.getuid()) or value.st_mode & 0o022:
            raise RuntimeError("来源必须是本账户或root拥有、非组/其他用户可写的普通二进制")
        spec = importlib.util.spec_from_file_location("validation", REPO / "scripts/validate-mvp.py")
        validation = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(validation)
        parent = Path("/Library/CodePerimeter")
        directory = parent / str(os.getuid())
        destination = directory / "codeperimeter"
        if os.path.lexists(destination):
            raise RuntimeError("已有受保护二进制，拒绝覆盖；先检查已有服务/版本并明确处理")
        validation.check_root_chain(Path("/Library"))
        subprocess.run(["/usr/bin/sudo", "-n", "/usr/bin/true"], check=True)
        for path in (parent, directory):
            if path.exists() or path.is_symlink():
                validation.check_root_chain(path)
                if not path.is_dir():
                    raise RuntimeError("受保护安装父路径不是目录")
            else:
                subprocess.run(["/usr/bin/sudo", "-n", "/usr/bin/install", "-d", "-o", "root", "-g", "wheel", "-m", "0755", str(path)], check=True)
                subprocess.run(["/usr/bin/sudo", "-n", "/bin/chmod", "-N", str(path)], check=True)
                validation.check_root_chain(path)
        expected = hashlib.sha256(source.read_bytes()).hexdigest()
        temporary = directory / f".codeperimeter-prepare-{uuid.uuid4().hex}"
        if os.path.lexists(temporary):
            raise RuntimeError("本轮私有暂存目标已存在，拒绝覆盖")
        try:
            subprocess.run(["/usr/bin/sudo", "-n", "/usr/bin/install", "-o", "root", "-g", "wheel", "-m", "0755", str(source), str(temporary)], check=True)
            subprocess.run(["/usr/bin/sudo", "-n", "/bin/chmod", "-N", str(temporary)], check=True)
            validation.trusted_collector(temporary)
            if hashlib.sha256(temporary.read_bytes()).hexdigest() != expected:
                raise RuntimeError("暂存副本哈希不符；未发布或执行 collector")
            # link 调用是原子、不覆盖的；并发准备或安装不能替换已有目标。
            subprocess.run(["/usr/bin/sudo", "-n", "/bin/link", str(temporary), str(destination)], check=True)
        finally:
            if os.path.lexists(temporary):
                subprocess.run(["/usr/bin/sudo", "-n", "/bin/rm", str(temporary)], check=True)
        validation.trusted_collector(destination)
        if hashlib.sha256(destination.read_bytes()).hexdigest() != expected:
            raise RuntimeError("复制后哈希不符；未执行collector，请检查刚创建的副本")
        print(json.dumps({"prepared": str(destination), "sha256": expected,
                          "launchd_installed": False, "fda_changed": False}, ensure_ascii=False))
        return 0
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(json.dumps({"result": "failed", "message": str(error).replace(str(Path.home()), "<home>")}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
