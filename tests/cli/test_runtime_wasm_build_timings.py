"""Tests for the runtime-wasm per-phase build-timing accumulator (doctrine 74 law 4)."""

from __future__ import annotations

import pytest

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
    _record_runtime_wasm_build_phase(
        "cargo_compile", 151.0, kind="shared", mode="build"
    )
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
    _record_runtime_wasm_build_phase(
        "cargo_compile", 150.0, kind="combined", mode="build"
    )
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


def test_runtime_identity_pre_and_post_wall_are_attributed_separately() -> None:
    _record_runtime_wasm_build_phase(
        "runtime_toolchain_identity",
        7.0,
        kind="pair",
        mode="pre_build",
        detail="status=ok,files=17983,bytes=249161774",
    )
    _record_runtime_wasm_build_phase(
        "runtime_source_identity", 1.5, kind="pair", mode="pre_build"
    )
    _record_runtime_wasm_build_phase(
        "runtime_toolchain_identity", 6.0, kind="pair", mode="post_build"
    )
    _record_runtime_wasm_build_phase(
        "runtime_source_identity", 1.0, kind="pair", mode="post_build"
    )

    snap = _runtime_wasm_build_timings_snapshot()

    assert snap is not None
    assert snap["runtime_identity_pre_wall_s"] == 8.5
    assert snap["runtime_identity_post_wall_s"] == 7.0
    assert snap["runtime_identity_wall_s"] == 15.5
    assert snap["phases"][0]["detail"] == ("status=ok,files=17983,bytes=249161774")


def test_runtime_identity_phase_detail_attests_selected_workers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import runtime_wasm_build_spec

    monkeypatch.setattr(
        runtime_wasm_build_spec, "_tree_hash_worker_count", lambda _count: 6
    )

    detail = runtime_wasm_build_spec._runtime_identity_tree_phase_detail(
        {"file_count": 17_983, "total_size": 249_161_774}, status="ok"
    )

    assert detail == "status=ok,files=17983,bytes=249161774,workers=6"


def test_atomic_pair_cache_hydration_precedes_combined_build() -> None:
    import inspect

    from molt.cli import runtime_wasm_pair_build

    source = inspect.getsource(runtime_wasm_pair_build._materialize_runtime_wasm_pair)
    hydrate_offset = source.index("hydrate_runtime_wasm_pair_from_shared_cache(")
    combined_build_offset = source.index("_prepopulate_combined_runtime_wasm_target(")

    assert hydrate_offset < combined_build_offset


def test_exact_pair_build_records_pre_and_post_identity_phases() -> None:
    import inspect

    from molt.cli import runtime_wasm_pair_build

    identity_source = inspect.getsource(
        runtime_wasm_pair_build._resolve_runtime_wasm_pair_identity
    )
    prebuild_source = inspect.getsource(
        runtime_wasm_pair_build._materialize_runtime_wasm_pair
    )
    publication_source = inspect.getsource(
        runtime_wasm_pair_build._publish_runtime_wasm_pair
    )

    assert identity_source.count('phase="runtime_toolchain_identity"') == 1
    assert identity_source.count('phase="runtime_source_identity"') == 1
    assert 'mode="pre_build"' in prebuild_source
    assert 'mode="post_build"' in publication_source
