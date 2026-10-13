from __future__ import annotations

import ast
import inspect
from pathlib import Path

from molt.cli.models import _DiscoveredModuleGraph

import pytest

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_graph_discovery
from molt.cli import module_graph_cache
from molt.cli import module_import_scanner
from molt.compiler_analysis.python_imports import UnresolvedStaticImportError

_MODULE_GRAPH_DISCOVERY_NAMES = (
    "PLATFORM_EXCLUDED_SUBMODULES",
    "_discover_module_graph",
    "_discover_module_graph_from_paths",
    "_extend_module_graph_with_closure",
    "_extend_module_graph_with_static_import_modules",
    "_load_module_imports",
    "_load_module_import_scan",
    "_LoadedModuleImportScan",
    "_parse_static_import_modules",
    "_parse_static_import_modules_from_env",
    "_record_module_reason",
    "_record_new_module_reasons",
    "_resolve_static_import_module_paths",
)

_MODULE_GRAPH_DISCOVERY_DEFINITIONS = (
    "PLATFORM_EXCLUDED_SUBMODULES =",
    "def _discover_module_graph(",
    "def _discover_module_graph_from_paths(",
    "def _extend_module_graph_with_closure(",
    "def _extend_module_graph_with_static_import_modules(",
    "def _load_module_imports(",
    "def _load_module_import_scan(",
    "class _LoadedModuleImportScan",
    "def _parse_static_import_modules(",
    "def _parse_static_import_modules_from_env(",
    "def _record_module_reason(",
    "def _record_new_module_reasons(",
    "def _resolve_static_import_module_paths(",
)


def test_cli_module_graph_discovery_authority_is_single_home() -> None:
    for name in _MODULE_GRAPH_DISCOVERY_NAMES:
        assert hasattr(module_graph_discovery, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_GRAPH_DISCOVERY_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source


@pytest.mark.parametrize("full_scan", [False, True])
@pytest.mark.parametrize("module_name", ["plain", "collections", "email.message"])
def test_scan_mode_projects_explicit_source_depth(
    full_scan: bool, module_name: str
) -> None:
    assert module_import_scanner._module_import_scan_mode(
        module_name, full_scan=full_scan
    ) == ("full" if full_scan else "module_init")


@pytest.mark.parametrize("first_full", [False, True])
def test_graph_cache_keeps_initialization_and_full_roots_disjoint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, first_full: bool
) -> None:
    entry = tmp_path / "entry.py"
    entry.write_text("def deferred():\n    import lazy\n", encoding="utf-8")
    lazy = tmp_path / "lazy.py"
    lazy.write_text("VALUE = 1\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    monkeypatch.setattr(
        module_graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "test"
    )
    for full in (first_full, not first_full, first_full):
        discovery_result = module_graph_discovery._discover_module_graph_from_paths(
            (entry,),
            [tmp_path],
            [tmp_path],
            stdlib,
            tmp_path,
            set(),
            full_scan_roots=full,
        )
        graph = discovery_result.graph
        imports = discovery_result.explicit_imports
        assert set(graph) == ({"entry", "lazy"} if full else {"entry"})
        assert imports == ({"lazy"} if full else set())


@pytest.mark.parametrize("deferred", [False, True])
def test_initialization_seed_does_not_grant_dynamic_import_custody(
    tmp_path: Path, deferred: bool
) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    entry = package / "__init__.py"
    entry.write_text(
        "__package__ = choose_package()\n"
        + (
            "def deferred():\n    from . import lazy\n"
            if deferred
            else "from . import lazy\n"
        ),
        encoding="utf-8",
    )

    def discover(full: bool) -> _DiscoveredModuleGraph:
        return module_graph_discovery._discover_module_graph_from_paths(
            (entry,),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            None,
            set(),
            full_scan_roots=full,
        )

    for full in (False, True):
        discovered = discover(full)
        authority = discovered.scan_authority.by_module["pkg"]
        needs_runtime = full or not deferred
        assert authority.requires_runtime_package_anchor is needs_runtime
        assert discovered.graph == {"pkg": entry}
        # Finding the source owner never seals its dynamic package operand.
        if needs_runtime:
            with pytest.raises(
                UnresolvedStaticImportError, match="runtime import custody"
            ):
                module_import_scanner._collect_imports(
                    ast.parse(entry.read_text(encoding="utf-8")),
                    module_name="pkg",
                    is_package=True,
                    import_scan_mode="full" if full else "module_init",
                )


def test_core_initialization_does_not_activate_undemanded_class_methods(
    tmp_path: Path,
) -> None:
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    entry = stdlib / "collections.py"
    entry.write_text(
        "class UserDict:\n    def copy(self):\n        import copy\n",
        encoding="utf-8",
    )
    copy = stdlib / "copy.py"
    copy.write_text("VALUE = 1\n", encoding="utf-8")
    discovery_result = module_graph_discovery._discover_module_graph_from_paths(
        (entry,),
        [stdlib],
        [stdlib],
        stdlib,
        None,
        {"collections", "copy"},
        full_scan_roots=False,
    )
    graph = discovery_result.graph
    imports = discovery_result.explicit_imports
    assert set(graph) == {"collections"}
    assert imports == set()


@pytest.mark.parametrize(
    ("entry_source", "expected"),
    [
        ("import api\n", {"entry", "api"}),
        (
            "from api import Counter\nx = Counter()\nx.most_common()\n",
            {"entry", "api", "heap", "classdep"},
        ),
        (
            "import api as library\nf = library.helper\nf()\n",
            {"entry", "api", "helperdep"},
        ),
        ("import api\nf = api.helper\nf = lambda: None\nf()\n", {"entry", "api"}),
        (
            "from api import *\nhelper()\n",
            {"entry", "api", "heap", "classdep", "helperdep", "deaddep", "callbackdep"},
        ),
        ("import api\napi.factory()()\n", {"entry", "api", "callbackdep"}),
    ],
)
def test_deferred_source_demand_closes_only_live_source_owners(
    tmp_path: Path, entry_source: str, expected: set[str]
) -> None:
    (tmp_path / "entry.py").write_text(entry_source)
    (tmp_path / "api.py").write_text(
        "class Counter:\n"
        "    def most_common(self):\n        import heap\n"
        "    def sibling(self):\n        import classdep\n"
        "def helper():\n    import helperdep\n"
        "def unused():\n    import deaddep\n"
        "def factory():\n"
        "    def callback():\n        import callbackdep\n"
        "    return callback\n"
    )
    for name in ("heap", "classdep", "helperdep", "deaddep", "callbackdep"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert set(result.graph) == expected


def test_module_getattr_demands_lazy_returned_class(tmp_path: Path) -> None:
    (tmp_path / "api.py").write_text(
        "def __getattr__(name):\n    from lazy import Result\n    return Result\n"
    )
    (tmp_path / "lazy.py").write_text(
        "class Result:\n    def run(self):\n        import dependency\n"
    )
    (tmp_path / "dependency.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    for source, expected in (
        ("import api\n", {"entry", "api"}),
        ("import api\napi.Result().run()\n", {"entry", "api", "lazy", "dependency"}),
    ):
        (tmp_path / "entry.py").write_text(source)
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            stdlib,
            None,
            set(),
            full_scan_roots=True,
        )
        assert set(result.graph) == expected


@pytest.mark.parametrize(
    ("source", "required", "excluded"),
    [
        ("import unittest\n", {"unittest"}, {"unittest.async_case", "asyncio"}),
        (
            "import unittest\nunittest.IsolatedAsyncioTestCase\n",
            {"unittest.async_case", "asyncio"},
            set(),
        ),
        (
            "from collections import Counter\nCounter([1, 2]).most_common(1)\n",
            {"heapq"},
            set(),
        ),
        ("import json\njson.JSONDecoder().raw_decode('1')\n", {"operator"}, set()),
        (
            "from email.message import EmailMessage\nEmailMessage().as_string()\n",
            {"email.generator"},
            set(),
        ),
    ],
)
def test_public_stdlib_deferred_demand_graph(
    tmp_path: Path, source: str, required: set[str], excluded: set[str]
) -> None:
    from molt.cli import module_resolution, module_stdlib_policy

    entry = tmp_path / "entry.py"
    entry.write_text(source)
    stdlib = module_resolution._stdlib_root_path()
    result = module_graph_discovery._discover_module_graph(
        entry,
        [tmp_path, stdlib],
        [tmp_path],
        stdlib,
        None,
        module_stdlib_policy._stdlib_allowlist(),
    )
    assert required <= result.graph.keys()
    assert excluded.isdisjoint(result.graph)
    print(
        "HF126_GRAPH",
        source.splitlines()[-1],
        "modules",
        len(result.graph),
        "bodies",
        sum(len(owner.deferred_owners) for owner in result.scan_authority.sources),
    )


@pytest.mark.parametrize(
    ("hook", "use", "required", "excluded"),
    [
        (
            "if name == 'live':\n        import selected\n    else:\n        import alternate",
            "api.live",
            {"selected"},
            {"alternate"},
        ),
        (
            "alias = name\n    if alias == 'live':\n        import selected\n    else:\n        import alternate",
            "api.live",
            {"selected"},
            {"alternate"},
        ),
        (
            "name = 'other'\n    if name == 'live':\n        import selected\n    else:\n        import alternate",
            "api.live",
            {"alternate"},
            {"selected"},
        ),
        (
            "name = unknown()\n    if name == 'live':\n        import selected\n    else:\n        import alternate",
            "api.live",
            {"selected", "alternate"},
            set(),
        ),
        (
            "if name == 'live':\n        import selected\n    else:\n        import alternate",
            "getattr(api, unknown())",
            {"selected", "alternate"},
            set(),
        ),
        (
            "if name == 'live':\n        import selected\n    else:\n        import alternate",
            "publish(api.__getattr__)",
            {"selected", "alternate"},
            set(),
        ),
    ],
)
def test_member_context_is_proven_or_widens(
    tmp_path, monkeypatch, hook, use, required, excluded
):
    monkeypatch.setattr(
        module_graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "test"
    )
    (tmp_path / "entry.py").write_text("import api\n" + use + "\n")
    (tmp_path / "api.py").write_text("def __getattr__(name):\n    " + hook + "\n")
    for name in ("selected", "alternate"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    graphs = []
    for _ in range(2):
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            stdlib,
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        graphs.append(set(result.graph))
    assert graphs[0] == graphs[1]
    assert required <= graphs[0]
    assert excluded.isdisjoint(graphs[0])


def test_deferred_publication_does_not_define_initial_namespace(tmp_path):
    (tmp_path / "entry.py").write_text("import api\napi.install()\napi.published()\n")
    (tmp_path / "api.py").write_text(
        "def install():\n    global published\n    from provider import callback as published\n"
    )
    (tmp_path / "provider.py").write_text("def callback():\n    import dependency\n")
    (tmp_path / "dependency.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert set(result.graph) == {"entry", "api", "provider", "dependency"}


def test_context_requests_reuse_the_parsed_source_record(tmp_path, monkeypatch):
    from molt.cli.module_resolution import _ModuleResolutionCache
    from molt.compiler_analysis import python_source_keys
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    cache = _ModuleResolutionCache()
    path = tmp_path / "api.py"
    source = "def __getattr__(name):\n    if name == 'left':\n        import left\n    else:\n        import right\n"
    path.write_text(source)
    parsed = cache.parse_module_ast(path, source, filename=str(path))
    digest_calls = []
    digest = python_source_keys.python_ast_digest

    def counted(tree):
        if tree is parsed.tree:
            digest_calls.append(tree)
        return digest(tree)

    monkeypatch.setattr(python_source_keys, "python_ast_digest", counted)
    owner = python_node_source_key(parsed.tree.body[0])
    for argument, expected in (
        ("left", ("left",)),
        ("right", ("right",)),
        ("left", ("left",)),
    ):
        loaded = module_graph_discovery._load_module_import_scan(
            path,
            module_name="api",
            is_package=False,
            import_scan_mode="module_init",
            source_owner=owner,
            source_arguments=(argument,),
            resolution_cache=cache,
            project_root=None,
        )
        assert loaded.scan.imports == expected
    assert len(digest_calls) == 1
    assert len(cache.ast_cache) == 1
    assert len(cache.graph_import_scan_cache) == 2
    assert parsed.admission.deferred_nodes[owner] is parsed.tree.body[0]
    with pytest.raises(ValueError, match="lost source owner"):
        module_graph_discovery._load_module_import_scan(
            path,
            module_name="api",
            is_package=False,
            import_scan_mode="module_init",
            source_owner=(99, 0, 99, 1, "FunctionDef"),
            resolution_cache=cache,
            project_root=None,
        )


def test_cyclic_initialization_does_not_suppress_module_hook(tmp_path):
    (tmp_path / "entry.py").write_text("import api\n")
    (tmp_path / "api.py").write_text(
        "def __getattr__(name):\n    if name == 'f':\n        import dependency\n        return lambda: None\nimport cycle\ndef f():\n    pass\n"
    )
    (tmp_path / "cycle.py").write_text("import api\napi.f()\n")
    (tmp_path / "dependency.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert set(result.graph) == {"entry", "api", "cycle", "dependency"}


@pytest.mark.parametrize(
    "declaration",
    ["class Unused(Base):\n    pass\n", "class Unused(metaclass=Meta):\n    pass\n"],
)
def test_class_creation_demands_hooks_without_instantiation(tmp_path, declaration):
    (tmp_path / "entry.py").write_text("from bases import Base, Meta\n" + declaration)
    (tmp_path / "bases.py").write_text(
        "class Base:\n    def __init_subclass__(cls):\n        import dependency\nclass Meta(type):\n    def __new__(cls, name, bases, namespace):\n        import dependency\n        return super().__new__(cls, name, bases, namespace)\n"
    )
    (tmp_path / "dependency.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert set(result.graph) == {"entry", "bases", "dependency"}


@pytest.mark.parametrize(
    "publication",
    [
        "def callback():\n        import dependency",
        "from provider import callback as value\n    (callback := value)",
    ],
)
def test_deferred_namespace_publication_family(tmp_path, publication):
    (tmp_path / "entry.py").write_text("import api\napi.install()\napi.callback()\n")
    (tmp_path / "api.py").write_text(
        "def install():\n    global callback\n    " + publication + "\n"
    )
    (tmp_path / "provider.py").write_text("def callback():\n    import dependency\n")
    (tmp_path / "dependency.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert "dependency" in result.graph


@pytest.mark.parametrize(
    "source",
    [
        "def f():\n    import dependency\nimport cycle\ndef f():\n    pass\n",
        "def __getattr__(name):\n    if name == 'f':\n        import dependency\n        return lambda: None\nimport cycle\ndel __getattr__\n",
    ],
)
def test_cyclic_import_observes_prior_publications(tmp_path, source):
    (tmp_path / "entry.py").write_text("import api\n")
    (tmp_path / "api.py").write_text(source)
    (tmp_path / "cycle.py").write_text("import api\napi.f()\n")
    (tmp_path / "dependency.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert "dependency" in result.graph


@pytest.mark.parametrize(
    "use,required",
    [("import api\n", False), ("import api\napi.Alias.__value__\n", True)],
)
def test_type_alias_value_uses_the_same_deferred_owner(tmp_path, use, required):
    (tmp_path / "entry.py").write_text(use)
    (tmp_path / "api.py").write_text("type Alias = __import__('dependency').Value\n")
    (tmp_path / "dependency.py").write_text("Value = int\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert ("dependency" in result.graph) is required


def test_member_context_reaches_import_call_name(tmp_path):
    (tmp_path / "entry.py").write_text("import api\napi.live\n")
    (tmp_path / "api.py").write_text(
        "def __getattr__(name):\n    return __import__('selected' if name == 'live' else 'alternate')\n"
    )
    for name in ("selected", "alternate"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert "selected" in result.graph
    assert "alternate" not in result.graph


def test_source_change_during_body_scan_refuses_fixpoint(tmp_path, monkeypatch):
    from molt.cli.module_source import PythonSourceChangedError

    (tmp_path / "entry.py").write_text("from api import live\nlive()\n")
    api = tmp_path / "api.py"
    api.write_text("def live():\n    import first\n")
    for name in ("first", "other"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    collect = module_import_scanner._collect_imports_for_graph

    def mutate(*args, **kwargs):
        result = collect(*args, **kwargs)
        if kwargs.get("source_owner") is not None:
            api.write_text("def live():\n    import other\n")
        return result

    monkeypatch.setattr(module_import_scanner, "_collect_imports_for_graph", mutate)
    with pytest.raises(PythonSourceChangedError, match="source changed"):
        module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            stdlib,
            None,
            set(),
            full_scan_roots=True,
        )


def test_scan_authority_projects_and_merges_exact_body_contexts(tmp_path):
    from molt.cli.models import _ModuleGraphScanAuthority, _ModuleSourceScanAuthority

    owner = (1, 0, 5, 0, "FunctionDef")
    path = tmp_path / "api.py"
    left = _ModuleSourceScanAuthority(
        "api", path, "module_init", deferred_owners=frozenset({(owner, ("left",))})
    )
    right = _ModuleSourceScanAuthority(
        "api",
        path,
        "module_init",
        deferred_owners=frozenset({(owner, ("right",)), (owner, ())}),
    )
    authority = _ModuleGraphScanAuthority((left,)).merged(
        _ModuleGraphScanAuthority((right,))
    )
    assert (
        authority.by_module["api"].deferred_owners
        == left.deferred_owners | right.deferred_owners
    )
    assert authority.payload({"api": "api.py"})[0]["deferred_bodies"] == [
        {"source": list(owner), "arguments": []},
        {"source": list(owner), "arguments": ["left"]},
        {"source": list(owner), "arguments": ["right"]},
    ]


@pytest.mark.parametrize(
    "publication",
    [
        "callback += [provider.callback]",
        "for callback in [provider.callback]:\n        pass",
        "with provider.Manager() as callback:\n        pass",
        "match [provider.callback]:\n        case [callback]:\n            pass",
    ],
)
@pytest.mark.parametrize("invoke", [False, True])
def test_deferred_assignment_publication_operations(tmp_path, publication, invoke):
    (tmp_path / "entry.py").write_text(
        "import api\n" + ("api.install()\n" if invoke else "")
    )
    (tmp_path / "api.py").write_text(
        "callback = []\ndef install():\n    global callback\n    import provider\n    "
        + publication
        + "\n"
    )
    (tmp_path / "provider.py").write_text(
        "def callback():\n    import dependency\n"
        "class Manager:\n    def __enter__(self):\n        return callback\n"
        "    def __exit__(self, *args):\n        pass\n"
    )
    (tmp_path / "dependency.py").write_text("VALUE = 1\n")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        full_scan_roots=True,
    )
    assert ("dependency" in result.graph) is invoke
