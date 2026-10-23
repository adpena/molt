from __future__ import annotations

from molt.compilation_target import admit_target

from tests.cli.native_link_test_support import transport_codegen_binding

import copy
import hashlib
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
from molt.cli.backend_ir_transport import (
    BackendIrProgram,
    _bin_header,
    _container_header,
)
from molt.cli.cache_keys import FunctionDigests
from molt.cli.function_records import FunctionDigestSeed, function_body_digest

from molt.cli import backend_cache, backend_execution, build_output_layout, factgraph
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.native_target import NativeArtifactKind


def _program(functions, **header):
    """An addressed program whose digests the transport computes itself."""
    return BackendIrProgram({"functions": functions, **header}, FunctionDigests())


_DOMAIN = b"molt.function-body.msgpack.v1\0"


def _lease(program, shipped=None):
    output = io.BytesIO()
    program.write(output, shipped)
    return output.getvalue()


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
        probe_function_digests=("ab" * 32) if probe else None,
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
    assert job.get("function_digests") == (("ab" * 32) if probe else None)
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
        probe_function_digests="" if probe else None,
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
    assert job.get("function_digests") == ("" if probe else None)
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
            assert job["function_digests"] == ""
            return {
                "ok": True,
                "jobs": [
                    {
                        "ok": True,
                        "needs_ir": True,
                        "missing_functions": [],
                        "output_written": False,
                    }
                ],
            }, None
        lease = Path(job["ir_path"])
        assert msgpack.unpackb(lease.read_bytes(), raw=False) == {
            "function_bodies": [],
            "function_digests": b"",
        }
        leases.append(lease)
        return {
            "ok": True,
            "jobs": [{"ok": True, "cached": False, "output_written": False}],
        }, None

    monkeypatch.setattr(backend_execution, "_backend_daemon_request_bytes", request)
    result = backend_execution._compile_with_backend_daemon(
        tmp_path / "daemon.sock",
        project_root=tmp_path,
        program=_program([]),
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


def _needs(missing):
    return {
        "ok": True,
        "jobs": [
            {
                "ok": True,
                "needs_ir": True,
                "missing_functions": missing,
                "output_written": False,
            }
        ],
    }


_COMPILED = {
    "ok": True,
    "jobs": [{"ok": True, "cached": False, "output_written": False}],
}
_CACHED = {
    "ok": True,
    "jobs": [
        {"ok": True, "cached": True, "cache_tier": "module", "output_written": False}
    ],
}


@pytest.mark.parametrize(
    "replies,expected_shipped,expected_error",
    [
        # A warm artifact cache: one probe, no lease.
        ([_CACHED], [], None),
        # The store lacks one body; the lease ships exactly that body.
        ([_needs([1]), _COMPILED], [[1]], None),
        # Every body is resident; the lease ships none.
        ([_needs([]), _COMPILED], [[]], None),
        # A body was evicted after the probe; the second lease ships every body.
        ([_needs([1]), _needs([0]), _COMPILED], [[1], [0, 1, 2]], None),
        # A complete lease cannot miss: a daemon that still asks is an error.
        (
            [_needs([1]), _needs([0]), _needs([0])],
            [[1], [0, 1, 2]],
            "lacks function bodies after a complete IR lease",
        ),
        # Positions must name the program's functions.
        ([_needs([3])], [], "lacks valid missing_functions"),
        ([_needs(None)], [], "lacks valid missing_functions"),
    ],
)
def test_daemon_client_ships_only_the_bodies_the_daemon_lacks(
    tmp_path, monkeypatch, replies, expected_shipped, expected_error
):
    contract = resolve_backend_artifact_contract(
        target=admit_target("wasm"), emit_mode="wasm"
    )
    functions = [
        {"name": f"f{index}", "ops": [{"kind": "ret_void"}]} for index in range(3)
    ]
    program = _program(functions)
    bodies = [msgpack.packb(function, use_bin_type=True) for function in functions]
    pending = list(replies)
    shipped = []

    def request(socket_path, payload, **kwargs):
        job = json.loads(payload)["jobs"][0]
        if job.get("probe_cache_only"):
            assert job["function_digests"] == program.digest_hex()
        else:
            lease = msgpack.unpackb(Path(job["ir_path"]).read_bytes(), raw=False)
            assert lease["function_digests"] == bytes.fromhex(program.digest_hex())
            shipped.append([bodies.index(body) for body in lease["function_bodies"]])
        return pending.pop(0), None

    monkeypatch.setattr(backend_execution, "_backend_daemon_request_bytes", request)
    result = backend_execution._compile_with_backend_daemon(
        tmp_path / "daemon.sock",
        project_root=tmp_path,
        program=program,
        backend_output=tmp_path / "output.wasm",
        artifact_contract=contract,
        wasm_link=True,
        wasm_data_base=None,
        wasm_table_base=None,
        cache_key="module-key",
        function_cache_key=None,
        config_digest=None,
        timeout=None,
        request_environment=os.environ,
    )
    assert not pending, "every scripted reply is consumed"
    assert shipped == expected_shipped
    if expected_error is None:
        assert result.ok, result.error
        assert result.full_request_sent is bool(expected_shipped)
    else:
        assert not result.ok
        assert expected_error in (result.error or "")
    assert not list((tmp_path).rglob("*.msgpack")), "every lease is released"


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
        program=_program([]),
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
    good = backend_execution._write_backend_ir_lease(tmp_path / "project", _program([]))
    lease_dir = good.parent
    assert msgpack.unpackb(good.read_bytes(), raw=False) == {
        "function_bodies": [],
        "function_digests": b"",
    }

    class Unencodable:
        pass

    # A seeded digest admits the program without encoding it, so the first
    # body streams out before the encoder meets the bad value.
    bad = {"name": Unencodable()}
    program = BackendIrProgram(
        {"functions": [{"name": "main"}, bad]},
        FunctionDigests([FunctionDigestSeed(bad, b"\0" * 32)]),
    )
    with pytest.raises(TypeError):
        backend_execution._write_backend_ir_lease(tmp_path / "project", program)
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
    assert _container_header(count, array=False) == map_header
    assert _container_header(count, array=True) == array_header
    # The reference encoder frames containers of the same size identically.
    assert msgpack.packb({str(i): 0 for i in range(count)}).startswith(map_header)
    assert msgpack.packb([0] * count).startswith(array_header)


@pytest.mark.parametrize("length", [0, 255, 256, 65535, 65536])
def test_binary_ir_bin_headers_follow_messagepack_boundaries(length):
    payload = b"\x07" * length
    assert _bin_header(length) + payload == msgpack.packb(payload, use_bin_type=True)


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
    function = {
        "name": "main",
        "ops": [{"kind": "const_float", "f_value": value} for value in values],
    }
    decoded = msgpack.unpackb(_lease(_program([function])), raw=False)
    (body,) = decoded["function_bodies"]
    decoded_values = [op["f_value"] for op in msgpack.unpackb(body)["ops"]]
    assert [struct.pack(">d", value).hex() for value in decoded_values] == bit_patterns
    # Independent encoder agreement also rejects f32 or JSON float projection.
    canonical = {
        "name": "main",
        "ops": [{"f_value": value, "kind": "const_float"} for value in values],
    }
    assert body == msgpack.packb(canonical, use_bin_type=True, use_single_float=False)
    assert decoded["function_digests"] == hashlib.sha256(_DOMAIN + body).digest()


def test_binary_ir_lease_ships_selected_bodies_without_mutating_source():
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
        "code_slots": {"function_0": 0},
        "module_registry": None,
        "profile": {"enabled": True, "label": "é"},
        "runtime_feedback": {"not": "a backend input"},
    }
    before = copy.deepcopy(ir)
    program = BackendIrProgram(ir, FunctionDigests())
    writes = []

    class RecordingHandle:
        def write(self, data):
            assert isinstance(data, bytes)
            writes.append(data)
            return len(data)

    program.write(RecordingHandle(), [3, 1, 1])
    encoded = b"".join(writes)
    decoded = msgpack.unpackb(encoded, raw=False)
    assert ir == before
    # Sorted keys; only the header fields the backend admits travel.
    assert list(decoded) == [
        "code_slots",
        "function_bodies",
        "function_digests",
        "module_registry",
        "profile",
    ]
    assert decoded["code_slots"] == before["code_slots"]
    assert decoded["profile"] == before["profile"]
    bodies = [msgpack.packb(f, use_bin_type=True) for f in functions]
    expected_digests = [hashlib.sha256(_DOMAIN + body).digest() for body in bodies]
    assert decoded["function_digests"] == b"".join(expected_digests)
    assert program.digest_hex() == b"".join(expected_digests).hex()
    # Program order ships each selected body once, as its exact bytes.
    assert decoded["function_bodies"] == [bodies[1], bodies[3]]
    assert function_body_digest(decoded["function_bodies"][0]) == expected_digests[1]
    # One body (or the 32-byte-per-function digest list) is resident at a
    # time, never the whole document.
    largest = max(32 * len(functions), *(len(body) for body in bodies))
    assert max(map(len, writes)) == largest < len(encoded)
    with pytest.raises(ValueError, match="outside the program"):
        program.write(RecordingHandle(), [4])


def test_ir_leases_are_exclusive_and_preserve_an_existing_owner(tmp_path, monkeypatch):
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    project = tmp_path / "project"
    empty = {"function_bodies": [], "function_digests": b""}
    first = backend_execution._write_backend_ir_lease(project, _program([]))
    second = backend_execution._write_backend_ir_lease(project, _program([]))
    assert first != second
    assert first.suffix == second.suffix == ".msgpack"
    assert msgpack.unpackb(first.read_bytes(), raw=False) == empty
    assert msgpack.unpackb(second.read_bytes(), raw=False) == empty
    original = first.read_bytes()
    first_uuid = first.stem.rsplit("-", 1)[1]
    monkeypatch.setattr(
        backend_execution.uuid, "uuid4", lambda: SimpleNamespace(hex=first_uuid)
    )
    with pytest.raises(FileExistsError):
        backend_execution._write_backend_ir_lease(
            project, _program([{"name": "other"}])
        )
    assert first.read_bytes() == original
    assert set(first.parent.iterdir()) == {first, second}


def test_partial_ir_write_failure_releases_only_its_lease(tmp_path, monkeypatch):
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    project = tmp_path / "project"
    good = backend_execution._write_backend_ir_lease(project, _program([]))
    original = good.read_bytes()

    def fail_after_write(self, handle, shipped=None):
        handle.write(b"\x82\xaffunction_bodies\x91")
        handle.flush()
        raise OSError("injected write failure")

    monkeypatch.setattr(BackendIrProgram, "write", fail_after_write)
    with pytest.raises(OSError, match="injected write failure"):
        backend_execution._write_backend_ir_lease(project, _program([{"name": "main"}]))
    assert good.read_bytes() == original
    assert list(good.parent.iterdir()) == [good]
