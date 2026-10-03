from __future__ import annotations

import ast
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PADDLEOCR = ROOT / "demos" / "tinygrad" / "paddleocr.py"
ONNX_INTERPRETER = ROOT / "demos" / "tinygrad" / "onnx_interpreter.py"


def _imported_top_level_modules(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    imports: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imports.extend(alias.name.split(".")[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module:
            imports.append(node.module.split(".")[0])
    return imports


def _tinygrad_imports(path: Path) -> set[tuple[str, str | None]]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    imports: set[tuple[str, str | None]] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imports.update(
                (alias.name, None)
                for alias in node.names
                if alias.name == "tinygrad" or alias.name.startswith("tinygrad.")
            )
        elif (
            isinstance(node, ast.ImportFrom)
            and node.module
            and (node.module == "tinygrad" or node.module.startswith("tinygrad."))
        ):
            imports.update((node.module, alias.name) for alias in node.names)
    return imports


def test_tinygrad_ocr_demo_does_not_import_host_onnx_or_numpy() -> None:
    forbidden = {"onnx", "numpy"}
    imports = {
        path.name: sorted(forbidden.intersection(_imported_top_level_modules(path)))
        for path in (PADDLEOCR, ONNX_INTERPRETER)
    }
    leaked = {name: modules for name, modules in imports.items() if modules}

    assert not leaked, f"compiled OCR demo imported host deps: {leaked}"


def test_onnx_interpreter_uses_only_public_tinygrad_modules() -> None:
    tree = ast.parse(ONNX_INTERPRETER.read_text(encoding="utf-8"))
    assert _tinygrad_imports(ONNX_INTERPRETER) == {
        ("tinygrad.dtypes", "dtypes"),
        ("tinygrad.tensor", "Tensor"),
    }

    forbidden_names = {"LazyBuffer", "LazyOp", "lazydata", "require_intrinsic"}
    referenced_names = {
        node.id for node in ast.walk(tree) if isinstance(node, ast.Name)
    }
    referenced_attributes = {
        node.attr for node in ast.walk(tree) if isinstance(node, ast.Attribute)
    }
    assert forbidden_names.isdisjoint(referenced_names)
    assert {"_broadcast_to", "lazydata"}.isdisjoint(referenced_attributes)
