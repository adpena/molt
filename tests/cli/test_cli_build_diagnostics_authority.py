from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_diagnostics

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
    assert attribution["phase_share"]["backend_codegen"] == 0.4
    assert attribution["phase_sec"]["frontend_lowering"] == 2.0
    assert attribution["ranked_phases"][0] == "backend_codegen"


def test_wasm_link_operation_counts_preserve_all_build_time_rungs() -> None:
    assert build_diagnostics._wasm_link_operation_counts(
        {
            "split_app_optimize_requests": 1,
            "split_app_wasm_opt_runs": 0,
            "wasm_whole_artifact_full_binary_parses": 17,
            "wasm_whole_artifact_section_walks": 29,
            "wasm_whole_artifact_reserializations": 11,
            "wasm_whole_artifact_redundant_parses_eliminated": 6,
            "split_runtime_processing": 1000.0,
        }
    ) == {
        "split_app_optimize_requests": 1,
        "split_app_wasm_opt_runs": 0,
        "wasm_whole_artifact_full_binary_parses": 17,
        "wasm_whole_artifact_redundant_parses_eliminated": 6,
        "wasm_whole_artifact_reserializations": 11,
        "wasm_whole_artifact_section_walks": 29,
    }
