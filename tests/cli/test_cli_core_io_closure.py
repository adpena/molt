"""CPython io cannot seed Molt-only streaming or compiler package imports."""

from __future__ import annotations

import ast
from pathlib import Path

import pytest

from molt.compiler_analysis.python_import_scan import (
    SourceImportPurpose,
    _collect_imports_for_graph,
)
from molt.target_python_policy import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
from molt.target_python import (
    _parse_target_python_version,
)

ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.parametrize("version", SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS)
@pytest.mark.parametrize("module", ["io", "_io"])
def test_core_io_has_no_extension_or_compiler_dependency(
    version: str, module: str
) -> None:
    path = ROOT / "src/molt/stdlib" / f"{module}.py"
    tree = ast.parse(path.read_text(encoding="utf-8"))
    # Full scans also cover deferred bodies and conservative reentrant branches.
    projection = _collect_imports_for_graph(
        tree,
        module_name=module,
        import_scan_mode="full",
        target_python=_parse_target_python_version(version),
        purpose=SourceImportPurpose.INVENTORY,
    )
    assert not {
        name for name in projection.imports if name.split(".")[0] in {"molt", "moltlib"}
    }
    assert not any(
        isinstance(node, (ast.FunctionDef, ast.ClassDef))
        and node.name in {"stream", "_StreamIter"}
        for node in tree.body
    )
