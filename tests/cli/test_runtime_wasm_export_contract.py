from __future__ import annotations

from pathlib import Path

import pytest

from molt._wasm_abi_generated import runtime_execution_abi
from molt.cli.runtime_wasm_validation import _runtime_wasm_typed_export_names
from molt.wasm_artifact import (
    _build_wasm_sections,
    _write_wasm_string,
    read_wasm_exports,
    transform_wasm_publication_file,
    wasm_custom_section_names,
)


def _typed_exports(path: Path, expected: dict[str, str]) -> set[str]:
    from molt.toolchain_identity import stable_regular_file_identity
    from molt.wasm_artifact import read_wasm_runtime_facts

    direct = _runtime_wasm_typed_export_names(path, expected)
    facts = read_wasm_runtime_facts(
        stable_regular_file_identity(path, label="export fixture"),
        relocatable=False,
    )
    assert _runtime_wasm_typed_export_names(path, expected, facts=facts) == direct
    return direct


def _u32(value: int) -> bytes:
    encoded = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        encoded.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(encoded)


def _data_export_wasm(*, immediate: bytes, names: tuple[str, ...]) -> bytes:
    memory = b"\x01\x00\x01"  # one defined memory, minimum one 64KiB page
    global_section = b"\x01\x7f\x00\x41" + immediate + b"\x0b"
    exports = bytearray(_u32(len(names)))
    for name in names:
        exports += _write_wasm_string(name) + b"\x03\x00"
    return _build_wasm_sections([(5, memory), (6, global_section), (7, bytes(exports))])


def _function_export_wasm(name: str, *, params: bytes, results: bytes) -> bytes:
    type_section = b"\x01\x60" + params + results
    function_section = b"\x01\x00"
    export_section = b"\x01" + _write_wasm_string(name) + b"\x00\x00"
    return _build_wasm_sections(
        [(1, type_section), (3, function_section), (7, export_section)]
    )


def test_data_export_requires_nonzero_canonical_u32_address_in_memory(
    tmp_path: Path,
) -> None:
    path = tmp_path / "runtime.wasm"
    expected = {"PyLong_Type": "data"}

    path.write_bytes(_data_export_wasm(immediate=b"\x10", names=("PyLong_Type",)))
    assert _typed_exports(path, expected) == {"PyLong_Type"}

    for invalid in (
        b"\x00",  # NULL
        b"\x90\x00",  # overlong/non-canonical encoding of 16
        b"\x80\x80\x04",  # 65536, exactly outside the initial memory
        b"\x7f",  # -1 interpreted as u32 and therefore outside memory
    ):
        path.write_bytes(_data_export_wasm(immediate=invalid, names=("PyLong_Type",)))
        assert _typed_exports(path, expected) == set()


def test_data_export_alias_collision_fails_closed(tmp_path: Path) -> None:
    path = tmp_path / "runtime.wasm"
    path.write_bytes(
        _data_export_wasm(immediate=b"\x10", names=("PyLong_Type", "PyFloat_Type"))
    )

    assert (
        _typed_exports(path, {"PyLong_Type": "data", "PyFloat_Type": "data"}) == set()
    )


def test_cpython_function_export_must_match_generated_c_abi_signature(
    tmp_path: Path,
) -> None:
    path = tmp_path / "runtime.wasm"
    expected = {"PyLong_FromLong": "function"}
    path.write_bytes(
        _function_export_wasm(
            "PyLong_FromLong", params=b"\x01\x7f", results=b"\x01\x7f"
        )
    )
    assert _typed_exports(path, expected) == {"PyLong_FromLong"}

    path.write_bytes(
        _function_export_wasm(
            "PyLong_FromLong", params=b"\x01\x7e", results=b"\x01\x7f"
        )
    )
    assert _typed_exports(path, expected) == set()


def test_public_c_runtime_export_preserves_its_declared_signature(
    tmp_path: Path,
) -> None:
    path = tmp_path / "runtime.wasm"
    expected = {"molt_buffer_acquire": "function"}
    path.write_bytes(
        _function_export_wasm(
            "molt_buffer_acquire",
            params=b"\x02\x7e\x7f",
            results=b"\x01\x7f",
        )
    )

    assert _typed_exports(path, expected) == {"molt_buffer_acquire"}


def test_split_runtime_export_uses_generated_canonical_import_signature(
    tmp_path: Path,
) -> None:
    path = tmp_path / "runtime.wasm"
    expected = {"molt_PyLong_FromLong": "function"}
    path.write_bytes(
        _function_export_wasm(
            "molt_PyLong_FromLong",
            params=b"\x01\x7f",
            results=b"\x01\x7f",
        )
    )

    assert _typed_exports(path, expected) == {"molt_PyLong_FromLong"}


def test_unknown_runtime_export_signature_fails_closed(tmp_path: Path) -> None:
    path = tmp_path / "runtime.wasm"
    path.write_bytes(
        _function_export_wasm(
            "molt_unknown_runtime_export",
            params=b"\x01\x7e",
            results=b"\x01\x7e",
        )
    )

    assert _typed_exports(path, {"molt_unknown_runtime_export": "function"}) == set()


def test_direct_runtime_export_signature_mismatch_fails_closed(tmp_path: Path) -> None:
    path = tmp_path / "runtime.wasm"
    path.write_bytes(
        _function_export_wasm(
            "molt_fast_list_append",
            params=b"\x01\x7e",
            results=b"\x01\x7e",
        )
    )

    assert _typed_exports(path, {"molt_fast_list_append": "function"}) == set()


def test_publication_transform_has_one_bounded_buffered_pass(tmp_path: Path) -> None:
    path = tmp_path / "runtime.wasm"
    export_section = b"\x01" + _write_wasm_string("PyLong_FromLong") + b"\x00\x00"
    debug_payload = _write_wasm_string(".debug_info") + b"x" * (9 * 1024 * 1024)
    path.write_bytes(
        _build_wasm_sections(
            [
                (1, b"\x01\x60\x01\x7f\x01\x7f"),
                (3, b"\x01\x00"),
                (7, export_section),
                (0, debug_payload),
            ]
        )
    )

    metrics = transform_wasm_publication_file(
        path,
        rename_map={"PyLong_FromLong": "molt_PyLong_FromLong"},
        final_artifact=True,
        preserve_debug=False,
    )

    assert metrics.changed
    assert metrics.input_bytes > 9 * 1024 * 1024
    assert metrics.max_buffer_bytes <= 8 * 1024 * 1024
    assert [export.name for export in read_wasm_exports(path)] == [
        "molt_PyLong_FromLong"
    ]
    assert ".debug_info" not in wasm_custom_section_names(path.read_bytes())
    warm = transform_wasm_publication_file(
        path,
        rename_map={"PyLong_FromLong": "molt_PyLong_FromLong"},
        final_artifact=True,
        preserve_debug=False,
    )
    assert not warm.changed
    assert warm.written_bytes == 0
    assert not tuple(tmp_path.glob(".molt-wasm-transform-*.tmp"))


@pytest.mark.parametrize(
    ("params", "results", "accepted"),
    [
        (b"\x03\x7e\x7e\x7e", b"\x01\x7e", True),
        (b"\x02\x7e\x7e", b"\x01\x7e", False),  # missing context
        (b"\x04\x7e\x7e\x7e\x7e", b"\x01\x7e", False),
        (b"\x03\x7f\x7e\x7e", b"\x01\x7e", False),  # i32 context
        (b"\x03\x7e\x7e\x7e", b"\x01\x7f", False),  # wrong result
    ],
)
def test_attached_export_requires_exact_physical_signature(
    tmp_path: Path, params: bytes, results: bytes, accepted: bool
) -> None:
    # Encode the physical contract independently of the signature resolver:
    # execution context, list and item are i64; the return value is i64.
    abi = runtime_execution_abi("molt_fast_list_append")
    assert abi is not None and abi.context == "Attached"
    symbol = abi.physical_symbol
    path = tmp_path / "runtime.wasm"
    path.write_bytes(_function_export_wasm(symbol, params=params, results=results))
    assert _typed_exports(path, {symbol: "function"}) == (
        {symbol} if accepted else set()
    )


@pytest.mark.parametrize(
    ("symbol", "params", "results", "extra_context"),
    [
        ("molt_runtime_execution_leave", b"\x01\x7e", b"\x00", b"\x02\x7e\x7e"),
        ("molt_none", b"\x00", b"\x01\x7e", b"\x01\x7e"),
    ],
)
def test_boundary_export_does_not_acquire_an_extra_context_parameter(
    tmp_path: Path, symbol: str, params: bytes, results: bytes, extra_context: bytes
) -> None:
    abi = runtime_execution_abi(symbol)
    assert abi is not None and abi.context == "Boundary"
    assert abi.physical_symbol == symbol
    path = tmp_path / "runtime.wasm"
    path.write_bytes(_function_export_wasm(symbol, params=params, results=results))
    assert _typed_exports(path, {symbol: "function"}) == {symbol}
    path.write_bytes(
        _function_export_wasm(symbol, params=extra_context, results=results)
    )
    assert _typed_exports(path, {symbol: "function"}) == set()
