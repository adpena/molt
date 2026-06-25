from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_graph_cache

_MODULE_GRAPH_CACHE_NAMES = (
    "_IMPORT_SCAN_CACHE_SCHEMA_VERSION",
    "_MODULE_GRAPH_CACHE_SCHEMA_VERSION",
    "_PersistedModuleGraphState",
    "_import_scan_cache_path",
    "_module_graph_cache_key",
    "_module_graph_cache_path",
    "_module_graph_policy_digest",
    "_read_persisted_import_scan",
    "_read_persisted_module_graph",
    "_resolved_module_cache_key",
    "_write_persisted_import_scan",
    "_write_persisted_module_graph",
)

_MODULE_GRAPH_CACHE_DEFINITIONS = (
    "_IMPORT_SCAN_CACHE_SCHEMA_VERSION =",
    "_MODULE_GRAPH_CACHE_SCHEMA_VERSION =",
    "class _PersistedModuleGraphState",
    "def _import_scan_cache_path(",
    "def _module_graph_cache_key(",
    "def _module_graph_cache_path(",
    "def _module_graph_policy_digest(",
    "def _read_persisted_import_scan(",
    "def _read_persisted_module_graph(",
    "def _resolved_module_cache_key(",
    "def _write_persisted_import_scan(",
    "def _write_persisted_module_graph(",
)


def test_cli_module_graph_cache_authority_is_single_home() -> None:
    for name in _MODULE_GRAPH_CACHE_NAMES:
        assert hasattr(module_graph_cache, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_GRAPH_CACHE_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source
