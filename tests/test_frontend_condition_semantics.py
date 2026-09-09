"""Execute the frontend's structured expression IR against Python callbacks.

This intentionally stops before the midend/backends: it makes truth-observation
regressions a seconds-scale test, not a substitute for native/WASM differential
execution of the sibling corpus.
"""

from __future__ import annotations

import ast
import operator
import sys
from typing import Any

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator


def _execute_expression_ops(ops: list[MoltOp], inputs: dict[str, Any]) -> Any:
    values = dict(inputs)
    variables: dict[str, Any] = {}
    frames: list[tuple[bool, bool]] = []
    active = True
    completed_branch = False
    comparisons = {
        "EQ": operator.eq,
        "NE": operator.ne,
        "LT": operator.lt,
        "LE": operator.le,
        "GT": operator.gt,
        "GE": operator.ge,
        "IS": operator.is_,
        "CONTAINS": operator.contains,
    }

    def read(value: MoltValue) -> Any:
        return values[value.name]

    for op in ops:
        if op.kind == "IF":
            branch = bool(read(op.args[0])) if active else False
            frames.append((active, branch))
            active = active and branch
        elif op.kind == "ELSE":
            parent, branch = frames[-1]
            active = parent and not branch
        elif op.kind == "END_IF":
            active, completed_branch = frames.pop()
        elif not active:
            continue
        elif op.kind in {"LINE", "CHECK_EXCEPTION", "TRACE_EXIT"}:
            continue
        elif op.kind == "STORE_VAR":
            assert op.metadata is not None
            variables[op.metadata["var"]] = read(op.args[0])
        elif op.kind == "LOAD_VAR":
            assert op.metadata is not None
            values[op.result.name] = variables[op.metadata["var"]]
        elif op.kind in {"COPY", "IDENTITY_ALIAS"}:
            values[op.result.name] = read(op.args[0])
        elif op.kind == "PHI":
            values[op.result.name] = read(op.args[0 if completed_branch else 1])
        elif op.kind == "CONST_NONE":
            values[op.result.name] = None
        elif op.kind in {"CONST", "CONST_BOOL"}:
            values[op.result.name] = op.args[0]
        elif op.kind == "BOOL":
            values[op.result.name] = bool(read(op.args[0]))
        elif op.kind == "NOT":
            values[op.result.name] = not read(op.args[0])
        elif op.kind in comparisons:
            values[op.result.name] = comparisons[op.kind](
                read(op.args[0]), read(op.args[1])
            )
        elif op.kind == "ret":
            return read(op.args[0])
        else:
            pytest.fail(f"unmodeled expression operation: {op.kind}")
    pytest.fail("expression IR did not return")


class _Probe:
    def __init__(self, name: str, events: list[str], first: bool, raises: bool):
        self.name = name
        self.events = events
        self.next_truth = first
        self.raises = raises

    def __bool__(self) -> bool:
        self.events.append(f"{self.name}:{self.next_truth}")
        if self.raises:
            raise ValueError(self.name)
        result = self.next_truth
        self.next_truth = not result
        return result

    def _compare(self, other: _Probe, symbol: str) -> _Probe:
        self.events.append(f"{self.name}{symbol}{other.name}")
        return self

    def __eq__(self, other: _Probe) -> _Probe:
        return self._compare(other, "==")

    def __ne__(self, other: _Probe) -> _Probe:
        return self._compare(other, "!=")

    def __lt__(self, other: _Probe) -> _Probe:
        return self._compare(other, "<")

    def __le__(self, other: _Probe) -> _Probe:
        return self._compare(other, "<=")

    def __gt__(self, other: _Probe) -> _Probe:
        return self._compare(other, ">")

    def __ge__(self, other: _Probe) -> _Probe:
        return self._compare(other, ">=")


def _observe(run: Any, *, first: bool, raises: bool) -> tuple[Any, list[str]]:
    events: list[str] = []
    inputs = {name: _Probe(name, events, first, raises) for name in "abcd"}
    try:
        result = run(inputs)
        outcome = ("value", result.name if isinstance(result, _Probe) else result)
    except ValueError as error:
        outcome = ("error", str(error))
    return outcome, events


def _lower(source: str, phi: bool, version: tuple[int, int]) -> list[MoltOp]:
    generator = SimpleTIRGenerator(enable_phi=phi, target_python=version)
    generator.visit(ast.parse(source))
    return next(
        function["ops"]
        for name, function in generator.funcs_map.items()
        if name.endswith("__probe")
    )


@pytest.mark.parametrize("phi", [False, True])
@pytest.mark.parametrize("condition", [False, True])
@pytest.mark.parametrize("first,raises", [(False, False), (True, False), (True, True)])
@pytest.mark.parametrize(
    "expression",
    [
        "a and b and c",
        "a or b or c",
        "(a and b) or c",
        "(a or b) and c",
        "a < b < c",
        "(a < b < c) and d",
        "(a and b if True else c) or d",
        "not (a and b)",
        "(not (a and b)) or d",
        "a if b and c else d",
    ]
    + [
        expression
        for symbol in ("==", "!=", "<", "<=", ">", ">=")
        for expression in (
            f"a {symbol} b",
            f"(a {symbol} b) and d",
            f"(a {symbol} b {symbol} c) or d",
        )
    ],
)
def test_expression_callback_trace_matches_running_cpython(
    phi: bool,
    condition: bool,
    first: bool,
    raises: bool,
    expression: str,
) -> None:
    body = (
        f"if {expression}:\n  return 1\n return 0"
        if condition
        else f"return {expression}"
    )
    source = f"def probe(a,b,c,d):\n {body}\n"
    namespace: dict[str, Any] = {}
    exec(compile(source, "<truth-oracle>", "exec"), namespace)
    ops = _lower(source, phi, sys.version_info[:2])
    expected = _observe(
        lambda inputs: namespace["probe"](**inputs), first=first, raises=raises
    )
    actual = _observe(
        lambda inputs: _execute_expression_ops(ops, inputs), first=first, raises=raises
    )
    assert actual == expected


@pytest.mark.parametrize("phi", [False, True])
@pytest.mark.parametrize("version", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "expression,first,old_value",
    [
        ("(a and b) or c", False, "a"),
        ("(a or b) and c", True, "a"),
    ],
)
def test_nested_value_boolop_version_boundary(
    phi: bool,
    version: tuple[int, int],
    expression: str,
    first: bool,
    old_value: str,
) -> None:
    ops = _lower(f"def probe(a,b,c,d):\n return {expression}", phi, version)
    actual = _observe(
        lambda inputs: _execute_expression_ops(ops, inputs), first=first, raises=False
    )
    expected_events = [f"a:{first}"]
    if version < (3, 14):
        expected_events.append(f"a:{not first}")
    assert actual == (
        ("value", old_value if version < (3, 14) else "c"),
        expected_events,
    )
