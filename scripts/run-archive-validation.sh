#!/bin/sh
# 只准备受保护的验收副本并运行真实采集，不安装后台服务。
set -eu

TASK_SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TASK_REPO=$(dirname -- "$TASK_SCRIPT_DIR")
cd "$TASK_REPO"
export PYTHONDONTWRITEBYTECODE=1
trap 'printf "%s\n" "{\"result\":\"interrupted\",\"failure_code\":\"interrupted\"}" >&2; exit 130' INT

if [ ! -x target/release/codeperimeter ]; then
    printf '%s\n' '{"result":"failed","failure_code":"binary_missing"}' >&2
    exit 2
fi

printf '%s\n' '请输入本机管理员密码以启动真实采集验收。'
if /usr/bin/sudo -v 2>/dev/null; then
    :
else
    printf '%s\n' '{"result":"failed","failure_code":"sudo_authorization_required"}' >&2
    exit 2
fi
python3 -B - "$TASK_REPO" <<'PY'
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

def prepare_collector(repo):
    failure_code = "collector_prepare_failed"
    try:
        binary = repo / "target/release/codeperimeter"
        destination = Path("/Library/CodePerimeter") / str(os.getuid()) / "codeperimeter"
        command = [sys.executable, "-B", str(repo / "scripts/validate-prepare-collector.py"),
                   "--binary", str(binary)]
        if os.path.lexists(destination):
            spec = importlib.util.spec_from_file_location("validation", repo / "scripts/validate-mvp.py")
            validation = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(validation)
            failure_code = "collector_untrusted"
            validation.trusted_collector(destination)
            failure_code = "collector_prepare_failed"
            previous = hashlib.sha256(destination.read_bytes()).hexdigest()
            current = hashlib.sha256(binary.read_bytes()).hexdigest()
            if previous == current:
                print(json.dumps({"collector_already_matches": True, "sha256": current}), flush=True)
                return 0
            command.extend(["--replace-sha256", previous])
        result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, check=False)
        if result.returncode:
            raise RuntimeError("collector_prepare_failed")
        return 0
    except KeyboardInterrupt:
        print(json.dumps({"result": "interrupted", "failure_code": "interrupted"}), file=sys.stderr)
        return 130
    except Exception:
        print(json.dumps({"result": "failed", "failure_code": failure_code}), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(prepare_collector(Path(sys.argv[1])))
PY

exec python3 -B scripts/validate-archive-commands.py --binary target/release/codeperimeter "$@"
