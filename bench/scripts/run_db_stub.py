from __future__ import annotations

import os
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import harness_memory_guard  # noqa: E402


def main() -> int:
    env = harness_memory_guard.canonical_harness_env(os.environ, repo_root=ROOT)
    env["PYTHONPATH"] = f"{ROOT / 'src'}:{ROOT / 'demo' / 'django_app'}"
    env["DJANGO_SETTINGS_MODULE"] = "demoapp.settings"
    env["MOLT_WIRE"] = env.get("MOLT_WIRE", "msgpack")
    context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_BENCH",
        env,
        repo_root=ROOT,
    )

    exports = ROOT / "demo" / "molt_worker_app" / "molt_exports.json"
    worker_bin = Path(
        env.get("WORKER_BIN", str(ROOT / "target" / "debug" / "molt-worker"))
    )
    if not worker_bin.exists():
        build = context.run(
            ["cargo", "build", "-p", "molt-worker"],
            cwd=ROOT,
            env=env,
        )
        if build.returncode != 0:
            return build.returncode
        worker_bin = ROOT / "target" / "debug" / "molt-worker"
    env["MOLT_WORKER_CMD"] = (
        f"{worker_bin} --stdio --exports {exports} --compiled-exports {exports}"
    )

    probe = (
        "from django.test import Client\n"
        "client = Client()\n"
        "resp = client.get('/offload_table/?rows=20000')\n"
        "print('offload_table status', resp.status_code)\n"
        "print(resp.json())\n"
    )
    result = context.run(
        [sys.executable, "-c", probe],
        cwd=ROOT,
        env=env,
    )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
