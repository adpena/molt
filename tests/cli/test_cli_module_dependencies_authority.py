from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_dependencies
from molt.cli import module_graph

_MODULE_DEPENDENCY_NAMES = (
    "_DEAD_MODULE_ELIMINATION_SAFELIST",
    "_PURE_WASM_DEAD_MODULE_ELIMINATION_SAFELIST",
    "_analyze_module_schedule",
    "_apply_dead_module_elimination",
    "_compute_reachable_modules",
    "_dependent_module_closure",
    "_expand_module_chain",
    "_expand_module_chain_cached",
    "_module_dependencies",
    "_module_dependencies_from_imports",
    "_module_dependency_closure",
    "_module_dependency_closures",
    "_module_dependency_layers",
    "_module_order_has_back_edges",
    "_reverse_module_dependencies",
    "_topo_sort_modules",
)

_MODULE_DEPENDENCY_DEFINITIONS = (
    "_DEAD_MODULE_ELIMINATION_SAFELIST",
    "_PURE_WASM_DEAD_MODULE_ELIMINATION_SAFELIST",
    "def _analyze_module_schedule(",
    "def _apply_dead_module_elimination(",
    "def _compute_reachable_modules(",
    "def _dependent_module_closure(",
    "def _expand_module_chain(",
    "def _expand_module_chain_cached(",
    "def _module_dependencies(",
    "def _module_dependencies_from_imports(",
    "def _module_dependency_closure(",
    "def _module_dependency_closures(",
    "def _module_dependency_layers(",
    "def _module_order_has_back_edges(",
    "def _reverse_module_dependencies(",
    "def _topo_sort_modules(",
)


def test_cli_module_dependencies_authority_is_single_home() -> None:
    for name in _MODULE_DEPENDENCY_NAMES:
        assert hasattr(module_dependencies, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_DEPENDENCY_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source
