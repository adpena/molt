from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import wasm as cli_wasm

_WASM_BINARY_READER_NAMES = (
    "_read_wasm_varuint",
    "_read_wasm_string",
    "_read_wasm_ref_func_expr",
    "_read_wasm_varint",
    "_read_wasm_const_expr_i32",
    "_read_wasm_data_end",
    "_read_wasm_memory_min_bytes",
    "_read_wasm_table_min",
    "_collect_wasm_module_import_names",
)

_CLI_REEXPORTED_WASM_BINARY_READER_NAMES = (
    "_read_wasm_ref_func_expr",
    "_read_wasm_data_end",
    "_read_wasm_memory_min_bytes",
    "_read_wasm_table_min",
    "_collect_wasm_module_import_names",
)


def _wasm_import(module: str, name: str, kind: int, payload: bytes) -> bytes:
    return (
        cli_wasm._write_wasm_string(module)
        + cli_wasm._write_wasm_string(name)
        + bytes([kind])
        + payload
    )


def test_cli_wasm_binary_inspection_authority_is_single_home() -> None:
    """Residual wasm binary readers must live in ``molt.cli.wasm`` only."""
    for name in _CLI_REEXPORTED_WASM_BINARY_READER_NAMES:
        assert getattr(cli, name) is getattr(cli_wasm, name)

    cli_source = inspect.getsource(cli)
    for name in _WASM_BINARY_READER_NAMES:
        assert f"def {name}(" not in cli_source


def test_cli_wasm_binary_inspection_reads_import_minima_and_required_names(
    tmp_path,
) -> None:
    import_section = bytearray()
    import_section.extend(cli_wasm._write_wasm_varuint(3))
    import_section.extend(
        _wasm_import("molt_runtime", "alloc", 0, cli_wasm._write_wasm_varuint(0))
    )
    import_section.extend(
        _wasm_import(
            "env",
            "__indirect_function_table",
            1,
            b"\x70" + cli_wasm._write_wasm_varuint(0) + cli_wasm._write_wasm_varuint(321),
        )
    )
    import_section.extend(
        _wasm_import(
            "env",
            "memory",
            2,
            cli_wasm._write_wasm_varuint(0) + cli_wasm._write_wasm_varuint(2),
        )
    )

    wasm_path = tmp_path / "imports.wasm"
    wasm_path.write_bytes(cli_wasm._build_wasm_sections([(2, bytes(import_section))]))

    assert cli_wasm._read_wasm_table_min(wasm_path) == 321
    assert cli_wasm._read_wasm_memory_min_bytes(wasm_path) == 2 * 65536
    assert cli_wasm._collect_wasm_module_import_names(wasm_path, "molt_runtime") == {
        "alloc"
    }


def test_cli_wasm_binary_inspection_reads_active_data_end(tmp_path) -> None:
    data_section = bytearray()
    data_section.extend(cli_wasm._write_wasm_varuint(1))
    data_section.append(0)  # active segment, memory 0
    data_section.append(0x41)  # i32.const
    data_section.extend(cli_wasm._write_wasm_varuint(4096))
    data_section.append(0x0B)
    data_section.extend(cli_wasm._write_wasm_varuint(3))
    data_section.extend(b"abc")

    wasm_path = tmp_path / "data.wasm"
    wasm_path.write_bytes(cli_wasm._build_wasm_sections([(11, bytes(data_section))]))

    assert cli_wasm._read_wasm_data_end(wasm_path) == 4099
