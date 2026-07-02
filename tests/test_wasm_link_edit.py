from __future__ import annotations

import tempfile

from tools import wasm_link_edit
from tools.wasm_link_format import (
    FLAG_BINDING_GLOBAL,
    FLAG_EXPORTED,
    FLAG_EXPLICIT_NAME,
    FLAG_NO_STRIP,
    _write_varuint,
    table_ref_export_name,
)


def _module_with_table_ref_export(*, export_index: int) -> bytes:
    table_ref = table_ref_export_name(4096)
    type_payload = bytearray()
    type_payload.extend(_write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(_write_varuint(0))
    type_payload.extend(_write_varuint(0))

    import_payload = bytearray()
    import_payload.extend(_write_varuint(1))
    import_payload.extend(wasm_link_edit._write_string("molt_runtime"))
    import_payload.extend(wasm_link_edit._write_string("time_timezone"))
    import_payload.append(0x00)
    import_payload.extend(_write_varuint(0))

    function_payload = _write_varuint(1) + _write_varuint(0)

    export_payload = bytearray()
    export_payload.extend(_write_varuint(1))
    export_payload.extend(wasm_link_edit._write_string(table_ref))
    export_payload.append(0x00)
    export_payload.extend(_write_varuint(export_index))

    body = b"\x00\x0b"
    code_payload = _write_varuint(1) + _write_varuint(len(body)) + body

    return wasm_link_edit._build_sections(
        [
            (1, bytes(type_payload)),
            (2, bytes(import_payload)),
            (3, bytes(function_payload)),
            (7, bytes(export_payload)),
            (10, bytes(code_payload)),
        ]
    )


def test_table_ref_export_symbol_map_uses_stable_public_name_without_symtab() -> None:
    table_ref = table_ref_export_name(4096)
    data = _module_with_table_ref_export(export_index=1)

    assert wasm_link_edit._collect_output_export_symbol_map(data)[table_ref] == table_ref


def test_table_ref_export_symbols_restore_after_import_prefix_growth(tmp_path) -> None:
    table_ref = table_ref_export_name(4096)
    output = tmp_path / "output.wasm"
    output.write_bytes(_module_with_table_ref_export(export_index=1))

    temp_dir = tempfile.TemporaryDirectory(dir=tmp_path)
    try:
        symbolized = wasm_link_edit._inject_table_ref_export_symbols(output, temp_dir)
        symbols = wasm_link_edit._collect_linking_function_symbols(symbolized.read_bytes())
    finally:
        temp_dir.cleanup()

    assert any(
        name == table_ref
        and index == 1
        and (flags & (FLAG_BINDING_GLOBAL | FLAG_EXPLICIT_NAME | FLAG_EXPORTED | FLAG_NO_STRIP))
        == (FLAG_BINDING_GLOBAL | FLAG_EXPLICIT_NAME | FLAG_EXPORTED | FLAG_NO_STRIP)
        for flags, index, name, _kind in symbols
    )

    linked_like = _module_with_table_ref_export(export_index=0)
    linked_like = wasm_link_edit._append_linking_function_symbols(
        linked_like,
        [
            (
                table_ref,
                1,
                FLAG_BINDING_GLOBAL | FLAG_EXPLICIT_NAME | FLAG_EXPORTED | FLAG_NO_STRIP,
            )
        ],
    )
    assert linked_like is not None

    restored = wasm_link_edit._ensure_function_exports_by_symbol_names(
        linked_like, {table_ref: table_ref}
    )
    assert restored is not None
    assert wasm_link_edit._collect_function_exports(restored)[table_ref] == 1
