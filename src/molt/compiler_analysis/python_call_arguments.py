"""CPython call-argument order, expansion and call form (Python >= 3.12).

The sole starred positional operand stays unexpanded until keyword assembly is
complete. Otherwise positional expansions happen immediately. Consecutive named
keyword values are evaluated together before their merge can report duplicates.
Both binding effects and lowering consume this schedule.

The call form is the instruction CPython's compiler emits for the call site:
CALL keeps the operands on the value stack; CALL_FUNCTION_EX passes a
positional tuple and a keyword mapping. It decides who owns the arguments while
the callee binds them, and runtime values cannot recover it, so lowering
records it on the call's argument builder.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import Literal

from molt.compiler_analysis.native_support_slice import import_bound_names


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


CallForm = Literal["stack", "expanded"]

# CPython's compiler keeps a call's operands on the value stack only while
# `positional + 2 * keywords <= STACK_USE_GUIDELINE`. An attribute callee whose
# base is not a module-scope import first tries the method-call form, which
# requires `positional + keywords + (keywords != 0) < STACK_USE_GUIDELINE`
# (`maybe_optimize_method_call` and the call helper in compile.c/codegen.c).
STACK_USE_GUIDELINE = 30


def call_form(node: ast.Call, *, module_imports: frozenset[str]) -> CallForm:
    """CALL (``"stack"``) or CALL_FUNCTION_EX (``"expanded"``) for this call."""
    if any(isinstance(argument, ast.Starred) for argument in node.args) or any(
        keyword.arg is None for keyword in node.keywords
    ):
        return "expanded"
    positional = len(node.args)
    keywords = len(node.keywords)
    if (
        isinstance(node.func, ast.Attribute)
        and not (
            isinstance(node.func.value, ast.Name)
            and node.func.value.id in module_imports
        )
        and positional + keywords + (keywords != 0) < STACK_USE_GUIDELINE
    ):
        return "stack"
    return "expanded" if positional + 2 * keywords > STACK_USE_GUIDELINE else "stack"


def collect_module_import_names(module: ast.Module) -> frozenset[str]:
    """Module-scope names an import binds: CPython's DEF_IMPORT symbols.

    Statements nested in compound statements share the module scope; function
    and class bodies do not.
    """
    names: set[str] = set()
    pending: list[ast.stmt] = list(module.body)
    while pending:
        statement = pending.pop()
        names |= import_bound_names(statement)
        if isinstance(
            statement, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)
        ):
            continue
        for field in ("body", "orelse", "finalbody"):
            pending.extend(getattr(statement, field, ()))
        for handler in getattr(statement, "handlers", ()):
            pending.extend(handler.body)
        for case in getattr(statement, "cases", ()):
            pending.extend(case.body)
    return frozenset(names)
