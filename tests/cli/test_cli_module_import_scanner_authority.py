from __future__ import annotations

import ast
import inspect
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_import_scanner
from molt.cli.models import _StaticSourceExecutionRequest
from molt.compiler_analysis import python_binding_flow, python_source_keys
from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
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
    "_collect_import_scan_requests",
    "_collect_static_source_execution_requests",
    "_expand_static_package_all_star_children",
    "_complete_import_scan",
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
    "def _collect_import_scan_requests(",
    "def _collect_static_source_execution_requests(",
    "def _expand_static_package_all_star_children(",
    "def _complete_import_scan(",
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
    digest = python_source_keys.python_ast_digest
    calls = 0

    def record_digest(candidate: ast.AST) -> str:
        nonlocal calls
        calls += 1
        return digest(candidate)

    monkeypatch.setattr(python_source_keys, "python_ast_digest", record_digest)
    assert "pkg" in module_import_scanner._collect_imports(tree)
    assert module_import_scanner._collect_import_star_modules(tree) == ("pkg",)
    assert module_import_scanner._collect_static_source_execution_requests(
        tree, source_path=tmp_path / "entry.py"
    ) == (_StaticSourceExecutionRequest("loaded", "loaded.py"),)
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


@pytest.mark.parametrize(
    "iterable", ["['float96', 'float128']", "('float96',)", "'ab'"]
)
def test_iterating_a_constant_display_keeps_relative_imports_static(
    iterable: str,
) -> None:
    tree = ast.parse(f"for ta in {iterable}:\n    pass\nfrom . import lib\n")
    assert "pkg.lib" in module_import_scanner._collect_imports(
        tree, module_name="pkg", is_package=True, import_scan_mode="module_init"
    )


def test_constant_iteration_does_not_erase_body_callback_custody() -> None:
    # Retain the incoming NumPy-shaped witness, but not its old unsound static
    # expectation: getattr can execute a descriptor, and replacement/release
    # can call Python even though advancing the literal list cannot.

    static = ast.parse(
        "for ta in ['float96', 'float128']:\n"
        "    try:\n"
        "        globals()[ta] = getattr(_core, ta)\n"
        "    except AttributeError:\n"
        "        pass\n"
        "del ta\n"
        "from . import lib\n"
    )
    with pytest.raises(UnresolvedStaticImportError):
        module_import_scanner._collect_imports(
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
    # Builtin kind survives a tracked builtin mutation, but not a callback that
    # can replace the module binding. No import-specific provenance set exists.
    tree = ast.parse(
        "env_added = []\n"
        "env_added.append('OPENBLAS_MAIN_FREE')\n"
        "for envkey in env_added:\n"
        "    pass\n"
        "from . import umath\n"
    )
    assert "pkg.umath" in module_import_scanner._collect_imports(
        tree, module_name="pkg", is_package=True, import_scan_mode="module_init"
    )


def test_static_and_context_scans_share_one_binding_fixpoint(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    flow = python_binding_flow
    monkeypatch.setattr(flow, "_CORE_CACHE", flow._BindingCache())
    tree = ast.parse("if True:\n    from . import child\n")
    module_import_scanner._static_scan_nodes(tree, include_function_bodies=False)
    core = next(iter(flow._CORE_CACHE._ready.values()))
    assert not core.projections._ready
    for name in ("first", "second"):
        assert f"{name}.child" in module_import_scanner._collect_imports(
            tree, module_name=name, is_package=True, import_scan_mode="module_init"
        )
    assert flow.python_binding_core_computations() == 1
    assert len(core.projections._ready) == 2


def test_builtin_binding_does_not_survive_unknown_callbacks() -> None:
    # The incoming NumPy-shaped source crosses descriptor/comparison/call
    # boundaries. For example os.environ.__contains__ can replace env_added
    # with a custom iterator that changes __package__; list presence is not
    # custody of the later loaded binding.

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
    with pytest.raises(UnresolvedStaticImportError):
        module_import_scanner._collect_imports(
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


def test_callback_bearing_finally_retains_runtime_import_custody() -> None:
    # Keep the original incoming package-init shape. The same finally executes
    # on raised and normal paths; env_added is a live module binding across the
    # callback-bearing handler, not a permanently builtin import-scanner fact.

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
    with pytest.raises(UnresolvedStaticImportError):
        module_import_scanner._collect_imports(
            raising, module_name="pkg", is_package=True, import_scan_mode="module_init"
        )


def test_a_raising_handler_does_not_taint_the_imports_after_its_try() -> None:
    raising = ast.parse(
        "try:\n"
        "    from . import multiarray\n"
        "except ImportError:\n"
        "    for f in discover():\n"
        "        pass\n"
        "    raise\n"
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


def test_rebound_package_path_is_not_builtin_iteration_provenance() -> None:
    tree = ast.parse(
        "__path__ = discover()\nfor path in __path__:\n    pass\nfrom . import child\n"
    )
    with pytest.raises(UnresolvedStaticImportError):
        module_import_scanner._collect_imports(
            tree, module_name="pkg", is_package=True, import_scan_mode="module_init"
        )


def test_callback_can_replace_a_previously_builtin_iterable() -> None:
    tree = ast.parse(
        "class Redirect:\n"
        "    def __iter__(self):\n"
        "        globals()['__package__'] = 'other'\n"
        "        return iter(())\n"
        "def replace():\n"
        "    global env_added\n"
        "    env_added = Redirect()\n"
        "env_added = []\n"
        "replace()\n"
        "for envkey in env_added:\n"
        "    pass\n"
        "from . import child\n"
    )
    with pytest.raises(UnresolvedStaticImportError):
        module_import_scanner._collect_imports(
            tree, module_name="pkg", is_package=True, import_scan_mode="module_init"
        )
