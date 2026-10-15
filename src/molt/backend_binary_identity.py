"""One content identity for a backend compiler executable.

Object caches and daemon selection key on it, and the perf scoreboard records
it for each measured lane, so every consumer must derive the same digest.
"""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path

from molt.exact_json import canonical_json_sha256
from molt.toolchain_identity import executable_content_identity


def backend_binary_digest(identity: Mapping[str, str | int]) -> str:
    """Digest an ``executable_content_identity`` record."""
    return canonical_json_sha256({"schema": 2, "binary": dict(identity)})


def identify_backend_binary(backend_bin: Path) -> str:
    """Bind object-cache and daemon selection to stable executable content."""
    try:
        identity = executable_content_identity(backend_bin, label="backend executable")
    except (OSError, ValueError):
        if not backend_bin.exists():
            return f"missing:{backend_bin.absolute()}"
        raise
    return backend_binary_digest(identity)
