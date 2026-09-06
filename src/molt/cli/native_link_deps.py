from __future__ import annotations

from pathlib import Path
import sys
from molt.cli.native_link_manifest import (
    read_native_link_flags,
)
from molt.cli.runtime_build_identity import RuntimeBuildIdentity


def _collect_cargo_native_link_deps(
    runtime_lib: Path,
    *,
    target_triple: str | None = None,
    object_format: str,
    runtime_build_identity: RuntimeBuildIdentity,
) -> list[str]:
    """Load the artifact-bound, order-preserving Cargo native link plan."""
    return read_native_link_flags(
        runtime_lib,
        target_triple=target_triple,
        object_format=object_format,
        runtime_build_identity=runtime_build_identity,
    )


def _native_target_is_windows(target_triple: str | None) -> bool:
    triple = (target_triple or "").lower()
    return (
        ("windows" in triple or "msvc" in triple)
        if target_triple
        else sys.platform == "win32"
    )
