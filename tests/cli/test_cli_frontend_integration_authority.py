from __future__ import annotations

import importlib
import inspect

import molt.cli as cli
from molt.cli import frontend_execution

frontend_integration = importlib.import_module("molt.cli.frontend_integration")
frontend_worker = importlib.import_module("molt.cli.frontend_worker")

_FRONTEND_INTEGRATION_NAMES = (
    "_accumulate_midend_diagnostics_with_state",
    "_integrate_module_frontend_result_with_state",
    "_register_global_code_id_with_state",
    "_remap_module_code_ops_with_state",
)

_FRONTEND_INTEGRATION_DEFINITIONS = tuple(
    f"def {name}(" for name in _FRONTEND_INTEGRATION_NAMES
)


def test_cli_frontend_integration_authority_is_single_home() -> None:
    for name in _FRONTEND_INTEGRATION_NAMES:
        assert hasattr(frontend_integration, name), name
        assert not hasattr(frontend_execution, name), name
        assert not hasattr(frontend_worker, name), name
        assert not hasattr(cli, name), name

    frontend_execution_source = inspect.getsource(frontend_execution)
    frontend_worker_source = inspect.getsource(frontend_worker)
    cli_source = inspect.getsource(cli)
    for marker in _FRONTEND_INTEGRATION_DEFINITIONS:
        assert marker not in frontend_execution_source
        assert marker not in frontend_worker_source
        assert marker not in cli_source
