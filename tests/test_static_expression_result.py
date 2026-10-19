from __future__ import annotations

import ast
import codecs
from dataclasses import replace
import io
import math
import os
from pathlib import Path
import runpy

import pytest

from molt.compiler_analysis.static_truth import (
    ExpressionSequenceItem,
    ExpressionKind,
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    _same_scalar_value,
    expression_result_for_owned_binding,
    expression_result_for_publication,
    expression_result_without_mutable_contents,
    iterable_element_result,
    join_static_expression_results,
    static_binary_result,
    static_expression_result,
)
from molt.compiler_analysis.python_binding_flow import analyze_python_source_bindings
from molt.compiler_analysis.python_effects import (
    AccumulatedKeyEffects,
    binary_operation_effects,
    expression_effect_mask,
    unary_operation_effects,
    expression_may_execute_python,
    iterable_unpack_effects,
)
from molt.compiler_analysis.python_effects_generated import (
    EXECUTES_ARBITRARY_PYTHON,
    INVOKES_COMPARISON_CALLBACK,
    NO_EFFECTS,
    NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
    RAISES,
)


def test_cpython_owned_result_lifetime_capsule() -> None:
    # One replayable source owns the oracle and future native/WASM comparison.
    runpy.run_path(
        str(Path(__file__).parent / "differential/basic/builtin_shape_lifetimes.py")
    )


@pytest.mark.parametrize(
    "expression",
    [
        "(1, 2)[0]",
        "[1, 2][-1]",
        "('pkg', 'pkg.alt')[True]",
        "(*('pkg',), 'pkg.alt')[1]",
        "'package'[2]",
        "b'package'[2]",
        "'package'[::-1]",
        "b'package'[1:4]",
    ],
)
def test_builtin_subscription_scalar_matches_cpython(expression: str) -> None:
    node = ast.parse(expression, mode="eval").body
    expected = eval(compile(ast.Expression(node), "<subscription-oracle>", "eval"))
    result = static_expression_result(node)
    assert result.value_known
    assert type(result.value) is type(expected)
    assert result.value == expected
    assert result.evaluation_required
    index = analyze_python_source_bindings(f"result = {expression}\n")
    indexed_node = ast.parse(f"result = {expression}\n").body[0].value
    fact = index.expression_fact(indexed_node)
    assert fact is not None
    assert fact.result.value_known and fact.result.value == expected
    assert not fact.effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS


@pytest.mark.parametrize(
    "expression", ["(1, 2)[0]", "[1, 2][True]", "'abc'[1]", "b'abc'[1:2]"]
)
def test_syntax_subscription_projection_reuses_builtin_protocol(
    expression: str,
) -> None:
    assert not expression_may_execute_python(ast.parse(expression, mode="eval").body)


@pytest.mark.parametrize(
    "expression", ["(1, 2)[1:]", "[1, 2][::-1]", "range(4)[1]", "bytearray(b'ab')[0]"]
)
def test_builtin_subscription_normal_kind_matches_cpython(expression: str) -> None:
    expected = eval(expression)
    source = f"result = {expression}\n"
    index = analyze_python_source_bindings(source)
    node = ast.parse(source).body[0].value
    fact = index.expression_fact(node)
    assert fact is not None
    assert fact.result.kind == type(expected).__name__
    assert not fact.effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS


@pytest.mark.parametrize("expression", ["(1,)[4]", "[1]['bad']", "'abc'[::0]"])
def test_invalid_builtin_subscription_raises_without_a_protocol_callback(
    expression: str,
) -> None:
    with pytest.raises((IndexError, TypeError, ValueError)):
        eval(expression)
    source = f"result = {expression}\n"
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(ast.parse(source).body[0].value)
    assert fact is not None and fact.effects & RAISES
    assert not fact.result.value_known
    assert not fact.effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS


@pytest.mark.parametrize("selection", ["Index()", ":Index()"])
def test_subscription_index_protocol_keeps_callback_obligation(selection: str) -> None:
    source = (
        "class Index:\n"
        "    def __index__(self):\n"
        "        events.append('index')\n"
        "        return 1\n"
        f"result = (4, 5)[{selection}]\n"
    )
    events = []
    exec(compile(source, "<index-callback-oracle>", "exec"), {"events": events})
    assert events == ["index"]
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(ast.parse(source).body[-1].value)
    assert fact is not None and fact.effects & EXECUTES_ARBITRARY_PYTHON


def test_subscription_selected_scalar_does_not_elide_sibling_retirement() -> None:
    source = (
        "class Retired:\n"
        "    def __del__(self):\n"
        "        events.append('released')\n"
        "result = (Retired(), 7)[1]\n"
    )
    events = []
    namespace = {"events": events}
    exec(compile(source, "<subscript-retirement-oracle>", "exec"), namespace)
    assert namespace["result"] == 7 and events == ["released"]
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(ast.parse(source).body[-1].value)
    assert fact is not None and fact.result.value_known and fact.result.value == 7
    assert fact.effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS


def test_subscription_expires_captured_mutable_contents_after_index_callback() -> None:
    source = (
        "values = [1]\n"
        "def replace():\n"
        "    values[0] = 'changed'\n"
        "    return 0\n"
        "result = values[(replace(), 0)[1]]\n"
    )
    namespace = {}
    exec(compile(source, "<captured-subscript-oracle>", "exec"), namespace)
    assert namespace["result"] == "changed"
    index = analyze_python_source_bindings(source)
    fact = index.expression_fact(ast.parse(source).body[-1].value)
    assert fact is not None and not fact.result.value_known
    assert fact.result.kind == "unknown"


def test_cpython_split_protocol_capsule() -> None:
    # The differential source is also the direct CPython protocol-order oracle.
    runpy.run_path(
        str(Path(__file__).parent / "differential/basic/split_protocol_order.py")
    )


def test_cpython_string_predicate_capsule() -> None:
    runpy.run_path(
        str(Path(__file__).parent / "differential/basic/str_predicate_protocol.py")
    )


def test_cpython_float_protocol_capsule() -> None:
    runpy.run_path(str(Path(__file__).parent / "differential/basic/float_protocol.py"))


@pytest.mark.parametrize("source", ["[]", "[1]", "{}", "{'key': 1}", "{1}"])
def test_tracked_allocation_binding_preserves_current_contents_not_freshness(
    source: str,
) -> None:
    result = static_expression_result(ast.parse(source, mode="eval").body)
    published = expression_result_for_owned_binding(result)
    assert not published.fresh_container
    assert published.items is result.items
    assert published.element_result is result.element_result
    assert published.truth is result.truth
    assert published.length == result.length
    assert published.release_may_call is result.release_may_call
    assert expression_result_for_owned_binding(published) is published
    # Arbitrary publication and later mutation still erase those facts.
    exposed = expression_result_for_publication(published)
    expired = expression_result_without_mutable_contents(published)
    assert exposed.items is None and exposed.truth is None
    assert expired.items is None and expired.element_result is None


def test_nonalias_outer_owner_expires_all_mutable_descendant_projections() -> None:
    child = static_expression_result(ast.parse("[1]", mode="eval").body)
    outer = StaticExpressionResult(
        kind="list",
        truth=True,
        length=1,
        fresh_container=True,
        release_may_call=False,
        items=(ExpressionSequenceItem(child),),
        element_result=child,
    )
    expired = expression_result_without_mutable_contents(outer, preserve_owner=True)
    assert expired.truth is True and expired.length == 1
    assert not expired.fresh_container
    assert expired.items is not None
    assert expired.items[0].result is expired.element_result
    descendant = expired.element_result
    assert descendant is not None and descendant.kind == "list"
    assert descendant.items is None and descendant.element_result is None
    assert descendant.release_may_call and expired.release_may_call


def test_nonalias_outer_owner_descendant_expiry_is_stack_safe() -> None:
    result = static_expression_result(ast.parse("[1]", mode="eval").body)
    for _ in range(1200):
        result = StaticExpressionResult(
            kind="tuple",
            items=(ExpressionSequenceItem(result),),
            element_result=result,
            release_may_call=False,
        )
    expired = expression_result_without_mutable_contents(result, preserve_owner=True)
    assert expired.release_may_call
    for _ in range(1200):
        assert expired.element_result is not None
        expired = expired.element_result
    assert expired.kind == "list" and expired.element_result is None


@pytest.mark.parametrize("kind", ["file_text", "file_bytes"])
def test_file_mode_does_not_certify_iteration_element(kind: ExpressionKind) -> None:
    assert iterable_element_result(StaticExpressionResult(kind=kind)) is None
    item = StaticExpressionResult.scalar(b"proven separately")
    assert (
        iterable_element_result(StaticExpressionResult(kind=kind, element_result=item))
        is item
    )


def test_cpython_text_decoder_can_return_a_subclass_as_a_complete_line() -> None:
    class DecodedLine(str):
        pass

    class Decoder(codecs.IncrementalDecoder):
        def decode(self, data: bytes, final: bool = False) -> str:
            return DecodedLine(data.decode("ascii"))

    def lookup(name: str) -> codecs.CodecInfo | None:
        if name != "molt_decoded_line_subclass":
            return None
        return codecs.CodecInfo(
            name=name,
            encode=codecs.ascii_encode,
            decode=codecs.ascii_decode,
            incrementalencoder=codecs.getincrementalencoder("ascii"),
            incrementaldecoder=Decoder,
        )

    codecs.register(lookup)
    try:
        with io.TextIOWrapper(
            io.BytesIO(b"whole line\n"), encoding="molt_decoded_line_subclass"
        ) as stream:
            assert type(next(stream)) is DecodedLine
    finally:
        codecs.unregister(lookup)


def test_cpython_unbuffered_file_iteration_uses_live_readline() -> None:
    # The file_bytes mode includes FileIO, whose iterator returns the result
    # of ordinary readline lookup rather than certifying an exact bytes value.
    with io.FileIO(os.devnull, "rb") as stream:
        stream.readline = lambda: ("not bytes",)
        assert next(stream) == ("not bytes",)


def test_cpython_exact_text_wrapper_enter_calls_underlying_closed() -> None:
    observations: list[str] = []

    class Buffer(io.BytesIO):
        @property
        def closed(self) -> bool:
            observations.append("closed")
            return super().closed

    # An exact wrapper's normal return is itself; its closed check can still
    # enter Python through the wrapped stream. These are independent facts.
    stream = io.TextIOWrapper(Buffer())
    observations.clear()
    with stream as entered:
        assert type(entered) is io.TextIOWrapper
        assert entered is stream
        assert observations == ["closed"]


@pytest.mark.parametrize(
    "source",
    ["[*()]", "(*(),)", "{*()}", "{**{}}", "2 == True", "[1] == True", "1 is True"],
)
def test_literal_result_matches_cpython_without_truth_value_conflation(source: str):
    expression = ast.parse(source, mode="eval").body
    expected = bool(
        eval(compile(ast.Expression(expression), "<literal-oracle>", "eval"))
    )
    assert expected is False
    assert static_expression_result(expression).truth is expected


@pytest.mark.parametrize(
    ("source", "truth"),
    [
        ("[1, *unknown]", True),
        ("[*unknown]", None),
        ("{**unknown}", None),
        ("{1: 2, **unknown}", True),
        ("[*[*((),), *()]]", True),
        ("{**{**{}}}", False),
        ("False and unknown()", False),
        ("True or unknown()", True),
        ("not not [*()]", False),
        ("('win32',) in 'win32'", None),
    ],
)
def test_shape_and_short_circuit_result_is_sound(source: str, truth: bool | None):
    assert static_expression_result(ast.parse(source, mode="eval").body).truth is truth


@pytest.mark.parametrize(
    "source",
    [
        "[callback()]",
        "[missing]",
        "{key: value}",
        "[*callback()]",
        "callback() or True",
    ],
)
def test_known_truth_never_grants_permission_to_erase_evaluation(source: str):
    assert static_expression_result(
        ast.parse(source, mode="eval").body
    ).evaluation_required


def test_source_unknown_cannot_fall_back_to_name_or_platform_spelling():
    for source in ["TYPE_CHECKING", "typing.TYPE_CHECKING", "sys.platform == 'win32'"]:
        result = static_expression_result(
            ast.parse(source, mode="eval").body,
            fact_result=lambda _node: UNKNOWN_EXPRESSION_RESULT,
        )
        assert result.truth is None


def test_source_result_provider_survives_negation_recursion():
    expr = ast.parse("not not alias", mode="eval").body
    result = static_expression_result(
        expr,
        fact_result=lambda node: (
            StaticExpressionResult.scalar(False) if isinstance(node, ast.Name) else None
        ),
    )
    assert result.truth is False


@pytest.mark.parametrize(
    "source", ["(x := ())", "(x := 1) and ()", "(x := {})", "(x := 1) and {}"]
)
def test_namedexpr_and_selected_empty_container_preserve_shape(source: str) -> None:
    expression = ast.parse(source, mode="eval").body
    direct = static_expression_result(expression)
    index = analyze_python_source_bindings(source)
    bound = index.expression_result(expression)
    for result in (direct, bound):
        assert result.truth is False
        assert result.items == ()
        assert result.kind in {"tuple", "dict"}
        assert result.evaluation_required
        keys = AccumulatedKeyEffects()
        assert keys.add(UNKNOWN_EXPRESSION_RESULT) != NO_EFFECTS
        # The expansion itself cannot collide with retained arbitrary keys.
        # Child evaluation (including release on the walrus write) is separate.
        assert keys.extend(result) == NO_EFFECTS


def test_nested_namedexpr_preserves_value_without_erasing_bindings() -> None:
    expression = ast.parse("(x := (y := 1))", mode="eval").body
    result = static_expression_result(expression)
    assert result.kind == "int" and result.value_known and result.value == 1
    assert result.evaluation_required


@pytest.mark.parametrize("arguments", ["", "1", "*items", "**mapping"])
def test_call_spelling_never_proves_purity_or_erases_expansion_callbacks(
    arguments: str,
) -> None:
    expression = ast.parse(f"pure({arguments})", mode="eval").body
    assert expression_may_execute_python(expression)


def test_unknown_rich_comparison_exit_is_not_the_false_singleton():
    expression = ast.parse("(unknown == 0 == 1) is False", mode="eval").body
    result = static_expression_result(expression)
    assert result.truth is None
    assert not result.value_known


@pytest.mark.parametrize(
    ("left", "right"),
    [(0.0, -0.0), (complex(0.0, 0.0), complex(-0.0, 0.0)), (True, 1)],
)
def test_exact_merge_identity_does_not_use_numeric_equality(left, right):
    assert left == right
    assert not _same_scalar_value(left, right)


def test_nan_does_not_manufacture_exact_merge_identity():
    nan = math.nan
    assert not _same_scalar_value(nan, nan)


def test_nested_expansion_facts_have_linear_structural_storage():
    source = "value = " + "[0, *" * 64 + "[]" + "]" * 64
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    displays = [node for node in ast.walk(tree) if isinstance(node, ast.List)]
    assert sum(
        len(index.expression_result(node).items or ()) for node in displays
    ) <= sum(len(node.elts) for node in displays)
    for node in displays:
        if node.elts:
            expanded = node.elts[1]
            assert isinstance(expanded, ast.Starred)
            item = index.expression_result(node).items[1]
            assert item.expanded
            assert item.result is index.expression_result(expanded.value)


@pytest.mark.parametrize("nan", [math.nan, complex(math.nan, 0.0)])
def test_nan_membership_does_not_assume_scalar_identity(nan):
    expression = ast.parse("needle in [needle]", mode="eval").body
    result = static_expression_result(
        expression,
        fact_result=lambda node: (
            StaticExpressionResult.scalar(nan) if isinstance(node, ast.Name) else None
        ),
    )
    assert result.truth is None


@pytest.mark.parametrize("source", ["[*()]", "{*()}", "{**{}}", "[*[1]]", "{*(1, 2)}"])
def test_exact_builtin_unpack_has_no_invented_python_callback(source: str) -> None:
    assert not expression_may_execute_python(ast.parse(source, mode="eval").body)


@pytest.mark.parametrize(
    "source", ["[*unknown]", "{*unknown}", "{**unknown}", "{unknown: 1}"]
)
def test_unknown_unpack_or_hash_retains_callback_boundary(source: str) -> None:
    assert expression_may_execute_python(ast.parse(source, mode="eval").body)


@pytest.mark.parametrize(
    "source,may_call",
    [
        ("[*()]", False),
        ("{**{1: 2}}", False),
        ("[1]", False),
        ("{1: unknown}", True),
        ("{**{1: unknown}}", True),
        ("[unknown]", True),
    ],
)
def test_recursive_release_fact_includes_mapping_values(
    source: str, may_call: bool
) -> None:
    result = static_expression_result(ast.parse(source, mode="eval").body)
    assert result.release_may_call is may_call


@pytest.mark.parametrize("kind", ["bytearray", "list", "set", "dict"])
def test_unhashable_exact_builtin_keys_raise_without_callbacks(kind) -> None:
    assert AccumulatedKeyEffects().add(StaticExpressionResult(kind=kind)) == RAISES


@pytest.mark.parametrize("kind", ["tuple", "frozenset"])
def test_recursive_key_shapes_retain_collision_callbacks(kind) -> None:
    keys = AccumulatedKeyEffects()
    unknown_contents = StaticExpressionResult(kind=kind)
    expected = EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
    assert keys.add(unknown_contents) == expected
    # A later inert key can still collide with a callbackful retained key.
    assert keys.add(StaticExpressionResult.scalar(1)) == expected
    inert = StaticExpressionResult(
        kind=kind, items=(ExpressionSequenceItem(StaticExpressionResult.scalar(1)),)
    )
    assert AccumulatedKeyEffects().add(inert) == NO_EFFECTS
    nested = StaticExpressionResult(
        kind=kind, items=(ExpressionSequenceItem(unknown_contents),)
    )
    assert AccumulatedKeyEffects().add(nested) == expected


@pytest.mark.parametrize("kind", ["str", "bytes", "bytearray", "range"])
def test_builtin_scalar_iteration_does_not_invent_key_callbacks(kind) -> None:
    assert (
        AccumulatedKeyEffects().extend(StaticExpressionResult(kind=kind)) == NO_EFFECTS
    )


@pytest.mark.parametrize(
    "kind",
    ["tuple", "list", "set", "frozenset", "dict", "str", "bytes", "bytearray", "range"],
)
def test_exact_iterable_protocol_is_separate_from_element_callbacks(kind) -> None:
    node = ast.Name(id="value", ctx=ast.Load())
    assert (
        iterable_unpack_effects(
            node, fact_result=lambda _: StaticExpressionResult(kind=kind)
        )
        == NO_EFFECTS
    )


def test_key_effects_visit_shared_shape_dag_without_expanding_cardinality() -> None:
    result = StaticExpressionResult.scalar(1)
    for _ in range(1100):
        result = StaticExpressionResult(
            kind="tuple",
            items=(ExpressionSequenceItem(result), ExpressionSequenceItem(result)),
        )
    assert AccumulatedKeyEffects().add(result) == NO_EFFECTS


def test_key_effects_distinguish_shared_direct_and_expanded_nodes() -> None:
    bytearray = StaticExpressionResult(kind="bytearray")
    result = StaticExpressionResult(
        kind="tuple",
        items=(
            ExpressionSequenceItem(bytearray),
            ExpressionSequenceItem(bytearray, expanded=True),
        ),
    )
    assert AccumulatedKeyEffects().add(result) == RAISES


@pytest.mark.parametrize(
    "source", ["-1", "+1", "~1", "-(-1)", "+True", "-False", "-1.5", "+2j"]
)
def test_numeric_unary_result_matches_exact_cpython_scalar(source: str) -> None:
    expression = ast.parse(source, mode="eval").body
    expected = eval(compile(ast.Expression(expression), "<unary-oracle>", "eval"))
    result = static_expression_result(expression)
    assert result.value_known
    assert type(result.value) is type(expected)
    assert result.value == expected
    assert result.truth is bool(expected)
    assert not result.release_may_call


@pytest.mark.parametrize("source", ["-1", "+1", "~1"])
def test_numeric_unary_does_not_recover_authoritatively_unknown_operand(
    source: str,
) -> None:
    result = static_expression_result(
        ast.parse(source, mode="eval").body,
        fact_result=lambda _node: UNKNOWN_EXPRESSION_RESULT,
    )
    assert not result.value_known
    assert result.kind == "unknown"


@pytest.mark.parametrize("source", ["~True", "~1.5", "-'text'", "+unknown"])
def test_numeric_unary_does_not_invent_invalid_or_callback_results(source: str) -> None:
    assert not static_expression_result(ast.parse(source, mode="eval").body).value_known


@pytest.mark.parametrize(
    "source,kind,expected",
    [
        ("-value", "int", "int"),
        ("+value", "bool", "int"),
        ("~value", "int", "int"),
        ("-value", "float", "float"),
    ],
)
def test_numeric_unary_retains_exact_kind_without_inventing_a_value(
    source, kind, expected
):
    result = static_expression_result(
        ast.parse(source, mode="eval").body,
        fact_result=lambda _node: StaticExpressionResult(
            kind=kind, release_may_call=False
        ),
    )
    assert result.kind == expected
    assert not result.value_known
    assert result.evaluation_required
    assert not result.release_may_call


def test_unknown_shape_retains_value_identity_through_join_and_publication() -> None:
    from molt.compiler_analysis.python_identity_catalog_generated import (
        PythonIdentity,
        OTHER_IDENTITY,
    )
    from molt.compiler_analysis.static_truth import (
        join_static_expression_results,
        static_subscription_shape,
    )

    importer = StaticExpressionResult(identities=int(PythonIdentity.BUILTINS_IMPORT))
    namespace = StaticExpressionResult(
        kind="dict", identities=int(PythonIdentity.CURRENT_GLOBALS)
    )
    assert importer.kind == "unknown" and importer != UNKNOWN_EXPRESSION_RESULT
    assert len({importer, UNKNOWN_EXPRESSION_RESULT}) == 2
    joined = join_static_expression_results((importer, UNKNOWN_EXPRESSION_RESULT))
    assert joined.identities == int(PythonIdentity.BUILTINS_IMPORT) | OTHER_IDENTITY
    assert join_static_expression_results((joined, importer)) is joined
    outer = StaticExpressionResult(
        kind="tuple",
        length=1,
        items=(ExpressionSequenceItem(namespace),),
        element_result=namespace,
    )
    published = expression_result_for_publication(outer)
    expired = expression_result_without_mutable_contents(published)
    assert published.exposes_module_globals and expired.exposes_module_globals
    assert expired.element_result.identities == int(PythonIdentity.CURRENT_GLOBALS)
    assert static_subscription_shape(
        expired, StaticExpressionResult.scalar(0)
    ).result.identities == int(PythonIdentity.CURRENT_GLOBALS)
    mixed = join_static_expression_results((expired, UNKNOWN_EXPRESSION_RESULT))
    assert mixed.kind == "unknown" and mixed.exposes_module_globals


@pytest.mark.parametrize("construct", ["if", "ifexp", "while", "or", "and"])
def test_subscription_projection_uses_post_index_fact_for_eager_branches(
    construct: str,
) -> None:
    from molt.compiler_analysis.python_lexical_scope import python_eager_nodes
    from molt.compiler_analysis.static_truth import static_test_truthiness

    # CPython mutates the observed element during index evaluation. Both static
    # branches must remain candidates once that callback invalidates its fact.
    prefix = "values = [1]\ndef replace():\n    values[0] = 0\n    return 0\n"
    test = "values[(replace(), 0)[1]]"
    statements = {
        "if": f"if {test}:\n    import math\nelse:\n    import fractions\n",
        "ifexp": f"selected = __import__('math') if {test} else __import__('fractions')\n",
        "while": f"while {test}:\n    import math\n    break\nelse:\n    import fractions\n",
        "or": f"selected = {test} or __import__('fractions')\n",
        "and": f"selected = {test} and __import__('math')\n",
    }
    source = prefix + statements[construct]
    namespace = {}
    exec(compile(source, "<post-index-branch-oracle>", "exec"), namespace)
    assert namespace["values"] == [0]
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    condition = next(
        node
        for node in ast.walk(tree.body[-1])
        if isinstance(node, ast.Subscript) and isinstance(node.value, ast.Name)
    )
    assert (
        static_test_truthiness(condition, fact_result=index.expression_result) is None
    )
    assert static_expression_result(
        condition, fact_result=index.expression_result
    ) is index.expression_result(condition)
    nodes = python_eager_nodes(
        tree, target_python=(3, 12), fact_result=index.expression_result
    )
    candidates = {
        alias.name
        for node in nodes
        if isinstance(node, ast.Import)
        for alias in node.names
    }
    candidates |= {
        node.args[0].value
        for node in nodes
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "__import__"
    }
    assert candidates == (
        {"fractions"}
        if construct == "or"
        else {"math"}
        if construct == "and"
        else {"math", "fractions"}
    )


def test_partial_subscription_provider_expires_mutable_owner_and_honors_unknown() -> (
    None
):
    node = ast.parse("values[(mutate(), 0)[1]]", mode="eval").body
    owner = static_expression_result(ast.parse("[1]", mode="eval").body)
    index_value = StaticExpressionResult.scalar(0, evaluation_required=True)

    def partial(expression):
        if expression is node.value:
            return owner
        if expression is node.slice:
            return index_value
        return None

    result = static_expression_result(node, fact_result=partial)
    assert not result.value_known and result.kind == "unknown"

    def complete(expression):
        return UNKNOWN_EXPRESSION_RESULT if expression is node else partial(expression)

    assert (
        static_expression_result(node, fact_result=complete)
        is UNKNOWN_EXPRESSION_RESULT
    )


@pytest.mark.parametrize(
    "generators", ["for _ in (0, 1)", "for _ in (0, 1) for inner in (0,)"]
)
def test_comprehension_backedges_keep_eager_import_branches(generators: str) -> None:
    from molt.compiler_analysis.python_lexical_scope import python_eager_nodes
    from molt.compiler_analysis.static_truth import static_test_truthiness

    source = (
        "y = 0\n"
        f"if [(y, (y := 1))[0] {generators}][1]:\n"
        "    import math\n    selected = 'later'\n"
        "else:\n    import fractions\n    selected = 'first'\n"
    )
    namespace = {}
    exec(compile(source, "<comprehension-branch-oracle>", "exec"), namespace)
    assert namespace["selected"] == "later"
    tree = ast.parse(source)
    index = analyze_python_source_bindings(source)
    assert (
        static_test_truthiness(tree.body[-1].test, fact_result=index.expression_result)
        is None
    )
    eager = python_eager_nodes(
        tree, target_python=(3, 12), fact_result=index.expression_result
    )
    assert {
        alias.name
        for node in eager
        if isinstance(node, ast.Import)
        for alias in node.names
    } == {"math", "fractions"}


def test_expired_element_candidates_survive_selection_and_unknown_join() -> None:
    from molt.compiler_analysis.python_identity_catalog_generated import (
        OTHER_IDENTITY,
        PythonIdentity,
    )
    from molt.compiler_analysis.static_truth import (
        expression_result_without_value_facts,
        iterable_element_result,
        join_static_expression_results,
        static_subscription_shape,
        static_unpack_results,
    )
    from molt.compiler_analysis.python_builtin_shapes import builtin_method_call_shape
    from molt.compiler_analysis.python_effects_generated import (
        EXECUTES_ARBITRARY_PYTHON,
    )

    importer = StaticExpressionResult(identities=int(PythonIdentity.BUILTINS_IMPORT))
    source = "box = [__import__]\nbox[0] = lambda name: 'replacement'\nloaded = box[0]('math')\n"
    namespace = {}
    exec(compile(source, "<expired-selection-oracle>", "exec"), namespace)
    assert namespace["loaded"] == "replacement"
    owner = StaticExpressionResult(kind="list", element_result=importer)
    expired = expression_result_without_mutable_contents(owner)
    unknown = expression_result_without_value_facts(owner)
    joined = join_static_expression_results((unknown, UNKNOWN_EXPRESSION_RESULT))
    selection = static_subscription_shape(joined, StaticExpressionResult.scalar(0))
    assert selection.invokes_python
    unpacked = static_unpack_results(expired, 1)
    assert unpacked is not None
    unknown_unpack = static_unpack_results(joined, 1)
    assert unknown_unpack is not None
    method = ast.parse("box.pop(index)", mode="eval").body
    assert isinstance(method, ast.Call)
    popped = builtin_method_call_shape(
        expired, "pop", method, (UNKNOWN_EXPRESSION_RESULT,)
    )
    assert popped is not None and popped.invocation_effects & EXECUTES_ARBITRARY_PYTHON
    assert (
        builtin_method_call_shape(joined, "pop", method, (UNKNOWN_EXPRESSION_RESULT,))
        is None
    )
    slice_result = static_subscription_shape(
        joined,
        UNKNOWN_EXPRESSION_RESULT,
        slice_parts=(StaticExpressionResult.scalar(None),) * 3,
    )
    assert slice_result.invokes_python
    for candidate in (
        selection.result,
        iterable_element_result(joined),
        unpacked[0],
        unknown_unpack[0],
        popped.result,
        iterable_element_result(slice_result.result),
        static_subscription_shape(expired, UNKNOWN_EXPRESSION_RESULT).result,
        static_subscription_shape(expired, StaticExpressionResult.scalar(0)).result,
    ):
        assert candidate is not None
        assert candidate.identities & int(PythonIdentity.BUILTINS_IMPORT)
        assert candidate.identities & OTHER_IDENTITY
        assert candidate.kind == "unknown" and not candidate.value_known
        assert (
            candidate.truth is None
            and candidate.length is None
            and candidate.items is None
        )
        assert candidate.release_may_call and not candidate.fresh_container


def test_possible_element_candidates_cover_items_and_unknown_expansion() -> None:
    from molt.compiler_analysis.python_identity_catalog_generated import (
        OTHER_IDENTITY,
        PythonIdentity,
    )
    from molt.compiler_analysis.static_truth import (
        expression_result_without_value_facts,
    )
    from molt.compiler_analysis.python_builtin_shapes import (
        builtin_method_call_shape,
        builtin_call_shape,
    )
    from molt.compiler_analysis.python_effects_generated import (
        EXECUTES_ARBITRARY_PYTHON,
    )

    importer = StaticExpressionResult(identities=int(PythonIdentity.BUILTINS_IMPORT))
    items_only = StaticExpressionResult(
        kind="list", items=(ExpressionSequenceItem(importer),)
    )
    expired = expression_result_without_value_facts(items_only)
    assert expired.element_result is not None
    assert expired.element_result.identities == OTHER_IDENTITY | int(
        PythonIdentity.BUILTINS_IMPORT
    )
    mixed = static_expression_result(
        ast.parse("[load, *unknown]", mode="eval").body,
        fact_result=lambda node: (
            importer if isinstance(node, ast.Name) and node.id == "load" else None
        ),
    )
    assert mixed.items is None and mixed.length is None
    assert mixed.element_result is not None
    assert mixed.element_result.identities == OTHER_IDENTITY | int(
        PythonIdentity.BUILTINS_IMPORT
    )
    from molt.compiler_analysis.python_binding_facts import PythonIterationFact

    iteration = PythonIterationFact.from_result(importer, EXECUTES_ARBITRARY_PYTHON)
    assert iteration.element_result.identities == OTHER_IDENTITY
    method = ast.parse("box.extend(other)", mode="eval").body
    assert isinstance(method, ast.Call)
    extended = builtin_method_call_shape(
        StaticExpressionResult(kind="list", element_result=importer),
        "extend",
        method,
        (StaticExpressionResult(kind="list"),),
    )
    assert extended is not None and extended.receiver_after is not None
    element = extended.receiver_after.element_result
    assert element is not None and element.kind == "unknown"
    assert element.identities == OTHER_IDENTITY | int(PythonIdentity.BUILTINS_IMPORT)
    assert element.release_may_call and not element.fresh_container
    call = ast.parse("list(source)", mode="eval").body
    assert isinstance(call, ast.Call)
    copied = builtin_call_shape("list", call, (expired,))
    assert copied.invocation_effects & EXECUTES_ARBITRARY_PYTHON
    element = copied.result.element_result
    assert element is not None and element.kind == "unknown"
    assert element.identities == OTHER_IDENTITY | int(PythonIdentity.BUILTINS_IMPORT)
    assert element.items is None and element.length is None
    assert element.release_may_call and not element.fresh_container


def test_possible_element_widening_is_idempotent_and_stack_safe() -> None:
    from molt.compiler_analysis.python_identity_catalog_generated import (
        OTHER_IDENTITY,
        PythonIdentity,
    )
    from molt.compiler_analysis.static_truth import (
        expression_result_without_value_facts,
    )

    value = StaticExpressionResult(identities=int(PythonIdentity.BUILTINS_IMPORT))
    for _ in range(1500):
        value = StaticExpressionResult(
            kind="list", element_result=value, fresh_container=True
        )
    expired = expression_result_without_value_facts(value)
    assert expression_result_without_value_facts(expired) is expired
    for _ in range(1500):
        assert expired.kind == "unknown" and expired.length is None
        assert expired.items is None and not expired.fresh_container
        assert expired.release_may_call and expired.element_result is not None
        expired = expired.element_result
    assert expired.identities == OTHER_IDENTITY | int(PythonIdentity.BUILTINS_IMPORT)


def test_widening_reuses_provenance_graph_without_result_allocations(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.compiler_analysis.python_identity_catalog_generated import (
        OTHER_IDENTITY,
        PythonIdentity,
    )
    from molt.compiler_analysis.static_truth import (
        DeferredExecution,
        expression_result_without_value_facts,
    )

    source = (1, 0, 1, 8, "Lambda")
    deferred = DeferredExecution(source, "call")
    leaf = StaticExpressionResult(
        identities=OTHER_IDENTITY | int(PythonIdentity.BUILTINS_IMPORT),
        source_origins=frozenset({source}),
        deferred=frozenset({deferred}),
    )
    equal_leaf = replace(leaf)
    middle = StaticExpressionResult(element_result=leaf)
    outer = StaticExpressionResult(element_result=middle)
    unknown_copy = StaticExpressionResult()
    assert equal_leaf == leaf and equal_leaf is not leaf

    def forbidden_constructor(_self: StaticExpressionResult) -> None:
        pytest.fail("already widened results must not be reconstructed")

    monkeypatch.setattr(StaticExpressionResult, "__post_init__", forbidden_constructor)
    assert expression_result_without_value_facts(outer) is outer
    assert outer.element_result is middle and middle.element_result is leaf
    assert expression_result_without_value_facts(leaf) is leaf
    assert expression_result_without_value_facts(equal_leaf) is equal_leaf
    assert (
        expression_result_without_value_facts(unknown_copy) is UNKNOWN_EXPRESSION_RESULT
    )
    assert join_static_expression_results((leaf, equal_leaf)) is leaf
    assert join_static_expression_results((equal_leaf, leaf)) is equal_leaf


def test_widening_expires_shared_owner_and_preserves_child_provenance() -> None:
    from molt.compiler_analysis.python_identity_catalog_generated import (
        OTHER_IDENTITY,
        PythonIdentity,
    )
    from molt.compiler_analysis.static_truth import (
        DeferredExecution,
        expression_result_without_value_facts,
    )

    direct_source = (1, 0, 1, 8, "Lambda")
    child_source = (2, 0, 2, 8, "Lambda")
    direct = DeferredExecution(direct_source, "call")
    child = DeferredExecution(child_source, "resume")
    leaf = StaticExpressionResult(
        identities=OTHER_IDENTITY | int(PythonIdentity.CURRENT_GLOBALS),
        source_origins=frozenset({child_source}),
        deferred=frozenset({child}),
    )
    owner = StaticExpressionResult(
        kind="tuple",
        truth=True,
        length=2,
        items=(ExpressionSequenceItem(leaf), ExpressionSequenceItem(leaf)),
        evaluation_required=False,
        release_may_call=False,
        fresh_container=True,
        source_origins=frozenset({direct_source}),
        deferred=frozenset({direct}),
        deferred_complete=True,
        attribute_hooks=frozenset({direct}),
        instance_attribute_hooks=frozenset({child}),
        class_instantiation_inert=True,
    )
    expired = expression_result_without_value_facts(owner)
    assert expired is not owner
    assert expired == StaticExpressionResult(
        element_result=leaf,
        source_origins=frozenset({direct_source}),
        exposed_source_origins=frozenset({direct_source, child_source}),
        deferred=frozenset({direct}),
        exposed_deferred=frozenset({direct, child}),
        exposes_module_globals=True,
    )
    assert expired.element_result is leaf
    assert expression_result_without_value_facts(expired) is expired
    assert owner.items is not None and owner.items[0].result is owner.items[1].result


@pytest.mark.parametrize(
    "changed",
    [
        {"truth": False},
        {"value": 0},
        {"value_known": True},
        {"kind": "int"},
        {"scalar_kinds": frozenset({"int", "float"})},
        {"evaluation_required": False},
        {"items": ()},
        {"release_may_call": False},
        {"fresh_container": True},
        {"length": 0},
        {"element_result": UNKNOWN_EXPRESSION_RESULT},
        {"deferred_complete": True},
        {"attribute_hooks": frozenset()},
        {"instance_attribute_hooks": frozenset()},
        {"class_instantiation_inert": True},
        {"_publication_release_stable": True},
    ],
)
def test_widening_reuse_cannot_preserve_exact_or_dispatch_facts(
    changed: dict[str, object],
) -> None:
    from molt.compiler_analysis.python_identity_catalog_generated import (
        OTHER_IDENTITY,
        PythonIdentity,
    )
    from molt.compiler_analysis.static_truth import (
        expression_result_without_value_facts,
    )

    expected = StaticExpressionResult(
        identities=OTHER_IDENTITY | int(PythonIdentity.BUILTINS_IMPORT)
    )
    current = replace(expected, **changed)
    expired = expression_result_without_value_facts(current)
    assert expired is not current
    assert expired == expected
    assert expression_result_without_value_facts(expired) is expired
    owner = StaticExpressionResult(element_result=current)
    expired_owner = expression_result_without_value_facts(owner)
    assert expired_owner is not owner
    assert expired_owner.element_result is not current
    assert expired_owner.element_result == expected
    assert expired_owner.element_result.value is None


@pytest.mark.parametrize(
    "left_values,operator,right_values",
    [
        ((2, 5), "+", (3, 7)),
        ((2, 5), "-", (3, 7)),
        ((2, 5), "*", (3, 7)),
        ((2, 5), "/", (3, 7)),
        ((2, 5), "//", (3, 7)),
        ((2, 5), "%", (3, 7)),
        ((True, False), "&", (True, False)),
        ((True, False), "|", (True, False)),
        ((True, False), "^", (2, 3)),
        ((2, 5), "<<", (1, 2)),
        ((2, 5), ">>", (1, 2)),
        ((1, 2.5), "+", (2, 3.5)),
        ((1, 2.5), "-", (2j, 3j)),
        ((1, 2.5), "*", (2j, 3j)),
        ((1, 2.5), "/", (2j, 3j)),
        ((1, 2.5), "//", (2, 3.5)),
        ((1, 2.5), "%", (2, 3.5)),
        ((-2, 2), "**", (-1, 2)),
        ((-2.0, 2.0), "**", (0.5, 2.0)),
        ((-2, 2), "**", (0.5, 2.0)),
        ((1j, 2j), "**", (1, 2)),
        (("a", "bc"), "+", ("d", "ef")),
        ((b"a", b"bc"), "+", (b"d", b"ef")),
        (("a", "bc"), "*", (1, 2)),
        ((1, 2), "*", (b"a", b"bc")),
        (("%s", "[%s]"), "%", (1, 2)),
        ((b"%d", b"[%d]"), "%", (1, 2)),
    ],
)
def test_scalar_operator_kind_domain_matches_cpython(
    left_values, operator, right_values
) -> None:
    # Independent CPython operations supply all normal-result kinds. Joining
    # different values removes constants before testing the shared transfer.
    source = "left " + operator + " right"
    node = ast.parse(source, mode="eval").body
    assert isinstance(node, ast.BinOp)
    operands = {
        "left": join_static_expression_results(
            tuple(StaticExpressionResult.scalar(value) for value in left_values)
        ),
        "right": join_static_expression_results(
            tuple(StaticExpressionResult.scalar(value) for value in right_values)
        ),
    }
    assert all(not result.value_known for result in operands.values())
    expected = {
        type(eval(source, {}, {"left": left, "right": right})).__name__
        for left in left_values
        for right in right_values
    }
    result = static_expression_result(
        node,
        fact_result=lambda child: (
            operands.get(child.id) if isinstance(child, ast.Name) else None
        ),
    )
    assert result.scalar_kinds == expected
    assert result.kind == (next(iter(expected)) if len(expected) == 1 else "unknown")
    assert result.evaluation_required and not result.release_may_call
    for inplace in (False, True):
        effects = binary_operation_effects(
            operands["left"], operands["right"], inplace=inplace
        )
        assert effects & RAISES
        assert not effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    assert (
        not binary_operation_effects(result, StaticExpressionResult.scalar(1))
        & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    )


@pytest.mark.parametrize("value", ["x" * 4096, b"x" * 4096, 1 << 4096])
def test_binary_addition_fold_budget_keeps_exact_representation(value) -> None:
    left = StaticExpressionResult.scalar(value)
    result = static_binary_result(left, ast.Add(), left)
    assert not result.value_known
    assert result.kind == type(value).__name__
    assert result.evaluation_required and not result.release_may_call
    # Later operations keep the type proof after folding stops.
    again = static_binary_result(result, ast.Add(), result)
    assert again.kind == result.kind and not again.value_known
    assert (
        not binary_operation_effects(result, result, inplace=True)
        & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    )


def test_scalar_domain_unknown_alternative_and_replacement_expire_proof() -> None:
    from molt.compiler_analysis.static_truth import (
        expression_result_without_value_facts,
    )

    numeric = join_static_expression_results(
        (StaticExpressionResult.scalar(1), StaticExpressionResult.scalar(1.5))
    )
    assert numeric.scalar_kinds == {"int", "float"}
    unknown = join_static_expression_results((numeric, UNKNOWN_EXPRESSION_RESULT))
    expired = expression_result_without_value_facts(numeric)
    for result in (unknown, expired):
        assert not result.is_exact_scalar
        assert binary_operation_effects(result, numeric) & EXECUTES_ARBITRARY_PYTHON

    class IntegerSubclass(int):
        pass

    assert not StaticExpressionResult.scalar(IntegerSubclass()).is_exact_scalar


def test_unary_transfer_preserves_mixed_scalar_domain_and_bool_warning() -> None:
    operand = join_static_expression_results(
        (StaticExpressionResult.scalar(1), StaticExpressionResult.scalar(1.5))
    )
    node = ast.parse("-value", mode="eval").body
    result = static_expression_result(node, fact_result=lambda _: operand)
    assert result.scalar_kinds == {"int", "float"}
    assert (
        not unary_operation_effects(operand, ast.USub())
        & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    )
    assert (
        unary_operation_effects(StaticExpressionResult.scalar(True), ast.Invert())
        & EXECUTES_ARBITRARY_PYTHON
    )
    # A raising exact-scalar alternative does not erase the valid normal type.
    mixed = join_static_expression_results(
        (StaticExpressionResult.scalar(1), StaticExpressionResult.scalar("text"))
    )
    result = static_expression_result(node, fact_result=lambda _: mixed)
    assert result.scalar_kinds == {"int"}


@pytest.mark.parametrize(
    "expression,error",
    [
        ("1 / 0", ZeroDivisionError),
        ("1 << -1", ValueError),
        ("1 + 'x'", TypeError),
        ("1 @ 2", TypeError),
        ("1e308 ** 2", OverflowError),
    ],
)
def test_scalar_operator_effects_preserve_runtime_errors(expression, error) -> None:
    with pytest.raises(error):
        eval(expression)
    node = ast.parse(expression, mode="eval").body
    result = static_expression_result(node)
    effects = expression_effect_mask(node)
    assert result.evaluation_required
    assert effects & RAISES
    assert not effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS


def test_definition_phase_projection_matches_actual_host_execution():
    import sys
    from molt.compiler_analysis.python_lexical_scope import python_eager_nodes
    from molt.compiler_analysis.python_lexical_scope import python_execution_regions

    source = """
events = []
def mark(label, value=0):
    events.append(label)
    return value
def decorate(value):
    return lambda function: function
try:
    @decorate(mark('decorator'))
    def function[T: mark('bound')](arg: mark('annotation') = mark('default')):
        mark('function_body')
    class C(mark('base', object)):
        mark('class_body')
        def method(self, arg=mark('method_default')):
            mark('method_body')
    generator = (mark('generator_body') for item in mark('generator_iter', (1,)))
    eager = [mark('list_body') for item in mark('list_iter', (1,))]
    type Alias[T: mark('alias_bound')] = mark('alias_body')
except Exception:
    raise
"""
    namespace = {}
    exec(
        compile(source, "<definition-phase-oracle>", "exec", dont_inherit=True),
        namespace,
    )
    observed = namespace["events"]
    expected = [
        "decorator",
        "default",
        "base",
        "class_body",
        "method_default",
        "generator_iter",
        "list_iter",
        "list_body",
    ]
    if sys.version_info[:2] < (3, 14):
        expected.insert(2, "annotation")
    assert observed == expected
    tree = ast.parse(source)

    def marker(node):
        return (
            node.args[0].value
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Name)
            and node.func.id == "mark"
            else None
        )

    assert [
        name
        for node in python_eager_nodes(tree, target_python=sys.version_info[:2])
        if (name := marker(node)) is not None
    ] == observed
    regions = python_execution_regions(
        tree, target_python=sys.version_info[:2], include_deferred=True
    )
    assert [
        name
        for region in regions
        if region.owner is None and (name := marker(region.node)) is not None
    ] == observed
    owners = {
        name: region.owner
        for region in regions
        if (name := marker(region.node)) is not None
    }
    assert all(
        owner is not None for name, owner in owners.items() if name not in observed
    )
    assert {
        name: (owner[0], owner[-1])
        for name, owner in owners.items()
        if name in {"function_body", "method_body", "generator_body"}
        and owner is not None
    } == {
        "function_body": (10, "FunctionDef"),
        "method_body": (14, "FunctionDef"),
        "generator_body": (16, "GeneratorExp"),
    }


@pytest.mark.parametrize("asynchronous", [False, True])
@pytest.mark.parametrize("nested_target", [False, True])
def test_context_execution_phases_match_cpython_order_and_owner(
    asynchronous, nested_target
):
    import asyncio
    import inspect
    import sys
    from types import SimpleNamespace
    from molt.compiler_analysis.python_lexical_scope import python_execution_regions
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    events = []
    prefix = "async_" if asynchronous else ""

    class EnteredValue:
        def __init__(self, label):
            self.label = label

        def __iter__(self):
            events.append(("manager", self.label, prefix + "context_unpack"))
            # Nested unpack of an exact tuple has no user callback. The source
            # phase denotes root iteration, not a complete nested heap trace.
            return iter((1, (2, 3)) if nested_target else (1, 2))

    class Manager:
        def __init__(self, label):
            self.label = label

        def __enter__(self):
            events.append(("manager", self.label, "context_enter"))
            return EnteredValue(self.label)

        def __exit__(self, *_error):
            events.append(("manager", self.label, "context_exit"))

        async def __aenter__(self):
            events.append(("manager", self.label, "async_context_enter"))
            return EnteredValue(self.label)

        async def __aexit__(self, *_error):
            events.append(("manager", self.label, "async_context_exit"))

    def acquire(label):
        events.append(("acquire", label, "evaluate"))
        return Manager(label)

    def target(label):
        events.append(("target", label, "evaluate"))
        return SimpleNamespace()

    def body():
        events.append(("body", "", "evaluate"))

    outer_rest = "(target('outer_nested').value, spare)" if nested_target else "spare"
    inner_rest = "(target('inner_nested').value, spare)" if nested_target else "spare"
    statement = (
        f"{'async ' if asynchronous else ''}with acquire('outer') as "
        f"(target('outer').value, {outer_rest}), acquire('inner') as "
        f"[target('inner').value, {inner_rest}]:\n    body()\n"
    )
    source = (
        "async def run():\n"
        + "".join("    " + line for line in statement.splitlines(True))
        if asynchronous
        else statement
    )
    namespace: dict[str, object] = {"acquire": acquire, "target": target, "body": body}
    exec(compile(source, "<context-phase-oracle>", "exec"), namespace)
    if asynchronous:
        run = namespace["run"]
        assert inspect.iscoroutinefunction(run)
        asyncio.run(run())

    tree = ast.parse(source)
    projected = []
    owners = set()
    for region in python_execution_regions(
        tree, target_python=sys.version_info[:2], include_deferred=asynchronous
    ):
        node = region.node
        if not (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Name)
            and node.func.id in {"acquire", "target", "body"}
        ):
            continue
        if node.args:
            label_node = node.args[0]
            assert isinstance(label_node, ast.Constant)
            label = label_node.value
        else:
            label = ""
        projected.append(
            (
                node.func.id if region.phase == "evaluate" else "manager",
                label,
                region.phase,
            )
        )
        owners.add(region.owner)
    assert projected == events
    assert owners == {python_node_source_key(tree.body[0]) if asynchronous else None}
