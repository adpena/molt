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


def _build_custom_section(name: str, payload: bytes = b"") -> tuple[int, bytes]:
    return (0, wasm_link._write_string(name) + payload)


def _attach_function_name(data: bytes, func_idx: int, name: str) -> bytes:
    write_varuint = wasm_link._write_varuint
    sections = wasm_link._parse_sections(data)
    func_name_subsection = bytearray()
    func_name_subsection.extend(write_varuint(1))
    func_name_subsection.extend(write_varuint(func_idx))
    func_name_subsection.extend(wasm_link._write_string(name))

    custom_name_payload = bytearray()
    custom_name_payload.extend(wasm_link._write_string("name"))
    custom_name_payload.append(1)
    custom_name_payload.extend(write_varuint(len(func_name_subsection)))
    custom_name_payload.extend(func_name_subsection)
    sections.insert(0, (0, bytes(custom_name_payload)))
    return wasm_link._build_sections(sections)


def test_strip_nonsemantic_custom_sections_removes_known_sections() -> None:
    stripped = wasm_link._strip_nonsemantic_custom_sections(
        wasm_link._build_sections(
            [
                _build_custom_section(".debug_info", b"x"),
                _build_custom_section("name", b"y"),
                _build_custom_section("producers", b"z"),
                _build_custom_section("target_features", b"w"),
                _build_custom_section("molt.keep", b"keep"),
            ]
        )
    )
    assert stripped is not None
    custom_names = wasm_link._collect_custom_names(stripped)
    assert custom_names == ["molt.keep"]


def test_strip_nonsemantic_custom_sections_returns_none_when_no_match() -> None:
    result = wasm_link._strip_nonsemantic_custom_sections(
        wasm_link._build_sections(
            [
                _build_custom_section("molt.keep"),
                _build_custom_section("custom.vendor"),
            ]
        )
    )
    assert result is None


def test_extract_call_indirect_mangled_names_is_deterministic() -> None:
    payload = (
        b"xxmolt_call_indirect212hbbbbEyy"
        b"zzmolt_call_indirect111h0123abEww"
        b"qqmolt_call_indirect212haaaaErr"
    )
    names = wasm_link._extract_call_indirect_mangled_names(payload)
    assert names["molt_call_indirect1"] == "molt_call_indirect111h0123abE"
    assert names["molt_call_indirect2"] == "molt_call_indirect212haaaaE"


def test_find_output_call_indirect_symbol_uses_name_section(tmp_path: Path) -> None:
    module = _build_minimal_module(wasm_link._write_varuint(0))
    named = _attach_function_name(module, 0, "molt_call_indirect0")
    output = tmp_path / "output.wasm"
    output.write_bytes(named)
    symbols = wasm_link._find_output_call_indirect_symbol(output)
    assert symbols["molt_call_indirect0"] == (0, wasm_link.FLAG_BINDING_GLOBAL)
