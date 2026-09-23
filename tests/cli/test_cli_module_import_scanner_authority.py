from __future__ import annotations

import ast
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
    "def _collect_import_star_modules(",
    "def _collect_imports(",
    "def _expand_imports_with_static_package_all_star_children(",
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


def test_iterating_a_constant_display_keeps_relative_imports_static() -> None:
    # numpy's package init aliases extended-precision scalars in a loop over a
    # literal list before its later `from . import lib` statements; a builtin
    # container's iteration runs no user code, so the package anchor survives.
    from molt.compiler_analysis.python_imports import UnresolvedStaticImportError

    static = ast.parse(
        "for ta in ['float96', 'float128']:\n"
        "    try:\n"
        "        globals()[ta] = getattr(_core, ta)\n"
        "    except AttributeError:\n"
        "        pass\n"
        "del ta\n"
        "from . import lib\n"
    )
    assert "pkg.lib" in module_import_scanner._collect_imports(
        static, module_name="pkg", is_package=True, import_scan_mode="module_init"
    )

    dynamic = ast.parse(
        "for ta in aliases():\n"
        "    globals()[ta] = getattr(_core, ta)\n"
        "from . import lib\n"
    )
    try:
        module_import_scanner._collect_imports(
            dynamic, module_name="pkg", is_package=True, import_scan_mode="module_init"
        )
    except UnresolvedStaticImportError:
        pass
    else:
        raise AssertionError("iteration over a call must keep runtime custody")


def test_iterating_a_name_bound_to_a_constant_display_stays_static() -> None:
    # numpy._core's package init collects environment keys in a list literal,
    # iterates it in a finally clause, and only then imports its siblings.
    from molt.compiler_analysis.python_imports import UnresolvedStaticImportError

    static = ast.parse(
        "env_added = []\n"
        "for envkey in ['OPENBLAS_MAIN_FREE']:\n"
        "    if envkey not in os.environ:\n"
        "        env_added.append(envkey)\n"
        "try:\n"
        "    from . import multiarray\n"
        "finally:\n"
        "    for envkey in env_added:\n"
        "        os.unsetenv(envkey)\n"
        "del envkey\n"
        "del env_added\n"
        "from . import umath\n"
    )
    assert "pkg.umath" in module_import_scanner._collect_imports(
        static, module_name="pkg", is_package=True, import_scan_mode="module_init"
    )

    rebound = ast.parse(
        "env_added = []\n"
        "env_added = discover()\n"
        "for envkey in env_added:\n"
        "    os.unsetenv(envkey)\n"
        "from . import umath\n"
    )
    try:
        module_import_scanner._collect_imports(
            rebound, module_name="pkg", is_package=True, import_scan_mode="module_init"
        )
    except UnresolvedStaticImportError:
        pass
    else:
        raise AssertionError("a name rebound to a call must keep runtime custody")


def test_a_raising_handler_does_not_taint_the_imports_after_its_try() -> None:
    # numpy._core's import fallback iterates a call result while composing its
    # error, then raises; the package anchor of the statements after the try
    # comes only from paths that complete normally. Iterating the package's
    # own __path__ is builtin iteration while the path is known.
    from molt.compiler_analysis.python_imports import UnresolvedStaticImportError

    raising = ast.parse(
        "env_added = []\n"
        "try:\n"
        "    from . import multiarray\n"
        "except ImportError as exc:\n"
        "    candidates = []\n"
        "    for path in __path__:\n"
        "        candidates.extend(f for f in os.listdir(path))\n"
        "    for f in discover():\n"
        "        candidates.append(f)\n"
        "    raise ImportError(candidates) from exc\n"
        "finally:\n"
        "    for envkey in env_added:\n"
        "        os.unsetenv(envkey)\n"
        "from . import umath\n"
    )
    assert "pkg.umath" in module_import_scanner._collect_imports(
        raising, module_name="pkg", is_package=True, import_scan_mode="module_init"
    )

    falls_through = ast.parse(
        "try:\n"
        "    from . import multiarray\n"
        "except ImportError:\n"
        "    for f in discover():\n"
        "        pass\n"
        "from . import umath\n"
    )
    try:
        module_import_scanner._collect_imports(
            falls_through,
            module_name="pkg",
            is_package=True,
            import_scan_mode="module_init",
        )
    except UnresolvedStaticImportError:
        pass
    else:
        raise AssertionError("a handler that completes normally still taints")
