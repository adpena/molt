from __future__ import annotations

import ast
import inspect
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_import_scanner
from molt.compiler_analysis import python_binding_flow
from molt.target_python import TargetPythonVersion

_MODULE_IMPORT_SCANNER_NAMES = (
    "IMPORTER_MODULE_NAME",
    "STDLIB_STATIC_IMPORT_HELPER_MODULES",
    "STDLIB_STATIC_IMPORT_HELPER_QUALNAMES",
    "_IMPORT_SCAN_MODES",
    "_RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES",
    "_RUNTIME_IMPORT_PROTOCOL_MARKERS",
    "_RUNTIME_IMPORT_PROTOCOL_TARGETS",
    "_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES",
    "_DynamicRelativeImportDiscovery",
    "_collect_import_star_modules",
    "_collect_imports",
    "_collect_imports_for_graph",
    "_expand_imports_with_static_package_all_star_children",
    "_expand_imports_with_static_package_all_star_children_for_graph",
    "_explicit_imports_reference_generated_importer",
    "_module_graph_needs_runtime_import_support",
    "_module_init_static_helper_scan_nodes",
    "_module_init_scan_nodes",
    "_module_uses_runtime_import_protocol",
    "_qualified_child",
    "_resolve_runtime_import_expr_name",
    "_runtime_import_alias_bindings",
    "_source_may_use_runtime_import_protocol",
    "_StaticImportCallPayload",
    "_static_import_helper_qualnames",
    "_static_module_all_exports",
    "_static_string_sequence",
    "_tree_uses_runtime_import_protocol",
    "_validate_import_scan_mode",
)

_MODULE_IMPORT_SCANNER_DEFINITIONS = (
    "IMPORTER_MODULE_NAME =",
    "STDLIB_STATIC_IMPORT_HELPER_MODULES =",
    "STDLIB_STATIC_IMPORT_HELPER_QUALNAMES:",
    "_IMPORT_SCAN_MODES =",
    "_RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES =",
    "_RUNTIME_IMPORT_PROTOCOL_MARKERS =",
    "_RUNTIME_IMPORT_PROTOCOL_TARGETS =",
    "_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES =",
    "class _DynamicRelativeImportDiscovery:",
    "def _collect_import_star_modules(",
    "def _collect_imports(",
    "def _collect_imports_for_graph(",
    "def _expand_imports_with_static_package_all_star_children(",
    "def _expand_imports_with_static_package_all_star_children_for_graph(",
    "def _explicit_imports_reference_generated_importer(",
    "def _module_graph_needs_runtime_import_support(",
    "def _module_init_static_helper_scan_nodes(",
    "def _module_init_scan_nodes(",
    "def _module_uses_runtime_import_protocol(",
    "def _qualified_child(",
    "def _resolve_runtime_import_expr_name(",
    "def _runtime_import_alias_bindings(",
    "def _source_may_use_runtime_import_protocol(",
    "class _StaticImportCallPayload:",
    "def _static_import_helper_qualnames(",
    "def _static_module_all_exports(",
    "def _static_string_sequence(",
    "def _tree_uses_runtime_import_protocol(",
    "def _validate_import_scan_mode(",
)


def test_cli_module_import_scanner_authority_is_single_home() -> None:
    for name in _MODULE_IMPORT_SCANNER_NAMES:
        assert hasattr(module_import_scanner, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_IMPORT_SCANNER_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source


def test_default_argument_dunder_import_helper_is_in_static_closure() -> None:
    tree = ast.parse(
        "def load(name, importer=__import__):\n"
        "    return importer(name)\n"
        "load('pkg.leaf')\n"
    )

    assert "pkg.leaf" in module_import_scanner._collect_imports(tree)


def test_direct_synthetic_scans_retain_digest_fallback(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    loaded = tmp_path / "loaded.py"
    loaded.write_text("VALUE = 1\n", encoding="utf-8")
    tree = ast.parse(
        "from pkg import *\n"
        "from importlib.util import spec_from_file_location\n"
        "spec_from_file_location('loaded', 'loaded.py')\n"
    )
    digest = python_binding_flow.python_ast_digest
    calls = 0

    def record_digest(candidate: ast.AST) -> str:
        nonlocal calls
        calls += 1
        return digest(candidate)

    monkeypatch.setattr(python_binding_flow, "python_ast_digest", record_digest)
    assert "pkg" in module_import_scanner._collect_imports(tree)
    assert module_import_scanner._collect_import_star_modules(tree) == ("pkg",)
    assert module_import_scanner._collect_static_source_executions(
        tree, source_path=tmp_path / "entry.py"
    ) == (module_import_scanner._StaticSourceExecution("loaded", loaded.resolve()),)
    assert calls == 3


def test_static_scan_uses_selected_target_annotation_policy() -> None:
    tree = ast.parse(
        "items[(__package__ := 'target')]: "
        "globals().__setitem__('__package__', 'annotation')\n"
        "from . import *\n"
    )

    eager = module_import_scanner._collect_import_star_modules(
        tree,
        module_name="pkg.entry",
        target_python=TargetPythonVersion(3, 13, 0),
    )
    deferred = module_import_scanner._collect_import_star_modules(
        tree,
        module_name="pkg.entry",
        target_python=TargetPythonVersion(3, 14, 0),
    )

    assert eager == ("annotation",)
    assert deferred == ("target",)
