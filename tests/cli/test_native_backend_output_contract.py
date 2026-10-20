from __future__ import annotations

from molt.compilation_target import admit_target

from tests.cli.native_link_test_support import transport_codegen_binding

import copy
import io
import json
import os
import struct
from pathlib import Path
from types import SimpleNamespace

import msgpack
import pytest

from molt.backend_protocol import BACKEND_DAEMON_PROTOCOL_VERSION
from molt.cli import native_symbol_inspection
from molt.cli.backend_ir_transport import write_backend_ir

from molt.cli import backend_cache, backend_execution, build_output_layout, factgraph
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.native_target import NativeArtifactKind


@pytest.mark.parametrize("kind", list(NativeArtifactKind))
@pytest.mark.parametrize("probe", [False, True])
def test_native_output_kind_is_explicit_in_full_and_probe_requests(
    tmp_path, kind, probe
):
    contract = resolve_backend_artifact_contract(
        target=admit_target("x86_64-pc-windows-msvc" or "native"),
        emit_mode="obj" if kind is NativeArtifactKind.OBJECT else "bin",
    )
    stdlib_key = "stdlib-key" if kind is NativeArtifactKind.ARCHIVE else None
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir_path=None if probe else Path("backend-ir.msgpack"),
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
        native_runtime_codegen_binding=transport_codegen_binding(
            tmp_path / "runtime-transport"
        )
        if contract.is_native
        else None,
        request_environment=os.environ,
    )
    assert error is None
    assert payload is not None
    request = json.loads(payload)
    assert request["version"] == BACKEND_DAEMON_PROTOCOL_VERSION
    job = request["jobs"][0]
    assert ("probe_cache_only" in job) is probe
    assert ("ir_path" in job) is not probe
    assert "ir" not in job
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
        target=admit_target(target), emit_mode="wasm"
    )
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir_path=None if probe else Path("backend-ir.msgpack"),
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
        request_environment=os.environ,
    )
    assert error is None
    assert payload is not None
    request = json.loads(payload)
    assert request["version"] == BACKEND_DAEMON_PROTOCOL_VERSION
    job = request["jobs"][0]
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
    assert ("ir_path" in job) is not probe
    assert "ir" not in job


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
    contract = resolve_backend_artifact_contract(
        target=admit_target(target), emit_mode=emit
    )
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
        assert msgpack.unpackb(lease.read_bytes(), raw=False) == {"functions": []}
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
        native_runtime_codegen_binding=transport_codegen_binding(
            tmp_path / "runtime-transport"
        )
        if contract.is_native
        else None,
        request_environment=os.environ,
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
    contract = resolve_backend_artifact_contract(
        target=admit_target(target), emit_mode="bin"
    )

    def forbidden(*args, **kwargs):
        pytest.fail("text output must not reach daemon lease creation or transport")

    monkeypatch.setattr(backend_execution, "_write_backend_ir_lease", forbidden)
    monkeypatch.setattr(backend_execution, "_backend_daemon_request_bytes", forbidden)
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir_path=Path("backend-ir.msgpack"),
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
        request_environment=os.environ,
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
        request_environment=os.environ,
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
        target=admit_target(target),
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
            target=admit_target(target),
            emit_mode=layout.emit_mode,
        )
        == suffix
    )


def test_object_request_keeps_complete_stdlib_in_compilation_unit():
    assert not backend_cache._native_stdlib_object_split_enabled(
        target=admit_target("native"), emit_mode="obj"
    )
    assert backend_cache._native_stdlib_object_split_enabled(
        target=admit_target("native"), emit_mode="bin"
    )


def test_archive_internal_references_resolve_against_included_members(monkeypatch):
    definitions = {"molt_init_sys", "molt_sys_helper"}
    monkeypatch.setattr(
        native_symbol_inspection,
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
        native_symbol_inspection,
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


def test_failed_ir_lease_write_leaves_no_partial_lease(tmp_path, monkeypatch):
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    good = backend_execution._write_backend_ir_lease(
        tmp_path / "project", {"functions": []}
    )
    lease_dir = good.parent
    assert msgpack.unpackb(good.read_bytes(), raw=False) == {"functions": []}

    class Unencodable:
        pass

    # The first function streams out before the encoder meets the bad value.
    ir = {"functions": [{"name": "main"}, {"name": Unencodable()}]}
    with pytest.raises(TypeError):
        backend_execution._write_backend_ir_lease(tmp_path / "project", ir)
    assert sorted(lease_dir.iterdir()) == [good]


@pytest.mark.parametrize(
    "count,map_header,array_header",
    [
        (15, b"\x8f", b"\x9f"),
        (16, b"\xde\x00\x10", b"\xdc\x00\x10"),
        (65535, b"\xde\xff\xff", b"\xdc\xff\xff"),
        (65536, b"\xdf\x00\x01\x00\x00", b"\xdd\x00\x01\x00\x00"),
    ],
)
def test_binary_ir_container_headers_follow_messagepack_boundaries(
    count, map_header, array_header
):
    mapping = {str(index): index for index in range(count)}
    output = io.BytesIO()
    write_backend_ir(output, mapping)
    encoded = output.getvalue()
    assert encoded.startswith(map_header)
    assert msgpack.unpackb(encoded, raw=False) == mapping

    values = list(range(count))
    output = io.BytesIO()
    write_backend_ir(output, {"values": values})
    encoded = output.getvalue()
    # One-entry map, six-byte string key, then the actual array header.
    assert encoded.startswith(b"\x81\xa6values" + array_header)
    assert msgpack.unpackb(encoded, raw=False) == {"values": values}


def test_binary_ir_function_values_preserve_exact_f64_bits():
    bit_patterns = [
        "0000000000000000",  # positive zero
        "8000000000000000",  # negative zero
        "3ff0000000000001",  # a finite value requiring all 64 bits
        "7ff0000000000000",  # positive infinity
        "fff0000000000000",  # negative infinity
        "7ff8000000000001",  # distinct quiet NaN payloads
        "fff8000000000042",
        "7ff0000000000001",  # signaling NaN and subnormal payloads
        "0000000000000001",
    ]
    values = [struct.unpack(">d", bytes.fromhex(bits))[0] for bits in bit_patterns]
    ir = {
        "functions": [
            {
                "name": "main",
                "ops": [{"kind": "const_float", "f_value": value} for value in values],
            }
        ]
    }
    output = io.BytesIO()
    write_backend_ir(output, ir)
    encoded = output.getvalue()
    decoded = msgpack.unpackb(encoded, raw=False)
    decoded_values = [op["f_value"] for op in decoded["functions"][0]["ops"]]
    assert [struct.pack(">d", value).hex() for value in decoded_values] == bit_patterns
    assert [struct.pack(">d", value).hex() for value in values] == bit_patterns
    # Independent encoder agreement also rejects f32 or JSON float projection.
    canonical = {
        "functions": [
            {
                "name": "main",
                "ops": [{"f_value": value, "kind": "const_float"} for value in values],
            }
        ]
    }
    assert encoded == msgpack.packb(
        canonical, use_bin_type=True, use_single_float=False
    )


def test_binary_ir_writes_bounded_elements_without_mutating_source():
    functions = [
        {
            "name": f"function_{index}",
            "ops": [{"args": [index], "kind": "ret"}],
            "params": ["value"],
        }
        for index in range(4)
    ]
    ir = {
        "functions": functions,
        "modules": [{"name": "module", "payload": b"\x00\xff"}],
        "profile": {"enabled": True, "label": "é"},
        "empty": [],
    }
    before = copy.deepcopy(ir)
    writes = []

    class RecordingHandle:
        def write(self, data):
            assert isinstance(data, bytes)
            writes.append(data)
            return len(data)

    write_backend_ir(RecordingHandle(), ir)
    encoded = b"".join(writes)
    assert msgpack.unpackb(encoded, raw=False) == before
    assert ir == before
    elements = [
        element
        for value in before.values()
        for element in (value if isinstance(value, list) else [value])
    ]
    largest_element = max(
        len(msgpack.packb(element, use_bin_type=True)) for element in elements
    )
    assert max(map(len, writes)) == largest_element < len(encoded)
    assert len(writes) <= 1 + 2 * len(ir) + len(elements)
    # A complete function is emitted once, rather than token by token.
    assert all(
        msgpack.packb(function, use_bin_type=True) in writes for function in functions
    )


def test_ir_leases_are_exclusive_and_preserve_an_existing_owner(tmp_path, monkeypatch):
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    project = tmp_path / "project"
    ir = {"functions": []}
    first = backend_execution._write_backend_ir_lease(project, ir)
    second = backend_execution._write_backend_ir_lease(project, ir)
    assert first != second
    assert first.suffix == second.suffix == ".msgpack"
    assert msgpack.unpackb(first.read_bytes(), raw=False) == ir
    assert msgpack.unpackb(second.read_bytes(), raw=False) == ir
    original = first.read_bytes()
    first_uuid = first.stem.rsplit("-", 1)[1]
    monkeypatch.setattr(
        backend_execution.uuid, "uuid4", lambda: SimpleNamespace(hex=first_uuid)
    )
    with pytest.raises(FileExistsError):
        backend_execution._write_backend_ir_lease(
            project, {"functions": [{"name": "other"}]}
        )
    assert first.read_bytes() == original
    assert set(first.parent.iterdir()) == {first, second}


def test_partial_ir_write_failure_releases_only_its_lease(tmp_path, monkeypatch):
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    project = tmp_path / "project"
    good = backend_execution._write_backend_ir_lease(project, {"functions": []})
    original = good.read_bytes()

    def fail_after_write(handle, ir):
        handle.write(b"\x81\xa9functions\x91")
        handle.flush()
        raise OSError("injected write failure")

    monkeypatch.setattr(backend_execution, "write_backend_ir", fail_after_write)
    with pytest.raises(OSError, match="injected write failure"):
        backend_execution._write_backend_ir_lease(
            project, {"functions": [{"name": "main"}]}
        )
    assert good.read_bytes() == original
    assert list(good.parent.iterdir()) == [good]
