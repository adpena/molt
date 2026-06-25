from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import wrapper_build

_MODULE_GRAPH_CORE_NAMES = (
    "ModuleSyntaxErrorInfo",
    "_build_module_graph_metadata",
    "_collect_package_parents",
    "_module_dependencies",
    "_module_dependencies_from_imports",
    "_module_dependency_closure",
    "_module_dependency_closures",
    "_module_dependency_layers",
    "_module_order_has_back_edges",
    "_topo_sort_modules",
    "_analyze_module_schedule",
    "_reverse_module_dependencies",
    "_dependent_module_closure",
    "_compute_reachable_modules",
    "_apply_dead_module_elimination",
    "_discover_module_graph",
    "_discover_module_graph_from_paths",
    "_extend_module_graph_with_static_import_modules",
    "_load_module_imports",
    "_materialize_import_plan",
    "_parse_static_import_modules",
    "_prepare_entry_module_graph",
)

_MODULE_GRAPH_CORE_DEFINITIONS = (
    "class ModuleSyntaxErrorInfo",
    "def _build_module_graph_metadata(",
    "def _collect_package_parents(",
    "def _module_dependencies(",
    "def _module_dependencies_from_imports(",
    "def _module_dependency_closure(",
    "def _module_dependency_closures(",
    "def _module_dependency_layers(",
    "def _module_order_has_back_edges(",
    "def _topo_sort_modules(",
    "def _analyze_module_schedule(",
    "def _reverse_module_dependencies(",
    "def _dependent_module_closure(",
    "def _compute_reachable_modules(",
    "def _apply_dead_module_elimination(",
    "_DEAD_MODULE_ELIMINATION_SAFELIST",
    "def _discover_module_graph(",
    "def _discover_module_graph_from_paths(",
    "def _extend_module_graph_with_static_import_modules(",
    "def _load_module_imports(",
    "def _materialize_import_plan(",
    "def _parse_static_import_modules(",
    "def _prepare_entry_module_graph(",
)

def test_cli_module_graph_core_authority_is_single_home() -> None:
    for name in _MODULE_GRAPH_CORE_NAMES:
        assert getattr(cli, name) is getattr(module_graph, name)

    cli_source = inspect.getsource(cli)
    for marker in _MODULE_GRAPH_CORE_DEFINITIONS:
        assert marker not in cli_source


def test_wrapper_build_cache_uses_module_graph_core_directly() -> None:
    source = inspect.getsource(wrapper_build)
    assert "cli._ModuleResolutionCache" not in source
    assert "cli._source_content_sha256" not in source
    assert "cli._stdlib_root_path" not in source
    assert "cli._discover_module_graph" not in source
    assert "cli._extend_module_graph_with_static_import_modules" not in source
    assert "cli._parse_static_import_modules" not in source
