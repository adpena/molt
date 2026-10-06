from __future__ import annotations

import ast
from pathlib import Path

import pytest

from molt.cli import (
    module_graph_discovery,
    module_import_scanner,
    module_resolution,
    module_stdlib_policy,
)


def _imports(source: str, *, module: str = "pkg.consumer") -> set[str]:
    return set(
        module_import_scanner._collect_imports(
            ast.parse(source),
            module_name=module,
            is_package=False,
        )
    )


def test_import_statements_share_relative_and_fromlist_projection() -> None:
    # Each branch reads the initial namespace; executing one relative import
    # would taint the next statement and require runtime source custody.
    imports = _imports(
        "if flag is None:\n    from . import sibling as renamed\n"
        "else:\n    from ..parent import child as renamed_child\n"
        "import alpha.beta as ab\nfrom alpha import *\n",
        module="pkg.sub.consumer",
    )

    assert {
        "alpha.beta",
        "pkg.sub",
        "pkg.sub.sibling",
        "pkg.parent",
        "pkg.parent.child",
        "alpha",
    } <= imports
    assert "alpha.*" not in imports


@pytest.mark.parametrize(
    "source",
    [
        "__import__('sibling', globals(), locals(), ('leaf',), 1)",
        (
            "import builtins as runtime_builtins\n"
            "runtime_builtins.__import__(\n"
            "    name='sibling', globals=globals(), locals=locals(),\n"
            "    fromlist=('leaf',), level=1)\n"
        ),
        (
            "from builtins import __import__ as load\n"
            "load('sibling', globals(), locals(), ('leaf',), 1)\n"
        ),
    ],
)
def test_dunder_import_aliases_preserve_level_and_fromlist(source: str) -> None:
    imports = _imports(source)

    assert "pkg.sibling" in imports
    assert "pkg.sibling.leaf" in imports
    assert "sibling" not in imports


@pytest.mark.parametrize(
    "source",
    [
        "import importlib\nimportlib.import_module('.child', 'pkg')\n",
        "import importlib as loader\nloader.import_module(name='.child', package='pkg')\n",
        (
            "from importlib import import_module as load\n"
            "load('.child', package='pkg')\n"
        ),
    ],
)
def test_import_module_aliases_share_explicit_package_resolution(source: str) -> None:
    imports = _imports(source)

    assert "pkg.child" in imports
    assert ".child" not in imports


_DEFERRED_GLOBALS_FORMS = (
    ("", "globals()"),
    ("    def anchor(): pass\n", "anchor.__globals__"),
    ("    import inspect\n", "inspect.currentframe().f_globals"),
    ("    from builtins import globals as current_globals\n", "current_globals()"),
    (
        "    def anchor(): pass\n    namespace = anchor.__globals__\n",
        "namespace",
    ),
    ("", "{'__package__': __package__}"),
    ("", "{'__name__': __name__}"),
)


@pytest.mark.parametrize("setup,namespace", _DEFERRED_GLOBALS_FORMS)
def test_helper_wrapper_keeps_complete_import_request_payload(setup, namespace) -> None:
    from molt.compiler_analysis.python_imports import UnresolvedStaticImportError

    source = (
        "from builtins import __import__ as runtime_import\n"
        "def load(name, children, level):\n"
        + setup
        + f"    return runtime_import(name, {namespace}, locals(), children, level)\n"
        "load('sibling', ('leaf',), 1)\n"
    )
    projection = module_import_scanner._collect_imports_for_graph(
        ast.parse(source), "pkg.consumer"
    )
    # Deferred functions can run with replaced activation globals. Preserve the
    # complete payload as discovery edges without asserting an exact namespace.
    assert "pkg.sibling" in projection.dynamic_relative_import_candidates
    assert "pkg.sibling.leaf" in projection.dynamic_relative_import_candidates
    assert "sibling" not in projection.dynamic_relative_import_candidates
    assert "pkg.sibling" not in projection.imports
    assert projection.requires_runtime_package_anchor
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _imports(source)


@pytest.mark.parametrize("setup,namespace", _DEFERRED_GLOBALS_FORMS)
def test_cpython_helper_globals_spellings_resolve_foreign_activation_package(
    setup, namespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    import sys
    from types import FunctionType, ModuleType

    package = ModuleType("foreign_activation")
    package.__path__ = []
    sibling = ModuleType("foreign_activation.sibling")
    sibling.__path__ = []
    leaf = ModuleType("foreign_activation.sibling.leaf")
    package.sibling = sibling
    sibling.leaf = leaf
    for module in (package, sibling, leaf):
        monkeypatch.setitem(sys.modules, module.__name__, module)
    definitions = {}
    exec(
        "from builtins import __import__ as runtime_import\n"
        "def load(name, children, level):\n"
        + setup
        + f"    return runtime_import(name, {namespace}, locals(), children, level)\n",
        definitions,
    )
    foreign = {
        "__package__": "foreign_activation",
        "__name__": "foreign_activation.consumer",
        "runtime_import": __import__,
    }
    rebound = FunctionType(definitions["load"].__code__, foreign)
    assert rebound("sibling", ("leaf",), 1) is sibling
    assert sibling.leaf is leaf


@pytest.mark.parametrize(
    "source",
    [
        "def anchor(): pass\n"
        "__import__('sibling', anchor.__globals__, None, ('leaf',), 1)\n",
        "import inspect\n"
        "__import__('sibling', inspect.currentframe().f_globals, None, ('leaf',), 1)\n",
        "from builtins import globals as current_globals\n"
        "__import__('sibling', current_globals(), None, ('leaf',), 1)\n",
    ],
)
def test_module_globals_spellings_preserve_strict_package_authority(source) -> None:
    imports = _imports(source)
    assert {"pkg.sibling", "pkg.sibling.leaf"} <= imports


def test_deferred_relative_statement_keeps_lexical_dependency_discovery() -> None:
    from molt.compiler_analysis.python_imports import UnresolvedStaticImportError

    source = "def load():\n    from .sibling import leaf\n"
    projection = module_import_scanner._collect_imports_for_graph(
        ast.parse(source), "pkg.consumer"
    )
    assert {"pkg.sibling", "pkg.sibling.leaf"} <= set(
        projection.dynamic_relative_import_candidates
    )
    assert not projection.imports
    assert projection.requires_runtime_package_anchor
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _imports(source)


def test_helper_wrapper_known_foreign_globals_keeps_strict_payload() -> None:
    imports = _imports(
        "from builtins import __import__ as runtime_import\n"
        "def load(name, children, level):\n"
        "    return runtime_import(name, {'__package__': 'pkg'}, locals(), children, level)\n"
        "load('sibling', ('leaf',), 1)\n"
    )
    assert "pkg.sibling" in imports
    assert "pkg.sibling.leaf" in imports
    assert "sibling" not in imports


def test_import_alias_rebinding_disables_static_call_projection() -> None:
    imports = _imports(
        "import builtins as runtime_builtins\n"
        "runtime_builtins = custom_runtime\n"
        "runtime_builtins.__import__('should_not_be_static')\n"
    )

    assert "should_not_be_static" not in imports


@pytest.mark.parametrize(
    "source",
    [
        "import custom_loader as importlib\nimportlib.import_module('nope')\n",
        "from custom_loader import load as __import__\n__import__('nope')\n",
        ("from custom_loader import locate as find_spec\nfind_spec('nope')\n"),
    ],
)
def test_unrelated_import_binding_invalidates_reserved_alias(source: str) -> None:
    assert "nope" not in _imports(source)


@pytest.mark.parametrize(
    "source",
    [
        "from importlib.util import find_spec as locate\nlocate('pkg.child')\n",
        "import importlib.util as util\nutil.find_spec('pkg.child')\n",
        "import importlib\nimportlib.util.find_spec('pkg.child')\n",
    ],
)
def test_find_spec_aliases_share_static_request_projection(source: str) -> None:
    assert "pkg.child" in _imports(source)


@pytest.mark.parametrize(
    "source",
    [
        (
            "import builtins\n"
            "builtins.__import__ = custom_import\n"
            "builtins.__import__('should_not_be_static')\n"
        ),
        (
            "import importlib.util as util\n"
            "util.find_spec = custom_find_spec\n"
            "util.find_spec('should_not_be_static')\n"
        ),
    ],
)
def test_import_callable_mutation_disables_static_projection(source: str) -> None:
    assert "should_not_be_static" not in _imports(source)


def test_module_graph_consumes_relative_fromlist_request_projection(
    tmp_path: Path,
) -> None:
    package = tmp_path / "pkg"
    child = package / "child"
    child.mkdir(parents=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    (child / "__init__.py").write_text("", encoding="utf-8")
    leaf = child / "leaf.py"
    leaf.write_text("VALUE = 1\n", encoding="utf-8")
    entry = package / "consumer.py"
    entry.write_text(
        "__import__('child', globals(), locals(), ('leaf',), 1)\n",
        encoding="utf-8",
    )
    stdlib_root = module_resolution._stdlib_root_path()

    discovery_result = module_graph_discovery._discover_module_graph(
        entry,
        [tmp_path.resolve(), stdlib_root],
        [tmp_path.resolve()],
        stdlib_root,
        tmp_path,
        module_stdlib_policy._stdlib_allowlist(),
    )
    graph = discovery_result.graph
    explicit_imports = discovery_result.explicit_imports

    assert graph["pkg.child.leaf"] == leaf
    assert {"pkg.child", "pkg.child.leaf"} <= explicit_imports


def test_helper_forwarding_keeps_eager_default_import_call_identity():
    source = (
        "def load(value=__import__('child', {'__package__': __package__}, level=1)):\n"
        "    return value\n"
        "__package__ = 'other'\n"
        "load()\n"
    )
    imports = _imports(source)
    assert "pkg.child" in imports
    assert "other.child" not in imports
