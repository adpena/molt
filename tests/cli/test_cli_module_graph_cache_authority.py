from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_graph_cache

_MODULE_GRAPH_CACHE_NAMES = (
    "_IMPORT_SCAN_CACHE_SCHEMA_VERSION",
    "_import_scan_cache_path",
    "_read_persisted_import_scan_record",
    "_resolved_module_cache_key",
    "_write_persisted_import_scan",
)

_MODULE_GRAPH_CACHE_DEFINITIONS = (
    "_IMPORT_SCAN_CACHE_SCHEMA_VERSION =",
    "def _import_scan_cache_path(",
    "def _read_persisted_import_scan_record(",
    "def _resolved_module_cache_key(",
    "def _write_persisted_import_scan(",
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


def test_persisted_whole_graph_authority_is_removed() -> None:
    for name in (
        "_MODULE_GRAPH_CACHE_SCHEMA_VERSION",
        "_PersistedModuleGraphState",
        "_PersistedImportScan",
        "_module_graph_cache_key",
        "_module_graph_cache_path",
        "_module_graph_policy_digest",
        "_read_persisted_module_graph",
        "_write_persisted_module_graph",
    ):
        for owner in (module_graph_cache, module_graph, cli):
            assert not hasattr(owner, name)
