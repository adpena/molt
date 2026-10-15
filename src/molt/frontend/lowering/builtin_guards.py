"""BuiltinGuardMixin: guarded builtin specialization inside functions (HF-79).

Inside a Python function, a read of a builtin name is a live lookup. The
active globals may bind the name, and any code may rebind or delete
``builtins.len`` before or while the function runs, so the binding analysis
proves no identity there. CPython 3.12 still specializes such reads behind a
runtime guard (``LOAD_GLOBAL_BUILTIN``). Molt does the same.

The binding analysis gives each builtin read in an activation scope whose
module never binds the name a bit of the guard mask
(``runtime/builtin_binding_guards.toml``). At the source point where Python
loads the name, the lowering emits one ``molt_guard_builtin_bindings(mask)``
call and branches on it:

- the fast arm lowers the source region with the covered reads trusted as the
  runtime's builtins, so the existing specializations apply (``len`` to
  ``LEN``, ``ord(s[i])`` to ``ord_at``, ``range`` to a counted loop);
- the slow arm lowers the same region with no guard inside, as the dynamic
  lookup and call it has always been.

One guard covers the callee and every later builtin read that Python performs
before any code that could run Python (and so rebind a name): ``range(len(xs))``
and ``isinstance(x, int)`` need one guard each. Both arms lower the same
source nodes, so a region that defines a scope or suspends is never
duplicated; it keeps the dynamic lowering.
"""

from __future__ import annotations

import ast
from typing import Any, Literal

from molt.compiler_analysis.python_binding_facts import PythonNodeKey
from molt.compiler_analysis.python_effects_generated import (
    NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
    REFLECTS_NAMESPACE,
    SUSPENDS,
    WRITES_FRAME_STATE,
    WRITES_GLOBAL_NAMESPACE,
)
from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import MoltValue
from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED

GUARD_RUNTIME_SYMBOL = "molt_guard_builtin_bindings"

# An expression with none of these effects runs no Python code and writes no
# namespace, so no binding can change while it evaluates.
_GUARD_CROSSING_FORBIDDEN_EFFECTS = (
    NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    | SUSPENDS
    | WRITES_GLOBAL_NAMESPACE
    | WRITES_FRAME_STATE
    | REFLECTS_NAMESPACE
)

# Both arms lower the same nodes. A nested scope would be defined twice and a
# suspension point would get two resume states, so such regions stay dynamic.
_UNDUPLICABLE_NODES = (
    ast.Lambda,
    ast.FunctionDef,
    ast.AsyncFunctionDef,
    ast.ClassDef,
    ast.ListComp,
    ast.SetComp,
    ast.DictComp,
    ast.GeneratorExp,
    ast.Yield,
    ast.YieldFrom,
    ast.Await,
    ast.AsyncFor,
    ast.AsyncWith,
    ast.Global,
    ast.Nonlocal,
)

# A guarded loop lowers its body twice; past this size the dynamic loop is the
# better trade for code size.
_MAX_DUPLICATED_NODES = 600

GuardArmKind = Literal["fast", "slow"]


class MoltBuiltinGuardTest(ast.expr):
    """Synthetic condition: the guard call for ``mask``."""

    _fields = ()

    def __init__(self, mask: int) -> None:
        super().__init__()
        self.mask = mask


class MoltBuiltinGuardArmExpr(ast.expr):
    """Synthetic arm wrapping one source expression."""

    _fields = ("value",)

    def __init__(
        self, value: ast.expr, kind: GuardArmKind, covered: frozenset[PythonNodeKey]
    ) -> None:
        super().__init__()
        self.value = value
        self.kind = kind
        self.covered = covered


class MoltBuiltinGuardArm(ast.stmt):
    """Synthetic arm wrapping one source statement."""

    _fields = ("body",)

    def __init__(
        self,
        body: list[ast.stmt],
        kind: GuardArmKind,
        covered: frozenset[PythonNodeKey],
    ) -> None:
        super().__init__()
        self.body = body
        self.kind = kind
        self.covered = covered


class BuiltinGuardMixin(GeneratorMixinBase):
    # (arm kind, reads the enclosing guard proves); None outside any arm.
    _builtin_guard_arm: tuple[GuardArmKind, frozenset[PythonNodeKey]] | None

    def _builtin_guard_bit(self, node: ast.AST) -> int:
        if (
            not isinstance(node, ast.Name)
            or not isinstance(node.ctx, ast.Load)
            or self.python_binding_index is None
            or node.id in self.locals
            or node.id in self.boxed_locals
        ):
            return 0
        fact = self.python_binding_index.expression_fact(node)
        return 0 if fact is None else fact.builtin_guard_bit

    def _builtin_guard_covers(self, node: ast.AST) -> bool:
        """The enclosing fast arm's guard proves this read is the builtin."""
        arm = self._builtin_guard_arm
        return (
            arm is not None
            and arm[0] == "fast"
            and isinstance(node, ast.Name)
            and PythonNodeKey.from_node(node) in arm[1]
        )

    def _builtin_guard_may_open(self, callee: ast.AST) -> bool:
        arm = self._builtin_guard_arm
        if arm is not None and arm[0] == "slow":
            return False
        return bool(self._builtin_guard_bit(callee)) and not self._builtin_guard_covers(
            callee
        )

    def _expression_runs_no_python(self, node: ast.expr) -> bool:
        if self.python_binding_index is None:
            return False
        fact = self.python_binding_index.expression_fact(node)
        return fact is not None and not fact.effects & _GUARD_CROSSING_FORBIDDEN_EFFECTS

    def _builtin_guard_coverage(
        self, root: ast.expr
    ) -> tuple[int, frozenset[PythonNodeKey]]:
        """The guard mask and reads one guard at ``root``'s first load proves.

        Walk Python's evaluation order from the callee. Name loads and
        constants run no Python code; a call runs its callee; any other
        expression continues the walk only when the binding analysis proves it
        runs no Python code, and its inner reads stay unguarded.
        """
        mask = 0
        covered: set[PythonNodeKey] = set()

        def walk(node: ast.expr) -> bool:
            nonlocal mask
            if isinstance(node, ast.Name):
                bit = self._builtin_guard_bit(node)
                if bit:
                    mask |= bit
                    covered.add(PythonNodeKey.from_node(node))
                return True
            if isinstance(node, ast.Constant):
                return True
            if isinstance(node, ast.Call):
                if not walk(node.func):
                    return False
                for arg in node.args:
                    if isinstance(arg, ast.Starred) or not walk(arg):
                        return False
                for keyword in node.keywords:
                    if keyword.arg is None or not walk(keyword.value):
                        return False
                return False
            if isinstance(node, (ast.Tuple, ast.List)) and isinstance(
                node.ctx, ast.Load
            ):
                return all(
                    not isinstance(element, ast.Starred) and walk(element)
                    for element in node.elts
                )
            return self._expression_runs_no_python(node)

        walk(root)
        return mask, frozenset(covered)

    @staticmethod
    def _builtin_guard_region_duplicable(region: ast.AST) -> bool:
        count = 0
        for node in ast.walk(region):
            count += 1
            if count > _MAX_DUPLICATED_NODES or isinstance(node, _UNDUPLICABLE_NODES):
                return False
        return True

    def _try_emit_guarded_super_method_call(self, node: ast.Call) -> Any:
        """Guard zero-argument ``super().name(*args)``: CPython's LOAD_SUPER_ATTR.

        CPython checks at run time that its global ``super`` is the builtin type
        before it loads the method without a super object. Here one guard at
        the ``super`` read proves that binding for the whole call: the fast arm
        loads the method pair from the ``__class__`` cell and argument zero, and
        the slow arm calls whatever ``super`` names, as CPython's fallback does.
        """
        receiver = self._zero_argument_super_receiver(node)
        if receiver is None:
            return CALL_NOT_HANDLED
        if self._builtin_guard_covers(receiver.func):
            return self._emit_super_method_call(node)
        if not self._builtin_guard_may_open(
            receiver.func
        ) or not self._builtin_guard_region_duplicable(node):
            return CALL_NOT_HANDLED
        covered = frozenset({PythonNodeKey.from_node(receiver.func)})
        conditional = ast.copy_location(
            ast.IfExp(
                test=ast.copy_location(
                    MoltBuiltinGuardTest(self._builtin_guard_bit(receiver.func)), node
                ),
                body=ast.copy_location(
                    MoltBuiltinGuardArmExpr(node, "fast", covered), node
                ),
                orelse=ast.copy_location(
                    MoltBuiltinGuardArmExpr(node, "slow", frozenset()), node
                ),
            ),
            node,
        )
        return self.visit_IfExp(conditional)

    def _try_emit_builtin_guarded_call(self, node: ast.Call) -> Any:
        """Guard a builtin call: specialized when proven, dynamic otherwise."""
        if (
            # A bare `super()` lowers alike in both arms; only its method form
            # specializes (`_try_emit_guarded_super_method_call`).
            (isinstance(node.func, ast.Name) and node.func.id == "super")
            or not self._builtin_guard_may_open(node.func)
            or any(isinstance(arg, ast.Starred) for arg in node.args)
            or any(keyword.arg is None for keyword in node.keywords)
        ):
            # The runtime binder owns splat cardinality and keyword errors, so
            # a splat call has no specialized lowering to guard.
            return CALL_NOT_HANDLED
        mask, covered = self._builtin_guard_coverage(node)
        if not mask or not self._builtin_guard_region_duplicable(node):
            return CALL_NOT_HANDLED
        conditional = ast.copy_location(
            ast.IfExp(
                test=ast.copy_location(MoltBuiltinGuardTest(mask), node),
                body=ast.copy_location(
                    MoltBuiltinGuardArmExpr(node, "fast", covered), node
                ),
                orelse=ast.copy_location(
                    MoltBuiltinGuardArmExpr(node, "slow", frozenset()), node
                ),
            ),
            node,
        )
        return self.visit_IfExp(conditional)

    def _try_emit_builtin_guarded_range_loop(self, node: ast.For) -> bool:
        """Guard ``for ... in range(...)``: a counted loop when proven."""
        iterable = node.iter
        if (
            not isinstance(iterable, ast.Call)
            or not isinstance(iterable.func, ast.Name)
            or iterable.func.id != "range"
            or not self._builtin_guard_may_open(iterable.func)
        ):
            return False
        mask, covered = self._builtin_guard_coverage(iterable)
        if not mask or not self._builtin_guard_region_duplicable(node):
            return False
        guarded = ast.copy_location(
            ast.If(
                test=ast.copy_location(MoltBuiltinGuardTest(mask), iterable),
                body=[
                    ast.copy_location(
                        MoltBuiltinGuardArm([node], "fast", covered), node
                    )
                ],
                orelse=[
                    ast.copy_location(
                        MoltBuiltinGuardArm([node], "slow", frozenset()), node
                    )
                ],
            ),
            node,
        )
        self.visit_If(guarded)
        return True

    def visit_MoltBuiltinGuardTest(self, node: MoltBuiltinGuardTest) -> MoltValue:
        mask = self._emit_const_value(node.mask)
        return self._emit_runtime_call(GUARD_RUNTIME_SYMBOL, [mask], type_hint="bool")

    def visit_MoltBuiltinGuardArmExpr(self, node: MoltBuiltinGuardArmExpr) -> Any:
        previous = self._builtin_guard_arm
        self._builtin_guard_arm = (node.kind, node.covered)
        try:
            return self.visit(node.value)
        finally:
            self._builtin_guard_arm = previous

    def visit_MoltBuiltinGuardArm(self, node: MoltBuiltinGuardArm) -> None:
        previous = self._builtin_guard_arm
        self._builtin_guard_arm = (node.kind, node.covered)
        try:
            self._visit_block(node.body)
        finally:
            self._builtin_guard_arm = previous
