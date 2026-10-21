"""Publish backend generations through the CLI's own publication code.

Tests that need an admitted backend publish its bytes as a real generation in
the checkout's product store and select it with a real store receipt, the way
``_ensure_backend_binary`` does.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.cli import backend_binary
from molt.cli.backend_generation import publish_backend_generation
from molt.toolchain_identity import stable_regular_file_identity


def stored_backend_coordinate(
    project_root: Path,
    coordinate: Path,
    fingerprint: dict[str, Any],
    cargo_environment: CargoEnvironmentInputs,
) -> Path:
    """The store coordinate of ``coordinate``'s variant for these inputs."""
    store = backend_binary.host_tool_store(project_root)
    identity = backend_binary.host_tool_identity(coordinate, fingerprint)
    return backend_binary.stored_host_tool(
        store, identity, coordinate, cargo_environment
    ).coordinate


def publish_backend(
    coordinate: Path,
    data: bytes,
    *,
    project_root: Path | None = None,
    fingerprint: dict[str, Any] | None = None,
    cargo_environment: CargoEnvironmentInputs | None = None,
) -> Path:
    """Publish ``data`` as a backend generation and return its path.

    With ``project_root``, ``fingerprint`` and ``cargo_environment`` the
    generation is published in that checkout's product store and selected by a
    store receipt under the fingerprint's identity. Without them it is an
    unselected generation of ``coordinate`` that no admission names.
    """
    selected = project_root is not None
    if selected:
        assert fingerprint is not None
        assert cargo_environment is not None
        destination = stored_backend_coordinate(
            project_root, coordinate, fingerprint, cargo_environment
        )
    else:
        destination = coordinate
    source = coordinate.with_name(f".source-{coordinate.name}")
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_bytes(data)
    source.chmod(0o755)
    observed = stable_regular_file_identity(source, label="test backend source")
    generation = publish_backend_generation(destination, source, observed=observed)
    source.unlink()
    if selected:
        backend_binary._select_backend_generation(
            backend_binary.StoredHostTool(destination).receipt,
            generation,
            fingerprint=fingerprint,
            cargo_environment=cargo_environment,
        )
    return generation.path


def selected_backend(project_root: Path, coordinate: Path, cargo_profile: str) -> Path:
    """The generation the store selects for this checkout; fails when none is."""
    del cargo_profile
    from molt.cli.backend_generation import selected_backend_generation
    from molt.cli.runtime_fingerprints import _read_runtime_fingerprint

    store = backend_binary.host_tool_store(project_root)
    entries = sorted(store.root.glob("[0-9a-f]" * 64)) if store.root.exists() else []
    for entry in entries:
        for variant in store.variants(entry.name):
            stored = backend_binary.StoredHostTool(variant / coordinate.name)
            generation = selected_backend_generation(
                stored.coordinate, _read_runtime_fingerprint(stored.receipt)
            )
            if generation is not None:
                return generation
    raise AssertionError(f"no generation stored for {coordinate}")
