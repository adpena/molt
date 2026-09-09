from __future__ import annotations

import ast

import pytest

from molt.compiler_analysis.python_inlining import (
    inline_expression_is_frame_independent,
)
from molt.frontend._types import MoltOp, MoltValue
from molt.frontend.visitors.call_method_dispatch import CallMethodDispatchMixin
from molt.frontend.visitors.class_method_compilation import ClassMethodCompilationMixin


def expression(source: str) -> ast.expr:
    return ast.parse(source, mode="eval").body


@pytest.mark.parametrize("source", ["x", "42", "None", "(x, [1, y])"])
def test_parameter_only_values_preserve_frame_elision(source: str) -> None:
    assert inline_expression_is_frame_independent(expression(source), {"x", "y"})


@pytest.mark.parametrize(
    "source",
    [
        "super().value()",
        "super_alias()",
        "builtins.super()",
        "next(iter(super, None))",
        "self.value",
        "self[0]",
        "x + y",
        "not x",
        "x == y",
        "x and y",
        "x if y else self",
        "(x := y)",
        "lambda: x",
        "[x for x in y]",
        "unknown_global",
    ],
)
def test_callback_or_scope_observation_requires_real_frame(source: str) -> None:
    assert not inline_expression_is_frame_independent(
        expression(source), {"self", "x", "y"}
    )


class InlineHarness(CallMethodDispatchMixin, ClassMethodCompilationMixin):
    def __init__(self) -> None:
        self.events: list[str] = []
        self.locals = {"x": MoltValue("caller_x", type_hint="str")}
        self.free_vars = {"x": 0}
        self.classes = {"C": {"fields": {"first": 0, "second": 8}, "methods": {}}}

    def visit(self, node: ast.Constant) -> MoltValue:
        assert isinstance(node, ast.Constant)
        self.events.append(f"constant:{node.value}")
        return MoltValue(f"constant_{node.value}", type_hint="int")

    def next_var(self) -> str:
        return f"v{len(self.events)}"

    def emit(self, op: MoltOp) -> None:
        self.events.append(op.kind)

    def _class_mro_names(self, name: str) -> list[str]:
        return [name, "object"]

    def _class_attr_is_data_descriptor(self, name: str, field: str) -> bool:
        return False

    def _emit_guarded_setattr(self, receiver, name, value, owner, **kwargs) -> None:
        assert kwargs == {"use_init": True, "assume_exact": True}
        self.events.append(f"store:{name}")


def test_inline_parameter_never_resolves_through_callers_local_or_cell() -> None:
    harness = InlineHarness()
    argument = MoltValue("argument", type_hint="int")
    before = harness.locals
    result = harness._try_inline_method_call(
        {"inline_return": expression("x"), "inline_params": ["self", "x"]},
        MoltValue("receiver", type_hint="C"),
        [argument],
    )
    assert result is argument
    assert harness.locals is before
    assert harness.free_vars == {"x": 0}
    assert harness.events == []


def test_rejected_inline_emits_no_prefix_before_fallback() -> None:
    harness = InlineHarness()
    result = harness._try_inline_method_call(
        {
            "inline_return": expression("side_effect() + super().value()"),
            "inline_params": ["self"],
        },
        MoltValue("receiver", type_hint="C"),
        [],
    )
    assert result is None
    assert harness.events == []


def test_constructor_preflights_all_fields_before_emission() -> None:
    harness = InlineHarness()
    assert not harness._try_inline_init_assigns(
        [("first", expression("1")), ("second", expression("callback()"))],
        ["self"],
        MoltValue("receiver", type_hint="C"),
        [],
    )
    assert harness.events == []


def test_constructor_evaluates_then_stores_each_field_in_source_order() -> None:
    harness = InlineHarness()
    assert harness._try_inline_init_assigns(
        [("first", expression("1")), ("second", expression("2"))],
        ["self"],
        MoltValue("receiver", type_hint="C"),
        [],
    )
    assert harness.events == ["constant:1", "store:first", "constant:2", "store:second"]


def test_constructor_never_bypasses_custom_setattr_or_reuses_initialization_store() -> (
    None
):
    for custom_setattr in (False, True):
        harness = InlineHarness()
        if custom_setattr:
            harness.classes["C"]["class_attrs"] = {"__setattr__": object()}
            assignments = [("first", expression("1"))]
        else:
            assignments = [("first", expression("1")), ("first", expression("2"))]
        assert not harness._try_inline_init_assigns(
            assignments, ["self"], MoltValue("receiver", type_hint="C"), []
        )
        assert harness.events == []


def test_extractors_share_legality_and_honor_constructor_return() -> None:
    harness = InlineHarness()
    method = ast.parse("def method(self):\n    return super_alias()\n").body[0]
    assert harness._extract_inline_return(method, ["self"]) is None
    constructor = ast.parse(
        "def __init__(self):\n"
        "    self.first = 1\n"
        "    return None\n"
        "    self.second = callback()\n"
    ).body[0]
    assignments = harness._extract_inline_init_assigns(constructor, ["self"])
    assert assignments is not None
    assert [name for name, _ in assignments] == ["first"]


def test_static_super_folding_has_no_frontend_entry_point() -> None:
    assert not hasattr(CallMethodDispatchMixin, "_try_emit_super_static_call")
    assert not hasattr(CallMethodDispatchMixin, "_fold_bare_super_static")
