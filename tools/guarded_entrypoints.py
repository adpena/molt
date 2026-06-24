from __future__ import annotations

import ast
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]

GUARDED_ENTRYPOINT_SCAN_ROOTS = (
    "tools",
    "tests/harness",
    "bench",
    "tests/benchmarks",
)

EXPLICIT_GUARDED_ENTRYPOINTS = (
    "src/molt/cli/__init__.py",
    "src/molt/harness_layers.py",
)


def _imports_harness_memory_guard(path: Path) -> bool:
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except SyntaxError:
        return False
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            if any(
                alias.name in {"harness_memory_guard", "tools.harness_memory_guard"}
                for alias in node.names
            ):
                return True
        elif isinstance(node, ast.ImportFrom) and node.module == "tools":
            if any(alias.name == "harness_memory_guard" for alias in node.names):
                return True
    return False


def guarded_entrypoint_tokens(repo_root: Path | None = None) -> tuple[str, ...]:
    root = (repo_root or REPO_ROOT).resolve()
    entrypoints: list[str] = []
    seen: set[str] = set()

    def add_if_guarded(path: Path) -> None:
        if "__pycache__" in path.parts or not _imports_harness_memory_guard(path):
            return
        token = "/" + path.relative_to(root).as_posix()
        if token not in seen:
            seen.add(token)
            entrypoints.append(token)

    for rel_root in GUARDED_ENTRYPOINT_SCAN_ROOTS:
        base = root / rel_root
        if not base.exists():
            continue
        for path in sorted(base.rglob("*.py")):
            add_if_guarded(path)

    for rel_path in EXPLICIT_GUARDED_ENTRYPOINTS:
        path = root / rel_path
        if path.exists():
            add_if_guarded(path)

    return tuple(entrypoints)
