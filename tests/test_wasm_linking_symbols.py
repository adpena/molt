from __future__ import annotations

from pathlib import Path

import pytest

from molt.wasm_artifact import (
    WASM_EXTERN_KIND_FUNCTION,
    WASM_EXTERN_KIND_GLOBAL,
    WASM_EXTERN_KIND_MEMORY,
    WASM_EXTERN_KIND_TABLE,
    WASM_EXTERN_KIND_TAG,
    WasmImport,
    flatten_wasm_plain_function_rec_groups,
    parse_wasm_exports,
    parse_wasm_imports,
    parse_wasm_section_spans,
    parse_wasm_sections,
    parse_wasm_relocatable_object_interface,
    parse_wasm_type_section_groups,
)
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


def _unnamed_indexed(kind: int, *, index: int) -> bytes:
    return bytes([kind]) + _u32(0x10) + _u32(index)


def _defined_indexed(kind: int, name: str, *, index: int) -> bytes:
    return bytes([kind]) + _u32(0) + _u32(index) + _text(name)


def _module(*entries: bytes) -> bytes:
    symbol_table = _u32(len(entries)) + b"".join(entries)
    linking = _u32(2) + bytes([8]) + _u32(len(symbol_table)) + symbol_table
    custom = _text("linking") + linking
    return b"\0asm\x01\0\0\0" + bytes([0]) + _u32(len(custom)) + custom


def _module_from_linking_payload(linking: bytes) -> bytes:
    custom = _text("linking") + linking
    return b"\0asm\x01\0\0\0" + bytes([0]) + _u32(len(custom)) + custom


def _section(section_id: int, payload: bytes) -> bytes:
    return bytes([section_id]) + _u32(len(payload)) + payload


def _import(module: str, name: str, kind: int, description: bytes) -> bytes:
    return _text(module) + _text(name) + bytes([kind]) + description


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


def test_import_parser_retains_complete_non_function_descriptors() -> None:
    import_payload = _u32(4) + b"".join(
        (
            _import(
                "env",
                "table",
                WASM_EXTERN_KIND_TABLE,
                b"\x63\x70" + _u32(1) + _u32(3) + _u32(7),
            ),
            _import(
                "env",
                "memory",
                WASM_EXTERN_KIND_MEMORY,
                _u32(3) + _u32(2) + _u32(11),
            ),
            _import(
                "env",
                "global",
                WASM_EXTERN_KIND_GLOBAL,
                b"\x64\x70\x01",
            ),
            _import(
                "env",
                "tag",
                WASM_EXTERN_KIND_TAG,
                b"\x00" + _u32(5),
            ),
        )
    )
    artifact = b"\0asm\x01\0\0\0" + _section(2, import_payload)

    assert parse_wasm_imports(artifact) == [
        WasmImport(
            "env",
            "table",
            WASM_EXTERN_KIND_TABLE,
            limits_flags=1,
            minimum=3,
            maximum=7,
            reference_type=b"\x63\x70",
        ),
        WasmImport(
            "env",
            "memory",
            WASM_EXTERN_KIND_MEMORY,
            limits_flags=3,
            minimum=2,
            maximum=11,
        ),
        WasmImport(
            "env",
            "global",
            WASM_EXTERN_KIND_GLOBAL,
            value_type=b"\x64\x70",
            mutable=True,
        ),
        WasmImport(
            "env",
            "tag",
            WASM_EXTERN_KIND_TAG,
            type_index=5,
            tag_attribute=0,
        ),
    ]


def test_import_parser_rejects_invalid_global_mutability() -> None:
    payload = _u32(1) + _import(
        "env",
        "global",
        WASM_EXTERN_KIND_GLOBAL,
        b"\x7f\x02",
    )

    with pytest.raises(ValueError, match="global mutability"):
        parse_wasm_imports(b"\0asm\x01\0\0\0" + _section(2, payload))


def test_canonical_section_and_vector_parsers_reject_duplicate_or_trailing_data() -> (
    None
):
    header = b"\0asm\x01\0\0\0"
    empty_imports = _section(2, _u32(0))
    with pytest.raises(ValueError, match="Duplicate wasm section id 2"):
        parse_wasm_sections(header + empty_imports + empty_imports)

    with pytest.raises(ValueError, match="Trailing bytes in wasm import section"):
        parse_wasm_imports(header + _section(2, _u32(0) + b"\0"))

    with pytest.raises(ValueError, match="Trailing bytes in wasm export section"):
        parse_wasm_exports(header + _section(7, _u32(0) + b"\0"))


def test_linking_parser_rejects_duplicate_metadata_sections() -> None:
    linking_section = _module()[8:]

    with pytest.raises(ValueError, match="duplicate WebAssembly linking metadata"):
        parse_wasm_linking_symbols(b"\0asm\x01\0\0\0" + linking_section * 2)


def test_unnamed_indexed_undefined_symbols_resolve_in_kind_specific_import_spaces(
    tmp_path: Path,
) -> None:
    member = tmp_path / "indexed-imports.wasm"
    member.write_bytes(
        _module(
            _unnamed_indexed(0, index=0),
            _unnamed_indexed(2, index=0),
            _unnamed_indexed(5, index=0),
            _unnamed_indexed(4, index=0),
        )
    )
    imports = (
        WasmImport("env", "function_import", WASM_EXTERN_KIND_FUNCTION),
        WasmImport("env", "global_import", WASM_EXTERN_KIND_GLOBAL),
        WasmImport("env", "table_import", WASM_EXTERN_KIND_TABLE),
        WasmImport("env", "tag_import", WASM_EXTERN_KIND_TAG),
    )

    table = read_wasm_linking_symbols(member, wasm_imports=imports)

    assert table.undefined_names == frozenset(
        {"function_import", "global_import", "table_import", "tag_import"}
    )


@pytest.mark.parametrize(
    ("kind", "wasm_import"),
    [
        (0, WasmImport("env", "global_import", WASM_EXTERN_KIND_GLOBAL)),
        (2, WasmImport("env", "function_import", WASM_EXTERN_KIND_FUNCTION)),
        (5, WasmImport("env", "tag_import", WASM_EXTERN_KIND_TAG)),
        (4, WasmImport("env", "table_import", WASM_EXTERN_KIND_TABLE)),
    ],
)
def test_unnamed_indexed_undefined_symbols_reject_missing_same_kind_import(
    tmp_path: Path,
    kind: int,
    wasm_import: WasmImport,
) -> None:
    member = tmp_path / f"wrong-kind-{kind}.wasm"
    member.write_bytes(_module(_unnamed_indexed(kind, index=0)))

    with pytest.raises(ValueError, match="references missing import"):
        read_wasm_linking_symbols(member, wasm_imports=(wasm_import,))


def test_unnamed_indexed_undefined_symbols_reject_out_of_range_index(
    tmp_path: Path,
) -> None:
    member = tmp_path / "out-of-range.wasm"
    member.write_bytes(_module(_unnamed_indexed(0, index=1)))
    wasm_import = WasmImport("env", "only_function", WASM_EXTERN_KIND_FUNCTION)

    with pytest.raises(ValueError, match=r"missing import function\[1\]"):
        read_wasm_linking_symbols(member, wasm_imports=(wasm_import,))


def test_undefined_names_rejects_unresolved_unnamed_symbol() -> None:
    table = parse_wasm_linking_symbols(_module(_unnamed_indexed(0, index=0)))

    with pytest.raises(ValueError, match=r"unnamed.*function\[0\]"):
        _ = table.undefined_names


def test_selected_import_signature_ignores_unrelated_gc_struct_type() -> None:
    name = "PyLong_FromLong"
    linking_module = _module(_function(name, flags=0x50, index=0))
    function_type = b"\x60" + _u32(1) + b"\x7e" + _u32(1) + b"\x7f"
    empty_struct_type = b"\x5f\x00"
    type_section = _section(1, _u32(2) + function_type + empty_struct_type)
    import_section = _section(
        2,
        _u32(1) + _text("env") + _text(name) + b"\x00" + _u32(0),
    )
    artifact = b"\0asm\x01\0\0\0" + type_section + import_section + linking_module[8:]

    interface = parse_wasm_relocatable_object_interface(
        artifact,
        signature_import_names={name},
    )

    assert interface.function_import_signatures == (("env", name, ("i64",), "i32"),)
    assert interface.linking_symbols.undefined_names == frozenset({name})


def test_rec_group_flattening_reuses_gc_walker_and_preserves_standalone_gc_type() -> (
    None
):
    typed_ref_function = b"\x60" + _u32(1) + b"\x63\x70" + _u32(0)
    i64_function = b"\x60" + _u32(1) + b"\x7e" + _u32(1) + b"\x7e"
    empty_struct = b"\x5f\x00"
    type_payload = (
        _u32(2) + b"\x4e" + _u32(2) + typed_ref_function + i64_function + empty_struct
    )
    artifact = b"\0asm\x01\0\0\0" + _section(1, type_payload)

    original_groups = parse_wasm_type_section_groups(type_payload)
    assert original_groups[0].recursive is True
    assert original_groups[0].entries[0].function_signature == ((b"\x63\x70",), ())

    flattened = flatten_wasm_plain_function_rec_groups(artifact)

    assert flattened is not None
    type_span = next(
        span for span in parse_wasm_section_spans(flattened) if span.id == 1
    )
    flattened_groups = parse_wasm_type_section_groups(
        flattened[type_span.offset : type_span.offset + type_span.size]
    )
    assert [group.recursive for group in flattened_groups] == [False, False, False]
    assert [
        entry.encoding for group in flattened_groups for entry in group.entries
    ] == [
        typed_ref_function,
        i64_function,
        empty_struct,
    ]


def test_rec_group_flattening_rejects_explicit_subtype() -> None:
    explicit_subtype = b"\x50\x00\x60\x00\x00"
    type_payload = _u32(1) + b"\x4e" + _u32(1) + explicit_subtype
    artifact = b"\0asm\x01\0\0\0" + _section(1, type_payload)

    with pytest.raises(ValueError, match="not a plain func type"):
        flatten_wasm_plain_function_rec_groups(artifact)


def test_kind_filtered_definitions_ignore_non_function_data_linking_kinds() -> None:
    table = parse_wasm_linking_symbols(
        _module(
            _function("defined_fn", flags=0, index=0),
            _defined_indexed(2, "defined_global", index=0),
            _defined_indexed(5, "defined_table", index=0),
            _defined_indexed(4, "defined_tag", index=0),
        )
    )

    assert table.defined_names_for_kinds({"defined_fn": "function"}) == frozenset(
        {"defined_fn"}
    )
    assert {"defined_global", "defined_table", "defined_tag"} <= table.defined_names


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
