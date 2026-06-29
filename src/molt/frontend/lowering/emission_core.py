"""EmissionCoreMixin: dispatch, op emission, and bridge-unavailable policy.

Move-only extraction from frontend/__init__.py. This mixin owns the generator's
cross-cutting emission control path: AST dispatch line markers, temporary and
label allocation, CHECK_EXCEPTION insertion, scalar fast-path predicates, and
unsupported-feature bridge diagnostics.
"""

from __future__ import annotations

import ast
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any, Literal

from molt.frontend._types import (
    _FAST_ARITH_OPS,
    CompatibilityError,
    MoltOp,
    MoltValue,
)
from molt.frontend.lowering.op_kinds_generated import (
    CHECK_EXCEPTION_SKIP_KINDS,
    RAISING_KIND_NAMES,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class EmissionCoreMixin(_MixinBase):
    def visit(self, node: ast.AST) -> Any:
        try:
            if isinstance(node, (ast.stmt, ast.ExceptHandler)):
                lineno = getattr(node, "lineno", None)
                if lineno:
                    col = getattr(node, "col_offset", None)
                    end_col = getattr(node, "end_col_offset", None)
                    self._emit_line_marker(int(lineno), col, end_col)
            # Track expression-level column offsets for traceback carets.
            # When an expression node is visited, record its position so
            # that ops emitted during this visit carry the expression's
            # col_offset (not the statement's).
            if isinstance(node, ast.expr):
                col = getattr(node, "col_offset", None)
                end_col = getattr(node, "end_col_offset", None)
                if col is not None and end_col is not None:
                    prev = getattr(self, "_expr_col", None)
                    self._expr_col = (col, end_col)
                    result = super().visit(node)
                    self._expr_col = prev
                    return result
            return super().visit(node)
        except CompatibilityError:
            raise
        except NotImplementedError as exc:
            raise self.compat.unsupported(
                node,
                feature=str(exc),
                tier="bridge",
                impact="high",
            ) from exc

    def next_var(self) -> str:
        name = f"v{self.var_count}"
        self.var_count += 1
        return name

    def next_label(self) -> int:
        self.state_count += 1
        return self.state_count

    @contextmanager
    def _suppress_check_exception(self, *, emit_on_exit: bool = True) -> Any:
        prior = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)
        try:
            yield
        finally:
            self.try_suppress_depth = prior
            if emit_on_exit:
                if (
                    self.try_suppress_depth is None
                    or len(self.try_end_labels) > self.try_suppress_depth
                ):
                    if self.try_end_labels:
                        handler_label = self.try_end_labels[-1]
                    else:
                        handler_label = self.function_exception_label
                    if handler_label is not None:
                        self.emit(
                            MoltOp(
                                kind="CHECK_EXCEPTION",
                                args=[handler_label],
                                result=MoltValue("none"),
                            )
                        )

    def emit(self, op: MoltOp) -> None:
        # Auto-attach expression column offsets to raising ops. RAISING_KIND_NAMES
        # is generated from runtime/molt-ir/src/tir/op_kinds.toml (the
        # [[frontend_raising_kind]] table cross-checked against the [[opcode]]
        # may_throw oracle) - see the module-level import.
        expr_col = getattr(self, "_expr_col", None)
        if (
            op.col_offset is None
            and op.kind in RAISING_KIND_NAMES
            and expr_col is not None
        ):
            op.col_offset, op.end_col_offset = expr_col
        if (
            op.kind == "CONST"
            and op.result
            and isinstance(op.args[0], int)
            and not isinstance(op.args[0], bool)
        ):
            self.const_ints[op.result.name] = op.args[0]
        if op.result is not None and op.result.name not in ("none", ""):
            self._op_by_result[op.result.name] = op
        self.current_ops.append(op)
        if (
            self.try_suppress_depth is not None
            and len(self.try_end_labels) <= self.try_suppress_depth
        ):
            return
        if self.try_end_labels:
            handler_label = self.try_end_labels[-1]
        else:
            if self.function_exception_label is None:
                return
            handler_label = self.function_exception_label
        # CHECK_EXCEPTION_SKIP_KINDS is generated from op_kinds.toml's
        # [[frontend_check_exception_skip]] table (control-flow / structural
        # kinds, plus RAISE / STATE_TRANSITION whose exceptional edge is handled
        # structurally). Opcode-backed members are cross-checked against the
        # may_throw oracle at generation. See the module-level import.
        if op.kind in CHECK_EXCEPTION_SKIP_KINDS:
            return
        self.current_ops.append(
            MoltOp(
                kind="CHECK_EXCEPTION",
                args=[handler_label],
                result=MoltValue("none"),
            )
        )

    def _emit_line_marker(
        self,
        lineno: int,
        col_offset: int | None = None,
        end_col_offset: int | None = None,
    ) -> None:
        if lineno <= 0:
            return
        if self.current_line == lineno:
            return
        self.current_line = lineno
        op = MoltOp(
            kind="LINE",
            args=[lineno],
            result=MoltValue("none"),
            source_line=lineno,
        )
        # Attach column offsets for traceback caret annotations.
        if col_offset is not None:
            op.col_offset = col_offset
        if end_col_offset is not None:
            op.end_col_offset = end_col_offset
        self.emit(op)

    def _emit_line_marker_force(self) -> None:
        if not self.current_line or self.current_line <= 0:
            return
        self.emit(
            MoltOp(
                kind="LINE",
                args=[self.current_line],
                result=MoltValue("none"),
            )
        )

    def _fast_int_enabled(self) -> bool:
        return self._hints_enabled()

    def _hints_enabled(self) -> bool:
        return self.type_hint_policy in {"trust", "check"} or self.stdlib_hint_trust

    def _should_fast_int(self, op: MoltOp) -> bool:
        if op.kind not in _FAST_ARITH_OPS:
            return False
        if op.kind in {"NEG", "POS"}:
            return all(
                isinstance(arg, MoltValue) and arg.type_hint == "int" for arg in op.args
            )
        # Bitwise ops on bools must NOT use the fast_int path because the
        # backend's inline band/bor/bxor + box_int_value always returns an
        # int, losing the bool type.  CPython preserves bool: True & False
        # returns False (bool), not 0 (int).  The slow path (runtime call)
        # handles bool operands correctly via from_bool.
        if op.kind in {"BIT_AND", "BIT_OR", "BIT_XOR"} and any(
            isinstance(arg, MoltValue) and arg.type_hint == "bool" for arg in op.args
        ):
            return False
        return all(
            isinstance(arg, MoltValue) and arg.type_hint in {"int", "bool"}
            for arg in op.args
        )

    def _should_fast_float(self, op: MoltOp) -> bool:
        if op.kind not in _FAST_ARITH_OPS:
            return False
        return all(
            isinstance(arg, MoltValue) and arg.type_hint == "float" for arg in op.args
        )

    def _emit_bridge_unavailable(self, message: str) -> MoltValue:
        msg_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[message], result=msg_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="BRIDGE_UNAVAILABLE", args=[msg_val], result=res))
        return res

    def _bridge_fallback(
        self,
        node: ast.AST,
        feature: str,
        *,
        impact: Literal["low", "medium", "high"] = "high",
        alternative: str | None = None,
        detail: str | None = None,
    ) -> MoltValue:
        issue = self.compat.bridge_unavailable(
            node, feature, impact=impact, alternative=alternative, detail=detail
        )
        if self.fallback_policy != "bridge":
            raise self.compat.error(issue)
        return self._emit_bridge_unavailable(issue.runtime_message())
