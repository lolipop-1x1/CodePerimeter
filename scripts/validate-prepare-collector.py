#!/usr/bin/env python3
"""显式准备无服务安装的验收副本；默认拒绝覆盖，替换必须指定已知旧SHA256。"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
from pathlib import Path
import stat
import subprocess
import sys
import uuid

REPO = Path(__file__).resolve().parent.parent


# root 只执行内嵌固定代码；隔离系统 Python 不加载用户模块或脚本。
ROOT_REPLACE_CODE = r"""
import hashlib
import os
from pathlib import Path
import pwd
import re
import stat
import subprocess
import sys

INSTALL_ROOT = Path("/Library/CodePerimeter")


def check_chain(path):
    for item in [*reversed(path.parents), path]:
        value = item.lstat()
        if stat.S_ISLNK(value.st_mode) or value.st_uid != 0 or value.st_mode & 0o022:
            raise RuntimeError("替换路径必须root拥有且不可写、不可为链接")
        acl = subprocess.run(["/bin/ls", "-lde", str(item)], capture_output=True, text=True, check=True).stdout
        if any(" allow " in row and any(right in row for right in ("write", "delete", "add_file", "add_subdirectory", "chown")) for row in acl.splitlines()[1:]):
            raise RuntimeError("替换路径存在额外写ACL")


def ensure_quiet(uid, directory):
    home = Path(pwd.getpwuid(uid).pw_dir)
    endpoints = (directory / "run/collector.sock", Path("/var/run/codeperimeter-{}".format(uid)) / "collector.sock",
                 home / "Library/Application Support/CodePerimeter/host.sock")
    if any(os.path.lexists(path) for path in endpoints):
        raise RuntimeError("本服务新旧端点或控制端点已存在；拒绝替换，不删除端点")
    for role in ("collector", "daemon", "notify"):
        label = "com.codeperimeter.{}.{}".format(role, uid)
        is_agent = role == "notify"
        plist = home / "Library/LaunchAgents" / (label + ".plist") if is_agent else Path("/Library/LaunchDaemons") / (label + ".plist")
        domain = "gui/{}".format(uid) if is_agent else "system"
        if os.path.lexists(plist) or subprocess.run(["/bin/launchctl", "print", domain + "/" + label], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
            raise RuntimeError("已安装或加载本账户服务；此入口只替换无launchd安装的验收副本")
    rows = subprocess.run(["/bin/ps", "-axo", "comm="], capture_output=True, text=True, check=True).stdout.splitlines()
    if any(Path(row.strip()).name == "codeperimeter" for row in rows):
        raise RuntimeError("仍有codeperimeter进程；拒绝替换活动二进制")


def replace(destination, temporary, expected_old, expected_new, uid):
    directory = INSTALL_ROOT / str(uid)
    if os.geteuid() != 0 or uid <= 0 or destination != directory / "codeperimeter" or temporary.parent != directory or not re.fullmatch(r"\.codeperimeter-prepare-[0-9a-f]{32}", temporary.name):
        raise RuntimeError("替换身份或固定路径不符")
    if not all(re.fullmatch("[0-9a-f]{64}", value) for value in (expected_old, expected_new)):
        raise RuntimeError("替换必须指定完整SHA256")
    for path, expected in ((destination, expected_old), (temporary, expected_new)):
        check_chain(path)
        if not stat.S_ISREG(path.lstat().st_mode) or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise RuntimeError("已有或暂存二进制不符合明确指定的SHA256；未替换")
    ensure_quiet(uid, directory)
    # 安全核验全部通过后原子换入；不 unlink 未知目标或未知端点。
    os.replace(temporary, destination)


if __name__ == "__main__":
    try:
        replace(Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], sys.argv[4], int(sys.argv[5]))
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(2)
"""

REPLACE_GUARDS = {"__name__": "replacement_guards"}
exec(ROOT_REPLACE_CODE, REPLACE_GUARDS)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=REPO / "target/release/codeperimeter")
    parser.add_argument("--replace-sha256", help="仅替换无launchd服务的验收副本，必须明确指定已有副本SHA256")
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
        replacing = os.path.lexists(destination)
        if args.replace_sha256 and not re.fullmatch("[0-9a-f]{64}", args.replace_sha256):
            raise RuntimeError("replace-sha256必须为明确的64位小写旧SHA256")
        if replacing and not args.replace_sha256:
            raise RuntimeError("已有受保护二进制，拒绝覆盖；无服务验收副本可明确指定 --replace-sha256")
        if args.replace_sha256 and not replacing:
            raise RuntimeError("明确指定的旧副本不存在；拒绝替换")
        if replacing:
            validation.trusted_collector(destination)
            if hashlib.sha256(destination.read_bytes()).hexdigest() != args.replace_sha256:
                raise RuntimeError("已有副本与明确指定的旧SHA256不符；拒绝替换")
        REPLACE_GUARDS["ensure_quiet"](os.getuid(), directory)
        if replacing:
            validation.trusted_collector(Path("/usr/bin/python3"))
            subprocess.run(["/usr/bin/python3", "-I", "-B", "-S", "-c", "import hashlib, pwd, subprocess"], check=True)
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
            if replacing:
                # root 发布时再次验证旧hash、完整权限和静止状态，隔离运行固定内嵌代码。
                subprocess.run(["/usr/bin/sudo", "-n", "/usr/bin/python3", "-I", "-B", "-S", "-c", ROOT_REPLACE_CODE,
                                str(destination), str(temporary), args.replace_sha256, expected, str(os.getuid())], check=True)
            else:
                # link 原子且不覆盖；并发准备不能替换已有目标。
                subprocess.run(["/usr/bin/sudo", "-n", "/bin/link", str(temporary), str(destination)], check=True)
        finally:
            if os.path.lexists(temporary):
                subprocess.run(["/usr/bin/sudo", "-n", "/bin/rm", str(temporary)], check=True)
        validation.trusted_collector(destination)
        if hashlib.sha256(destination.read_bytes()).hexdigest() != expected:
            raise RuntimeError("复制后哈希不符；未执行collector，请检查刚创建的副本")
        print(json.dumps({"prepared": str(destination), "sha256": expected,
                          "replaced_sha256": args.replace_sha256 if replacing else None,
                          "launchd_installed": False, "fda_changed": False}, ensure_ascii=False))
        return 0
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(json.dumps({"result": "failed", "message": str(error).replace(str(Path.home()), "<home>")}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
