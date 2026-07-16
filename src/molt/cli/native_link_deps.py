from __future__ import annotations

from pathlib import Path
import sys
from typing import Mapping

from molt.cli.native_link_manifest import (
    read_native_link_flags,
)


def _collect_cargo_native_link_deps(
    runtime_lib: Path,
    *,
    target_triple: str | None = None,
    object_format: str,
    source_root: Path,
    source_fingerprint: Mapping[str, object],
) -> list[str]:
    """Load the artifact-bound, order-preserving Cargo native link plan."""
    return read_native_link_flags(
        runtime_lib,
        target_triple=target_triple,
        object_format=object_format,
        source_root=source_root,
        source_fingerprint=source_fingerprint,
    )


def _native_target_is_windows(target_triple: str | None) -> bool:
    triple = (target_triple or "").lower()
    return (
        ("windows" in triple or "msvc" in triple)
        if target_triple
        else sys.platform == "win32"
    )
