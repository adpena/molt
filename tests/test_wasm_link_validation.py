import importlib.util
from pathlib import Path


def _load_wasm_link():
    root = Path(__file__).resolve().parents[1]
    path = root / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


wasm_link = _load_wasm_link()


def test_wasm_link_default_artifact_paths_use_canonical_dist(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_DIR", raising=False)

    assert wasm_link._default_input_path() == Path("dist") / "output.wasm"
    assert wasm_link._default_output_path() == Path("dist") / "output_linked.wasm"


def test_wasm_link_default_artifact_paths_follow_external_root(
    tmp_path: Path,
    monkeypatch,
) -> None:
    ext_root = tmp_path / "ext-root"
    ext_root.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    monkeypatch.delenv("MOLT_WASM_RUNTIME_DIR", raising=False)

    assert wasm_link._default_input_path() == ext_root / "dist" / "output.wasm"
    assert wasm_link._default_output_path() == Path(
        ext_root / "dist" / "output_linked.wasm"
    )


def _build_minimal_module(element_payload: bytes) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections = []

    # Type section: one empty function type.
    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    # Function section: one function of type 0.
    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, func_payload))

    # Table section: one funcref table with min 1.
    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    # Code section: one empty function body.
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    # Element section.
    sections.append((9, element_payload))

    return wasm_link._build_sections(sections)


def _build_symbol_subsection(entries: list[bytes]) -> bytes:
    return wasm_link._write_varuint(len(entries)) + b"".join(entries)


def _function_symbol_entry(*, flags: int, index: int | None, name: str | None) -> bytes:
    entry = bytearray()
    entry.append(wasm_link.SYMBOL_KIND_FUNCTION)
    entry.extend(wasm_link._write_varuint(flags))
    assert index is not None
    entry.extend(wasm_link._write_varuint(index))
    if not (flags & wasm_link.FLAG_UNDEFINED) or (flags & wasm_link.FLAG_EXPLICIT_NAME):
        assert name is not None
        entry.extend(wasm_link._write_string(name))
    return bytes(entry)


def _data_symbol_entry(
    *,
    flags: int,
    name: str | None,
    segment_index: int = 0,
    offset: int = 0,
    size: int = 0,
) -> bytes:
    entry = bytearray()
    entry.append(1)
    entry.extend(wasm_link._write_varuint(flags))
    if flags & (wasm_link.FLAG_EXPLICIT_NAME | wasm_link.FLAG_UNDEFINED):
        assert name is not None
        entry.extend(wasm_link._write_string(name))
    if not (flags & wasm_link.FLAG_UNDEFINED):
        entry.extend(wasm_link._write_varuint(segment_index))
        entry.extend(wasm_link._write_varuint(offset))
        entry.extend(wasm_link._write_varuint(size))
    return bytes(entry)


def _module_with_linking_symbols(entries: list[bytes]) -> bytes:
    linking_payload = wasm_link._build_linking_payload(
        2,
        [(wasm_link.SYMTAB_SUBSECTION_ID, _build_symbol_subsection(entries))],
    )
    custom = wasm_link._build_custom_section("linking", linking_payload)
    return wasm_link._build_sections([(0, custom)])


def _parse_data_segments(data: bytes) -> list[bytes]:
    sections = wasm_link._parse_sections(data)
    for section_id, payload in sections:
        if section_id != 11:
            continue
        offset = 0
        seg_count, offset = wasm_link._read_varuint(payload, offset)
        out: list[bytes] = []
        parse_offset = offset
        for _ in range(seg_count):
            flags = payload[parse_offset]
            parse_offset += 1
            if flags == 0:
                parse_offset = wasm_link._skip_init_expr(payload, parse_offset)
            elif flags == 1:
                pass
            elif flags == 2:
                _, parse_offset = wasm_link._read_varuint(payload, parse_offset)
                parse_offset = wasm_link._skip_init_expr(payload, parse_offset)
            else:
                raise AssertionError(f"unexpected data segment flags: {flags}")
            data_len, parse_offset = wasm_link._read_varuint(payload, parse_offset)
            out.append(payload[parse_offset : parse_offset + data_len])
            parse_offset += data_len
        return out
    return []


def test_wasm_link_allows_ref_func_element_expr() -> None:
    write_varuint = wasm_link._write_varuint
    payload = bytearray()
    payload.extend(write_varuint(1))  # count
    payload.extend(write_varuint(0x04))  # active, elemtype + exprs
    payload.extend(b"\x41\x00\x0b")  # i32.const 0; end
    payload.append(0x70)  # funcref
    payload.extend(write_varuint(1))
    payload.append(0xD2)  # ref.func
    payload.extend(write_varuint(0))
    payload.append(0x0B)  # end
    data = _build_minimal_module(bytes(payload))
    ok, err = wasm_link._validate_elements(data)
    assert ok, err


def test_collect_linking_function_symbols_parses_defined_and_undefined_entries() -> None:
    data = _module_with_linking_symbols(
        [
            _data_symbol_entry(
                flags=wasm_link.FLAG_EXPLICIT_NAME,
                name="not_a_function",
                segment_index=3,
                offset=12,
                size=8,
            ),
            _function_symbol_entry(
                flags=wasm_link.FLAG_BINDING_GLOBAL | wasm_link.FLAG_EXPLICIT_NAME,
                index=7,
                name="molt_call_indirect0",
            ),
            _function_symbol_entry(
                flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                index=11,
                name="molt_call_indirect13",
            ),
        ]
    )

    symbols = wasm_link._collect_linking_function_symbols(data)

    assert [(flags, index, name) for flags, index, name, _ in symbols] == [
        (
            wasm_link.FLAG_BINDING_GLOBAL | wasm_link.FLAG_EXPLICIT_NAME,
            7,
            "molt_call_indirect0",
        ),
        (
            wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
            11,
            "molt_call_indirect13",
        ),
    ]


def test_call_indirect_symbol_discovery_does_not_require_wasm_tools(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime = tmp_path / "runtime_reloc.wasm"
    runtime.write_bytes(
        _module_with_linking_symbols(
            [
                _function_symbol_entry(
                    flags=wasm_link.FLAG_UNDEFINED | wasm_link.FLAG_EXPLICIT_NAME,
                    index=3,
                    name="_ZN4molt19molt_call_indirect1317hfeedfaceE",
                )
            ]
        )
    )
    output = tmp_path / "output.wasm"
    output.write_bytes(
        _module_with_linking_symbols(
            [
                _function_symbol_entry(
                    flags=wasm_link.FLAG_BINDING_GLOBAL
                    | wasm_link.FLAG_EXPLICIT_NAME
                    | wasm_link.FLAG_EXPORTED,
                    index=41,
                    name="molt_call_indirect13",
                )
            ]
        )
    )
    monkeypatch.setattr(wasm_link, "_find_tool", lambda _names: None)

    mangled = wasm_link._find_call_indirect_mangled(runtime)
    output_symbols = wasm_link._find_output_call_indirect_symbol(output)

    assert mangled == {
        "molt_call_indirect13": "_ZN4molt19molt_call_indirect1317hfeedfaceE"
    }
    assert output_symbols["molt_call_indirect13"] == (
        41,
        wasm_link.FLAG_BINDING_GLOBAL
        | wasm_link.FLAG_EXPLICIT_NAME
        | wasm_link.FLAG_EXPORTED,
    )


def test_wasm_link_allows_ref_null_element_expr() -> None:
    write_varuint = wasm_link._write_varuint
    payload = bytearray()
    payload.extend(write_varuint(1))
    payload.extend(write_varuint(0x04))
    payload.extend(b"\x41\x00\x0b")
    payload.append(0x70)
    payload.extend(write_varuint(1))
    payload.append(0xD0)  # ref.null
    payload.append(0x70)  # funcref
    payload.append(0x0B)
    data = _build_minimal_module(bytes(payload))
    ok, err = wasm_link._validate_elements(data)
    assert ok, err


def test_append_table_ref_elements_tolerates_malformed_name_utf8() -> None:
    write_varuint = wasm_link._write_varuint
    data = _build_minimal_module(write_varuint(0))
    sections = wasm_link._parse_sections(data)

    func_name_subsection = bytearray()
    func_name_subsection.extend(write_varuint(1))  # one function-name mapping
    func_name_subsection.extend(write_varuint(0))  # func index
    func_name_subsection.extend(write_varuint(1))  # name length
    func_name_subsection.extend(b"\x97")  # invalid UTF-8 byte

    custom_name_payload = bytearray()
    custom_name_payload.extend(wasm_link._write_string("name"))
    custom_name_payload.append(1)  # function names subsection
    custom_name_payload.extend(write_varuint(len(func_name_subsection)))
    custom_name_payload.extend(func_name_subsection)

    sections.insert(0, (0, bytes(custom_name_payload)))
    malformed = wasm_link._build_sections(sections)

    # Malformed name entries should be ignored, not crash wasm linking.
    result = wasm_link._append_table_ref_elements(malformed)
    assert result is None or isinstance(result, bytes)


def test_append_table_ref_elements_uses_export_names_without_name_section() -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string("__molt_table_ref_0"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)
    updated = wasm_link._append_table_ref_elements(data)
    assert updated is not None
    ok, err = wasm_link._validate_elements(updated)
    assert ok, err


def test_strip_internal_exports_keeps_table_ref_exports() -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(2))
    export_payload.extend(wasm_link._write_string("__molt_table_ref_7"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)
    updated = wasm_link._strip_internal_exports(data)
    exports = wasm_link._collect_function_exports(updated or data)
    assert "__molt_table_ref_7" in exports
    assert "molt_main" in exports


def test_required_linked_table_min_respects_exported_table_refs() -> None:
    write_varuint = wasm_link._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link._write_string("env"))
    import_payload.extend(wasm_link._write_string("__indirect_function_table"))
    import_payload.append(0x01)
    import_payload.append(0x70)
    import_payload.extend(write_varuint(0))
    import_payload.extend(write_varuint(10))
    sections.append((2, bytes(import_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link._write_string("__molt_table_ref_20"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)

    assert wasm_link._table_import_min(data) == 10
    assert wasm_link._required_linked_table_min(data, 5) == 21
    updated = wasm_link._rewrite_table_import_min(
        data, wasm_link._required_linked_table_min(data, 5)
    )
    assert updated is not None
    assert wasm_link._table_import_min(updated) == 21


def test_neutralize_dead_element_entries_skips_modules_with_call_indirect() -> None:
    write_varuint = wasm_link._write_varuint
    sections = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, func_payload))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x41\x00\x0b")
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    sections.append((9, bytes(element_payload)))

    code_payload = bytearray()
    body = bytearray()
    body.extend(write_varuint(0))  # local decl count
    body.extend(b"\x41\x00")      # i32.const 0
    body.extend(b"\x11\x00\x00")  # call_indirect type 0 table 0
    body.append(0x0B)               # end
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    data = wasm_link._build_sections(sections)
    assert wasm_link._neutralize_dead_element_entries(data) is None


def test_dedup_data_segments_stops_scrub_at_path_extension_boundary() -> None:
    write_varuint = wasm_link._write_varuint
    sections = []

    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(1))
    sections.append((5, bytes(memory_payload)))

    path_and_adjacent = (
        b"/Users/alice/project/tmp/class_method_probe.py"
        b"f__name__hi"
    )
    second_segment = b"keep-me"

    data_payload = bytearray()
    data_payload.extend(write_varuint(2))
    for offset, raw in ((0, path_and_adjacent), (128, second_segment)):
        data_payload.append(0x00)
        data_payload.extend(b"\x41")
        data_payload.extend(write_varuint(offset))
        data_payload.extend(b"\x0b")
        data_payload.extend(write_varuint(len(raw)))
        data_payload.extend(raw)
    sections.append((11, bytes(data_payload)))

    data = wasm_link._build_sections(sections)
    updated = wasm_link._dedup_data_segments(data)
    assert updated is not None

    segs = _parse_data_segments(updated)
    assert segs[0].endswith(b"f__name__hi")
    assert b"/Users/" not in segs[0]
    assert segs[1] == second_segment


# ---------------------------------------------------------------------------
# Allowlist validation
# ---------------------------------------------------------------------------


def _parse_allowlist(path: Path) -> set[str]:
    lines = path.read_text().splitlines()
    return {
        line.strip()
        for line in lines
        if line.strip() and not line.strip().startswith("#")
    }


def test_allowlist_file_exists():
    """The WASI allowlist must exist and contain the expected symbols."""
    allowlist = Path(__file__).resolve().parents[1] / "tools" / "wasm_allowed_imports.txt"
    assert allowlist.exists(), f"Missing allowlist: {allowlist}"
    symbols = _parse_allowlist(allowlist)
    # Must contain core WASI symbols
    assert "fd_write" in symbols
    assert "proc_exit" in symbols
    assert "__indirect_function_table" in symbols
    # Must contain indirect call trampolines
    assert "molt_call_indirect0" in symbols
    assert "molt_call_indirect13" in symbols
    # Must NOT contain molt_runtime namespace symbols (those are resolved by linking),
    # except for serialization/compression builtins that are direct WASM imports.
    _ALLOWED_MOLT_PREFIXES = (
        "molt_call_indirect",
        "molt_cbor_",
        "molt_msgpack_",
        "molt_deflate_",
        "molt_inflate_",
    )
    runtime_syms = {
        s for s in symbols
        if s.startswith("molt_") and not any(s.startswith(p) for p in _ALLOWED_MOLT_PREFIXES)
    }
    assert runtime_syms == set(), f"Unexpected molt_runtime symbols in allowlist: {runtime_syms}"
