from __future__ import annotations

import inspect

import molt.cli as cli
import molt.wasm_artifact as wasm_artifact
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

_WASM_SIGNATURE_READER_NAMES = (
    "_wasm_import_function_result_kinds",
    "_wasm_import_function_signatures",
    "_wasm_export_function_signatures",
)


def _wasm_import(module: str, name: str, kind: int, payload: bytes) -> bytes:
    return (
        wasm_artifact._write_wasm_string(module)
        + wasm_artifact._write_wasm_string(name)
        + bytes([kind])
        + payload
    )


def test_cli_wasm_binary_inspection_authority_is_single_home() -> None:
    """Residual wasm binary readers must live in ``molt.wasm_artifact`` only."""
    cli_wasm_source = inspect.getsource(cli_wasm)
    for name in _CLI_REEXPORTED_WASM_BINARY_READER_NAMES:
        assert not hasattr(cli, name)
        assert not hasattr(cli_wasm, name)
        assert hasattr(wasm_artifact, name)

    cli_source = inspect.getsource(cli)
    for name in _WASM_BINARY_READER_NAMES:
        assert f"def {name}(" not in cli_source
        assert f"def {name}(" not in cli_wasm_source
    for name in _WASM_SIGNATURE_READER_NAMES:
        assert not hasattr(cli, name)
        assert not hasattr(cli_wasm, name)
        assert hasattr(wasm_artifact, name)
        assert f"def {name}(" not in cli_wasm_source


def test_cli_wasm_binary_inspection_reads_import_minima_and_required_names(
    tmp_path,
) -> None:
    import_section = bytearray()
    import_section.extend(wasm_artifact._write_wasm_varuint(3))
    import_section.extend(
        _wasm_import("molt_runtime", "alloc", 0, wasm_artifact._write_wasm_varuint(0))
    )
    import_section.extend(
        _wasm_import(
            "env",
            "__indirect_function_table",
            1,
            b"\x70"
            + wasm_artifact._write_wasm_varuint(0)
            + wasm_artifact._write_wasm_varuint(321),
        )
    )
    import_section.extend(
        _wasm_import(
            "env",
            "memory",
            2,
            wasm_artifact._write_wasm_varuint(0) + wasm_artifact._write_wasm_varuint(2),
        )
    )

    wasm_path = tmp_path / "imports.wasm"
    wasm_path.write_bytes(
        wasm_artifact._build_wasm_sections([(2, bytes(import_section))])
    )

    assert wasm_artifact._read_wasm_table_min(wasm_path) == 321
    assert wasm_artifact._read_wasm_memory_min_bytes(wasm_path) == 2 * 65536
    assert wasm_artifact._collect_wasm_module_import_names(
        wasm_path, "molt_runtime"
    ) == {"alloc"}


def test_cli_wasm_binary_inspection_reads_active_data_end(tmp_path) -> None:
    data_section = bytearray()
    data_section.extend(wasm_artifact._write_wasm_varuint(1))
    data_section.append(0)  # active segment, memory 0
    data_section.append(0x41)  # i32.const
    data_section.extend(wasm_artifact._write_wasm_varuint(4096))
    data_section.append(0x0B)
    data_section.extend(wasm_artifact._write_wasm_varuint(3))
    data_section.extend(b"abc")

    wasm_path = tmp_path / "data.wasm"
    wasm_path.write_bytes(
        wasm_artifact._build_wasm_sections([(11, bytes(data_section))])
    )

    assert wasm_artifact._read_wasm_data_end(wasm_path) == 4099
