from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import frontend_execution
from molt.cli import frontend_worker

_FRONTEND_EXECUTION_NAMES = (
    "_consume_frontend_module_result",
    "_consume_frontend_parallel_layer_result",
    "_consume_frontend_serial_layer_result",
    "_lower_entry_module_as_main",
    "_prepare_frontend_execution",
    "_run_frontend_layer",
    "_run_frontend_parallel_enabled_layers",
    "_run_frontend_parallel_layer_batches",
    "_run_frontend_pipeline",
    "_run_frontend_serial_disabled_layers",
    "_run_frontend_serial_layer_modules",
    "_write_parallel_persisted_module_lowering",
)


def test_cli_frontend_execution_authority_is_single_home() -> None:
    for name in _FRONTEND_EXECUTION_NAMES:
        assert hasattr(frontend_execution, name)
        assert not hasattr(cli, name)
        assert not hasattr(frontend_worker, name)

    cli_source = inspect.getsource(cli)
    frontend_worker_source = inspect.getsource(frontend_worker)
    for name in _FRONTEND_EXECUTION_NAMES:
        assert f"def {name}(" not in cli_source
        assert f"def {name}(" not in frontend_worker_source
