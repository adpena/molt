from __future__ import annotations

import inspect
import os

import pytest

import molt.cli as cli
from molt.cli import backend_compile, backend_execution
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
    child_env = backend_execution._backend_environment_with_compiler_fingerprint(
        source_env, "compiler-build-v2"
    )
    assert os.environ[name] == "stale"
    assert source_env == {name: "stale", "unrelated": "preserved"}
    assert child_env[name] == "compiler-build-v2"
    cleared_env = backend_execution._backend_environment_with_compiler_fingerprint(
        child_env, None
    )
    assert cleared_env == {"unrelated": "preserved"}
    assert child_env[name] == "compiler-build-v2"
    assert os.environ[name] == "stale"


@pytest.mark.parametrize("capture", [False, True])
@pytest.mark.parametrize("scope", [None, "a" * 64, "invalid"])
def test_capture_scope_is_explicit_and_never_a_semantic_codegen_input(
    tmp_path, monkeypatch, capture, scope
):
    from molt.backend_environment import (
        CodegenSelection,
        codegen_environment_inputs,
        native_object_environment_inputs,
    )
    from molt.cli.rust_cpu_policy import RustCpuPolicy
    from tests.cli.native_link_test_support import (
        RUNTIME_BUILD_IDENTITY,
        native_codegen_binding,
        write_test_static_archive,
    )

    if scope is None:
        monkeypatch.delenv("MOLT_EMISSION_SCOPE", raising=False)
    else:
        monkeypatch.setenv("MOLT_EMISSION_SCOPE", scope)
    runtime = tmp_path / "runtime.a"
    write_test_static_archive(runtime)
    binding = native_codegen_binding(runtime, RUNTIME_BUILD_IDENTITY)
    supplied = {"MOLT_EMISSION_SCOPE": "b" * 64}
    dispatch, error = backend_compile._prepare_backend_dispatch(
        capture_origin=capture,
        is_rust_transpile=False,
        is_wasm=False,
        wasm_layout=None,
        backend_environment=supplied,
        deterministic=False,
        profile="dev",
        cargo_timeout=None,
        molt_root=tmp_path,
        target_triple=None,
        backend_cargo_profile="dev",
        diagnostics_enabled=False,
        phase_starts={},
        json_output=True,
        backend_daemon_config_digest=None,
        warnings=[],
        codegen=CodegenSelection(),
        rust_cpu=RustCpuPolicy.BASELINE,
        backend_bin=tmp_path / "backend",
        start_daemon=False,
        native_runtime_codegen_binding=binding,
    )
    assert supplied == {"MOLT_EMISSION_SCOPE": "b" * 64}
    if capture and scope == "invalid":
        assert error is not None and dispatch is None
        return
    assert error is None and dispatch is not None
    assert dispatch.backend_env.get("MOLT_EMISSION_SCOPE") == (
        scope if capture else None
    )
    baseline = dict(dispatch.backend_env)
    baseline.pop("MOLT_EMISSION_SCOPE", None)
    for is_wasm in (False, True):
        assert codegen_environment_inputs(
            is_wasm=is_wasm, env=dispatch.backend_env
        ) == codegen_environment_inputs(is_wasm=is_wasm, env=baseline)
    assert native_object_environment_inputs(
        dispatch.backend_env
    ) == native_object_environment_inputs(baseline)
