"""Shared, content-addressed cache for the ``molt_runtime.wasm`` artifact.

The runtime wasm artifact (the whole Molt runtime crate compiled to
``wasm32-wasip1``) is a *fixed* function of the runtime source tree, the target,
the cargo profile, the resolved feature set, and the linker flags -- it does not
depend on the user program being built. Its build identity is already fully
captured by the runtime fingerprint computed in
``molt.cli.runtime_fingerprints._runtime_fingerprint`` (``hash`` over the runtime
source tree, ``meta_digest`` over profile/target/rustflags/features).

The per-session cargo target dir (``target/sessions/<MOLT_SESSION_ID>/``) is a
*correctness* isolation boundary for concurrent agents, not a caching home. A
fresh session/worktree therefore starts with a cold target dir and would
recompile the entire runtime crate from scratch (~45min, memory-thrashing on a
small box) even though a byte-identical runtime wasm has already been built by
another session for the same runtime source + build identity.

This module is the single authority for a runtime-wasm cache that lives *outside*
any per-session target dir, keyed only on the content-addressed fingerprint, so a
fresh session can reuse a warm artifact instead of recompiling:

* The cache root is ``_default_molt_cache() / "runtime_wasm"`` -- the same shared
  cache authority the backend compilation cache uses. It is independent of
  ``MOLT_SESSION_ID`` and of the worktree ``project_root``, so it survives across
  sessions and worktrees.
* The cache key is derived from the fingerprint ``hash`` and ``meta_digest`` plus
  the artifact kind (shared vs reloc). Identical runtime source + target + profile
  + features resolve to the *same* cache slot regardless of session.
* Publication uses ``_atomic_copy_file`` (a byte copy), never ``os.link``, so it
  is safe on the exFAT APDataStore build volume which rejects hard links.
* Because the cache lives outside every cargo target dir, the memory-guard cargo
  incremental quarantine (which only relocates ``target/**/incremental`` dirs)
  can never destroy it.
"""

from __future__ import annotations

import re
from collections.abc import Callable, Mapping
from pathlib import Path

from molt.cli.atomic_io import _atomic_copy_file, _write_json_sidecar
from molt.cli.default_paths import _default_molt_cache


_CACHE_KEY_HEX_RE = re.compile(r"\A[0-9a-f]{64}\Z")


def _shared_runtime_wasm_cache_root() -> Path:
    """Session-independent shared cache root for runtime wasm artifacts."""
    return _default_molt_cache() / "runtime_wasm"


def _runtime_wasm_cache_key(
    fingerprint: Mapping[str, object],
) -> tuple[str, str] | None:
    """Return ``(hash, meta_digest)`` content-address key, or ``None``.

    Both values are the sha256 hex digests produced by ``_runtime_fingerprint``.
    ``hash`` covers the runtime source tree plus the build-identity meta;
    ``meta_digest`` covers only the resolved profile/target/rustflags/features.
    Requiring both keeps distinct build identities in distinct cache slots even
    in the (impossible-by-construction) event of a ``hash`` collision, and makes
    the slot name self-describing for offline inspection.
    """
    hash_value = fingerprint.get("hash")
    meta_digest = fingerprint.get("meta_digest")
    if (
        not isinstance(hash_value, str)
        or _CACHE_KEY_HEX_RE.match(hash_value) is None
        or not isinstance(meta_digest, str)
        or _CACHE_KEY_HEX_RE.match(meta_digest) is None
    ):
        return None
    return hash_value, meta_digest


def _shared_runtime_wasm_cache_path(
    fingerprint: Mapping[str, object],
    *,
    reloc: bool,
) -> Path | None:
    """Absolute path of the shared cache slot for one build identity.

    Returns ``None`` when the fingerprint lacks a usable content-address key
    (e.g. a legacy fingerprint that never resolved ``hash``/``meta_digest``),
    so callers fall back to a normal build instead of caching under a bad key.
    """
    key = _runtime_wasm_cache_key(fingerprint)
    if key is None:
        return None
    hash_value, meta_digest = key
    kind = "reloc" if reloc else "shared"
    filename = f"molt_runtime.{kind}.{hash_value}.{meta_digest}.wasm"
    return _shared_runtime_wasm_cache_root() / filename


def _hydrate_runtime_wasm_from_shared_cache(
    *,
    dest: Path,
    fingerprint: Mapping[str, object],
    reloc: bool,
    is_valid: Callable[[Path], bool],
) -> bool:
    """Copy a warm shared-cache runtime wasm into ``dest`` when one matches.

    ``is_valid`` is the mode-appropriate structural validator (a callable taking
    the artifact path and returning ``bool``) so the shared cache never hands a
    corrupt artifact to the build even if a stray file lands in the cache dir.
    Returns ``True`` only when ``dest`` now holds the reused, validated artifact.
    """
    cache_path = _shared_runtime_wasm_cache_path(fingerprint, reloc=reloc)
    if cache_path is None or not cache_path.is_file():
        return False
    if not bool(is_valid(cache_path)):
        return False
    try:
        _atomic_copy_file(cache_path, dest)
    except OSError:
        return False
    return bool(is_valid(dest))


def _publish_runtime_wasm_to_shared_cache(
    *,
    src: Path,
    fingerprint: Mapping[str, object],
    reloc: bool,
) -> None:
    """Publish a freshly built runtime wasm into the shared cache.

    Best-effort: a failure to publish must never fail the build (the artifact is
    already staged into the session/destination path). It only means the next
    fresh session will pay for a rebuild. Uses a byte copy so it is safe on
    filesystems without hard-link support (exFAT APDataStore volume).
    """
    cache_path = _shared_runtime_wasm_cache_path(fingerprint, reloc=reloc)
    if cache_path is None or not src.is_file():
        return
    try:
        _atomic_copy_file(src, cache_path)
    except OSError:
        return
    key = _runtime_wasm_cache_key(fingerprint)
    if key is None:
        return
    hash_value, meta_digest = key
    try:
        _write_json_sidecar(
            cache_path.with_suffix(".wasm.json"),
            {
                "hash": hash_value,
                "meta_digest": meta_digest,
                "reloc": reloc,
                "rustc": fingerprint.get("rustc"),
            },
        )
    except OSError:
        return
