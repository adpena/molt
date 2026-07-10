"""Per-phase wall-clock instrumentation for the runtime-wasm build.

This is the process-local attestation surface for build-work-deduplication
doctrine 74 law 4 ("Attested"): every runtime-wasm build step emits
``{phase, kind, mode, wall}`` so a claim about the *number of cargo compiles per
source change* (the V1 dual-compile violation) is a measured number, not a hope.

The accumulator mirrors the shared-cache diagnostics counters in
``runtime_wasm_cache`` -- a module-global list appended to as phases complete,
snapshotted into ``MOLT_BUILD_DIAGNOSTICS`` (task #21 surface) at the end of a
build.  Modes:

* ``build``       -- a cargo ``rustc`` compile actually ran (the cost we dedup).
* ``target_reuse``-- reused an artifact already in the cargo target dir
                     (same-session second pass / same-fingerprint rebuild).
* ``shared_cache``-- hydrated from the session-independent shared cache (V2).
* ``fingerprint`` -- the destination artifact already matched its fingerprint.
* ``link``        -- a post-cargo ``wasm-ld`` link (reloc ``-r`` / recovery).

The count of ``cargo_compile`` phases whose ``mode == "build"`` for one
``--kind both`` invocation is exactly the V1 acceptance metric: 2 before the
single-compile fix, 1 after.
"""

from __future__ import annotations

import threading
from typing import Any


_LOCK = threading.Lock()
_RUNTIME_WASM_BUILD_PHASES: list[dict[str, Any]] = []


def _record_runtime_wasm_build_phase(
    phase: str,
    wall_s: float,
    *,
    kind: str,
    mode: str,
    detail: str | None = None,
) -> None:
    """Append one completed runtime-wasm build phase to the accumulator.

    ``wall_s`` is a wall-clock duration in seconds (``time.perf_counter`` delta).
    ``kind`` is the artifact kind (``shared`` / ``reloc`` / ``combined``).
    ``mode`` is one of the modes documented in the module docstring.
    """
    record: dict[str, Any] = {
        "phase": phase,
        "kind": kind,
        "mode": mode,
        "wall_s": round(max(0.0, float(wall_s)), 6),
    }
    if detail:
        record["detail"] = detail
    with _LOCK:
        _RUNTIME_WASM_BUILD_PHASES.append(record)


def _runtime_wasm_build_timings_snapshot() -> dict[str, Any] | None:
    """Return a diagnostics snapshot of runtime-wasm build phases, or ``None``.

    ``cargo_compile_builds`` is the load-bearing metric: how many cargo compiles
    actually ran (mode == ``build``).  ``cargo_compile_reuses`` counts compiles
    avoided by the dedup machinery.
    """
    with _LOCK:
        phases = [dict(record) for record in _RUNTIME_WASM_BUILD_PHASES]
    if not phases:
        return None
    cargo_builds = [
        record
        for record in phases
        if record["phase"] == "cargo_compile" and record["mode"] == "build"
    ]
    cargo_reuses = [
        record
        for record in phases
        if record["phase"] == "cargo_compile" and record["mode"] != "build"
    ]
    return {
        "phases": phases,
        "cargo_compile_builds": len(cargo_builds),
        "cargo_compile_reuses": len(cargo_reuses),
        "cargo_compile_build_wall_s": round(
            sum(record["wall_s"] for record in cargo_builds), 6
        ),
        "total_wall_s": round(sum(record["wall_s"] for record in phases), 6),
    }


def _reset_runtime_wasm_build_timings() -> None:
    """Clear the process-local accumulator. Intended for tests and per-run reuse."""
    with _LOCK:
        _RUNTIME_WASM_BUILD_PHASES.clear()
