from __future__ import annotations

import ast
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PADDLEOCR = ROOT / "src" / "molt" / "stdlib" / "tinygrad" / "paddleocr.py"
ONNX_INTERPRETER = (
    ROOT / "src" / "molt" / "stdlib" / "tinygrad" / "onnx_interpreter.py"
)


def _imported_top_level_modules(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    imports: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imports.extend(alias.name.split(".")[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module:
            imports.append(node.module.split(".")[0])
    return imports


def test_tinygrad_ocr_stdlib_does_not_import_host_onnx_or_numpy() -> None:
    forbidden = {"onnx", "numpy"}
    imports = {
        path.name: sorted(forbidden.intersection(_imported_top_level_modules(path)))
        for path in (PADDLEOCR, ONNX_INTERPRETER)
    }
    leaked = {name: modules for name, modules in imports.items() if modules}

    assert not leaked, f"compiled OCR stdlib imported host deps: {leaked}"
