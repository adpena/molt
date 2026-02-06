from __future__ import annotations

import ast
from pathlib import Path

import molt.cli as cli


ROOT = Path(__file__).resolve().parents[2]


def _discover_with_core_modules(entry: Path) -> dict[str, Path]:
    stdlib_root = cli._stdlib_root_path()
    module_roots = [ROOT.resolve(), (ROOT / "src").resolve(), entry.parent.resolve()]
    roots = module_roots + [stdlib_root]
    stdlib_allowlist = cli._stdlib_allowlist()
    module_graph, _ = cli._discover_module_graph(
        entry,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        skip_modules=cli.STUB_MODULES,
        stub_parents=cli.STUB_PARENT_MODULES,
    )
    cli._collect_package_parents(module_graph, roots, stdlib_root, stdlib_allowlist)
    cli._ensure_core_stdlib_modules(module_graph, stdlib_root)
    core_paths = [
        path
        for name in ("builtins", "sys")
        if (path := module_graph.get(name)) is not None
    ]
    for core_path in core_paths:
        core_graph, _ = cli._discover_module_graph(
            core_path,
            roots,
            module_roots,
            stdlib_root,
            stdlib_allowlist,
            skip_modules=cli.STUB_MODULES,
            stub_parents=cli.STUB_PARENT_MODULES,
        )
        for name, path in core_graph.items():
            module_graph.setdefault(name, path)
    return module_graph


def test_collect_imports_can_skip_nested_imports() -> None:
    tree = ast.parse(
        "import os\n"
        "def f() -> None:\n"
        "    import warnings\n"
        "class C:\n"
        "    import re\n"
    )
    nested = cli._collect_imports(tree)
    top_level_only = cli._collect_imports(tree, include_nested=False)
    assert "warnings" in nested
    assert "re" in nested
    assert "warnings" not in top_level_only
    assert "re" not in top_level_only
    assert "os" in top_level_only


def test_stdlib_graph_ignores_nested_imports_for_core_scan(tmp_path: Path) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print(1)\n")
    graph = _discover_with_core_modules(entry)
    assert "builtins" in graph
    assert "sys" in graph
    assert "warnings" not in graph
    assert "re" not in graph
    assert "dataclasses" not in graph
