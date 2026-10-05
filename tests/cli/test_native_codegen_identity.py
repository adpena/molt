from __future__ import annotations

import json
import subprocess

import pytest

from molt.cli import backend_execution, backend_cache_setup
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.models import _ModuleGraphMetadata
from molt.target_python import TargetPythonVersion


TARGET = "x86_64-unknown-linux-gnu"


def test_baseline_identity_never_starts_a_backend(monkeypatch, tmp_path):
    def unexpected(*args, **kwargs):
        raise AssertionError("portable admission must be pure")

    monkeypatch.setattr(backend_execution, "_run_completed_command", unexpected)
    identities = {
        backend_execution._backend_native_codegen_identity(
            tmp_path / "backend",
            backend_identity="compiler",
            target_triple=TARGET,
            env=env,
        )
        for env in ({}, {"MOLT_PORTABLE": "1"}, {"MOLT_PORTABLE": "true"})
    }
    assert len(identities) == 1


def test_host_codegen_memo_binds_compiler_target_and_environment(monkeypatch, tmp_path):
    backend_execution._observed_native_codegen_identity.cache_clear()
    seen = []

    def query(command, **kwargs):
        seen.append((command, kwargs["env"]))
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(
                {
                    "schema": "molt.native-codegen-identity.v1",
                    "requested_target": command[-1],
                    "identity": {"isa_flags": {"has_avx2": "true"}},
                }
            ),
            "",
        )

    monkeypatch.setattr(backend_execution, "_run_completed_command", query)

    def identity(compiler="compiler-a", target=TARGET, extra="a"):
        return backend_execution._backend_native_codegen_identity(
            tmp_path / "backend",
            backend_identity=compiler,
            target_triple=target,
            env={"MOLT_PORTABLE": "0", "MOLT_BACKEND_OPT_LEVEL": extra},
        )

    assert identity() == identity()
    assert len(seen) == 1
    identity(compiler="compiler-b")
    identity(target="aarch64-apple-darwin")
    identity(extra="b")
    assert len(seen) == 4


@pytest.mark.parametrize(
    "payload",
    [
        {},
        {
            "schema": "molt.native-codegen-identity.v1",
            "requested_target": "wrong",
            "identity": {"isa_flags": {}},
        },
    ],
)
def test_host_codegen_rejects_unbound_identity(monkeypatch, tmp_path, payload):
    backend_execution._observed_native_codegen_identity.cache_clear()
    monkeypatch.setattr(
        backend_execution,
        "_run_completed_command",
        lambda command, **kwargs: subprocess.CompletedProcess(
            command, 0, json.dumps(payload), ""
        ),
    )
    with pytest.raises(ValueError, match="does not match"):
        backend_execution._backend_native_codegen_identity(
            tmp_path / "backend",
            backend_identity="compiler",
            target_triple=TARGET,
            env={"MOLT_PORTABLE": "0"},
        )


def test_effective_cpu_features_separate_all_outer_native_cache_keys(
    monkeypatch, tmp_path
):
    backend = tmp_path / "backend"
    backend.write_bytes(b"fixed compiler content")
    effective = "baseline"
    monkeypatch.setattr(
        backend_cache_setup,
        "_backend_native_codegen_identity",
        lambda *a, **k: effective,
    )
    metadata = _ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        module_execution_kind_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    contract = resolve_backend_artifact_contract(target="native", emit_mode="bin")

    def setup():
        return backend_cache_setup._prepare_backend_cache_setup(
            backend_bin=backend,
            cache_enabled=True,
            ir={"functions": [], "module": "__main__", "ops": []},
            target="native",
            artifact_contract=contract,
            profile="dev",
            runtime_cargo_profile="dev-fast",
            backend_cargo_profile="dev-fast",
            emit_mode="bin",
            is_wasm=False,
            linked=False,
            project_root=tmp_path,
            cache_dir=str(tmp_path / "cache"),
            output_artifact=tmp_path / "app.a",
            warnings=[],
            entry_module="__main__",
            module_graph_metadata=metadata,
            target_python=TargetPythonVersion(3, 12, 0),
            backend_compiler_fingerprint="compiler-source",
        )

    baseline = setup()
    effective = "host-avx2"
    specialized = setup()
    for name in ("cache_key", "function_cache_key", "stdlib_object_cache_key"):
        assert getattr(baseline, name) is not None
        assert getattr(baseline, name) != getattr(specialized, name)
