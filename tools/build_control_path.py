"""Print canonical build-control and guard-profile paths for CI consumers."""

from __future__ import annotations

from collections.abc import Mapping
import os
from pathlib import Path

from molt.backend_daemon_custody import backend_daemon_build_state_root_from_env
from molt.memory_guard_paths import memory_guard_state_root, pytest_guard_summary_dir
from tools.harness_memory_guard import canonical_harness_env, command_profile_log_path

ROOT = Path(__file__).resolve().parents[1]


def build_control_output(environment: Mapping[str, str], *, repo_root: Path) -> str:
    admitted = canonical_harness_env(environment, repo_root=repo_root)
    root = backend_daemon_build_state_root_from_env(admitted, project_root=repo_root)
    profile_log = command_profile_log_path(admitted, repo_root=repo_root)
    guard_state = memory_guard_state_root(repo_root, admitted)
    pytest_state = pytest_guard_summary_dir(repo_root, admitted)
    return (
        f"root={root}\nprofile_log={profile_log}\n"
        f"guard_state_root={guard_state}\npytest_guard_root={pytest_state}"
    )


def main() -> None:
    print(build_control_output(os.environ, repo_root=ROOT))


if __name__ == "__main__":
    main()
