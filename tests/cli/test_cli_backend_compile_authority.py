from __future__ import annotations

import inspect
import os

import pytest

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


def test_backend_compiler_fingerprint_is_exact_in_process_and_child_environments(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "MOLT_BACKEND_COMPILER_FINGERPRINT"
    monkeypatch.setenv(name, "stale")
    child_env = {name: "stale"}

    backend_compile._apply_backend_compiler_fingerprint(
        "compiler-build-v2", backend_env=child_env
    )
    assert os.environ[name] == "compiler-build-v2"
    assert child_env[name] == "compiler-build-v2"

    backend_compile._apply_backend_compiler_fingerprint(None, backend_env=child_env)
    assert name not in os.environ
    assert name not in child_env
