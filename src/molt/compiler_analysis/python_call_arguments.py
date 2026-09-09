"""CPython call-argument evaluation and expansion order (Python >= 3.12).

The sole starred positional operand stays unexpanded until keyword assembly is
complete. Otherwise positional expansions happen immediately. Consecutive named
keyword values are evaluated together before their merge can report duplicates.
Both binding effects and lowering consume this schedule.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import Literal


@dataclass(frozen=True)
class CallArgumentStep:
    action: Literal["evaluate", "pos", "star", "kw", "kwstar"]
    index: int
    expression: ast.expr
    name: str | None = None
    materialization: Literal["tuple"] | None = None


def call_argument_schedule(
    node: ast.Call | ast.ClassDef,
) -> tuple[CallArgumentStep, ...]:
    arguments = node.bases if isinstance(node, ast.ClassDef) else node.args
    steps: list[CallArgumentStep] = []
    deferred: CallArgumentStep | None = None
    for index, argument in enumerate(arguments):
        starred = isinstance(argument, ast.Starred)
        expression = argument.value if starred else argument
        steps.append(CallArgumentStep("evaluate", index, expression))
        # __build_class__ always has the body function and class name before
        # source bases, so even one starred base expands before keywords.
        if starred and len(arguments) == 1 and isinstance(node, ast.Call):
            deferred = CallArgumentStep(
                "star", index, expression, materialization="tuple"
            )
        else:
            steps.append(
                CallArgumentStep("star" if starred else "pos", index, expression)
            )
    group: list[CallArgumentStep] = []
    for index, keyword in enumerate(node.keywords, len(arguments)):
        if keyword.arg is None:
            steps.extend(group)
            group.clear()
            steps.append(CallArgumentStep("evaluate", index, keyword.value))
            steps.append(CallArgumentStep("kwstar", index, keyword.value))
        else:
            steps.append(CallArgumentStep("evaluate", index, keyword.value))
            group.append(CallArgumentStep("kw", index, keyword.value, keyword.arg))
    steps.extend(group)
    if deferred is not None:
        steps.append(deferred)
    return tuple(steps)
