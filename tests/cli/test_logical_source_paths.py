"""Emitted module paths do not depend on the compiler checkout (HF-372).

A file of the compiler's own ``molt`` package reports its place in that package
below ``/__molt__``; a program module reports the absolute file it was compiled
from, as CPython does (``molt.logical_source_paths``). Each test checks one
projection of that one fact: the module graph's logical paths, the lowered IR,
the module registry origins and the traceback source registry. The native
object proof is tests/cli/test_checkout_independent_objects.py.
"""

from __future__ import annotations

import ast
import json
from pathlib import Path

import pytest

from molt.cli import backend_ir, module_graph
from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease
from molt.frontend import SimpleTIRGenerator
from molt.logical_source_paths import TOOLCHAIN_ROOT

_PACKAGE_FILES = (
    "__init__.py",
    "stdlib/os.py",
    "stdlib/json/__init__.py",
    "gpu/__init__.py",
)


def _checkout(root: Path) -> Path:
    """A compiler checkout that holds only the files the tests name."""
    paths = (
        *(root / "src" / "molt" / relative for relative in _PACKAGE_FILES),
        root / "src" / "molt_extra" / "decoy.py",
        root / "tests" / "prog.py",
    )
    for path in paths:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("VALUE = 1\n", encoding="utf-8")
    return root.resolve()


def _graph(checkout: Path, program: Path) -> dict[str, Path]:
    package = checkout / "src" / "molt"
    return {
        "app": program,
        "molt": package / "__init__.py",
        "os": package / "stdlib" / "os.py",
        "json": package / "stdlib" / "json" / "__init__.py",
        "molt.gpu": package / "gpu" / "__init__.py",
        # A sibling directory whose name extends the package name.
        "decoy": checkout / "src" / "molt_extra" / "decoy.py",
        # A program inside the checkout but outside the compiler package, as
        # the differential corpus is.
        "prog": checkout / "tests" / "prog.py",
        "ns": checkout / "generated" / "ns.py",
    }


def _logical_paths(
    monkeypatch: pytest.MonkeyPatch, checkout: Path, program: Path
) -> dict[str, str]:
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(checkout))
    logical, *_ = module_graph._build_module_lowering_metadata(
        _graph(checkout, program),
        generated_module_source_paths={"ns": "/__molt_generated__/ns.py"},
        entry_module="app",
        entry_execution_kind="script",
        namespace_module_names={"ns"},
    )
    return logical


@pytest.fixture
def checkouts(tmp_path: Path) -> tuple[Path, Path, Path]:
    program = tmp_path / "project" / "app.py"
    program.parent.mkdir()
    program.write_text("import json\n", encoding="utf-8")
    return (
        _checkout(tmp_path / "checkout-a"),
        _checkout(tmp_path / "worktrees" / "checkout-b"),
        program.resolve(),
    )


def test_toolchain_modules_report_one_path_from_every_checkout(
    monkeypatch: pytest.MonkeyPatch, checkouts: tuple[Path, Path, Path]
) -> None:
    first, second, program = checkouts
    logical = _logical_paths(monkeypatch, first, program)
    assert logical == {
        "app": str(program),
        "molt": "/__molt__/__init__.py",
        "os": "/__molt__/stdlib/os.py",
        "json": "/__molt__/stdlib/json/__init__.py",
        "molt.gpu": "/__molt__/gpu/__init__.py",
        "decoy": str(first / "src" / "molt_extra" / "decoy.py"),
        "prog": str(first / "tests" / "prog.py"),
        "ns": "/__molt_generated__/ns.py",
    }
    relocated = _logical_paths(monkeypatch, second, program)
    independent = {"app", "molt", "os", "json", "molt.gpu", "ns"}
    assert {name: relocated[name] for name in independent} == {
        name: logical[name] for name in independent
    }
    # Program files keep their own paths, as CPython reports them.
    assert relocated["prog"] == str(second / "tests" / "prog.py")


_STDLIB_PACKAGE_SOURCE = """\
import sys

def helper(value):
    return value + 1

class Box:
    def get(self):
        return helper(1)
"""


def _lower(source_path: str, module_name: str) -> tuple[SimpleTIRGenerator, str]:
    generator = SimpleTIRGenerator(
        source_path=source_path,
        module_name=module_name,
        module_execution_kind="imported",
    )
    generator.visit(ast.parse(_STDLIB_PACKAGE_SOURCE))
    return generator, json.dumps(generator.to_json(scope="module"), sort_keys=True)


def _const_strings(ir_text: str) -> set[str]:
    strings: set[str] = set()
    for function in json.loads(ir_text)["functions"]:
        strings.update(
            op["s_value"] for op in function["ops"] if op.get("kind") == "const_str"
        )
    return strings


def test_stdlib_ir_reports_its_logical_path_and_keeps_stdlib_policy(
    monkeypatch: pytest.MonkeyPatch, checkouts: tuple[Path, Path, Path]
) -> None:
    first, second, program = checkouts
    lowered = []
    for checkout in (first, second):
        source_path = _logical_paths(monkeypatch, checkout, program)["json"]
        generator, ir_text = _lower(source_path, "json")
        assert generator.stdlib_hint_trust
        assert str(checkout) not in ir_text
        lowered.append(ir_text)
    assert lowered[0] == lowered[1]
    strings = _const_strings(lowered[0])
    # __file__, __spec__.origin and co_filename, then the package __path__.
    assert {"/__molt__/stdlib/json/__init__.py", "/__molt__/stdlib/json"} <= strings
    sources = {
        function.get("source_file") for function in json.loads(lowered[0])["functions"]
    }
    assert sources == {"/__molt__/stdlib/json/__init__.py"}


def test_program_ir_reports_its_compiled_file(
    checkouts: tuple[Path, Path, Path],
) -> None:
    first, _second, _program = checkouts
    program_file = first / "tests" / "prog.py"
    generator, ir_text = _lower(str(program_file), "prog")
    assert not generator.stdlib_hint_trust
    assert str(program_file) in _const_strings(ir_text)
    assert TOOLCHAIN_ROOT not in ir_text


def test_module_registry_origins_are_the_reported_paths(
    monkeypatch: pytest.MonkeyPatch, checkouts: tuple[Path, Path, Path]
) -> None:
    first, _second, program = checkouts
    graph = _graph(first, program)
    logical = _logical_paths(monkeypatch, first, program)
    order = ["app", "json", "os"]
    metadata = module_graph._build_module_graph_metadata(
        {name: graph[name] for name in order},
        stdlib_root=first / "src" / "molt" / "stdlib",
        generated_module_source_paths={},
        entry_module="app",
        entry_execution_kind="script",
        namespace_module_names=(),
    )
    registry = backend_ir._build_module_registry(
        entry_module="app",
        module_graph={name: graph[name] for name in order},
        logical_source_path_by_module=logical,
        module_is_namespace_by_module=metadata.module_is_namespace_by_module,
        module_source_role_by_module=metadata.module_source_role_by_module,
        module_order=order,
        runtime_import_dispatch_roots=set(order),
        native_module_init_specs=(),
        generated_native_init_modules=(),
        spawn_enabled=False,
    )
    origins = {row.name: row.origin for row in registry.rows}
    assert origins == {
        "__main__": str(program),
        "app": str(program),
        "json": "/__molt__/stdlib/json/__init__.py",
        "os": "/__molt__/stdlib/os.py",
    }
    assert str(first) not in json.dumps(registry.registry_json_payload())


def test_traceback_source_registry_lists_only_reported_files(
    monkeypatch: pytest.MonkeyPatch, checkouts: tuple[Path, Path, Path]
) -> None:
    first, _second, program = checkouts
    graph = _graph(first, program)
    logical = _logical_paths(monkeypatch, first, program)
    catalog = _ModuleSourceCatalog(
        {
            name: _ModuleSourceLease.path_backed(graph[name])
            for name in ("app", "json", "os", "prog")
        }
    )
    # A frame names its module's logical path; a stdlib frame never names its
    # checkout file, so registering that file would only embed the checkout.
    assert catalog.source_paths(logical.values()) == tuple(
        sorted((str(program), str(first / "tests" / "prog.py")))
    )
