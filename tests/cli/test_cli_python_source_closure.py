from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import hashlib
import json
import os

import pytest

from molt.cli.python_source_closure import local_python_import_closure


def test_local_python_import_closure_follows_tools_and_src_packages(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "demo"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    (tools / "entry.py").write_text(
        "import helper\nfrom demo import leaf\n",
        encoding="utf-8",
    )
    (tools / "helper.py").write_text(
        "from demo.shared import value\n", encoding="utf-8"
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "leaf.py").write_text("from .shared import value\n", encoding="utf-8")
    (package / "shared.py").write_text("value = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",))

    assert {path.relative_to(tmp_path).as_posix() for path in closure} == {
        "src/demo/__init__.py",
        "src/demo/leaf.py",
        "src/demo/shared.py",
        "tools/entry.py",
        "tools/helper.py",
    }


def test_local_python_import_closure_fails_closed_on_malformed_seed(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("from broken import (\n", encoding="utf-8")

    with pytest.raises(ValueError, match="cannot derive Python tooling import closure"):
        local_python_import_closure(tmp_path, (seed,))


def _relative_paths(root: Path, paths: tuple[Path, ...]) -> set[str]:
    return {path.relative_to(root).as_posix() for path in paths}


def test_importing_submodule_executes_and_traverses_parent_initializers(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    nested = package / "nested"
    tools.mkdir(parents=True)
    nested.mkdir(parents=True)
    (tools / "entry.py").write_text("import pkg.nested.leaf\n", encoding="utf-8")
    (package / "__init__.py").write_text("from . import parent_dep\n", encoding="utf-8")
    (package / "parent_dep.py").write_text("VALUE = 1\n", encoding="utf-8")
    (nested / "__init__.py").write_text("from . import nested_dep\n", encoding="utf-8")
    (nested / "nested_dep.py").write_text("VALUE = 2\n", encoding="utf-8")
    (nested / "leaf.py").write_text("VALUE = 3\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",))

    assert _relative_paths(tmp_path, closure) == {
        "src/pkg/__init__.py",
        "src/pkg/nested/__init__.py",
        "src/pkg/nested/leaf.py",
        "src/pkg/nested/nested_dep.py",
        "src/pkg/parent_dep.py",
        "tools/entry.py",
    }


def test_nested_relative_star_and_alias_imports_resolve_to_local_sources(
    tmp_path: Path,
) -> None:
    package = tmp_path / "src" / "pkg"
    nested = package / "nested"
    nested.mkdir(parents=True)
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "shared.py").write_text("VALUE = 1\n", encoding="utf-8")
    (nested / "__init__.py").write_text("\n", encoding="utf-8")
    (nested / "leaf.py").write_text(
        "from ..shared import *\nfrom pkg import shared as renamed\n",
        encoding="utf-8",
    )

    closure = local_python_import_closure(tmp_path, (nested / "leaf.py",))

    assert _relative_paths(tmp_path, closure) == {
        "src/pkg/__init__.py",
        "src/pkg/nested/leaf.py",
        "src/pkg/shared.py",
    }


def test_namespace_package_parents_do_not_require_initializer(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    namespace = tmp_path / "src" / "namespace" / "nested"
    tools.mkdir(parents=True)
    namespace.mkdir(parents=True)
    (tools / "entry.py").write_text("import namespace.nested.leaf\n", encoding="utf-8")
    (namespace / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",))

    assert _relative_paths(tmp_path, closure) == {
        "src/namespace/nested/leaf.py",
        "tools/entry.py",
    }


def test_pep263_encoded_source_is_parsed_with_declared_encoding(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "entry.py").write_bytes(
        "# -*- coding: latin-1 -*-\nLABEL = 'café'\nimport helper\n".encode("latin-1")
    )
    (tools / "helper.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",))

    assert _relative_paths(tmp_path, closure) == {
        "tools/entry.py",
        "tools/helper.py",
    }


def test_parent_initializer_content_is_a_link_fingerprint_input(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    initializer = package / "__init__.py"
    seed.write_text("import pkg.leaf\n", encoding="utf-8")
    initializer.write_text("VALUE = 1\n", encoding="utf-8")
    (package / "leaf.py").write_text("VALUE = 2\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))
    before = hashlib.sha256(
        b"\0".join(path.read_bytes() for path in closure)
    ).hexdigest()
    initializer.write_text("VALUE = 9\n", encoding="utf-8")
    after = hashlib.sha256(
        b"\0".join(path.read_bytes() for path in closure)
    ).hexdigest()

    assert initializer.resolve() in closure
    assert before != after


def test_literal_dynamic_import_forms_and_aliases_join_closure(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    (tools / "entry.py").write_text(
        "import importlib as il\n"
        "from importlib import import_module as load\n"
        "il.import_module('pkg.first')\n"
        "load('pkg.second')\n"
        "__import__('pkg.third')\n",
        encoding="utf-8",
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    for name in ("first", "second", "third"):
        (package / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",))

    assert _relative_paths(tmp_path, closure) == {
        "src/pkg/__init__.py",
        "src/pkg/first.py",
        "src/pkg/second.py",
        "src/pkg/third.py",
        "tools/entry.py",
    }


@pytest.mark.parametrize(
    "source",
    [
        "import importlib\nname = 'pkg.leaf'\nimportlib.import_module(name)\n",
        "name = 'pkg.leaf'\n__import__(name)\n",
    ],
)
def test_nonliteral_dynamic_imports_fail_closed(tmp_path: Path, source: str) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(source, encoding="utf-8")
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")

    with pytest.raises(ValueError, match="non-literal dynamic Python import"):
        local_python_import_closure(tmp_path, (seed,))


def test_explicit_dynamic_import_manifest_edges_join_closure(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    cli = tmp_path / "src" / "molt" / "cli"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    cli.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "import importlib\nname = 'pkg.leaf'\nimportlib.import_module(name)\n",
        encoding="utf-8",
    )
    manifest = cli / "python_source_closure.toml"
    manifest.write_text(
        "schema_version = 1\n"
        "[[source]]\n"
        "path = 'tools/entry.py'\n"
        "nonliteral_calls = 1\n"
        "modules = ['pkg.leaf']\n",
        encoding="utf-8",
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert _relative_paths(tmp_path, closure) == {
        "src/molt/cli/python_source_closure.toml",
        "src/pkg/__init__.py",
        "src/pkg/leaf.py",
        "tools/entry.py",
    }


def test_dynamic_import_manifest_call_count_drift_fails_closed(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    cli = tmp_path / "src" / "molt" / "cli"
    tools.mkdir(parents=True)
    cli.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "import importlib\nname = 'external'\nimportlib.import_module(name)\n",
        encoding="utf-8",
    )
    (cli / "python_source_closure.toml").write_text(
        "schema_version = 1\n"
        "[[source]]\n"
        "path = 'tools/entry.py'\n"
        "nonliteral_calls = 2\n",
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="dynamic Python import manifest drift"):
        local_python_import_closure(tmp_path, (seed,))


def test_persistent_graph_reparses_changed_source(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import first\n", encoding="utf-8")
    (tools / "first.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tools / "second.py").write_text("VALUE = 2\n", encoding="utf-8")
    local_python_import_closure(tmp_path, (seed,))

    seed.write_text("import second\n", encoding="utf-8")
    closure = local_python_import_closure(tmp_path, (seed,))

    assert _relative_paths(tmp_path, closure) == {
        "tools/entry.py",
        "tools/second.py",
    }


def test_persistent_graph_reresolves_new_submodule_without_importer_edit(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("from pkg import feature\n", encoding="utf-8")
    (package / "__init__.py").write_text("feature = 1\n", encoding="utf-8")
    first = local_python_import_closure(tmp_path, (seed,))
    assert "src/pkg/feature.py" not in _relative_paths(tmp_path, first)

    (package / "feature.py").write_text("VALUE = 2\n", encoding="utf-8")
    second = local_python_import_closure(tmp_path, (seed,))

    assert "src/pkg/feature.py" in _relative_paths(tmp_path, second)


def test_persistent_graph_detects_namespace_parent_becoming_regular_package(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "namespace"
    nested = package / "nested"
    tools.mkdir(parents=True)
    nested.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("import namespace.nested.leaf\n", encoding="utf-8")
    (nested / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")
    first = local_python_import_closure(tmp_path, (seed,))
    assert "src/namespace/__init__.py" not in _relative_paths(tmp_path, first)

    (package / "__init__.py").write_text("from . import parent_dep\n", encoding="utf-8")
    (package / "parent_dep.py").write_text("VALUE = 2\n", encoding="utf-8")
    second = local_python_import_closure(tmp_path, (seed,))

    assert _relative_paths(tmp_path, second) >= {
        "src/namespace/__init__.py",
        "src/namespace/parent_dep.py",
    }


@pytest.mark.parametrize(
    "source",
    [
        "def load(importlib):\n    return importlib.import_module('pkg.bad')\n",
        "import importlib as loader\nloader = object()\nloader.import_module('pkg.bad')\n",
        "import importlib\ndef load():\n    importlib.import_module('pkg.bad')\n    importlib = object()\n",
        "def first():\n    import importlib as loader\ndef second():\n    loader.import_module('pkg.bad')\n",
        "def load(__import__):\n    return __import__('pkg.bad')\n",
    ],
)
def test_dynamic_import_alias_shadowing_does_not_invent_edges(
    tmp_path: Path,
    source: str,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(source, encoding="utf-8")
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "bad.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert "src/pkg/bad.py" not in _relative_paths(tmp_path, closure)


def test_function_local_dynamic_alias_and_keyword_arguments_join_closure(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "def load():\n"
        "    from importlib import import_module as resolve\n"
        "    return resolve(name='.leaf', package='pkg')\n",
        encoding="utf-8",
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert "src/pkg/leaf.py" in _relative_paths(tmp_path, closure)


def test_dunder_import_keyword_fromlist_joins_submodule_closure(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "__import__(name='pkg', fromlist=('leaf',), level=0)\n",
        encoding="utf-8",
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert "src/pkg/leaf.py" in _relative_paths(tmp_path, closure)


def test_dunder_import_relative_level_uses_enclosing_package(tmp_path: Path) -> None:
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    seed = package / "entry.py"
    seed.write_text(
        "__import__(name='leaf', globals=globals(), locals=locals(), fromlist=(), level=1)\n",
        encoding="utf-8",
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert "src/pkg/leaf.py" in _relative_paths(tmp_path, closure)


def test_relative_dynamic_import_requires_explicit_package(tmp_path: Path) -> None:
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    seed = package / "entry.py"
    seed.write_text(
        "from importlib import import_module\nimport_module('.leaf')\n",
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="requires a package"):
        local_python_import_closure(tmp_path, (seed,))


def test_relative_statement_without_parent_package_fails_closed(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("from . import leaf\n", encoding="utf-8")

    with pytest.raises(ValueError, match="no known parent package"):
        local_python_import_closure(tmp_path, (seed,))


def test_same_root_package_precedes_same_named_module(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "dual"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("import dual\n", encoding="utf-8")
    (tmp_path / "src" / "dual.py").write_text("MODULE = True\n", encoding="utf-8")
    (package / "__init__.py").write_text("PACKAGE = True\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    paths = _relative_paths(tmp_path, closure)
    assert "src/dual/__init__.py" in paths
    assert "src/dual.py" not in paths


def test_explicit_search_root_order_matches_sys_path_precedence(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    src = tmp_path / "src"
    tools.mkdir()
    src.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import shared\n", encoding="utf-8")
    (tools / "shared.py").write_text("OWNER = 'tools'\n", encoding="utf-8")
    (src / "shared.py").write_text("OWNER = 'src'\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    paths = _relative_paths(tmp_path, closure)
    assert "tools/shared.py" in paths
    assert "src/shared.py" not in paths


def test_earlier_module_shadows_later_package_for_child_import(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    later_package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    later_package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("import pkg.child\n", encoding="utf-8")
    (tools / "pkg.py").write_text("OWNER = 'tools module'\n", encoding="utf-8")
    (later_package / "__init__.py").write_text(
        "OWNER = 'later package'\n", encoding="utf-8"
    )
    (later_package / "child.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert _relative_paths(tmp_path, closure) == {"tools/entry.py", "tools/pkg.py"}


def test_earlier_regular_package_blocks_later_namespace_portion(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    early_package = tools / "pkg"
    later_namespace = tmp_path / "src" / "pkg"
    early_package.mkdir(parents=True)
    later_namespace.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("import pkg.later\n", encoding="utf-8")
    (early_package / "__init__.py").write_text(
        "OWNER = 'early package'\n", encoding="utf-8"
    )
    (later_namespace / "later.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert _relative_paths(tmp_path, closure) == {
        "tools/entry.py",
        "tools/pkg/__init__.py",
    }


def test_later_regular_package_supersedes_earlier_namespace_portions(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    early_namespace = tools / "pkg"
    later_package = tmp_path / "src" / "pkg"
    early_namespace.mkdir(parents=True)
    later_package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("import pkg.child\n", encoding="utf-8")
    (early_namespace / "child.py").write_text(
        "OWNER = 'namespace portion'\n", encoding="utf-8"
    )
    (later_package / "__init__.py").write_text(
        "OWNER = 'regular package'\n", encoding="utf-8"
    )
    (later_package / "child.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert _relative_paths(tmp_path, closure) == {
        "src/pkg/__init__.py",
        "src/pkg/child.py",
        "tools/entry.py",
    }


def test_subpackage_precedes_same_location_submodule(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    subpackage = package / "item"
    tools.mkdir(parents=True)
    subpackage.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("import pkg.item\n", encoding="utf-8")
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "item.py").write_text("OWNER = 'module'\n", encoding="utf-8")
    (subpackage / "__init__.py").write_text("OWNER = 'subpackage'\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))
    paths = _relative_paths(tmp_path, closure)

    assert "src/pkg/item/__init__.py" in paths
    assert "src/pkg/item.py" not in paths


def test_concurrent_graph_cache_publication_is_atomic(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import helper\n", encoding="utf-8")
    (tools / "helper.py").write_text("VALUE = 1\n", encoding="utf-8")

    with ThreadPoolExecutor(max_workers=8) as pool:
        closures = list(
            pool.map(
                lambda _index: local_python_import_closure(tmp_path, (seed,)), range(32)
            )
        )

    assert len({closure for closure in closures}) == 1
    cache = json.loads(
        (tmp_path / ".molt_cache" / "python_source_closure_graph.json").read_text(
            encoding="utf-8"
        )
    )
    assert cache["schema_version"] == 3
    assert set(cache["entries"]) == {"tools/entry.py", "tools/helper.py"}


def test_non_mapping_graph_cache_is_ignored_and_replaced(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    cache_path = tmp_path / ".molt_cache" / "python_source_closure_graph.json"
    tools.mkdir()
    cache_path.parent.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import helper\n", encoding="utf-8")
    (tools / "helper.py").write_text("VALUE = 1\n", encoding="utf-8")
    cache_path.write_text("[]\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert _relative_paths(tmp_path, closure) == {
        "tools/entry.py",
        "tools/helper.py",
    }
    assert json.loads(cache_path.read_text(encoding="utf-8"))["schema_version"] == 3


def test_static_import_syntax_family_closes_over_all_named_modules(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "import pkg.alpha as alpha_alias, pkg.beta\n"
        "from pkg import (gamma as gamma_alias, delta)\n"
        "from pkg import *\n",
        encoding="utf-8",
    )
    (package / "__init__.py").write_text("PACKAGE = True\n", encoding="utf-8")
    for name in ("alpha", "beta", "gamma", "delta"):
        (package / f"{name}.py").write_text(f"NAME = {name!r}\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))
    paths = _relative_paths(tmp_path, closure)

    assert {
        f"src/pkg/{name}.py" for name in ("alpha", "beta", "gamma", "delta")
    } <= paths
    assert "src/pkg/__init__.py" in paths


def test_all_valid_relative_levels_resolve_and_beyond_top_fails(tmp_path: Path) -> None:
    package = tmp_path / "src" / "pkg"
    nested = package / "deep" / "more"
    nested.mkdir(parents=True)
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "root_leaf.py").write_text("VALUE = 1\n", encoding="utf-8")
    (package / "deep" / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "deep" / "middle_leaf.py").write_text("VALUE = 2\n", encoding="utf-8")
    (nested / "__init__.py").write_text("\n", encoding="utf-8")
    (nested / "local_leaf.py").write_text("VALUE = 3\n", encoding="utf-8")
    seed = nested / "entry.py"
    seed.write_text(
        "from . import local_leaf\n"
        "from .. import middle_leaf\n"
        "from ... import root_leaf\n",
        encoding="utf-8",
    )

    closure = local_python_import_closure(tmp_path, (seed,))
    paths = _relative_paths(tmp_path, closure)
    assert {
        "src/pkg/root_leaf.py",
        "src/pkg/deep/middle_leaf.py",
        "src/pkg/deep/more/local_leaf.py",
    } <= paths

    seed.write_text("from .... import escaped\n", encoding="utf-8")
    with pytest.raises(ValueError, match="escapes local package"):
        local_python_import_closure(tmp_path, (seed,))


def test_namespace_package_portions_span_ordered_search_roots(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    src = tmp_path / "src"
    (tools / "namespace").mkdir(parents=True)
    (src / "namespace").mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "import namespace.tool_leaf, namespace.src_leaf\n", encoding="utf-8"
    )
    (tools / "namespace" / "tool_leaf.py").write_text(
        "OWNER = 'tools'\n", encoding="utf-8"
    )
    (src / "namespace" / "src_leaf.py").write_text("OWNER = 'src'\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))

    assert _relative_paths(tmp_path, closure) == {
        "tools/entry.py",
        "tools/namespace/tool_leaf.py",
        "src/namespace/src_leaf.py",
    }


def test_non_source_module_shapes_are_not_claimed_as_python_source(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    cache = package / "__pycache__"
    tools.mkdir(parents=True)
    cache.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "import pkg.real, pkg.stub, pkg.native, pkg.cached\n", encoding="utf-8"
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    (package / "real.py").write_text("VALUE = 1\n", encoding="utf-8")
    (package / "stub.pyi").write_text("VALUE: int\n", encoding="utf-8")
    (package / "native.pyd").write_bytes(b"not-a-real-extension")
    (cache / "cached.py").write_text("VALUE = 2\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))
    paths = _relative_paths(tmp_path, closure)

    assert "src/pkg/real.py" in paths
    assert not any(path.endswith((".pyi", ".pyd", ".pyc")) for path in paths)
    assert not any("__pycache__" in path for path in paths)


def test_conditional_try_function_class_and_cycle_imports_are_closed(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "if FLAG:\n    import pkg.conditional\n"
        "try:\n    import pkg.primary\nexcept ImportError:\n    import pkg.fallback\n"
        "def load():\n    import pkg.function\n"
        "class Loader:\n    import pkg.class_body\n",
        encoding="utf-8",
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    for name in ("conditional", "primary", "fallback", "function", "class_body"):
        (package / f"{name}.py").write_text("import pkg.cycle\n", encoding="utf-8")
    (package / "cycle.py").write_text("import pkg.primary\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))
    paths = _relative_paths(tmp_path, closure)

    assert {
        f"src/pkg/{name}.py"
        for name in (
            "conditional",
            "primary",
            "fallback",
            "function",
            "class_body",
            "cycle",
        )
    } <= paths


def test_seed_symlink_cannot_escape_project_authority(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    outside = tmp_path / "outside.py"
    tools.mkdir()
    outside.write_text("VALUE = 1\n", encoding="utf-8")
    seed = tools / "entry.py"
    try:
        seed.symlink_to(outside)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")

    with pytest.raises(ValueError, match="outside local search roots"):
        local_python_import_closure(tmp_path, (seed,))


def test_module_name_case_resolution_follows_host_filesystem(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    src = tmp_path / "src"
    tools.mkdir()
    src.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import mixedcase\n", encoding="utf-8")
    (src / "MixedCase.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,))
    resolved = "src/MixedCase.py" in _relative_paths(tmp_path, closure)

    assert resolved is (
        os.path.normcase("MixedCase.py") == os.path.normcase("mixedcase.py")
    )


def test_dynamic_module_name_with_path_separator_fails_closed(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("__import__('../outside')\n", encoding="utf-8")

    with pytest.raises(ValueError, match="invalid local Python module name"):
        local_python_import_closure(tmp_path, (seed,))


@pytest.mark.parametrize(
    ("source", "message"),
    [
        ("__import__('pkg', level=-1)\n", "negative __import__ level"),
        ("__import__('pkg', level=1.0)\n", "non-literal __import__ level"),
    ],
)
def test_invalid_dunder_import_levels_fail_closed(
    tmp_path: Path,
    source: str,
    message: str,
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text(source, encoding="utf-8")

    with pytest.raises(ValueError, match=message):
        local_python_import_closure(tmp_path, (seed,))
