from __future__ import annotations

import ast
from functools import lru_cache
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]

GUARDED_ENTRYPOINT_SCAN_ROOTS = (
    "tools",
    "tests/harness",
    "bench",
    "tests/benchmarks",
)

GUARDED_ENTRYPOINT_SKIP_ROOTS = (
    ("bench", "friends", "repos"),
    ("bench", "results"),
)

EXPLICIT_GUARDED_ENTRYPOINTS = (
    "src/molt/cli/__init__.py",
    "src/molt/harness_layers.py",
)

_HARNESS_MEMORY_GUARD_TOKEN = b"harness_memory_guard"


def _path_has_parts(path: Path, parts: tuple[str, ...]) -> bool:
    path_parts = path.parts
    width = len(parts)
    return any(
        path_parts[idx : idx + width] == parts
        for idx in range(0, len(path_parts) - width + 1)
    )


def _skip_guarded_entrypoint_scan(path: Path, *, root: Path) -> bool:
    try:
        rel = path.relative_to(root)
    except ValueError:
        rel = path
    if "__pycache__" in rel.parts:
        return True
    return any(
        _path_has_parts(rel, skipped) for skipped in GUARDED_ENTRYPOINT_SKIP_ROOTS
    )


def _imports_harness_memory_guard(path: Path) -> bool:
    try:
        source_bytes = path.read_bytes()
    except OSError:
        return False
    if _HARNESS_MEMORY_GUARD_TOKEN not in source_bytes:
        return False
    try:
        source = source_bytes.decode("utf-8")
    except UnicodeDecodeError:
        source = source_bytes.decode("utf-8", errors="ignore")
    try:
        tree = ast.parse(source, filename=str(path))
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


@lru_cache(maxsize=8)
def _guarded_entrypoint_tokens(root: Path) -> tuple[str, ...]:
    root = root.resolve()
    entrypoints: list[str] = []
    seen: set[str] = set()

    def add_if_guarded(path: Path) -> None:
        if _skip_guarded_entrypoint_scan(path, root=root):
            return
        if not _imports_harness_memory_guard(path):
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


def guarded_entrypoint_tokens(repo_root: Path | None = None) -> tuple[str, ...]:
    root = (repo_root or REPO_ROOT).resolve()
    return _guarded_entrypoint_tokens(root)
