"""Real publication observations of synthetic scoreboard-fixture artifacts.

Only test artifact bytes and selected profiles are observed. Compiler/runtime
identities remain unknown, and no compiled-with or used-byte admission is
claimed. Phase and release tests keep their real fail-closed toolchain checks.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from molt.cli.build_results import _observed_build_toolchain
from tools.perf_scoreboard_build_profiles import profile_selection_for_backend


def write_synthetic_build_observation(
    artifact: Path, *, backend: str, profile: str
) -> dict[str, Any]:
    selection = profile_selection_for_backend(backend, profile)
    artifact.parent.mkdir(parents=True, exist_ok=True)
    artifact.write_bytes(b"synthetic scoreboard artifact; not executable\n")
    return _observed_build_toolchain(
        backend_bin=None,
        runtime_lib=None,
        output=artifact,
        selected_profiles=selection.as_record(),
    )
