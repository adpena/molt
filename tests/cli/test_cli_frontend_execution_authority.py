from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import frontend_execution

_FRONTEND_EXECUTION_NAMES = (
    "_accumulate_midend_diagnostics_with_state",
    "_consume_frontend_module_result",
    "_consume_frontend_parallel_layer_result",
    "_consume_frontend_serial_layer_result",
    "_format_syntax_error_message",
    "_frontend_lower_module_worker",
    "_integrate_module_frontend_result_with_state",
    "_known_classes_snapshot_copy",
    "_lower_entry_module_as_main",
    "_lower_module_serial_with_context",
    "_module_frontend_generator",
    "_module_frontend_payload",
    "_phase_timeout",
    "_prepare_frontend_execution",
    "_prepare_frontend_parallel_batch",
    "_read_worker_source_lease",
    "_register_global_code_id_with_state",
    "_remap_module_code_ops_with_state",
    "_resolve_tree_for_serial_frontend_module",
    "_run_frontend_layer",
    "_run_frontend_parallel_enabled_layers",
    "_run_frontend_parallel_layer_batches",
    "_run_frontend_pipeline",
    "_run_frontend_serial_disabled_layers",
    "_run_frontend_serial_layer_modules",
    "_run_serial_frontend_lower_with_context",
    "_syntax_error_stub_ast",
    "_write_parallel_persisted_module_lowering",
)


def test_cli_frontend_execution_authority_is_single_home() -> None:
    for name in _FRONTEND_EXECUTION_NAMES:
        assert getattr(cli, name) is getattr(frontend_execution, name)

    cli_source = inspect.getsource(cli)
    for name in _FRONTEND_EXECUTION_NAMES:
        assert f"def {name}(" not in cli_source
