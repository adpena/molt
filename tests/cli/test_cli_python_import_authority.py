from __future__ import annotations

import ast
import inspect
from pathlib import Path
from typing import cast

import pytest

import molt.cli.build_inputs as build_inputs
import molt.cli.extension_support as extension_support
import molt.cli.module_graph as module_graph
import molt.cli.module_import_scanner as module_import_scanner
import molt.cli.python_import_resolution as python_import_resolution
import molt.stdlib_intrinsic_policy as stdlib_intrinsic_policy
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION
from molt.cli.python_import_resolution import (
    LocalPythonModuleResolver,
    PythonImportPolicy,
    local_import_targets,
)
from molt.compiler_analysis import python_imports
from molt.compiler_analysis import python_binding_flow
from molt.compiler_analysis import python_effects
from molt.compiler_analysis.python_imports import (
    INVALID_VALUE,
    NONE_VALUE,
    ModuleImportContext,
    ModuleImportState,
    StaticImportRequest,
    StaticMetadataValue,
    UnresolvedStaticImportError,
    analyze_module_import_flow,
    loader_module_import_state,
    dunder_globals_state_from_expression,
    parse_module_spec_parent,
    plan_static_import_request,
    project_static_import_request,
    resolve_relative_import,
)
from molt.frontend.lowering import import_lowering, module_lifecycle
from molt.frontend import SimpleTIRGenerator


_SEMANTIC_DEFINITIONS = (
    "class StaticMetadataValue:",
    "class ModuleImportState:",
    "class ModuleImportContext:",
    "class StaticImportRequest:",
    "class StaticImportCallArguments:",
    "class StaticImportPlan:",
    "class ModuleImportFlow:",
    "def parse_module_spec_parent(",
    "def bind_static_import_call_arguments(",
    "def update_module_import_state(",
    "def analyze_module_import_flow(",
    "def effective_relative_package(",
    "def resolve_relative_import(",
    "def project_static_import_request(",
    "def plan_static_import_request(",
    "def require_static_import_modules(",
)


def test_python_import_semantics_have_one_source_authority() -> None:
    authority_source = inspect.getsource(python_imports)
    consumers = (
        build_inputs,
        extension_support,
        module_graph,
        module_import_scanner,
        python_import_resolution,
        stdlib_intrinsic_policy,
        import_lowering,
        module_lifecycle,
    )
    for definition in _SEMANTIC_DEFINITIONS:
        assert definition in authority_source
        for consumer in consumers:
            assert definition not in inspect.getsource(consumer)


def test_pep695_type_parameter_factory_is_closed_and_fails_unknown_kinds() -> None:
    typing_path = Path(python_imports.__file__).parents[1] / "stdlib" / "typing.py"
    source = typing_path.read_text(encoding="utf-8")
    assert 'if kind == "TypeVar":' in source
    assert 'if kind == "ParamSpec":' in source
    assert 'if kind == "TypeVarTuple":' in source
    assert "unsupported PEP 695 type parameter kind" in source
    assert "Type parameter defaults require target Python 3.13+" in Path(
        module_import_scanner.__file__
    ).parents[1].joinpath("frontend", "lowering", "type_annotations.py").read_text(
        encoding="utf-8"
    )


def _contexts_for(
    source: str,
    *,
    module_name: str = "pkg.entry",
    is_package: bool = False,
) -> tuple[ast.Module, ModuleImportContext, python_imports.ModuleImportFlow]:
    tree = ast.parse(source)
    context = ModuleImportContext(module_name, is_package)
    return tree, context, analyze_module_import_flow(tree, context)


def test_loader_package_precedes_mismatched_source_spec() -> None:
    source = (
        "from importlib.machinery import ModuleSpec\n"
        "__package__ = 'pkg'\n"
        "__spec__ = ModuleSpec('other.pkg.entry', loader=None)\n"
        "from .child import name\n"
    )
    tree, context, flow = _contexts_for(source)
    request = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.level
    )
    state = flow.states_for(request)
    assert len(state) == 1
    resolution = resolve_relative_import("child", 1, context.with_state(state[0]))
    assert resolution.module == "pkg.child"
    assert resolution.requires_runtime


def test_package_none_falls_back_to_valid_module_spec_parent() -> None:
    source = (
        "from importlib.machinery import ModuleSpec\n"
        "__package__ = None\n"
        "__spec__ = ModuleSpec('real.pkg.entry', loader=None)\n"
        "from .child import name\n"
    )
    assert set(
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )
    ) == {
        "importlib.machinery",
        "importlib.machinery.ModuleSpec",
        "real.pkg.child",
        "real.pkg.child.name",
    }


def test_source_order_uses_state_at_each_import(tmp_path: Path) -> None:
    # Identity comparison does not invoke a user truth callback. The imports
    # are on disjoint paths: executing one cannot mutate the other's anchor.
    source = (
        "if condition is None:\n    from .a import x\n"
        "else:\n    __package__ = 'other.pkg'\n    from .b import y\n"
    )
    expected = {"pkg.a", "pkg.a.x", "other.pkg.b", "other.pkg.b.y"}
    tree = ast.parse(source)
    assert (
        set(
            module_import_scanner._collect_imports(
                tree, module_name="pkg.entry", is_package=False
            )
        )
        == expected
    )
    path = tmp_path / "pkg" / "entry.py"
    path.parent.mkdir()
    path.write_text(source, encoding="utf-8")
    assert (
        local_import_targets(
            path,
            LocalPythonModuleResolver((tmp_path,)),
            PythonImportPolicy(False, True, True),
        )
        == expected
    )


def test_module_level_policy_skips_deferred_bodies_without_hiding_module_imports(
    tmp_path: Path,
) -> None:
    path = tmp_path / "entry.py"
    path.write_text(
        "import package.eager\ndef deferred():\n    import package.lazy\n",
        encoding="utf-8",
    )
    resolver = LocalPythonModuleResolver((tmp_path,))

    assert local_import_targets(
        path,
        resolver,
        PythonImportPolicy(True, False, True),
    ) == {"package.eager"}
    assert local_import_targets(
        path,
        resolver,
        PythonImportPolicy(False, False, True),
    ) == {"package.eager", "package.lazy"}


def test_context_independent_dependency_requests_do_not_build_binding_facts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "entry.py"
    path.write_text(
        "import package.eager\nfrom package import member\n"
        "registry = make_registry()\n"
        "def deferred():\n    __import__(unknown)\n",
        encoding="utf-8",
    )

    def forbidden(*args, **kwargs):
        raise AssertionError(
            "absolute module-level requests do not demand binding analysis"
        )

    monkeypatch.setattr(python_import_resolution, "analyze_python_bindings", forbidden)
    assert local_import_targets(
        path,
        LocalPythonModuleResolver((tmp_path,)),
        PythonImportPolicy(True, False, True),
    ) == {"package.eager", "package", "package.member"}


@pytest.mark.parametrize("module_only", [False, True])
def test_relative_dependency_requests_demand_canonical_binding_facts_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, module_only: bool
) -> None:
    path = tmp_path / "pkg" / "entry.py"
    path.parent.mkdir()
    path.write_text(
        "__package__ = 'selected'\nfrom .first import one\nfrom .second import two\n",
        encoding="utf-8",
    )
    analyze = python_import_resolution.analyze_python_bindings
    calls = []

    def record(*args, **kwargs):
        calls.append(kwargs["policy"])
        return analyze(*args, **kwargs)

    monkeypatch.setattr(python_import_resolution, "analyze_python_bindings", record)
    assert local_import_targets(
        path,
        LocalPythonModuleResolver((tmp_path,)),
        PythonImportPolicy(module_only, False, True),
    ) == {
        "selected.first",
        "selected.first.one",
        "selected.second",
        "selected.second.two",
    }
    assert len(calls) == 1
    assert calls[0].analyze_deferred_bodies is not module_only


def test_python_source_snapshot_caches_its_ast_digest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "entry.py"
    path.write_text("from pkg import child\n", encoding="utf-8")
    digest = python_binding_flow.python_ast_digest
    calls = 0

    def record_digest(tree: ast.AST) -> str:
        nonlocal calls
        calls += 1
        return digest(tree)

    monkeypatch.setattr(python_binding_flow, "python_ast_digest", record_digest)
    snapshot = LocalPythonModuleResolver((tmp_path,)).capture_source(path)
    assert snapshot.ast_digest == snapshot.ast_digest
    assert calls == 1


def test_local_resolver_reuses_regular_package_prefix(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    initializer = package / "__init__.py"
    initializer.write_text("VALUE = 1\n", encoding="utf-8")
    first = package / "first.py"
    second = package / "second.py"
    first.write_text("VALUE = 2\n", encoding="utf-8")
    second.write_text("VALUE = 3\n", encoding="utf-8")
    checked: list[Path] = []
    probe_path = LocalPythonModuleResolver._probe_path

    def record_probe_path(self: LocalPythonModuleResolver, candidate: Path):
        checked.append(candidate)
        return probe_path(self, candidate)

    monkeypatch.setattr(LocalPythonModuleResolver, "_probe_path", record_probe_path)
    resolver = LocalPythonModuleResolver((tmp_path,))
    assert [
        source.path
        for source in resolver.resolve_import_sources(
            "pkg.first", include_parent_packages=True
        )
    ] == [initializer.resolve(), first.resolve()]
    prefix_checks = checked.count(package) + checked.count(initializer)
    assert prefix_checks == 2
    assert [
        source.path
        for source in resolver.resolve_import_sources(
            "pkg.second", include_parent_packages=True
        )
    ] == [initializer.resolve(), second.resolve()]
    assert checked.count(package) + checked.count(initializer) == prefix_checks
    assert {"pkg", "pkg.first", "pkg.second"}.issubset(resolver._resolution_cache)


def test_local_resolver_reuses_namespace_package_prefix(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    roots = (tmp_path / "left", tmp_path / "right")
    for root in roots:
        (root / "namespace").mkdir(parents=True)
    first = roots[0] / "namespace" / "first.py"
    second = roots[1] / "namespace" / "second.py"
    first.write_text("VALUE = 1\n", encoding="utf-8")
    second.write_text("VALUE = 2\n", encoding="utf-8")
    checked: list[Path] = []
    probe_path = LocalPythonModuleResolver._probe_path

    def record_probe_path(self: LocalPythonModuleResolver, candidate: Path):
        checked.append(candidate)
        return probe_path(self, candidate)

    monkeypatch.setattr(LocalPythonModuleResolver, "_probe_path", record_probe_path)
    resolver = LocalPythonModuleResolver(roots)
    assert resolver.source_for_module("namespace.first") == first.resolve()
    prefix_checks = tuple(checked.count(root / "namespace") for root in roots)
    assert prefix_checks == (1, 1)
    assert resolver.source_for_module("namespace.second") == second.resolve()
    assert tuple(checked.count(root / "namespace") for root in roots) == prefix_checks
    assert {"namespace", "namespace.first", "namespace.second"}.issubset(
        resolver._resolution_cache
    )


def test_branch_join_preserves_whole_possible_states() -> None:
    source = (
        "if condition is None:\n"
        "    __package__ = 'left'\n"
        "else:\n"
        "    __package__ = 'right'\n"
        "from .child import value\n"
    )
    assert set(
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )
    ) == {"left.child", "left.child.value", "right.child", "right.child.value"}


def test_mixed_static_and_error_paths_are_not_erased() -> None:
    source = (
        "if flag is None:\n"
        "    __package__ = 'left'\n"
        "else:\n"
        "    __package__ = 42\n"
        "from .child import value\n"
    )
    tree, context, flow = _contexts_for(source)
    request_node = next(
        node for node in ast.walk(tree) if isinstance(node, ast.ImportFrom)
    )
    plan = plan_static_import_request(
        StaticImportRequest.statement("child", level=1),
        tuple(context.with_state(state) for state in flow.states_for(request_node)),
    )
    assert plan.modules == ("left.child",)
    assert plan.errors == ("invalid_package",)
    assert plan.requires_runtime_execution
    with pytest.raises(UnresolvedStaticImportError, match="invalid_package"):
        module_import_scanner._collect_imports(
            tree, module_name="pkg.entry", is_package=False
        )


def test_package_spec_warning_path_is_retained_in_import_plan() -> None:
    state = ModuleImportState(
        StaticMetadataValue.known("pkg"),
        StaticMetadataValue.known("other"),
        StaticMetadataValue.known("pkg.entry"),
        False,
    )
    context = ModuleImportContext("pkg.entry", False, state=state)
    plan = plan_static_import_request(
        StaticImportRequest.statement("child", level=1), (context,)
    )
    assert plan.modules == ("pkg.child",)
    assert plan.requires_runtime_execution


def test_unresolved_effect_boundary_requires_explicit_runtime_custody() -> None:
    tree = ast.parse(
        "def mutate():\n"
        "    global __package__\n"
        "    __package__ = 'other'\n"
        "mutate()\n"
        "from .child import value\n"
    )
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(
            tree, module_name="pkg.entry", is_package=False
        )


def test_class_global_metadata_write_updates_module_import_state() -> None:
    source = (
        "class Scope:\n"
        "    global __package__\n"
        "    __package__ = 'other.pkg'\n"
        "from .child import value\n"
    )
    assert set(
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )
    ) == {"other.pkg.child", "other.pkg.child.value"}


def test_try_star_handler_participates_in_import_state_flow() -> None:
    source = (
        "try:\n"
        "    raise ExceptionGroup('group', [ValueError()])\n"
        "except* ValueError:\n"
        "    __package__ = 'other.pkg'\n"
        "from .child import value\n"
    )
    tree, _context, flow = _contexts_for(source)
    request = cast(ast.ImportFrom, tree.body[-1])
    assert flow.states_for(request)
    # ExceptionGroup split/derive and replacement release may run Python;
    # a handler assignment cannot overwrite that uncertainty with a literal.
    assert any(state.package.kind == "unknown" for state in flow.states_for(request))
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(tree, module_name="pkg.entry")


def test_known_package_with_unknown_spec_keeps_graph_root_and_runtime_protocol() -> (
    None
):
    context = ModuleImportContext("pkg.entry", False).with_state(
        ModuleImportState(
            StaticMetadataValue.known("other.pkg"),
            python_imports.UNKNOWN_VALUE,
            python_imports.UNKNOWN_VALUE,
            None,
        )
    )
    plan = plan_static_import_request(
        StaticImportRequest.statement("child", level=1, fromlist=("value",)),
        (context,),
    )
    assert plan.modules == ("other.pkg.child", "other.pkg.child.value")
    assert not plan.requires_runtime
    assert plan.requires_runtime_execution


def test_cpython_retains_explicit_package_before_spec_parent_callback(
    monkeypatch,
) -> None:
    import sys
    from types import ModuleType

    package = ModuleType("_molt_import_protocol_oracle")
    package.__path__ = []
    child = ModuleType("_molt_import_protocol_oracle.child")
    monkeypatch.setitem(sys.modules, package.__name__, package)
    monkeypatch.setitem(sys.modules, child.__name__, child)
    namespace: dict[str, object] = {"__package__": package.__name__}
    events: list[str] = []

    class Spec:
        @property
        def parent(self) -> str:
            events.append("parent")
            namespace["__package__"] = "unavailable.changed"
            return package.__name__

    namespace["__spec__"] = Spec()
    assert __import__("child", namespace, namespace, ("value",), 1) is child
    assert events == ["parent"]
    assert namespace["__package__"] == "unavailable.changed"


def test_match_mapping_rest_capture_invalidates_import_metadata() -> None:
    source = (
        "class Scope:\n"
        "    global __package__\n"
        "    match {'key': 'value'}:\n"
        "        case {**__package__}:\n"
        "            pass\n"
        "from .child import value\n"
    )

    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )


@pytest.mark.parametrize(
    "execution",
    (
        "class Scope(metaclass=mutate()):\n    pass\n",
        "factory = lambda value=mutate(): None\n",
        "callbacks = [mutate]\ncallbacks[0]()\n",
    ),
)
def test_indirect_definition_time_execution_invalidates_import_state(
    execution: str,
) -> None:
    source = (
        "def mutate():\n"
        "    global __package__\n"
        "    __package__ = 'other.pkg'\n"
        f"{execution}"
        "from .child import value\n"
    )
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )


@pytest.mark.parametrize(
    "invocation",
    (
        "require('molt_demo', globals())\n",
        "value = require('molt_demo', globals())\n",
        "if condition:\n    require = replacement\nrequire('molt_demo', globals())\n",
    ),
)
def test_intrinsic_lookup_has_no_implicit_metadata_permission(invocation: str) -> None:
    source = (
        "from _intrinsics import require_intrinsic as require\n"
        f"{invocation}"
        "from .child import value\n"
    )
    # Symbol identity is not a capability to preserve an escaped namespace.
    # Dynamic stdlib imports require the same source-bound custody as any caller.
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )


def test_rebound_intrinsic_lookup_does_not_retain_metadata_capability() -> None:
    source = (
        "from _intrinsics import require_intrinsic as require\n"
        "require = replacement\n"
        "value = require('molt_demo', globals())\n"
        "from .child import value\n"
    )
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )


def test_deferred_relative_import_without_module_name_uses_runtime_transaction() -> (
    None
):
    generator = SimpleTIRGenerator(
        source_path="pkg/module.py",
        module_name="pkg.module",
        module_execution_kind="imported",
        known_modules={"pkg", "pkg.module", "pkg.sibling"},
    )
    generator.visit(
        ast.parse("def load():\n    from . import sibling\n    return sibling\n")
    )

    load_ops = generator.funcs_map["pkg_module__load"]["ops"]
    assert any(op.kind == "CALL_FUNC" for op in load_ops)
    assert any(op.kind == "CONST" and op.args == [1] for op in load_ops)


def test_import_flow_is_cached_and_state_growth_is_bounded() -> None:
    source = "\n".join(
        [f"if p{i}:\n    __package__ = 'p{i}'" for i in range(100)]
        + [f"if n{i}:\n    __name__ = 'n{i}'" for i in range(100)]
        + ["from .child import value"]
    )
    tree = ast.parse(source)
    context = ModuleImportContext("pkg.entry", False)
    flow = analyze_module_import_flow(tree, context)
    request = next(node for node in ast.walk(tree) if isinstance(node, ast.ImportFrom))
    assert len(flow.states_for(request)) <= 64
    assert analyze_module_import_flow(tree, context) is flow


def test_import_flow_cache_invalidates_after_ast_mutation() -> None:
    tree = ast.parse("__package__ = 'a'\nfrom .child import value\n")
    context = ModuleImportContext("pkg.entry", False)
    original = analyze_module_import_flow(tree, context)
    assignment = tree.body[0]
    assert isinstance(assignment, ast.Assign)
    assert isinstance(assignment.value, ast.Constant)
    assignment.value.value = "b"
    updated = analyze_module_import_flow(tree, context)
    request = tree.body[1]
    assert isinstance(request, ast.ImportFrom)
    state = updated.states_for(request)
    assert state[0].package == StaticMetadataValue.known("b")
    assert updated is not original


def test_import_flow_cache_is_content_addressed_across_reparse() -> None:
    source = "__package__ = 'pkg'\nfrom .child import value\n"
    context = ModuleImportContext("pkg.entry", False)
    first_tree = ast.parse(source, filename="first.py")
    second_tree = ast.parse(source, filename="second.py")

    first = analyze_module_import_flow(first_tree, context)
    second = analyze_module_import_flow(second_tree, context)
    second_request = second_tree.body[1]

    assert second is first
    assert second.states_for(second_request)[0].package == StaticMetadataValue.known(
        "pkg"
    )


def test_import_flow_cache_publishes_transitively_immutable_facts() -> None:
    source = "__package__ = 'pkg'\nfrom .child import value\n"
    context = ModuleImportContext("pkg.entry", False)
    first_tree = ast.parse(source)
    first = analyze_module_import_flow(first_tree, context)
    request = first_tree.body[1]
    assert isinstance(request, ast.ImportFrom)
    key = next(iter(first.states_by_node))
    mutable_view = cast(dict[object, object], first.states_by_node)

    with pytest.raises(TypeError):
        mutable_view[key] = ()

    second_tree = ast.parse(source)
    second = analyze_module_import_flow(second_tree, context)
    second_request = second_tree.body[1]
    assert second is first
    assert second.states_for(second_request)[0].package == StaticMetadataValue.known(
        "pkg"
    )


def test_dynamic_import_closure_uses_canonical_binding_facts(tmp_path: Path) -> None:
    cases = {
        "assigned": (
            "import importlib\nload = importlib.import_module\nload('pkg.assigned')\n",
            "pkg.assigned",
        ),
        "module_after_definition": (
            "def load_later():\n"
            "    load('pkg.after')\n"
            "from importlib import import_module as load\n",
            "pkg.after",
        ),
        "enclosing_after_definition": (
            "def outer():\n"
            "    def load_later():\n"
            "        load('pkg.enclosing')\n"
            "    from importlib import import_module as load\n",
            "pkg.enclosing",
        ),
        "class_enclosing_after_definition": (
            "def outer():\n"
            "    class DeferredOwner:\n"
            "        def load_later(self):\n"
            "            load('pkg.class_enclosing')\n"
            "    from importlib import import_module as load\n",
            "pkg.class_enclosing",
        ),
        "global_skips_enclosing": (
            "from importlib import import_module as load\n"
            "def outer():\n"
            "    load = print\n"
            "    def load_global():\n"
            "        global load\n"
            "        load('pkg.global')\n",
            "pkg.global",
        ),
    }
    package = tmp_path / "pkg"
    package.mkdir()
    resolver = LocalPythonModuleResolver((tmp_path,))
    policy = PythonImportPolicy(False, True, True)
    for name, (source, expected) in cases.items():
        path = package / f"{name}.py"
        path.write_text(source, encoding="utf-8")
        assert expected in local_import_targets(path, resolver, policy)

    shadowed = package / "shadowed.py"
    shadowed.write_text(
        "from importlib import import_module as load\n"
        "def local_parameter(load):\n"
        "    load('pkg.not_an_import')\n",
        encoding="utf-8",
    )
    assert "pkg.not_an_import" not in local_import_targets(shadowed, resolver, policy)


def test_type_alias_dynamic_imports_are_lazy_full_graph_edges() -> None:
    source = (
        "type Alias = load('pkg.lazy')\nfrom importlib import import_module as load\n"
    )
    tree = ast.parse(source)
    full = set(
        module_import_scanner._collect_imports(
            tree,
            module_name="pkg.entry",
            import_scan_mode="full",
        )
    )
    module_init = set(
        module_import_scanner._collect_imports(
            tree,
            module_name="pkg.entry",
            import_scan_mode="module_init",
        )
    )

    assert {"typing", "pkg.lazy"} <= full
    assert "typing" in module_init
    assert "pkg.lazy" not in module_init


@pytest.mark.parametrize(
    "source",
    (
        "for item in iterable:\n    pass\nfrom .child import value\n",
        "def decorate(func):\n"
        "    global __package__\n"
        "    __package__ = 'other'\n"
        "    return func\n"
        "@decorate\n"
        "def f():\n"
        "    pass\n"
        "from .child import value\n",
        "def mutate():\n"
        "    global __package__\n"
        "    __package__ = 'other'\n"
        "def f(x: mutate()):\n"
        "    pass\n"
        "from .child import value\n",
    ),
)
def test_python_execution_effects_never_leave_a_stale_static_anchor(
    source: str,
) -> None:
    tree = ast.parse(source)
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(
            tree, module_name="pkg.entry", is_package=False
        )


def test_deferred_function_graph_unions_call_time_module_states() -> None:
    source = "def load():\n    from .child import value\n__package__ = 'other.pkg'\n"
    assert set(
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )
    ) == {
        "pkg.child",
        "pkg.child.value",
        "other.pkg.child",
        "other.pkg.child.value",
    }


def test_deferred_explicit_package_store_keeps_successful_graph_root() -> None:
    source = (
        "def f():\n"
        "    global __package__\n"
        "    __package__ = 'other'\n"
        "    from .child import value\n"
    )
    # The deferred entry makes __spec__ unknown, not the freshly stored exact
    # package string. Its parent callback still requires runtime execution, but
    # CPython retains that package before consulting the spec (oracle above).
    assert set(
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )
    ) == {"other.child", "other.child.value"}


def test_modulespec_signature_and_parent_are_cpython_valid() -> None:
    assert parse_module_spec_parent(
        ast.parse("ModuleSpec('a.b.entry', None)", mode="eval").body,
        {"ModuleSpec"},
    ) == StaticMetadataValue.known("a.b")
    assert parse_module_spec_parent(
        ast.parse("ModuleSpec('a.b', None, is_package=True)", mode="eval").body,
        {"ModuleSpec"},
    ) == StaticMetadataValue.known("a.b")
    assert (
        parse_module_spec_parent(
            ast.parse("ModuleSpec('a.b', None, None, True)", mode="eval").body,
            {"ModuleSpec"},
        )
        == INVALID_VALUE
    )


def test_import_module_explicit_package_never_uses_current_fallback() -> None:
    context = ModuleImportContext("pkg.entry", False)
    missing = project_static_import_request(
        StaticImportRequest.import_module(".child", NONE_VALUE), context
    )
    assert missing.modules == ()
    assert missing.error == "no_parent"
    absolute = project_static_import_request(
        StaticImportRequest.import_module("child", INVALID_VALUE), context
    )
    assert absolute.modules == ("child",)
    assert absolute.error is None


def test_dunder_import_requires_its_own_globals_context() -> None:
    context = ModuleImportContext("pkg.entry", False)
    missing = project_static_import_request(
        StaticImportRequest("dunder_import", "child", level=1), context
    )
    assert missing.error == "missing_globals"
    globals_state = ModuleImportState(
        StaticMetadataValue.known("other.pkg"),
        NONE_VALUE,
        StaticMetadataValue.known("ignored"),
        False,
    )
    resolved = project_static_import_request(
        StaticImportRequest(
            "dunder_import",
            "child",
            level=True,
            fromlist=("name",),
            globals_state=globals_state,
            globals_were_supplied=True,
        ),
        context,
    )
    assert resolved.modules == ("other.pkg.child", "other.pkg.child.name")


def test_dunder_globals_dict_unpack_respects_order_and_unknown_overwrite() -> None:
    context = ModuleImportContext("pkg.entry", False)
    known_overlay = ast.parse(
        "{'__package__': 'a', **{'__package__': 'b'}}", mode="eval"
    ).body
    state = dunder_globals_state_from_expression(known_overlay, context)
    assert state is not None
    assert state.package == StaticMetadataValue.known("b")

    unknown_overlay = ast.parse("{'__package__': 'a', **dynamic}", mode="eval").body
    state = dunder_globals_state_from_expression(unknown_overlay, context)
    assert state is not None
    assert state.package.kind == "unknown"


def test_absolute_import_module_ignores_dynamic_package(tmp_path: Path) -> None:
    source = (
        "from importlib import import_module\nimport_module('pkg.child', object())\n"
    )
    path = tmp_path / "entry.py"
    path.write_text(source, encoding="utf-8")
    expected = {"importlib", "importlib.import_module", "pkg.child"}
    assert set(module_import_scanner._collect_imports(ast.parse(source))) == expected
    assert (
        local_import_targets(
            path,
            LocalPythonModuleResolver((tmp_path,)),
            PythonImportPolicy(False, True, True),
        )
        == expected
    )


def test_stdlib_and_extension_support_share_package_resolution(tmp_path: Path) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    initializer = package / "__init__.py"
    initializer.write_text("from . import child\n", encoding="utf-8")
    (package / "child.py").write_text("VALUE = 1\n", encoding="utf-8")
    assert "pkg.child" in stdlib_intrinsic_policy.stdlib_module_static_imports(
        "pkg",
        initializer,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert extension_support._package_internal_imports(
        source_root=tmp_path,
        package="pkg",
        module_name="pkg",
        source_path=initializer,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    ) == ("pkg.child",)


def test_expression_effect_projection_uses_generated_capability_lattice() -> None:
    closed = ast.parse("{'name': ('pkg.mod', None)}", mode="eval").body
    assert python_effects.expression_preserves_import_state(closed)

    opaque_call = ast.parse("callback()", mode="eval").body
    assert not python_effects.expression_preserves_import_state(opaque_call)
    assert python_effects.expression_may_execute_python(opaque_call)

    module_spec = ast.parse("ModuleSpec('pkg.mod', None)", mode="eval").body
    assert python_effects.expression_preserves_import_state(
        module_spec,
        proven_pure_calls={"ModuleSpec"},
    )
    assert not python_effects.expression_may_execute_python(
        module_spec,
        proven_pure_calls={"ModuleSpec"},
    )

    descriptor_read = ast.parse("owner.value", mode="eval").body
    assert not python_effects.expression_preserves_import_state(descriptor_read)


def test_loader_state_is_explicit_not_an_unset_override() -> None:
    state = loader_module_import_state(ModuleImportContext("pkg.entry", False))
    assert state.package == StaticMetadataValue.known("pkg")
    assert state.spec_parent == StaticMetadataValue.known("pkg")
    assert state.name == StaticMetadataValue.known("pkg.entry")
    assert state.has_path is False


def test_script_and_module_execution_have_distinct_loader_metadata() -> None:
    script = loader_module_import_state(
        ModuleImportContext("__main__", False, execution_kind="script")
    )
    assert script.package == NONE_VALUE
    assert script.spec_parent == NONE_VALUE
    assert script.name == StaticMetadataValue.known("__main__")

    as_module = loader_module_import_state(
        ModuleImportContext(
            "__main__",
            False,
            spec_name="pkg.entry",
            execution_kind="module",
        )
    )
    assert as_module.package == StaticMetadataValue.known("pkg")
    assert as_module.spec_parent == StaticMetadataValue.known("pkg")


def _known_completion_packages(
    flow: python_imports.ModuleImportFlow,
    node: ast.AST,
) -> set[str]:
    return {
        state.package.value
        for state in flow.states_for(node)
        if state.package.kind == "known" and state.package.value is not None
    }


@pytest.mark.parametrize("header", ["if predicate():", "while predicate():"])
def test_import_completion_preserves_header_exceptions_beside_body_raises(
    header: str,
) -> None:
    source = (
        "__package__ = 'outer'\n"
        "try:\n"
        f"    {header}\n"
        "        __package__ = 'inner'\n"
        "        raise RuntimeError()\n"
        "except:\n"
        "    from .child import value\n"
    )
    tree, _context, flow = _contexts_for(source)
    request = next(node for node in ast.walk(tree) if isinstance(node, ast.ImportFrom))
    assert "outer" in _known_completion_packages(flow, request)
    assert any(state.package.kind == "unknown" for state in flow.states_for(request))
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(tree, module_name="pkg.entry")


def test_import_completion_exception_before_assignment_keeps_prior_metadata() -> None:
    tree, _context, flow = _contexts_for(
        "try:\n    __package__ = 1 / 0\nexcept:\n    from .child import value\n"
    )
    request = next(node for node in ast.walk(tree) if isinstance(node, ast.ImportFrom))
    # RHS failure occurs before the package assignment commits.
    assert "pkg" in _known_completion_packages(flow, request)


@pytest.mark.parametrize("backedge", ["", "    continue\n"])
def test_import_completion_revisits_loop_import_sites_on_backedges(
    backedge: str,
) -> None:
    source = (
        "__package__ = 'first'\n"
        "while predicate():\n"
        "    from .child import value\n"
        "    __package__ = 'later'\n" + backedge
    )
    tree, _context, flow = _contexts_for(source)
    request = next(node for node in ast.walk(tree) if isinstance(node, ast.ImportFrom))
    assert "first" in _known_completion_packages(flow, request)
    # The backedge must add the callback/release-tainted successor instead of
    # caching the first visit or publishing an unjustified 'later' anchor.
    assert any(state.package.kind == "unknown" for state in flow.states_for(request))
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(tree, module_name="pkg.entry")


def test_import_completion_break_does_not_enter_loop_else() -> None:
    source = (
        "while True:\n"
        "    __package__ = 'broken'\n"
        "    break\n"
        "else:\n"
        "    __package__ = 'not_exhausted'\n"
        "from .child import value\n"
    )
    tree, _context, flow = _contexts_for(source)
    request = cast(ast.ImportFrom, tree.body[-1])
    assert _known_completion_packages(flow, request) == {"broken"}
    imports = set(module_import_scanner._collect_imports(tree, module_name="pkg.entry"))
    assert "broken.child" in imports
    assert "not_exhausted.child" not in imports


def test_import_completion_try_star_keeps_pending_exception_provenance() -> None:
    source = (
        "try:\n"
        "    raise group\n"
        "except* ValueError:\n"
        "    __package__ = 'poison'\n"
        "    raise RuntimeError()\n"
        "except* TypeError:\n"
        "    pass\n"
        "from .child import value\n"
    )
    tree, _context, flow = _contexts_for(source)
    request = cast(ast.ImportFrom, tree.body[-1])
    # A subgroup path that wrote poison must still propagate its new exception,
    # even if a later except* handler completes normally.
    assert flow.states_for(request)
    assert "poison" not in _known_completion_packages(flow, request)
    # Unknown ExceptionGroup subclasses can change globals in split/derive.
    # No exact package anchor survives that protocol on the normal path.
    assert all(state.package.kind == "unknown" for state in flow.states_for(request))
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        module_import_scanner._collect_imports(tree, module_name="pkg.entry")


@pytest.mark.parametrize("owner", ["function", "class"])
def test_import_completion_local_walrus_does_not_write_module_metadata(
    owner: str,
) -> None:
    header = "def local():" if owner == "function" else "class Local:"
    source = (
        f"{header}\n    (__package__ := 'local_only')\n    from .child import value\n"
    )
    tree, _context, flow = _contexts_for(source)
    request = next(node for node in ast.walk(tree) if isinstance(node, ast.ImportFrom))
    assert _known_completion_packages(flow, request) == {"pkg"}
    imports = set(module_import_scanner._collect_imports(tree, module_name="pkg.entry"))
    assert "pkg.child" in imports
    assert "local_only.child" not in imports


@pytest.mark.parametrize(
    "write",
    [
        "globals()['__package__'] = 'other'",
        "globals().__setitem__('__package__', 'other')",
    ],
)
def test_import_completion_class_explicit_globals_write_needs_no_global_statement(
    write: str,
) -> None:
    source = f"class Local:\n    {write}\nfrom .child import value\n"
    tree, _context, flow = _contexts_for(source)
    request = cast(ast.ImportFrom, tree.body[-1])
    assert _known_completion_packages(flow, request) == {"other"}
    assert "other.child" in module_import_scanner._collect_imports(
        tree, module_name="pkg.entry"
    )


def test_import_completion_records_nested_import_call_after_prior_argument_write() -> (
    None
):
    source = "(__package__ := 'next', __import__('child', globals(), level=1))\n"
    tree, _context, flow = _contexts_for(source)
    request = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "__import__"
    )
    assert _known_completion_packages(flow, request) == {"next"}
    assert set(
        module_import_scanner._collect_imports(tree, module_name="pkg.entry")
    ) == {"next.child"}


def test_import_completion_distinguishes_unobserved_from_explicitly_unreachable() -> (
    None
):
    tree, _context, flow = _contexts_for(
        "raise RuntimeError()\nimport sealed.unreachable\n"
    )
    dead = tree.body[-1]
    missing = ast.parse("import sealed.unobserved").body[0]
    ast.increment_lineno(missing, 100)
    assert python_imports.python_node_source_key(dead) in flow.states_by_node
    assert flow.states_for(dead) == ()
    assert python_imports.python_node_source_key(missing) not in flow.states_by_node
    assert flow.all_states
    assert flow.states_for(missing) == flow.all_states


def test_import_completion_marks_finally_after_divergence_unreachable() -> None:
    source = (
        "try:\n"
        "    while True:\n"
        "        pass\n"
        "finally:\n"
        "    import sealed.dead_finally\n"
    )
    tree, _context, flow = _contexts_for(source)
    request = next(node for node in ast.walk(tree) if isinstance(node, ast.Import))
    assert python_imports.python_node_source_key(request) in flow.states_by_node
    assert flow.states_for(request) == ()
    assert "sealed.dead_finally" not in module_import_scanner._collect_imports(
        tree, module_name="pkg.entry"
    )


@pytest.mark.parametrize(
    "terminal,reachable",
    [("return", False), ("raise RuntimeError()", True)],
)
def test_import_completion_with_only_suppresses_exception_successors(
    terminal: str,
    reachable: bool,
) -> None:
    source = (
        "def load():\n"
        "    with manager():\n"
        f"        {terminal}\n"
        "    import sealed.after_with\n"
    )
    tree, _context, flow = _contexts_for(source)
    request = next(node for node in ast.walk(tree) if isinstance(node, ast.Import))
    assert bool(flow.states_for(request)) is reachable
    imports = module_import_scanner._collect_imports(tree, module_name="pkg.entry")
    assert ("sealed.after_with" in imports) is reachable


@pytest.mark.parametrize(
    "tail",
    [
        "import sealed.dead_absolute\n",
        "from sealed.dead_from import member\n",
        "__import__('sealed.dead_builtin')\n",
    ],
)
def test_import_scanner_excludes_all_direct_import_dead_tails(tail: str) -> None:
    source = "raise RuntimeError()\n" + tail
    tree, _context, flow = _contexts_for(source)
    assert flow.states_for(tree.body[-1]) == ()
    assert module_import_scanner._collect_imports(tree, module_name="pkg.entry") == []


@pytest.mark.parametrize("dead_location", ["helper_body", "helper_call"])
def test_import_scanner_excludes_unreachable_helper_import_payloads(
    dead_location: str,
) -> None:
    source = (
        "def load(name):\n"
        "    return name\n"
        "    return __import__(name)\n"
        "load('sealed.dead_helper')\n"
        if dead_location == "helper_body"
        else "def load(name):\n"
        "    return __import__(name)\n"
        "raise RuntimeError()\n"
        "load('sealed.dead_helper')\n"
    )
    imports = module_import_scanner._collect_imports(
        ast.parse(source), module_name="pkg.entry"
    )
    assert "sealed.dead_helper" not in imports


@pytest.mark.parametrize("dead", [False, True])
@pytest.mark.parametrize("loader", ["spec", "run_path"])
def test_static_source_scanner_uses_completion_reachability(
    tmp_path: Path,
    dead: bool,
    loader: str,
) -> None:
    target = tmp_path / "target.py"
    target.write_text("VALUE = 1\n", encoding="utf-8")
    call = (
        f"importlib.util.spec_from_file_location('loaded_name', {str(target)!r})\n"
        if loader == "spec"
        else f"runpy.run_path({str(target)!r})\n"
    )
    source = (
        "import importlib.util\nimport runpy\n"
        + ("raise RuntimeError()\n" if dead else "")
        + call
    )
    requests = module_import_scanner._collect_static_source_execution_requests(
        ast.parse(source),
        source_path=tmp_path / "entry.py",
        module_name="pkg.entry",
    )
    assert len(requests) == (0 if dead else 1)
    if requests:
        assert requests[0].path == str(target)
        assert requests[0].module_name == ("loaded_name" if loader == "spec" else None)


@pytest.mark.parametrize("dead", [False, True])
@pytest.mark.parametrize(
    "call",
    [
        "__import__('sealed.runtime')",
        "importlib.import_module('sealed.runtime')",
        "importlib.util.find_spec('sealed.runtime')",
    ],
)
def test_runtime_protocol_scanner_uses_completion_reachability(
    dead: bool,
    call: str,
) -> None:
    source = (
        "import importlib\nimport importlib.util\n"
        + ("raise RuntimeError()\n" if dead else "")
        + call
        + "\n"
    )
    assert module_import_scanner._tree_uses_runtime_import_protocol(
        ast.parse(source),
        module_name="pkg.entry",
        is_package=False,
    ) is (not dead)


@pytest.mark.parametrize("mode", ["full", "module_init_static_helpers"])
def test_selected_stdlib_helper_scan_does_not_revive_dead_body_imports(
    mode: module_import_scanner.ImportScanMode,
) -> None:
    source = (
        "class UserDict:\n"
        "    def copy(self):\n"
        "        return self\n"
        "        import sealed.dead_static_helper\n"
    )
    imports = module_import_scanner._collect_imports(
        ast.parse(source),
        module_name="collections",
        import_scan_mode=mode,
    )
    assert "sealed.dead_static_helper" not in imports


def test_unobserved_deferred_lambda_import_is_not_treated_as_dead() -> None:
    source = "load = lambda: __import__('sealed.deferred_lambda')\n"
    tree, _context, flow = _contexts_for(source)
    request = next(node for node in ast.walk(tree) if isinstance(node, ast.Call))
    assert flow.states_for(request)
    assert "sealed.deferred_lambda" in module_import_scanner._collect_imports(
        tree, module_name="pkg.entry"
    )


@pytest.mark.parametrize(
    "statement,expected",
    [
        (
            "items[__import__('child', globals(), level=1)] = (__package__ := 'rhs')",
            "rhs",
        ),
        (
            "items[(__package__ := 'target')] += __import__('child', globals(), level=1)",
            "target",
        ),
        (
            "del items[(__package__ := 'deleted_index', __import__('child', globals(), level=1))]",
            "deleted_index",
        ),
    ],
)
def test_import_completion_assignment_family_evaluates_target_expressions_in_order(
    statement: str,
    expected: str,
) -> None:
    tree, _context, flow = _contexts_for(statement + "\n")
    request = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom)
        or (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Name)
            and node.func.id == "__import__"
        )
    )
    assert _known_completion_packages(flow, request) == {expected}
    imports = module_import_scanner._collect_imports(tree, module_name="pkg.entry")
    assert f"{expected}.child" in imports
    assert "pkg.child" not in imports


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("future_annotations", [False, True])
def test_import_completion_annotation_order_respects_version_and_future_policy(
    target_python: tuple[int, int],
    future_annotations: bool,
) -> None:
    source = (
        ("from __future__ import annotations\n" if future_annotations else "")
        + "items[(__package__ := 'target')]: "
        "globals().__setitem__('__package__', 'annotation')\n"
        "from .child import value\n"
    )
    tree = ast.parse(source)
    context = ModuleImportContext("pkg.entry", False, target_python=target_python)
    flow = analyze_module_import_flow(tree, context)
    request = cast(ast.ImportFrom, tree.body[-1])
    # A non-simple target is evaluated even without an assigned value. Its
    # annotation follows on eager versions, but cannot mutate module-init state
    # under future-string or Python 3.14 deferred annotation semantics.
    expected = (
        "annotation" if target_python < (3, 14) and not future_annotations else "target"
    )
    assert _known_completion_packages(flow, request) == {expected}
    assert {state.package.value for state in flow.final_states} == {expected}


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("future_annotations", [False, True])
def test_import_completion_function_local_annotation_never_executes(
    target_python: tuple[int, int],
    future_annotations: bool,
) -> None:
    source = (
        ("from __future__ import annotations\n" if future_annotations else "")
        + "def local():\n"
        "    value: globals().__setitem__('__package__', 'annotation')\n"
        "    from .child import member\n"
    )
    tree = ast.parse(source)
    context = ModuleImportContext("pkg.entry", False, target_python=target_python)
    flow = analyze_module_import_flow(tree, context)
    request = next(
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.level
    )
    assert _known_completion_packages(flow, request) == {"pkg"}
