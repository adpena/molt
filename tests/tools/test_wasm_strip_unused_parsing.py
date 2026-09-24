"""tools/wasm_strip_unused.py parses imports through the link-format authority."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

from tools import wasm_link_format as fmt

REPO_ROOT = Path(__file__).resolve().parents[2]


def _load_tool():
    path = REPO_ROOT / "tools" / "wasm_strip_unused.py"
    spec = importlib.util.spec_from_file_location("wasm_strip_unused_under_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # dataclasses resolve the defining module through sys.modules.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _import_module_bytes() -> bytes:
    write_varuint = fmt._write_varuint
    write_string = fmt._write_string
    type_payload = write_varuint(2)
    type_payload += bytes([0x60]) + write_varuint(0) + write_varuint(0)
    type_payload += bytes([0x60]) + write_varuint(1) + bytes([0x7F]) + write_varuint(0)
    entries = [
        write_string("env")
        + write_string("memory")
        + bytes([0x02, 0x00])
        + write_varuint(1),
        write_string("env")
        + write_string("__indirect_function_table")
        + bytes([0x01, 0x70, 0x00])
        + write_varuint(4),
        write_string("molt_runtime")
        + write_string("molt_alloc")
        + bytes([0x00])
        + write_varuint(1),
        write_string("env")
        + write_string("__stack_pointer")
        + bytes([0x03, 0x7F, 0x01]),
    ]
    import_payload = write_varuint(len(entries)) + b"".join(entries)
    return fmt._build_sections([(1, type_payload), (2, import_payload)])


def test_parse_imports_reports_every_kind_with_function_type_index(
    tmp_path: Path,
) -> None:
    tool = _load_tool()
    module = tmp_path / "module.wasm"
    module.write_bytes(_import_module_bytes())

    imports = tool.parse_imports(module)

    assert [(i.index, i.module, i.name, i.kind, i.type_index) for i in imports] == [
        (0, "env", "memory", "memory", -1),
        (1, "env", "__indirect_function_table", "table", -1),
        (2, "molt_runtime", "molt_alloc", "func", 1),
        (3, "env", "__stack_pointer", "global", -1),
    ]
    assert all(isinstance(i.category, tool.ImportCategory) for i in imports)


def test_parse_imports_of_an_importless_module_is_empty(tmp_path: Path) -> None:
    tool = _load_tool()
    module = tmp_path / "module.wasm"
    module.write_bytes(fmt._build_sections([]))

    assert tool.parse_imports(module) == []
