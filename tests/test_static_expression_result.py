from __future__ import annotations

import ast
import math

import pytest

from molt.compiler_analysis.static_truth import (
    ExpressionSequenceItem,
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    _same_scalar_value,
    static_expression_result,
)
from molt.compiler_analysis.python_binding_flow import analyze_python_source_bindings
from molt.compiler_analysis.python_effects import (
    AccumulatedKeyEffects,
    expression_may_execute_python,
    iterable_unpack_effects,
)
from molt.compiler_analysis.python_effects_generated import (
    EXECUTES_ARBITRARY_PYTHON,
    INVOKES_COMPARISON_CALLBACK,
    NO_EFFECTS,
    RAISES,
)


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


@pytest.mark.parametrize("arguments", ["*items", "**mapping"])
def test_pure_call_identity_does_not_erase_expansion_callbacks(arguments: str) -> None:
    expression = ast.parse(f"pure({arguments})", mode="eval").body
    assert expression_may_execute_python(expression, proven_pure_calls={"pure"})


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
