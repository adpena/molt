"""Print canonical build-control and guard-profile paths for CI consumers."""

from __future__ import annotations

from collections.abc import Mapping
import os
from pathlib import Path

from molt.backend_daemon_custody import backend_daemon_build_state_root_from_env
from tools.harness_memory_guard import canonical_harness_env, command_profile_log_path

ROOT = Path(__file__).resolve().parents[1]


def build_control_output(environment: Mapping[str, str], *, repo_root: Path) -> str:
    admitted = canonical_harness_env(environment, repo_root=repo_root)
    root = backend_daemon_build_state_root_from_env(admitted, project_root=repo_root)
    profile_log = command_profile_log_path(admitted, repo_root=repo_root)
    return f"root={root}\nprofile_log={profile_log}"


def main() -> None:
    print(build_control_output(os.environ, repo_root=ROOT))


if __name__ == "__main__":
    main()
