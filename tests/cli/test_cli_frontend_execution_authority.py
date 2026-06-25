from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import frontend_execution

_FRONTEND_EXECUTION_NAMES = (
    "_accumulate_midend_diagnostics_with_state",
    "_append_frontend_parallel_layer_detail",
    "_append_frontend_serial_disabled_layer_detail",
    "_choose_frontend_parallel_layer_workers",
    "_consume_frontend_module_result",
    "_consume_frontend_parallel_layer_result",
    "_consume_frontend_serial_layer_result",
    "_fallback_frontend_parallel_layer_to_serial",
    "_format_syntax_error_message",
    "_fresh_frontend_parallel_layer_state",
    "_frontend_layer_plan",
    "_frontend_layer_policy_summary",
    "_frontend_layer_static_metrics",
    "_frontend_lower_module_worker",
    "_frontend_parallel_layer_detail",
    "_frontend_parallel_policy_payload",
    "_frontend_parallel_result_error",
    "_frontend_parallel_worker_timing_inputs",
    "_frontend_result_timings",
    "_frontend_serial_worker_mode",
    "_initialize_frontend_parallel_details",
    "_integrate_module_frontend_result_with_state",
    "_known_classes_snapshot_copy",
    "_layer_cache_hit_count",
    "_lower_entry_module_as_main",
    "_lower_module_serial_with_context",
    "_module_frontend_generator",
    "_module_frontend_payload",
    "_phase_timeout",
    "_predict_frontend_module_cost",
    "_prepare_frontend_execution",
    "_prepare_frontend_parallel_batch",
    "_read_worker_source_lease",
    "_record_parallel_cached_module_result",
    "_record_parallel_layer_module_timing",
    "_record_parallel_worker_result",
    "_record_serial_frontend_worker_timing",
    "_register_global_code_id_with_state",
    "_remap_module_code_ops_with_state",
    "_resolve_frontend_parallel_config",
    "_resolve_frontend_parallel_min_modules",
    "_resolve_frontend_parallel_min_predicted_cost",
    "_resolve_frontend_parallel_module_workers",
    "_resolve_frontend_parallel_stdlib_min_cost_scale",
    "_resolve_frontend_parallel_target_cost_per_worker",
    "_resolve_tree_for_serial_frontend_module",
    "_run_frontend_layer",
    "_run_frontend_parallel_enabled_layers",
    "_run_frontend_parallel_layer_batches",
    "_run_frontend_pipeline",
    "_run_frontend_serial_disabled_layers",
    "_run_frontend_serial_layer_modules",
    "_run_serial_frontend_lower_with_context",
    "_summarize_frontend_parallel_worker_timings",
    "_summarize_worker_timing_items",
    "_syntax_error_stub_ast",
    "_take_frontend_parallel_layer_result",
    "_worker_timing_summary_payload",
    "_write_parallel_persisted_module_lowering",
)


def test_cli_frontend_execution_authority_is_single_home() -> None:
    for name in _FRONTEND_EXECUTION_NAMES:
        assert getattr(cli, name) is getattr(frontend_execution, name)

    cli_source = inspect.getsource(cli)
    for name in _FRONTEND_EXECUTION_NAMES:
        assert f"def {name}(" not in cli_source
