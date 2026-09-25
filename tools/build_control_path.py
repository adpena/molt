"""Print the canonical harness build-control root for CI artifact consumers."""

from __future__ import annotations

from collections.abc import Mapping
import os
from pathlib import Path

from molt.backend_daemon_custody import backend_daemon_build_state_root_from_env
from tools.harness_memory_guard import canonical_harness_env

ROOT = Path(__file__).resolve().parents[1]


def build_control_output(environment: Mapping[str, str], *, repo_root: Path) -> str:
    admitted = canonical_harness_env(environment, repo_root=repo_root)
    root = backend_daemon_build_state_root_from_env(admitted, project_root=repo_root)
    return f"root={root}"


def main() -> None:
    print(build_control_output(os.environ, repo_root=ROOT))


if __name__ == "__main__":
    main()
