"""Tests for the runtime-wasm per-phase build-timing accumulator (doctrine 74 law 4)."""

from __future__ import annotations

from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
    _reset_runtime_wasm_build_timings,
    _runtime_wasm_build_timings_snapshot,
)


def setup_function() -> None:
    _reset_runtime_wasm_build_timings()


def teardown_function() -> None:
    _reset_runtime_wasm_build_timings()


def test_empty_snapshot_is_none() -> None:
    assert _runtime_wasm_build_timings_snapshot() is None


def test_dual_compile_reports_two_builds() -> None:
    # The V1 "before" shape: --kind both drives two full cargo compiles.
    _record_runtime_wasm_build_phase("cargo_compile", 151.0, kind="shared", mode="build")
    _record_runtime_wasm_build_phase("cargo_compile", 149.0, kind="reloc", mode="build")
    _record_runtime_wasm_build_phase("reloc_link", 3.5, kind="reloc", mode="link")
    snap = _runtime_wasm_build_timings_snapshot()
    assert snap is not None
    assert snap["cargo_compile_builds"] == 2
    assert snap["cargo_compile_reuses"] == 0
    assert snap["cargo_compile_build_wall_s"] == 300.0
    assert len(snap["phases"]) == 3


def test_single_compile_reports_one_build_and_a_reuse() -> None:
    # The V1 "after" shape: one combined compile, the second kind reused.
    _record_runtime_wasm_build_phase("cargo_compile", 150.0, kind="combined", mode="build")
    _record_runtime_wasm_build_phase(
        "cargo_compile", 0.0, kind="reloc", mode="target_reuse"
    )
    _record_runtime_wasm_build_phase("reloc_link", 3.2, kind="reloc", mode="link")
    snap = _runtime_wasm_build_timings_snapshot()
    assert snap is not None
    assert snap["cargo_compile_builds"] == 1
    assert snap["cargo_compile_reuses"] == 1
    assert snap["cargo_compile_build_wall_s"] == 150.0


def test_negative_wall_is_clamped_and_detail_preserved() -> None:
    _record_runtime_wasm_build_phase(
        "cargo_compile", -5.0, kind="shared", mode="shared_cache", detail="hydrated"
    )
    snap = _runtime_wasm_build_timings_snapshot()
    assert snap is not None
    assert snap["phases"][0]["wall_s"] == 0.0
    assert snap["phases"][0]["detail"] == "hydrated"
    assert snap["cargo_compile_reuses"] == 1


def test_exact_cache_hydration_precedes_reloc_target_relink() -> None:
    import inspect

    from molt.cli import runtime_build

    source = inspect.getsource(runtime_build._ensure_runtime_wasm)
    hydrate_offset = source.index("_hydrate_runtime_wasm_from_shared_cache(")
    target_relink_offset = source.index(
        "target_runtime_staticlib_current = _current_runtime_target_artifact("
    )

    assert hydrate_offset < target_relink_offset
