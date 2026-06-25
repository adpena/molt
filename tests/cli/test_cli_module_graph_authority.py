from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import wrapper_build

_MODULE_GRAPH_CORE_NAMES = (
    "_ModuleSourceCatalog",
    "_ModuleResolutionCache",
    "ModuleSyntaxErrorInfo",
    "_build_module_graph_metadata",
    "_build_module_source_catalog",
    "_collect_package_parents",
    "_source_content_sha256",
    "_source_content_sha256_cached",
    "_resolve_module_path",
    "_resolve_module_path_parts",
    "_collect_imports",
    "_discover_module_graph",
    "_discover_module_graph_from_paths",
    "_extend_module_graph_with_static_import_modules",
    "_load_module_imports",
    "_materialize_import_plan",
    "_module_graph_cache_path",
    "_module_graph_needs_runtime_import_support",
    "_parse_static_import_modules",
    "_prepare_entry_module_graph",
    "_read_persisted_import_scan",
    "_read_persisted_module_graph",
    "_tree_uses_runtime_import_protocol",
    "_read_module_source",
    "_stdlib_root_path",
    "_write_persisted_import_scan",
    "_write_persisted_module_graph",
)

_MODULE_GRAPH_CORE_DEFINITIONS = (
    "class _ModuleSourceCatalog",
    "class _ModuleResolutionCache",
    "class ModuleSyntaxErrorInfo",
    "def _build_module_graph_metadata(",
    "def _build_module_source_catalog(",
    "def _collect_package_parents(",
    "def _source_content_sha256(",
    "def _source_content_sha256_cached(",
    "def _resolve_module_path(",
    "def _resolve_module_path_parts(",
    "def _collect_imports(",
    "def _discover_module_graph(",
    "def _discover_module_graph_from_paths(",
    "def _extend_module_graph_with_static_import_modules(",
    "def _load_module_imports(",
    "def _materialize_import_plan(",
    "def _module_graph_cache_path(",
    "def _module_graph_needs_runtime_import_support(",
    "def _parse_static_import_modules(",
    "def _prepare_entry_module_graph(",
    "def _read_persisted_import_scan(",
    "def _read_persisted_module_graph(",
    "def _tree_uses_runtime_import_protocol(",
    "def _read_module_source(",
    "def _stdlib_root_path(",
    "def _write_persisted_import_scan(",
    "def _write_persisted_module_graph(",
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
