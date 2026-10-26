"""Print canonical build-control and guard-profile paths for CI consumers."""

from __future__ import annotations

from collections.abc import Mapping
import os
from pathlib import Path

import sys

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.memory_guard_paths import memory_guard_state_root, pytest_guard_summary_dir  # noqa: E402
from molt.build_state_layout import project_build_state_root  # noqa: E402
from molt.dx import proof_scratch_root  # noqa: E402
from tools.harness_memory_guard import (  # noqa: E402
    canonical_harness_env,
    command_profile_incident_dir,
    command_profile_log_path,
)

ROOT = Path(__file__).resolve().parents[1]


def build_control_output(environment: Mapping[str, str], *, repo_root: Path) -> str:
    admitted = canonical_harness_env(environment, repo_root=repo_root)
    root = project_build_state_root(repo_root, admitted)
    profile_log = command_profile_log_path(admitted, repo_root=repo_root)
    guard_state = memory_guard_state_root(repo_root, admitted)
    pytest_state = pytest_guard_summary_dir(repo_root, admitted)
    proof_scratch = proof_scratch_root(repo_root, admitted)
    return (
        f"root={root}\nprofile_log={profile_log}\n"
        f"incident_root={command_profile_incident_dir(profile_log)}\n"
        f"guard_state_root={guard_state}\npytest_guard_root={pytest_state}\n"
        f"proof_scratch_root={proof_scratch}"
    )


def main() -> None:
    print(build_control_output(os.environ, repo_root=ROOT))


if __name__ == "__main__":
    main()
