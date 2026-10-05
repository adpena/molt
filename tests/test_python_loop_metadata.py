"""Consumer regressions for the shared statement-loop metadata authority."""

from __future__ import annotations

import ast

import pytest

from molt.cli import module_import_scanner
from molt.compiler_analysis.python_binding_flow import analyze_python_bindings
from molt.compiler_analysis.python_source_keys import python_ast_digest
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
        "for key in ('safe_alias',):\n    exposed = globals()\n",
        "for key in ('safe_alias',):\n    exposed = (globals(),)\n",
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
    namespace = {"__package__": "pkg"}
    exec(compile(prefix, "<closed-metadata-storage-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "pkg"
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
    assert not module_import_scanner._tree_uses_runtime_import_protocol(
        tree, module_name="pkg.entry", is_package=False
    )
    assert module_import_scanner._tree_uses_runtime_import_protocol(
        tree, module_name="pkg.entry", is_package=False, include_statements=True
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
        iteration_facts=dict(index.iterations),
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
    from molt.compiler_analysis.python_value_identity import PythonIdentity

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
    tree = ast.parse(source)
    context = ModuleImportContext("pkg.entry", False, target_python=(3, 12))
    flow = analyze_module_import_flow(tree, context)
    request = tree.body[-1]
    plan = plan_static_import_request(
        StaticImportRequest.statement("child", level=1),
        tuple(context.with_state(state) for state in flow.states_for(request)),
    )
    assert "claimed.pkg.child" not in plan.modules
    assert plan.requires_runtime
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(source, (3, 12))
    namespace = {"__package__": "pkg"}

    class Receiver:
        def __setitem__(self, key, value):
            namespace["__package__"] = "callback.pkg"

    namespace["unknown"] = lambda: Receiver()
    exec(source.rsplit("from .child", 1)[0], namespace)
    assert namespace["__package__"] == "callback.pkg"


@pytest.mark.parametrize(
    "mutation",
    [
        "globals().__setitem__('safe_alias', 2)",
        "put = globals().__setitem__; put('safe_alias', 2)",
        "globals().__delitem__('safe_alias')",
        "remove = globals().__delitem__; remove('safe_alias')",
    ],
)
def test_rooted_namespace_method_cleanup_preserves_closed_import_anchor(
    mutation: str,
) -> None:
    source = "safe_alias = 1\n" + mutation + "\nfrom .child import value\n"
    namespace = {"__package__": "pkg"}
    exec(source.rsplit("from .child", 1)[0], namespace)
    assert namespace["__package__"] == "pkg"
    if "__delitem__" in mutation:
        assert "safe_alias" not in namespace
    else:
        assert namespace["safe_alias"] == 2
    assert _modules(source, (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize(
    "display",
    [
        "globals()",
        "(globals(),)",
        "[globals() for _ in (0,)]",
        "{'namespace': globals() for _ in (0,)}",
        "(globals() for _ in (0,))",
    ],
)
@pytest.mark.parametrize("position", ["plain", "before_empty_loop", "before_match"])
def test_namespace_storage_without_escape_preserves_closed_import_anchor(
    display: str,
    position: str,
) -> None:
    prefix = f"exposed = {display}\n"
    if position == "before_empty_loop":
        prefix += "for _ in ():\n    pass\n"
    elif position == "before_match":
        prefix += "match 0:\n    case captured: pass\n"
    namespace = {"__package__": "pkg"}
    exec(compile(prefix, "<namespace-storage-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "pkg"
    assert _modules(prefix + "from .child import value\n", (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize("intervening_callback", [False, True])
@pytest.mark.parametrize(
    ("display", "argument"),
    [
        ("globals()", "exposed"),
        ("(globals(),)", "exposed"),
        ("(globals(),)", "exposed[0]"),
        ("[globals() for _ in (0,)]", "exposed"),
        ("[globals() for _ in (0,)]", "exposed[0]"),
        ("{'namespace': globals() for _ in (0,)}", "exposed"),
        ("{'namespace': globals() for _ in (0,)}", "exposed['namespace']"),
        ("(globals() for _ in (0,))", "exposed"),
        ("(globals() for _ in (0,))", "next(exposed)"),
        ("[globals(), *unknown]", "exposed"),
        ("{'namespace': globals(), **unknown}", "exposed"),
    ],
)
def test_foreign_namespace_consumer_requires_import_custody(
    display: str, argument: str, intervening_callback: bool
) -> None:
    prefix = f"exposed = {display}\n"
    if intervening_callback:
        prefix += "callback()\n"
    prefix += f"consume({argument})\n"

    # This function owns the test module's globals, not the guest namespace.
    # Only the supplied value grants access to the guest's package metadata.
    def consume(value):
        if type(value) is dict:
            target = value if "__package__" in value else value["namespace"]
        else:
            target = next(iter(value))
        target["__package__"] = "changed.pkg"

    namespace = {
        "__package__": "pkg",
        "unknown": {},
        "callback": lambda: None,
        "consume": consume,
    }
    assert consume.__globals__ is not namespace
    exec(compile(prefix, "<namespace-escape-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "changed.pkg"
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(prefix + "from .child import value\n", (3, 12))


@pytest.mark.parametrize(
    "comprehension",
    [
        "[(__package__ := 'other') for _ in ()]",
        "[(__package__ := 'other') for _ in (0, 1) if False]",
        "[(__package__ := 'other') for _ in (0, 1) for inner in ()]",
        "((__package__ := 'other') for _ in (0, 1))",
    ],
)
def test_comprehension_import_state_respects_zero_and_deferred_execution(
    comprehension: str,
) -> None:
    prefix = f"values = {comprehension}\nfor _ in ():\n    pass\n"
    namespace = {"__package__": "pkg"}
    exec(compile(prefix, "<comprehension-metadata-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "pkg"
    assert _modules(prefix + "from .child import value\n", (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize("unrelated_loop", [False, True])
def test_namespace_inplace_update_requires_actual_relative_anchor(
    monkeypatch: pytest.MonkeyPatch, unrelated_loop: bool
) -> None:
    import importlib.machinery
    import sys
    import types

    for name in ("pkg", "other"):
        package = types.ModuleType(name)
        package.__path__ = []
        child = types.ModuleType(name + ".child")
        child.value = name
        monkeypatch.setitem(sys.modules, name, package)
        monkeypatch.setitem(sys.modules, child.__name__, child)
    prefix = "exposed = globals()\nexposed |= {'__package__': 'other'}\n"
    if unrelated_loop:
        prefix += "for _ in ():\n    pass\n"
    source = prefix + "from .child import value\n"
    namespace = {
        "__name__": "pkg.entry",
        "__package__": "pkg",
        "__spec__": importlib.machinery.ModuleSpec("pkg.entry", None),
    }
    with pytest.warns(DeprecationWarning, match=r"__package__ != __spec__\.parent"):
        exec(compile(source, "<inplace-relative-import-oracle>", "exec"), namespace)
    assert namespace["value"] == "other"
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(source, (3, 12))


@pytest.mark.parametrize("deferred", [False, True])
@pytest.mark.parametrize("unrelated_loop", [False, True])
def test_scalar_augmented_assignment_preserves_actual_relative_anchor(
    monkeypatch: pytest.MonkeyPatch, deferred: bool, unrelated_loop: bool
) -> None:
    import importlib.machinery
    import sys
    import types

    package = types.ModuleType("pkg")
    package.__path__ = []
    child = types.ModuleType("pkg.child")
    child.value = "pkg"
    monkeypatch.setitem(sys.modules, package.__name__, package)
    monkeypatch.setitem(sys.modules, child.__name__, child)
    prefix = "counter = 0\n"
    if deferred:
        prefix += "def unused():\n    counter = 0\n    counter += 1\n"
    else:
        prefix += "counter += 1\n"
    if unrelated_loop:
        prefix += "for _ in ():\n    pass\n"
    source = prefix + "from .child import value\n"
    namespace = {
        "__name__": "pkg.entry",
        "__package__": "pkg",
        "__spec__": importlib.machinery.ModuleSpec("pkg.entry", None),
    }
    exec(compile(source, "<scalar-inplace-import-oracle>", "exec"), namespace)
    assert namespace["counter"] == (0 if deferred else 1)
    assert namespace["__package__"] == "pkg"
    assert namespace["value"] == "pkg"
    assert _modules(source, (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize(
    "prefix",
    [
        "g = ((__package__ := 'other') for _ in (0,))\nlist(g)\n",
        "g = ((__package__ := 'other') for _ in (0,))\nnext(g)\n",
        "g = ((__package__ := 'other') for _ in (0,))\ng.send(None)\n",
        "g = ((__package__ := 'other') for _ in (0,))\nfor item in g: pass\n",
        "g = ((__package__ := 'other') for _ in (0,))\n(item,) = g\n",
        "g = ((__package__ := 'other') for _ in (0,))\nconsume(*g)\n",
        "exposed = globals()\ndef rebind():\n    exposed['__package__'] = 'other'\nrebind()\n",
        "exposed = globals()\ndef rebind():\n    exposed['__package__'] = 'other'\nfunctions = (rebind,)\nfunctions[0]()\n",
        "exposed = globals()\ndef rebind():\n    exposed['__package__'] = 'other'\nfunctions = {'change': rebind}\nfunctions['change']()\n",
        "exposed = globals()\nf = lambda: exposed.__setitem__('__package__', 'other')\nf()\n",
        "exposed = globals()\ndef outer():\n    return lambda: exposed.__setitem__('__package__', 'other')\nouter()()\n",
        "def stream():\n    global __package__\n    __package__ = 'other'\n    yield 1\ng = stream()\nnext(g)\n",
        "async def rebind():\n    global __package__\n    __package__ = 'other'\nasync def run():\n    await rebind()\nc = run()\ntry:\n    c.send(None)\nexcept StopIteration:\n    pass\n",
        "exposed = globals()\ndef rebind():\n    exposed['__package__'] = 'other'\ndef use(cb=rebind):\n    cb()\nuse()\n",
        "exposed = globals()\ndef rebind():\n    exposed['__package__'] = 'other'\ndef choose():\n    return rebind\nchoose()()\n",
        "exposed = globals()\ndef rebind():\n    exposed['__package__'] = 'other'\ndef stream():\n    yield rebind\nvalues = list(stream())\nvalues[0]()\n",
        "exposed = globals()\nclass Change:\n    def apply(self):\n        exposed['__package__'] = 'other'\nvalue = Change()\nvalue.apply()\n",
        "exposed = globals()\nclass Change:\n    def __iadd__(self, value):\n        exposed['__package__'] = 'other'\n        return self\nvalue = Change()\nvalue += 1\n",
        "exposed = globals()\ndef decorate(fn):\n    exposed['__package__'] = 'other'\n    return fn\n@decorate\ndef f(): pass\n",
    ],
)
def test_deferred_execution_changes_actual_relative_anchor(monkeypatch, prefix):
    import sys
    import types

    for name in ("pkg", "other"):
        package = types.ModuleType(name)
        package.__path__ = []
        child = types.ModuleType(name + ".child")
        child.value = name
        monkeypatch.setitem(sys.modules, name, package)
        monkeypatch.setitem(sys.modules, child.__name__, child)
    source = prefix + "from .child import value\n"
    namespace = {
        "__name__": "pkg.entry",
        "__package__": "pkg",
        "__spec__": None,
        "consume": lambda *values: None,
    }
    exec(compile(source, "<deferred-relative-import-oracle>", "exec"), namespace)
    assert namespace["__package__"] == namespace["value"] == "other"
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(source, (3, 12))


@pytest.mark.parametrize(
    "prefix",
    [
        "g = ((__package__ := 'other') for _ in (0,))\n",
        "g = ((__package__ := 'other') for _ in (0,))\nstored = list((g,))\n",
        "g = ((__package__ := 'other') for _ in (0,))\nstored = []\nstored.append(g)\n",
        "g = ((__package__ := 'other') for _ in (0,))\nstored = g.send\n",
        "def stream():\n    global __package__\n    __package__ = 'other'\n    yield 1\ng = stream()\n",
        "exposed = globals()\ndef stream(namespace):\n    namespace['__package__'] = 'other'\n    yield 1\ng = stream(exposed)\n",
        "exposed = globals()\ndef rebind():\n    exposed['__package__'] = 'other'\nstored = [rebind]\n",
        "exposed = globals()\nf = lambda: exposed.__setitem__('__package__', 'other')\n",
        "def local():\n    __package__ = 'other'\nlocal()\n",
        "exposed = globals()\nclass Change:\n    def apply(self):\n        exposed['__package__'] = 'other'\nvalue = Change()\nstored = value.apply\n",
    ],
)
def test_deferred_creation_and_storage_preserve_relative_anchor(prefix):
    namespace = {"__package__": "pkg"}
    exec(compile(prefix, "<deferred-storage-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "pkg"
    assert _modules(prefix + "from .child import value\n", (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize("argument", ["*exposed", "*[globals()]", "*(globals(),)"])
def test_starred_foreign_namespace_escape_changes_actual_anchor(argument):
    def consume(namespace):
        namespace["__package__"] = "other"

    prefix = "exposed = (globals(),)\nconsume(" + argument + ")\n"
    namespace = {"__package__": "pkg", "consume": consume}
    assert consume.__globals__ is not namespace
    exec(compile(prefix, "<starred-namespace-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "other"
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(prefix + "from .child import value\n", (3, 12))


@pytest.mark.parametrize("rhs", ["operand", "replace_target()"])
def test_augmented_publication_releases_callback_replacement(rhs: str) -> None:
    namespace: dict[str, object] = {"__name__": "pkg.entry", "__package__": "pkg"}
    events: list[str] = []

    class Previous:
        def __del__(self) -> None:
            events.append("retired")
            namespace["__package__"] = "changed.pkg"

    class Operand(int):
        def __radd__(self, value):
            assert value == 0
            namespace["counter"] = Previous()
            return 1

    def replace_target():
        namespace["counter"] = Previous()
        return 1

    # Foreign callbacks hide their mutation from source syntax. STORE_NAME
    # must retire the current binding, not the scalar retained before RHS/op.
    namespace.update(operand=Operand(), replace_target=replace_target)
    prefix = f"counter = 0\ncounter += {rhs}\n"
    exec(compile(prefix, "<augmented-retirement-oracle>", "exec"), namespace)
    assert namespace["counter"] == 1
    assert namespace["__package__"] == "changed.pkg"
    assert events == ["retired"]
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(prefix + "from .child import value\n", (3, 12))


def test_successive_scalar_increments_keep_exact_result_and_import_anchor() -> None:
    prefix = "counter = 0\ncounter += 1\ncounter += 1\n"
    namespace = {"__package__": "pkg"}
    exec(compile(prefix, "<successive-inplace-oracle>", "exec"), namespace)
    assert namespace["counter"] == 2
    assert _modules(prefix + "from .child import value\n", (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize(
    ("setup", "operation"),
    [
        (
            "def rebind():\n    global __package__\n    __package__ = 'other'\ndef keep(): pass\nchoice = rebind if flag else keep",
            "choice()",
        ),
        (
            "def rebind():\n    global __package__\n    __package__ = 'other'\ndef keep(): pass\naction = keep\ndef swap():\n    global action\n    action = rebind",
            "swap()\naction()",
        ),
        (
            "def rebind():\n    global __package__\n    __package__ = 'other'\ndef swap():\n    global action\n    action = rebind",
            "swap()\naction()",
        ),
        ("import sys", "setattr(sys.modules.get(__name__), '__package__', 'other')"),
        ("import sys", "consume_module(sys.modules[__name__])"),
        (
            "exposed = globals()\nclass Key:\n    def __hash__(self):\n        exposed['__package__'] = 'other'\n        return 0",
            "{Key()}",
        ),
        (
            "exposed = globals()\nclass Key:\n    def __hash__(self):\n        exposed['__package__'] = 'other'\n        return 0",
            "set([Key()])",
        ),
        (
            "exposed = globals()\nclass Key:\n    def __hash__(self):\n        exposed['__package__'] = 'other'\n        return 0",
            "{(Key(),): 1}",
        ),
        (
            "exposed = globals()\nclass Change:\n    def __getattr__(self, name):\n        exposed['__package__'] = 'other'\n        return 1",
            "Change().missing",
        ),
        (
            "exposed = globals()\nclass Change:\n    def __del__(self):\n        exposed['__package__'] = 'other'",
            "Change()",
        ),
        (
            "exposed = globals()\nclass Change:\n    def __del__(self):\n        exposed['__package__'] = 'other'\nvalue = Change()",
            "value = None",
        ),
        (
            "exposed = globals()\nclass Change:\n    def __del__(self):\n        exposed['__package__'] = 'other'\nvalue = Change()",
            "del value",
        ),
        (
            "exposed = globals()\nclass Sink:\n    def __setitem__(self, key, value):\n        exposed['__package__'] = 'other'\nsink = Sink()\ndef change():\n    sink[0] = 1",
            "change()",
        ),
        (
            "exposed = globals()\nclass Sink:\n    def __delitem__(self, key):\n        exposed['__package__'] = 'other'\nsink = Sink()\ndef change():\n    del sink[0]",
            "change()",
        ),
        (
            "exposed = globals()\nclass Sink:\n    def __setattr__(self, key, value):\n        exposed['__package__'] = 'other'\nsink = Sink()",
            "sink.value = 1",
        ),
        (
            "exposed = globals()\nclass Sink:\n    def __delattr__(self, key):\n        exposed['__package__'] = 'other'\nsink = Sink()",
            "del sink.value",
        ),
        (
            "exposed = globals()\nclass Mapping:\n    def keys(self):\n        exposed['__package__'] = 'other'\n        return ()\n    def __getitem__(self, key):\n        return 1",
            "{**Mapping()}",
        ),
        (
            "exposed = globals()\nclass Index:\n    def __index__(self):\n        exposed['__package__'] = 'other'\n        return 0",
            "[1][Index()]",
        ),
        (
            "exposed = globals()\nclass Change:\n    def __eq__(self, value):\n        exposed['__package__'] = 'other'\n        return True",
            "Change() in [1]",
        ),
        (
            "exposed = globals()\nclass Change:\n    def __getattr__(self, name):\n        exposed['__package__'] = 'other'\n        return 1\nsubject = Change()",
            "match subject:\n    case Change(value=1): pass",
        ),
        (
            "exposed = globals()\nclass Base:\n    def __init__(self):\n        exposed['__package__'] = 'other'\nclass Child(Base): pass",
            "Child()",
        ),
        (
            "exposed = globals()\nclass Base:\n    def apply(self):\n        exposed['__package__'] = 'other'\nclass Child(Base): pass\nvalue = Child()",
            "value.apply()",
        ),
        (
            "exposed = globals()\nclass Base: pass\nclass Child(Base):\n    def __init__(self):\n        exposed['__package__'] = 'other'",
            "Child()",
        ),
        (
            "exposed = globals()\nclass Base:\n    def __init_subclass__(cls):\n        exposed['__package__'] = 'other'",
            "class Child(Base): pass",
        ),
        (
            "exposed = globals()\nclass Meta(type):\n    @classmethod\n    def __prepare__(cls, name, bases):\n        exposed['__package__'] = 'other'\n        return {}",
            "class Child(metaclass=Meta): pass",
        ),
        (
            "exposed = globals()\nclass Descriptor:\n    def __set_name__(self, owner, name):\n        exposed['__package__'] = 'other'",
            "class Owner:\n    member = Descriptor()",
        ),
        (
            "exposed = globals()\nclass Descriptor:\n    def __get__(self, instance, owner):\n        exposed['__package__'] = 'other'\n        return 1\nclass Owner:\n    member = Descriptor()\nowner = Owner()",
            "owner.member",
        ),
        (
            "def rebind():\n    global __package__\n    __package__ = 'other'\nclass Registry:\n    handler = rebind",
            "Registry.handler()",
        ),
        (
            "d = {'g': globals()}\ndef rebind():\n    d['g']['__package__'] = 'other'",
            "rebind()",
        ),
        (
            "d = {'g': globals()}\n__name__ = 'other.entry'\ndef rebind():\n    del d['g']['__package__']",
            "rebind()",
        ),
    ],
)
def test_protocol_provenance_selects_actual_relative_import(
    monkeypatch, setup, operation
):
    import sys
    import types

    for name in ("pkg", "other"):
        package = types.ModuleType(name)
        package.__path__ = []
        child = types.ModuleType(name + ".child")
        child.value = name
        monkeypatch.setitem(sys.modules, name, package)
        monkeypatch.setitem(sys.modules, child.__name__, child)
    module = types.ModuleType("pkg.entry")
    module.__package__ = "pkg"
    module.flag = True
    module.consume_module = lambda value: setattr(value, "__package__", "other")
    monkeypatch.setitem(sys.modules, module.__name__, module)
    # Each operation starts from an exact stale anchor. Earlier conservative
    # class/preparation/callback effects cannot make this discriminator pass.
    source = (
        setup + "\n__package__ = 'pkg'\n" + operation + "\nfrom .child import value\n"
    )
    exec(compile(source, "<protocol-relative-import-oracle>", "exec"), module.__dict__)
    assert module.value == "other"
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(source, (3, 12))


@pytest.mark.parametrize(
    "storage",
    ["stored = value.apply", "stored = [value.apply]", "stored = (value.apply,)"],
)
def test_plain_class_method_lookup_only_transports_body_provenance(storage):
    source = (
        "exposed = globals()\nclass Change:\n"
        "    def apply(self):\n        exposed['__package__'] = 'other'\n"
        "value = Change()\n__package__ = 'pkg'\n" + storage + "\n"
    )
    namespace = {"__package__": "pkg"}
    exec(compile(source, "<method-storage-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "pkg"
    assert _modules(source + "from .child import value\n", (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize(
    "prefix,expected",
    [
        ("counter = 10\ncounter -= 1\ncounter -= 1\n", 8),
        ("counter = 10\ncounter = counter - 1\ncounter = counter - 1\n", 8),
        ("counter = 0\nfor item in (1, 2):\n    counter += item\n", 3),
        ("counter = 1\nfor item in (2, 3):\n    counter *= item\n", 6),
        ("counter = 8\ncounter /= 2\ncounter /= 2\n", 2.0),
        ("counter = 10\ncounter //= 2\ncounter %= 3\n", 2),
        ("counter = 3\ncounter <<= 2\ncounter >>= 1\ncounter ^= 2\n", 4),
        ("counter = 3\ncounter |= 4\ncounter &= 6\n", 6),
        ("counter = True\ncounter ^= False\ncounter &= True\n", True),
        ("counter = 'a'\ncounter *= 2\ncounter += 'b'\n", "aab"),
        ("counter = b'a'\ncounter *= 2\ncounter += b'b'\n", b"aab"),
        (
            "counter = 2\nfor exponent in (-1, 2):\n    counter **= exponent\n    counter += 1\n",
            3.25,
        ),
        (
            "counter = 0\nfor item in (1, 2.5):\n    counter += item\n    counter = +counter\n",
            3.5,
        ),
        (
            "counter = 0\nfor choice in (True, False):\n    counter = 1 if choice else 1.5\n    counter += 1\n",
            2.5,
        ),
        (
            "counter = 0\nfor base in (-1.0, 1.0):\n    counter = base ** 0.5\n    counter += 1\n",
            2.0,
        ),
        ("counter = ''\n" + "counter += 'x'\n" * 2, "xx"),
    ],
)
def test_scalar_operation_family_preserves_actual_import_anchor(
    monkeypatch: pytest.MonkeyPatch, prefix: str, expected: object
) -> None:
    import importlib.machinery
    import sys
    import types

    package = types.ModuleType("pkg")
    package.__path__ = []
    child = types.ModuleType("pkg.child")
    child.value = "pkg"
    monkeypatch.setitem(sys.modules, "pkg", package)
    monkeypatch.setitem(sys.modules, "pkg.child", child)
    source = prefix + "from .child import value\n"
    namespace = {
        "__name__": "pkg.entry",
        "__package__": "pkg",
        "__spec__": importlib.machinery.ModuleSpec("pkg.entry", None),
    }
    exec(compile(source, "<scalar-operation-import-oracle>", "exec"), namespace)
    assert namespace["counter"] == expected
    assert type(namespace["counter"]) is type(expected)
    assert namespace["value"] == namespace["__package__"] == "pkg"
    assert _modules(source, (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize("kind", ["str", "bytes"])
def test_repeated_concatenation_stops_folding_and_preserves_import_anchor(kind) -> None:
    literal = "'x'" if kind == "str" else "b'x'"
    prefix = f"counter = {literal}\n" + "counter += counter\n" * 16
    namespace = {"__package__": "pkg"}
    exec(compile(prefix, "<bounded-concat-oracle>", "exec"), namespace)
    assert len(namespace["counter"]) == 65536
    assert _modules(prefix + "from .child import value\n", (3, 12)) == ("pkg.child",)
    tree = ast.parse(prefix + "counter\n")
    index = analyze_python_bindings(tree, source_digest=python_ast_digest(tree))
    fact = index.expression_fact(tree.body[-1].value)
    assert fact is not None and fact.result.kind == kind
    assert not fact.result.value_known


@pytest.mark.parametrize("operation", ["counter -= operand", "operand -= 1"])
def test_scalar_subclass_reflected_and_inplace_dispatch_keep_import_custody(
    operation: str,
) -> None:
    namespace: dict[str, object] = {"__package__": "pkg", "counter": 10}
    calls: list[str] = []

    class Operand(int):
        def __rsub__(self, other):
            calls.append("reflected")
            namespace["__package__"] = "other"
            return other - int(self)

        def __isub__(self, other):
            calls.append("inplace")
            namespace["__package__"] = "other"
            return int(self) - other

    namespace["operand"] = Operand(2)
    prefix = "counter = 10\n" + operation + "\n"
    exec(compile(prefix, "<scalar-subclass-oracle>", "exec"), namespace)
    assert calls == (["reflected"] if operation.startswith("counter") else ["inplace"])
    assert namespace["__package__"] == "other"
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(prefix + "from .child import value\n", (3, 12))


@pytest.mark.parametrize("constructor", ["list", "tuple"])
@pytest.mark.parametrize("published", [False, True])
def test_builtin_copy_retains_deferred_elements_without_running_them(
    constructor, published
):
    setup = "g = ((__package__ := 'other') for _ in (0,))\n"
    if published:
        setup += "items = (g,)\n"
    source = setup + f"stored = {constructor}({'items' if published else '(g,)'})\n"
    namespace = {"__package__": "pkg"}
    exec(compile(source, "<deferred-copy-custody-oracle>", "exec"), namespace)
    assert namespace["stored"][0] is namespace["g"]
    assert namespace["__package__"] == "pkg"
    assert _modules(source + "from .child import value\n", (3, 12)) == ("pkg.child",)


@pytest.mark.parametrize("factory", ["expression", "function", "coroutine"])
def test_exact_deferred_object_method_storage_keeps_import_anchor(factory):
    setup = {
        "expression": "g = ((__package__ := 'other') for _ in (0,))\n",
        "function": "def stream():\n    global __package__\n    __package__ = 'other'\n    yield 1\ng = stream()\n",
        "coroutine": "async def stream():\n    global __package__\n    __package__ = 'other'\ng = stream()\n",
    }[factory]
    source = setup + "stored = g.send\n"
    namespace = {"__package__": "pkg"}
    try:
        exec(compile(source, "<deferred-method-storage-oracle>", "exec"), namespace)
        assert namespace["__package__"] == "pkg"
        assert _modules(source + "from .child import value\n", (3, 12)) == (
            "pkg.child",
        )
    finally:
        if "g" in namespace:
            namespace["g"].close()


@pytest.mark.parametrize(
    "setup,operation",
    [
        (
            "class Change:\n    def __init__(self):\n        global __package__\n        __package__ = 'other'",
            "Change()",
        ),
        (
            "class Change:\n    def __new__(cls):\n        global __package__\n        __package__ = 'other'\n        return object.__new__(cls)",
            "Change()",
        ),
        (
            "class Meta(type):\n    def __call__(cls):\n        global __package__\n        __package__ = 'other'\nclass Change(metaclass=Meta): pass",
            "Change()",
        ),
        (
            "class Change:\n    def __del__(self):\n        global __package__\n        __package__ = 'other'",
            "Change()",
        ),
        (
            "class Change:\n    def __getattr__(self, name):\n        global __package__\n        __package__ = 'other'\n        return 1",
            "Change().missing",
        ),
        (
            "class Items(tuple):\n    def __iter__(self):\n        global __package__\n        __package__ = 'other'\n        return iter(())",
            "list(Items())",
        ),
        (
            "class Returned:\n    def __getattr__(self, name):\n        global __package__\n        __package__ = 'other'\n        return 1\nclass Change:\n    def __new__(cls):\n        return Returned()",
            "Change().missing",
        ),
        (
            "class Argument:\n    def __del__(self):\n        global __package__\n        __package__ = 'other'\nclass Change: pass",
            "try:\n    Change(Argument())\nexcept TypeError:\n    pass",
        ),
    ],
)
def test_deferred_storage_custody_preserves_actual_protocol_callbacks(setup, operation):
    source = setup + "\n__package__ = 'pkg'\n" + operation + "\n"
    namespace = {"__package__": "pkg"}
    exec(compile(source, "<deferred-storage-protocol-oracle>", "exec"), namespace)
    assert namespace["__package__"] == "other"
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(source + "from .child import value\n", (3, 12))


def test_default_class_constructor_fact_expires_at_unknown_callback():
    from molt.compiler_analysis.static_truth import (
        StaticExpressionResult,
        expression_result_without_value_facts,
        join_static_expression_results,
    )

    inert = StaticExpressionResult(class_instantiation_inert=True)
    dynamic = StaticExpressionResult()
    assert inert != dynamic
    assert join_static_expression_results((inert, inert)).class_instantiation_inert
    assert not join_static_expression_results(
        (inert, dynamic)
    ).class_instantiation_inert
    assert not expression_result_without_value_facts(inert).class_instantiation_inert


@pytest.mark.parametrize("flag", [False, True])
def test_mixed_scalar_truth_keeps_loop_arithmetic_and_import_anchor(monkeypatch, flag):
    import sys
    import types
    from molt.compiler_analysis.python_binding_flow import (
        analyze_python_source_bindings,
    )
    from molt.compiler_analysis.python_effects_generated import (
        NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
    )

    source = (
        "value = False if flag is True else 1.0\n"
        "__package__ = 'pkg'\n"
        "while value:\n"
        "    value -= 0.5\n"
        "from .child import value as value_from_child\n"
    )
    package = types.ModuleType("pkg")
    package.__path__ = []
    child = types.ModuleType("pkg.child")
    child.value = "pkg"
    monkeypatch.setitem(sys.modules, "pkg", package)
    monkeypatch.setitem(sys.modules, "pkg.child", child)
    namespace = {"flag": flag, "__name__": "pkg.entry", "__spec__": None}
    exec(compile(source, "<mixed-scalar-truth-oracle>", "exec"), namespace)
    assert namespace["value"] == 0
    assert namespace["__package__"] == "pkg"
    assert namespace["value_from_child"] == "pkg"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    loop = tree.body[2]
    truth = index.expression_fact(loop.test)
    assert truth is not None
    assert truth.result.scalar_kinds == frozenset({"bool", "float"})
    assert not truth.truth_effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    operation = index.statement_fact(loop.body[0])
    assert operation is not None
    assert not operation.effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    assert _modules(source, (3, 12)) == ("pkg.child",)


def test_unknown_truth_can_replace_scalar_binding_before_loop(monkeypatch):
    import sys
    import types
    from molt.compiler_analysis.python_binding_flow import (
        analyze_python_source_bindings,
    )
    from molt.compiler_analysis.python_effects_generated import (
        NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
    )

    source = (
        "value = False if flag else 1.0\n"
        "__package__ = 'pkg'\n"
        "while value:\n"
        "    value -= 0.5\n"
        "from .child import value as value_from_child\n"
    )
    package = types.ModuleType("other")
    package.__path__ = []
    child = types.ModuleType("other.child")
    child.value = "callback package"
    monkeypatch.setitem(sys.modules, "other", package)
    monkeypatch.setitem(sys.modules, "other.child", child)
    namespace = {"__name__": "pkg.entry", "__spec__": None}
    events = []

    class CallbackTruth:
        def __bool__(self):
            events.append("loop truth")
            namespace["__package__"] = "other"
            return False

    class PackageOnRelease:
        def __del__(self):
            events.append(("release", namespace["__package__"]))
            namespace["value"] = CallbackTruth()

    class Flag:
        def __bool__(self):
            namespace["__package__"] = PackageOnRelease()
            return False

    namespace["flag"] = Flag()
    exec(compile(source, "<unknown-truth-release-oracle>", "exec"), namespace)
    assert events == [("release", "pkg"), "loop truth"]
    assert namespace["value_from_child"] == "callback package"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    truth = index.expression_fact(tree.body[2].test)
    assert truth is not None
    assert not truth.result.scalar_kinds
    assert truth.truth_effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
        _modules(source, (3, 12))
