from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli import frontend_parallel

_FRONTEND_PARALLEL_NAMES = (
    "_append_frontend_parallel_layer_detail",
    "_append_frontend_serial_disabled_layer_detail",
    "_choose_frontend_parallel_layer_workers",
    "_fallback_frontend_parallel_layer_to_serial",
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
