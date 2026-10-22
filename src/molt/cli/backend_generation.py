"""Immutable, content-addressed generations of compiler and host-tool executables.

A host tool's coordinate names a selection, not a file a build runs. The
coordinates that publish generations live in a product store variant
(``molt.cli.product_store``, kind ``host-tool``): each published executable
lives at ``<variant>/.molt-backend-generations/<tool>/<sha256>/<tool>`` and
never changes after publication. A build publishes another generation or
reuses this identical one. The variant's store receipt is the selection; its
byte identity names the selected generation, and it is written once, after
the generation and its probe evidence. Bazel and Nix publish outputs the same
way: content-addressed paths and an atomic pointer. The WASM facts product
reuses this store and receipt authority with its own product coordinate; the
historical directory name is not a tool kind.

The generation keeps the variant's basename, so process discovery and the
executable's content identity read the same name a variant always had.
Retention of whole entries belongs to the product store. Inside a variant the
shared generation store owner selects each published generation and evicts
unselected ones (a publication whose probe failed) after its idle window; the
selected generation is never evicted, so admitting it writes nothing.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Mapping

from molt.cli.atomic_io import _staged_copy_file
from molt.cli.generation_store import (
    GENERATION_STORE_IDLE_SECONDS,
    GenerationStoreLayout,
    generation_store,
)
from molt.file_deletion import delete_path, unlink_file
from molt.file_publication import durable_publish_exclusive, is_link_like
from molt.python_identity_common import _valid_sha256
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)


BACKEND_GENERATIONS_DIR = ".molt-backend-generations"
_ENTRY_RE = re.compile(r"^[0-9a-f]{64}$")


def _remove_generation(entry: Path) -> None:
    """Remove one idle generation directory and the executable it holds."""
    removed, error = delete_path(entry)
    if not removed:
        raise OSError(error)


_BACKEND_STORE = GenerationStoreLayout(
    label="backend compiler",
    lock_name=".backend-store.lock",
    ledger_name=".backend-store.json",
    ledger_schema="molt.backend-generation-store.v1",
    entry=_ENTRY_RE,
    remove=_remove_generation,
)


def backend_generation_store_root(coordinate: Path) -> Path:
    """The store that holds every generation of one backend variant."""
    return coordinate.parent / BACKEND_GENERATIONS_DIR / coordinate.name


def backend_generation_path(coordinate: Path, sha256: str) -> Path:
    """The immutable executable of ``coordinate`` whose bytes hash to ``sha256``."""
    if not _valid_sha256(sha256):
        raise ValueError(f"backend generation needs a SHA-256 address: {sha256!r}")
    return backend_generation_store_root(coordinate) / sha256 / coordinate.name


def selected_backend_generation(
    coordinate: Path, receipt: Mapping[str, Any] | None
) -> Path | None:
    """The generation a backend receipt selects, or None for no selection."""
    if receipt is None:
        return None
    content = receipt.get("artifact_content_identity")
    sha256 = content.get("sha256") if isinstance(content, Mapping) else None
    if not isinstance(sha256, str) or not _valid_sha256(sha256):
        return None
    return backend_generation_path(coordinate, sha256)


def is_backend_generation(coordinate: Path, path: Path) -> bool:
    """Whether ``path`` names one generation of ``coordinate``'s store."""
    if path.name != coordinate.name or _ENTRY_RE.match(path.parent.name) is None:
        return False
    try:
        return path.parent.parent.resolve(strict=True) == backend_generation_store_root(
            coordinate
        ).resolve(strict=True)
    except OSError:
        return False


def _existing_generation(
    generation: Path, source: StableRegularFileIdentity
) -> StableRegularFileIdentity | None:
    """Admit a generation already published with ``source``'s bytes, or None.

    A file at a content address whose bytes hash to another digest was never
    admitted by any build, so nothing can hold it: the caller replaces it.
    """
    if is_link_like(generation.parent) or is_link_like(generation):
        raise ValueError(f"backend generation is indirect: {generation}")
    if not generation.exists():
        return None
    existing = stable_regular_file_identity(generation, label="backend generation")
    if existing.sha256 == source.sha256 and existing.size == source.size:
        return existing
    unlink_file(generation)
    return None


def publish_backend_generation(
    coordinate: Path, source: Path, *, observed: StableRegularFileIdentity
) -> StableRegularFileIdentity:
    """Publish ``source``'s admitted bytes as one generation of ``coordinate``.

    The copy is verified against ``observed`` while it streams, then appears
    under its content address by an exclusive rename; an identical generation
    is reused without a write. The store selects it and evicts idle ones. The
    caller holds the backend admission lock and writes the receipt that
    selects the returned generation.
    """
    if source.absolute() != observed.path:
        raise ValueError("backend generation source differs from its observation")
    store_root = backend_generation_store_root(coordinate)
    store_root.mkdir(parents=True, exist_ok=True)
    # Resolved, so the returned identity is the path an executable probe sees.
    store_root = store_root.resolve(strict=True)
    generation = store_root / observed.sha256 / coordinate.name
    with generation_store(store_root, _BACKEND_STORE) as store:
        published = _existing_generation(generation, observed)
        if published is None:
            generation.parent.mkdir(exist_ok=True)
            with _staged_copy_file(source, generation, observed=observed) as staged:
                durable_publish_exclusive(staged, generation)
            published = stable_regular_file_identity(
                generation, label="published backend generation"
            )
            if published.sha256 != observed.sha256 or published.size != observed.size:
                raise ValueError(
                    f"published backend generation differs from its source: {generation}"
                )
        verify_stable_regular_file_identity(observed, label="backend generation source")
        store.select(generation.parent.name)
        store.evict_idle(GENERATION_STORE_IDLE_SECONDS)
    return published
