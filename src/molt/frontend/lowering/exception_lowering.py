"""ExceptionLoweringMixin: exception construction, unwinds, and try/except CFG.

Move-only extraction from frontend/__init__.py. This lowering authority owns
exception object construction, unbound/error guards, active exception handler
cleanup, finalbody/control-flow unwinds, raise exits, pending exception checks,
and the synchronous try/except split CFG shared by control-flow, function, loop,
call, import, annotation, comprehension, and async visitors.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Sequence

from molt.frontend._types import (
    ActiveException,
    BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS,
    MoltOp,
    MoltValue,
    TryScope,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ExceptionLoweringMixin(_MixinBase):
    def _emit_exception_class(self, name: str) -> MoltValue:
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=kind_val))
        class_val = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="EXCEPTION_CLASS", args=[kind_val], result=class_val))
        return class_val

    def _emit_exception_new_from_args(
        self, kind: str, args: list[MoltValue]
    ) -> MoltValue:
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        if kind_tag := BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS.get(kind):
            if not args:
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_NEW_BUILTIN_EMPTY",
                        args=[],
                        result=exc_val,
                        metadata={"exception_name": kind, "exception_tag": kind_tag},
                    )
                )
                return exc_val
            if len(args) == 1:
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_NEW_BUILTIN_ONE",
                        args=[args[0]],
                        result=exc_val,
                        metadata={"exception_name": kind, "exception_tag": kind_tag},
                    )
                )
                return exc_val
            args_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=args, result=args_val))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_NEW_BUILTIN",
                    args=[args_val],
                    result=exc_val,
                    metadata={"exception_name": kind, "exception_tag": kind_tag},
                )
            )
            return exc_val
        args_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=args, result=args_val))
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[kind], result=kind_val))
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW",
                args=[kind_val, args_val],
                result=exc_val,
            )
        )
        return exc_val

    def _emit_exception_new(self, kind: str, message: str | MoltValue) -> MoltValue:
        args: list[MoltValue] = []
        if isinstance(message, MoltValue):
            if message.type_hint == "str":
                args = [message]
            else:
                args = [self._emit_str_from_obj(message)]
        elif message:
            msg_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[message], result=msg_val))
            args = [msg_val]
        return self._emit_exception_new_from_args(kind, args)

    def _emit_missing_value(self) -> MoltValue:
        missing = MoltValue(self.next_var(), type_hint="missing")
        self.emit(MoltOp(kind="MISSING", args=[], result=missing))
        return missing

    def _emit_unbound_local_guard(self, value: MoltValue, name: str) -> None:
        missing = self._emit_missing_value()
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, missing], result=is_missing))
        self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
        if self.current_func_name == "molt_main":
            msg = f"name '{name}' is not defined"
            err_val = self._emit_exception_new("NameError", msg)
        else:
            msg = (
                "cannot access local variable "
                f"'{name}' where it is not associated with a value"
            )
            err_val = self._emit_exception_new("UnboundLocalError", msg)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_unbound_free_guard(self, value: MoltValue, name: str) -> None:
        missing = self._emit_missing_value()
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, missing], result=is_missing))
        self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
        msg = (
            "cannot access free variable "
            f"'{name}' where it is not associated with a value in enclosing scope"
        )
        err_val = self._emit_exception_new("NameError", msg)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_type_error(self, message: str | MoltValue) -> None:
        err_val = self._emit_exception_new("TypeError", message)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))

    def _emit_exception_match(
        self, handler: ast.ExceptHandler, exc_val: MoltValue
    ) -> MoltValue:
        if handler.type is None:
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[1], result=res))
            return res
        if (
            isinstance(handler.type, ast.Name)
            and (kind_tag := BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS.get(handler.type.id))
            is not None
        ):
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_MATCH_BUILTIN",
                    args=[exc_val],
                    result=res,
                    metadata={
                        "exception_name": handler.type.id,
                        "exception_tag": kind_tag,
                    },
                )
            )
            return res
        # Evaluate the handler expression with the pending exception temporarily
        # cleared. Attribute-based handlers (e.g. `except mod.Error`) otherwise
        # fail to resolve correctly while an exception is active.
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        class_val = self.visit(handler.type)
        if class_val is None:
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[exc_val],
                    result=MoltValue("none"),
                )
            )
            self._bridge_fallback(
                handler,
                "except (unsupported handler)",
                alternative="use a lowered exception name or tuple",
                detail="handler expression could not be lowered",
            )
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[0], result=res))
            return res
        # Keep the pending exception cleared while matching. `isinstance`
        # only needs the explicit exception object and resolved class value;
        # restoring the global "last exception" here reintroduces stale
        # exception state into the handler CFG and is not semantically needed
        # for the match itself.
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="ISINSTANCE", args=[exc_val, class_val], result=res))
        return res

    def _active_exception_value(self, exc: ActiveException) -> MoltValue:
        if self.is_async() and exc.slot is not None:
            return self._reload_async_value(exc.slot, exc.value.type_hint)
        return exc.value

    def _emit_exception_handler_exit_cleanup(
        self, exc: ActiveException | None = None
    ) -> None:
        if exc is not None:
            handlers = [exc] if exc.is_handler else []
        else:
            handlers = [entry for entry in self.active_exceptions if entry.is_handler]
        if not handlers:
            return
        cleared_ctx = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_ctx))
        self.emit(
            MoltOp(
                kind="EXCEPTION_CONTEXT_SET",
                args=[cleared_ctx],
                result=MoltValue("none"),
            )
        )
        self._emit_active_handler_name_deletes(handlers)

    def _emit_active_handler_name_deletes(
        self, handlers: list["ActiveException"]
    ) -> None:
        """Delete the `except ... as NAME` target bindings for `handlers`.

        CPython lowers `except E as e:` to an implicit ``finally: del e`` that
        runs on *every* exit edge of the handler — normal fall-through, return,
        break/continue, and an exception escaping the handler body.  The normal
        and return/break paths route through `_emit_exception_handler_exit_cleanup`
        (which also clears the handling context); a `raise` inside the handler
        must delete the same names without clearing the context it just captured
        into the new exception.  Deleting the name only drops the binding — the
        exception object itself stays alive via any reference already taken
        (e.g. `raise X from e`).
        """
        for entry in reversed(handlers):
            if entry.handler_name:
                self._emit_delete_name(entry.handler_name, allow_missing=True)

    def _emit_escaping_handler_name_deletes(self) -> None:
        """Delete `except ... as NAME` targets for handlers a `raise` escapes.

        A `raise` only leaves an active handler — and so only runs that
        handler's implicit ``del NAME`` — when it is not caught by a `try`
        opened *after* the handler body began.  `handler_try_depth` records the
        live `try` nesting at the handler's entry; a handler is escaped iff the
        current live depth is no deeper than that recorded value (no inner
        `try` is protecting the raise).  Handlers protected by a nested `try`
        (whose own cleanup deletes their name when control actually leaves them)
        are left untouched.
        """
        if not self.active_exceptions:
            return
        live_depth = len(self.try_end_labels)
        escaped = [
            entry
            for entry in self.active_exceptions
            if entry.is_handler and live_depth <= entry.handler_try_depth
        ]
        self._emit_active_handler_name_deletes(escaped)

    def _emit_guarded_body(
        self, body: list[ast.stmt], baseline_exc: ActiveException | None
    ) -> None:
        if not body:
            return
        self.visit(body[0])
        remaining = body[1:]
        if not remaining:
            return
        skip_label = self.next_label()
        self.emit(
            MoltOp(
                kind="CHECK_EXCEPTION",
                args=[skip_label],
                result=MoltValue("none"),
            )
        )
        self._emit_guarded_body(remaining, baseline_exc)
        self.emit(MoltOp(kind="LABEL", args=[skip_label], result=MoltValue("none")))

    def _emit_finalbody(
        self,
        finalbody: list[ast.stmt],
        baseline_exc: ActiveException | None,
        *,
        popped_scopes: int = 0,
    ) -> None:
        self.return_unwind_depth += 1
        self.finally_depth += 1
        self.return_unwind_popped_scopes.append(popped_scopes)
        self._emit_guarded_body(finalbody, baseline_exc)
        self.return_unwind_popped_scopes.pop()
        self.finally_depth -= 1
        self.return_unwind_depth -= 1

    def _ctx_mark_arg(self, scope: TryScope) -> MoltValue:
        if not scope.needs_context_unwind or scope.ctx_mark is None:
            raise AssertionError("context unwind requested without a context mark")
        if scope.ctx_mark_offset is None or not self.is_async():
            return scope.ctx_mark
        res = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", scope.ctx_mark_offset],
                result=res,
            )
        )
        return res

    def _emit_context_unwind_to(self, scope: TryScope, exc_val: MoltValue) -> None:
        if not scope.needs_context_unwind:
            return
        ctx_arg = self._ctx_mark_arg(scope)
        self.emit(
            MoltOp(
                kind="CONTEXT_UNWIND_TO",
                args=[ctx_arg, exc_val],
                result=MoltValue("none"),
            )
        )

    def _emit_control_flow_scope_unwind(self, scopes: Sequence[TryScope]) -> list[int]:
        unwind_scopes = list(scopes)
        if not unwind_scopes:
            return []
        none_exc = None
        if self.context_depth > 0 and any(
            scope.needs_context_unwind for scope in unwind_scopes
        ):
            none_exc = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
        skip_pops = 0
        if self.return_unwind_depth > 0 and self.return_unwind_popped_scopes:
            skip_pops = self.return_unwind_popped_scopes[-1]
        popped_scopes = 0
        skip_finalbody = self.return_unwind_depth
        popped_labels: list[int] = []
        for scope in reversed(unwind_scopes):
            if skip_pops > 0:
                skip_pops -= 1
                popped_scopes += 1
                if skip_finalbody > 0:
                    skip_finalbody -= 1
                continue
            if none_exc is not None:
                self._emit_context_unwind_to(scope, none_exc)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_POP",
                    args=[],
                    result=MoltValue("none"),
                )
            )
            if scope.handler_label is not None:
                if scope.handler_label not in self.try_end_labels:
                    pass
                elif self.try_end_labels[-1] != scope.handler_label:
                    raise AssertionError(
                        "control-flow unwind tried to pop handler label "
                        f"{scope.handler_label}, active labels={self.try_end_labels}"
                    )
                else:
                    popped_labels.append(self.try_end_labels.pop())
            popped_scopes += 1
            if scope.finalbody:
                if skip_finalbody > 0:
                    skip_finalbody -= 1
                else:
                    prior_active = self.active_exceptions[:]
                    self.active_exceptions.clear()
                    self._emit_finalbody(
                        scope.finalbody, None, popped_scopes=popped_scopes
                    )
                    self.active_exceptions = prior_active
        return popped_labels

    def _restore_control_flow_unwind_labels(self, popped_labels: Sequence[int]) -> None:
        for label in reversed(popped_labels):
            self.try_end_labels.append(label)

    def _emit_raise_exit(self) -> None:
        if self.try_end_labels:
            if (
                self.try_suppress_depth is None
                or len(self.try_end_labels) > self.try_suppress_depth
            ):
                self.emit(
                    MoltOp(
                        kind="CHECK_EXCEPTION",
                        args=[self.try_end_labels[-1]],
                        result=MoltValue("none"),
                    )
                )
                self.emit(
                    MoltOp(
                        kind="JUMP",
                        args=[self.try_end_labels[-1]],
                        result=MoltValue("none"),
                    )
                )
                return
        if self.try_handler_scopes:
            done_label = self.try_handler_scopes[-1].done_label
            if done_label is not None:
                self.emit(
                    MoltOp(
                        kind="CHECK_EXCEPTION",
                        args=[done_label],
                        result=MoltValue("none"),
                    )
                )
                self.emit(
                    MoltOp(
                        kind="JUMP",
                        args=[done_label],
                        result=MoltValue("none"),
                    )
                )
                return
        if self.function_exception_label is not None:
            self._emit_restore_exception_stack_depth()
            self.emit(
                MoltOp(
                    kind="CHECK_EXCEPTION",
                    args=[self.function_exception_label],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="JUMP",
                    args=[self.function_exception_label],
                    result=MoltValue("none"),
                )
            )
            return
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        self.emit(MoltOp(kind="ret", args=[none_val], result=MoltValue("none")))

    def _emit_raise_if_pending(
        self,
        *,
        emit_exit: bool = False,
        clear_handlers: bool = False,
        force_exit: bool = False,
    ) -> None:
        # Use the same fast inline flag check as check_exception instead
        # of the exception_last → is → not → if → raise pattern.  The old
        # pattern produced stale-exception re-raise bugs because
        # exception_last() and the inline flag byte could disagree, and
        # the Cranelift-compiled if/raise/end_if sometimes executed the
        # raise unconditionally.
        handler_label: int | None
        if self.try_end_labels:
            handler_label = self.try_end_labels[-1]
        else:
            handler_label = self.function_exception_label
        if handler_label is not None:
            if (
                self.current_func_name == "molt_main"
                or self.current_func_name.startswith("molt_init_")
            ):
                self._emit_line_marker_force()
            self.emit(
                MoltOp(
                    kind="CHECK_EXCEPTION",
                    args=[handler_label],
                    result=MoltValue("none"),
                )
            )

    def _emit_sync_try_except_split(
        self,
        node: ast.Try,
        scope: TryScope,
        unbound_snapshot_try: set[str],
        prior_terminated: bool,
    ) -> None:
        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_exc_label = self.next_label()
        try_normal_label = self.next_label()
        try_clean_cleanup_label = self.next_label()
        try_pending_cleanup_label = self.next_label()
        try_done_label = self.next_label()
        scope.handler_label = try_exc_label
        scope.done_label = try_pending_cleanup_label
        self.try_end_labels.append(try_exc_label)
        self.emit(
            MoltOp(
                kind="TRY_START",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self._visit_block(node.body)
        body_terminated = self.block_terminated
        self.block_terminated = False
        if not body_terminated:
            self.emit(
                MoltOp(
                    kind="TRY_END",
                    args=[try_exc_label],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(kind="JUMP", args=[try_normal_label], result=MoltValue("none"))
            )
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(
                kind="TRY_END",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self.try_end_labels.pop()
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)
        self.try_handler_scopes.append(scope)

        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST_PENDING", args=[], result=exc_val))
        self._emit_context_unwind_to(scope, exc_val)

        def emit_handlers(handlers: list[ast.ExceptHandler]) -> None:
            if not handlers:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
                return
            handler = handlers[0]
            match_val = self._emit_exception_match(handler, exc_val)
            self.emit(MoltOp(kind="IF", args=[match_val], result=MoltValue("none")))
            exc_slot_offset = None
            if handler.name:
                if self.current_func_name == "molt_main":
                    self.module_global_mutations.add(handler.name)
                self._store_local_value(handler.name, exc_val)
            exc_entry = ActiveException(
                value=exc_val,
                slot=exc_slot_offset,
                handler_name=handler.name,
                is_handler=True,
                handler_try_depth=len(self.try_end_labels),
            )
            self.active_exceptions.append(exc_entry)
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[exc_val],
                    result=MoltValue("none"),
                )
            )
            self._emit_guarded_body(handler.body, exc_entry)
            handler_terminated = self.block_terminated
            if not handler_terminated:
                self._emit_exception_handler_exit_cleanup(exc_entry)
            self.active_exceptions.pop()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            if len(handlers) > 1:
                emit_handlers(handlers[1:])
            else:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        emit_handlers(node.handlers)
        self.emit(
            MoltOp(
                kind="JUMP",
                args=[try_pending_cleanup_label],
                result=MoltValue("none"),
            )
        )

        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_normal_label],
                result=MoltValue("none"),
            )
        )
        if node.orelse:
            self._emit_guarded_body(node.orelse, None)
            self.emit(
                MoltOp(
                    kind="JUMP",
                    args=[try_pending_cleanup_label],
                    result=MoltValue("none"),
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="JUMP",
                    args=[try_clean_cleanup_label],
                    result=MoltValue("none"),
                )
            )
        self.try_handler_scopes.pop()
        self.try_suppress_depth = prior_suppress
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_clean_cleanup_label],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="JUMP", args=[try_done_label], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_pending_cleanup_label],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True)
        self.emit(MoltOp(kind="JUMP", args=[try_done_label], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_done_label],
                result=MoltValue("none"),
            )
        )
        self.try_scopes.pop()
        self.unbound_check_names = unbound_snapshot_try
        self.control_flow_depth -= 1
        self.block_terminated = prior_terminated
