from __future__ import annotations

import ast
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"
EXCLUDED_PATHS = {
    STDLIB_ROOT / "datetime.py",
}


def _has_nested_require_intrinsic_use(tree: ast.AST) -> bool:
    class Visitor(ast.NodeVisitor):
        def __init__(self) -> None:
            self.scope = 0
            self.nested_use = False

        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            self.scope += 1
            self.generic_visit(node)
            self.scope -= 1

        visit_AsyncFunctionDef = visit_FunctionDef

        def visit_ClassDef(self, node: ast.ClassDef) -> None:
            self.scope += 1
            self.generic_visit(node)
            self.scope -= 1

        def visit_Lambda(self, node: ast.Lambda) -> None:
            self.scope += 1
            self.generic_visit(node)
            self.scope -= 1

        def visit_Name(self, node: ast.Name) -> None:
            if node.id == "_require_intrinsic" and self.scope > 0:
                self.nested_use = True

    visitor = Visitor()
    visitor.visit(tree)
    return visitor.nested_use


def test_ast_safe_private_intrinsic_helper_lane_is_exhausted() -> None:
    remaining: list[str] = []

    for path in sorted(STDLIB_ROOT.rglob("*.py")):
        if path in EXCLUDED_PATHS:
            continue
        text = path.read_text(encoding="utf-8")
        if "require_intrinsic as _require_intrinsic" not in text:
            continue
        if 'globals().pop("_require_intrinsic", None)' in text:
            continue
        tree = ast.parse(text)
        if _has_nested_require_intrinsic_use(tree):
            continue
        remaining.append(str(path.relative_to(REPO_ROOT)))

    assert remaining == []
