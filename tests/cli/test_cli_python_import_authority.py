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
from molt.cli.target_python import _DEFAULT_TARGET_PYTHON_VERSION
from molt.cli.python_import_resolution import (
    LocalPythonModuleResolver,
    PythonImportPolicy,
    local_import_targets,
)
from molt.compiler_analysis import python_imports
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
    source = "from .a import x\n__package__ = 'other.pkg'\nfrom .b import y\n"
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


def test_branch_join_preserves_whole_possible_states() -> None:
    source = (
        "if condition:\n"
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
        "if flag:\n"
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
    imports = set(
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )
    )

    assert {"other.pkg.child", "other.pkg.child.value"} <= imports


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


def test_intrinsic_lookup_cannot_mutate_escaped_module_metadata() -> None:
    source = (
        "from _intrinsics import require_intrinsic as require\n"
        "value = require('molt_demo', globals())\n"
        "from .child import value\n"
    )
    assert set(
        module_import_scanner._collect_imports(
            ast.parse(source), module_name="pkg.entry", is_package=False
        )
    ) == {
        "_intrinsics",
        "_intrinsics.require_intrinsic",
        "pkg.child",
        "pkg.child.value",
    }


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
        "def f():\n    global __package__\n    __package__ = 'other'\n    from .child import value\n",
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
