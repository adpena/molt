"""ExceptionLoweringMixin: exception construction, unwinds, and try/except CFG.

Move-only extraction from frontend/__init__.py. This lowering authority owns
exception object construction, unbound/error guards, active exception handler
cleanup, finalbody/control-flow unwinds, raise exits, pending exception checks,
and the synchronous try/except split CFG shared by control-flow, function, loop,
call, import, annotation, comprehension, and async visitors.
"""

from __future__ import annotations

import ast
from typing import Sequence

from molt.frontend._types import (
    ActiveException,
    AsyncContextExit,
    BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS,
    MoltOp,
    MoltValue,
    ScratchCell,
    SyncContextExit,
    TryScope,
)
from molt.frontend._mixin_base import GeneratorMixinBase


class ExceptionLoweringMixin(GeneratorMixinBase):
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
        self,
        body: list[ast.stmt],
        *,
        abandon_on_unwind: ScratchCell | None = None,
    ) -> None:
        if not body:
            return
        # Handler, else and finally bodies need a real cleanup continuation.
        # Per-statement checks miss exceptions inside a nonlocal unwind and
        # can dispatch past the enclosing finally. One scope also avoids the
        # old recursive lowering of long guarded statement lists.
        cleanup_label = self.next_label()
        done_label = self.next_label() if abandon_on_unwind is not None else None
        scope = TryScope(
            finalbody=None,
            handler_label=cleanup_label,
            abandon_on_unwind=abandon_on_unwind,
        )
        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        self.try_scopes.append(scope)
        self.try_end_labels.append(cleanup_label)
        self.emit(
            MoltOp(kind="TRY_START", args=[cleanup_label], result=MoltValue("none"))
        )
        try:
            self._visit_block(body)
        finally:
            self.try_end_labels.pop()
            self.try_scopes.pop()
        if done_label is not None:
            # Ordinary cleanup completion keeps the pending transfer's value.
            # Only exceptional or escaping nonlocal exits abandon that value.
            self.emit(
                MoltOp(kind="TRY_END", args=[cleanup_label], result=MoltValue("none"))
            )
            self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="JUMP", args=[done_label], result=MoltValue("none")))
        self.emit(MoltOp(kind="LABEL", args=[cleanup_label], result=MoltValue("none")))
        self.emit(
            MoltOp(kind="TRY_END", args=[cleanup_label], result=MoltValue("none"))
        )
        if abandon_on_unwind is not None:
            self._clear_scratch_cell(abandon_on_unwind)
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        if done_label is not None:
            self.emit(MoltOp(kind="LABEL", args=[done_label], result=MoltValue("none")))
        # Successful nonlocal exits already left. The failure continuation is
        # reachable even if every source-level path ends with return/raise.
        self.block_terminated = False

    def _emit_finalbody(
        self,
        scope: TryScope,
        *,
        pending_return: ScratchCell | None = None,
    ) -> None:
        assert scope.finalbody is not None
        prior_running = scope.finalbody_running
        prior_loops = self.loop_scopes
        scope.finalbody_running = True
        # A nonlocal transfer can inline this body inside younger loops. Its
        # break/continue destinations still belong to the try's lexical site.
        self.loop_scopes = list(scope.lexical_loops)
        self.return_unwind_depth += 1
        self.finally_depth += 1
        try:
            self._emit_guarded_body(scope.finalbody, abandon_on_unwind=pending_return)
        finally:
            self.finally_depth -= 1
            self.return_unwind_depth -= 1
            self.loop_scopes = prior_loops
            scope.finalbody_running = prior_running

    def _emit_context_entry(
        self,
        action: SyncContextExit | AsyncContextExit,
        enter: MoltValue,
    ) -> MoltValue:
        """Release captured exit storage if entry fails, without calling exit."""
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = None
        try:
            failed = self.next_label()
            done = self.next_label()
            self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
            self.try_end_labels.append(failed)
            self.emit(MoltOp(kind="TRY_START", args=[failed], result=MoltValue("none")))
            try:
                if isinstance(action, SyncContextExit):
                    hint = (
                        enter.type_hint
                        if enter.type_hint in {"file_text", "file_bytes"}
                        else "Any"
                    )
                    entered = MoltValue(self.next_var(), type_hint=hint)
                    self.emit(
                        MoltOp(kind="CONTEXT_ENTER", args=[enter], result=entered)
                    )
                    self._emit_raise_if_pending()
                else:
                    awaitable = self._emit_call_bound_or_func(enter, [])
                    self._emit_raise_if_pending()
                    entered = self._emit_await_value(awaitable)
            finally:
                self.try_end_labels.pop()
            self.emit(MoltOp(kind="TRY_END", args=[failed], result=MoltValue("none")))
            self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="JUMP", args=[done], result=MoltValue("none")))
            self.emit(MoltOp(kind="LABEL", args=[failed], result=MoltValue("none")))
            self.emit(MoltOp(kind="TRY_END", args=[failed], result=MoltValue("none")))
            capture = (
                action.manager
                if isinstance(action, SyncContextExit)
                else action.callback
            )
            with self._suppress_check_exception(emit_on_exit=False):
                self._clear_scratch_cell(capture)
                self.emit(
                    MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none"))
                )
            self._emit_raise_exit()
            self.emit(MoltOp(kind="LABEL", args=[done], result=MoltValue("none")))
            return entered
        finally:
            self.try_suppress_depth = prior_suppress

    def _emit_context_body(
        self,
        node: ast.With | ast.AsyncWith,
        entered: MoltValue,
        action: SyncContextExit | AsyncContextExit,
    ) -> None:
        """One protected body/cleanup authority for sync and async managers."""
        handler = self.next_label()
        done = self.next_label()
        scope = TryScope(
            finalbody=None,
            handler_label=handler,
            done_label=done,
            context_exit=action,
        )
        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        self.try_scopes.append(scope)
        self.try_end_labels.append(handler)
        self.emit(
            MoltOp(
                kind="TRY_START",
                args=[handler],
                result=MoltValue("none"),
            )
        )
        self.control_flow_depth += 1
        unbound_snapshot = set(self.unbound_check_names)
        prior_terminated = self.block_terminated
        self.block_terminated = False
        try:
            target = node.items[0].optional_vars
            if target is not None:
                self._emit_assign_target(target, entered, None)
            terminated = self._visit_block(node.body)
        finally:
            self.unbound_check_names = unbound_snapshot
            self.block_terminated = prior_terminated
            self.control_flow_depth -= 1
            self.try_end_labels.pop()
            self.try_scopes.pop()
        if not terminated:
            self.emit(MoltOp(kind="TRY_END", args=[handler], result=MoltValue("none")))
            self._emit_context_exit(action)
            self.emit(MoltOp(kind="JUMP", args=[done], result=MoltValue("none")))
        self.emit(MoltOp(kind="LABEL", args=[handler], result=MoltValue("none")))
        self.emit(MoltOp(kind="TRY_END", args=[handler], result=MoltValue("none")))
        pending = MoltValue(self.next_var(), type_hint="exception")
        with self._suppress_check_exception(emit_on_exit=False):
            self.emit(MoltOp(kind="EXCEPTION_LAST_PENDING", args=[], result=pending))
            saved: MoltValue | ScratchCell
            if self.is_async():
                saved = self._new_scratch_cell(pending, type_hint="exception")
            else:
                # The observer's MatchRef is released at the body frame pop.
                # Cleanup needs an independent owned root, not an SSA copy of
                # that region-owned reference. Ordinary drop insertion owns it.
                saved = MoltValue(self.next_var(), type_hint="exception")
                self.emit(MoltOp(kind="BINDING_ALIAS", args=[pending], result=saved))
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self._emit_context_exit(action, exception=saved)
        self.emit(MoltOp(kind="LABEL", args=[done], result=MoltValue("none")))
        self._expire_exact_class_facts()

    def _emit_context_exit(
        self,
        action: SyncContextExit | AsyncContextExit,
        *,
        exception: MoltValue | ScratchCell | None = None,
        abandon_on_error: ScratchCell | None = None,
    ) -> None:
        """Retain handled context through exit invocation, await and truth test.

        The consumed body cannot catch an exit failure. A separate cleanup
        continuation restores the enclosing context on every exceptional path.
        Normal exits never truth-test the callback's ignored return value.
        """
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        guarded = exception is not None or abandon_on_error is not None
        cleanup = self.next_label() if guarded else None
        done = self.next_label() if guarded else None
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = None
        if cleanup is not None:
            self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
            self.try_end_labels.append(cleanup)
            self.emit(
                MoltOp(kind="TRY_START", args=[cleanup], result=MoltValue("none"))
            )
        try:
            try:
                if exception is None:
                    error = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=error))
                else:
                    error = (
                        self._load_scratch_cell(exception)
                        if isinstance(exception, ScratchCell)
                        else exception
                    )
                    self.emit(
                        MoltOp(
                            kind="EXCEPTION_CONTEXT_SET",
                            args=[error],
                            result=MoltValue("none"),
                        )
                    )
                if isinstance(action, SyncContextExit):
                    manager = self._consume_scratch_cell(action.manager)
                    result = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CONTEXT_EXIT", args=[manager, error], result=result
                        )
                    )
                    self._emit_raise_if_pending()
                else:
                    callback = self._consume_scratch_cell(action.callback)
                    if exception is None:
                        args = [error, error, error]
                    else:
                        kind = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(MoltOp(kind="TYPE_OF", args=[error], result=kind))
                        traceback = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(
                                kind="GETATTR_GENERIC_OBJ",
                                args=[error, "__traceback__"],
                                result=traceback,
                            )
                        )
                        args = [kind, error, traceback]
                    awaitable = self._emit_call_bound_or_func(callback, args)
                    self._emit_raise_if_pending()
                    result = self._emit_await_value(awaitable)
                if exception is not None:
                    not_suppressed = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="NOT", args=[result], result=not_suppressed))
            finally:
                if cleanup is not None:
                    self.try_end_labels.pop()
            if cleanup is not None:
                assert done is not None
                if exception is not None:
                    error = (
                        self._consume_scratch_cell(exception)
                        if isinstance(exception, ScratchCell)
                        else exception
                    )
                self.emit(
                    MoltOp(kind="TRY_END", args=[cleanup], result=MoltValue("none"))
                )
                self.emit(
                    MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none"))
                )
                if exception is not None:
                    self.emit(
                        MoltOp(
                            kind="IF", args=[not_suppressed], result=MoltValue("none")
                        )
                    )
                    self.emit(
                        MoltOp(kind="RAISE", args=[error], result=MoltValue("none"))
                    )
                    self._emit_raise_if_pending()
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                self.emit(MoltOp(kind="JUMP", args=[done], result=MoltValue("none")))
                self.emit(
                    MoltOp(kind="LABEL", args=[cleanup], result=MoltValue("none"))
                )
                self.emit(
                    MoltOp(kind="TRY_END", args=[cleanup], result=MoltValue("none"))
                )
                if isinstance(exception, ScratchCell):
                    self._clear_scratch_cell(exception)
                if abandon_on_error is not None:
                    self._clear_scratch_cell(abandon_on_error)
                self.emit(
                    MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none"))
                )
                self._emit_raise_if_pending()
                self.emit(MoltOp(kind="LABEL", args=[done], result=MoltValue("none")))
        finally:
            self.try_suppress_depth = prior_suppress

    def _emit_control_flow_scope_unwind(
        self,
        scopes: Sequence[TryScope],
        *,
        pending_return: ScratchCell | None = None,
    ) -> list[int]:
        unwind_scopes = list(scopes)
        if not unwind_scopes:
            return []
        popped_labels: list[int] = []
        prior_scopes = self.try_scopes
        prior_active = self.active_exceptions
        self.try_scopes = list(prior_scopes)
        self.active_exceptions = list(prior_active)
        try:
            for scope in reversed(unwind_scopes):
                if not self.try_scopes or self.try_scopes[-1] is not scope:
                    raise AssertionError(
                        "control-flow unwind must consume a scope suffix"
                    )
                # A nested manager must finish while its enclosing handler's
                # exception and target binding remain visible to callbacks.
                for entry in reversed(self.active_exceptions):
                    if entry.is_handler and entry.scope is scope:
                        self._emit_exception_handler_exit_cleanup(entry)
                if scope.handler_label in self.try_end_labels:
                    if self.try_end_labels[-1] != scope.handler_label:
                        raise AssertionError(
                            "control-flow unwind tried to pop handler label "
                            f"{scope.handler_label}, active labels={self.try_end_labels}"
                        )
                    popped_labels.append(self.try_end_labels.pop())
                self.try_scopes.pop()
                self.active_exceptions = [
                    entry
                    for entry in self.active_exceptions
                    if entry.scope is not scope
                ]
                if scope.context_exit is not None:
                    self.emit(
                        MoltOp(
                            kind="TRY_END",
                            args=[scope.handler_label],
                            result=MoltValue("none"),
                        )
                    )
                    self._emit_context_exit(
                        scope.context_exit, abandon_on_error=pending_return
                    )
                else:
                    self.emit(
                        MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none"))
                    )
                    if scope.abandon_on_unwind is not None:
                        self._clear_scratch_cell(scope.abandon_on_unwind)
                    self._emit_raise_if_pending()
                if scope.finalbody and not scope.finalbody_running:
                    self._emit_finalbody(scope, pending_return=pending_return)
                    self._emit_raise_if_pending()
        finally:
            self.try_scopes = prior_scopes
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
        self._emit_normal_return_terminator(none_val)

    def _emit_raise_if_pending(self) -> None:
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
        body_terminated = self._visit_block(node.body)
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
                scope=scope,
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
            self._emit_guarded_body(handler.body)
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
            self._emit_guarded_body(node.orelse)
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
        self._emit_raise_if_pending()
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
