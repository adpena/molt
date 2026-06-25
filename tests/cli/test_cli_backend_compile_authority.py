from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import backend_compile
from molt.cli import build_pipeline

_BACKEND_COMPILE_NAMES = (
    "_execute_backend_compile",
    "_prepare_backend_compile",
    "_prepare_backend_dispatch",
    "_prepare_backend_runtime_context",
    "_prepare_backend_setup",
)


def test_cli_backend_compile_authority_is_single_home() -> None:
    for name in _BACKEND_COMPILE_NAMES:
        assert hasattr(backend_compile, name)
        assert not hasattr(build_pipeline, name), name
        assert not hasattr(cli, name)

    build_pipeline_source = inspect.getsource(build_pipeline)
    cli_source = inspect.getsource(cli)
    for name in _BACKEND_COMPILE_NAMES:
        assert f"def {name}(" not in build_pipeline_source
        assert f"def {name}(" not in cli_source
