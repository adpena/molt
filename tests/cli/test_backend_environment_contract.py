from pathlib import Path

import pytest

from molt.backend_environment import (
    codegen_environment_inputs,
    compilation_diagnostics_requested,
    environment_keys,
)
from molt.cli import backend_cache
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
)
from molt.cli.backend_execution import _backend_codegen_env_digest

# These cases build synthetic projects and assert developer-host roots;
# hosted custody has its own cases in tests/test_dx_run_context.py.
pytestmark = pytest.mark.usefixtures("developer_host_context")


def test_emitter_and_presence_gated_pass_inputs_invalidate_frontend_identity():
    baseline = _backend_codegen_env_digest(is_wasm=False, env={})
    for name, value in (
        ("MOLT_BACKEND_INLINE_EXC_DISABLED", "1"),
        ("MOLT_DISABLE_RC_COALESCE", ""),
        ("MOLT_DISABLE_METHOD_FUSION", "1"),
        ("MOLT_DISABLE_EXC_ELIDE", "1"),
    ):
        assert _backend_codegen_env_digest(is_wasm=False, env={name: value}) != baseline
        assert (
            codegen_environment_inputs(is_wasm=False, env={name: value})[name] == value
        )
        assert name in environment_keys("common", "native")
    assert "MOLT_DISABLE_RC_COALESCING" not in environment_keys("common", "native")


def test_wasm_consumers_share_the_same_catalog():
    for name in ("MOLT_WASM_TAIL_CALL", "MOLT_WASM_NATIVE_EH", "MOLT_WASM_PROFILE"):
        assert _backend_codegen_env_digest(
            is_wasm=True, env={name: "0"}
        ) != _backend_codegen_env_digest(is_wasm=True, env={})


def test_compilation_diagnostic_presence_and_observation_policy():
    assert not compilation_diagnostics_requested({})
    for name in environment_keys("diagnostic"):
        for value in ("", "0", "1"):
            assert compilation_diagnostics_requested({name: value})
    assert not compilation_diagnostics_requested(
        {name: "1" for name in environment_keys("observation", "resource")}
    )


@pytest.mark.parametrize("tier", ["module", "function"])
@pytest.mark.parametrize("synced_output", [False, True])
def test_compilation_diagnostics_bypass_backend_candidates_and_synced_outputs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    tier: str,
    synced_output: bool,
):
    for name in environment_keys("diagnostic", "observation"):
        monkeypatch.delenv(name, raising=False)
    # WASM reuse also needs the provisioned structural validator, which a
    # build-free unit cell lacks; this case checks only the diagnostics bypass.
    monkeypatch.setattr(
        "molt.cli.runtime_wasm_validation._reusable_wasm_artifact_validation_error",
        lambda _path: None,
    )
    candidate = tmp_path / "cached.wasm"
    wasm_bytes = b"\x00asm\x01\x00\x00\x00"
    candidate.write_bytes(wasm_bytes)
    output = tmp_path / "output.wasm"
    contract = BackendArtifactContract(BackendArtifactKind.WASM)

    def admit():
        return backend_cache._try_cached_backend_candidates(
            project_root=tmp_path,
            cache_candidates=[(tier, candidate)],
            output_artifact=output,
            artifact_contract=contract,
            cache_key="module-key" if tier == "module" else None,
            function_cache_key="function-key",
            cache_path=None,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            warnings=[],
        )

    if synced_output:
        assert admit() == (True, tier)
        assert output.read_bytes() == wasm_bytes
        candidate.unlink()
    for value in ("", "1"):
        monkeypatch.setenv("MOLT_DUMP_IR", value)
        assert admit() == (False, None)
        assert output.exists() == synced_output
    monkeypatch.delenv("MOLT_DUMP_IR")
    monkeypatch.setenv("MOLT_BACKEND_TIMING", "1")
    assert admit() == (True, tier)
    assert output.read_bytes() == wasm_bytes


def test_diagnostics_do_not_bypass_backend_transport_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("MOLT_DUMP_IR", "")
    with pytest.raises(ValueError, match="Shared stdlib extraction requires"):
        backend_cache._try_cached_backend_candidates(
            project_root=tmp_path,
            cache_candidates=[],
            output_artifact=tmp_path / "output.wasm",
            artifact_contract=BackendArtifactContract(BackendArtifactKind.WASM),
            cache_key=None,
            function_cache_key=None,
            cache_path=None,
            stdlib_object_path=tmp_path / "stdlib.a",
            stdlib_object_cache_key=None,
            warnings=[],
        )
