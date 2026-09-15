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


def test_backend_compiler_fingerprint_is_exact_without_mutating_ambient_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    name = "MOLT_BACKEND_COMPILER_FINGERPRINT"
    monkeypatch.setenv(name, "stale")
    source_env = {name: "stale", "unrelated": "preserved"}
    child_env = backend_compile._backend_environment_with_compiler_fingerprint(
        source_env, "compiler-build-v2"
    )
    assert os.environ[name] == "stale"
    assert source_env == {name: "stale", "unrelated": "preserved"}
    assert child_env[name] == "compiler-build-v2"
    cleared_env = backend_compile._backend_environment_with_compiler_fingerprint(
        child_env, None
    )
    assert cleared_env == {"unrelated": "preserved"}
    assert child_env[name] == "compiler-build-v2"
    assert os.environ[name] == "stale"
