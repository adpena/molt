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
