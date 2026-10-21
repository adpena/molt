from __future__ import annotations

import os
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[2]

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.dx import project_cargo_target_dir  # noqa: E402
from molt.process_environment import (  # noqa: E402
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)
from tools import harness_memory_guard  # noqa: E402


def _built_worker(env: ProcessEnvironment) -> Path:
    """The `molt-worker` that `cargo build -p molt-worker` writes under ``env``."""
    name = "molt-worker.exe" if os.name == "nt" else "molt-worker"
    return project_cargo_target_dir(ROOT, env) / "debug" / name


def main() -> int:
    env = ProcessEnvironmentBuilder(
        harness_memory_guard.canonical_harness_env(os.environ, repo_root=ROOT)
    )
    env["PYTHONPATH"] = os.pathsep.join(
        [str(ROOT / "src"), str(ROOT / "demo" / "django_app")]
    )
    env["DJANGO_SETTINGS_MODULE"] = "demoapp.settings"
    env.setdefault("MOLT_WORKER_WIRE", "msgpack")
    env = ProcessEnvironment(env)
    context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_BENCH",
        env,
        repo_root=ROOT,
    )

    exports = ROOT / "demo" / "molt_worker_app" / "molt_exports.json"
    worker_bin = Path(env.get("WORKER_BIN", str(_built_worker(env))))
    if not worker_bin.exists():
        build = context.run(
            ["cargo", "build", "-p", "molt-worker"],
            cwd=ROOT,
            env=env,
        )
        if build.returncode != 0:
            return build.returncode
        worker_bin = _built_worker(env)
    env = env.updated(
        {
            "MOLT_WORKER_CMD": (
                f"{worker_bin} --stdio --exports {exports} --compiled-exports {exports}"
            )
        }
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
