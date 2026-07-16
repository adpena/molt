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
    imports = _imports(
        "import alpha.beta as ab\n"
        "from . import sibling as renamed\n"
        "from ..parent import child as renamed_child\n"
        "from alpha import *\n",
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


def test_helper_wrapper_keeps_complete_import_request_payload() -> None:
    imports = _imports(
        "from builtins import __import__ as runtime_import\n"
        "def load(name, children, level):\n"
        "    return runtime_import(name, globals(), locals(), children, level)\n"
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

    graph, explicit_imports = module_graph_discovery._discover_module_graph(
        entry,
        [tmp_path.resolve(), stdlib_root],
        [tmp_path.resolve()],
        stdlib_root,
        tmp_path,
        module_stdlib_policy._stdlib_allowlist(),
    )

    assert graph["pkg.child.leaf"] == leaf
    assert {"pkg.child", "pkg.child.leaf"} <= explicit_imports
