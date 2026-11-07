"""Argument classes for parameters that a body observes only by literal tests.

Discovery analyzes a function body once per distinct argument tuple
(``PythonSourceDemands._activate``). A module's PEP 562 ``__getattr__`` is
activated once per attribute name that other modules read; asyncio's hook
compares ``name`` with four literals, yet discovery analyzed it 240 times.

A positional parameter is *collapsible* when the binding flow can observe its
value only through:

1. a single-operator test of the parameter against string literals
   (``p == "a"``, ``"a" != p``, ``p in ("a", "b")``, ``p not in [...]``);
2. any load inside the true branch of an ``if`` whose test is a positive
   literal test (``==`` or ``in``), because the parameter equals one of the
   literals there;
3. any load after a *terminal chain* in the same block: an ``if``/``elif``
   chain of positive literal tests whose final ``else`` ends in ``raise`` or
   ``return``, because control reaches the next statement only through a
   literal branch;
4. a direct ``{p}`` field of an f-string that is a positional argument of a
   ``raise`` whose call reaches no source callable and whose callee identities
   are only builtin exception classes or unknown objects: the analysis records
   no argument-dependent fact for such a call.

The parameter must never be a binding target of its scope
(``python_scope_declarations``), no nested scope may name it,
and no call in the body may reach a builtin that reads the caller's frame
(``BUILTIN_CALLER_FRAME_IDENTITIES``: ``locals``, ``vars``, ``eval``,
``exec``, zero-argument ``super``). Every call needs an analysis fact.

Identities are typed: the binding analysis supplies each call's callee mask
and source reach (``CallEvidence``), and the identity catalog declares the
builtin roles (``python_identity_catalog.toml``, ``role``).

For a collapsible parameter, every argument outside its literal set takes the
same branches and reaches no other use, so the analysis is identical for all
of them; they share the representative ``UNMATCHED_ARGUMENT``. Anything the
classifier cannot prove keeps exact activations (``None``).

Prior art: predicate abstraction (Graf and Saidi, CAV 1997) partitions a
value domain by the predicates a program tests; Salsa-style early cutoff
reuses a result when the inputs it observed are unchanged.
"""

from __future__ import annotations

import ast
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Final

from molt.compiler_analysis.python_identity_catalog_generated import (
    BUILTIN_CALLER_FRAME_IDENTITIES,
    BUILTIN_EXCEPTION_CLASS_IDENTITIES,
    UNKNOWN_IDENTITY,
    IdentityMask,
)
from molt.compiler_analysis.python_lexical_scope import python_scope_declarations

# Not a Python identifier, so no attribute name, keyword or module name equals
# it. A body that compares with this exact string is not collapsible.
UNMATCHED_ARGUMENT: Final = "\x00molt:unmatched-argument"

type ParameterLiteralClass = frozenset[str] | None

# A raised call may take the parameter in its message only with these callees.
_MESSAGE_CALLEES: Final[IdentityMask] = (
    UNKNOWN_IDENTITY | BUILTIN_EXCEPTION_CLASS_IDENTITIES
)
_NESTED_SCOPES: Final = (
    ast.FunctionDef,
    ast.AsyncFunctionDef,
    ast.ClassDef,
    ast.Lambda,
    ast.GeneratorExp,
    ast.ListComp,
    ast.SetComp,
    ast.DictComp,
)


@dataclass(frozen=True, slots=True)
class CallEvidence:
    """One analyzed call: its callee identities and whether a source callable
    (a function or class defined in source) may receive its arguments."""

    callees: IdentityMask
    reaches_source: bool


type CallEvidenceLookup = Callable[[ast.Call], CallEvidence | None]


def parameter_literal_classes(
    node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
    *,
    call_evidence: CallEvidenceLookup,
) -> tuple[ParameterLiteralClass, ...]:
    """One class per positional parameter, aligned with posonly + args."""
    parameters = (*node.args.posonlyargs, *node.args.args)
    if not parameters:
        return ()
    body: ast.expr | tuple[ast.stmt, ...] = (
        node.body if isinstance(node, ast.Lambda) else tuple(node.body)
    )
    statements = body if isinstance(body, tuple) else (ast.Expr(body),)
    if _may_read_caller_frame(statements, call_evidence):
        return (None,) * len(parameters)
    # The scope's own declarations (the lexical authority) decide rebinding;
    # eager annotations make the collected set a superset.
    declared = python_scope_declarations(statements, eager_annotations=True)
    rebound = declared.bound | declared.globals | declared.nonlocals
    return tuple(
        None
        if parameter.arg in rebound
        else _Classifier(parameter.arg, call_evidence).classify(body)
        for parameter in parameters
    )


def representative_arguments(
    arguments: tuple[str, ...], classes: Sequence[ParameterLiteralClass]
) -> tuple[str, ...]:
    """Map each collapsible argument outside its literal set to the sentinel."""
    if len(arguments) > len(classes):
        return arguments
    return tuple(
        UNMATCHED_ARGUMENT
        if literal_class is not None and argument not in literal_class
        else argument
        for argument, literal_class in zip(arguments, classes, strict=False)
    )


def _may_read_caller_frame(
    body: Sequence[ast.AST], call_evidence: CallEvidenceLookup
) -> bool:
    for root in body:
        for child in ast.walk(root):
            if isinstance(child, ast.Call):
                evidence = call_evidence(child)
                if (
                    evidence is None
                    or evidence.callees & BUILTIN_CALLER_FRAME_IDENTITIES
                ):
                    return True
    return False


def _string_literals(node: ast.expr) -> frozenset[str] | None:
    if not isinstance(node, (ast.Tuple, ast.List, ast.Set)):
        return None
    values: list[str] = []
    for element in node.elts:
        if not isinstance(element, ast.Constant) or type(element.value) is not str:
            return None
        values.append(element.value)
    return frozenset(values)


class _Classifier:
    def __init__(self, name: str, call_evidence: CallEvidenceLookup):
        self.name = name
        self.call_evidence = call_evidence
        self.literals: set[str] = set()
        self.collapsible = True

    def classify(self, body: ast.expr | tuple[ast.stmt, ...]) -> ParameterLiteralClass:
        if isinstance(body, tuple):
            self.block(body, narrowed=False)
        else:
            self.expression(body)
        if not self.collapsible or UNMATCHED_ARGUMENT in self.literals:
            return None
        return frozenset(self.literals)

    # --- nested-scope references (a syntactic superset of captures) ------

    def _names_parameter(self, root: ast.AST) -> bool:
        return any(
            isinstance(child, ast.Name)
            and child.id == self.name
            or isinstance(child, ast.arg)
            and child.arg == self.name
            for child in ast.walk(root)
        )

    # --- statements ---------------------------------------------------------

    def block(self, statements: Sequence[ast.stmt], *, narrowed: bool) -> None:
        for statement in statements:
            if not narrowed:
                self.statement(statement)
                narrowed = self._terminal_chain(statement)

    def statement(self, statement: ast.stmt) -> None:
        if isinstance(statement, _NESTED_SCOPES):
            if self._names_parameter(statement):
                self.collapsible = False
            return
        if isinstance(statement, ast.If):
            literals = self._positive_literal_test(statement.test)
            if literals is not None:
                self.literals |= literals
                # The true branch sees the parameter equal to a literal.
            else:
                self.expression(statement.test)
                self.block(statement.body, narrowed=False)
            self.block(statement.orelse, narrowed=False)
            return
        if isinstance(statement, ast.Raise) and self._builtin_message(statement):
            return
        for field, value in ast.iter_fields(statement):
            del field
            if isinstance(value, list):
                if value and isinstance(value[0], ast.stmt):
                    self.block(value, narrowed=False)
                    continue
                for item in value:
                    if isinstance(item, ast.AST):  # Global/Nonlocal names are str
                        self.child(item)
            elif isinstance(value, ast.AST):
                self.child(value)

    def child(self, node: ast.AST) -> None:
        if isinstance(node, ast.stmt):
            self.statement(node)
        elif isinstance(node, ast.expr):
            self.expression(node)
        elif isinstance(node, ast.ExceptHandler):
            if node.type is not None:
                self.expression(node.type)
            self.block(node.body, narrowed=False)
        elif isinstance(node, ast.match_case):
            # A pattern or guard that names the parameter is not a literal test.
            if self._names_parameter(node.pattern) or (
                node.guard is not None and self._names_parameter(node.guard)
            ):
                self.collapsible = False
            if node.guard is not None:
                self.expression(node.guard)
            self.block(node.body, narrowed=False)
        else:
            for item in ast.iter_child_nodes(node):
                self.child(item)

    def _terminal_chain(self, statement: ast.stmt) -> bool:
        while isinstance(statement, ast.If):
            if self._positive_literal_test(statement.test) is None:
                return False
            if len(statement.orelse) == 1 and isinstance(statement.orelse[0], ast.If):
                statement = statement.orelse[0]
                continue
            return bool(statement.orelse) and isinstance(
                statement.orelse[-1], (ast.Raise, ast.Return)
            )
        return False

    def _builtin_message(self, statement: ast.Raise) -> bool:
        call = statement.exc
        if (
            statement.cause is not None
            or not isinstance(call, ast.Call)
            or call.keywords
        ):
            return False
        evidence = self.call_evidence(call)
        if (
            evidence is None
            or evidence.reaches_source
            or evidence.callees & ~_MESSAGE_CALLEES
        ):
            return False
        for argument in call.args:
            if isinstance(argument, ast.JoinedStr):
                for part in argument.values:
                    if (
                        isinstance(part, ast.FormattedValue)
                        and isinstance(part.value, ast.Name)
                        and part.value.id == self.name
                        and part.format_spec is None
                    ):
                        continue
                    self.expression(part)
            else:
                self.expression(argument)
        self.expression(call.func)
        return True

    # --- expressions --------------------------------------------------------

    def _literal_test(self, node: ast.expr) -> frozenset[str] | None:
        if not isinstance(node, ast.Compare) or len(node.ops) != 1:
            return None
        operator, left, right = node.ops[0], node.left, node.comparators[0]
        if isinstance(operator, (ast.Eq, ast.NotEq)):
            for subject, literal in ((left, right), (right, left)):
                if (
                    isinstance(subject, ast.Name)
                    and subject.id == self.name
                    and isinstance(literal, ast.Constant)
                    and type(literal.value) is str
                ):
                    return frozenset((literal.value,))
            return None
        if (
            isinstance(operator, (ast.In, ast.NotIn))
            and isinstance(left, ast.Name)
            and left.id == self.name
        ):
            return _string_literals(right)
        return None

    def _positive_literal_test(self, node: ast.expr) -> frozenset[str] | None:
        if isinstance(node, ast.Compare) and isinstance(node.ops[0], (ast.Eq, ast.In)):
            return self._literal_test(node)
        return None

    def expression(self, node: ast.AST) -> None:
        if isinstance(node, _NESTED_SCOPES):
            if self._names_parameter(node):
                self.collapsible = False
            return
        if isinstance(node, ast.expr):
            literals = self._literal_test(node)
            if literals is not None:
                self.literals |= literals
                return
            if isinstance(node, ast.Name) and node.id == self.name:
                self.collapsible = False
                return
        for child in ast.iter_child_nodes(node):
            self.expression(child)
