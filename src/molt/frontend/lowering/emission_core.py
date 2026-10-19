"""EmissionCoreMixin: dispatch, op emission, and typed capability diagnostics.

Move-only extraction from frontend/__init__.py. This mixin owns the generator's
cross-cutting emission control path: AST dispatch line markers, temporary and
label allocation, CHECK_EXCEPTION insertion, scalar fast-path predicates, and
unsupported-feature diagnostic conversion.
"""

from __future__ import annotations

import ast
from contextlib import contextmanager
from typing import Any

from molt.compiler_analysis.python_effects_generated import (
    NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
)
from molt.frontend._types import (
    CompatibilityError,
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendRejection, raise_compatibility_error
from molt.frontend.lowering.op_kinds_generated import (
    CHECK_EXCEPTION_SKIP_KINDS,
    FRONTEND_ARBITRARY_HEAP_EFFECT,
    FRONTEND_REPOLL_KINDS,
    RAISING_KIND_NAMES,
)
from molt.frontend._mixin_base import GeneratorMixinBase


class EmissionCoreMixin(GeneratorMixinBase):
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
                exact_token_before = self.exact_class_token
                col = getattr(node, "col_offset", None)
                end_col = getattr(node, "end_col_offset", None)
                prev = getattr(self, "_expr_col", None)
                if col is not None and end_col is not None:
                    self._expr_col = (col, end_col)
                try:
                    result = super().visit(node)
                finally:
                    if col is not None and end_col is not None:
                        self._expr_col = prev
                if isinstance(node, ast.Call):
                    # The instruction that evaluates a source call adopts its
                    # operands, as CPython's CALL does.
                    self._mark_source_call(result, node)
                fact = (
                    self.python_binding_index.expression_fact(node)
                    if self.python_binding_index is not None
                    else None
                )
                if fact is None or (
                    fact.effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
                ):
                    # Any transfer into Python can mutate a reachable
                    # instance's ``__class__``. Emitted callback boundaries
                    # normally advance the token in source order. This
                    # aggregate fallback fires only when lowering emitted no
                    # such boundary; it never re-mints a producer fact.
                    if self.exact_class_token == exact_token_before:
                        self._expire_exact_class_facts()
                return result
            return super().visit(node)
        except CompatibilityError:
            raise
        except FrontendRejection as rejection:
            raise_compatibility_error(self.compat, node, rejection)

    def next_var(self) -> str:
        while True:
            name = f"v{self.var_count}"
            self.var_count += 1
            if name not in self.reserved_python_identifiers:
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
        if op.kind in {"COPY", "IDENTITY_ALIAS", "PHI"} and op.result is not None:
            op.result.borrows_binding |= any(
                isinstance(value, MoltValue) and value.borrows_binding
                for value in op.args
            )
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
        if FRONTEND_ARBITRARY_HEAP_EFFECT.get(op.kind, True):
            # This generated opcode-effect projection also covers callback
            # boundaries synthesized by statement lowering. Unknown kinds fail
            # closed instead of creating a second handwritten effect registry.
            self._expire_exact_class_facts()
        if self.python_frame_context_active and (
            op.kind == "STATE_LABEL" or op.kind in FRONTEND_REPOLL_KINDS
        ):
            # Explicit states and implicit post-await continuations re-enter
            # fresh runtime frames before any resumed Python callback.
            self._publish_python_frame_context()
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

    def _hints_enabled(self) -> bool:
        return self.type_hint_policy in {"trust", "check"} or self.stdlib_hint_trust
