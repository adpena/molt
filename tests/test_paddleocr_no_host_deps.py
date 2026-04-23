from __future__ import annotations

import ast
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PADDLEOCR = ROOT / "src" / "molt" / "stdlib" / "tinygrad" / "paddleocr.py"


def test_paddleocr_stdlib_does_not_import_host_onnx_or_numpy() -> None:
    tree = ast.parse(PADDLEOCR.read_text(encoding="utf-8"))
    forbidden = {"onnx", "numpy"}
    imports: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imports.extend(alias.name.split(".")[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module:
            imports.append(node.module.split(".")[0])

    leaked = sorted(forbidden.intersection(imports))
    assert not leaked, f"compiled PaddleOCR stdlib imported host deps: {leaked}"
