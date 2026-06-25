from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_diagnostics

_BUILD_DIAGNOSTICS_NAMES = (
    "_build_allocation_diagnostics_enabled",
    "_build_build_diagnostics_payload",
    "_build_diagnostics_enabled",
    "_build_midend_diagnostics_payload",
    "_build_reason_summary",
    "_capture_build_allocation_diagnostics",
    "_duration_ms_from_ns",
    "_emit_build_diagnostics",
    "_emit_build_diagnostics_if_present",
    "_midend_policy_config_snapshot",
    "_midend_sample_p95",
    "_midend_sample_percentile",
    "_normalize_midend_pass_stat",
    "_phase_duration_map",
    "_record_frontend_timing_item",
    "_resolve_build_diagnostics_path",
    "_resolve_build_diagnostics_verbosity",
)

_BUILD_DIAGNOSTICS_DEFINITIONS = (
    "def _build_allocation_diagnostics_enabled(",
    "def _build_build_diagnostics_payload(",
    "def _build_diagnostics_enabled(",
    "def _build_midend_diagnostics_payload(",
    "def _build_reason_summary(",
    "def _capture_build_allocation_diagnostics(",
    "def _duration_ms_from_ns(",
    "def _emit_build_diagnostics(",
    "def _emit_build_diagnostics_if_present(",
    "def _midend_policy_config_snapshot(",
    "def _midend_sample_p95(",
    "def _midend_sample_percentile(",
    "def _normalize_midend_pass_stat(",
    "def _phase_duration_map(",
    "def _record_frontend_timing_item(",
    "def _resolve_build_diagnostics_path(",
    "def _resolve_build_diagnostics_verbosity(",
)


def test_cli_build_diagnostics_authority_is_single_home() -> None:
    for name in _BUILD_DIAGNOSTICS_NAMES:
        assert hasattr(build_diagnostics, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for marker in _BUILD_DIAGNOSTICS_DEFINITIONS:
        assert marker not in cli_source
