from __future__ import annotations

import json
from pathlib import Path

import pytest

from molt.cli import backend_cache, backend_execution, build_output_layout, factgraph
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.native_link_plan import NativeArtifactKind


@pytest.mark.parametrize("kind", list(NativeArtifactKind))
@pytest.mark.parametrize("probe", [False, True])
def test_native_output_kind_is_explicit_in_full_and_probe_requests(kind, probe):
    contract = resolve_backend_artifact_contract(
        target="native",
        emit_mode="obj" if kind is NativeArtifactKind.OBJECT else "bin",
        target_triple="x86_64-pc-windows-msvc",
    )
    stdlib_key = "stdlib-key" if kind is NativeArtifactKind.ARCHIVE else None
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir=None if probe else {"functions": []},
        backend_output=Path("arbitrary.filename"),
        artifact_contract=contract,
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        cache_key="same-key",
        function_cache_key="function-key",
        stdlib_object_cache_key=stdlib_key,
        config_digest="config",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        probe_cache_only=probe,
    )
    assert error is None
    assert payload is not None
    job = json.loads(payload)["jobs"][0]
    assert job["native_output_kind"] == kind.value
    assert job["target_triple"] == contract.target_triple
    for field, base in (
        ("cache_key", "same-key"),
        ("function_cache_key", "function-key"),
    ):
        assert job[field] == backend_cache._backend_artifact_source_key(
            base, artifact_contract=contract, stdlib_object_cache_key=stdlib_key
        )


@pytest.mark.parametrize("target", ["wasm", "wasm-freestanding"])
@pytest.mark.parametrize("probe", [False, True])
def test_wasm_requests_omit_native_output_kind(target: str, probe: bool) -> None:
    contract = resolve_backend_artifact_contract(
        target=target, emit_mode="wasm", target_triple=None
    )
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir=None if probe else {"functions": []},
        backend_output=Path("output.wasm"),
        artifact_contract=contract,
        wasm_link=True,
        wasm_data_base=1024,
        wasm_table_base=8,
        cache_key="same-key",
        function_cache_key="function-key",
        config_digest="config",
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        probe_cache_only=probe,
    )
    assert error is None
    assert payload is not None
    job = json.loads(payload)["jobs"][0]
    assert "native_output_kind" not in job
    assert job["is_wasm"] is True
    assert job["target_triple"] == (
        "wasm32-unknown-unknown" if target == "wasm-freestanding" else "wasm32-wasip1"
    )
    for field, base in (
        ("cache_key", "same-key"),
        ("function_cache_key", "function-key"),
    ):
        assert job[field] == backend_cache._backend_artifact_source_key(
            base, artifact_contract=contract, stdlib_object_cache_key=None
        )
    assert job["wasm_link"] is True
    assert job["wasm_data_base"] == 1024
    assert job["wasm_table_base"] == 8
    assert ("probe_cache_only" in job) is probe
    assert ("ir" in job) is not probe


@pytest.mark.parametrize(
    "target,emit",
    [
        ("native", "obj"),
        ("native", "bin"),
        ("wasm", "wasm"),
        ("wasm-freestanding", "wasm"),
    ],
)
def test_daemon_probe_and_full_request_preserve_cache_setup_contract(
    tmp_path, monkeypatch, target, emit
):
    contract = resolve_backend_artifact_contract(target=target, emit_mode=emit)
    output = tmp_path / "output_with_unrelated.suffix"
    output.write_bytes(b"existing output; transport fixture only")
    jobs = []
    leases = []

    def request(socket_path, payload, **kwargs):
        job = json.loads(payload)["jobs"][0]
        jobs.append(job)
        if job.get("probe_cache_only"):
            return {
                "ok": True,
                "jobs": [{"ok": True, "needs_ir": True, "output_written": False}],
            }, None
        lease = Path(job["ir_path"])
        assert json.loads(lease.read_text(encoding="utf-8")) == {"functions": []}
        leases.append(lease)
        return {
            "ok": True,
            "jobs": [{"ok": True, "cached": False, "output_written": False}],
        }, None

    monkeypatch.setattr(backend_execution, "_backend_daemon_request_bytes", request)
    result = backend_execution._compile_with_backend_daemon(
        tmp_path / "daemon.sock",
        project_root=tmp_path,
        ir={"functions": []},
        backend_output=output,
        artifact_contract=contract,
        wasm_link=contract.is_wasm,
        wasm_data_base=None,
        wasm_table_base=None,
        cache_key="same-key",
        function_cache_key="function-key",
        config_digest=None,
        timeout=None,
    )
    assert result.ok, result.error
    assert len(jobs) == 2
    assert len(leases) == 1 and not leases[0].exists()
    for job in jobs:
        assert job["is_wasm"] is contract.is_wasm
        assert job["target_triple"] == contract.target_triple
        assert job.get("native_output_kind") == (
            contract.native_kind.value if contract.native_kind is not None else None
        )
        for field, base in (
            ("cache_key", "same-key"),
            ("function_cache_key", "function-key"),
        ):
            assert job[field] == backend_cache._backend_artifact_source_key(
                base, artifact_contract=contract, stdlib_object_cache_key=None
            )


@pytest.mark.parametrize("target", ["rust", "luau", "mlir"])
def test_daemon_rejects_text_contract_before_ir_lease_or_transport(
    tmp_path, monkeypatch, target
):
    contract = resolve_backend_artifact_contract(target=target, emit_mode="bin")

    def forbidden(*args, **kwargs):
        pytest.fail("text output must not reach daemon lease creation or transport")

    monkeypatch.setattr(backend_execution, "_write_backend_daemon_ir_lease", forbidden)
    monkeypatch.setattr(backend_execution, "_backend_daemon_request_bytes", forbidden)
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir={"functions": []},
        backend_output=tmp_path / "output",
        artifact_contract=contract,
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        cache_key="same-key",
        function_cache_key=None,
        config_digest=None,
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
    )
    assert payload is None
    assert error is not None and "supports only native or WASM" in error
    result = backend_execution._compile_with_backend_daemon(
        tmp_path / "daemon.sock",
        project_root=tmp_path,
        ir={"functions": []},
        backend_output=tmp_path / "output",
        artifact_contract=contract,
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        cache_key="same-key",
        function_cache_key=None,
        config_digest=None,
        timeout=None,
    )
    assert not result.ok
    assert result.error == error


@pytest.mark.parametrize("kind", list(NativeArtifactKind))
def test_native_subprocess_uses_same_explicit_kind(kind):
    command = factgraph.backend_command_prefix(
        backend_bin=Path("molt-backend"),
        is_luau_transpile=False,
        is_rust_transpile=False,
        is_wasm=False,
        native_output_kind=kind,
        target_triple="aarch64-apple-darwin",
    )
    assert command[-2:] == ["--native-output-kind", kind.value]


@pytest.mark.parametrize(
    "target,emit,suffix",
    [
        ("x86_64-pc-windows-msvc", "bin", ".lib"),
        ("x86_64-pc-windows-msvc", "obj", ".obj"),
        ("aarch64-apple-darwin", "bin", ".a"),
        ("aarch64-apple-darwin", "obj", ".o"),
        ("x86_64-unknown-linux-gnu", "bin", ".a"),
        ("rust", "bin", ".rs"),
        ("luau", "bin", ".luau"),
        ("mlir", "bin", ".mlir"),
        ("wasm", "wasm", ".wasm"),
        ("wasm-freestanding", "wasm", ".wasm"),
    ],
)
def test_native_output_filename_matches_requested_format(
    tmp_path, target, emit, suffix
):
    layout = build_output_layout._resolve_build_output_layout(
        target=target,
        trusted=False,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit=emit,
        output=None,
        emit_ir=None,
        artifacts_root=tmp_path / "artifacts",
        bin_root=tmp_path / "bin",
        output_root=tmp_path / "output",
        output_base="program",
        out_dir_path=None,
        project_root=tmp_path,
    )
    assert layout.output_artifact.suffix == suffix
    assert (
        build_output_layout._backend_artifact_suffix(
            target=target,
            emit_mode=layout.emit_mode,
            target_triple=layout.target_triple,
        )
        == suffix
    )


def test_object_request_keeps_complete_stdlib_in_compilation_unit():
    assert not backend_cache._native_stdlib_object_split_enabled(
        target="native", emit_mode="obj"
    )
    assert backend_cache._native_stdlib_object_split_enabled(
        target="native", emit_mode="bin"
    )


def test_archive_internal_references_resolve_against_included_members(monkeypatch):
    definitions = {"molt_init_sys", "molt_sys_helper"}
    monkeypatch.setattr(
        backend_cache,
        "_native_object_global_symbol_sets",
        lambda path, *, target_triple=None: (
            definitions,
            {"molt_sys_helper", "molt_runtime_external"},
        ),
    )
    monkeypatch.setattr(
        backend_cache,
        "_read_shared_stdlib_partition_functions",
        lambda path: definitions,
    )
    assert (
        backend_cache._shared_stdlib_native_symbol_closure_issue(
            Path("stdlib.a"), stdlib_module_symbols={"sys"}
        )
        is None
    )


def test_missing_archive_member_definition_remains_a_closure_error(monkeypatch):
    monkeypatch.setattr(
        backend_cache,
        "_native_object_global_symbol_sets",
        lambda path, *, target_triple=None: ({"molt_init_sys"}, {"molt_sys_helper"}),
    )
    monkeypatch.setattr(
        backend_cache,
        "_read_shared_stdlib_partition_functions",
        lambda path: {"molt_init_sys", "molt_sys_helper"},
    )
    issue = backend_cache._shared_stdlib_native_symbol_closure_issue(
        Path("stdlib.a"), stdlib_module_symbols={"sys"}
    )
    assert issue is not None
    assert "missing partition definitions: molt_sys_helper" in issue
