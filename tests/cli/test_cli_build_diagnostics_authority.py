from __future__ import annotations

import inspect
import json
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import build_diagnostics
from molt.cli.models import _BuildDiagnosticsContext
from molt.cli import wasm_final_link_cache
from molt.link_outputs import wasm_link_output_paths

_BUILD_DIAGNOSTICS_NAMES = (
    "_build_allocation_diagnostics_enabled",
    "_build_phase_attribution",
    "_build_build_diagnostics_payload",
    "_build_diagnostics_enabled",
    "_build_midend_diagnostics_payload",
    "_build_reason_summary",
    "_capture_build_allocation_diagnostics",
    "_duration_ms_from_ns",
    "_emit_build_diagnostics",
    "_emit_build_diagnostics_if_present",
    "_frontend_lowering_cache_summary",
    "_midend_policy_config_snapshot",
    "_midend_sample_p95",
    "_midend_sample_percentile",
    "_normalize_midend_pass_stat",
    "_phase_duration_map",
    "_record_frontend_timing_item",
    "_resolve_build_diagnostics_path",
    "_resolve_build_diagnostics_verbosity",
    "_wasm_link_operation_counts",
)

_BUILD_DIAGNOSTICS_DEFINITIONS = (
    "def _build_allocation_diagnostics_enabled(",
    "def _build_phase_attribution(",
    "def _build_build_diagnostics_payload(",
    "def _build_diagnostics_enabled(",
    "def _build_midend_diagnostics_payload(",
    "def _build_reason_summary(",
    "def _capture_build_allocation_diagnostics(",
    "def _duration_ms_from_ns(",
    "def _emit_build_diagnostics(",
    "def _emit_build_diagnostics_if_present(",
    "def _frontend_lowering_cache_summary(",
    "def _midend_policy_config_snapshot(",
    "def _midend_sample_p95(",
    "def _midend_sample_percentile(",
    "def _normalize_midend_pass_stat(",
    "def _phase_duration_map(",
    "def _record_frontend_timing_item(",
    "def _resolve_build_diagnostics_path(",
    "def _resolve_build_diagnostics_verbosity(",
    "def _wasm_link_operation_counts(",
)


def test_cli_build_diagnostics_authority_is_single_home() -> None:
    for name in _BUILD_DIAGNOSTICS_NAMES:
        assert hasattr(build_diagnostics, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for marker in _BUILD_DIAGNOSTICS_DEFINITIONS:
        assert marker not in cli_source


def test_build_phase_attribution_reports_relative_shares_and_link_children() -> None:
    attribution = build_diagnostics._build_phase_attribution(
        total_sec=20.0,
        phase_sec={"ir_lowering": 2.0, "backend_codegen": 8.0},
        pipeline_stage_ms={
            "wasm_reloc_preflight": 750.0,
            "wasm_link_total": 6000.0,
            "split_runtime_processing": 1000.0,
            "wasm_strip": 500.0,
            "fail_closed_validation": 500.0,
            "wasm_whole_artifact_full_binary_parses": 17,
            "wasm_whole_artifact_section_walks": 29,
            "wasm_whole_artifact_reserializations": 11,
            "wasm_whole_artifact_redundant_parses_eliminated": 6,
        },
    )

    assert attribution["phase_sec"]["wasm_link_core"] == 4.0
    # Admission precedes the pipeline timer; it must not be subtracted from
    # the pipeline's core or charged to frontend work.
    assert attribution["phase_sec"]["wasm_reloc_preflight"] == 0.75
    assert attribution["phase_share"]["backend_codegen"] == 0.4
    assert attribution["phase_sec"]["frontend_lowering"] == 2.0
    assert attribution["ranked_phases"][0] == "backend_codegen"


def test_wasm_link_operation_counts_preserve_all_build_time_rungs() -> None:
    assert build_diagnostics._wasm_link_operation_counts(
        {
            "split_app_optimize_requests": 1,
            "wasm_reloc_preflight_invocations": 1.0,
            "wasm_reloc_preflight": 750.0,
            "split_app_wasm_opt_runs": 0,
            "runtime_tree_shake_cache_hits": 1.0,
            "runtime_tree_shake_cache_wall_ms": 0.25,
            "wasm_whole_artifact_full_binary_parses": 17,
            "wasm_whole_artifact_section_walks": 29,
            "wasm_whole_artifact_reserializations": 11,
            "wasm_whole_artifact_redundant_parses_eliminated": 6,
            "split_runtime_processing": 1000.0,
        }
    ) == {
        "split_app_optimize_requests": 1,
        "wasm_reloc_preflight_invocations": 1,
        "split_app_wasm_opt_runs": 0,
        "runtime_tree_shake_cache_hits": 1,
        "wasm_whole_artifact_full_binary_parses": 17,
        "wasm_whole_artifact_redundant_parses_eliminated": 6,
        "wasm_whole_artifact_reserializations": 11,
        "wasm_whole_artifact_section_walks": 29,
    }


@pytest.mark.parametrize("optimizer_wall_ms", [None, 0.0, 1250.0])
def test_optimizer_phase_is_observed_and_subtracted_once(
    optimizer_wall_ms: float | None,
) -> None:
    stages = {
        "wasm_link_total": 6000.0,
        "split_runtime_processing": 1000.0,
        "wasm_strip": 500.0,
        "fail_closed_validation": 500.0,
        # A split cache hit has its own current lookup cost and does not replay
        # the original optimizer cost into either the combined or split phase.
        "split_app_optimize_cache_hits": 1.0,
        "split_app_optimize_cache_wall_ms": 10.0,
    }
    if optimizer_wall_ms is not None:
        stages["wasm_link_optimizer_wall_ms"] = optimizer_wall_ms
    attribution = build_diagnostics._build_phase_attribution(
        total_sec=20.0, phase_sec={"wasm_link": 6.0}, pipeline_stage_ms=stages
    )
    phases = attribution["phase_sec"]
    if optimizer_wall_ms is None:
        assert "wasm_link_optimizer" not in phases
        assert phases["wasm_link_core"] == 4.0
    else:
        assert phases["wasm_link_optimizer"] == optimizer_wall_ms / 1000.0
        assert phases["wasm_link_core"] == 4.0 - optimizer_wall_ms / 1000.0
    assert phases["split_runtime_processing"] == 1.0


def test_optimizer_cost_reaches_cli_json_without_entering_operation_wall_counts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(build_diagnostics.time, "perf_counter", lambda: 20.0)
    context = _BuildDiagnosticsContext(
        diagnostics_enabled=True,
        diagnostics_start=0.0,
        phase_starts={"wasm_link": 10.0},
        image_scope=None,
        binary_image_closure=None,
        binary_image_analysis=None,
        module_graph={},
        module_graph_operation_counts={},
        module_reasons={},
        frontend_module_timings=(),
        allocation_diagnostics_enabled=False,
        frontend_parallel_details={
            "pipeline_stage_ms": {
                "wasm_link_total": 6000.0,
                "wasm_link_optimizer_requests": 1,
                "wasm_link_optimizer_wall_ms": 1250.0,
                "wasm_link_optimizer_peak_rss_kb": 800,
                "wasm_link_optimizer_peak_total_rss_kb": 1200,
                "wasm_link_timeouts": 1,
                "wasm_link_failures": 1,
                "wasm_link_identity_errors": 1,
            }
        },
        profile="dev",
        midend_policy_outcomes_by_function={},
        midend_pass_stats_by_function={},
        backend_daemon_health=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_config_digest=None,
        diagnostics_path_spec="diagnostics.json",
        artifacts_root=tmp_path,
        entry_module="fixture",
        entry_path=tmp_path / "fixture.py",
    )
    payload, path = build_diagnostics._build_build_diagnostics_payload(context)
    build_diagnostics._emit_build_diagnostics(
        diagnostics=payload, diagnostics_path=path, json_output=True
    )
    emitted = json.loads((tmp_path / "diagnostics.json").read_text())
    assert emitted["phase_attribution"]["phase_sec"]["wasm_link_optimizer"] == 1.25
    assert emitted["phase_attribution"]["phase_sec"]["wasm_link_core"] == 4.75
    assert emitted["wasm_link_operation_counts"] == {
        "wasm_link_optimizer_requests": 1,
        "wasm_link_optimizer_peak_rss_kb": 800,
        "wasm_link_optimizer_peak_total_rss_kb": 1200,
        "wasm_link_timeouts": 1,
        "wasm_link_failures": 1,
        "wasm_link_identity_errors": 1,
    }


@pytest.mark.parametrize("split", [False, True])
def test_final_link_replay_excludes_historical_optimizer_cost(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, split: bool
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    produced = wasm_link_output_paths(
        tmp_path / "old" / "linked.wasm",
        split_output_dir=tmp_path / "old" if split else None,
        optimize=True,
    )
    historical_timings = tmp_path / "old" / "link-timings.json"
    for role, path in produced.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(role.encode())
    historical_timings.write_text('{"wasm_link_optimizer_wall_ms": 999000}')
    assert historical_timings not in produced.values()
    entry = wasm_final_link_cache.final_link_cache_entry("f" * 64)
    wasm_final_link_cache.publish_final_link_result(entry, produced)
    restored = wasm_link_output_paths(
        tmp_path / "current" / "linked.wasm",
        split_output_dir=tmp_path / "current" if split else None,
        optimize=True,
    )
    assert wasm_final_link_cache.restore_final_link_result(entry, restored)
    assert {role: path.read_bytes() for role, path in restored.items()} == {
        role: path.read_bytes() for role, path in produced.items()
    }
    assert not (tmp_path / "current" / historical_timings.name).exists()
    attribution = build_diagnostics._build_phase_attribution(
        total_sec=1.0,
        phase_sec={"wasm_link": 0.1},
        pipeline_stage_ms={"wasm_link_result_cache_hits": 1},
    )
    assert "wasm_link_optimizer" not in attribution["phase_sec"]


def test_warm_cache_phases_keep_runtime_and_backend_work_out_of_frontend():
    phases = build_diagnostics._phase_duration_map(
        {
            "resolve_entry": 0.0,
            "module_graph": 0.25,
            "module_analysis": 1.0,
            "ir_lowering": 2.0,
            "backend_prepare": 4.0,
            "cache_lookup": 40.0,
            "runtime_setup": 40.25,
        },
        ended_at=52.0,
    )
    attribution = build_diagnostics._build_phase_attribution(
        total_sec=52.0, phase_sec=phases, pipeline_stage_ms={}
    )
    actual = attribution["phase_sec"]
    assert actual["frontend_lowering"] == 4.0
    assert actual["ir_lowering"] == 2.0
    assert actual["backend_prepare"] == 36.0
    assert actual["runtime_setup"] == 11.75
    assert actual["backend_codegen"] == 0.0
    assert attribution["ranked_phases"][0] == "backend_prepare"


def test_phase_attribution_preserves_observed_seal_duration() -> None:
    attribution = build_diagnostics._build_phase_attribution(
        total_sec=20.0, phase_sec={"link": 5.0, "seal": 7.0}, pipeline_stage_ms={}
    )
    assert attribution["phase_sec"]["seal"] == 7.0
    assert attribution["phase_share"]["seal"] == 0.35


def test_a_directory_spec_gives_each_build_its_own_diagnostics_file(
    tmp_path: Path,
) -> None:
    from molt.cli.build_diagnostics import _resolve_build_diagnostics_path

    first = _resolve_build_diagnostics_path("diagnostics/", tmp_path)
    second = _resolve_build_diagnostics_path("diagnostics/", tmp_path)

    assert first.parent == second.parent == tmp_path / "diagnostics"
    assert first.parent.is_dir()
    assert first != second
    # Without the trailing separator the spec stays one file, as before.
    assert (
        _resolve_build_diagnostics_path("one.json", tmp_path) == tmp_path / "one.json"
    )
