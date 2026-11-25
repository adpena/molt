from __future__ import annotations

from molt.compiler_analysis.python_import_scan import SourceImportPurpose

from molt.compiler_analysis.native_support_slice import ModuleBodySelection

from molt.compiler_analysis import python_import_scan as _source_import_scan

import ast
import inspect
from dataclasses import replace
from pathlib import Path

from molt.cli.module_graph_discovery import _DiscoveredModuleGraph
from molt.cli.models import ImportScanAdmission, ModuleReason

import pytest
from tests.process_guard_common import run_guarded_test_process

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_graph_discovery
from molt.cli import module_graph_cache
from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
from molt.compiler_analysis.python_source_keys import python_node_source_key

_MODULE_GRAPH_DISCOVERY_NAMES = (
    "PLATFORM_EXCLUDED_SUBMODULES",
    "_discover_module_graph",
    "_discover_module_graph_from_paths",
    "_extend_module_graph_with_closure",
    "_extend_module_graph_with_include_modules",
    "_load_module_imports",
    "_load_module_import_scan",
    "_LoadedModuleImportScan",
    "_record_module_reason",
    "_record_new_module_reasons",
    "_resolve_include_module_paths",
)

_MODULE_GRAPH_DISCOVERY_DEFINITIONS = (
    "PLATFORM_EXCLUDED_SUBMODULES =",
    "def _discover_module_graph(",
    "def _discover_module_graph_from_paths(",
    "def _extend_module_graph_with_closure(",
    "def _extend_module_graph_with_include_modules(",
    "def _load_module_imports(",
    "def _load_module_import_scan(",
    "class _LoadedModuleImportScan",
    "def _record_module_reason(",
    "def _record_new_module_reasons(",
    "def _resolve_include_module_paths(",
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
    assert _source_import_scan._module_import_scan_mode(
        module_name, full_scan=full_scan
    ) == ("full" if full_scan else "module_init")


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize("first_full", [False, True])
def test_graph_cache_keeps_initialization_and_full_roots_disjoint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, first_full: bool
) -> None:
    entry = tmp_path / "entry.py"
    entry.write_text("def deferred():\n    import lazy\ndeferred()\n", encoding="utf-8")
    lazy = tmp_path / "lazy.py"
    lazy.write_text("VALUE = 1\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    monkeypatch.setattr(
        module_graph_cache, "_import_scan_tooling_fingerprint", lambda: "test"
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
        assert set(graph) == {"entry", "lazy"}
        assert imports == {"lazy"}
        assert discovery_result.scan_authority.by_module["entry"].mode == (
            "full" if full else "module_init"
        )


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
                _source_import_scan._collect_imports(
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
            {"entry", "api", "heap"},
        ),
        (
            "import api as library\nf = library.helper\nf()\n",
            {"entry", "api", "helperdep"},
        ),
        ("import api\nf = api.helper\nf = lambda: None\nf()\n", {"entry", "api"}),
        ("from api import *\nhelper()\n", {"entry", "api", "helperdep"}),
        ("import api\napi.factory()()\n", {"entry", "api", "callbackdep"}),
    ],
)
def test_deferred_source_demand_closes_only_live_source_owners(
    tmp_path: Path, entry_source: str, expected: set[str]
) -> None:
    import json
    import sys

    (tmp_path / "entry.py").write_text(entry_source, encoding="utf-8")
    (tmp_path / "api.py").write_text(
        "class Counter:\n"
        "    def most_common(self):\n        import heap\n"
        "    def sibling(self):\n        import classdep\n"
        "def helper():\n    import helperdep\n"
        "def unused():\n    import deaddep\n"
        "def factory():\n"
        "    def callback():\n        import callbackdep\n"
        "    return callback\n",
        encoding="utf-8",
    )
    for name in ("heap", "classdep", "helperdep", "deaddep", "callbackdep"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")
    observed_modules = {
        "entry",
        "api",
        "heap",
        "classdep",
        "helperdep",
        "deaddep",
        "callbackdep",
    }
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\n"
            "sys.path.insert(0,sys.argv[1])\n"
            "import entry\n"
            "print(json.dumps(sorted(set(sys.modules)&set(json.loads(sys.argv[2])))))\n",
            str(tmp_path),
            json.dumps(sorted(observed_modules)),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert set(json.loads(executed.stdout)) == expected
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


@pytest.mark.parametrize(
    ("entry_source", "expected"),
    [
        ("import api\n", {"entry", "api", "impl"}),
        ("import api\napi.helper()\n", {"entry", "api", "impl", "helperdep"}),
        ("from api import helper\nhelper()\n", {"entry", "api", "impl", "helperdep"}),
        (
            "import chain\nchain.helper()\n",
            {"entry", "chain", "api", "impl", "helperdep"},
        ),
        ("import runner\nrunner.run()\n", {"entry", "runner", "impl", "helperdep"}),
        (
            "import api\nprint(api)\n",
            {"entry", "api", "impl", "helperdep", "deaddep"},
        ),
        ("import cyc_a\ncyc_a.b_helper()\n", {"entry", "cyc_a", "cyc_b", "bdep"}),
    ],
)
def test_star_import_selects_star_bound_names_on_demand(
    tmp_path: Path, entry_source: str, expected: set[str]
) -> None:
    # CPython copies the exported names at the star import. A read selects one
    # name; only an escaped importing namespace exposes the whole source.
    sources = {
        "impl": "def helper():\n    import helperdep\ndef unused():\n    import deaddep\n",
        "api": "from impl import *\n",
        "chain": "from api import *\n",
        "runner": "from impl import *\ndef run():\n    helper()\n",
        "cyc_a": "from cyc_b import *\ndef a_helper():\n    import adep\n",
        "cyc_b": "from cyc_a import *\ndef b_helper():\n    import bdep\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    for name in ("helperdep", "deaddep", "adep", "bdep"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tmp_path / "entry.py").write_text(entry_source, encoding="utf-8")
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


@pytest.mark.parametrize(
    ("entry_source", "expected"),
    [
        ("import importlib\nimportlib.import_module('plain')\n", {"entry", "plain"}),
        (
            "import importlib\nimportlib.import_module('plain').helper()\n",
            {"entry", "plain", "helperdep"},
        ),
        ("__import__('plain').helper()\n", {"entry", "plain", "helperdep"}),
        (
            "import importlib\nprint(importlib.import_module('plain'))\n",
            {"entry", "plain", "helperdep", "deaddep"},
        ),
    ],
)
def test_import_call_result_selects_members_like_an_import_alias(
    tmp_path: Path, entry_source: str, expected: set[str]
) -> None:
    # An import call returns a module. Its member reads select that module's
    # members; passing it to unknown code exposes the module whole.
    (tmp_path / "plain.py").write_text(
        "def helper():\n    import helperdep\ndef unused():\n    import deaddep\n",
        encoding="utf-8",
    )
    for name in ("helperdep", "deaddep"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tmp_path / "entry.py").write_text(entry_source, encoding="utf-8")
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


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    ("hook", "entry_source", "expected"),
    [
        ("class", "import api\n", {"entry", "api"}),
        ("class", "import api\napi.Result\n", {"entry", "api", "lazy"}),
        ("class", "import api\napi.Result()\n", {"entry", "api", "lazy"}),
        (
            "class",
            "import api\napi.Result().run()\n",
            {"entry", "api", "lazy", "dependency"},
        ),
        (
            "class",
            "from api import Result as Selected\nSelected().run()\n",
            {"entry", "api", "lazy", "dependency"},
        ),
        (
            "alias",
            "import api\napi.Result().run()\n",
            {"entry", "api", "lazy", "dependency"},
        ),
        (
            "imported",
            "import api\napi.Result().run()\n",
            {"entry", "api", "helper", "lazy", "dependency"},
        ),
        (
            "instance",
            "import api\napi.result.run()\n",
            {"entry", "api", "lazy", "dependency"},
        ),
        (
            "callback",
            "import api\napi.callback()\n",
            {"entry", "api", "lazy", "dependency"},
        ),
        (
            "generator",
            "import api\napi.items()\n",
            {"entry", "api", "lazy"},
        ),
        (
            "generator",
            "import api\nnext(api.items())\n",
            {"entry", "api", "lazy", "dependency"},
        ),
    ],
)
def test_module_getattr_demands_lazy_returned_class(
    tmp_path: Path, monkeypatch, hook: str, entry_source: str, expected: set[str]
) -> None:
    import json
    import sys
    from molt.cli import module_resolution

    direct = (
        "def resolve(name):\n"
        "    if name != 'Result':\n        raise AttributeError(name)\n"
        "    from lazy import Result\n    return Result\n"
    )
    if hook == "alias":
        api = direct + "__getattr__ = resolve\n"
    elif hook == "imported":
        api = "from helper import resolve as __getattr__\n"
    else:
        selected, returned = {
            "class": ("Result", "Result"),
            "instance": ("result", "Result()"),
            "callback": ("callback", "callback"),
            "generator": ("items", "items"),
        }[hook]
        imported = "Result" if hook == "instance" else selected
        api = (
            "def __getattr__(name):\n"
            f"    if name != {selected!r}:\n        raise AttributeError(name)\n"
            f"    from lazy import {imported}\n    return {returned}\n"
        )
    (tmp_path / "api.py").write_text(api, encoding="utf-8")
    (tmp_path / "helper.py").write_text(direct, encoding="utf-8")
    (tmp_path / "lazy.py").write_text(
        "class Result:\n"
        "    def run(self):\n        import dependency\n"
        "    def unused(self):\n        import unused_dependency\n"
        "def callback():\n    import dependency\n"
        "def items():\n    import dependency\n    yield 1\n",
        encoding="utf-8",
    )
    (tmp_path / "dependency.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tmp_path / "unused_dependency.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tmp_path / "entry.py").write_text(entry_source, encoding="utf-8")
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\n"
            "sys.path.insert(0,sys.argv[1])\n"
            "import entry\n"
            "print(json.dumps(sorted(set(sys.modules)&set(json.loads(sys.argv[2])))))\n",
            str(tmp_path),
            json.dumps(
                ["entry", "api", "helper", "lazy", "dependency", "unused_dependency"]
            ),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert set(json.loads(executed.stdout)) == expected
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "lazy-module-return",
    )
    parse = module_resolution._parse_source_for_target
    parses = []

    def observe_parse(*args, **kwargs):
        parses.append(kwargs["filename"])
        return parse(*args, **kwargs)

    monkeypatch.setattr(module_resolution, "_parse_source_for_target", observe_parse)
    for warm in (False, True):
        parses.clear()
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            stdlib,
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        assert set(result.graph) == expected
        if warm:
            assert not parses
        else:
            assert len(parses) == len(set(parses))


@pytest.mark.long_pole
@pytest.mark.parametrize(
    ("source", "required", "excluded"),
    [
        # Only the two unittest closures exceed the presubmit median (HF-938).
        pytest.param(
            "import unittest\n",
            {"unittest"},
            {"unittest.async_case", "asyncio"},
            marks=pytest.mark.slow,
        ),
        pytest.param(
            "import unittest\nunittest.IsolatedAsyncioTestCase\n",
            {"unittest.async_case", "asyncio"},
            set(),
            marks=pytest.mark.slow,
        ),
        (
            "from collections import Counter\nCounter([1, 2]).most_common(1)\n",
            {"heapq"},
            set(),
        ),
        ("import json\njson.JSONDecoder().raw_decode('1')\n", {"operator"}, set()),
        # Bootstrap modules expose no namespace: types binds its runtime
        # payload by name, the frozen importer passes no globals, and the
        # ByteString hooks select one warnings member.
        (
            "import types\nimport _collections_abc\nimport importlib._bootstrap_external\n",
            set(),
            {"functools", "collections", "re", "reprlib", "math", "itertools"},
        ),
        ("import types\ntypes.coroutine(lambda: None)\n", {"functools"}, set()),
        (
            "from dataclasses import dataclass\n"
            "@dataclass\nclass Point:\n    x: int\n    y: int = 2\n"
            "print(Point(1).x)\n",
            {"dataclasses"},
            {
                "_collections",
                "_compatibility_errors",
                "_weakref",
                "atexit",
                "collections",
                "collections.abc",
                "copy",
                "functools",
                "gc",
                "heapq",
                "math",
                "re",
                "weakref",
            },
        ),
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
    entry.write_text(source, encoding="utf-8")
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


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    ("use", "required"),
    [
        ("api.__dict__", set()),
        ("getattr(api, '__dict__')", set()),
        ("api.__dict__['callback']()", {"live_leaf"}),
        ("api.__dict__.get('callback')()", {"live_leaf"}),
        ("dict.get(api.__dict__, 'callback')()", {"live_leaf"}),
        ("api.__dict__.copy()['callback']()", {"live_leaf"}),
        ("read(api)['callback']()", {"live_leaf"}),
        ("read(api)", set()),
        # ``molt_namespace_entry_get`` is ``api.__dict__.get(name, missing)``:
        # a value read and then called runs; a read alone runs nothing, and
        # neither escapes ``api`` to the ``__getattr__`` hook.
        ("entry_get(api, 'callback', None)()", {"live_leaf"}),
        ("entry_get(api, 'callback', None)", set()),
        ("entry_get(api, 'absent', len)('')", set()),
    ],
)
def test_namespace_acquisition_selects_values_without_calling_the_hook(
    tmp_path, monkeypatch, use, required
):
    import json
    import sys

    sources = {
        "entry": "import api\nfrom _intrinsics import require_intrinsic\n"
        "read = require_intrinsic('molt_dataclasses_init_globals')\n"
        "entry_get = require_intrinsic('molt_namespace_entry_get')\n" + use + "\n",
        "api": "def callback():\n    import live_leaf\n"
        "def unused():\n    import dead_leaf\n"
        "def __getattr__(name):\n    import hook_leaf\n    raise AttributeError(name)\n",
        "live_leaf": "pass\n",
        "dead_leaf": "pass\n",
        "hook_leaf": "pass\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys,types\n"
            "intrinsics=types.ModuleType('_intrinsics')\n"
            "intrinsics.require_intrinsic=lambda name: ("
            "(lambda owner, key, missing: owner.__dict__.get(key, missing)) "
            "if name == 'molt_namespace_entry_get' "
            "else (lambda owner: getattr(owner, '__dict__')))\n"
            "sys.modules['_intrinsics']=intrinsics\n"
            "sys.path.insert(0,sys.argv[1])\nimport entry\n"
            "print(json.dumps(sorted(set(sys.modules)&{'live_leaf','dead_leaf','hook_leaf'})))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert set(json.loads(observed.stdout)) == required
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "namespace-view",
    )
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
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
        assert required <= result.graph.keys()
        assert ({"live_leaf", "dead_leaf", "hook_leaf"} - required).isdisjoint(
            result.graph
        )


@pytest.mark.usefixtures("isolated_molt_cache")
def test_namespace_view_survives_a_partial_module_and_late_publication(
    tmp_path, monkeypatch
):
    import sys

    sources = {
        "entry": "import api\nimport cycle\ncycle.later()\n",
        "api": "def __getattr__(name):\n    import hook_leaf\n    raise AttributeError(name)\n"
        "import cycle\ndef callback():\n    import live_leaf\n",
        "cycle": "import api\nnamespace = api.__dict__\ndef later():\n    namespace['callback']()\n",
        "live_leaf": "pass\n",
        "hook_leaf": "pass\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys; sys.path.insert(0,sys.argv[1]); import entry; assert 'live_leaf' in sys.modules; assert 'hook_leaf' not in sys.modules",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert observed.returncode == 0
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "namespace-cycle",
    )
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        tmp_path,
        set(),
        full_scan_roots=True,
    )
    assert "live_leaf" in result.graph
    assert "hook_leaf" not in result.graph


@pytest.mark.usefixtures("isolated_molt_cache")
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
        module_graph_cache, "_import_scan_tooling_fingerprint", lambda: "test"
    )
    (tmp_path / "entry.py").write_text("import api\n" + use + "\n", encoding="utf-8")
    (tmp_path / "api.py").write_text(
        "def __getattr__(name):\n    " + hook + "\n", encoding="utf-8"
    )
    for name in ("selected", "alternate"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")
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
    (tmp_path / "entry.py").write_text(
        "import api\napi.install()\napi.published()\n", encoding="utf-8"
    )
    (tmp_path / "api.py").write_text(
        "def install():\n    global published\n    from provider import callback as published\n",
        encoding="utf-8",
    )
    (tmp_path / "provider.py").write_text(
        "def callback():\n    import dependency\n", encoding="utf-8"
    )
    (tmp_path / "dependency.py").write_text("VALUE = 1\n", encoding="utf-8")
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
    path.write_text(source, encoding="utf-8")
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
            purpose=SourceImportPurpose.EXECUTION,
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
            purpose=SourceImportPurpose.EXECUTION,
            module_name="api",
            is_package=False,
            import_scan_mode="module_init",
            source_owner=(99, 0, 99, 1, "FunctionDef"),
            resolution_cache=cache,
            project_root=None,
        )


def test_cyclic_initialization_does_not_suppress_module_hook(tmp_path):
    (tmp_path / "entry.py").write_text("import api\n", encoding="utf-8")
    (tmp_path / "api.py").write_text(
        "def __getattr__(name):\n    if name == 'f':\n        import dependency\n        return lambda: None\nimport cycle\ndef f():\n    pass\n",
        encoding="utf-8",
    )
    (tmp_path / "cycle.py").write_text("import api\napi.f()\n", encoding="utf-8")
    (tmp_path / "dependency.py").write_text("VALUE = 1\n", encoding="utf-8")
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
    (tmp_path / "entry.py").write_text(
        "from bases import Base, Meta\n" + declaration, encoding="utf-8"
    )
    (tmp_path / "bases.py").write_text(
        "class Base:\n    def __init_subclass__(cls):\n        import dependency\nclass Meta(type):\n    def __new__(cls, name, bases, namespace):\n        import dependency\n        return super().__new__(cls, name, bases, namespace)\n",
        encoding="utf-8",
    )
    (tmp_path / "dependency.py").write_text("VALUE = 1\n", encoding="utf-8")
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
    ("body", "invocation", "hook"),
    [
        (
            "def __eq__(self, other): import reached; return True",
            "Value() == Value()",
            "__eq__",
        ),
        ("def __hash__(self): import reached; return 42", "hash(Value())", "__hash__"),
        (
            "def __iter__(self): import reached; return iter(())",
            "list(Value())",
            "__iter__",
        ),
        (
            "def __enter__(self): return self\n"
            "    def __exit__(self, *args): import reached",
            "with Value(): pass",
            "__exit__",
        ),
        ("def __del__(self): import reached", "value = Value()\ndel value", "__del__"),
        (
            "def __init_subclass__(cls): import reached",
            "class Child(Value): pass",
            "__init_subclass__",
        ),
        (
            "def __set_name__(self, owner, name): import reached",
            "class Owner: item = Value()",
            "__set_name__",
        ),
        (
            "def __class_getitem__(cls, item): import reached",
            "Value[int]",
            "__class_getitem__",
        ),
        (
            "def __class_getitem__(cls, item): import reached",
            "class Child(Value): pass\nChild[int]",
            "__class_getitem__",
        ),
        (
            "def __get__(self, instance, owner): import reached",
            "class Owner: item = Value()\nOwner().item",
            "__get__",
        ),
        (
            "def __set__(self, instance, value): import reached",
            "class Owner: item = Value()\nOwner().item = 1",
            "__set__",
        ),
        (
            "def decorate(fn):\n"
            "        def wrapper(*args):\n"
            "            import reached\n"
            "            return fn(*args)\n"
            "        return wrapper\n"
            "    @decorate\n"
            "    def __eq__(self, other): return True",
            "Value() == Value()",
            "wrapper",
        ),
        (
            "def key(self, item): import reached; return item",
            "sorted([2, 1], key=Value().key)",
            "key",
        ),
    ],
)
def test_implicit_source_body_custody(
    tmp_path: Path, body: str, invocation: str, hook: str
) -> None:
    import sys

    sources = {
        "entry": "from provider import Value\n" + invocation + "\n",
        "provider": "class Value:\n    "
        + body
        + "\n\ndef unused(): import untouched\n",
        "reached": "pass\n",
        "untouched": "pass\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys; sys.path.insert(0, sys.argv[1]); import entry; "
            "assert 'reached' in sys.modules and 'untouched' not in sys.modules",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
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
    assert "reached" in result.graph
    assert "untouched" not in result.graph
    owners = {
        owner
        for owner, _ in result.scan_authority.by_module["provider"].deferred_owners
    }
    provider = ast.parse(sources["provider"])
    hook_node = next(
        node
        for node in ast.walk(provider)
        if isinstance(node, ast.FunctionDef) and node.name == hook
    )
    assert python_node_source_key(hook_node) in owners


def test_callable_reads_require_bodies_without_executing_them(tmp_path: Path) -> None:
    source = (
        "def selected(): import missing\n"
        "def member(): import missing\n"
        "def reflected(): import missing\n"
        "def local_read(): import missing\n"
        "def unread(): import missing\n"
        "alias = local_read\n"
    )
    (tmp_path / "provider.py").write_text(source, encoding="utf-8")
    entry = tmp_path / "entry.py"
    entry.write_text(
        "import provider\n"
        "from provider import selected\n"
        "value = provider.member\n"
        "getattr(provider, 'reflected')\n",
        encoding="utf-8",
    )
    (tmp_path / "missing.py").write_text("raise AssertionError\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (entry,), [tmp_path], [tmp_path], stdlib, None, set(), full_scan_roots=True
    )
    provider = result.scan_authority.by_module["provider"]
    declarations = {
        python_node_source_key(node): node.name
        for node in ast.parse(source).body
        if isinstance(node, ast.FunctionDef)
    }
    assert {declarations[key] for key in provider.read_callables} == {
        "selected",
        "member",
        "reflected",
        "local_read",
    }
    assert "missing" not in result.graph


@pytest.mark.parametrize(
    "publication",
    [
        "def callback():\n        import dependency",
        "from provider import callback as value\n    (callback := value)",
    ],
)
def test_deferred_namespace_publication_family(tmp_path, publication):
    (tmp_path / "entry.py").write_text(
        "import api\napi.install()\napi.callback()\n", encoding="utf-8"
    )
    (tmp_path / "api.py").write_text(
        "def install():\n    global callback\n    " + publication + "\n",
        encoding="utf-8",
    )
    (tmp_path / "provider.py").write_text(
        "def callback():\n    import dependency\n", encoding="utf-8"
    )
    (tmp_path / "dependency.py").write_text("VALUE = 1\n", encoding="utf-8")
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
    (tmp_path / "entry.py").write_text("import api\n", encoding="utf-8")
    (tmp_path / "api.py").write_text(source, encoding="utf-8")
    (tmp_path / "cycle.py").write_text("import api\napi.f()\n", encoding="utf-8")
    (tmp_path / "dependency.py").write_text("VALUE = 1\n", encoding="utf-8")
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
    (tmp_path / "entry.py").write_text(use, encoding="utf-8")
    (tmp_path / "api.py").write_text(
        "type Alias = __import__('dependency').Value\n", encoding="utf-8"
    )
    (tmp_path / "dependency.py").write_text("Value = int\n", encoding="utf-8")
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
    (tmp_path / "entry.py").write_text("import api\napi.live\n", encoding="utf-8")
    (tmp_path / "api.py").write_text(
        "def __getattr__(name):\n    return __import__('selected' if name == 'live' else 'alternate')\n",
        encoding="utf-8",
    )
    for name in ("selected", "alternate"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")
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

    (tmp_path / "entry.py").write_text(
        "from api import live\nlive()\n", encoding="utf-8"
    )
    api = tmp_path / "api.py"
    api.write_text("def live():\n    import first\n", encoding="utf-8")
    for name in ("first", "other"):
        (tmp_path / f"{name}.py").write_text("VALUE = 1\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    collect = _source_import_scan._collect_imports_for_graph

    def mutate(*args, **kwargs):
        result = collect(*args, **kwargs)
        if kwargs.get("source_owner") is not None:
            api.write_text("def live():\n    import other\n", encoding="utf-8")
        return result

    monkeypatch.setattr(_source_import_scan, "_collect_imports_for_graph", mutate)
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
        "api",
        path,
        "module_init",
        body_selection=ModuleBodySelection("complete_source"),
        deferred_owners=frozenset({(owner, ("left",))}),
    )
    right = _ModuleSourceScanAuthority(
        "api",
        path,
        "module_init",
        body_selection=ModuleBodySelection("complete_source"),
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
        "import api\n" + ("api.install()\n" if invoke else ""), encoding="utf-8"
    )
    (tmp_path / "api.py").write_text(
        "callback = []\ndef install():\n    global callback\n    import provider\n    "
        + publication
        + "\n",
        encoding="utf-8",
    )
    (tmp_path / "provider.py").write_text(
        "def callback():\n    import dependency\n"
        "class Manager:\n    def __enter__(self):\n        return callback\n"
        "    def __exit__(self, *args):\n        pass\n",
        encoding="utf-8",
    )
    (tmp_path / "dependency.py").write_text("VALUE = 1\n", encoding="utf-8")
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


@pytest.mark.parametrize("consume", [False, True])
def test_generator_source_demand_is_resumed_not_its_creation(tmp_path, consume):
    """Real import side effects distinguish factory creation from iteration."""
    import sys

    package = "_molt_generator_demand_oracle"
    leaf = tmp_path / (package + ".py")
    leaf.write_text("VALUE = 17\n", encoding="utf-8")
    source = f"items = (__import__({package!r}) for item in (1,))\n" + (
        "list(items)\n" if consume else ""
    )
    entry = tmp_path / "entry.py"
    entry.write_text(source, encoding="utf-8")
    old = sys.modules.pop(package, None)
    sys.path.insert(0, str(tmp_path))
    try:
        exec(compile(source, str(entry), "exec", dont_inherit=True), {})
        assert (package in sys.modules) is consume
    finally:
        sys.path.remove(str(tmp_path))
        sys.modules.pop(package, None)
        if old is not None:
            sys.modules[package] = old
    result = module_graph_discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=False,
    )
    assert (package in result.graph) is consume
    owners = result.scan_authority.by_module["entry"].deferred_owners
    assert any(key[-1] == "GeneratorExp" for key, _args in owners) is consume


def test_nested_writer_revisions_hold_receivers_until_the_outer_exit(
    tmp_path: Path,
) -> None:
    from molt.cli.module_resolution import _ModuleResolutionCache
    from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    discovery = module_graph_discovery
    state = discovery._ModuleGraphDiscovery(
        _ModuleResolutionCache(), _DEFAULT_TARGET_PYTHON_VERSION
    )
    # a ↔ b contains the b ↔ c cycle after its head a is removed. The
    # receiver d follows both exits. e has no dependency on this component.
    for writer, receivers in (
        ("a", ("b",)),
        ("b", ("a", "c")),
        ("c", ("b", "d")),
        ("d", ()),
        ("e", ()),
    ):
        path = state.graph[writer] = tmp_path / f"{writer}.py"
        state.modules_by_source[path] = {writer}
        state.active_owners[writer] = {None}
        state.namespace_writes[writer] = {
            PythonNamespaceWrite(receiver, "value", "attribute", 1)
            for receiver in receivers
        }
        state.scan_revisions.add(discovery._SourceScanJob(writer, None))

    for expected in ({"c", "e"}, {"b"}, {"c"}, {"a"}, {"d"}):
        ready = discovery._ready_namespace_revisions(state)
        assert {job.module_name for job in ready} == expected
        state.scan_revisions.difference_update(ready)
        if expected == {"b"}:
            # Revising the inner head can make its child dirty again.
            state.scan_revisions.add(discovery._SourceScanJob("c", None))
    assert not state.scan_revisions


@pytest.mark.parametrize("active", [False, True])
def test_writer_with_unknown_receiver_orders_revisions(
    tmp_path: Path, active: bool
) -> None:
    from molt.cli.module_resolution import _ModuleResolutionCache
    from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    discovery = module_graph_discovery
    state = discovery._ModuleGraphDiscovery(
        _ModuleResolutionCache(), _DEFAULT_TARGET_PYTHON_VERSION
    )
    for module in ("first", "second", "writer"):
        state.graph[module] = tmp_path / f"{module}.py"
        state.scan_revisions.add(discovery._SourceScanJob(module, None))
    state.namespace_writes["writer"] = {
        PythonNamespaceWrite(None, "member", "namespace", 1)
    }
    if active:
        state.active_owners["writer"] = {None}

    ready = discovery._ready_namespace_revisions(state)
    if active:
        assert {job.module_name for job in ready} == {"writer"}
        state.scan_revisions.difference_update(ready)
        assert {
            job.module_name for job in discovery._ready_namespace_revisions(state)
        } == {
            "first",
            "second",
        }
    else:
        assert ready == state.scan_revisions


def test_opaque_activation_revision_follows_the_escaping_consumer(
    tmp_path: Path,
) -> None:
    from molt.cli.module_resolution import _ModuleResolutionCache
    from molt.compiler_analysis.python_source_demands import (
        PythonSourceDemands,
        SourceCandidate,
    )
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    discovery = module_graph_discovery
    state = discovery._ModuleGraphDiscovery(
        _ModuleResolutionCache(), _DEFAULT_TARGET_PYTHON_VERSION
    )
    state.demands = PythonSourceDemands(
        admit_imports=lambda *_: None,
        queue_body=lambda *_: None,
        require_runtime_import_protocol=lambda *_: None,
    )
    for module in ("consumer", "provider", "unrelated"):
        state.graph[module] = tmp_path / f"{module}.py"
        state.scan_revisions.add(discovery._SourceScanJob(module, None))
    state.demands.activate(
        SourceCandidate("provider", (1, 0, 2, 8, "FunctionDef")),
        "escape",
        consumer="consumer",
    )

    first = discovery._ready_namespace_revisions(state)
    assert {job.module_name for job in first} == {"consumer", "unrelated"}
    state.scan_revisions.difference_update(first)
    assert discovery._ready_namespace_revisions(state) == {
        discovery._SourceScanJob("provider", None)
    }


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize("write_kind", ["named", "computed", "canonical-member"])
def test_namespace_writer_delta_reprojects_only_affected_reached_contexts(
    tmp_path, monkeypatch, write_kind
):
    import json
    import sys
    import time
    from molt.cli import module_resolution

    if write_kind == "canonical-member":
        sources = {
            "entry": "import api\napi.run()\n",
            "api": "import typing\nfrom helper import trigger\ndef run():\n    trigger()\n    if typing.TYPE_CHECKING:\n        import late_leaf\n",
            "helper": "def trigger():\n    import writer\n",
            "writer": "import typing\ntyping.TYPE_CHECKING = True\n",
            "typing": "TYPE_CHECKING = False\n",
            "late_leaf": "VALUE = 1\n",
        }
        required = {"api", "helper", "writer", "typing", "late_leaf"}
    else:
        assignment = (
            "api.callback = callback"
            if write_kind == "named"
            else "api.__dict__[choose()] = callback"
        )
        sources = {
            "entry": "import api\napi.run()\n",
            "api": "from original import callback\ndef run():\n    import writer\n    callback()\n",
            "original": "def callback():\n    import original_leaf\n",
            "writer": "import api\nfrom replacement import callback\ndef choose(): return 'callback'\n"
            + assignment
            + "\n",
            "replacement": "def callback():\n    import late_leaf\n",
            "original_leaf": "VALUE = 1\n",
            "late_leaf": "VALUE = 1\n",
        }
        required = {"api", "original", "writer", "replacement", "late_leaf"}
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    (tmp_path / "never_reached.py").write_text(
        "raise AssertionError\n", encoding="utf-8"
    )
    script = "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\nprint(json.dumps(sorted(set(sys.modules)&set(json.loads(sys.argv[2])))))\n"
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            script,
            str(tmp_path),
            json.dumps(sorted(sources)),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert required <= set(json.loads(executed.stdout))
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "namespace-delta",
    )
    load = module_graph_discovery._load_module_import_scan
    parse = module_resolution._parse_source_for_target
    loads = []
    parses = []

    def observe_load(path, **kwargs):
        loads.append(
            (
                kwargs["module_name"],
                kwargs.get("source_owner"),
                kwargs.get("namespace_premise"),
            )
        )
        return load(path, **kwargs)

    def observe_parse(*args, **kwargs):
        parses.append(kwargs["filename"])
        return parse(*args, **kwargs)

    monkeypatch.setattr(
        module_graph_discovery, "_load_module_import_scan", observe_load
    )
    monkeypatch.setattr(module_resolution, "_parse_source_for_target", observe_parse)
    for warm in (False, True):
        loads.clear()
        parses.clear()
        started = time.perf_counter()
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            stdlib,
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        assert required <= result.graph.keys()
        assert "never_reached" not in result.graph
        assert not any("never_reached" in path for path in parses)
        api_premises = {
            premise
            for module, owner, premise in loads
            if module == "api" and owner is None
        }
        # ``api``'s module-level scan reads the written ``callback`` binding,
        # so it reprojects; it never reads ``typing.TYPE_CHECKING`` (only
        # ``run`` does), so a cold run's early cutoff keeps its one scan. A
        # warm run, whose first scans came from records, reads the published
        # alias for the second view (HF-926).
        assert len(
            {replace(p, opaque_activation_sources=()) for p in api_premises}
        ) == (1 if write_kind == "canonical-member" and not warm else 2)
        assert len(api_premises) <= (3 if write_kind == "computed" else 2)
        body_premises = {
            premise
            for module, owner, premise in loads
            if module == "api" and owner is not None
        }
        # Namespace-write deltas still require exactly two projections. The
        # fifth premise field independently records newly exposed code owners.
        assert (
            len({replace(p, opaque_activation_sources=()) for p in body_premises}) == 2
        )
        assert len(body_premises) <= (3 if write_kind == "computed" else 2)
        final = result.scan_authority.by_module["api"].namespace_premise
        assert final is not None
        if write_kind == "named":
            assert final.foreign_writes == ("callback",)
        elif write_kind == "computed":
            assert final.foreign_writes is None
            from molt.compiler_analysis.python_source_keys import python_node_source_key

            run = ast.parse(sources["api"]).body[1]
            assert final.opaque_activation_sources == (python_node_source_key(run),)
        else:
            assert final.member_writes
        if write_kind != "canonical-member":
            assert (
                sum(module == "entry" and owner is None for module, owner, _ in loads)
                == 1
            )
        if warm:
            assert not parses
        else:
            assert len(parses) == len(set(parses))
        print(
            "HF226_NAMESPACE_DELTA",
            write_kind,
            "warm",
            warm,
            "loads",
            len(loads),
            "parses",
            len(parses),
            "seconds",
            time.perf_counter() - started,
        )


def test_package_child_bindings_bound_callback_namespace_without_scanning_children(
    tmp_path,
):
    import json
    import sys

    package = tmp_path / "pkg"
    package.mkdir()
    (tmp_path / "entry.py").write_text("import pkg\npkg.run()\n", encoding="utf-8")
    (package / "__init__.py").write_text(
        "marker = False\ndef run():\n    from helper import trigger\n    trigger()\n    if marker:\n        import late_leaf\n",
        encoding="utf-8",
    )
    (package / "marker.py").write_text("VALUE = 1\n", encoding="utf-8")
    (package / "unused.py").write_text("raise AssertionError\n", encoding="utf-8")
    (tmp_path / "helper.py").write_text(
        "def trigger():\n    import pkg.marker\n", encoding="utf-8"
    )
    (tmp_path / "late_leaf.py").write_text("VALUE = 1\n", encoding="utf-8")
    script = "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\nprint(json.dumps('late_leaf' in sys.modules))\n"
    actual = run_guarded_test_process(
        [sys.executable, "-I", "-B", "-c", script, str(tmp_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(actual.stdout) is True
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
    assert {"late_leaf", "pkg.marker"} <= result.graph.keys()
    assert "pkg.unused" not in result.graph
    assert (
        "marker"
        in result.scan_authority.by_module["pkg"].namespace_premise.foreign_writes
    )


def test_artifact_owned_source_gap_widens_namespace_without_interpreting_it(tmp_path):
    import json
    import sys
    from molt.cli.models import _ImportAdmissionPolicy

    sources = {
        "entry": "import api\nimport native\napi.run()\n",
        "api": "from typing import TYPE_CHECKING\ndef run():\n    if TYPE_CHECKING:\n        import late_leaf\n",
        "typing": "TYPE_CHECKING = False\n",
        "native": "import api\napi.TYPE_CHECKING = True\n",
        "late_leaf": "VALUE = 1\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    script = "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\nprint(json.dumps('late_leaf' in sys.modules))\n"
    actual = run_guarded_test_process(
        [sys.executable, "-I", "-B", "-c", script, str(tmp_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(actual.stdout) is True
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        None,
        set(),
        import_admission_policy=_ImportAdmissionPolicy(
            native_artifact_source_packages=frozenset({"native"})
        ),
        full_scan_roots=True,
    )
    assert "native" not in result.graph
    assert "late_leaf" in result.graph
    premise = result.scan_authority.by_module["api"].namespace_premise
    assert premise.foreign_writes is None and premise.member_writes


def test_late_foreign_all_write_wakes_previously_excluded_star_reader(tmp_path):
    import json
    import sys

    sources = {
        "entry": "import api\nimport writer\nimport consumer\nconsumer.run()\n",
        "api": "__all__ = ['unused']\ndef callback():\n    import selected\ndef unused():\n    import unused_leaf\n",
        "writer": "import api\napi.__all__ = ['callback']\n",
        "consumer": "from api import *\ndef run():\n    callback()\n",
        "selected": "VALUE = 1\n",
        "unused_leaf": "VALUE = 1\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    script = "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\nprint(json.dumps(sorted(set(sys.modules)&{'selected','unused_leaf'})))\n"
    actual = run_guarded_test_process(
        [sys.executable, "-I", "-B", "-c", script, str(tmp_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(actual.stdout) == ["selected"]
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
    assert (
        "__all__"
        in result.scan_authority.by_module["api"].namespace_premise.foreign_writes
    )


def test_precomputed_namespace_mismatch_reprojects_identified_bytes(tmp_path):
    from molt.cli import module_resolution
    from molt.cli.module_source import PythonSourceSnapshot
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.target_python import TargetPythonVersion

    path = tmp_path / "entry.py"
    path.write_text(
        "from typing import TYPE_CHECKING\ncallback()\nif TYPE_CHECKING:\n    import lazy\n",
        encoding="utf-8",
    )
    cache = module_resolution._ModuleResolutionCache()
    target = TargetPythonVersion(3, 12, 0)
    snapshot = PythonSourceSnapshot.capture(path)
    unknown = module_graph_discovery._load_module_import_scan(
        path,
        purpose=SourceImportPurpose.EXECUTION,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=cache,
        project_root=None,
        source_snapshot=snapshot,
        target_python=target,
    )
    assert "lazy" in unknown.scan.imports
    precomputed = module_graph_discovery._bind_precomputed_module_import_scan(
        path,
        purpose=SourceImportPurpose.EXECUTION,
        module_name="entry",
        import_scan_mode="full",
        scan=unknown.scan,
        snapshot=snapshot,
        target_python=target,
    )
    precise = module_graph_discovery._load_module_import_scan(
        path,
        purpose=SourceImportPurpose.EXECUTION,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=cache,
        project_root=None,
        source_snapshot=snapshot,
        target_python=target,
        precomputed_scan=precomputed,
        namespace_premise=PythonNamespacePremise(
            (),
            0,
            entry_member_writes=0,
            noncanonical_imports=(),
            opaque_activation_sources=None,
        ),
    )
    assert "lazy" not in precise.scan.imports
    assert precise.snapshot is snapshot
    assert (
        precise.scan.source_requests.namespace_summary
        == unknown.scan.source_requests.namespace_summary
    )
    assert len(cache.ast_cache) == 1


def test_matching_precomputed_scan_keeps_bytes_until_graph_fixpoint(
    tmp_path, monkeypatch
):
    from molt.cli import module_resolution
    from molt.cli.module_source import PythonSourceChangedError, PythonSourceSnapshot
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.target_python import TargetPythonVersion

    entry = tmp_path / "entry.py"
    entry.write_text("from api import live\nlive()\n", encoding="utf-8")
    (tmp_path / "api.py").write_text(
        "def live():\n    import selected\n", encoding="utf-8"
    )
    (tmp_path / "selected.py").write_text("VALUE = 1\n", encoding="utf-8")
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    target = TargetPythonVersion(3, 12, 0)
    premise = PythonNamespacePremise(
        (),
        0,
        entry_member_writes=0,
        noncanonical_imports=(),
        opaque_activation_sources=None,
    )
    snapshot = PythonSourceSnapshot.capture(entry)
    prepared = module_graph_discovery._load_module_import_scan(
        entry,
        purpose=SourceImportPurpose.EXECUTION,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=module_resolution._ModuleResolutionCache(),
        project_root=None,
        source_snapshot=snapshot,
        target_python=target,
        namespace_premise=premise,
    )
    precomputed = module_graph_discovery._bind_precomputed_module_import_scan(
        entry,
        purpose=SourceImportPurpose.EXECUTION,
        module_name="entry",
        import_scan_mode="full",
        scan=prepared.scan,
        snapshot=snapshot,
        is_package=False,
        target_python=target,
        namespace_premise=premise,
    )
    collect = _source_import_scan._collect_imports_for_graph

    def mutate(*args, **kwargs):
        result = collect(*args, **kwargs)
        if kwargs.get("source_owner") is not None:
            entry.write_text("import replaced\n", encoding="utf-8")
        return result

    monkeypatch.setattr(_source_import_scan, "_collect_imports_for_graph", mutate)
    with pytest.raises(PythonSourceChangedError, match="source changed"):
        module_graph_discovery._discover_module_graph_from_paths(
            (entry,),
            [tmp_path],
            [tmp_path],
            stdlib,
            None,
            set(),
            full_scan_roots=True,
            precomputed_scans_by_path={entry.resolve(): precomputed},
            target_python=target,
        )


@pytest.mark.parametrize("explicit_input", ["source", "tree"])
def test_matching_precomputed_scan_cannot_override_explicit_source(
    tmp_path, explicit_input
):
    from molt.cli import module_resolution
    from molt.cli.module_source import PythonSourceSnapshot
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.target_python import TargetPythonVersion

    path = tmp_path / "entry.py"
    path.write_text("import stored\n", encoding="utf-8")
    target = TargetPythonVersion(3, 12, 0)
    premise = PythonNamespacePremise(
        (),
        0,
        entry_member_writes=0,
        noncanonical_imports=(),
        opaque_activation_sources=None,
    )
    snapshot = PythonSourceSnapshot.capture(path)
    cache = module_resolution._ModuleResolutionCache()
    prepared = module_graph_discovery._load_module_import_scan(
        path,
        purpose=SourceImportPurpose.EXECUTION,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=cache,
        project_root=None,
        source_snapshot=snapshot,
        target_python=target,
        namespace_premise=premise,
    )
    precomputed = module_graph_discovery._bind_precomputed_module_import_scan(
        path,
        purpose=SourceImportPurpose.EXECUTION,
        module_name="entry",
        import_scan_mode="full",
        scan=prepared.scan,
        snapshot=snapshot,
        is_package=False,
        target_python=target,
        namespace_premise=premise,
    )
    source = "import supplied\n"
    explicit = (
        {"source": source}
        if explicit_input == "source"
        else {"tree": ast.parse(source)}
    )
    loaded = module_graph_discovery._load_module_import_scan(
        path,
        purpose=SourceImportPurpose.EXECUTION,
        module_name="entry",
        is_package=False,
        import_scan_mode="full",
        resolution_cache=cache,
        project_root=None,
        target_python=target,
        namespace_premise=premise,
        precomputed_scan=precomputed,
        **explicit,
    )
    assert set(loaded.scan.imports) == {"supplied"}


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    "activation",
    ["unused", "called", "decorated", "unstarted-generator", "resumed-generator"],
)
def test_foreign_namespace_writes_require_actual_source_body_custody(
    tmp_path, monkeypatch, activation
):
    import json
    import sys
    from molt.cli import module_resolution
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    write = "sys.modules[choose_module()].__dict__[choose_name()] = callback"
    writer = (
        "import sys\nfrom replacement import callback\n"
        "def choose_module(): return 'api'\n"
        "def choose_name(): return 'callback'\n"
    )
    if activation == "decorated":
        writer += "def activate(fn):\n    fn()\n    return fn\n@activate\n"
    writer += "def install():\n    " + write + "\n"
    if "generator" in activation:
        writer += "    yield 1\n"
    invoke = {
        "unused": "",
        "called": "    writer.install()\n",
        "decorated": "",
        "unstarted-generator": "    writer.install()\n",
        "resumed-generator": "    next(writer.install())\n",
    }[activation]
    sources = {
        "entry": "import api\napi.run()\n",
        "api": "def callback():\n    pass\ndef unused():\n    import unused_leaf\ndef run():\n    import writer\n"
        + invoke
        + "    callback()\n",
        "writer": writer,
        "replacement": "def callback():\n    import late_leaf\n",
        "late_leaf": "VALUE = 1\n",
        "unused_leaf": "VALUE = 1\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\nprint(json.dumps(sorted(set(sys.modules)&set(json.loads(sys.argv[2])))))\n",
            str(tmp_path),
            json.dumps(sorted(sources)),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    actual = set(json.loads(executed.stdout))
    writes_execute = activation in {"called", "decorated", "resumed-generator"}
    assert ("late_leaf" in actual) is writes_execute
    assert "unused_leaf" not in actual
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "writer-owner",
    )
    install = next(
        node
        for node in ast.parse(writer).body
        if isinstance(node, ast.FunctionDef) and node.name == "install"
    )
    install_key = python_node_source_key(install)
    load = module_graph_discovery._load_module_import_scan
    parse = module_resolution._parse_source_for_target
    body_loads = []
    parses = []

    def observe_load(path, **kwargs):
        if (
            kwargs.get("module_name") == "writer"
            and kwargs.get("source_owner") == install_key
        ):
            body_loads.append(kwargs.get("namespace_premise"))
        return load(path, **kwargs)

    def observe_parse(*args, **kwargs):
        parses.append(kwargs["filename"])
        return parse(*args, **kwargs)

    monkeypatch.setattr(
        module_graph_discovery, "_load_module_import_scan", observe_load
    )
    monkeypatch.setattr(module_resolution, "_parse_source_for_target", observe_parse)
    for _warm in (False, True):
        body_loads.clear()
        parses.clear()
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        assert actual <= result.graph.keys()
        premise = result.scan_authority.by_module["api"].namespace_premise
        assert premise is not None
        assert len(
            {replace(p, opaque_activation_sources=()) for p in body_loads}
        ) == int(writes_execute)
        assert len(body_loads) == len(set(body_loads))
        assert len(body_loads) <= (2 if writes_execute else 0)
        if _warm:
            assert not parses
        if writes_execute:
            assert premise.foreign_writes is None
            assert body_loads[0].foreign_writes is None
            expected_opaque = tuple(
                sorted(
                    python_node_source_key(node)
                    for node in ast.parse(writer).body
                    if isinstance(node, ast.FunctionDef)
                )
            )
            # The body's answers do not depend on its siblings' activation, so
            # early cutoff keeps its scan while the premise gains them (HF-926).
            writer_premise = result.scan_authority.by_module["writer"].namespace_premise
            assert writer_premise is not None
            assert writer_premise.opaque_activation_sources == expected_opaque
        else:
            assert premise.foreign_writes == ()
            assert "late_leaf" not in result.graph
            assert "unused_leaf" not in result.graph


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    ("invoke", "subclass"), [(False, False), (True, False), (True, True)]
)
def test_star_custom_method_keeps_its_actual_hook_writer_provenance(
    tmp_path, monkeypatch, invoke, subclass
):
    import json
    import sys
    from molt.compiler_analysis.python_identity_catalog_generated import PythonMember

    (tmp_path / "entry.py").write_text(
        "from exports import *\n" + ("hooks.register()\n" if invoke else ""),
        encoding="utf-8",
    )
    (tmp_path / "exports.py").write_text(
        "import sys\n"
        "class Finder:\n"
        "    def find_spec(self, fullname, path=None, target=None):\n        return None\n"
        + ("class Hooks(list):\n" if subclass else "class Hooks:\n")
        + "    def register(self):\n        sys.meta_path.append(Finder())\n"
        "hooks = Hooks()\n__all__ = ['hooks']\n",
        encoding="utf-8",
    )
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\nbefore=len(sys.meta_path)\nimport entry\nprint(json.dumps(len(sys.meta_path)-before))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(executed.stdout) == int(invoke)
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "custom-hook-writer",
    )
    for _warm in (False, True):
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        premise = result.scan_authority.by_module["entry"].namespace_premise
        assert premise is not None
        assert bool(premise.member_writes & PythonMember.IMPORT_HOOKS) is invoke


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    ("expression", "method"),
    [
        (None, None),
        ("f'{formatter}.cache'", "format"),
        ("f'{formatter!s}.cache'", "str"),
        ("f'{formatter!r}.cache'", "repr"),
        ("f'{0:{formatter}}.cache'", "format"),
    ],
)
def test_disjoint_import_cache_key_keeps_actual_format_callback_custody(
    tmp_path, monkeypatch, expression, method
):
    import json
    import sys

    sources = {
        "entry": ("import sys\n" if expression else "")
        + "from formatter_provider import formatter\n"
        + (f"sys.modules[{expression}] = None\n" if expression else "")
        + "import consumer\n",
        "formatter_provider": "events = []\nclass Formatter:\n"
        + "".join(
            f"    def {special}(self{', spec' if special == '__format__' else ''}):\n"
            f"        events.append({event!r})\n"
            "        import typing\n        typing.TYPE_CHECKING = True\n"
            "        import formatting_leaf\n        return '1'\n"
            for special, event in (
                ("__format__", "format"),
                ("__str__", "str"),
                ("__repr__", "repr"),
            )
        )
        + "formatter = Formatter()\n",
        # Observe the actual module-initialization binding. A deferred global
        # read deliberately also admits reconstructed activation namespaces.
        "consumer": "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import late_leaf\n",
        "typing": "TYPE_CHECKING = False\n",
        "formatting_leaf": "VALUE = 1\n",
        "late_leaf": "VALUE = 1\n",
    }
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    for name, source in sources.items():
        # This tests canonical typing-member custody, not a project module
        # shadowing typing. CPython and discovery use the same provider roots.
        root = stdlib if name == "typing" else tmp_path
        (root / f"{name}.py").write_text(source, encoding="utf-8")
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path[:0] = sys.argv[1:3]\nimport entry,formatter_provider\nprint(json.dumps([formatter_provider.events, sorted(set(sys.modules)&{'formatting_leaf','late_leaf'})]))\n",
            str(tmp_path),
            str(stdlib),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    events, imported = json.loads(executed.stdout)
    assert events == ([] if method is None else [method])
    assert set(imported) == (
        set() if method is None else {"formatting_leaf", "late_leaf"}
    )
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "format-writer-custody",
    )
    for _warm in (False, True):
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            stdlib,
            tmp_path,
            {"typing"},
            full_scan_roots=True,
        )
        assert ("formatting_leaf" in result.graph) is (method is not None)
        assert ("late_leaf" in result.graph) is (method is not None)


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize("publication", ["local", "imported", "global_fallback"])
def test_class_mapping_call_keeps_source_candidates_without_module_universe(
    tmp_path, monkeypatch, publication
):
    import json
    import sys

    sources = {
        "entry": "import holder\n",
        "actions": "def action():\n    import leaf\n",
        "holder": "def unrelated():\n    import unused_leaf\n"
        + (
            "def install():\n    global action\n    from actions import action\ninstall()\n"
            if publication == "global_fallback"
            else ""
        )
        + "class Base:\n    pass\nclass D(Base):\n"
        + {
            "local": "    def action():\n        import leaf\n",
            "imported": "    from actions import action\n",
            "global_fallback": "    if False:\n        action = None\n",
        }[publication]
        + "    alias = action\n    alias()\n",
        "leaf": "VALUE = 1\n",
        "unused_leaf": "VALUE = 1\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\n"
            "print(json.dumps(sorted(set(sys.modules)&{'leaf','unused_leaf'})))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(executed.stdout) == ["leaf"]
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "class-source-mapping-custody",
    )
    for _warm in (False, True):
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        assert "leaf" in result.graph
        if publication != "global_fallback":
            assert "unused_leaf" not in result.graph


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    "read",
    [
        "globals()['callback']",
        "globals().get('callback')",
        "globals().__getitem__('callback')",
        "globals().pop('callback')",
    ],
)
def test_current_namespace_read_keeps_imported_callable_source(
    tmp_path, monkeypatch, read
):
    import json
    import sys

    sources = {
        "entry": "import holder\n",
        "holder": "from actions import action\nglobals()['callback'] = action\n"
        "def unused():\n    import unused_namespace_leaf\n" + read + "()\n",
        "actions": "def action():\n    import namespace_callback_leaf\n",
        "namespace_callback_leaf": "VALUE = 1\n",
        "unused_namespace_leaf": "VALUE = 1\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\n"
            "print(json.dumps(['namespace_callback_leaf' in sys.modules,\n"
            "                  'unused_namespace_leaf' in sys.modules]))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(observed.stdout) == [True, False]
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "namespace-callable-source-custody",
    )
    for _warm in (False, True):
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        assert "namespace_callback_leaf" in result.graph
        assert "unused_namespace_leaf" not in result.graph


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize("reader", ["globals()", "locals()", "vars()"])
@pytest.mark.parametrize(
    ("use", "escaped"),
    [
        ("{reader}", False),
        ("{reader}.get('missing')", False),
        ("{reader}.pop('missing', None)", False),
        ("consume({reader})", True),
        ("globals()['self'] = {reader}\nconsume(globals()['self'])", True),
    ],
)
def test_namespace_call_result_read_is_not_namespace_escape(
    tmp_path, monkeypatch, reader, use, escaped
):
    import json
    import sys

    sources = {
        "entry": "import holder\n",
        "holder": "from consumer import consume\n"
        "def unused():\n    import unused_namespace_leaf\n"
        + use.format(reader=reader)
        + "\n",
        "consumer": "def consume(namespace):\n    namespace['unused']()\n",
        "unused_namespace_leaf": "VALUE = 1\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\n"
            "print(json.dumps('unused_namespace_leaf' in sys.modules))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(observed.stdout) is escaped
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "namespace-read",
    )
    for _warm in (False, True):
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        assert ("unused_namespace_leaf" in result.graph) is escaped


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize("publication", ["container", "local", "parameter", "starred"])
def test_named_source_writer_keeps_only_published_callable_availability(
    tmp_path, monkeypatch, publication
):
    import json
    import sys

    if publication == "container":
        body = (
            "state = set()\ndef replace_state():\n    global state\n    state = set()\n"
            "def run():\n    if 'x' in state:\n        raise AssertionError\n"
        )
    else:
        body = (
            "from actions import callback as selected\ndef original():\n    pass\n"
            "callback = original\n"
            + (
                "def install(value):\n    global callback\n    callback = value\n"
                if publication in {"parameter", "starred"}
                else "def install():\n    global callback\n    callback = selected\n"
            )
            + "def run():\n    alias = callback\n    install("
            + (
                "*(selected,)"
                if publication == "starred"
                else "selected"
                if publication == "parameter"
                else ""
            )
            + ")\n    alias()\n    callback()\n"
        )
    sources = {
        "entry": "import holder\nholder.run()\n",
        "holder": body + "def unrelated():\n    import unrelated_leaf\n",
        "actions": "def callback():\n    import chosen_leaf\n",
        "chosen_leaf": "VALUE = 1\n",
        "unrelated_leaf": "VALUE = 1\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\n"
            "print(json.dumps(sorted(set(sys.modules)&{'chosen_leaf','unrelated_leaf'})))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    expected = [] if publication == "container" else ["chosen_leaf"]
    assert json.loads(observed.stdout) == expected
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "named-source-writer-availability",
    )
    for _warm in (False, True):
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        assert "unrelated_leaf" not in result.graph
        assert ("chosen_leaf" in result.graph) is (publication != "container")


@pytest.mark.usefixtures("isolated_molt_cache")
def test_starred_class_base_keeps_actual_subclass_hook_source(tmp_path, monkeypatch):
    import json
    import sys

    sources = {
        "entry": "import holder\n",
        "holder": "from bases import Base\nclass D(*(Base,)):\n    pass\n",
        "bases": "class Base:\n    def __init_subclass__(cls, **kwargs):\n"
        "        import subclass_leaf\ndef unrelated():\n    import unrelated_leaf\n",
        "subclass_leaf": "VALUE = 1\n",
        "unrelated_leaf": "VALUE = 1\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\n"
            "print(json.dumps(sorted(set(sys.modules)&{'subclass_leaf','unrelated_leaf'})))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(observed.stdout) == ["subclass_leaf"]
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "starred-base-source-custody",
    )
    for _warm in (False, True):
        result = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            full_scan_roots=True,
        )
        assert "subclass_leaf" in result.graph
        assert "unrelated_leaf" not in result.graph


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize("warm", [False, True])
def test_late_closure_writer_updates_prior_source_without_rescanning_siblings(
    tmp_path, monkeypatch, warm
):
    import json
    import sys
    from molt.cli import module_resolution

    package = tmp_path / "hf226_pkg"
    package.mkdir()
    (package / "__init__.py").write_text("", encoding="utf-8")
    (package / "sub.py").write_text("def handler(): return 'old'\n", encoding="utf-8")
    (tmp_path / "untouched.py").write_text("VALUE = 1\n", encoding="utf-8")
    (tmp_path / "entry.py").write_text(
        "import hf226_pkg.sub\nimport untouched\nhf226_pkg.sub.handler()\n",
        encoding="utf-8",
    )
    support = tmp_path / "support.py"
    support.write_text(
        "import hf226_pkg\ndef replacement(): return 'new'\n"
        "hf226_pkg.sub.handler = replacement\n",
        encoding="utf-8",
    )
    actual = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\n"
            "import entry,hf226_pkg.sub\nbefore=hf226_pkg.sub.handler()\n"
            "import support\nprint(json.dumps([before,hf226_pkg.sub.handler()]))\n",
            str(tmp_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(actual.stdout) == ["old", "new"]
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "late-closure-writer",
    )
    load = module_graph_discovery._load_module_import_scan
    loads = []

    def observe(path, **kwargs):
        loaded = load(path, **kwargs)
        loads.append((kwargs["module_name"], kwargs.get("source_owner"), loaded))
        return loaded

    monkeypatch.setattr(module_graph_discovery, "_load_module_import_scan", observe)
    for run in range(2 if warm else 1):
        cache = module_resolution._ModuleResolutionCache()
        state = module_graph_discovery._ModuleGraphDiscovery(
            cache, module_graph_discovery._DEFAULT_TARGET_PYTHON_VERSION
        )
        initial = module_graph_discovery._discover_module_graph_from_paths(
            (tmp_path / "entry.py",),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            tmp_path,
            set(),
            resolver_cache=cache,
            full_scan_roots=True,
            discovery=state,
        )
        prior = initial.scan_authority.by_module["hf226_pkg.sub"]
        assert prior.namespace_premise.foreign_writes == ()
        graph = dict(initial.graph)
        sources = dict(initial.scan_authority.by_module)
        loads.clear()
        parse_before = cache.source_parse_count
        later = module_graph_discovery._extend_module_graph_with_closure(
            graph,
            scan_authorities=sources,
            entry_paths=(support,),
            full_scan_roots=False,
            roots=[tmp_path],
            module_roots=[tmp_path],
            stdlib_root=tmp_path / "stdlib",
            project_root=tmp_path,
            stdlib_allowlist=set(),
            resolver_cache=cache,
            diagnostics_enabled=False,
            module_reasons={},
            root_reason=ModuleReason.NATIVE_SUPPORT_SOURCE,
            discovery=state,
        )
        assert later.discovery is state
        assert sources["hf226_pkg.sub"].namespace_premise.foreign_writes == ("handler",)
        projected = [
            observed
            for observed in cache.import_query_closure().observations
            if observed.query.module_name == "hf226_pkg.sub"
            and observed.query.namespace_premise is not None
            and observed.query.namespace_premise.foreign_writes == ("handler",)
        ]
        assert projected
        if run == 0:
            assert any(
                ImportScanAdmission.PREMISE_REUSE in observed.admissions
                for observed in projected
            )
        assert prior.namespace_premise.foreign_writes == ()
        assert "hf226_pkg.sub" not in later.graph
        # ``sub``'s scan reads nothing the writer changed: early cutoff keeps
        # it while its premise gains ``handler`` (HF-926). A warm run, whose
        # first scans came from records, reads the published alias instead.
        assert not any(
            name == "hf226_pkg.sub" and not loaded.cache_hit
            for name, _, loaded in loads
        )
        assert not any(name in {"entry", "untouched"} for name, _, _ in loads)
        # Prior captures are reused, even when their namespace projection widens.
        assert cache.source_parse_count - parse_before <= 1
        solver = state.demands
        loads.clear()
        module_graph_discovery._extend_module_graph_with_closure(
            graph,
            scan_authorities=sources,
            entry_paths=(support,),
            full_scan_roots=False,
            roots=[tmp_path],
            module_roots=[tmp_path],
            stdlib_root=tmp_path / "stdlib",
            project_root=tmp_path,
            stdlib_allowlist=set(),
            resolver_cache=cache,
            diagnostics_enabled=False,
            module_reasons={},
            root_reason=ModuleReason.NATIVE_SUPPORT_SOURCE,
            discovery=state,
        )
        assert state.demands is solver
        assert not loads
        state.validate_sources()


def test_later_source_admission_resolves_earlier_writer_target(tmp_path, monkeypatch):
    from molt.cli import module_resolution

    writers = tmp_path / "writers"
    readers = tmp_path / "readers"
    writers.mkdir()
    readers.mkdir()
    writer = writers / "writer.py"
    writer.write_text(
        "import late\ndef replacement(): return 2\nlate.handler = replacement\n",
        encoding="utf-8",
    )
    late = readers / "late.py"
    late.write_text("def handler(): return 1\n", encoding="utf-8")
    cache = module_resolution._ModuleResolutionCache()
    state = module_graph_discovery._ModuleGraphDiscovery(
        cache, module_graph_discovery._DEFAULT_TARGET_PYTHON_VERSION
    )
    initial = module_graph_discovery._discover_module_graph_from_paths(
        (writer,),
        [writers],
        [writers],
        tmp_path / "stdlib",
        None,
        set(),
        resolver_cache=cache,
        full_scan_roots=False,
        discovery=state,
    )
    assert initial.graph == {"writer": writer}
    assert "late" in state.writer_targets
    result = module_graph_discovery._discover_module_graph_from_paths(
        (late,),
        [readers],
        [readers],
        tmp_path / "stdlib",
        None,
        set(),
        resolver_cache=cache,
        full_scan_roots=False,
        discovery=state,
    )
    assert result.scan_authority.by_module["late"].namespace_premise.foreign_writes == (
        "handler",
    )


@pytest.mark.parametrize("verified", [False, True])
def test_closure_publication_visits_only_its_changed_authority_rows(tmp_path, verified):
    from dataclasses import replace
    from molt.cli.models import _ModuleGraphScanAuthority, _ModuleSourceScanAuthority
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    class DeltaOnlyMap(dict):
        def __iter__(self):
            raise AssertionError("publication scanned unrelated module keys")

        def items(self):
            raise AssertionError("publication scanned unrelated module rows")

        def values(self):
            raise AssertionError("publication scanned unrelated source rows")

    prior = _ModuleSourceScanAuthority(
        "prior",
        tmp_path / "prior.py",
        "module_init",
        body_selection=ModuleBodySelection("complete_source"),
        namespace_premise=PythonNamespacePremise(
            (),
            0,
            entry_member_writes=0,
            noncanonical_imports=(),
            opaque_activation_sources=None,
        )
        if verified
        else None,
    )
    added = _ModuleSourceScanAuthority(
        "added",
        tmp_path / "added.py",
        "module_init",
        body_selection=ModuleBodySelection("complete_source"),
    )
    revised = replace(
        prior,
        namespace_premise=PythonNamespacePremise(
            ("handler",),
            0,
            entry_member_writes=0,
            noncanonical_imports=(),
            opaque_activation_sources=None,
        ),
    )
    graph = DeltaOnlyMap({"prior": prior.source_path})
    sources = DeltaOnlyMap({"prior": prior})
    module_graph_discovery._merge_discovered_module_graph(
        graph,
        sources,
        _DiscoveredModuleGraph(
            {"added": added.source_path},
            set(),
            _ModuleGraphScanAuthority((added,)),
            revised_sources=(revised,),
        ),
    )
    assert graph["added"] == added.source_path
    assert sources["prior"].namespace_premise == (
        revised.namespace_premise if verified else None
    )
    assert sources["added"] == added


def test_unchanged_request_still_checks_live_source_generation(tmp_path):
    from molt.cli.module_source import PythonSourceChangedError

    entry = tmp_path / "entry.py"
    entry.write_text("VALUE = 1\n", encoding="utf-8")
    first = module_graph_discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=False,
    )
    entry.write_text("VALUE = 2\n", encoding="utf-8")
    with pytest.raises(PythonSourceChangedError, match="source changed"):
        module_graph_discovery._discover_module_graph_from_paths(
            (entry,),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            None,
            set(),
            full_scan_roots=False,
            discovery=first.discovery,
        )


def test_full_runtime_owner_promotion_retires_prior_partial_body_jobs(
    tmp_path, monkeypatch
):
    from molt.cli.models import _RuntimeImportScanCustody
    from molt.compiler_analysis.python_source_keys import python_ast_digest

    entry = tmp_path / "entry.py"
    entry.write_text("import owner\nowner.run()\n", encoding="utf-8")
    owner = tmp_path / "owner.py"
    owner.write_text("def run():\n    import leaf\n", encoding="utf-8")
    (tmp_path / "leaf.py").write_text("pass\n", encoding="utf-8")
    writer = tmp_path / "writer.py"
    writer.write_text(
        "import sys\nsys.modules[choose_module()].__dict__[choose_name()] = 1\n",
        encoding="utf-8",
    )
    initial = module_graph_discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=True,
    )
    state = initial.discovery
    assert state.scanned_bodies["owner"]
    custody = _RuntimeImportScanCustody(
        (("owner", owner),),
        tuple(sorted(initial.graph.items())),
        (("owner", python_ast_digest(ast.parse(owner.read_text(encoding="utf-8")))),),
    )
    module_graph_discovery._discover_module_graph_from_paths(
        (owner,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=True,
        runtime_import_custody=custody,
        discovery=state,
    )
    load = module_graph_discovery._load_module_import_scan
    owner_jobs = []

    def observe(path, **kwargs):
        if kwargs["module_name"] == "owner":
            owner_jobs.append(
                (
                    kwargs["import_scan_mode"],
                    kwargs.get("source_owner"),
                    kwargs["namespace_premise"],
                )
            )
        return load(path, **kwargs)

    monkeypatch.setattr(module_graph_discovery, "_load_module_import_scan", observe)
    module_graph_discovery._discover_module_graph_from_paths(
        (writer,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=False,
        discovery=state,
    )
    # The writer supersedes owner's outputs, so the operation replays its
    # history: the pre-custody call scans owner's body again, once, under the
    # settled premise. Full custody still scans only the module (HF-926).
    assert len(set(owner_jobs)) == len(owner_jobs)
    full_jobs = [
        (owner, premise) for mode, owner, premise in owner_jobs if mode == "full"
    ]
    assert 1 <= len(full_jobs) <= 2
    assert all(owner is None for owner, _ in full_jobs)
    premises = [premise for _, premise in full_jobs]
    assert len({replace(p, opaque_activation_sources=()) for p in premises}) == 1
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    run = ast.parse(owner.read_text(encoding="utf-8")).body[0]
    assert len(set(premises)) == len(premises)
    assert set(premises[0].opaque_activation_sources) <= {python_node_source_key(run)}
    assert premises[-1].opaque_activation_sources == (python_node_source_key(run),)
    assert state.scan_sources["owner"].namespace_premise.foreign_writes is None


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    ("post", "capture", "stores", "inputs", "invoke", "required", "excluded"),
    [
        (
            "def __post_init__(self, callback): callback()",
            "()",
            "()",
            "(('callback', 1),)",
            "Point(callback)",
            {"live_leaf"},
            {"dead_leaf"},
        ),
        (
            "def __post_init__(self, *callbacks): callbacks[0]()",
            "()",
            "()",
            "(('callback', 1),)",
            "Point(callback=callback)",
            {"live_leaf"},
            {"dead_leaf"},
        ),
        (
            "def __post_init__(self, callback): callback()",
            "()",
            "()",
            "(('callback', 1),)",
            "Point.__init__.__call__(Point.__new__(Point), callback)",
            {"live_leaf"},
            {"dead_leaf"},
        ),
        (
            "pass",
            "(callback, unused)",
            "(('value', -1, -1, 0, True),)",
            "()",
            "Point()",
            {"live_leaf"},
            {"dead_leaf"},
        ),
        (
            "pass",
            "(unused,)",
            "(('value', -1, -1, 0, False),)",
            "()",
            "Point()",
            set(),
            {"live_leaf", "dead_leaf"},
        ),
        (
            "def __post_init__(self, callback): callback()",
            "()",
            "()",
            "(('callback', -1),)",
            "Point()",
            {"live_leaf"},
            {"dead_leaf"},
        ),
        (
            "def __post_init__(self, callback): callback()",
            "()",
            "()",
            "(('callback', 1),)",
            "Point()",
            {"live_leaf"},
            {"dead_leaf"},
        ),
        (
            "def __post_init__(self, callback): callback()",
            "()",
            "()",
            "(('builtin_callback', -1),)",
            "Point()",
            {"builtin_leaf"},
            {"live_leaf", "dead_leaf"},
        ),
    ],
)
def test_native_initializer_uses_plan_selected_callbacks(
    tmp_path, monkeypatch, post, capture, stores, inputs, invoke, required, excluded
):
    # The public CPython dataclass oracle is independent of the private native
    # metadata representation. A factory invokes its binding; a stored default
    # remains a value, and InitVar values flow to positional or variadic consumers.
    import dataclasses

    observed = []
    if "True" in stores:

        @dataclasses.dataclass
        class Oracle:
            value: object = dataclasses.field(
                default_factory=lambda: observed.append("live_leaf")
            )

        Oracle()
    elif "False" in stores:

        @dataclasses.dataclass
        class Oracle:
            value: object = lambda: observed.append("dead_leaf")

        Oracle()
    elif "builtin_callback" in inputs:
        namespace = {
            "__builtins__": {
                "builtin_callback": lambda: observed.append("builtin_leaf")
            }
        }
        exec("def oracle(): builtin_callback()", namespace)
        namespace["oracle"]()
    else:
        Oracle = dataclasses.make_dataclass(
            "Oracle",
            [("callback", dataclasses.InitVar[object])],
            namespace={"__post_init__": lambda self, *callbacks: callbacks[0]()},
        )
        Oracle(lambda: observed.append("live_leaf"))
    assert set(observed) == required

    import ast

    capture_names = tuple(
        f"capture{index}"
        for index, _ in enumerate(ast.parse(capture, mode="eval").body.elts)
    )
    parameters = ("self", "callback") if " 1)" in inputs else ("self",)
    schema = ((), False, post != "pass", False, False)
    defaults = "(callback,)" if len(parameters) == 2 and invoke == "Point()" else "()"
    sources = {
        "entry": "from _intrinsics import require_intrinsic\nimport api, builtin_api\n"
        "make = require_intrinsic('molt_dataclasses_init')\n"
        "def callback():\n    import live_leaf\n"
        "def unused():\n    import dead_leaf\n"
        "class Point:\n    "
        + post
        + "\n"
        + f"spec = ({parameters!r}, {len(parameters)}, {defaults}, {{}}, {capture_names!r}, "
        + capture
        + ", "
        + stores
        + ", "
        + inputs
        + ", (-1, -1), (-1, -1), {}, api.__dict__, 'entry', builtin_api)\n"
        + f"Point.__init__ = make(Point, {schema!r}, spec)\n"
        + invoke
        + "\n",
        "api": "def callback():\n    import live_leaf\n",
        "builtin_api": "def builtin_callback():\n    import builtin_leaf\n",
        "builtin_leaf": "pass\n",
        "live_leaf": "pass\n",
        "dead_leaf": "pass\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "native-plan-callbacks",
    )
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
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
        assert required <= result.graph.keys()
        assert excluded.isdisjoint(result.graph)


@pytest.mark.usefixtures("isolated_molt_cache")
def test_namespace_clear_preserves_contained_destructor(tmp_path, monkeypatch):
    import sys

    sources = {
        "entry": "import api\napi.__dict__.clear()\n",
        "api": "from values import Payload\nvalue = Payload()\ndel Payload\n",
        "values": "class Payload:\n    def __del__(self):\n        import cleanup_leaf\n",
        "cleanup_leaf": "pass\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    result = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys; sys.path.insert(0, sys.argv[1]); import entry; assert 'cleanup_leaf' in sys.modules",
            str(tmp_path),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    assert result.returncode == 0
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "namespace-cleanup",
    )
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
    result = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        stdlib,
        tmp_path,
        set(),
        full_scan_roots=True,
    )
    assert "cleanup_leaf" in result.graph


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    "selection",
    [
        "api.__dict__.popitem()[1]()",
        "key, callback = api.__dict__.popitem(); callback()",
    ],
)
def test_namespace_popitem_keeps_returned_tuple_value(tmp_path, monkeypatch, selection):
    import sys

    for name, source in {
        "entry": "import api\n" + selection + "\n",
        "api": "def callback():\n    import live_leaf\n",
        "live_leaf": "pass\n",
    }.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import sys; sys.path.insert(0, sys.argv[1]); import entry; assert 'live_leaf' in sys.modules",
            str(tmp_path),
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    assert observed.returncode == 0
    monkeypatch.setattr(
        module_graph_cache,
        "_import_scan_tooling_fingerprint",
        lambda: "namespace-popitem",
    )
    stdlib = tmp_path / "stdlib"
    stdlib.mkdir()
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
        assert "live_leaf" in result.graph


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    ("use", "exposed"),
    [
        ("p.x = i\n    p.y = p.x", False),
        # An operator runs only the special methods of the instance's class
        # family; Point defines none it could call.
        ("p + i", False),
    ],
)
def test_instance_attribute_access_keeps_its_class_definition_closed(
    tmp_path, use, exposed
):
    """A store or load on an instance runs only the descriptor protocol (the
    class's attribute hooks and data descriptors) and may release the old
    value; it does not hand the class to unknown code. The class statement
    stays out of the opaque activation sources, so construction and layout
    facts hold (HF-847, bench_struct). A binary operator escapes only the
    implicit special methods of the class family, never the class itself."""
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = (
        "class Point:\n"
        "    def __init__(self, x, y):\n"
        "        self.x = x\n"
        "        self.y = y\n"
        "def main(i):\n"
        "    p = Point(0, 0)\n"
        f"    {use}\n"
        "main(1)\n"
    )
    entry = tmp_path / "entry.py"
    entry.write_text(source, encoding="utf-8")
    result = module_graph_discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=False,
    )
    premise = result.scan_authority.by_module["entry"].namespace_premise
    assert premise is not None and premise.opaque_activation_sources is not None
    class_key = python_node_source_key(ast.parse(source).body[0])
    assert (class_key in premise.opaque_activation_sources) is exposed


@pytest.mark.usefixtures("isolated_molt_cache")
def test_superseded_scan_outputs_leave_the_graph(tmp_path: Path) -> None:
    """HF-926: the graph is the union of each scan unit's current output.

    `m` resolves `import_module(NAME)` while `NAME` is still its constant; the
    writer it imports rebinds `NAME` first, so that early scan's edge to
    `leaf` is superseded. CPython never imports `leaf`, and the discovered
    graph omits it, although the first pass's accumulated scans reached it.
    """
    import json
    import sys

    sources = {
        "entry": "import m\n",
        "m": "import importlib\nNAME = 'leaf'\nimport w\nimportlib.import_module(NAME)\n",
        "w": "import m\nm.NAME = 'other'\n",
        "leaf": "VALUE = 1\n",
        "other": "VALUE = 2\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\nimport entry\n"
            "print(json.dumps(sorted(set(sys.modules)&set(json.loads(sys.argv[2])))))\n",
            str(tmp_path),
            json.dumps(sorted(sources)),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(executed.stdout) == ["entry", "m", "other", "w"]
    arguments = (
        (tmp_path / "entry.py",),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        tmp_path,
        set(),
    )
    accumulated = module_graph_discovery._discovery_pass(
        *arguments, full_scan_roots=True
    )
    assert "leaf" in accumulated.graph
    result = module_graph_discovery._discover_module_graph_from_paths(
        *arguments, full_scan_roots=True
    )
    assert "leaf" not in result.graph
    assert {"entry", "m", "w"} <= result.graph.keys()
    premise = result.scan_authority.by_module["m"].namespace_premise
    assert premise is not None and premise.foreign_writes is not None
    assert "NAME" in premise.foreign_writes


@pytest.mark.usefixtures("isolated_molt_cache")
def test_continuation_keeps_every_module_an_earlier_call_published(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """HF-926: callers publish each call's graph additively, so a continuation
    never replays to a graph without a module an earlier call published.

    The first call reaches `leaf` through `m`'s `import_module(NAME)`; CPython
    imports it too when `entry` runs before `late`. `late` rebinds `m.NAME`,
    so `m`'s rescan may see another name after `import hook` and supersedes
    the scan that reached `leaf`. The continuation keeps `leaf`, and from then
    on the operation accumulates: a third call whose own scans supersede runs
    no replay pass (one would rebuild only one premise's graph, not the
    published accumulation, and the runtime catalog names its modules).
    """
    import json
    import sys

    sources = {
        "entry": "import m\n",
        "m": "import importlib\nNAME = 'leaf'\nimport hook\nimportlib.import_module(NAME)\n",
        "hook": "VALUE = 0\n",
        "late": "import m\nm.NAME = 'other'\n",
        "third": "import m3\n",
        "m3": "import importlib\nNAME = 'leaf3'\nimport w3\nimportlib.import_module(NAME)\n",
        "w3": "import m3\nm3.NAME = 'other3'\n",
        "leaf": "VALUE = 1\n",
        "other": "VALUE = 2\n",
        "leaf3": "VALUE = 3\n",
        "other3": "VALUE = 4\n",
    }
    for name, source in sources.items():
        (tmp_path / f"{name}.py").write_text(source, encoding="utf-8")
    executed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\n"
            "import entry, late, third\n"
            "print(json.dumps(sorted(set(sys.modules)&set(json.loads(sys.argv[2])))))\n",
            str(tmp_path),
            json.dumps(sorted(sources)),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert "leaf" in json.loads(executed.stdout)
    common = ([tmp_path], [tmp_path], tmp_path / "stdlib", tmp_path, set())
    first = module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "entry.py",), *common, full_scan_roots=True
    )
    state = first.discovery
    assert state is not None and "leaf" in state.graph
    module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "late.py",), *common, full_scan_roots=True, discovery=state
    )
    assert "leaf" in state.graph
    assert state.superseded_scans and state.accumulated

    replays: list[object] = []
    discovery_pass = module_graph_discovery._discovery_pass

    def counted(*args: object, **kwargs: object) -> object:
        if kwargs.get("settled_premises") is not None:
            replays.append(args[0])
        return discovery_pass(*args, **kwargs)

    monkeypatch.setattr(module_graph_discovery, "_discovery_pass", counted)
    module_graph_discovery._discover_module_graph_from_paths(
        (tmp_path / "third.py",), *common, full_scan_roots=True, discovery=state
    )
    assert state.superseded_scans
    assert replays == []
    assert {"leaf", "m3", "w3"} <= state.graph.keys()


@pytest.mark.usefixtures("isolated_molt_cache")
def test_runtime_custody_continuation_keeps_its_catalog_modules(
    tmp_path: Path,
) -> None:
    """HF-926: the runtime-import custody catalog is built from the published
    graph. When the custody call's own scans supersede, its replay rebuilds
    the earlier call under the wider premise, which loses `leaf` (`owner`
    rebinds `m.NAME`, so `m` may import another name after `import hook`).
    That replay cannot reproduce the published graph the catalog names, so
    the call keeps its accumulated outputs instead of refusing the build
    ("runtime catalog lacks live source discovery")."""
    from molt.cli.models import _RuntimeImportScanCustody

    sources = {
        "entry": "import m\n",
        "m": "import importlib\nNAME = 'leaf'\nimport hook\nimportlib.import_module(NAME)\n",
        "hook": "VALUE = 0\n",
        "owner": "import m\nm.NAME = 'other'\n",
        "leaf": "VALUE = 1\n",
        "other": "VALUE = 2\n",
    }
    paths = {}
    for name, source in sources.items():
        paths[name] = (tmp_path / f"{name}.py").resolve()
        paths[name].write_text(source, encoding="utf-8")
    common = ([tmp_path], [tmp_path], tmp_path / "stdlib", tmp_path, set())
    first = module_graph_discovery._discover_module_graph_from_paths(
        (paths["entry"],), *common, full_scan_roots=True
    )
    state = first.discovery
    assert state is not None and "leaf" in state.graph
    custody = _RuntimeImportScanCustody(
        (("owner", paths["owner"]),),
        (("leaf", paths["leaf"]), ("owner", paths["owner"])),
        (
            (
                "owner",
                state.resolver_cache.module_ast_digest(
                    paths["owner"], native_roots=(), target_python=state.target_python
                ),
            ),
        ),
    )
    module_graph_discovery._discover_module_graph_from_paths(
        (paths["owner"],),
        *common,
        runtime_import_custody=custody,
        full_scan_roots=True,
        discovery=state,
    )
    assert {"leaf", "owner"} <= state.graph.keys()
    assert state.superseded_scans and state.accumulated


@pytest.mark.usefixtures("isolated_molt_cache")
def test_early_cutoff_alias_serves_the_equal_scan_for_its_source_only(
    tmp_path: Path,
) -> None:
    """HF-926: a scan whose premise answers hold under a new view publishes
    a key row that aliases its stored result. A warm read of the new view
    gets that result; a changed source refuses it; an alias never replaces a
    stored scan of its own view."""
    from molt.cli import module_resolution
    from molt.cli import module_source
    from molt.compiler_analysis.python_identity_catalog_generated import (
        NO_INVALID_MEMBERS,
    )
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = "import os\ndef helper():\n    return 1\nVALUE = 1\n"
    path = tmp_path / "mod.py"
    path.write_text(source, encoding="utf-8")
    snapshot = module_source.PythonSourceSnapshot.capture(path)
    base = PythonNamespacePremise(
        (), NO_INVALID_MEMBERS, NO_INVALID_MEMBERS, (), opaque_activation_sources=()
    )
    # Unknown code may call ``helper``; the module's own code reads nothing
    # that changes. A write of ``VALUE`` does change what it reads.
    helper = python_node_source_key(ast.parse(source).body[1])
    widened = replace(base, opaque_activation_sources=(helper,))
    common = {
        "module_name": "mod",
        "is_package": False,
        "import_scan_mode": "module_init",
        "purpose": SourceImportPurpose.EXECUTION,
    }
    loaded = module_graph_discovery._load_module_import_scan(
        path,
        **common,
        resolution_cache=module_resolution._ModuleResolutionCache(),
        project_root=tmp_path,
        source_snapshot=snapshot,
        namespace_premise=base,
        record_premise_reads=True,
    )
    cutoff = loaded.premise_reads
    assert cutoff is not None and cutoff.publish is not None
    assert cutoff.unchanged_under(widened)
    assert not cutoff.unchanged_under(replace(base, foreign_writes=("VALUE",)))

    def read(
        premise: PythonNamespacePremise,
        source_snapshot: module_source.PythonSourceSnapshot = snapshot,
    ) -> object:
        return module_graph_cache._read_persisted_import_scan_record(
            path,
            **common,
            namespace_premise=cutoff.view(premise),
            snapshot=source_snapshot,
        )

    assert read(widened) is None
    cutoff.publish(widened)
    assert read(widened) == loaded.scan.source_requests
    # An alias for the scanned view itself is never written over its scan.
    cutoff.publish(base)
    assert read(base) == loaded.scan.source_requests
    path.write_text(source.replace("VALUE = 1", "VALUE = 2"), encoding="utf-8")
    changed = module_source.PythonSourceSnapshot.capture(path)
    assert read(widened, changed) is None


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    ("profile", "replace_provider", "possible"),
    [
        ("micro", False, False),
        ("full", False, True),
        ("auto", False, True),
        ("micro", True, True),
    ],
)
def test_archive_normal_result_controls_only_exact_calls(
    tmp_path: Path, profile: str, replace_provider: bool, possible: bool
) -> None:
    from molt.cli.module_resolution import _ModuleResolutionCache
    from molt.cli.runtime_features import runtime_normal_none_intrinsics
    from molt.compilation_target import TargetKind
    from molt.compiler_analysis.python_binding_flow import (
        PythonBindingPolicy,
        analyze_python_bindings,
    )
    from molt.compiler_analysis.python_namespace_graph import (
        python_namespace_binding_policy,
    )
    from molt.compiler_analysis.python_source_keys import python_ast_digest
    from molt.compiler_analysis.python_effects_generated import RAISES

    entry = tmp_path / "entry.py"
    entry.write_text(
        "from _intrinsics import require_intrinsic\n"
        "probe = require_intrinsic('molt_importlib_zip_archive_path')\n"
        + ("if input():\n    probe = lambda path: path\n" if replace_provider else "")
        + "result = probe('example.zip')\n"
        "if result is not None:\n    import archive_branch\n",
        encoding="utf-8",
    )
    (tmp_path / "archive_branch.py").write_text("VALUE = 1\n", encoding="utf-8")
    result = module_graph_discovery._discover_module_graph_from_paths(
        (entry,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        resolver_cache=_ModuleResolutionCache(
            normal_none_intrinsics=runtime_normal_none_intrinsics(
                TargetKind.NATIVE, profile
            )
        ),
        full_scan_roots=True,
    )
    assert ("archive_branch" in result.graph) is possible
    premise = result.scan_authority.by_module["entry"].namespace_premise
    assert premise is not None
    assert (
        module_graph_cache._decode_namespace_premise_cache_identity(
            module_graph_cache._namespace_premise_cache_identity(premise)
        )
        == premise
    )
    tree = ast.parse(entry.read_text(encoding="utf-8"))
    index = analyze_python_bindings(
        tree,
        source_digest=python_ast_digest(tree),
        policy=python_namespace_binding_policy(
            PythonBindingPolicy(), premise, lowering=True
        ),
    )
    call = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "probe"
    )
    fact = index.expression_fact(call)
    assert fact is not None and fact.effects & RAISES
    if not possible:
        assert fact.result.value_known and fact.result.value is None
        assert fact.result.evaluation_required


@pytest.mark.usefixtures("isolated_molt_cache")
def test_archive_feature_switch_separates_persisted_scan_results(
    tmp_path: Path,
) -> None:
    from molt.cli import module_resolution, module_source
    from molt.cli.runtime_features import runtime_normal_none_intrinsics
    from molt.compilation_target import TargetKind
    from molt.compiler_analysis.python_identity_catalog_generated import (
        NO_INVALID_MEMBERS,
    )
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    path = tmp_path / "entry.py"
    path.write_text(
        "from _intrinsics import require_intrinsic\n"
        "probe = require_intrinsic('molt_importlib_zip_archive_path')\n"
        "if probe('x.zip') is not None:\n    import archive_branch\n",
        encoding="utf-8",
    )
    snapshot = module_source.PythonSourceSnapshot.capture(path)
    base = PythonNamespacePremise(
        (), NO_INVALID_MEMBERS, NO_INVALID_MEMBERS, (), opaque_activation_sources=()
    )

    def profile_premise(profile: str) -> PythonNamespacePremise:
        return replace(
            base,
            normal_none_intrinsics=runtime_normal_none_intrinsics(
                TargetKind.NATIVE, profile
            ),
        )

    def scan(
        premise: PythonNamespacePremise,
    ) -> module_graph_discovery._LoadedModuleImportScan:
        return module_graph_discovery._load_module_import_scan(
            path,
            module_name="entry",
            is_package=False,
            import_scan_mode="module_init",
            purpose=SourceImportPurpose.EXECUTION,
            source_snapshot=snapshot,
            namespace_premise=premise,
            project_root=tmp_path,
            resolution_cache=module_resolution._ModuleResolutionCache(),
            record_premise_reads=True,
        )

    micro, full = profile_premise("micro"), profile_premise("full")
    restricted, general = scan(micro), scan(full)
    assert not restricted.cache_hit and not general.cache_hit
    assert restricted.premise_reads is not None and general.premise_reads is not None
    assert not restricted.premise_reads.unchanged_under(full)
    assert not general.premise_reads.unchanged_under(micro)
    assert restricted.scan.source_requests != general.scan.source_requests
    for premise, expected in ((micro, restricted), (full, general)):
        cached = scan(premise)
        assert cached.cache_hit
        assert cached.scan.source_requests == expected.scan.source_requests


@pytest.mark.usefixtures("isolated_molt_cache")
@pytest.mark.parametrize(
    "case",
    [
        "ordinary",
        "import_alias",
        "effectful_body",
        "provider_rebind",
        "consumer_rebind",
        "reflected_write",
        "partial_import",
        "decorator",
        "same_statement_writer",
        "unknown_result",
        "cache_redirect",
        "callee_before_arguments",
        "late_writer",
    ],
)
def test_qualified_argument_exposure_follows_publication(
    tmp_path: Path, case: str
) -> None:
    # Box has ordinary construction and one observable member. Passing an
    # instance to an exhaustively bound source function transfers it to its
    # formal. Every other callable can publish the object or shadow its member.
    provider = (
        "class Box:\n    def __init__(self, n): self.n = n\n    def value(self):\n        import used_leaf\n        return self.n\n"
        "def make(): return Box(7)\n"
        "def inspect_box(obj): return obj.value()\n"
    )
    entry = "import lib as api\ndef run(): return api.inspect_box(api.make())\nrun()\n"
    if case == "ordinary":
        entry = entry.replace("lib as api", "lib").replace("api.", "lib.")
    if case == "effectful_body":
        provider = provider.replace("return obj.value()", "return obj.__dict__")
    elif case == "provider_rebind":
        provider += "inspect_box = print\n"
    elif case == "consumer_rebind":
        entry = entry.replace(
            "run()\n",
            "api = type('Other', (), {'make': api.make, 'inspect_box': print})\nrun()\n",
        )
    elif case == "reflected_write":
        entry = entry.replace("run()\n", "vars(api)['inspect_box'] = print\nrun()\n")
    elif case == "partial_import":
        provider = provider.replace(
            "def inspect_box",
            "def __getattr__(name): return print\nimport cycle\ndef inspect_box",
        )
        (tmp_path / "cycle.py").write_text(
            "import lib\nlib.inspect_box(lib.make())\n", encoding="utf-8"
        )
    elif case == "decorator":
        provider = provider.replace(
            "def inspect_box",
            "def decorate(f): return print\n@decorate\ndef inspect_box",
        )
    elif case == "same_statement_writer":
        provider = provider.replace(
            "inspect_box(obj)", "inspect_box(obj, hidden=(inspect_box := print))"
        )
    elif case == "unknown_result":
        provider += "if __name__ != 'hidden': inspect_box = type('Callable', (), {'__call__': print})()\n"
    elif case == "cache_redirect":
        entry = "import sys\n" + entry.replace(
            "def run", "sys.modules['lib'] = api\ndef run"
        )
    elif case == "callee_before_arguments":
        entry = (
            "import lib as api\n"
            "def replace_target():\n    api.inspect_box = print\n    return api.make()\n"
            "api.inspect_box(replace_target())\n"
        )
    (tmp_path / "used_leaf.py").write_text("", encoding="utf-8")
    (tmp_path / "lib.py").write_text(provider, encoding="utf-8")
    root = tmp_path / "entry.py"
    root.write_text(entry, encoding="utf-8")
    key = python_node_source_key(ast.parse(provider).body[0])

    def discover(
        path: Path,
        continuation: module_graph_discovery._ModuleGraphDiscovery | None = None,
    ) -> _DiscoveredModuleGraph:
        return module_graph_discovery._discover_module_graph_from_paths(
            (path,),
            [tmp_path],
            [tmp_path],
            tmp_path / "stdlib",
            None,
            set(),
            full_scan_roots=False,
            discovery=continuation,
        )

    first = discover(root)

    def opaque(result: _DiscoveredModuleGraph) -> bool:
        assert result.discovery is not None
        premise = result.discovery.scan_sources["lib"].namespace_premise
        assert premise is not None and premise.opaque_activation_sources is not None
        return key in premise.opaque_activation_sources

    assert opaque(first) == (case not in {"ordinary", "import_alias", "late_writer"})
    if case in {"ordinary", "import_alias"}:
        assert "used_leaf" in first.graph
    if case in {"ordinary", "reflected_write"}:
        from molt.frontend import SimpleTIRGenerator

        frontend = SimpleTIRGenerator(
            module_name="lib",
            namespace_premise=first.scan_authority.by_module["lib"].namespace_premise,
        )
        frontend.visit(ast.parse(provider))
        factory = {op.kind for op in frontend.funcs_map["py.lib.make"]["ops"]}
        assert ("OBJECT_NEW_BOUND" in factory, "CALL_BIND" in factory) == (
            case == "ordinary",
            case == "reflected_write",
        )
    if case == "late_writer":
        writer = tmp_path / "writer.py"
        writer.write_text("import lib\nlib.inspect_box = print\n", encoding="utf-8")
        assert opaque(discover(writer, first.discovery))
