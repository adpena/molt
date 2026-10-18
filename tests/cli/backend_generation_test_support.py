"""Publish backend generations through the CLI's own publication code.

Tests that need an admitted backend publish its bytes as a real generation
and select it with a real receipt, the way ``_ensure_backend_binary`` does.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from molt.cli import backend_binary
from molt.cli.backend_generation import publish_backend_generation
from molt.toolchain_identity import stable_regular_file_identity


def publish_backend(
    coordinate: Path,
    data: bytes,
    *,
    receipt: Path | None = None,
    fingerprint: dict[str, Any] | None = None,
) -> Path:
    """Publish ``data`` as a generation of ``coordinate`` and return its path.

    With ``receipt`` and ``fingerprint`` the generation is also selected.
    """
    source = coordinate.with_name(f".source-{coordinate.name}")
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_bytes(data)
    source.chmod(0o755)
    observed = stable_regular_file_identity(source, label="test backend source")
    generation = publish_backend_generation(coordinate, source, observed=observed)
    source.unlink()
    if receipt is not None:
        assert fingerprint is not None
        backend_binary._select_backend_generation(
            receipt, generation, fingerprint=fingerprint
        )
    return generation.path


def selected_backend(project_root: Path, coordinate: Path, cargo_profile: str) -> Path:
    """The generation the coordinate's receipt selects; fails when none is."""
    from molt.cli.backend_generation import selected_backend_generation
    from molt.cli.runtime_fingerprints import _read_runtime_fingerprint

    generation = selected_backend_generation(
        coordinate,
        _read_runtime_fingerprint(
            backend_binary._backend_fingerprint_path(
                project_root, coordinate, cargo_profile
            )
        ),
    )
    assert generation is not None, f"no generation selected for {coordinate}"
    return generation
