"""Consumer regressions for the shared statement-loop metadata authority."""

from __future__ import annotations

import ast

import pytest

from molt.cli import module_import_scanner
from molt.compiler_analysis.python_binding_flow import (
    analyze_python_bindings,
    python_ast_digest,
)
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    UnresolvedStaticImportError,
    analyze_module_import_flow,
    plan_static_import_request,
    require_static_import_modules,
)
from molt.compiler_analysis.python_effects_generated import INVOKES_ITERATION_CALLBACK
from molt.compiler_analysis.static_truth import static_expression_result


def _modules(source: str, version: tuple[int, int]) -> tuple[str, ...]:
    tree = ast.parse(source)
    context = ModuleImportContext("pkg.entry", False, target_python=version)
    flow = analyze_module_import_flow(tree, context)
    return tuple(
        module
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom)
        for module in require_static_import_modules(
            plan_static_import_request(
                StaticImportRequest.statement(node.module or "", level=node.level),
                tuple(context.with_state(state) for state in flow.states_for(node)),
            ),
            consumer="loop metadata regression",
        )
    )


_SAFE = (
    "for key in ['safe_alias', 'other_alias']:\n"
    "    globals()[key] = 1\n"
    "from .child import value\n"
)

_UNSAFE = (
    "for key in ['safe_alias', '__package__']:\n"
    "    globals()[key] = 'other.pkg'\n"
    "from .child import value\n",
    "for key in ['safe_alias']:\n"
    "    key = dynamic\n"
    "    globals()[key] = 1\n"
    "from .child import value\n",
    "for key in ['safe_alias']:\n    exposed = globals()\nfrom .child import value\n",
    "for key in ['safe_alias']:\n"
    "    match '__package__':\n"
    "        case key:\n"
    "            globals()[key] = 'other.pkg'\n"
    "from .child import value\n",
    "key = '__package__'\n"
    "for key in []:\n"
    "    pass\n"
    "else:\n"
    "    globals()[key] = 'other.pkg'\n"
    "from .child import value\n",
    "for key in (keys := ['safe_alias']):\n"
    "    keys.append('__package__')\n"
    "    globals()[key] = 'other.pkg'\n"
    "from .child import value\n",
    "class Previous:\n"
    "    def __del__(self):\n"
    "        globals()['__package__'] = 'other.pkg'\n"
    "key = Previous()\n"
    "for key in ['safe_alias']:\n"
    "    pass\n"
    "from .child import value\n",
    "class Previous:\n"
    "    def __del__(self):\n"
    "        globals()['__package__'] = 'other.pkg'\n"
    "safe_alias = Previous()\n"
    "for key in ['safe_alias']:\n"
    "    globals()[key] = 1\n"
    "from .child import value\n",
    "class Previous:\n"
    "    def __del__(self):\n"
    "        globals()['__package__'] = 'other.pkg'\n"
    "for key in {'safe_alias': Previous()}:\n"
    "    pass\n"
    "from .child import value\n",
    "for key in opaque:\n    pass\nfrom .child import value\n",
    "for key in ['safe_alias']:\n"
    "    exposed = (globals(),)\n"
    "from .child import value\n",
    "for key in ['__package__']:\n"
    "    globals().__setitem__(key, 'other.pkg')\n"
    "from .child import value\n",
    "class Hostile:\n"
    "    def __hash__(self):\n"
    "        return hash('safe_alias')\n"
    "    def __eq__(self, other):\n"
    "        globals()['__package__'] = 'other.pkg'\n"
    "        return False\n"
    "globals()[Hostile()] = 1\n"
    "__package__ = 'pkg'\n"
    "for key in ['safe_alias']:\n"
    "    globals()[key] = 1\n"
    "from .child import value\n",
    "class Previous:\n"
    "    def __del__(self):\n"
    "        globals()['__package__'] = 'callback.pkg'\n"
    "__package__ = Previous()\n"
    "try:\n"
    "    for obj.attr in ['safe_alias']:\n"
    "        pass\n"
    "except:\n"
    "    __package__ = 'handler.pkg'\n"
    "    from .child import value\n",
)


@pytest.mark.parametrize("version", ((3, 12), (3, 13), (3, 14)))
def test_closed_iterable_metadata_authority(version: tuple[int, int]) -> None:
    assert _modules(_SAFE, version) == ("pkg.child",)
    for source in _UNSAFE:
        with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
            _modules(source, version)


@pytest.mark.parametrize(
    "prefix",
    (
        "for key in []:\n    globals()['__package__'] = 'other.pkg'\n",
        "for key in ('safe_alias',):\n    globals()[key] = 1\n",
        "for key in ['safe_alias']:\n    break\n"
        "    globals()['__package__'] = 'other.pkg'\n",
        "for key in ['safe_alias']:\n    continue\n"
        "    globals()['__package__'] = 'other.pkg'\n",
        "for key in ['safe_alias']:\n"
        "    for other in ('x',):\n"
        "        globals()[key] = 1\n",
        "__package__, __name__ = ('pkg', 'pkg.entry')\n",
        "globals()['__package__'] = 'pkg'\n",
        "globals().__setitem__('__package__', 'pkg')\n",
    ),
)
def test_completion_and_assignment_siblings_preserve_closed_anchors(
    prefix: str,
) -> None:
    assert _modules(prefix + "from .child import value\n", (3, 12)) == ("pkg.child",)


def test_import_consumers_share_loop_metadata_facts() -> None:
    tree = ast.parse(_SAFE)
    assert set(
        module_import_scanner._collect_imports(
            tree, module_name="pkg.entry", is_package=False
        )
    ) == {"pkg.child", "pkg.child.value"}
    star = ast.parse(_SAFE.replace("import value", "import *"))
    assert module_import_scanner._collect_import_star_modules(
        star, module_name="pkg.entry", is_package=False
    ) == ("pkg.child",)
    assert (
        module_import_scanner._runtime_import_alias_bindings(
            tree, module_name="pkg.entry", is_package=False
        )["value"]
        == "pkg.child.value"
    )


def test_fresh_container_provenance_does_not_follow_walrus_publication() -> None:
    literal = ast.parse("['safe_alias']", mode="eval").body
    published = ast.parse("(keys := ['safe_alias'])", mode="eval").body
    assert static_expression_result(literal).fresh_container
    assert not static_expression_result(published).fresh_container


def test_loop_protocol_fact_is_shared_and_effect_backed() -> None:
    tree = ast.parse("for key in ['safe_alias']:\n    pass\n")
    index = analyze_python_bindings(tree, source_digest=python_ast_digest(tree))
    fact = index.statement_fact(tree.body[0])
    assert fact is not None and fact.iteration is not None
    assert not fact.iteration.effects & INVOKES_ITERATION_CALLBACK
    assert fact.iteration.element_strings is not None
    assert fact.iteration.element_strings.values == frozenset(("safe_alias",))


@pytest.mark.parametrize("target", ("obj.attr", "globals()[unknown_key]"))
def test_literal_iterator_does_not_erase_target_exception_successors(
    target: str,
) -> None:
    source = (
        "try:\n"
        f"    for {target} in ['safe_alias']:\n"
        "        pass\n"
        "except:\n"
        "    __package__ = 'recovered.pkg'\n"
        "    from .recovery import value\n"
    )
    tree = ast.parse(source)
    context = ModuleImportContext("pkg.entry", False)
    flow = analyze_module_import_flow(tree, context)
    handler_import = tree.body[0].handlers[0].body[-1]
    assert flow.states_for(handler_import), "target failures must reach the handler"


def test_shared_loop_finalizes_exhaustion_before_else_and_other_exits_once() -> None:
    from molt.compiler_analysis.python_binding_facts import PythonCompletionFlow as Flow

    entry = frozenset({()})

    def add(state, event):
        return frozenset((*path, event) for path in state)

    def join(left, right):
        return left | right

    def advance(state):
        return add(state, "exhausted"), Flow(
            broken=add(state, "break"),
            returned=add(state, "return"),
            raised=add(state, "raise"),
        )

    flow = Flow.loop(
        entry,
        advance,
        lambda state: Flow(normal=add(state, "else")),
        join_states=join,
        equivalent_states=lambda left, right: left == right,
        widen_state=lambda state: state,
        finalize=lambda state: Flow(normal=add(state, "release")),
    )
    assert flow.normal == frozenset(
        {("break", "release"), ("exhausted", "release", "else")}
    )
    assert flow.returned == frozenset({("return", "release")})
    assert flow.raised == frozenset({("raise", "release")})


def test_imports_in_else_observe_iterator_release_fact() -> None:
    from dataclasses import replace
    from molt.compiler_analysis.python_binding_facts import PythonNodeKey
    from molt.compiler_analysis.python_effects_generated import RUNS_FINALIZER
    from molt.compiler_analysis.python_imports import (
        _analyze_module_import_flow_uncached,
    )

    tree = ast.parse("for key in []:\n    pass\nelse:\n    from .child import value\n")
    index = analyze_python_bindings(tree, source_digest=python_ast_digest(tree))
    statements = {fact.node: fact for fact in index.statements}
    loop_key = PythonNodeKey.from_node(tree.body[0])
    loop = statements[loop_key]
    assert loop.iteration is not None
    statements[loop_key] = replace(
        loop, iteration=replace(loop.iteration, release_effects=RUNS_FINALIZER)
    )
    context = ModuleImportContext("pkg.entry", False)
    flow = _analyze_module_import_flow_uncached(
        tree,
        context,
        statement_facts=statements,
        expression_facts={fact.node: fact for fact in index.expressions},
        assignment_effects={},
        call_facts={},
    )
    site = tree.body[0].orelse[0]
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        require_static_import_modules(
            plan_static_import_request(
                StaticImportRequest.statement("child", level=1),
                tuple(context.with_state(state) for state in flow.states_for(site)),
            ),
            consumer="iterator finalization before else",
        )


def test_finite_namespace_key_count_does_not_multiply_flow_states() -> None:
    counts = []
    for size in (4, 128):
        keys = [f"key_{index}" for index in range(size)]
        tree = ast.parse(f"for key in {keys!r}:\n    globals()[key] = 1\n")
        index = analyze_python_bindings(tree, source_digest=python_ast_digest(tree))
        counts.append(index.state_count)
    assert counts[0] == counts[1]


@pytest.mark.parametrize(
    "write",
    (
        "globals()['safe_alias'] = 1",
        "globals().__setitem__('safe_alias', 1)",
        "put = globals().__setitem__; put('safe_alias', 1)",
    ),
)
def test_namespace_syntax_and_method_alias_share_exact_publication(write: str) -> None:
    from molt.compiler_analysis.python_binding_facts import PythonIdentity

    tree = ast.parse(write + "\nsafe_alias\n")
    index = analyze_python_bindings(tree, source_digest=python_ast_digest(tree))
    value = index.expression_fact(tree.body[-1].value)
    assert value is not None
    assert value.identities == int(PythonIdentity.INERT_VALUE)
    assert value.static_value == 1


@pytest.mark.parametrize(
    "mutation",
    (
        "globals()['__package__'] = 'claimed.pkg'",
        "globals().__setitem__('__package__', 'claimed.pkg')",
        "put = globals().__setitem__; put('__package__', 'claimed.pkg')",
        "globals().__delitem__('__package__')",
        "del globals()['__package__']",
    ),
)
def test_all_namespace_mutations_preserve_post_publication_finalizer(
    mutation: str,
) -> None:
    source = (
        "class Previous:\n"
        "    def __del__(self):\n"
        "        globals()['__package__'] = 'callback.pkg'\n"
        "__package__ = Previous()\n" + mutation + "\nfrom .child import value\n"
    )
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(source, (3, 12))
    namespace = {"__name__": "pkg.entry", "__package__": "pkg"}
    exec(source.rsplit("from .child", 1)[0], namespace)
    assert namespace["__package__"] == "callback.pkg"


@pytest.mark.parametrize(
    "call",
    (
        "globals().__setitem__()",
        "globals().__setitem__('__package__')",
        "globals().__setitem__('__package__', 'pkg', 'extra')",
        "globals().__setitem__(key='__package__', value='pkg')",
        "globals().__delitem__()",
        "globals().__delitem__('__package__', 'extra')",
    ),
)
def test_namespace_method_wrong_arity_never_mints_static_anchor(call: str) -> None:
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(call + "\nfrom .child import value\n", (3, 12))


def test_shadowed_globals_method_does_not_publish_metadata_by_spelling() -> None:
    source = "globals = unknown\nglobals().__setitem__('__package__', 'claimed.pkg')\nfrom .child import value\n"
    assert "claimed.pkg.child" not in _modules(source, (3, 12))
