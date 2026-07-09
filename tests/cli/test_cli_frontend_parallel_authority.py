from __future__ import annotations

import inspect
from pathlib import Path

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli import frontend_parallel

_FRONTEND_PARALLEL_NAMES = (
    "_append_frontend_parallel_layer_detail",
    "_append_frontend_serial_disabled_layer_detail",
    "_choose_frontend_parallel_layer_workers",
    "_fresh_frontend_parallel_layer_state",
    "_frontend_layer_plan",
    "_frontend_layer_policy_summary",
    "_frontend_layer_static_metrics",
    "_frontend_parallel_layer_detail",
    "_frontend_parallel_policy_payload",
    "_frontend_parallel_result_error",
    "_frontend_parallel_worker_timing_inputs",
    "_frontend_result_timings",
    "_frontend_serial_worker_mode",
    "_initialize_frontend_parallel_details",
    "_known_classes_snapshot_copy",
    "_layer_cache_hit_count",
    "_note_frontend_parallel_layer_failure",
    "_predict_frontend_module_cost",
    "_record_parallel_cached_module_result",
    "_record_parallel_layer_module_timing",
    "_record_parallel_worker_result",
    "_record_serial_frontend_worker_timing",
    "_resolve_frontend_parallel_config",
    "_resolve_frontend_parallel_min_modules",
    "_resolve_frontend_parallel_min_predicted_cost",
    "_resolve_frontend_parallel_module_workers",
    "_resolve_frontend_parallel_stdlib_min_cost_scale",
    "_resolve_frontend_parallel_target_cost_per_worker",
    "_summarize_frontend_parallel_worker_timings",
    "_summarize_worker_timing_items",
    "_take_frontend_parallel_layer_result",
    "_worker_timing_summary_payload",
)

_FRONTEND_PARALLEL_DEFINITIONS = tuple(
    f"def {name}(" for name in _FRONTEND_PARALLEL_NAMES
)


def test_cli_frontend_parallel_authority_is_single_home() -> None:
    for name in _FRONTEND_PARALLEL_NAMES:
        assert hasattr(frontend_parallel, name)
        assert not hasattr(frontend_execution, name)
        assert not hasattr(cli, name)

    frontend_execution_source = inspect.getsource(frontend_execution)
    cli_source = inspect.getsource(cli)
    for marker in _FRONTEND_PARALLEL_DEFINITIONS:
        assert marker not in frontend_execution_source
        assert marker not in cli_source


def test_serial_cache_hit_counts_as_layer_cache_hit(tmp_path: Path) -> None:
    recorded: list[dict[str, object]] = []

    def record_worker_timing(**kwargs: object) -> dict[str, object]:
        return dict(kwargs)

    frontend_parallel._record_serial_frontend_worker_timing(
        record_frontend_parallel_worker_timing=record_worker_timing,
        recorded_worker_timings=recorded,
        layer_index=0,
        module_name="cached",
        module_path=tmp_path / "cached.py",
        mode="serial_cache_hit",
        total_s=0.0,
        reused_s=2.5,
    )

    assert recorded[0]["mode"] == "serial_cache_hit"
    assert recorded[0]["reused_ms"] == 2500.0
    assert frontend_parallel._layer_cache_hit_count(recorded) == 1
