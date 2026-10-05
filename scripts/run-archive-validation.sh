#!/bin/sh
# 只准备受保护的验收副本并运行真实采集，不安装后台服务。
set -eu

TASK_SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TASK_REPO=$(dirname -- "$TASK_SCRIPT_DIR")
cd "$TASK_REPO"
export PYTHONDONTWRITEBYTECODE=1

if [ ! -x target/release/codeperimeter ]; then
    echo '请先执行 cargo build --release --locked。' >&2
    exit 2
fi

/usr/bin/sudo -v
python3 -B - "$TASK_REPO" <<'PY'
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

repo = Path(sys.argv[1])
binary = repo / "target/release/codeperimeter"
destination = Path("/Library/CodePerimeter") / str(os.getuid()) / "codeperimeter"
command = [sys.executable, "-B", str(repo / "scripts/validate-prepare-collector.py"),
           "--binary", str(binary)]
if os.path.lexists(destination):
    spec = importlib.util.spec_from_file_location("validation", repo / "scripts/validate-mvp.py")
    validation = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(validation)
    validation.trusted_collector(destination)
    previous = hashlib.sha256(destination.read_bytes()).hexdigest()
    current = hashlib.sha256(binary.read_bytes()).hexdigest()
    if previous == current:
        print(json.dumps({"collector_already_matches": True, "sha256": current}), flush=True)
        raise SystemExit(0)
    command.extend(["--replace-sha256", previous])
subprocess.run(command, check=True)
PY

exec python3 -B scripts/validate-archive-commands.py --binary target/release/codeperimeter "$@"
