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

import hashlib
import json
import os
import re
from collections.abc import Callable, Iterable, Mapping
from pathlib import Path
from typing import Any

from molt.cli.atomic_io import _atomic_copy_file, _write_json_sidecar
from molt.cli.default_paths import _default_molt_cache


_CACHE_KEY_HEX_RE = re.compile(r"\A[0-9a-f]{64}\Z")
_RUNTIME_WASM_CACHE_STATS: dict[str, int | str] = {
    "hydrate_attempts": 0,
    "hydrate_hits": 0,
    "hydrate_misses": 0,
    "hydrate_failures": 0,
    "publish_attempts": 0,
    "publish_successes": 0,
    "publish_failures": 0,
    "last_publish_failure": "",
    # V3 config-lattice reuse (MOLT_BUILD_REUSE_COMPATIBLE): an iteration-profile
    # request served by a same-source compatible-or-better-opt cached artifact.
    "compat_hydrate_attempts": 0,
    "compat_hydrate_hits": 0,
    "compat_hydrate_misses": 0,
}


# Consumer-opt-reuse rank: a runtime-wasm request at rank R may be satisfied by
# a cached artifact at rank >= R (equal or higher optimisation) whose ABI inputs
# are identical, because run_wasm.js / tools/wasm_link.py consume the artifact by
# export/import symbol contract only -- the opt level is invisible to them
# (doctrine 74 law 3). Unknown profiles get the lowest rank so they only ever
# satisfy an exactly-equal-named request, never silently substitute a peer.
_PROFILE_REUSE_RANK: dict[str, int] = {
    "dev": 0,
    "debug": 0,
    "dev-fast": 1,
    "wasm-release-fallback": 2,
    "wasm-release": 3,
    "release": 3,
    "release-output": 4,
    "release-fast": 4,
}


def _profile_reuse_rank(profile: str) -> int:
    return _PROFILE_REUSE_RANK.get(profile, -1)


def _build_reuse_compatible_enabled() -> bool:
    """Whether V3 config-lattice reuse is active (opt-in; default OFF).

    Acceptance/proof lanes MUST pin exact content identity (M05), so this stays
    opt-in via ``MOLT_BUILD_REUSE_COMPATIBLE=1`` for heavy iteration dev loops.
    """
    return os.environ.get("MOLT_BUILD_REUSE_COMPATIBLE", "").strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }


def _runtime_wasm_compat_digest(
    *,
    target_triple: str | None,
    rustflags: str,
    features: Iterable[str],
) -> str:
    """ABI-identity digest EXCLUDING the cargo profile (opt level).

    Two runtime builds that differ only in opt level share this digest, so a
    lattice lookup can prove "same source ABI, only the profile differs" before
    substituting a compatible-or-better artifact.
    """
    payload = (
        f"target:{target_triple or 'native'}\n"
        f"rustflags:{rustflags}\n"
        f"features:{','.join(sorted(features))}\n"
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _runtime_wasm_cache_diagnostics_snapshot() -> dict[str, Any] | None:
    """Return a diagnostics snapshot for runtime-wasm shared-cache activity."""
    hydrate_attempts = int(_RUNTIME_WASM_CACHE_STATS["hydrate_attempts"])
    publish_attempts = int(_RUNTIME_WASM_CACHE_STATS["publish_attempts"])
    if hydrate_attempts == 0 and publish_attempts == 0:
        return None
    hydrate_hits = int(_RUNTIME_WASM_CACHE_STATS["hydrate_hits"])
    publish_successes = int(_RUNTIME_WASM_CACHE_STATS["publish_successes"])
    snapshot: dict[str, Any] = {
        "hydrate_attempts": hydrate_attempts,
        "hydrate_hits": hydrate_hits,
        "hydrate_misses": int(_RUNTIME_WASM_CACHE_STATS["hydrate_misses"]),
        "hydrate_failures": int(_RUNTIME_WASM_CACHE_STATS["hydrate_failures"]),
        "hydrate_hit_rate": round(hydrate_hits / max(1, hydrate_attempts), 6),
        "publish_attempts": publish_attempts,
        "publish_successes": publish_successes,
        "publish_failures": int(_RUNTIME_WASM_CACHE_STATS["publish_failures"]),
        "publish_success_rate": round(
            publish_successes / max(1, publish_attempts),
            6,
        ),
    }
    last_publish_failure = str(_RUNTIME_WASM_CACHE_STATS["last_publish_failure"])
    if last_publish_failure:
        snapshot["last_publish_failure"] = last_publish_failure
    compat_attempts = int(_RUNTIME_WASM_CACHE_STATS["compat_hydrate_attempts"])
    if compat_attempts:
        compat_hits = int(_RUNTIME_WASM_CACHE_STATS["compat_hydrate_hits"])
        snapshot["compat_hydrate_attempts"] = compat_attempts
        snapshot["compat_hydrate_hits"] = compat_hits
        snapshot["compat_hydrate_misses"] = int(
            _RUNTIME_WASM_CACHE_STATS["compat_hydrate_misses"]
        )
        snapshot["compat_hydrate_hit_rate"] = round(
            compat_hits / max(1, compat_attempts), 6
        )
    return snapshot


def _reset_runtime_wasm_cache_diagnostics() -> None:
    """Reset process-local diagnostics counters. Intended for tests."""
    for key in list(_RUNTIME_WASM_CACHE_STATS):
        _RUNTIME_WASM_CACHE_STATS[key] = "" if key == "last_publish_failure" else 0


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


def _runtime_wasm_compat_index_path(
    *,
    reloc: bool,
    inputs_digest: str,
    compat_digest: str,
    cargo_profile: str,
) -> Path:
    kind = "reloc" if reloc else "shared"
    return _shared_runtime_wasm_cache_root() / (
        f"compat.{kind}.{inputs_digest}.{compat_digest}.{cargo_profile}.json"
    )


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
    if cache_path is None:
        return False
    _RUNTIME_WASM_CACHE_STATS["hydrate_attempts"] = (
        int(_RUNTIME_WASM_CACHE_STATS["hydrate_attempts"]) + 1
    )
    if not cache_path.is_file():
        _RUNTIME_WASM_CACHE_STATS["hydrate_misses"] = (
            int(_RUNTIME_WASM_CACHE_STATS["hydrate_misses"]) + 1
        )
        return False
    if not bool(is_valid(cache_path)):
        _RUNTIME_WASM_CACHE_STATS["hydrate_failures"] = (
            int(_RUNTIME_WASM_CACHE_STATS["hydrate_failures"]) + 1
        )
        return False
    try:
        _atomic_copy_file(cache_path, dest)
    except OSError:
        _RUNTIME_WASM_CACHE_STATS["hydrate_failures"] = (
            int(_RUNTIME_WASM_CACHE_STATS["hydrate_failures"]) + 1
        )
        return False
    _RUNTIME_WASM_CACHE_STATS["hydrate_hits"] = (
        int(_RUNTIME_WASM_CACHE_STATS["hydrate_hits"]) + 1
    )
    return True


def _publish_runtime_wasm_to_shared_cache(
    *,
    src: Path,
    fingerprint: Mapping[str, object],
    reloc: bool,
    compat: Mapping[str, object] | None = None,
) -> str | None:
    """Publish a freshly built runtime wasm into the shared cache.

    Best-effort: a failure to publish must never fail the build (the artifact is
    already staged into the session/destination path). It only means the next
    fresh session will pay for a rebuild. Uses a byte copy so it is safe on
    filesystems without hard-link support (exFAT APDataStore volume).
    """
    cache_path = _shared_runtime_wasm_cache_path(fingerprint, reloc=reloc)
    if cache_path is None or not src.is_file():
        return None
    _RUNTIME_WASM_CACHE_STATS["publish_attempts"] = (
        int(_RUNTIME_WASM_CACHE_STATS["publish_attempts"]) + 1
    )
    try:
        _atomic_copy_file(src, cache_path)
    except OSError as exc:
        reason = f"artifact copy failed: {exc}"
        _RUNTIME_WASM_CACHE_STATS["publish_failures"] = (
            int(_RUNTIME_WASM_CACHE_STATS["publish_failures"]) + 1
        )
        _RUNTIME_WASM_CACHE_STATS["last_publish_failure"] = reason
        return reason
    key = _runtime_wasm_cache_key(fingerprint)
    if key is None:
        return None
    hash_value, meta_digest = key
    sidecar_payload: dict[str, object] = {
        "hash": hash_value,
        "meta_digest": meta_digest,
        "reloc": reloc,
        "rustc": fingerprint.get("rustc"),
    }
    # V3 lattice index: record the profile-independent ABI identity so a later
    # iteration-profile request can find this artifact as compatible-or-better.
    if compat is not None:
        inputs_digest = compat.get("inputs_digest")
        compat_digest = compat.get("compat_digest")
        cargo_profile = compat.get("cargo_profile")
        if inputs_digest and compat_digest and cargo_profile:
            sidecar_payload["inputs_digest"] = inputs_digest
            sidecar_payload["compat_digest"] = compat_digest
            sidecar_payload["cargo_profile"] = cargo_profile
    try:
        _write_json_sidecar(
            cache_path.with_suffix(".wasm.json"),
            sidecar_payload,
        )
    except OSError as exc:
        reason = f"metadata sidecar failed: {exc}"
        _RUNTIME_WASM_CACHE_STATS["publish_failures"] = (
            int(_RUNTIME_WASM_CACHE_STATS["publish_failures"]) + 1
        )
        _RUNTIME_WASM_CACHE_STATS["last_publish_failure"] = reason
        return reason
    if all(
        isinstance(sidecar_payload.get(key), str)
        for key in ("inputs_digest", "compat_digest", "cargo_profile")
    ):
        try:
            _write_json_sidecar(
                _runtime_wasm_compat_index_path(
                    reloc=reloc,
                    inputs_digest=str(sidecar_payload["inputs_digest"]),
                    compat_digest=str(sidecar_payload["compat_digest"]),
                    cargo_profile=str(sidecar_payload["cargo_profile"]),
                ),
                {
                    "artifact": cache_path.name,
                    "cargo_profile": sidecar_payload["cargo_profile"],
                },
            )
        except OSError as exc:
            reason = f"compatibility index failed: {exc}"
            _RUNTIME_WASM_CACHE_STATS["publish_failures"] = (
                int(_RUNTIME_WASM_CACHE_STATS["publish_failures"]) + 1
            )
            _RUNTIME_WASM_CACHE_STATS["last_publish_failure"] = reason
            return reason
    _RUNTIME_WASM_CACHE_STATS["publish_successes"] = (
        int(_RUNTIME_WASM_CACHE_STATS["publish_successes"]) + 1
    )
    return None


def _hydrate_runtime_wasm_from_compatible_cache(
    *,
    dest: Path,
    reloc: bool,
    inputs_digest: str | None,
    compat_digest: str,
    request_profile: str,
    is_valid: Callable[[Path], bool],
    exports_ok: Callable[[Path], bool],
) -> bool:
    """V3: reuse a same-source, compatible-or-better-opt cached runtime wasm.

    Only runs when ``MOLT_BUILD_REUSE_COMPATIBLE`` is set (checked by the caller).
    Scans the shared cache for an artifact of the same kind whose ABI identity
    (``inputs_digest`` + ``compat_digest``) matches this request but whose cargo
    profile is compatible-or-better (``_profile_reuse_rank`` >= the request's).
    The candidate must still pass the mode-appropriate structural validator AND
    the caller's export-satisfaction check, so a lattice reuse can never hand the
    consumer an artifact missing a required symbol. Highest-opt match wins.
    """
    if not inputs_digest:
        return False
    request_rank = _profile_reuse_rank(request_profile)
    _RUNTIME_WASM_CACHE_STATS["compat_hydrate_attempts"] = (
        int(_RUNTIME_WASM_CACHE_STATS["compat_hydrate_attempts"]) + 1
    )
    root = _shared_runtime_wasm_cache_root()
    kind = "reloc" if reloc else "shared"
    prefix = f"molt_runtime.{kind}."
    candidates: list[tuple[int, str, Path]] = []
    index_pattern = f"compat.{kind}.{inputs_digest}.{compat_digest}.*.json"
    try:
        index_paths = sorted(root.glob(index_pattern))
    except OSError:
        index_paths = []
    for index_path in index_paths:
        try:
            index = json.loads(index_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(index, dict):
            continue
        cand_profile = index.get("cargo_profile")
        artifact_name = index.get("artifact")
        if not isinstance(cand_profile, str) or not isinstance(artifact_name, str):
            continue
        cand_rank = _profile_reuse_rank(cand_profile)
        if cand_rank < 0 or cand_rank < request_rank:
            continue
        artifact = root / artifact_name
        if (
            artifact.parent != root
            or artifact.suffix != ".wasm"
            or not artifact.is_file()
        ):
            continue
        candidates.append((cand_rank, artifact.name, artifact))
    if not candidates:
        try:
            sidecars = sorted(root.glob(f"{prefix}*.wasm.json"))
        except OSError:
            sidecars = []
    else:
        sidecars = []
    for sidecar in sidecars:
        try:
            meta = json.loads(sidecar.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(meta, dict):
            continue
        if bool(meta.get("reloc")) != reloc:
            continue
        if meta.get("inputs_digest") != inputs_digest:
            continue
        if meta.get("compat_digest") != compat_digest:
            continue
        cand_profile = meta.get("cargo_profile")
        if not isinstance(cand_profile, str):
            continue
        cand_rank = _profile_reuse_rank(cand_profile)
        if cand_rank < 0 or cand_rank < request_rank:
            continue
        artifact = sidecar.with_suffix("")  # strip ".json" -> "....wasm"
        if artifact.suffix != ".wasm" or not artifact.is_file():
            continue
        candidates.append((cand_rank, artifact.name, artifact))
    # Highest opt rank first, then a stable name tiebreak for determinism.
    candidates.sort(key=lambda item: (-item[0], item[1]))
    for _rank, _name, artifact in candidates:
        if not bool(is_valid(artifact)):
            continue
        try:
            _atomic_copy_file(artifact, dest)
        except OSError:
            continue
        if bool(is_valid(dest)) and bool(exports_ok(dest)):
            _RUNTIME_WASM_CACHE_STATS["compat_hydrate_hits"] = (
                int(_RUNTIME_WASM_CACHE_STATS["compat_hydrate_hits"]) + 1
            )
            return True
    _RUNTIME_WASM_CACHE_STATS["compat_hydrate_misses"] = (
        int(_RUNTIME_WASM_CACHE_STATS["compat_hydrate_misses"]) + 1
    )
    return False
