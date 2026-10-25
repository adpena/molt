from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import hashlib
import json
import os

import pytest

from molt.cli.python_source_closure import local_python_import_closure
from molt.cli import python_source_closure as graph
from molt.cli.python_import_resolution import (
    PythonImportPolicy,
)
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from tests.process_guard_common import install_module_view
from tests.module_isolation import forget_module


pytestmark = pytest.mark.usefixtures("isolated_molt_cache")


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

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",)).paths

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
        local_python_import_closure(tmp_path, (seed,)).paths


def test_tooling_import_closure_handles_deep_dynamic_import_dispatch(
    tmp_path: Path,
) -> None:
    seed = tmp_path / "dispatch.py"
    source = (
        "import importlib\ndef dispatch(value):\n"
        + "".join(
            f"    {'if' if value == 0 else 'elif'} value == {value}:\n"
            "        return None\n"
            for value in range(384)
        )
        + "    else:\n        return importlib.import_module('payload')\n"
    )
    seed.write_text(source, encoding="utf-8")
    payload = tmp_path / "payload.py"
    payload.write_text("VALUE = 1\n", encoding="utf-8")
    assert local_python_import_closure(tmp_path, (seed,)).paths == (seed, payload)


@pytest.mark.slow
def test_repository_wasm_linker_closure_reaches_binding_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = Path(__file__).resolve().parents[2]
    seed = root / "tools" / "wasm_link.py"
    # Exercise the real cold consumer, not a prior successful graph projection.
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    closure = local_python_import_closure(root, (seed,)).paths
    assert seed in closure
    assert root / "src/molt/compiler_analysis/python_binding_flow.py" in closure
    assert root / "src/molt/compiler_analysis/python_lexical_scope.py" in closure
    assert root / "tools/wasm_link_optimizer_policy.py" in closure


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

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",)).paths

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

    closure = local_python_import_closure(tmp_path, (nested / "leaf.py",)).paths

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

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",)).paths

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

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",)).paths

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
    before = closure.content_digest
    initializer.write_text("VALUE = 9\n", encoding="utf-8")
    changed = local_python_import_closure(tmp_path, (seed,))

    assert initializer.resolve() in closure.paths
    assert changed.paths == closure.paths
    assert before == closure.content_digest
    assert before != changed.content_digest


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

    closure = local_python_import_closure(tmp_path, (tools / "entry.py",)).paths

    assert _relative_paths(tmp_path, closure) == {
        "src/pkg/__init__.py",
        "src/pkg/first.py",
        "src/pkg/second.py",
        "src/pkg/third.py",
        "tools/entry.py",
    }


def test_assigned_and_deferred_dynamic_aliases_join_persistent_closure(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "pkg"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text(
        "import importlib\n"
        "assigned = importlib.import_module\n"
        "assigned('pkg.assigned')\n"
        "def module_late():\n"
        "    load('pkg.module_late')\n"
        "def outer():\n"
        "    def enclosing_late():\n"
        "        nested('pkg.enclosing_late')\n"
        "    from importlib import import_module as nested\n"
        "from importlib import import_module as load\n",
        encoding="utf-8",
    )
    (package / "__init__.py").write_text("\n", encoding="utf-8")
    for name in ("assigned", "module_late", "enclosing_late"):
        (package / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,)).paths

    paths = _relative_paths(tmp_path, closure)
    assert {
        "src/pkg/assigned.py",
        "src/pkg/module_late.py",
        "src/pkg/enclosing_late.py",
    } <= paths


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
        local_python_import_closure(tmp_path, (seed,)).paths


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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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
        local_python_import_closure(tmp_path, (seed,)).paths


def test_persistent_graph_reparses_changed_source(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import first\n", encoding="utf-8")
    (tools / "first.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tools / "second.py").write_text("VALUE = 2\n", encoding="utf-8")
    local_python_import_closure(tmp_path, (seed,)).paths

    seed.write_text("import second\n", encoding="utf-8")
    closure = local_python_import_closure(tmp_path, (seed,)).paths

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
    first = local_python_import_closure(tmp_path, (seed,)).paths
    assert "src/pkg/feature.py" not in _relative_paths(tmp_path, first)

    (package / "feature.py").write_text("VALUE = 2\n", encoding="utf-8")
    second = local_python_import_closure(tmp_path, (seed,)).paths

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
    first = local_python_import_closure(tmp_path, (seed,)).paths
    assert "src/namespace/__init__.py" not in _relative_paths(tmp_path, first)

    (package / "__init__.py").write_text("from . import parent_dep\n", encoding="utf-8")
    (package / "parent_dep.py").write_text("VALUE = 2\n", encoding="utf-8")
    second = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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
        local_python_import_closure(tmp_path, (seed,)).paths


def test_relative_statement_without_parent_package_fails_closed(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("from . import leaf\n", encoding="utf-8")

    with pytest.raises(ValueError, match="no known parent package"):
        local_python_import_closure(tmp_path, (seed,)).paths


def test_same_root_package_precedes_same_named_module(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    package = tmp_path / "src" / "dual"
    tools.mkdir(parents=True)
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("import dual\n", encoding="utf-8")
    (tmp_path / "src" / "dual.py").write_text("MODULE = True\n", encoding="utf-8")
    (package / "__init__.py").write_text("PACKAGE = True\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths
    paths = _relative_paths(tmp_path, closure)

    assert "src/pkg/item/__init__.py" in paths
    assert "src/pkg/item.py" not in paths


def test_graph_cache_preserves_source_trees_and_separates_projects(
    tmp_path: Path, isolated_molt_cache: Path
) -> None:
    roots = [tmp_path / name for name in ("installed source", "other project")]
    before = {}
    for root, dependency in zip(roots, ("first", "second"), strict=True):
        root.mkdir()
        (root / "entry.py").write_text(f"import {dependency}\n", encoding="utf-8")
        (root / f"{dependency}.py").write_text("VALUE = 1\n", encoding="utf-8")
        before[root] = {path.name: path.read_bytes() for path in root.iterdir()}

    for root, dependency in zip(roots * 2, ("first", "second") * 2, strict=True):
        closure = local_python_import_closure(root, (root / "entry.py",))
        assert {path.name for path in closure.paths} == {"entry.py", f"{dependency}.py"}
        # The real persistent graph writer must not add even a directory to
        # immutable inputs, on either the cold or warm path.
        assert {path.name: path.read_bytes() for path in root.iterdir()} == before[root]

    caches = [graph.python_source_closure_cache_path(root) for root in roots]
    assert caches[0] != caches[1]
    for cache, dependency in zip(caches, ("first", "second"), strict=True):
        assert cache.is_relative_to(isolated_molt_cache)
        payload = json.loads(cache.read_text(encoding="utf-8"))
        assert set(payload["entries"]) == {"entry.py", f"{dependency}.py"}


def test_concurrent_graph_cache_publication_is_atomic(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import helper\n", encoding="utf-8")
    (tools / "helper.py").write_text("VALUE = 1\n", encoding="utf-8")

    with ThreadPoolExecutor(max_workers=8) as pool:
        closures = list(
            pool.map(
                lambda _index: local_python_import_closure(tmp_path, (seed,)).paths,
                range(32),
            )
        )

    assert len({closure for closure in closures}) == 1
    cache = json.loads(
        graph.python_source_closure_cache_path(tmp_path).read_text(encoding="utf-8")
    )
    assert cache["schema_version"] == graph._GRAPH_CACHE_SCHEMA_VERSION
    assert set(cache["entries"]) == {"tools/entry.py", "tools/helper.py"}


def test_non_mapping_graph_cache_is_ignored_and_replaced(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    cache_path = graph.python_source_closure_cache_path(tmp_path)
    tools.mkdir()
    cache_path.parent.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("import helper\n", encoding="utf-8")
    (tools / "helper.py").write_text("VALUE = 1\n", encoding="utf-8")
    cache_path.write_text("[]\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,)).paths

    assert _relative_paths(tmp_path, closure) == {
        "tools/entry.py",
        "tools/helper.py",
    }
    assert (
        json.loads(cache_path.read_text(encoding="utf-8"))["schema_version"]
        == graph._GRAPH_CACHE_SCHEMA_VERSION
    )


@pytest.mark.parametrize("unknown_fields", [False, True])
def test_validated_graph_hit_reuses_storage_and_changed_bytes_publish(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, unknown_fields: bool
) -> None:
    seed, helper = tmp_path / "entry.py", tmp_path / "helper.py"
    seed.write_bytes(b"import helper\n")
    helper.write_bytes(b"VALUE = 1\n")
    original = local_python_import_closure(tmp_path, (seed,))
    cache_path = graph.python_source_closure_cache_path(tmp_path)
    if unknown_fields:
        cache = json.loads(cache_path.read_text(encoding="utf-8"))
        row = next(iter(cache["entries"]["entry.py"].values()))
        # Unknown storage metadata cannot become a graph request, even when a
        # valid row is retained instead of round-tripped through its projection.
        row["future_requests"] = ["unowned"]
        row["requests"][0]["future_candidates"] = ["unowned"]
        (tmp_path / "unowned.py").write_text("VALUE = 9\n", encoding="utf-8")
        cache_path.write_text(json.dumps(cache), encoding="utf-8")
    before = cache_path.read_bytes()

    def unnecessary(*_args, **_kwargs):
        raise AssertionError("validated hit must not reanalyze, reserialize or publish")

    with monkeypatch.context() as patch:
        patch.setattr(graph, "analyze_local_imports", unnecessary)
        patch.setattr(graph, "_analysis_payload", unnecessary)
        patch.setattr(graph, "_write_graph_cache", unnecessary)
        hit = local_python_import_closure(tmp_path, (seed,))
    assert hit == original
    assert set(hit.paths) == {seed, helper}
    assert hit.source_sha256[helper] == hashlib.sha256(b"VALUE = 1\n").hexdigest()
    assert cache_path.read_bytes() == before

    helper.write_bytes(b"VALUE = 2\n")
    changed = local_python_import_closure(tmp_path, (seed,))
    assert set(changed.paths) == {seed, helper}
    assert changed.content_digest != original.content_digest
    cache = json.loads(cache_path.read_text(encoding="utf-8"))
    row = next(iter(cache["entries"]["helper.py"].values()))
    assert row["source_sha256"] == hashlib.sha256(b"VALUE = 2\n").hexdigest()
    assert cache_path.read_bytes() != before


@pytest.mark.parametrize(
    "field, invalid_row",
    [
        ("requests", {"kind": "direct", "candidates": [], "line": 1, "column": 0}),
        (
            "discovery_requests",
            {"kind": "direct", "candidates": ["unowned"], "line": True, "column": 0},
        ),
        ("unresolved_dynamic_imports", {"line": 1, "column": 0, "message": None}),
        (
            "relative_source_obligations",
            {
                "kind": "statement",
                "name": "",
                "level": True,
                "fromlist": [],
                "line": 1,
                "column": 0,
            },
        ),
    ],
)
def test_malformed_known_graph_fields_reanalyze_and_replace(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, field: str, invalid_row: dict
) -> None:
    seed, helper = tmp_path / "entry.py", tmp_path / "helper.py"
    seed.write_text("import helper\n", encoding="utf-8")
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    original = local_python_import_closure(tmp_path, (seed,))
    cache_path = graph.python_source_closure_cache_path(tmp_path)
    original_cache = cache_path.read_bytes()
    cache = json.loads(original_cache)
    row = next(iter(cache["entries"]["entry.py"].values()))
    row[field] = [invalid_row]
    cache_path.write_text(json.dumps(cache), encoding="utf-8")
    analyze = graph.analyze_local_imports
    analyzed = []

    def record(snapshot, *args, **kwargs):
        analyzed.append(snapshot.path)
        return analyze(snapshot, *args, **kwargs)

    monkeypatch.setattr(graph, "analyze_local_imports", record)
    repaired = local_python_import_closure(tmp_path, (seed,))
    assert repaired == original
    assert set(repaired.paths) == {seed, helper}
    assert analyzed == [seed]
    assert json.loads(cache_path.read_bytes()) == json.loads(original_cache)


@pytest.mark.parametrize("include_attribute", [False, True])
@pytest.mark.parametrize("full_first", [False, True])
def test_grouped_fromlist_graph_preserves_distinct_consumer_policies(
    tmp_path: Path, include_attribute: bool, full_first: bool
) -> None:
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    seed = package / "entry.py"
    seed.write_text(
        "from pkg import child" + (", attribute" if include_attribute else "") + "\n",
        encoding="utf-8",
    )
    initializer, child, aggregate = (
        package / "__init__.py",
        package / "child.py",
        package / "aggregate.py",
    )
    initializer.write_text("import pkg.aggregate\nattribute = 1\n", encoding="utf-8")
    child.write_text("VALUE = 1\n", encoding="utf-8")
    aggregate.write_text("VALUE = 2\n", encoding="utf-8")
    lowering = PythonImportPolicy(True, False, False)
    policies = (graph._EXECUTABLE_TOOL_IMPORT_POLICY, lowering)
    if not full_first:
        policies = policies[::-1]
    # Both orders, including persistent cache hits, must retain the independent
    # owner fallback for an attribute beside a successfully resolved submodule.
    for policy in (*policies, *policies):
        paths = set(local_python_import_closure(tmp_path, (seed,), policy=policy).paths)
        expected = {seed, child}
        if policy.include_parent_packages or include_attribute:
            expected.update((initializer, aggregate))
        assert paths == expected
    cache = json.loads(
        (graph.python_source_closure_cache_path(tmp_path)).read_text(encoding="utf-8")
    )
    assert len(cache["entries"]["src/pkg/entry.py"]) == 2


@pytest.mark.parametrize("warm", [False, True])
def test_transitive_dynamic_import_diagnostic_survives_cached_importer(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, warm: bool
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed, helper = tools / "entry.py", tools / "helper.py"
    seed.write_text("import helper\n", encoding="utf-8")
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    if warm:
        local_python_import_closure(tmp_path, (seed,)).paths
    helper.write_text(
        "import importlib\nimportlib.import_module(name)\n", encoding="utf-8"
    )
    analyzed = []
    analyze = graph.analyze_local_imports

    def record(snapshot, *args, **kwargs):
        analyzed.append(snapshot.path)
        return analyze(snapshot, *args, **kwargs)

    monkeypatch.setattr(graph, "analyze_local_imports", record)
    with pytest.raises(ValueError, match="non-literal dynamic Python import") as exc:
        local_python_import_closure(tmp_path, (seed,)).paths
    assert str(helper) + ":2:0" in str(exc.value)
    assert (seed in analyzed) == (not warm)


def test_lowering_cached_graph_cannot_hide_executable_dynamic_diagnostic(
    tmp_path: Path,
) -> None:
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    seed, helper = package / "entry.py", package / "helper.py"
    seed.write_text("import pkg.helper\n", encoding="utf-8")
    helper.write_text("def deferred():\n    __import__(unknown)\n", encoding="utf-8")
    assert set(
        local_python_import_closure(
            tmp_path, (seed,), policy=PythonImportPolicy(True, False, False)
        ).paths
    ) == {seed, helper}
    with pytest.raises(ValueError, match="non-literal dynamic Python import") as exc:
        local_python_import_closure(tmp_path, (seed,)).paths
    assert str(helper) + ":2:4" in str(exc.value)


def test_source_capture_keys_and_parses_the_same_byte_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed, first, later = (tools / name for name in ("entry.py", "first.py", "later.py"))
    original = b"import first\n"
    seed.write_bytes(original)
    first.write_text("VALUE = 1\n", encoding="utf-8")
    later.write_text("VALUE = 2\n", encoding="utf-8")
    capture = graph.LocalPythonModuleResolver.capture_source
    captures = []

    def capture_then_edit(resolver, path):
        snapshot = capture(resolver, path)
        captures.append(path)
        if path == seed:
            seed.write_text("import later\n", encoding="utf-8")
        return snapshot

    monkeypatch.setattr(
        graph.LocalPythonModuleResolver, "capture_source", capture_then_edit
    )
    receipt = local_python_import_closure(tmp_path, (seed,))
    assert set(receipt.paths) == {seed, first}
    assert receipt.source_sha256[seed] == hashlib.sha256(original).hexdigest()
    assert captures.count(seed) == 1
    cache = json.loads(
        (graph.python_source_closure_cache_path(tmp_path)).read_text(encoding="utf-8")
    )
    row = next(iter(cache["entries"]["tools/entry.py"].values()))
    assert row["source_sha256"] == hashlib.sha256(original).hexdigest()
    assert set(local_python_import_closure(tmp_path, (seed,)).paths) == {seed, later}


def test_graph_replaces_contract_generations_and_prunes_deleted_sources(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed, first, later = (tools / name for name in ("entry.py", "first.py", "later.py"))
    seed.write_text("__import__(name)\n", encoding="utf-8")
    first.write_text("VALUE = 1\n", encoding="utf-8")
    later.write_text("VALUE = 2\n", encoding="utf-8")
    manifest = tmp_path / graph._DYNAMIC_IMPORT_MANIFEST
    manifest.parent.mkdir(parents=True)
    for target in ("first", "later"):
        manifest.write_text(
            "schema_version = 1\n[[source]]\npath = 'tools/entry.py'\n"
            f"nonliteral_calls = 1\nmodules = ['{target}']\n",
            encoding="utf-8",
        )
        local_python_import_closure(tmp_path, (seed,)).paths
    first.unlink()
    local_python_import_closure(tmp_path, (seed,)).paths
    cache = json.loads(
        (graph.python_source_closure_cache_path(tmp_path)).read_text(encoding="utf-8")
    )
    assert set(cache["entries"]) == {"tools/entry.py", "tools/later.py"}
    assert len(cache["entries"]["tools/entry.py"]) == 1
    row = next(iter(cache["entries"]["tools/entry.py"].values()))
    assert len(row["unresolved_dynamic_imports"]) == 1
    # A well-typed cached diagnostic still has to satisfy the dynamic contract.
    # Hit storage reuse must not turn successful decoding into semantic approval.
    cache_path = graph.python_source_closure_cache_path(tmp_path)
    accepted_bytes = cache_path.read_bytes()
    row["unresolved_dynamic_imports"].append(dict(row["unresolved_dynamic_imports"][0]))
    cache_path.write_text(json.dumps(cache), encoding="utf-8")
    invalid_bytes = cache_path.read_bytes()
    with pytest.raises(ValueError, match="manifest drift"):
        local_python_import_closure(tmp_path, (seed,))
    assert cache_path.read_bytes() == invalid_bytes
    cache_path.write_bytes(accepted_bytes)
    # A changed contract cannot reuse the accepted unresolved-call row.
    manifest.write_text(
        "schema_version = 1\n[[source]]\npath = 'tools/entry.py'\n"
        "nonliteral_calls = 0\nmodules = ['later']\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="manifest drift"):
        local_python_import_closure(tmp_path, (seed,)).paths


def test_graph_reparses_after_parser_or_binding_authority_change(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("VALUE = 1\n", encoding="utf-8")
    local_python_import_closure(tmp_path, (seed,)).paths
    identity = graph.local_import_analysis_identity()
    monkeypatch.setattr(
        graph,
        "local_import_analysis_identity",
        lambda: (*identity, "changed-authority"),
    )
    analyzed = []
    analyze = graph.analyze_local_imports

    def record(snapshot, *args, **kwargs):
        analyzed.append(snapshot.path)
        return analyze(snapshot, *args, **kwargs)

    monkeypatch.setattr(graph, "analyze_local_imports", record)
    local_python_import_closure(tmp_path, (seed,)).paths
    assert analyzed == [seed]
    cache = json.loads(
        (graph.python_source_closure_cache_path(tmp_path)).read_text(encoding="utf-8")
    )
    assert len(cache["entries"]["tools/entry.py"]) == 1


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

    closure = local_python_import_closure(tmp_path, (seed,)).paths
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
    requests = (
        "from . import local_leaf\n",
        "from .. import middle_leaf\n",
        "from ... import root_leaf\n",
    )
    paths: set[str] = set()
    for request in requests:
        seed.write_text(request, encoding="utf-8")
        closure = local_python_import_closure(tmp_path, (seed,)).paths
        paths.update(_relative_paths(tmp_path, closure))
    assert {
        "src/pkg/root_leaf.py",
        "src/pkg/deep/middle_leaf.py",
        "src/pkg/deep/more/local_leaf.py",
    } <= paths

    # Executing an earlier import re-enters Python, but no code in this graph
    # writes the entry's metadata, so every later level keeps its anchor.
    seed.write_text("".join(requests), encoding="utf-8")
    exact = local_python_import_closure(tmp_path, (seed,))
    assert not exact.topology_digest
    assert paths <= _relative_paths(tmp_path, exact.paths)

    # A named write of the entry's __package__ from an imported module leaves
    # the later levels without an anchor: they cover every local source.
    (nested / "local_leaf.py").write_text(
        "import pkg.deep.more.entry\npkg.deep.more.entry.__package__ = 'pkg'\n",
        encoding="utf-8",
    )
    covered = local_python_import_closure(tmp_path, (seed,))
    assert covered.topology_digest
    assert paths <= _relative_paths(tmp_path, covered.paths)
    (nested / "local_leaf.py").write_text("VALUE = 3\n", encoding="utf-8")

    seed.write_text("from .... import escaped\n", encoding="utf-8")
    with pytest.raises(ValueError, match="escapes local package"):
        local_python_import_closure(tmp_path, (seed,)).paths


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

    closure = local_python_import_closure(tmp_path, (seed,)).paths

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

    closure = local_python_import_closure(tmp_path, (seed,)).paths
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

    closure = local_python_import_closure(tmp_path, (seed,)).paths
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
    root = tmp_path / "project"
    tools = root / "tools"
    outside = tmp_path / "outside.py"
    tools.mkdir(parents=True)
    outside.write_text("VALUE = 1\n", encoding="utf-8")
    seed = tools / "entry.py"
    try:
        seed.symlink_to(outside)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")

    with pytest.raises(ValueError, match="outside project root"):
        local_python_import_closure(root, (seed,)).paths


def test_module_name_case_resolution_follows_host_filesystem(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    src = tmp_path / "src"
    tools.mkdir()
    src.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import mixedcase\n", encoding="utf-8")
    (src / "MixedCase.py").write_text("VALUE = 1\n", encoding="utf-8")

    closure = local_python_import_closure(tmp_path, (seed,)).paths
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
        local_python_import_closure(tmp_path, (seed,)).paths


@pytest.mark.parametrize(
    ("source", "message"),
    [
        ("__import__('pkg', level=-1)\n", "negative __import__ level"),
        ("__import__('pkg', level=1.0)\n", "invalid __import__ level"),
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
        local_python_import_closure(tmp_path, (seed,)).paths


@pytest.mark.parametrize("host_version", [(3, 12), (3, 13), (3, 14)])
def test_graph_binding_policy_and_identity_share_host_source_semantics(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, host_version: tuple[int, int]
) -> None:
    from molt.cli import python_import_resolution as resolution
    from types import SimpleNamespace

    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    seed = package / "entry.py"
    seed.write_text("from . import leaf\n", encoding="utf-8")
    (package / "__init__.py").write_text("", encoding="utf-8")
    leaf = package / "leaf.py"
    leaf.write_text("VALUE = 1\n", encoding="utf-8")
    monkeypatch.setattr(
        resolution,
        "sys",
        SimpleNamespace(
            implementation=resolution.sys.implementation,
            version_info=(*host_version, 0, "final", 0),
        ),
    )
    policies = []
    analyze = resolution.analyze_python_bindings

    def record(tree, *, source_digest, policy):
        policies.append(policy)
        return analyze(tree, source_digest=source_digest, policy=policy)

    monkeypatch.setattr(resolution, "analyze_python_bindings", record)
    assert leaf in local_python_import_closure(tmp_path, (seed,)).paths
    assert policies
    assert all(policy.target_python == host_version for policy in policies)
    assert graph.local_import_analysis_identity()[-1]["target_python"] == host_version
    cold_count = len(policies)
    assert leaf in local_python_import_closure(tmp_path, (seed,)).paths
    assert len(policies) == cold_count


def test_canonical_tools_import_keeps_relative_package_context(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    seed, helper, child = (
        tools / name for name in ("entry.py", "helper.py", "child.py")
    )
    seed.write_text("import tools.helper\n", encoding="utf-8")
    helper.write_text("from . import child\n", encoding="utf-8")
    child.write_text("VALUE = 1\n", encoding="utf-8")
    expected = {seed, helper, child}
    assert set(local_python_import_closure(tmp_path, (seed,)).paths) == expected
    assert set(local_python_import_closure(tmp_path, (seed,)).paths) == expected
    # A source that is valid as tools.helper cannot lend its package context to
    # the same bytes imported as the top-level helper module, even on a warm hit.
    seed.write_text("import tools.helper\nimport helper\n", encoding="utf-8")
    with pytest.raises(ValueError, match="no known parent package"):
        local_python_import_closure(tmp_path, (seed,)).paths


def test_namespace_leaf_retains_regular_ancestor_initializers(tmp_path: Path) -> None:
    package = tmp_path / "src" / "pkg"
    (package / "namespace" / "nested").mkdir(parents=True)
    tools = tmp_path / "tools"
    tools.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import pkg.namespace.nested\n", encoding="utf-8")
    initializer = package / "__init__.py"
    initializer.write_text("from . import boot\n", encoding="utf-8")
    boot = package / "boot.py"
    boot.write_text("VALUE = 1\n", encoding="utf-8")
    assert set(local_python_import_closure(tmp_path, (seed,)).paths) == {
        seed,
        initializer,
        boot,
    }


def test_manifest_tool_package_tree_preserves_declared_qualified_names(
    tmp_path: Path,
) -> None:
    package = tmp_path / "tools" / "pkg"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    helper = package / "helper.py"
    helper.write_text("from .. import root_dep\n", encoding="utf-8")
    dependency = package.parent / "root_dep.py"
    dependency.write_text("VALUE = 1\n", encoding="utf-8")
    seed = package.parent / "entry.py"
    seed.write_text("__import__(name)\n", encoding="utf-8")
    manifest = tmp_path / graph._DYNAMIC_IMPORT_MANIFEST
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        "schema_version = 1\n[[source]]\npath = 'tools/entry.py'\n"
        "nonliteral_calls = 1\nmodule_trees = ['tools.pkg']\n",
        encoding="utf-8",
    )
    assert {helper, dependency, manifest} <= set(
        local_python_import_closure(tmp_path, (seed,)).paths
    )


def test_import_alias_contexts_share_bytes_not_analysis(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import ast
    import gc
    import weakref
    from molt.cli.python_import_resolution import LocalPythonModuleResolver

    package = tmp_path / "tools" / "pkg"
    package.mkdir(parents=True)
    seed = package.parent / "entry.py"
    seed.write_text("import pkg.helper\nimport tools.pkg.helper\n", encoding="utf-8")
    (package / "__init__.py").write_text("", encoding="utf-8")
    helper, child = package / "helper.py", package / "child.py"
    helper.write_text("from . import child\n", encoding="utf-8")
    child.write_text("VALUE = 1\n", encoding="utf-8")
    captures: list[Path] = []
    contexts: list[str] = []
    capture = LocalPythonModuleResolver.capture_source
    analyze = graph.analyze_local_imports
    parse = ast.parse
    trees = []

    def record_tree(*args, **kwargs):
        tree = parse(*args, **kwargs)
        if kwargs.get("filename") == str(helper):
            trees.append(weakref.ref(tree))
        return tree

    def record_capture(self, path):
        captures.append(path)
        return capture(self, path)

    def record_analysis(snapshot, module_source, *args, **kwargs):
        # Prior projections may keep compact facts, never a previous AST. Collect
        # cycles so the ownership oracle does not depend on reference counting.
        gc.collect()
        assert all(tree() is None for tree in trees)
        if snapshot.path == helper:
            contexts.append(module_source.name)
        return analyze(snapshot, module_source, *args, **kwargs)

    monkeypatch.setattr(LocalPythonModuleResolver, "capture_source", record_capture)
    monkeypatch.setattr(graph, "analyze_local_imports", record_analysis)
    monkeypatch.setattr(ast, "parse", record_tree)
    assert child in local_python_import_closure(tmp_path, (seed,)).paths
    gc.collect()
    assert trees and all(tree() is None for tree in trees)
    assert set(contexts) == {"pkg.helper", "tools.pkg.helper"}
    assert captures.count(helper) == 1
    cache = json.loads(
        (graph.python_source_closure_cache_path(tmp_path)).read_text(encoding="utf-8")
    )
    assert len(cache["entries"]["tools/pkg/helper.py"]) == 2
    contexts.clear()
    assert child in local_python_import_closure(tmp_path, (seed,)).paths
    assert contexts == []


@pytest.mark.parametrize("source_first", [False, True])
def test_source_discovery_cache_rows_cannot_be_reused_as_semantic_edges(
    tmp_path, source_first
):
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    seed, child = package / "entry.py", package / "child.py"
    # A module that hands its namespace on loses its anchor at the next call.
    seed.write_text("register(globals())\nfrom . import child\n", encoding="utf-8")
    child.write_text("VALUE = 1\n", encoding="utf-8")
    source = PythonImportPolicy(
        False,
        False,
        False,
        purpose="source_dependency",
        unknown_relative_sources="local_inventory",
    )
    semantic = PythonImportPolicy(False, False, False)
    policies = (source, semantic) if source_first else (semantic, source)
    for policy in (*policies, *policies):
        paths = set(local_python_import_closure(tmp_path, (seed,), policy=policy).paths)
        assert (child in paths) is (policy.purpose == "source_dependency")
    cache = json.loads(
        graph.python_source_closure_cache_path(tmp_path).read_text(encoding="utf-8")
    )
    variants = cache["entries"]["src/pkg/entry.py"]
    premise = PythonNamespacePremise((), 0)
    source_key = graph._analysis_policy_digest("pkg.entry", False, source, premise)
    semantic_key = graph._analysis_policy_digest("pkg.entry", False, semantic, premise)
    assert source_key != semantic_key
    assert variants[source_key]["discovery_requests"]
    assert variants[source_key]["relative_source_obligations"]
    assert not variants[source_key]["unresolved_dynamic_imports"]
    assert variants[semantic_key]["discovery_requests"] == []
    assert variants[semantic_key]["relative_source_obligations"] == []
    assert variants[semantic_key]["unresolved_dynamic_imports"]
    assert all(
        "pkg.child" not in row["candidates"] for row in variants[source_key]["requests"]
    )


def test_float_import_level_is_a_known_type_error():
    with pytest.raises(TypeError):
        __import__("pkg", level=1.0)


@pytest.mark.parametrize(
    "statement,package",
    [
        ("from .missing.deep import value", "alternate"),
        ("from ...missing.deep import value", "alternate.extra.deep"),
        ("from . import value", "alternate"),
        ("__import__('missing.deep', globals(), None, (), 1)", "alternate"),
        ("importlib.import_module('.missing.deep', package)", "alternate"),
    ],
)
def test_unknown_relative_inventory_covers_failed_owner_execution(
    tmp_path, monkeypatch, statement, package
):
    """Independent CPython witness: even a failed suffix executes its owner."""
    import importlib
    import sys

    marker = tmp_path / "executed.txt"
    alternate = tmp_path / "alternate.py"
    alternate.write_text(
        f"from pathlib import Path\nPath({str(marker)!r}).write_text('executed')\n",
        encoding="utf-8",
    )
    seed = tmp_path / "entry.py"
    # The operand is unknown to static analysis; the oracle supplies it exactly.
    seed.write_text("__package__ = package\n" + statement + "\n", encoding="utf-8")
    namespace = {
        "__name__": "entry",
        "package": package,
        "importlib": importlib,
    }
    monkeypatch.syspath_prepend(str(tmp_path))
    forget_module(monkeypatch, "alternate")
    try:
        with pytest.raises(ImportError):
            exec(
                compile(seed.read_text(encoding="utf-8"), str(seed), "exec"), namespace
            )
        assert marker.read_text(encoding="utf-8") == "executed"
    finally:
        sys.modules.pop("alternate", None)
    if statement.startswith("importlib"):
        seed.write_text(
            "import importlib\n" + seed.read_text(encoding="utf-8"), encoding="utf-8"
        )
    receipt = local_python_import_closure(tmp_path, (seed,))
    assert alternate in receipt.paths and receipt.topology_digest
    cache = json.loads(
        graph.python_source_closure_cache_path(tmp_path).read_text(encoding="utf-8")
    )
    variants = cache["entries"]["entry.py"].values()
    assert any(row["relative_source_obligations"] for row in variants)
    assert all(not row["unresolved_dynamic_imports"] for row in variants)
    # Owner coverage is byte custody; it never reparses speculative owner code.
    assert "alternate.py" not in cache["entries"]


def test_relative_inventory_uses_resolver_shadows_namespaces_and_new_topology(
    tmp_path, monkeypatch
):
    from molt.cli.python_import_resolution import LocalPythonModuleResolver

    first, second = tmp_path / "first", tmp_path / "second"
    for root in (first, second):
        (root / "ns").mkdir(parents=True)
    seed = first / "entry.py"
    seed.write_text(
        "__package__ = unknown\nfrom .missing import value\n", encoding="utf-8"
    )
    left, right = first / "ns/left.py", second / "ns/right.py"
    left.write_text("LEFT = 1\n", encoding="utf-8")
    right.write_text("RIGHT = 1\n", encoding="utf-8")
    (first / "blocked.py").write_text("BLOCK = 1\n", encoding="utf-8")
    (second / "blocked").mkdir()
    hidden = second / "blocked/child.py"
    hidden.write_text("HIDDEN = 1\n", encoding="utf-8")
    roots = (first, second)
    inventory = LocalPythonModuleResolver(roots).source_inventory(
        allowed_prefix=None, include_parent_packages=True
    )
    assert {source.path for source in inventory.sources} == {
        seed,
        left,
        right,
        first / "blocked.py",
    }
    before = local_python_import_closure(tmp_path, (seed,), search_roots=roots)
    assert left in before.paths and right in before.paths and hidden not in before.paths

    def no_reanalysis(*args, **kwargs):
        raise AssertionError(
            "unchanged seed should reuse its symbolic cached obligation"
        )

    monkeypatch.setattr(graph, "analyze_local_imports", no_reanalysis)
    # An empty namespace changes topology without changing captured source paths.
    (first / "new_namespace").mkdir()
    namespace = local_python_import_closure(tmp_path, (seed,), search_roots=roots)
    assert namespace.paths == before.paths
    assert namespace.topology_digest != before.topology_digest
    assert namespace.content_digest != before.content_digest
    # A regular package in the later root shadows both earlier namespace portions.
    initializer = second / "ns/__init__.py"
    initializer.write_text("VALUE = 1\n", encoding="utf-8")
    after = local_python_import_closure(tmp_path, (seed,), search_roots=roots)
    assert left not in after.paths and {right, initializer} <= set(after.paths)
    added = second / "ns/added.py"
    added.write_text("VALUE = 2\n", encoding="utf-8")
    newest = local_python_import_closure(tmp_path, (seed,), search_roots=roots)
    assert added in newest.paths and newest.content_digest != after.content_digest


def test_relative_inventory_ignores_the_bytecode_cache_directory(tmp_path):
    root = tmp_path / "root"
    (root / "pkg").mkdir(parents=True)
    seed = root / "entry.py"
    seed.write_text(
        "__package__ = unknown\nfrom .missing import value\n", encoding="utf-8"
    )
    (root / "pkg/mod.py").write_text("VALUE = 1\n", encoding="utf-8")
    before = local_python_import_closure(tmp_path, (seed,), search_roots=(root,))
    assert before.topology_digest
    # Python writes these on first import, so a fresh checkout gains them while
    # it runs; as namespace members they changed every tooling identity.
    (root / "__pycache__").mkdir()
    (root / "pkg/__pycache__").mkdir()
    (root / "pkg/__pycache__/mod.cpython-312.pyc").write_bytes(b"\0")
    after = local_python_import_closure(tmp_path, (seed,), search_roots=(root,))
    assert after.content_digest == before.content_digest


def test_relative_inventory_keeps_manifest_and_invalid_operand_errors(tmp_path):
    seed = tmp_path / "entry.py"
    manifest = tmp_path / graph._DYNAMIC_IMPORT_MANIFEST
    manifest.parent.mkdir(parents=True)
    seed.write_text(
        "__package__ = unknown\nfrom . import child\n__import__(name)\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="non-literal dynamic Python import"):
        local_python_import_closure(tmp_path, (seed,))
    manifest.write_text(
        "schema_version = 1\n[[source]]\npath = 'entry.py'\nnonliteral_calls = 1\n",
        encoding="utf-8",
    )
    receipt = local_python_import_closure(tmp_path, (seed,))
    assert receipt.topology_digest
    # The exact dynamic count survives full local coverage, including cache hits.
    seed.write_text(
        seed.read_text(encoding="utf-8") + "__import__(other)\n", encoding="utf-8"
    )
    with pytest.raises(ValueError, match="manifest drift"):
        local_python_import_closure(tmp_path, (seed,))
    manifest.unlink()
    for source, message in (
        ("__package__ = 1\nfrom . import child\n", "invalid import package"),
        ("from .. import child\n", "no known parent package"),
        ("__import__('child', globals(), None, (), -1)\n", "negative __import__ level"),
        (
            "import importlib\nimportlib.import_module('.child', 1)\n",
            "invalid import package",
        ),
    ):
        seed.write_text(source, encoding="utf-8")
        with pytest.raises(ValueError, match=message):
            local_python_import_closure(tmp_path, (seed,))


def test_inventory_failure_and_directory_cycle_never_publish_partial_coverage(
    tmp_path, monkeypatch
):
    from molt.cli import python_import_topology as topology

    seed = tmp_path / "entry.py"
    seed.write_text("__package__ = unknown\nfrom . import child\n", encoding="utf-8")
    real_scandir = topology.os.scandir

    def denied(path):
        if Path(path) == tmp_path:
            raise PermissionError("cannot inventory owner domain")
        return real_scandir(path)

    with monkeypatch.context() as patch:
        patch.setattr(topology.os, "scandir", denied)
        with pytest.raises(
            ValueError, match="cannot enumerate local Python source domain"
        ):
            local_python_import_closure(tmp_path, (seed,))
    package = tmp_path / "package"
    package.mkdir()
    try:
        (package / "loop").symlink_to(package, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlinks unavailable: {exc}")
    with pytest.raises(ValueError, match="cyclic local Python search topology"):
        local_python_import_closure(tmp_path, (seed,))


def test_declared_source_domain_bounds_inventory_and_qualified_tool_aliases(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import python_import_topology as topology

    tools = tmp_path / "tools"
    tools.mkdir()
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    seed = tools / "entry.py"
    seed.write_text("__package__ = unknown\nfrom . import child\n", encoding="utf-8")
    helper = tools / "helper.py"
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    initializer = package / "__init__.py"
    initializer.write_text("", encoding="utf-8")
    leaf = package / "leaf.py"
    leaf.write_text("VALUE = 2\n", encoding="utf-8")
    scratch = tmp_path / "tmp" / "pytest-current" / "package"
    scratch.mkdir(parents=True)
    noise = scratch / "child.py"
    noise.write_text("scratch must never be a tooling dependency", encoding="utf-8")
    root_module = tmp_path / "unowned.py"
    root_module.write_text("root namespace is not source ownership", encoding="utf-8")
    manifest = tmp_path / graph._DYNAMIC_IMPORT_MANIFEST
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        'schema_version = 1\nsearch_roots = ["src", "tools", "."]\n'
        'source_roots = ["src", "tools"]\nsource = []\n',
        encoding="utf-8",
    )
    real_scandir = topology.os.scandir

    def admitted_scandir(path):
        assert not Path(path).is_relative_to(tmp_path / "tmp")
        return real_scandir(path)

    install_module_view(monkeypatch, "os", os, topology, scandir=admitted_scandir)
    before = local_python_import_closure(tmp_path, (seed,))
    assert set(before.paths) == {seed, helper, initializer, leaf, manifest}
    assert before.topology_digest
    noise.write_text("changed scratch generation", encoding="utf-8")
    assert local_python_import_closure(tmp_path, (seed,)) == before
    helper.write_text("VALUE = 3\n", encoding="utf-8")
    assert local_python_import_closure(tmp_path, (seed,)).content_digest != (
        before.content_digest
    )
    seed.write_text("import tools.helper\nfrom pkg import leaf\n", encoding="utf-8")
    qualified = local_python_import_closure(tmp_path, (seed,))
    assert set(qualified.paths) == {seed, helper, initializer, leaf, manifest}
    with pytest.raises(ValueError, match="outside local source roots"):
        local_python_import_closure(tmp_path, (root_module,))


def test_manifest_search_order_and_domain_changes_invalidate_warm_closure(
    tmp_path: Path,
) -> None:
    tools = tmp_path / "tools"
    src = tmp_path / "src"
    tools.mkdir()
    src.mkdir()
    seed = tools / "entry.py"
    seed.write_text("import shared\n", encoding="utf-8")
    first = src / "shared.py"
    second = tools / "shared.py"
    first.write_text("OWNER = 'src'\n", encoding="utf-8")
    second.write_text("OWNER = 'tools'\n", encoding="utf-8")
    manifest = tmp_path / graph._DYNAMIC_IMPORT_MANIFEST
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        'schema_version = 1\nsearch_roots = ["src", "tools", "."]\n'
        'source_roots = ["src", "tools"]\nsource = []\n',
        encoding="utf-8",
    )
    before = local_python_import_closure(tmp_path, (seed,))
    assert set(before.paths) == {seed, first, manifest}
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace(
            'search_roots = ["src", "tools", "."]',
            'search_roots = ["tools", "src", "."]',
        ),
        encoding="utf-8",
    )
    after = local_python_import_closure(tmp_path, (seed,))
    assert set(after.paths) == {seed, second, manifest}
    assert after.content_digest != before.content_digest
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace(
            'source_roots = ["src", "tools"]', 'source_roots = ["tools"]'
        ),
        encoding="utf-8",
    )
    narrowed = local_python_import_closure(tmp_path, (seed,))
    assert narrowed.content_digest != after.content_digest


@pytest.mark.parametrize("field", ["search_roots", "source_roots"])
@pytest.mark.parametrize(
    "value, message",
    [
        ("[]", "invalid Python tooling"),
        ("[1]", "invalid Python tooling"),
        ('["../outside"]', "invalid Python tooling"),
        ('["C:/outside"]', "invalid Python tooling"),
        ('["/outside"]', "invalid Python tooling"),
        ('["missing"]', "missing Python tooling"),
        ('["src", "./src"]', "duplicate Python tooling"),
    ],
)
def test_manifest_root_contract_rejects_invalid_domains(
    tmp_path: Path, field: str, value: str, message: str
) -> None:
    seed = tmp_path / "src" / "entry.py"
    seed.parent.mkdir()
    seed.write_text("VALUE = 1\n", encoding="utf-8")
    manifest = tmp_path / graph._DYNAMIC_IMPORT_MANIFEST
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        f"schema_version = 1\n{field} = {value}\nsource = []\n", encoding="utf-8"
    )
    with pytest.raises(ValueError, match=message):
        local_python_import_closure(tmp_path, (seed,))


@pytest.mark.parametrize("cache_state", ["cold", "warm"])
@pytest.mark.parametrize("instrumentation", ["none", "tracemalloc"])
@pytest.mark.parametrize("workers", [1, 4])
def test_closure_profile_measures_explicit_cases_with_captured_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    cache_state: str,
    instrumentation: str,
    workers: int,
) -> None:
    from threading import get_ident
    from tools import profile_python_source_closure as profiler

    sources = {"entry.py": b"import leaf\n", "leaf.py": b"VALUE = 1\n"}
    expected_digest = hashlib.sha256()
    for name, content in sorted(sources.items()):
        (tmp_path / name).write_bytes(content)
        expected_digest.update(name.encode() + b"\0" + content + b"\0")
    calls: list[int] = []

    def observe(root: Path, seeds: tuple[Path, ...]):
        if cache_state == "warm" and instrumentation == "tracemalloc" and not calls:
            # A discarded preparation allocation must not become the measured
            # batch peak. Forgetting reset_peak preserves this independent marker.
            preparation = bytearray(8 * 1024 * 1024)
            del preparation
        calls.append(get_ident())
        return local_python_import_closure(root, seeds)

    monkeypatch.setattr(profiler, "local_python_import_closure", observe)
    iterations = 1 if cache_state == "cold" else 2
    result = profiler.profile_closure(
        tmp_path,
        tmp_path / "entry.py",
        cache_state=cache_state,
        instrumentation=instrumentation,
        workers=workers,
        iterations=iterations,
    )
    assert result["closure_content_digest"] == expected_digest.hexdigest()
    assert result["closure_count"] == len(sources)
    assert result["closure_source_bytes"] == sum(map(len, sources.values()))
    assert result["cache_state"] == cache_state
    assert result["cache_existed_before_warmup"] is False
    assert result["instrumentation"] == instrumentation
    assert result["workers"] == workers
    assert result["measured_calls"] == workers * iterations
    assert result["warmup_calls"] == int(cache_state == "warm")
    assert len(calls) == workers * iterations + int(cache_state == "warm")
    assert len(set(calls)) >= workers
    assert len(result["samples"]) == iterations
    for sample in result["samples"]:
        assert sample["wall_ns"] > 0
        assert sample["process_cpu_ns"] >= 0
        if instrumentation == "tracemalloc":
            assert sample["traced_peak_bytes"] >= sample["traced_baseline_bytes"] >= 0
            assert sample["traced_peak_bytes"] >= sample["traced_current_bytes"] >= 0
        else:
            assert sample["traced_peak_bytes"] is None
            assert sample["traced_baseline_bytes"] is None
            assert sample["traced_current_bytes"] is None
    if cache_state == "warm" and instrumentation == "tracemalloc":
        assert result["warmup_traced_peak_bytes"] > 0
        assert result["samples"][0]["traced_baseline_bytes"] > 0
        assert all(
            sample["traced_peak_bytes"] < result["warmup_traced_peak_bytes"]
            for sample in result["samples"]
        )
    else:
        assert result["warmup_traced_peak_bytes"] is None


def test_closure_profile_rejects_mislabeled_cold_without_clearing_cache(
    tmp_path: Path,
) -> None:
    from tools import profile_python_source_closure as profiler

    seed = tmp_path / "entry.py"
    seed.write_text("VALUE = 1\n", encoding="utf-8")
    with pytest.raises(ValueError, match="cold requires one batch"):
        profiler.profile_closure(
            tmp_path,
            seed,
            cache_state="cold",
            instrumentation="none",
            workers=1,
            iterations=2,
        )
    local_python_import_closure(tmp_path, (seed,))
    cache = graph.python_source_closure_cache_path(tmp_path)
    before = cache.read_bytes()
    with pytest.raises(ValueError, match="absent graph cache"):
        profiler.profile_closure(
            tmp_path,
            seed,
            cache_state="cold",
            instrumentation="none",
            workers=1,
            iterations=1,
        )
    assert cache.read_bytes() == before


def test_closure_profile_rejects_source_drift_and_releases_tracing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import tracemalloc
    from tools import profile_python_source_closure as profiler

    seed = tmp_path / "entry.py"
    leaf = tmp_path / "leaf.py"
    seed.write_text("import leaf\n", encoding="utf-8")
    leaf.write_text("VALUE = 1\n", encoding="utf-8")
    calls = 0

    def mutate(root: Path, seeds: tuple[Path, ...]):
        nonlocal calls
        result = local_python_import_closure(root, seeds)
        calls += 1
        if calls == 2:  # Warmup and first measured batch captured the old bytes.
            leaf.write_text("VALUE = 2\n", encoding="utf-8")
        return result

    monkeypatch.setattr(profiler, "local_python_import_closure", mutate)
    with pytest.raises(RuntimeError, match="closure changed"):
        profiler.profile_closure(
            tmp_path,
            seed,
            cache_state="warm",
            instrumentation="tracemalloc",
            workers=1,
            iterations=2,
        )
    assert calls == 3
    assert not tracemalloc.is_tracing()


@pytest.mark.parametrize("instrumentation", ["none", "tracemalloc"])
def test_closure_profile_preserves_another_tracing_owner(
    tmp_path: Path,
    instrumentation: str,
) -> None:
    import tracemalloc
    from tools import profile_python_source_closure as profiler

    tracemalloc.start()
    try:
        with pytest.raises(ValueError, match="exclusive ownership"):
            profiler.profile_closure(
                tmp_path,
                tmp_path / "entry.py",
                cache_state="cold",
                instrumentation=instrumentation,
                workers=1,
                iterations=1,
            )
        assert tracemalloc.is_tracing()
    finally:
        tracemalloc.stop()


def test_fresh_cache_root_reuses_this_process_analysis_but_not_changed_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    seed = project / "entry.py"
    helper = project / "helper.py"
    project.mkdir()
    seed.write_text("import helper\n", encoding="utf-8")
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "first-cache"))
    first = local_python_import_closure(project, (seed,))
    analyzed = []
    analyze = graph.analyze_local_imports

    def record(snapshot, *args, **kwargs):
        analyzed.append(snapshot.path)
        return analyze(snapshot, *args, **kwargs)

    monkeypatch.setattr(graph, "analyze_local_imports", record)
    # A test or build with its own empty cache root reuses this process's
    # content-keyed rows, and still publishes them to the new root.
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "second-cache"))
    assert local_python_import_closure(project, (seed,)) == first
    assert analyzed == []
    assert graph.python_source_closure_cache_path(project).is_file()
    # Changed bytes never reuse a remembered row.
    helper.write_text("import entry\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "third-cache"))
    local_python_import_closure(project, (seed,))
    assert analyzed == [helper.resolve()]


# Module-level execution graphs prune statically dead branches; the full
# lexical tool graph keeps every import statement.
_EAGER_SOURCE_POLICY = PythonImportPolicy(
    module_level_only=True,
    include_parent_packages=False,
    fail_on_nonliteral_dynamic_import=True,
    purpose="source_dependency",
    unknown_relative_sources="local_inventory",
)


def _eager_closure(tmp_path: Path, seed: Path) -> set[Path]:
    return set(
        local_python_import_closure(
            tmp_path, (seed,), policy=_EAGER_SOURCE_POLICY
        ).paths
    )


def _type_checking_package(tmp_path: Path) -> tuple[Path, Path]:
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    seed = package / "entry.py"
    seed.write_text(
        "from typing import TYPE_CHECKING\nimport os\nimport pkg.peer\nload()\n"
        "if TYPE_CHECKING:\n    import pkg.typeonly\n",
        encoding="utf-8",
    )
    (package / "peer.py").write_text("VALUE = 1\n", encoding="utf-8")
    typeonly = package / "typeonly.py"
    typeonly.write_text("VALUE = 2\n", encoding="utf-8")
    return seed, typeonly


def test_type_only_imports_stay_out_after_callbacks(tmp_path: Path) -> None:
    # HF-226: no code in this graph writes the entry's TYPE_CHECKING, so the
    # imports and the call before the guard cannot make it true.
    seed, typeonly = _type_checking_package(tmp_path)
    assert typeonly not in _eager_closure(tmp_path, seed)


@pytest.mark.parametrize("foreign_write", [False, True])
def test_foreign_member_write_precedes_first_tooling_acquisition(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, foreign_write: bool
) -> None:
    import sys

    from tests.process_guard_common import run_guarded_test_process

    seed = tmp_path / "entry.py"
    seed.write_text("import writer\nimport reader\n", encoding="utf-8")
    (tmp_path / "writer.py").write_text(
        "import typing\n" + ("typing.TYPE_CHECKING = True\n" if foreign_write else ""),
        encoding="utf-8",
    )
    (tmp_path / "reader.py").write_text(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import selected\n",
        encoding="utf-8",
    )
    selected = tmp_path / "selected.py"
    selected.write_text("VALUE = 'selected'\n", encoding="utf-8")
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\n"
            "import entry\nprint(json.dumps('selected' in sys.modules))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    imported = json.loads(executed.stdout)
    assert imported is foreign_write
    # The closed graph sees the foreign write even before the reader's first
    # acquisition. Its eager closure must not prune an actually executed edge.
    closure = _eager_closure(tmp_path, seed)
    assert (selected in closure) is imported
    if not foreign_write:
        return

    # Schema 20 could persist the pruned reader even under the grown graph
    # premise. Preserve every other receipt field so this exercises the retired
    # semantic generation rather than rejection of a malformed record.
    cache_path = graph.python_source_closure_cache_path(tmp_path)
    cache = json.loads(cache_path.read_text(encoding="utf-8"))
    cache["schema_version"] = 20
    removed = 0
    for row in cache["entries"]["reader.py"].values():
        requests = row["requests"]
        row["requests"] = [
            request for request in requests if "selected" not in request["candidates"]
        ]
        removed += len(requests) - len(row["requests"])
    assert removed > 0
    cache_path.write_text(json.dumps(cache), encoding="utf-8")
    # Model a fresh compiler process: remembered correct rows must not hide
    # acceptance of the old on-disk result.
    graph._PROCESS_GRAPH_ENTRIES.pop(tmp_path.resolve(), None)
    analyzed = []
    analyze = graph.analyze_local_imports

    def observe(snapshot, *args, **kwargs):
        analyzed.append(snapshot.path)
        return analyze(snapshot, *args, **kwargs)

    monkeypatch.setattr(graph, "analyze_local_imports", observe)
    assert selected in _eager_closure(tmp_path, seed)
    assert tmp_path / "reader.py" in analyzed


def test_a_named_foreign_write_keeps_the_guarded_import(tmp_path: Path) -> None:
    # The peer writes the entry's flag while the entry imports it, so CPython
    # executes the guarded import. The fixpoint finds the write and walks again.
    seed, typeonly = _type_checking_package(tmp_path)
    (seed.parent / "peer.py").write_text(
        "import pkg.entry\npkg.entry.TYPE_CHECKING = True\n", encoding="utf-8"
    )
    assert typeonly in _eager_closure(tmp_path, seed)


def test_a_package_child_binding_keeps_the_guarded_import(tmp_path: Path) -> None:
    # Importing pkg.TYPE_CHECKING binds a module, a true value, to that name in
    # the package's namespace; the import system's write needs no syntax.
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    init = package / "__init__.py"
    init.write_text(
        "from typing import TYPE_CHECKING\nload()\n"
        "if TYPE_CHECKING:\n    from . import typeonly\n",
        encoding="utf-8",
    )
    typeonly = package / "typeonly.py"
    typeonly.write_text("VALUE = 1\n", encoding="utf-8")
    assert typeonly not in _eager_closure(tmp_path, init)
    (package / "TYPE_CHECKING.py").write_text("", encoding="utf-8")
    assert typeonly in _eager_closure(tmp_path, init)


def test_a_dead_relative_import_is_a_lexical_edge(tmp_path: Path) -> None:
    # The flow proves the branch never runs, so no metadata anchors it; the
    # lexical graph resolves it from the module's identity like a dead
    # absolute import, instead of rejecting an unknown anchor.
    package = tmp_path / "src" / "pkg"
    package.mkdir(parents=True)
    init = package / "__init__.py"
    init.write_text(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from . import typeonly\n",
        encoding="utf-8",
    )
    typeonly = package / "typeonly.py"
    typeonly.write_text("VALUE = 1\n", encoding="utf-8")
    closure = local_python_import_closure(tmp_path, (init,))
    assert typeonly in closure.paths
    assert not closure.topology_digest
    assert typeonly not in _eager_closure(tmp_path, init)


def test_namespace_premise_fixpoint_reuses_every_cached_round(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.compiler_analysis import python_namespace_writes

    monkeypatch.setattr(
        python_namespace_writes,
        "resolve_python_private_names",
        lambda *_args: pytest.fail("captured host tree was normalized again"),
    )
    seed, typeonly = _type_checking_package(tmp_path)
    (seed.parent / "peer.py").write_text(
        "import pkg.entry\npkg.entry.TYPE_CHECKING = True\n", encoding="utf-8"
    )
    first = local_python_import_closure(tmp_path, (seed,), policy=_EAGER_SOURCE_POLICY)
    assert typeonly in first.paths
    graph._PROCESS_GRAPH_ENTRIES.clear()
    monkeypatch.setattr(
        graph,
        "analyze_local_imports",
        lambda *_args, **_kwargs: pytest.fail("unchanged source missed its cache"),
    )
    monkeypatch.setattr(
        graph,
        "_resolved_python_namespace_summary",
        lambda *_args, **_kwargs: pytest.fail("unchanged source was summarized again"),
    )
    assert (
        local_python_import_closure(tmp_path, (seed,), policy=_EAGER_SOURCE_POLICY)
        == first
    )


@pytest.mark.parametrize("reverse", [False, True])
def test_distinct_operation_queries_share_bytes_but_keep_exact_receipts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, reverse: bool
) -> None:
    left, right, shared, only_left, only_right = (
        tmp_path / name
        for name in (
            "left.py",
            "right.py",
            "shared.py",
            "only_left.py",
            "only_right.py",
        )
    )
    left.write_bytes(b"import shared\nimport only_left\n")
    right.write_bytes(b"import shared\nimport only_right\n")
    shared.write_bytes(b"VALUE = 'shared'\n")
    only_left.write_bytes(b"VALUE = 'left'\n")
    only_right.write_bytes(b"VALUE = 'right'\n")
    expected = {
        left: local_python_import_closure(tmp_path, (left,)),
        right: local_python_import_closure(tmp_path, (right,)),
    }
    assert set(expected[left].paths) == {left, shared, only_left}
    assert set(expected[right].paths) == {right, shared, only_right}
    captures = []
    capture = graph.LocalPythonModuleResolver.capture_source

    def observe(resolver, path):
        captures.append(path)
        return capture(resolver, path)

    monkeypatch.setattr(graph.LocalPythonModuleResolver, "capture_source", observe)
    seeds = (right, left) if reverse else (left, right)
    with graph.local_python_import_graph_transaction():
        for seed in seeds:
            actual = local_python_import_closure(tmp_path, (seed,))
            assert actual == expected[seed]
            assert (
                actual.source_sha256[shared]
                == hashlib.sha256(b"VALUE = 'shared'\n").hexdigest()
            )
            with graph.local_python_import_graph_transaction():
                assert local_python_import_closure(tmp_path, (seed,)) is actual
    assert set(captures) == {left, right, shared, only_left, only_right}
    assert len(captures) == 5


def test_operation_inventory_is_shared_across_seeds_and_premise_rounds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    left, right, speculative = (
        tmp_path / name for name in ("left.py", "right.py", "speculative.py")
    )
    for seed in (left, right):
        seed.write_text("__package__ = unknown\nfrom .missing import value\n")
    # Covered bytes must not acquire speculative analysis or syntax errors.
    speculative.write_bytes(b"this is not valid python (\n")
    inventories = []
    captures = []
    inventory = graph.LocalPythonModuleResolver.source_inventory
    capture = graph.LocalPythonModuleResolver.capture_source

    def observe_inventory(resolver, **kwargs):
        inventories.append(kwargs)
        return inventory(resolver, **kwargs)

    def observe_capture(resolver, path):
        captures.append(path)
        return capture(resolver, path)

    monkeypatch.setattr(
        graph.LocalPythonModuleResolver, "source_inventory", observe_inventory
    )
    monkeypatch.setattr(
        graph.LocalPythonModuleResolver, "capture_source", observe_capture
    )
    with graph.local_python_import_graph_transaction():
        first = local_python_import_closure(tmp_path, (left,))
        second = local_python_import_closure(tmp_path, (right,))
    assert first.topology_digest and first == second
    assert set(first.paths) == {left, right, speculative}
    assert (
        first.source_sha256[speculative]
        == hashlib.sha256(b"this is not valid python (\n").hexdigest()
    )
    assert len(inventories) == 1
    assert len(captures) == 3 and set(captures) == {left, right, speculative}
    cache = json.loads(graph.python_source_closure_cache_path(tmp_path).read_bytes())
    assert "speculative.py" not in cache["entries"]


def test_operation_keeps_alias_and_policy_analysis_contexts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    package = tmp_path / "tools" / "pkg"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text("")
    left, right = tmp_path / "left.py", tmp_path / "right.py"
    left.write_text("import pkg.helper\n")
    right.write_text("import tools.pkg.helper\n")
    helper, child = package / "helper.py", package / "child.py"
    helper.write_text("if False:\n    from . import child\n")
    child.write_text("VALUE = 1\n")
    captures, contexts = [], []
    capture = graph.LocalPythonModuleResolver.capture_source
    analyze = graph.analyze_local_imports

    def observe_capture(resolver, path):
        captures.append(path)
        return capture(resolver, path)

    def observe_analysis(snapshot, module, policy, **kwargs):
        if snapshot.path == helper:
            contexts.append((module.name, policy.module_level_only))
        return analyze(snapshot, module, policy, **kwargs)

    monkeypatch.setattr(
        graph.LocalPythonModuleResolver, "capture_source", observe_capture
    )
    monkeypatch.setattr(graph, "analyze_local_imports", observe_analysis)
    with graph.local_python_import_graph_transaction():
        for seed in (left, right):
            lexical = local_python_import_closure(tmp_path, (seed,))
            eager = local_python_import_closure(
                tmp_path, (seed,), policy=_EAGER_SOURCE_POLICY
            )
            assert child in lexical.paths and child not in eager.paths
            assert helper in lexical.paths and helper in eager.paths
    assert captures.count(helper) == 1
    assert set(contexts) == {
        ("pkg.helper", False),
        ("tools.pkg.helper", False),
        ("pkg.helper", True),
        ("tools.pkg.helper", True),
    }


@pytest.mark.parametrize("writer_first", [False, True])
def test_operation_queries_converge_independent_writer_premises(
    tmp_path: Path, writer_first: bool
) -> None:
    clean, dirty, reader, writer, selected = (
        tmp_path / name
        for name in ("clean.py", "dirty.py", "reader.py", "writer.py", "selected.py")
    )
    clean.write_text("import reader\n")
    dirty.write_text("import writer\nimport reader\n")
    reader.write_text(
        "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import selected\n"
    )
    writer.write_text("import typing\ntyping.TYPE_CHECKING = True\n")
    selected.write_text("VALUE = 1\n")
    order = (dirty, clean) if writer_first else (clean, dirty)
    with graph.local_python_import_graph_transaction():
        results = {seed: _eager_closure(tmp_path, seed) for seed in order}
    assert results[clean] == {clean, reader}
    assert results[dirty] == {dirty, writer, reader, selected}


@pytest.mark.parametrize("change", ["bytes", "add", "remove", "rename"])
def test_fresh_and_nested_fresh_operations_recapture_content_and_members(
    tmp_path: Path, change: str
) -> None:
    seed, helper, renamed = (
        tmp_path / name for name in ("entry.py", "helper.py", "renamed.py")
    )
    seed.write_bytes(b"import helper\nimport renamed\n")
    if change != "add":
        helper.write_bytes(b"VALUE = 1\n")
    with graph.local_python_import_graph_transaction():
        original = local_python_import_closure(tmp_path, (seed,))
        assert set(original.paths) == ({seed} if change == "add" else {seed, helper})
        if change == "bytes":
            before = helper.stat()
            replacement = tmp_path / "replacement"
            replacement.write_bytes(b"VALUE = 2\n")
            os.utime(replacement, ns=(before.st_atime_ns, before.st_mtime_ns))
            replacement.replace(helper)
        elif change == "add":
            helper.write_bytes(b"VALUE = 2\n")
        elif change == "remove":
            helper.unlink()
        else:
            helper.rename(renamed)
        # The explicit transaction retains an already admitted query. A nested
        # fresh operation must not inherit its positive or negative filesystem facts.
        assert local_python_import_closure(tmp_path, (seed,)) is original
        with graph.local_python_import_graph_transaction(fresh=True):
            fresh = local_python_import_closure(tmp_path, (seed,))
        assert local_python_import_closure(tmp_path, (seed,)) is original
    newest = local_python_import_closure(tmp_path, (seed,))
    assert newest == fresh and newest != original
    expected = {seed}
    if change in {"bytes", "add"}:
        expected.add(helper)
        assert (
            newest.source_sha256[helper] == hashlib.sha256(b"VALUE = 2\n").hexdigest()
        )
    elif change == "rename":
        expected.add(renamed)
        assert (
            newest.source_sha256[renamed] == hashlib.sha256(b"VALUE = 1\n").hexdigest()
        )
    assert set(newest.paths) == expected


def test_operation_domain_separates_root_order_and_cache_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    first, second = project / "first", project / "second"
    first.mkdir(parents=True)
    second.mkdir()
    seed = first / "entry.py"
    seed.write_text("import helper\n")
    left, right = first / "helper.py", second / "helper.py"
    left.write_bytes(b"VALUE = 'left'\n")
    right.write_bytes(b"VALUE = 'right'\n")
    with graph.local_python_import_graph_transaction():
        left_result = local_python_import_closure(
            project, (seed,), search_roots=(first, second)
        )
        right_result = local_python_import_closure(
            project, (seed,), search_roots=(second, first)
        )
        assert set(left_result.paths) == {seed, left}
        assert set(right_result.paths) == {seed, right}
        assert (
            left_result.source_sha256[left]
            == hashlib.sha256(b"VALUE = 'left'\n").hexdigest()
        )
        assert (
            right_result.source_sha256[right]
            == hashlib.sha256(b"VALUE = 'right'\n").hexdigest()
        )
        monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "new-cache"))
        copied = local_python_import_closure(
            project, (seed,), search_roots=(first, second)
        )
        assert copied == left_result and copied is not left_result
        stored = json.loads(
            graph.python_source_closure_cache_path(project).read_bytes()
        )
        assert "first/helper.py" in stored["entries"]


def test_operation_manifest_contract_changes_select_a_new_domain(
    tmp_path: Path,
) -> None:
    seed, first, later = (
        tmp_path / name for name in ("entry.py", "first.py", "later.py")
    )
    seed.write_text("__import__(name)\n")
    first.write_text("VALUE = 1\n")
    later.write_text("VALUE = 2\n")
    manifest = tmp_path / graph._DYNAMIC_IMPORT_MANIFEST
    manifest.parent.mkdir(parents=True)

    def write_manifest(target, count=1):
        manifest.write_text(
            "schema_version = 1\n[[source]]\npath = 'entry.py'\n"
            f"nonliteral_calls = {count}\nmodules = ['{target}']\n"
        )

    write_manifest("first")
    with graph.local_python_import_graph_transaction():
        original = local_python_import_closure(tmp_path, (seed,))
        assert set(original.paths) == {seed, first, manifest}
        write_manifest("later")
        changed = local_python_import_closure(tmp_path, (seed,))
        assert set(changed.paths) == {seed, later, manifest}
        assert (
            changed.source_sha256[manifest]
            == hashlib.sha256(manifest.read_bytes()).hexdigest()
        )
        cache = graph.python_source_closure_cache_path(tmp_path)
        accepted = cache.read_bytes()
        write_manifest("later", count=0)
        with pytest.raises(ValueError, match="manifest drift"):
            local_python_import_closure(tmp_path, (seed,))
        assert cache.read_bytes() == accepted
    with graph.local_python_import_graph_transaction(fresh=True):
        with pytest.raises(ValueError, match="manifest drift"):
            local_python_import_closure(tmp_path, (seed,))


def test_failed_query_cannot_publish_tentative_rows_through_its_sibling(
    tmp_path: Path,
) -> None:
    good, sibling, bad, failure, shared = (
        tmp_path / name
        for name in ("good.py", "sibling.py", "bad.py", "failure.py", "shared.py")
    )
    for seed in (good, sibling):
        seed.write_text("import shared\n")
    bad.write_text("import failure\n")
    failure.write_text("__import__(unknown)\n")
    shared.write_bytes(b"VALUE = 1\n")
    with graph.local_python_import_graph_transaction():
        good_result = local_python_import_closure(tmp_path, (good,))
        assert set(good_result.paths) == {good, shared}
        cache = graph.python_source_closure_cache_path(tmp_path)
        accepted = cache.read_bytes()
        with pytest.raises(ValueError, match="non-literal"):
            local_python_import_closure(tmp_path, (bad,))
        assert cache.read_bytes() == accepted
        sibling_result = local_python_import_closure(tmp_path, (sibling,))
        assert set(sibling_result.paths) == {sibling, shared}
    rows = json.loads(cache.read_bytes())["entries"]
    assert set(rows) == {"good.py", "sibling.py", "shared.py"}


def test_new_query_validates_the_shared_negative_topology_facts(tmp_path: Path) -> None:
    from molt.cli.python_import_topology import PythonImportTopologyChangedError

    package = tmp_path / "pkg"
    package.mkdir()
    (package / "__init__.py").write_text("")
    left, right = tmp_path / "left.py", tmp_path / "right.py"
    for seed in (left, right):
        seed.write_text("from pkg import missing\n")
    with graph.local_python_import_graph_transaction():
        first = local_python_import_closure(tmp_path, (left,))
        cache = graph.python_source_closure_cache_path(tmp_path)
        accepted = cache.read_bytes()
        (package / "missing.py").write_text("VALUE = 1\n")
        with pytest.raises(PythonImportTopologyChangedError):
            local_python_import_closure(tmp_path, (right,))
        assert cache.read_bytes() == accepted
    fresh = local_python_import_closure(tmp_path, (right,))
    assert package / "missing.py" in fresh.paths
    assert package / "missing.py" not in first.paths


def test_parallel_explicit_operations_do_not_share_domain_captures(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from collections import Counter
    from threading import Barrier, get_ident

    left, right, helper = (
        tmp_path / name for name in ("left.py", "right.py", "helper.py")
    )
    for seed in (left, right):
        seed.write_text("import helper\n")
    helper.write_bytes(b"VALUE = 1\n")
    captures = []
    capture = graph.LocalPythonModuleResolver.capture_source
    gate = Barrier(2)

    def observe(resolver, path):
        if path == helper:
            captures.append(get_ident())
        return capture(resolver, path)

    def worker(_index):
        with graph.local_python_import_graph_transaction():
            first = local_python_import_closure(tmp_path, (left,))
            gate.wait(timeout=30)
            second = local_python_import_closure(tmp_path, (right,))
            return first, second

    monkeypatch.setattr(graph.LocalPythonModuleResolver, "capture_source", observe)
    with graph.local_python_import_graph_transaction():
        parent = local_python_import_closure(tmp_path, (left,))
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(worker, range(2)))
        assert local_python_import_closure(tmp_path, (left,)) is parent
    assert len(Counter(captures)) == 3 and set(Counter(captures).values()) == {1}
    for first, second in results:
        assert first == parent and first is not parent
        assert set(second.paths) == {right, helper}
        assert (
            second.source_sha256[helper] == hashlib.sha256(b"VALUE = 1\n").hexdigest()
        )


def test_operation_inventory_keeps_policy_prefix_and_project_ownership(
    tmp_path: Path,
) -> None:
    project = tmp_path / "project"
    package = project / "pkg"
    package.mkdir(parents=True)
    seed = project / "entry.py"
    seed.write_text("__package__ = unknown\nfrom .missing import value\n")
    initializer, helper = package / "__init__.py", package / "helper.py"
    initializer.write_text("")
    helper.write_text("VALUE = 1\n")
    outsider = project / "outsider.py"
    outsider.write_text("VALUE = 2\n")
    restricted = PythonImportPolicy(
        module_level_only=False,
        include_parent_packages=True,
        fail_on_nonliteral_dynamic_import=True,
        allowed_prefix="pkg",
        purpose="source_dependency",
        unknown_relative_sources="local_inventory",
    )
    other = tmp_path / "other"
    other.mkdir()
    other_seed = other / "entry.py"
    other_seed.write_text("import foreign\n")
    foreign = other / "foreign.py"
    foreign.write_text("VALUE = 3\n")
    with graph.local_python_import_graph_transaction():
        complete = local_python_import_closure(project, (seed,))
        scoped = local_python_import_closure(project, (seed,), policy=restricted)
        separate = local_python_import_closure(other, (other_seed,))
    assert set(complete.paths) == {seed, initializer, helper, outsider}
    assert set(scoped.paths) == {seed, initializer, helper}
    assert scoped.topology_digest != complete.topology_digest
    assert set(separate.paths) == {other_seed, foreign}


def test_operation_releases_raw_captures_without_retaining_analysis_trees(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import gc
    import weakref

    left, right, helper = (
        tmp_path / name for name in ("left.py", "right.py", "helper.py")
    )
    for seed in (left, right):
        seed.write_text("import helper\n")
    helper.write_text("VALUE = 1\n")
    captures = []
    capture = graph.LocalPythonModuleResolver.capture_source

    def observe(resolver, path):
        snapshot = capture(resolver, path)
        captures.append(weakref.ref(snapshot))
        return snapshot

    monkeypatch.setattr(graph.LocalPythonModuleResolver, "capture_source", observe)
    with graph.local_python_import_graph_transaction():
        receipts = [
            local_python_import_closure(tmp_path, (seed,)) for seed in (left, right)
        ]
        assert len(captures) == 3
        assert all(reference() is not None for reference in captures)
        assert all(
            not {"tree", "_decoded_text", "ast_digest"}.intersection(
                reference().__dict__
            )
            for reference in captures
        )
    gc.collect()
    assert all(reference() is None for reference in captures)
    assert set(receipts[0].paths) == {left, helper}
    assert set(receipts[1].paths) == {right, helper}


def test_new_query_does_not_hide_a_removed_explicit_seed(tmp_path: Path) -> None:
    left, right = tmp_path / "left.py", tmp_path / "right.py"
    left.write_text("VALUE = 1\n")
    right.write_text("VALUE = 2\n")
    with graph.local_python_import_graph_transaction():
        local_python_import_closure(tmp_path, (left,))
        left.unlink()
        with pytest.raises(FileNotFoundError, match="missing Python tooling source"):
            local_python_import_closure(tmp_path, (left, right))
