from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_graph_discovery

_MODULE_GRAPH_DISCOVERY_NAMES = (
    "PLATFORM_EXCLUDED_SUBMODULES",
    "_discover_module_graph",
    "_discover_module_graph_from_paths",
    "_extend_module_graph_with_closure",
    "_extend_module_graph_with_static_import_modules",
    "_load_module_imports",
    "_module_graph_import_scan_mode",
    "_parse_static_import_modules",
    "_parse_static_import_modules_from_env",
    "_record_module_reason",
    "_record_new_module_reasons",
    "_resolve_static_import_module_paths",
)

_MODULE_GRAPH_DISCOVERY_DEFINITIONS = (
    "PLATFORM_EXCLUDED_SUBMODULES =",
    "def _discover_module_graph(",
    "def _discover_module_graph_from_paths(",
    "def _extend_module_graph_with_closure(",
    "def _extend_module_graph_with_static_import_modules(",
    "def _load_module_imports(",
    "def _module_graph_import_scan_mode(",
    "def _parse_static_import_modules(",
    "def _parse_static_import_modules_from_env(",
    "def _record_module_reason(",
    "def _record_new_module_reasons(",
    "def _resolve_static_import_module_paths(",
)


def test_cli_module_graph_discovery_authority_is_single_home() -> None:
    for name in _MODULE_GRAPH_DISCOVERY_NAMES:
        assert hasattr(module_graph_discovery, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_GRAPH_DISCOVERY_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source
