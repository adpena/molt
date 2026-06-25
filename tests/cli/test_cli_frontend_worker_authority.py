from __future__ import annotations

import inspect
import importlib

import molt.cli as cli
from molt.cli import frontend_execution

frontend_worker = importlib.import_module("molt.cli.frontend_worker")

_FRONTEND_WORKER_NAMES = (
    "_format_syntax_error_message",
    "_frontend_lower_module_worker",
    "_lower_module_serial_with_context",
    "_module_frontend_generator",
    "_module_frontend_payload",
    "_phase_timeout",
    "_prepare_frontend_parallel_batch",
    "_read_worker_source_lease",
    "_resolve_tree_for_serial_frontend_module",
    "_run_serial_frontend_lower_with_context",
    "_syntax_error_stub_ast",
)

_FRONTEND_WORKER_DEFINITIONS = tuple(
    f"def {name}(" for name in _FRONTEND_WORKER_NAMES
)


def test_cli_frontend_worker_authority_is_single_home() -> None:
    for name in _FRONTEND_WORKER_NAMES:
        assert hasattr(frontend_worker, name), name
        assert not hasattr(frontend_execution, name), name
        assert not hasattr(cli, name), name

    frontend_execution_source = inspect.getsource(frontend_execution)
    cli_source = inspect.getsource(cli)
    for marker in _FRONTEND_WORKER_DEFINITIONS:
        assert marker not in frontend_execution_source
        assert marker not in cli_source
