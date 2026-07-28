from __future__ import annotations

from pathlib import Path

import pytest

from molt.wasm_linking_symbols import (
    parse_wasm_linking_symbols,
    read_wasm_linking_symbols,
    wasm_linking_defined_names,
)


def _u32(value: int) -> bytes:
    encoded = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        encoded.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(encoded)


def _text(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return _u32(len(encoded)) + encoded


def _function(name: str, *, flags: int, index: int) -> bytes:
    return bytes([0]) + _u32(flags) + _u32(index) + _text(name)


def _data(
    name: str,
    *,
    flags: int,
    segment: int = 0,
    offset: int = 0,
    size: int = 8,
) -> bytes:
    entry = bytearray([1])
    entry.extend(_u32(flags))
    entry.extend(_text(name))
    if not flags & 0x10:
        entry.extend(_u32(segment))
        entry.extend(_u32(offset))
        entry.extend(_u32(size))
    return bytes(entry)


def _module(*entries: bytes) -> bytes:
    symbol_table = _u32(len(entries)) + b"".join(entries)
    linking = _u32(2) + bytes([8]) + _u32(len(symbol_table)) + symbol_table
    custom = _text("linking") + linking
    return b"\0asm\x01\0\0\0" + bytes([0]) + _u32(len(custom)) + custom


def _module_from_linking_payload(linking: bytes) -> bytes:
    custom = _text("linking") + linking
    return b"\0asm\x01\0\0\0" + bytes([0]) + _u32(len(custom)) + custom


def test_linking_symbol_table_distinguishes_global_weak_local_and_undefined() -> None:
    table = parse_wasm_linking_symbols(
        _module(
            _function("global_fn", flags=0, index=0),
            _function("weak_fn", flags=1, index=1),
            _function("local_fn", flags=2, index=2),
            _function("undefined_fn", flags=0x50, index=3),
            _data("global_data", flags=0, offset=4),
            _data("weak_data", flags=1, offset=12),
            _data("local_data", flags=2, offset=20),
            _data("undefined_data", flags=0x10),
        )
    )

    assert table.defined_functions == frozenset({"global_fn", "weak_fn"})
    assert table.defined_data == frozenset({"global_data", "weak_data"})
    assert table.undefined_functions == frozenset({"undefined_fn"})
    assert table.undefined_data == frozenset({"undefined_data"})


def test_kind_filtered_definitions_reject_missing_undefined_and_wrong_kind(
    tmp_path: Path,
) -> None:
    member = tmp_path / "molt_runtime_reloc.wasm.deadbeef.runtime-wasm-member"
    member.write_bytes(
        _module(
            _function("defined_fn", flags=0, index=0),
            _function("undefined_fn", flags=0x50, index=1),
            _data("defined_data", flags=0),
            _data("undefined_data", flags=0x10),
        )
    )
    expected = {
        "defined_fn": "function",
        "undefined_fn": "function",
        "defined_data": "data",
        "undefined_data": "data",
    }
    table = read_wasm_linking_symbols(member)

    assert table.defined_names_for_kinds(expected) == frozenset(
        {"defined_fn", "defined_data"}
    )
    assert wasm_linking_defined_names(member, expected) == frozenset(
        {"defined_fn", "defined_data"}
    )
    assert (
        table.defined_names_for_kinds(
            {"defined_fn": "data", "defined_data": "function"}
        )
        == frozenset()
    )


def test_file_apis_use_mmap_and_specialized_result_matches_full_table(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    member = tmp_path / "member.wasm"
    member.write_bytes(
        _module(
            _function("duplicate", flags=0, index=0),
            _function("duplicate", flags=1, index=1),
            _function("local", flags=2, index=2),
            _data("duplicate", flags=0),
            _data("wanted_data", flags=1),
        )
    )
    expected = {
        "duplicate": "function",
        "wanted_data": "data",
        "local": "function",
        "missing": "data",
    }
    monkeypatch.setattr(
        Path,
        "read_bytes",
        lambda self: pytest.fail(f"copied whole file through {self}.read_bytes()"),
    )

    table = read_wasm_linking_symbols(member)

    assert (
        wasm_linking_defined_names(member, expected)
        == (table.defined_names_for_kinds(expected))
        == frozenset({"duplicate", "wanted_data"})
    )


def test_empty_expected_symbols_bypass_file_io(tmp_path: Path) -> None:
    assert (
        wasm_linking_defined_names(tmp_path / "does-not-exist.wasm", {}) == frozenset()
    )


def test_full_and_specialized_filters_share_kind_validation(tmp_path: Path) -> None:
    member = tmp_path / "member.wasm"
    member.write_bytes(_module(_function("defined_fn", flags=0, index=0)))
    table = read_wasm_linking_symbols(member)

    with pytest.raises(
        ValueError, match="unsupported WebAssembly linking symbol kind 'global'"
    ):
        table.defined_names_for_kinds({"defined_fn": "global"})
    with pytest.raises(
        ValueError, match="unsupported WebAssembly linking symbol kind 'global'"
    ):
        wasm_linking_defined_names(member, {"defined_fn": "global"})


@pytest.mark.parametrize("trim", [1, 2, 4])
def test_full_and_specialized_parsers_reject_truncated_symbol_table(
    tmp_path: Path, trim: int
) -> None:
    complete = _module(_function("defined_fn", flags=0, index=0))
    member = tmp_path / f"truncated-{trim}.wasm"
    member.write_bytes(complete[:-trim])

    with pytest.raises(ValueError):
        read_wasm_linking_symbols(member)
    with pytest.raises(ValueError):
        wasm_linking_defined_names(member, {"defined_fn": "function"})


def test_full_and_specialized_parsers_bound_symbol_strings_to_subsection(
    tmp_path: Path,
) -> None:
    truncated_entry = _function("defined_fn", flags=0, index=0)[:-1]
    symbol_table = _u32(1) + truncated_entry
    subsection = bytes([8]) + _u32(len(symbol_table)) + symbol_table
    member = tmp_path / "truncated-symbol-string.wasm"
    member.write_bytes(_module_from_linking_payload(_u32(2) + subsection))

    with pytest.raises(ValueError, match="Unexpected EOF while reading wasm string"):
        read_wasm_linking_symbols(member)
    with pytest.raises(ValueError, match="Unexpected EOF while reading wasm string"):
        wasm_linking_defined_names(member, {"defined_fn": "function"})


def test_full_and_specialized_parsers_reject_duplicate_symbol_tables(
    tmp_path: Path,
) -> None:
    symbol_table = _u32(1) + _function("defined_fn", flags=0, index=0)
    subsection = bytes([8]) + _u32(len(symbol_table)) + symbol_table
    member = tmp_path / "duplicate-table.wasm"
    member.write_bytes(_module_from_linking_payload(_u32(2) + subsection + subsection))

    with pytest.raises(ValueError, match="duplicate WebAssembly linking symbol table"):
        read_wasm_linking_symbols(member)
    with pytest.raises(ValueError, match="duplicate WebAssembly linking symbol table"):
        wasm_linking_defined_names(member, {"defined_fn": "function"})
