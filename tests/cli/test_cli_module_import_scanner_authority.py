from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_import_scanner

_MODULE_IMPORT_SCANNER_NAMES = (
    "IMPORTER_MODULE_NAME",
    "STDLIB_STATIC_IMPORT_HELPER_MODULES",
    "STDLIB_STATIC_IMPORT_HELPER_QUALNAMES",
    "_IMPORT_SCAN_MODES",
    "_RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES",
    "_RUNTIME_IMPORT_PROTOCOL_MARKERS",
    "_RUNTIME_IMPORT_PROTOCOL_TARGETS",
    "_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES",
    "_collect_import_star_modules",
    "_collect_imports",
    "_expand_imports_with_static_package_all_star_children",
    "_explicit_imports_reference_generated_importer",
    "_infer_module_overrides",
    "_is_modulespec_ctor",
    "_module_graph_needs_runtime_import_support",
    "_module_init_static_helper_scan_nodes",
    "_module_init_scan_nodes",
    "_module_uses_runtime_import_protocol",
    "_parse_modulespec_override",
    "_qualified_child",
    "_resolve_relative_import",
    "_resolve_runtime_import_expr_name",
    "_runtime_import_alias_bindings",
    "_source_may_use_runtime_import_protocol",
    "_spec_parent",
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
    "def _collect_import_star_modules(",
    "def _collect_imports(",
    "def _expand_imports_with_static_package_all_star_children(",
    "def _explicit_imports_reference_generated_importer(",
    "def _infer_module_overrides(",
    "def _is_modulespec_ctor(",
    "def _module_graph_needs_runtime_import_support(",
    "def _module_init_static_helper_scan_nodes(",
    "def _module_init_scan_nodes(",
    "def _module_uses_runtime_import_protocol(",
    "def _parse_modulespec_override(",
    "def _qualified_child(",
    "def _resolve_relative_import(",
    "def _resolve_runtime_import_expr_name(",
    "def _runtime_import_alias_bindings(",
    "def _source_may_use_runtime_import_protocol(",
    "def _spec_parent(",
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
