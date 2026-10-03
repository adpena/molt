from __future__ import annotations

import ast
import sys
import types
from collections import Counter

import pytest

from molt.frontend import MoltOp, SimpleTIRGenerator
from molt.frontend._types import CodeSlotDeclaration


@pytest.mark.parametrize(
    "body",
    [
        "if flag: later\nearly = 1\nlater = 2",
        "x = (y := 1)",
        "@decorator(later)\ndef nested(x=default): pass\n"
        "later = 1\ndefault = 2\ndecorator = 3",
        "try:\n a = 1\nexcept E as error:\n b = 2\nelse:\n c = 3\nfinally:\n d = 4",
        "match subject:\n case cls(field) if guard: pass\n"
        "subject = 1\ncls = 2\nguard = 3",
        "result = [item + local for item in source]\nlocal = 1\nsource = []",
        "result = [(found := item) for item in source]\nsource = []",
        "local = 1\ndef inner(): return local\nother = 2",
        "x: T\nT = 1",
        "res = [[j for j in row] for row in source]\nsource = []",
        "result = [x for x in source]\nreturn x",
        "result = {(a := 1): (b := 2), (c := 3): (d := 4)}",
        "from .foo import bar\nimport a.b.c as named",
        "if False:\n hidden = missing\nreturn\nafter = other",
        "with manager as first, other(first) as second:\n result = second",
        "gen = (item + outside for item in source)\nsource = []\noutside = 1",
        "return sum(arg for item in (1, 2))",
        "return [lambda: item for item in (1, 2)]",
        "item = 7\nearly = 8\nreaders = [lambda: item for item in (1, 2)]\n"
        "return item, readers",
        "readers = [lambda: arg for arg in (1, 2)]\nreturn arg, readers",
        "readers = [lambda: item for item in (1, 2)]\nreturn item",
        "if arg:\n factory = lambda values=[(found := item) for item in (1, 2)]: values\n"
        "return found",
    ],
)
def test_callable_code_names_match_independent_cpython_layout(body: str) -> None:
    source = "def probe(arg, /, *args, kw=0, **kwargs):\n" + "".join(
        "    " + line + "\n" for line in body.splitlines()
    )
    reference = next(
        value
        for value in compile(
            source, "<code-layout>", "exec", dont_inherit=True
        ).co_consts
        if isinstance(value, types.CodeType)
    )
    node = ast.parse(source).body[0]
    assert isinstance(node, ast.FunctionDef)
    generator = SimpleTIRGenerator(target_python=sys.version_info[:2])
    cells = generator._callable_cell_plan(node).cellvars
    assert cells == reference.co_cellvars
    layout = generator._collect_callable_name_layout(
        posonly_params=["arg"],
        pos_or_kw_params=[],
        kwonly_params=["kw"],
        vararg="args",
        varkw="kwargs",
        body=node.body,
        cell_vars=cells,
    )
    assert layout.varnames == reference.co_varnames
    assert layout.names == reference.co_names


def test_inlined_cell_metadata_preserves_enclosing_binding_identity() -> None:
    generator = SimpleTIRGenerator(target_python=(3, 12))
    node = ast.parse(
        "def probe(arg):\n"
        "    readers = [lambda: item for item in (1, 2)]\n"
        "    return item, readers\n"
    ).body[0]
    plan = generator._callable_cell_plan(node)
    assert plan.cellvars == ("item",)
    assert plan.captured == ()  # The final item read still resolves globally.
    node = ast.parse(
        "def probe(arg):\n"
        "    readers = [lambda: arg for arg in (1, 2)]\n"
        "    return arg, readers\n"
    ).body[0]
    plan = generator._callable_cell_plan(node)
    assert plan.cellvars == plan.captured == ("arg",)


def test_default_comprehension_private_cell_is_planned_before_branch() -> None:
    generator = SimpleTIRGenerator(target_python=(3, 12))
    node = ast.parse(
        "def probe(flag):\n"
        "    if flag:\n"
        "        factory = lambda values=[(found := item) for item in (1, 2)]: values\n"
        "    return found\n"
    ).body[0]
    plan = generator._callable_cell_plan(node)
    assert plan.private == ("found",)
    assert plan.cellvars == plan.captured == ()


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_deep_conditional_collectors_share_traversal_not_branch_policy(
    target: tuple[int, int],
) -> None:
    source = (
        "".join(
            f"{'if' if number == 0 else 'elif'} flag == {number}:\n"
            f"    value_{number}: int = {number}\n"
            for number in range(384)
        )
        + "else:\n    final: int = 0\n"
    )
    compile(source, "<collector-depth>", "exec")
    tree = ast.parse(source)
    generator = SimpleTIRGenerator(module_name="condition_flow", target_python=target)
    expected = [*(f"value_{number}" for number in range(384)), "final"]
    assert generator._collect_assigned_names_ordered(tree.body) == expected
    counts, functions, _dynamic = generator._collect_module_assignments(tree)
    assert counts == dict.fromkeys(expected, 1)
    assert not functions
    items, _ids = generator._collect_module_annotation_items(tree)
    assert [name for name, _annotation, _index in items] == expected
    names = generator._collect_code_names_for_body(
        tree.body, varnames=(), free_vars=(), module_scope=True
    )
    assert [name for name in names if name in counts] == expected
    dead = ast.parse("if False:\n    hidden: int = 1\nelse:\n    visible: int = 2\n")
    assert generator._collect_assigned_names_ordered(dead.body) == ["hidden", "visible"]
    assert generator._collect_module_assignments(dead)[0] == {"visible": 1}
    assert [
        name for name, _, _ in generator._collect_module_annotation_items(dead)[0]
    ] == ["visible"]


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("future", [False, True])
@pytest.mark.parametrize("generic", [False, True])
def test_lexical_header_consumers_share_annotation_and_default_scope(
    target: tuple[int, int], future: bool, generic: bool
) -> None:
    # Non-eager/generic walrus annotations are deliberately AST-only: CPython
    # rejects them after parsing. Valid generic defaults remain enclosing-scope.
    source = (
        f"def inner{'[T]' if generic else ''}("
        "value: (annotation := int) = (default := 1), "
        "*, keyword=(kwdefault := 2)):\n"
        "    hidden = (body_only := 3)\n"
    )
    tree = ast.parse(source)
    generator = SimpleTIRGenerator(module_name="condition_flow", target_python=target)
    generator.future_annotations = future
    eager = target < (3, 14) and not future and not generic
    expected = ["default", "kwdefault", *(["annotation"] if eager else [])]
    assert generator._collect_namedexpr_names(tree) == expected
    assert generator._collect_assigned_names_ordered(tree.body) == ["inner", *expected]
    counts, functions, dynamic = generator._collect_module_assignments(tree)
    assert counts == dict.fromkeys(["inner", *expected], 1)
    assert functions == {"inner"} and not dynamic


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("future", [False, True])
@pytest.mark.parametrize("generic", [False, True])
def test_code_names_use_only_current_definition_header_scope(
    target: tuple[int, int], future: bool, generic: bool
) -> None:
    tree = ast.parse(
        f"def inner{'[T]' if generic else ''}(value: Annotation = Default):\n"
        "    return Hidden\n"
    )
    generator = SimpleTIRGenerator(module_name="condition_flow", target_python=target)
    generator.future_annotations = future
    names = generator._collect_code_names_for_body(
        tree.body, varnames=(), free_vars=(), module_scope=True
    )
    expected = ["Default"]
    if target < (3, 14) and not future and not generic:
        expected.append("Annotation")
    assert names == [*expected, "inner"]


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("future", [False, True])
def test_variable_annotation_projection_does_not_double_count_target_walrus(
    target: tuple[int, int], future: bool
) -> None:
    # Syntax-only policy coverage; deferred/future annotation walruses are not
    # accepted executable Python. Target/value writes still belong here.
    tree = ast.parse("owner[(key := 0)]: (annotation := int) = (value := 1)\n")
    generator = SimpleTIRGenerator(module_name="condition_flow", target_python=target)
    generator.future_annotations = future
    expected = ["key", "value"]
    if target < (3, 14) and not future:
        expected.append("annotation")
    assert generator._collect_namedexpr_names(tree) == expected
    assert generator._collect_assigned_names_ordered(tree.body) == expected
    counts, _, _ = generator._collect_module_assignments(tree)
    assert counts == dict.fromkeys(expected, 1)


@pytest.mark.parametrize("target", [(3, 12), (3, 13)])
def test_local_variable_annotation_declaration_is_not_a_runtime_name_load(
    target: tuple[int, int],
) -> None:
    tree = ast.parse("value: (annotation_local := External)\n")
    generator = SimpleTIRGenerator(module_name="condition_flow", target_python=target)
    assert generator._collect_assigned_names_ordered(tree.body) == [
        "value",
        "annotation_local",
    ]
    assert (
        generator._collect_code_names_for_body(
            tree.body,
            varnames=("value", "annotation_local"),
            free_vars=(),
            module_scope=False,
        )
        == []
    )


@pytest.mark.parametrize(
    "source, expected",
    [
        ("(a := (b := value))", ["a", "b"]),
        ("(a := (b := (a := value)))", ["a", "b"]),
        ("lambda value=(a := (b := 1)): (deferred := value)", ["a", "b"]),
        ("lambda *, value=(a := (b := 1)): (deferred := value)", ["a", "b"]),
        ("def f(value=(a := (b := 1))):\n    deferred = (hidden := 2)\n", ["a", "b"]),
        (
            "async def f(value=(a := (b := 1))):\n    deferred = (hidden := 2)\n",
            ["a", "b"],
        ),
        (
            "@(decorate := wrapper)\ndef f():\n    hidden = (deferred := 1)\n",
            ["decorate"],
        ),
        (
            "class C((base := object), metaclass=(meta := type)):\n    hidden = (deferred := 1)\n",
            ["base", "meta"],
        ),
    ],
)
def test_namedexpr_collection_tracks_immediate_scope_expressions(
    source: str, expected: list[str]
) -> None:
    generator = SimpleTIRGenerator(module_name="condition_flow")
    assert generator._collect_namedexpr_names(ast.parse(source)) == expected


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("future", [False, True])
def test_namedexpr_signature_collection_respects_annotation_evaluation(
    target: tuple[int, int], future: bool
) -> None:
    generator = SimpleTIRGenerator(module_name="condition_flow", target_python=target)
    generator.future_annotations = future
    tree = ast.parse("def f(a: (parameter := int)) -> (result := str):\n    pass\n")
    expected = ["parameter", "result"] if target < (3, 14) and not future else []
    assert generator._collect_namedexpr_names(tree) == expected


@pytest.mark.parametrize(
    "source, expected",
    [
        ("value = (outer := (inner := 1))", ["value", "outer", "inner"]),
        ("value: int = (outer := (inner := 1))", ["value", "outer", "inner"]),
        ("value += (outer := (inner := 1))", ["value", "outer", "inner"]),
        (
            "fn = lambda value=(outer := (inner := 1)): (hidden := value)",
            ["fn", "outer", "inner"],
        ),
        (
            "def fn(value=(outer := (inner := 1))):\n    hidden = 2\n",
            ["fn", "outer", "inner"],
        ),
        (
            "async def fn(value=(outer := (inner := 1))):\n    hidden = 2\n",
            ["fn", "outer", "inner"],
        ),
        ("class C((base := object)):\n    hidden = 2\n", ["C", "base"]),
        ("type Alias = int", ["Alias"]),
        ("a[(key := f())] = value", ["key"]),
        ("a[(key := f())]: int = value", ["key"]),
        ("a[(key := f())] += value", ["key"]),
        ("del a[(key := 0)]", ["key"]),
        ("a[(key := 0)], value = source", ["key", "value"]),
        ("value: (annotation := int) = 1", ["value", "annotation"]),
        ("value = [(outer := item) for item in data]", ["value", "outer"]),
        (
            "if False:\n    value = (outer := (inner := 1))\n",
            ["value", "outer", "inner"],
        ),
    ],
)
def test_assigned_name_consumers_share_ordered_scoped_authority(
    source: str, expected: list[str]
) -> None:
    generator = SimpleTIRGenerator(module_name="condition_flow")
    body = ast.parse(source).body
    assert generator._collect_assigned_names_ordered(body) == expected
    assert generator._collect_assigned_names(body) == set(expected)


@pytest.mark.parametrize(
    "source, expected",
    [
        (
            "fn = lambda value=(outer := (outer := 1)): (hidden := value)",
            {"fn": 1, "outer": 2},
        ),
        (
            "def fn(value=(outer := (inner := 1))):\n    hidden = 2\n",
            {"fn": 1, "outer": 1, "inner": 1},
        ),
        ("class C((base := object)):\n    hidden = 2\n", {"C": 1, "base": 1}),
        ("value = [(outer := item) for item in data]", {"value": 1, "outer": 1}),
        ("a[(key := 0)] = value", {"key": 1}),
        ("type Alias = int", {"Alias": 1}),
    ],
)
def test_module_binding_counts_share_scoped_walrus_walk_without_deduplicating_writes(
    source: str, expected: dict[str, int]
) -> None:
    generator = SimpleTIRGenerator(module_name="condition_flow")
    counts, _, _ = generator._collect_module_assignments(ast.parse(source))
    assert counts == expected


def _ops(
    source: str,
    *,
    phi: bool,
    target_python: tuple[int, int] = (3, 12),
    function_name: str | None = None,
) -> list[MoltOp]:
    generator = SimpleTIRGenerator(
        module_name="condition_flow", enable_phi=phi, target_python=target_python
    )
    generator.visit(ast.parse(source))
    if function_name is not None:
        return generator.funcs_map[function_name]["ops"]
    return [op for function in generator.funcs_map.values() for op in function["ops"]]


@pytest.mark.parametrize("phi", [False, True])
@pytest.mark.parametrize(
    "expression",
    [
        "a and b and c",
        "a or b or c",
        "(a and b) or c",
        "a < b < c",
        "a if b and c else d",
    ],
)
def test_short_circuit_values_never_reapply_binary_truth(
    expression: str, phi: bool
) -> None:
    ops = _ops(f"def f(a, b, c, d):\n    return {expression}\n", phi=phi)
    assert not {"AND", "OR"}.intersection(op.kind for op in ops)
    assert "IF" in {op.kind for op in ops}


@pytest.mark.parametrize("phi", [False, True])
def test_rich_comparison_value_is_not_claimed_to_be_exact_bool(phi: bool) -> None:
    ops = _ops("def f(a, b, c):\n    return a < b < c\n", phi=phi)
    comparisons = [op for op in ops if op.kind == "LT"]
    assert len(comparisons) == 2
    assert all(op.result.type_hint == "Any" for op in comparisons)


@pytest.mark.parametrize("phi", [False, True])
def test_flat_boolean_chain_has_nested_not_sequential_exit_tests(phi: bool) -> None:
    ops = _ops("def f(a, b, c, d):\n    return a and b and c and d\n", phi=phi)
    controls = [op.kind for op in ops if op.kind in {"IF", "ELSE", "END_IF"}]
    assert controls == [
        "IF",
        "ELSE",
        "IF",
        "ELSE",
        "IF",
        "ELSE",
        "END_IF",
        "END_IF",
        "END_IF",
    ]


@pytest.mark.parametrize("phi", [False, True])
@pytest.mark.parametrize(
    "statement",
    [
        "if a and b and c:\n        return 1",
        "while a and b and c:\n        break",
        "assert a and b and c",
        "return [1 for x in (1,) if a and b and c]",
        "return sum(1 for x in (1,) if a and b and c)",
        "return any(x for x in (1,) if a and b and c)",
        "match a:\n        case _ if a and b and c:\n            return 1",
        "return not (a and b and c)",
    ],
)
def test_all_syntax_condition_consumers_use_shared_flow(
    statement: str, phi: bool
) -> None:
    ops = _ops(f"def f(a, b, c):\n    {statement}\n", phi=phi)
    assert not {"AND", "OR"}.intersection(op.kind for op in ops)


@pytest.mark.parametrize("phi", [False, True])
@pytest.mark.parametrize("operator", ["and", "or"])
def test_suspending_boolean_flow_uses_frame_storage(operator: str, phi: bool) -> None:
    ops = _ops(
        f"async def f(a, b):\n    return a {operator} await b\n",
        phi=phi,
        function_name="condition_flow__f_poll",
    )
    baseline = _ops(
        "async def f(a, b):\n    return await b\n",
        phi=phi,
        function_name="condition_flow__f_poll",
    )
    kinds = Counter(op.kind for op in ops)
    baseline_kinds = Counter(op.kind for op in baseline)
    assert kinds["STORE_CLOSURE"] > baseline_kinds["STORE_CLOSURE"]
    assert kinds["LOAD_CLOSURE"] > baseline_kinds["LOAD_CLOSURE"]
    # Await protocol adaptation itself emits boolean guards and a result cell.
    # The short-circuit merge must not add another such lane to that body.
    for kind in ("AND", "OR", "STORE_INDEX"):
        assert kinds[kind] == baseline_kinds[kind]


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("phi", [False, True])
def test_nested_value_boolop_retest_is_target_version_gated(
    target_python: tuple[int, int], phi: bool
) -> None:
    ops = _ops(
        "def f(a, b, c):\n    return (a and b) or c\n",
        phi=phi,
        target_python=target_python,
    )
    kinds = [op.kind for op in ops]
    first_join = kinds.index("END_IF")
    next_branch = kinds.index("IF", first_join + 1)
    assert ("BOOL" in kinds[first_join + 1 : next_branch]) == (target_python < (3, 14))


@pytest.mark.parametrize("target_python", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("operand", ["a < b < c", "a and b if c else d"])
def test_comparison_and_ifexp_value_boundaries_always_retest(
    target_python: tuple[int, int], operand: str
) -> None:
    ops = _ops(
        f"def f(a, b, c, d):\n    return ({operand}) or d\n",
        phi=True,
        target_python=target_python,
    )
    kinds = [op.kind for op in ops]
    last_bool = max(index for index, kind in enumerate(kinds) if kind == "BOOL")
    assert "END_IF" in kinds[:last_bool]


@pytest.mark.parametrize("phi", [False, True])
@pytest.mark.parametrize(
    "body",
    [
        "flag and (x := 1)\n    return x",
        "flag or (x := 1)\n    return x",
        "return (x := 1) if flag else x",
        "return x if flag else (x := 1)",
        "0 < flag < (x := 1)\n    return x",
    ],
)
def test_conditional_walrus_retains_unbound_guard(body: str, phi: bool) -> None:
    ops = _ops(f"def f(flag):\n    {body}\n", phi=phi)
    assert any(
        op.kind == "CONST_STR"
        and op.args
        and op.args[0]
        == "cannot access local variable 'x' where it is not associated with a value"
        for op in ops
    )


@pytest.mark.parametrize("phi", [False, True])
def test_conditional_walrus_joins_local_type_hint(phi: bool) -> None:
    ops = _ops(
        "def f(flag):\n    x = 1\n    flag and (x := 'text')\n    return x\n",
        phi=phi,
        function_name="condition_flow__f",
    )
    loads = [
        op
        for op in ops
        if op.kind == "LOAD_VAR" and (op.metadata or {}).get("var") == "x"
    ]
    assert loads[-1].result.type_hint == "Any"


def test_module_walrus_uses_live_namespace_after_conditional_join() -> None:
    ops = _ops("x = 1\nflag and (x := 2)\nresult = x\n", phi=True)
    names = {
        op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR" and op.args
    }
    assert any(
        op.kind == "MODULE_GET_GLOBAL"
        and len(op.args) > 1
        and names.get(op.args[1].name) == "x"
        for op in ops
    )


def test_conditional_module_alias_retains_all_provenance_paths() -> None:
    generator = SimpleTIRGenerator(module_name="condition_flow")
    generator.visit(
        ast.parse("import sys\nnamespace = sys\nflag and (namespace := object())\n")
    )
    provenance = generator.imported_module_provenance["namespace"]
    assert "sys" in provenance
    assert len(provenance) == 2


@pytest.mark.parametrize(
    "source",
    [
        "def f(arg):\n    item = 7\n    readers = [lambda: item for item in (1, 2)]\n    return item, readers",
        "def f(arg):\n    return [lambda: item for item in (1, 2)]",
        "def f(arg):\n    local = 7\n    reader = lambda: (arg, local)\n    return reader",
        "def f(arg, /, extra=1, *rest, key=2, **kw):\n    return lambda: (arg, rest, kw)",
        "def outer(value):\n    def f(arg):\n        return [lambda: (item, value, arg) for item in (1, 2)]\n    return f",
    ],
)
def test_code_slot_declaration_matches_cpython_slot_lookup(source: str) -> None:
    # CPython's own lookup checks non-parameter overlaps independently of the
    # public-table projection used by the frontend and runtime.
    pending = [compile(source, "<slots>", "exec")]
    while pending:
        code = pending.pop()
        pending.extend(
            item for item in code.co_consts if isinstance(item, types.CodeType)
        )
        if code.co_name != "f":
            continue
        parameter_count = code.co_argcount + code.co_kwonlyargcount
        parameter_count += bool(code.co_flags & 0x04) + bool(code.co_flags & 0x08)
        declaration = CodeSlotDeclaration(
            code.co_varnames[:parameter_count],
            code.co_varnames,
            code.co_cellvars,
            code.co_freevars,
        )
        expected = []
        for index in range(
            len(code.co_varnames) + len(code.co_cellvars) + len(code.co_freevars)
        ):
            try:
                expected.append(code._varname_from_oparg(index))
            except IndexError:
                break
        assert declaration.slots() == tuple(expected)
        assert declaration.slots() is declaration.slots()
        assert list(declaration.release_order((3, 12))) == list(range(len(expected)))
        assert list(declaration.release_order((3, 13))) == list(range(len(expected)))
        assert list(declaration.release_order((3, 14))) == list(
            reversed(range(len(expected)))
        )
        return
    raise AssertionError("the oracle fixture has no f code object")


def test_code_slot_declaration_rejects_inconsistent_parameter_prefix() -> None:
    with pytest.raises(ValueError, match="parameters must prefix"):
        CodeSlotDeclaration(("argument",), ("other",), (), ())
