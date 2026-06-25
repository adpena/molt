from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_resolution

_MODULE_RESOLUTION_NAMES = (
    "_ModuleResolutionCache",
    "_case_exact_file",
    "_entry_module_root_for_path",
    "_has_namespace_dir",
    "_is_runtime_owned_module_path",
    "_is_stdlib_module",
    "_is_stdlib_resolved_path",
    "_module_name_from_path",
    "_module_name_from_relative_parts",
    "_module_name_from_resolved_path",
    "_relative_parts_if_within",
    "_resolve_module_path",
    "_resolve_module_path_parts",
    "_roots_for_module",
    "_runtime_owned_module_roots",
    "_stdlib_root_path",
)

_MODULE_RESOLUTION_DEFINITIONS = (
    "class _ModuleResolutionCache",
    "def _case_exact_file(",
    "def _entry_module_root_for_path(",
    "def _has_namespace_dir(",
    "def _is_runtime_owned_module_path(",
    "def _is_stdlib_module(",
    "def _is_stdlib_resolved_path(",
    "def _module_name_from_path(",
    "def _module_name_from_relative_parts(",
    "def _module_name_from_resolved_path(",
    "def _relative_parts_if_within(",
    "def _resolve_module_path(",
    "def _resolve_module_path_parts(",
    "def _roots_for_module(",
    "def _runtime_owned_module_roots(",
    "def _stdlib_root_path(",
)


def test_cli_module_resolution_authority_is_single_home() -> None:
    for name in _MODULE_RESOLUTION_NAMES:
        assert hasattr(module_resolution, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_RESOLUTION_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source
