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
    "parse_wasm_section_spans",
    "read_wasm_section_spans",
    "read_wasm_code_metrics",
    "read_wasm_function_bodies",
    "parse_wasm_imports",
    "read_wasm_imports",
    "parse_wasm_exports",
    "read_wasm_exports",
    "inspect_wasm_binary",
    "is_valid_wasm_binary",
    "has_nonempty_wasm_code_section",
)

_CLI_REEXPORTED_WASM_BINARY_READER_NAMES = (
    "_read_wasm_ref_func_expr",
    "_read_wasm_data_end",
    "_read_wasm_memory_min_bytes",
    "_read_wasm_table_min",
    "_collect_wasm_module_import_names",
)

_WASM_SIGNATURE_READER_NAMES = (
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
        assert hasattr(wasm_artifact, name)
        assert not hasattr(cli, name)
        assert not hasattr(cli_wasm, name)
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
    assert [
        (wasm_import.module, wasm_import.name, wasm_import.kind)
        for wasm_import in wasm_artifact.read_wasm_imports(wasm_path)
    ] == [
        ("molt_runtime", "alloc", 0),
        ("env", "__indirect_function_table", 1),
        ("env", "memory", 2),
    ]
    assert [
        (wasm_import.module, wasm_import.name, wasm_import.kind)
        for wasm_import in wasm_artifact.parse_wasm_imports(wasm_path.read_bytes())
    ] == [
        ("molt_runtime", "alloc", 0),
        ("env", "__indirect_function_table", 1),
        ("env", "memory", 2),
    ]


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


def test_cli_wasm_binary_inspection_reads_section_and_function_facts(
    tmp_path,
) -> None:
    def _vec(items: list[bytes]) -> bytes:
        return wasm_artifact._write_wasm_varuint(len(items)) + b"".join(items)

    name_records = _vec(
        [
            wasm_artifact._write_wasm_varuint(0)
            + wasm_artifact._write_wasm_string("molt_main")
        ]
    )
    name_payload = (
        wasm_artifact._write_wasm_string("name")
        + b"\x01"
        + wasm_artifact._write_wasm_varuint(len(name_records))
        + name_records
    )
    type_payload = _vec([b"\x60\x00\x00"])
    function_payload = _vec([wasm_artifact._write_wasm_varuint(0)])
    export_payload = _vec(
        [
            wasm_artifact._write_wasm_string("molt_main")
            + b"\x00"
            + wasm_artifact._write_wasm_varuint(0)
        ]
    )
    body = b"\x00\x0b"
    code_payload = _vec([wasm_artifact._write_wasm_varuint(len(body)) + body])

    wasm_path = tmp_path / "function.wasm"
    wasm_path.write_bytes(
        wasm_artifact._build_wasm_sections(
            [
                (0, name_payload),
                (1, type_payload),
                (3, function_payload),
                (7, export_payload),
                (10, code_payload),
            ]
        )
    )

    spans = wasm_artifact.read_wasm_section_spans(wasm_path)
    assert [(span.id, span.name, span.custom_name) for span in spans] == [
        (0, "custom", "name"),
        (1, "type", ""),
        (3, "function", ""),
        (7, "export", ""),
        (10, "code", ""),
    ]
    assert [
        wasm_export.to_tuple()
        for wasm_export in wasm_artifact.read_wasm_exports(wasm_path)
    ] == [("molt_main", 0, 0)]
    assert [
        wasm_export.to_tuple()
        for wasm_export in wasm_artifact.parse_wasm_exports(wasm_path.read_bytes())
    ] == [("molt_main", 0, 0)]
    metrics = wasm_artifact.read_wasm_code_metrics(wasm_path)
    assert metrics.defined_function_count == 1
    assert metrics.code_section_size == len(code_payload)
    assert [
        body_fact.to_dict()
        for body_fact in wasm_artifact.read_wasm_function_bodies(wasm_path)
    ] == [
        {
            "index": 0,
            "offset": spans[-1].offset + 2,
            "body_size_bytes": len(body),
            "name": "molt_main",
        }
    ]
