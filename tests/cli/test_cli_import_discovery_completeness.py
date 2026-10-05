from __future__ import annotations

import ast
import json
import sys
from types import ModuleType
from pathlib import Path
from dataclasses import replace

import pytest

from molt.cli import module_graph_discovery, module_import_scanner
from molt.cli import python_import_resolution as resolution
from molt.cli import python_source_closure as closure
from molt.compiler_analysis.python_imports import (
    INVALID_VALUE,
    UNKNOWN_VALUE,
    ModuleImportContext,
    ModuleImportState,
    StaticImportRequest,
    StaticMetadataValue,
    UnresolvedStaticImportError,
    static_import_discovery,
)
from molt.compiler_analysis import python_binding_flow
from molt.target_python import TargetPythonVersion


pytestmark = pytest.mark.usefixtures("isolated_molt_cache")


@pytest.mark.parametrize("eager", [False, True])
def test_partial_statement_analysis_uses_shared_policy_validation(
    tmp_path: Path, eager: bool
) -> None:
    path = tmp_path / "entry.py"
    path.write_text("import os\nfrom . import child\n", encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    snapshot = resolver.capture_source(path)
    owner = resolution.LocalPythonModuleSource("pkg.entry", path)
    candidate_policy = resolution.PythonImportPolicy(
        eager, False, False, purpose="source_dependency"
    )
    analysis = resolution.analyze_local_imports(snapshot, owner, candidate_policy)
    assert len(analysis.unresolved_dynamic_imports) == 1
    assert "pkg.child" in {
        name for row in analysis.discovery_requests for name in row.candidates
    }
    with pytest.raises(ValueError, match="non-literal or invalid import package"):
        resolution.analyze_local_imports(
            snapshot,
            owner,
            replace(candidate_policy, fail_on_nonliteral_dynamic_import=True),
        )


def _mixed_source() -> str:
    return (
        "if flag is None:\n"
        "    __package__ = 'pkg.alt'\n"
        "else:\n"
        "    __package__ = choose_package()\n"
        "from . import child\n"
    )


@pytest.mark.parametrize("other_kind", ["known", "unknown", "invalid"])
def test_discovery_preserves_source_candidates_and_reports_completeness(
    other_kind: str,
) -> None:
    context = ModuleImportContext("pkg.entry", False)
    known = context.with_state(
        ModuleImportState(
            StaticMetadataValue.known("pkg.alt"),
            StaticMetadataValue.known("pkg.alt"),
            StaticMetadataValue.known("pkg.alt.entry"),
            False,
        )
    )
    value = {
        "known": StaticMetadataValue.known("pkg"),
        "unknown": UNKNOWN_VALUE,
        "invalid": INVALID_VALUE,
    }[other_kind]
    other = context.with_state(ModuleImportState(value, value, value, False))
    discovery = static_import_discovery(
        StaticImportRequest.statement("", level=1, fromlist=("child",)),
        (known, other),
    )
    expected = {"pkg.alt", "pkg.alt.child"}
    if other_kind == "known":
        expected.update({"pkg", "pkg.child"})
    assert set(discovery.source_modules) == expected
    assert discovery.source_complete is (other_kind == "known")
    assert set(discovery.lexical_modules) == (
        {"pkg", "pkg.child"} if other_kind == "unknown" else set()
    )
    assert set(discovery.modules) == expected | set(discovery.lexical_modules)


@pytest.mark.parametrize("scan_mode", ["full", "module_init"])
def test_mixed_source_discovery_does_not_grant_runtime_custody(
    scan_mode: str,
) -> None:
    tree = ast.parse(_mixed_source())
    projection = module_import_scanner._collect_imports_for_graph(
        tree, "pkg.entry", import_scan_mode=scan_mode
    )
    assert set(projection.dynamic_relative_import_candidates) == {
        "pkg",
        "pkg.child",
        "pkg.alt",
        "pkg.alt.child",
    }
    assert "pkg.alt.child" not in projection.imports
    assert projection.requires_runtime_package_anchor
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(
            tree, "pkg.entry", import_scan_mode=scan_mode
        )


def test_partial_local_source_groups_keep_the_unknown_contract(tmp_path: Path) -> None:
    path = tmp_path / "pkg" / "entry.py"
    path.parent.mkdir()
    path.write_text(_mixed_source(), encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    snapshot = resolver.capture_source(path)
    owner = resolution.LocalPythonModuleSource("pkg.entry", path)
    policy = resolution.PythonImportPolicy(
        False, False, True, purpose="source_dependency"
    )
    with pytest.raises(ValueError):
        resolution.analyze_local_imports(snapshot, owner, policy)
    with pytest.raises(ValueError, match="manifest drift"):
        resolution.analyze_local_imports(
            snapshot, owner, policy, expected_nonliteral_dynamic_imports=0
        )
    analysis = resolution.analyze_local_imports(
        snapshot, owner, policy, expected_nonliteral_dynamic_imports=1
    )
    assert len(analysis.unresolved_dynamic_imports) == 1
    assert [request.candidates for request in analysis.discovery_requests] == [
        ("pkg.alt", "pkg.alt.child")
    ]
    assert not analysis.requests
    semantic = resolution.PythonImportPolicy(False, False, False)
    assert not analysis.requests_for(semantic)


def test_complete_callback_free_branches_keep_separate_owner_groups(
    tmp_path: Path,
) -> None:
    path = tmp_path / "entry.py"
    path.write_text(
        "if flag is None:\n    __package__ = 'left'\n"
        "else:\n    __package__ = 'right'\n"
        "from . import child\n",
        encoding="utf-8",
    )
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    analysis = resolution.analyze_local_imports(
        resolver.capture_source(path),
        resolution.LocalPythonModuleSource("entry", path),
        resolution.PythonImportPolicy(False, False, True, purpose="source_dependency"),
    )
    assert {request.candidates for request in analysis.requests} == {
        ("left", "left.child"),
        ("right", "right.child"),
    }
    assert not analysis.discovery_requests
    assert not analysis.unresolved_dynamic_imports


@pytest.mark.parametrize(
    "write",
    [
        "__package__ = 'pkg.alt'",
        "__package__: str = 'pkg.alt'",
        "(__package__ := 'pkg.alt')",
        "__package__, extra = ('pkg.alt', 1)",
        "__package__ = 'pkg.alt'\nextra = 1\ndel extra",
        "__package__ = 'pkg.alt'\nitems.attribute = 1",
        "__package__ = 'pkg.alt'\nwith manager:\n    pass",
        "__package__ = 'pkg.alt'\nfor item in items:\n    pass",
    ],
)
def test_source_store_candidates_do_not_replace_execution_metadata(
    tmp_path: Path, write: str
) -> None:
    source = "import os\n" + write + "\nfrom . import child\n"
    tree = ast.parse(source)
    policy = python_binding_flow.PythonBindingPolicy(module_name="pkg.entry")
    semantic = python_binding_flow.analyze_python_source_bindings(source, policy=policy)
    discovery = python_binding_flow.analyze_python_source_bindings(
        source, policy=replace(policy, include_import_discovery=True)
    )
    request_node = tree.body[-1]
    assert semantic.expressions is discovery.expressions
    assert semantic.statements is discovery.statements
    assert semantic.module_import_flow.states_for(request_node) == (
        discovery.module_import_flow.states_for(request_node)
    )
    with pytest.raises(ValueError, match="not requested"):
        semantic.module_import_flow.source_states_for(request_node)
    assert {
        state.package.value
        for state in discovery.module_import_flow.source_states_for(request_node)
    } == {"pkg.alt", None}
    fact = discovery.statement_fact(request_node)
    assert fact is not None
    assert not fact.module_metadata_at_entry.admits_current_namespace
    projection = module_import_scanner._collect_imports_for_graph(tree, "pkg.entry")
    assert {"pkg.alt", "pkg.alt.child"} <= set(
        projection.dynamic_relative_import_candidates
    )
    assert "pkg.alt.child" not in projection.imports
    assert projection.requires_runtime_package_anchor
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(tree, "pkg.entry")

    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    owner = resolution.LocalPythonModuleSource("pkg.entry", path)
    snapshot = resolver.capture_source(path)
    analysis = resolution.analyze_local_imports(
        snapshot,
        owner,
        resolution.PythonImportPolicy(False, False, True, purpose="source_dependency"),
        expected_nonliteral_dynamic_imports=1,
    )
    assert [row.candidates for row in analysis.discovery_requests] == [
        ("pkg.alt", "pkg.alt.child")
    ]
    assert len(analysis.unresolved_dynamic_imports) == 1
    with pytest.raises(ValueError):
        resolution.analyze_local_imports(
            snapshot, owner, resolution.PythonImportPolicy(False, False, True)
        )


@pytest.mark.parametrize("unknown_branch", [False, True])
@pytest.mark.parametrize(
    "import_statement",
    [
        "from . import child",
        "import_module('.child', __package__)",
        "__import__('child', {'__package__': __package__}, level=1)",
    ],
)
def test_callback_exposed_branch_candidates_keep_unknown_sibling_obligations(
    tmp_path: Path, import_statement: str, unknown_branch: bool
) -> None:
    other = "choose_package()" if unknown_branch else "'pkg.other'"
    source = (
        "from importlib import import_module\nimport os\n"
        "if flag is None:\n    __package__ = 'pkg.alt'\n"
        f"else:\n    __package__ = {other}\n" + import_statement + "\n"
    )
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    analysis = resolution.analyze_local_imports(
        resolver.capture_source(path),
        resolution.LocalPythonModuleSource("pkg.entry", path),
        resolution.PythonImportPolicy(False, False, True, purpose="source_dependency"),
        expected_nonliteral_dynamic_imports=1,
    )
    candidates = {
        name for row in analysis.discovery_requests for name in row.candidates
    }
    assert "pkg.alt.child" in candidates
    assert ("pkg.other.child" in candidates) is (not unknown_branch)
    assert "pkg.child" not in candidates
    assert len(analysis.unresolved_dynamic_imports) == 1
    assert all("pkg.alt.child" not in row.candidates for row in analysis.requests)
    with pytest.raises(ValueError, match="manifest drift"):
        resolution.analyze_local_imports(
            resolver.capture_source(path),
            resolution.LocalPythonModuleSource("pkg.entry", path),
            resolution.PythonImportPolicy(
                False, False, True, purpose="source_dependency"
            ),
            expected_nonliteral_dynamic_imports=0,
        )
    projection = module_import_scanner._collect_imports_for_graph(
        ast.parse(source), "pkg.entry"
    )
    assert "pkg.alt.child" in projection.dynamic_relative_import_candidates
    assert "pkg.child" in projection.dynamic_relative_import_candidates
    assert "pkg.alt.child" not in projection.imports
    assert projection.requires_runtime_package_anchor


def test_partial_source_cache_retains_candidates_and_diagnostics(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    package = tmp_path / "src" / "pkg"
    alternate = package / "alt"
    alternate.mkdir(parents=True)
    seed, child, lexical = (
        package / "entry.py",
        alternate / "child.py",
        package / "child.py",
    )
    seed.write_text(_mixed_source(), encoding="utf-8")
    for path in (child, lexical, package / "__init__.py", alternate / "__init__.py"):
        path.write_text("", encoding="utf-8")
    source = resolution.PythonImportPolicy(
        False, False, False, purpose="source_dependency"
    )
    semantic = resolution.PythonImportPolicy(False, False, False)

    def check() -> None:
        for policy in (source, semantic):
            paths = set(
                closure.local_python_import_closure(
                    tmp_path, (seed,), policy=policy
                ).paths
            )
            assert (child in paths) is (policy.purpose == "source_dependency")
            assert lexical not in paths

    check()
    cache = json.loads(closure.python_source_closure_cache_path(tmp_path).read_text())
    variants = cache["entries"]["src/pkg/entry.py"]
    source_key = closure._analysis_policy_digest("pkg.entry", False, source)
    semantic_key = closure._analysis_policy_digest("pkg.entry", False, semantic)
    assert variants[source_key]["discovery_requests"]
    assert variants[semantic_key]["discovery_requests"] == []
    assert len(variants[source_key]["unresolved_dynamic_imports"]) == 1
    assert len(variants[semantic_key]["unresolved_dynamic_imports"]) == 1
    monkeypatch.setattr(
        closure,
        "analyze_local_imports",
        lambda *_args, **_kwargs: pytest.fail("unchanged source missed its cache"),
    )
    check()


def test_product_graph_retains_known_and_lexical_candidates(tmp_path: Path) -> None:
    package = tmp_path / "pkg"
    alternate = package / "alt"
    alternate.mkdir(parents=True)
    seed = package / "entry.py"
    seed.write_text(_mixed_source(), encoding="utf-8")
    for path in (
        package / "__init__.py",
        package / "child.py",
        alternate / "__init__.py",
        alternate / "child.py",
    ):
        path.write_text("", encoding="utf-8")
    discovered = module_graph_discovery._discover_module_graph_from_paths(
        (seed,),
        [tmp_path],
        [tmp_path],
        tmp_path / "stdlib",
        None,
        set(),
        full_scan_roots=False,
    )
    assert {"pkg", "pkg.child", "pkg.alt", "pkg.alt.child"} <= set(discovered.graph)
    assert discovered.scan_authority.by_module[
        "pkg.entry"
    ].requires_runtime_package_anchor


@pytest.mark.parametrize("unknown_branch", [False, True])
def test_captured_source_alternatives_survive_later_metadata_deletion(
    tmp_path: Path, unknown_branch: bool
) -> None:
    other = "choose_package()" if unknown_branch else "'pkg.second'"
    source = (
        "import os\n"
        "if flag is None:\n    __package__ = 'pkg.alt'\n"
        f"else:\n    __package__ = {other}\n"
        "__import__('child', {'__package__': __package__, '__name__': 'wrong.mod'}, "
        "globals().__delitem__('__package__'), [], 1)\n"
    )
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    analysis = resolution.analyze_local_imports(
        resolver.capture_source(path),
        resolution.LocalPythonModuleSource("pkg.entry", path),
        resolution.PythonImportPolicy(False, False, True, purpose="source_dependency"),
        expected_nonliteral_dynamic_imports=1,
    )
    candidates = {
        name for row in analysis.discovery_requests for name in row.candidates
    }
    assert "pkg.alt.child" in candidates
    assert ("pkg.second.child" in candidates) is (not unknown_branch)
    assert "wrong.child" not in candidates
    assert len(analysis.unresolved_dynamic_imports) == 1
    assert all("pkg.alt.child" not in row.candidates for row in analysis.requests)
    projection = module_import_scanner._collect_imports_for_graph(
        ast.parse(source), "pkg.entry"
    )
    assert "pkg.alt.child" in projection.dynamic_relative_import_candidates
    assert "wrong.child" not in projection.dynamic_relative_import_candidates
    assert "pkg.alt.child" not in projection.imports


@pytest.fixture
def import_packages(monkeypatch: pytest.MonkeyPatch) -> None:
    # Real CPython imports choose the package; no replacement import hook decides
    # the expected package on behalf of the code under test.
    for name in ("pkg", "pkg.alt", "pkg.other"):
        package = ModuleType(name)
        package.__path__ = []
        child = ModuleType(f"{name}.child")
        package.child = child
        monkeypatch.setitem(sys.modules, name, package)
        monkeypatch.setitem(sys.modules, child.__name__, child)
    sys.modules["pkg"].alt = sys.modules["pkg.alt"]
    sys.modules["pkg"].other = sys.modules["pkg.other"]


_CALLBACK_SOURCES = (
    (
        "class Switch:\n"
        "    def __del__(self):\n"
        "        global __package__\n"
        "        __package__ = 'pkg.other'\n"
        "token = Switch()\n"
        "__package__ = 'pkg.alt'\n"
        "token = None\n"
    ),
    (
        "class Switch:\n"
        "    def __set__(self, owner, value):\n"
        "        global __package__\n"
        "        __package__ = 'pkg.other'\n"
        "class Owner:\n"
        "    field = Switch()\n"
        "owner = Owner()\n"
        "__package__ = 'pkg.alt'\n"
        "owner.field = None\n"
    ),
    (
        "class Switch:\n"
        "    def __enter__(self):\n"
        "        global __package__\n"
        "        __package__ = 'pkg.other'\n"
        "    def __exit__(self, *args):\n"
        "        pass\n"
        "__package__ = 'pkg.alt'\n"
        "with Switch():\n"
        "    pass\n"
    ),
    (
        "class Switch:\n"
        "    def __iter__(self):\n"
        "        return self\n"
        "    def __next__(self):\n"
        "        global __package__\n"
        "        __package__ = 'pkg.other'\n"
        "        raise StopIteration\n"
        "__package__ = 'pkg.alt'\n"
        "for item in Switch():\n"
        "    pass\n"
    ),
)


@pytest.mark.parametrize("callback_source", _CALLBACK_SOURCES)
@pytest.mark.parametrize(
    "import_statement",
    [
        "from . import child as loaded",
        "loaded = import_module('.child', __package__)",
        "loaded = __import__('child', {'__package__': __package__}, level=1)",
    ],
)
def test_callback_candidates_keep_cpython_unknown_import_obligation(
    tmp_path: Path,
    import_packages: None,
    callback_source: str,
    import_statement: str,
) -> None:
    source = (
        "from importlib import import_module\n"
        + callback_source
        + import_statement
        + "\n"
    )
    namespace = {"__name__": "pkg.entry", "__package__": "pkg"}
    exec(compile(source, "<callback-import>", "exec"), namespace)
    assert namespace["loaded"] is sys.modules["pkg.other.child"]
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    snapshot = resolver.capture_source(path)
    owner = resolution.LocalPythonModuleSource("pkg.entry", path)
    policy = resolution.PythonImportPolicy(
        False, False, True, purpose="source_dependency"
    )
    analysis = resolution.analyze_local_imports(
        snapshot, owner, policy, expected_nonliteral_dynamic_imports=1
    )
    assert len(analysis.unresolved_dynamic_imports) == 1
    assert "pkg.alt.child" in {
        name for row in analysis.discovery_requests for name in row.candidates
    }
    for eager in (False, True):
        with pytest.raises(ValueError):
            resolution.analyze_local_imports(
                snapshot, owner, replace(policy, module_level_only=eager)
            )
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(ast.parse(source), "pkg.entry")


@pytest.mark.parametrize(
    "operation, published",
    [
        ("globals().__setitem__('__package__', 'pkg.alt')", "pkg.alt"),
        ("globals().__delitem__('__package__')", None),
        ("globals().__setitem__('token', None)", "pkg.alt"),
        ("globals().__delitem__('token')", "pkg.alt"),
    ],
)
def test_globals_release_keeps_published_candidate_and_callback_obligation(
    tmp_path: Path,
    import_packages: None,
    operation: str,
    published: str | None,
) -> None:
    metadata_key = "'__package__'" in operation
    setup = (
        "__package__ = Switch()\n"
        if metadata_key
        else "token = Switch()\n__package__ = 'pkg.alt'\n"  # secret-guard: allow
    )
    source = (
        "class Switch:\n"
        "    def __del__(self):\n"
        "        global __package__\n"
        "        observed.append(globals().get('__package__'))\n"
        "        __package__ = 'pkg.other'\n"
        + setup
        + operation
        + "\nfrom . import child as loaded\n"
    )
    observed = []
    namespace = {"__name__": "pkg.entry", "__package__": "pkg", "observed": observed}
    exec(compile(source, "<globals-release-import>", "exec"), namespace)
    assert observed == [published]
    assert namespace["loaded"] is sys.modules["pkg.other.child"]
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    analysis = resolution.analyze_local_imports(
        resolver.capture_source(path),
        resolution.LocalPythonModuleSource("pkg.entry", path),
        resolution.PythonImportPolicy(False, False, True, purpose="source_dependency"),
        expected_nonliteral_dynamic_imports=1,
    )
    assert len(analysis.unresolved_dynamic_imports) == 1
    if published == "pkg.alt":
        assert "pkg.alt.child" in {
            name for row in analysis.discovery_requests for name in row.candidates
        }


def test_possible_globals_operand_keeps_unknown_branch_obligation(
    tmp_path: Path, import_packages: None
) -> None:
    source = (
        "loaded = __import__('child', "
        "globals() if flag else {'__package__': 'pkg.other'}, None, (), 1)\n"
    )
    observed = set()
    for flag in (False, True):
        namespace = {"__name__": "pkg.entry", "__package__": "pkg", "flag": flag}
        exec(compile(source, "<possible-globals-import>", "exec"), namespace)
        observed.add(namespace["loaded"].__name__)
    assert observed == {"pkg.child", "pkg.other.child"}
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    owner = resolution.LocalPythonModuleSource("pkg.entry", path)
    snapshot = resolver.capture_source(path)
    policy = resolution.PythonImportPolicy(
        False, False, True, purpose="source_dependency"
    )
    analysis = resolution.analyze_local_imports(
        snapshot, owner, policy, expected_nonliteral_dynamic_imports=1
    )
    assert len(analysis.unresolved_dynamic_imports) == 1
    assert "pkg.child" in {
        name for row in analysis.discovery_requests for name in row.candidates
    }
    with pytest.raises(ValueError, match="manifest drift"):
        resolution.analyze_local_imports(
            snapshot, owner, policy, expected_nonliteral_dynamic_imports=0
        )
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(ast.parse(source), "pkg.entry")


@pytest.mark.parametrize(
    "namespace_expression, expected_candidate",
    [
        ("{key: 'pkg.other', '__name__': 'pkg.entry'}", "pkg.child"),
        ("{'__package__': 'pkg.alt', key: 'pkg.other'}", "pkg.alt.child"),
        ("{**{key: 'pkg.other'}, '__name__': 'pkg.entry'}", "pkg.child"),
    ],
)
def test_unknown_dictionary_key_keeps_runtime_and_source_obligations(
    tmp_path: Path,
    import_packages: None,
    namespace_expression: str,
    expected_candidate: str,
) -> None:
    class PackageKey:
        def __hash__(self):
            return hash("__package__")

        def __eq__(self, other):
            return other == "__package__"

    source = f"loaded = __import__('child', {namespace_expression}, None, (), 1)\n"
    for key in ("__package__", PackageKey()):
        namespace = {"__name__": "pkg.entry", "__package__": "pkg", "key": key}
        exec(compile(source, "<dictionary-key-import>", "exec"), namespace)
        assert namespace["loaded"] is sys.modules["pkg.other.child"]
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    analysis = resolution.analyze_local_imports(
        resolver.capture_source(path),
        resolution.LocalPythonModuleSource("pkg.entry", path),
        resolution.PythonImportPolicy(False, False, True, purpose="source_dependency"),
        expected_nonliteral_dynamic_imports=1,
    )
    assert len(analysis.unresolved_dynamic_imports) == 1
    assert expected_candidate in {
        name for row in analysis.discovery_requests for name in row.candidates
    }
    assert "pkg.other.child" not in {
        name for row in analysis.requests for name in row.candidates
    }
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(ast.parse(source), "pkg.entry")
    projection = module_import_scanner._collect_imports_for_graph(
        ast.parse(source), "pkg.entry"
    )
    assert projection.requires_runtime_package_anchor
    assert not projection.imports


@pytest.mark.parametrize(
    "setup, namespace_expression, expected",
    [
        (
            "key = '__package__'\n",
            "{key: 'pkg.other', '__name__': 'pkg.entry'}",
            "pkg.other.child",
        ),
        (
            "key = 1\n",
            "{key: 'pkg.other', '__name__': 'pkg.entry'}",
            "pkg.child",
        ),
        (
            "",
            "{key: 'pkg.other', '__package__': 'pkg.alt', '__spec__': None}",
            "pkg.alt.child",
        ),
    ],
)
def test_dictionary_key_facts_and_later_overwrites_preserve_exact_imports(
    import_packages: None,
    setup: str,
    namespace_expression: str,
    expected: str,
) -> None:
    source = setup + (
        f"loaded = __import__('child', {namespace_expression}, None, (), 1)\n"
    )
    namespace = {"__name__": "pkg.entry", "__package__": "pkg", "key": "__package__"}
    exec(compile(source, "<exact-dictionary-key-import>", "exec"), namespace)
    assert namespace["loaded"] is sys.modules[expected]
    assert expected in module_import_scanner._collect_imports(
        ast.parse(source), "pkg.entry"
    )


@pytest.mark.parametrize(
    "prefix",
    [
        "",
        "value = (1, 2)[0]\n",
        "value = [1, 2][-1]\n",
        "value = 'package'[1:4]\n",
        "value = b'package'[0]\n",
        "value = (1, 2)[1:]\n",
        "value = len((1, 2))\n",
    ],
)
def test_callback_free_source_context_needs_no_runtime_catalog(
    tmp_path: Path, prefix: str
) -> None:
    path = tmp_path / "entry.py"
    path.write_text(prefix + "from . import child\n", encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    owner = resolution.LocalPythonModuleSource("pkg.entry", path)
    snapshot = resolver.capture_source(path)
    for purpose in ("semantic", "source_dependency"):
        analysis = resolution.analyze_local_imports(
            snapshot,
            owner,
            resolution.PythonImportPolicy(False, False, True, purpose=purpose),
        )
        assert not analysis.unresolved_dynamic_imports
        assert any("pkg.child" in row.candidates for row in analysis.requests)


@pytest.mark.parametrize("package_expression", ["'pkg.alt'", "chosen"])
def test_exact_captured_scalar_survives_unrelated_namespace_callback(
    tmp_path: Path, import_packages: None, package_expression: str
) -> None:
    source = (
        "chosen = 'pkg.alt'\n"
        "loaded = __import__('child', "
        f"{{'__package__': {package_expression}}}, callback(), (), 1)\n"
    )
    namespace = {"__name__": "pkg.entry", "__package__": "pkg"}

    def callback():
        namespace["__package__"] = "pkg.other"

    namespace["callback"] = callback
    exec(compile(source, "<exact-captured-scalar>", "exec"), namespace)
    assert namespace["loaded"] is sys.modules["pkg.alt.child"]
    assert namespace["__package__"] == "pkg.other"
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    analysis = resolution.analyze_local_imports(
        resolver.capture_source(path),
        resolution.LocalPythonModuleSource("pkg.entry", path),
        resolution.PythonImportPolicy(False, False, True, purpose="source_dependency"),
    )
    assert not analysis.unresolved_dynamic_imports
    assert "pkg.alt.child" in {
        name
        for row in (*analysis.requests, *analysis.discovery_requests)
        for name in row.candidates
    }


@pytest.mark.parametrize(
    "selection",
    [
        "load = (__import__,)[0]",
        "(load,) = (__import__,)",
        "load = [__import__].pop()",
        "load = {}.get('missing', __import__)",
        "for load in (__import__,):\n    pass",
    ],
)
@pytest.mark.parametrize("eager", [False, True])
def test_selected_importer_reaches_actual_dependency_consumer(
    tmp_path: Path, selection: str, eager: bool
) -> None:
    source = selection + "\nloaded = load('math')\n"
    namespace = {}
    exec(compile(source, "<selected-dependency-oracle>", "exec"), namespace)
    assert namespace["loaded"].__name__ == "math"
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    policy = resolution.PythonImportPolicy(
        eager, False, False, purpose="source_dependency"
    )
    for _ in range(2):
        assert "math" in resolution.local_import_targets(path, resolver, policy)


@pytest.mark.parametrize("mode", ["full", "module_init"])
def test_product_scan_keeps_boolean_import_after_index_callback(mode: str) -> None:
    source = (
        "values = [1]\n"
        "def replace():\n    values[0] = 0\n    return 0\n"
        "loaded = values[(replace(), 0)[1]] or __import__('fractions')\n"
    )
    namespace = {}
    exec(compile(source, "<boolean-import-oracle>", "exec"), namespace)
    assert namespace["loaded"].__name__ == "fractions"
    projection = module_import_scanner._collect_imports_for_graph(
        ast.parse(source),
        "entry",
        import_scan_mode=mode,
        target_python=TargetPythonVersion(3, 12, 0),
    )
    assert "fractions" in (
        *projection.imports,
        *projection.dynamic_relative_import_candidates,
    )


@pytest.mark.parametrize("eager", [False, True])
def test_later_comprehension_importer_reaches_dependency_consumer(
    tmp_path: Path, eager: bool
) -> None:
    source = (
        "load = None\n"
        "calls = [(load, (load := __import__))[0] for _ in (0, 1)]\n"
        "loaded = calls[1]('math')\n"
    )
    namespace = {}
    exec(compile(source, "<comprehension-importer-oracle>", "exec"), namespace)
    assert namespace["loaded"].sqrt(81) == 9.0
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    policy = resolution.PythonImportPolicy(
        eager, False, False, purpose="source_dependency"
    )
    for _ in range(2):
        assert "math" in resolution.local_import_targets(path, resolver, policy)


@pytest.mark.parametrize(
    "prefix",
    [
        "g = ((__package__ := 'other') for _ in (0,))\nlist(g)\n",
        "exposed = globals()\ndef rebind():\n    exposed['__package__'] = 'other'\nrebind()\n",
        "exposed = (globals(),)\nconsume(*exposed)\n",
        "consume(*[globals()])\n",
    ],
)
@pytest.mark.parametrize("mode", ["full", "module_init"])
def test_deferred_import_mutation_requires_matching_runtime_catalog(
    tmp_path, prefix, mode
):
    from molt.cli.models import _RuntimeImportScanCustody
    from molt.compiler_analysis.python_source_keys import python_ast_digest

    source = prefix + "from .child import value\n"
    tree = ast.parse(source)
    owner = (tmp_path / "entry.py").resolve()
    owner.write_text(source, encoding="utf-8")
    catalog = [("pkg.entry", owner)]
    for package in ("pkg", "other"):
        child = (tmp_path / (package + "_child.py")).resolve()
        child.write_text("value = " + repr(package) + "\n", encoding="utf-8")
        catalog.append((package + ".child", child))
    custody = _RuntimeImportScanCustody(
        owners=(("pkg.entry", owner),),
        catalog=tuple(catalog),
        owner_ast_digests=(("pkg.entry", python_ast_digest(tree)),),
    )
    projection = module_import_scanner._collect_imports_for_graph(
        tree,
        "pkg.entry",
        import_scan_mode=mode,
    )
    assert projection.requires_runtime_package_anchor
    assert "pkg.child" not in projection.imports
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(tree, "pkg.entry", import_scan_mode=mode)
    if mode != "full":
        with pytest.raises(ValueError, match="full-depth owner scans"):
            module_import_scanner._collect_imports(
                tree,
                "pkg.entry",
                import_scan_mode=mode,
                source_path=owner,
                runtime_import_custody=custody,
            )
    admitted = module_import_scanner._collect_imports(
        tree,
        "pkg.entry",
        import_scan_mode="full",
        source_path=owner,
        runtime_import_custody=custody,
    )
    assert set(custody.modules) <= set(admitted)
    resolver = resolution.LocalPythonModuleResolver((tmp_path,))
    for eager in (False, True):
        analysis = resolution.analyze_local_imports(
            resolver.capture_source(owner),
            resolution.LocalPythonModuleSource("pkg.entry", owner),
            resolution.PythonImportPolicy(
                eager, False, False, purpose="source_dependency"
            ),
        )
        assert analysis.unresolved_dynamic_imports
