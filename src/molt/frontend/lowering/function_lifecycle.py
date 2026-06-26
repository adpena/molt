"""FunctionLifecycleMixin: function frame setup, state, returns, and exits.

Move-only extraction from frontend/__init__.py. These helpers own per-function
lifecycle state: locals-call scans, function entry reset, nested-function state
capture/restore, async return slots, return lowering, and function exception
exit emission shared by function, async, module, and control-flow visitors.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Any

from molt.frontend._types import GEN_CONTROL_SIZE, FuncInfo, MoltOp, MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class FunctionLifecycleMixin(_MixinBase):
    def _task_closure_size(
        self, payload_slots: int, *, include_gen_control: bool
    ) -> int:
        base = self.async_locals_base + len(self.async_locals) * 8
        required = payload_slots * 8
        if include_gen_control:
            required += GEN_CONTROL_SIZE
        if base < required:
            return required
        return base

    @staticmethod
    def _function_contains_locals_call(
        node: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if (
                isinstance(current, ast.Call)
                and isinstance(current.func, ast.Name)
                and current.func.id == "locals"
            ):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _expr_contains_locals_call(node: ast.AST) -> bool:
        stack: list[ast.AST] = [node]
        while stack:
            current = stack.pop()
            if (
                isinstance(current, ast.Call)
                and isinstance(current.func, ast.Name)
                and current.func.id == "locals"
            ):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _function_contains_return(node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, ast.Return):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _body_has_exception_handlers(body: list[ast.stmt]) -> bool:
        """Return True if the body contains try/with handler constructs.

        This gates ONLY the exception-STACK depth bookkeeping
        (EXCEPTION_STACK_ENTER/DEPTH/SET_DEPTH/EXIT), which is needed solely
        when the function pushes/pops the runtime exception-handler stack — i.e.
        it contains ``try``/``with`` (and their async/star variants).

        It does NOT gate exception OBSERVATION: every function unconditionally
        carries a function-level exception label and the per-may-raise-op
        CHECK_EXCEPTION routing, so a raising callee's pending exception is
        always observed.  Decoupling these two concerns is the C2 fix — the old
        ``_function_needs_exception_stack`` conflated them and (by opting a
        function out of *observation*) caused silent-wrong exception
        propagation.  A bare ``raise`` does NOT require depth bookkeeping: it
        sets the pending flag and jumps to the function label, whose handler's
        depth-restore is a no-op when no handler stack was ever pushed.
        """
        stack: list[ast.AST] = list(body)
        while stack:
            current = stack.pop()
            if isinstance(current, (ast.Try, ast.TryStar, ast.With, ast.AsyncWith)):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _block_needs_context_unwind(body: list[ast.stmt]) -> bool:
        stack: list[ast.AST] = list(body)
        while stack:
            current = stack.pop()
            if isinstance(current, (ast.With, ast.AsyncWith)):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    def _has_typing_overload_decorator(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> bool:
        """Return True if the function has a @typing.overload or @overload decorator.

        Handles ``typing.overload``, bare ``overload``, and aliased forms
        like ``t.overload`` (from ``import typing as t``).
        """
        for deco in node.decorator_list:
            if isinstance(deco, ast.Attribute):
                if isinstance(deco.value, ast.Name) and deco.attr == "overload":
                    # Accept any <name>.overload where <name> resolves to
                    # the typing module — covers ``typing.overload``,
                    # ``t.overload``, etc.  We check a known set of names
                    # to avoid false positives with unrelated ``overload``
                    # attributes.
                    alias = deco.value.id
                    if alias == "typing" or alias in self._typing_import_aliases:
                        return True
            elif isinstance(deco, ast.Name) and deco.id == "overload":
                return True
        return False

    def start_function(
        self,
        name: str,
        params: list[str] | None = None,
        param_types: list[str] | None = None,
        type_facts_name: str | None = None,
        needs_return_slot: bool = False,
        has_exception_handlers: bool = True,
    ) -> None:
        if name not in self.funcs_map:
            self.funcs_map[name] = FuncInfo(
                params=params or [],
                param_types=param_types or [],
                return_hint=None,
                ops=self._new_tracked_ops(count_function=True),
            )
        else:
            self.funcs_map[name]["params"] = params or []
            self.funcs_map[name]["param_types"] = param_types or []
            self.funcs_map[name].setdefault("return_hint", None)
            self.funcs_map[name]["ops"].clear()
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]
        self._reset_local_binding_state(
            reset_locals_cache=True,
            reset_del_targets=True,
        )
        self._reset_import_resolution_state(reset_module_attr_mutations=True)
        self._reset_async_scope_state()
        self._reset_type_hint_scope_state(reset_bytearray_len=False)
        self._reset_function_cache_state()
        self._reset_control_flow_state(reset_function_exception_label=False)
        # ── Exception model (C2): two decoupled concerns ──────────────────
        # 1. OBSERVATION — every function unconditionally carries a
        #    function-level exception label.  `emit()` auto-routes a pending
        #    exception to this label after every may-raise op (the redundant
        #    checks are removed later by the oracle-driven `check_exception_elim`
        #    TIR pass).  A raising callee sets the runtime exception-pending flag
        #    regardless of the caller's syntactic shape, so there is NO sound way
        #    to opt a function out of observation without re-opening the
        #    silent-wrong-propagation bug class (a lambda that calls
        #    `int("x")` returning None instead of raising).  Hence the label is
        #    ALWAYS created.
        #
        # 2. STACK-DEPTH bookkeeping (EXCEPTION_STACK_ENTER/DEPTH and the
        #    matching SET_DEPTH/EXIT at returns) — needed ONLY when the function
        #    pushes/pops the runtime exception-handler stack, i.e. it contains a
        #    `try`/`with` handler.  A function without handlers never changes the
        #    depth, so the ENTER/DEPTH baselines (and their per-return restore)
        #    are pure overhead.  Gating them on `has_exception_handlers` keeps a
        #    trivial leaf like `lambda x: x + 1` cheap (label + post-op check
        #    only) — the same cost CPython pays — while preserving full
        #    correctness for handler-bearing functions.
        self.function_exception_label = self.next_label()
        if has_exception_handlers:
            self.exception_stack_prev_baseline = MoltValue(
                self.next_var(), type_hint="int"
            )
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_ENTER",
                    args=[],
                    result=self.exception_stack_prev_baseline,
                )
            )
            self.exception_stack_depth_baseline = MoltValue(
                self.next_var(), type_hint="int"
            )
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_DEPTH",
                    args=[],
                    result=self.exception_stack_depth_baseline,
                )
            )
        else:
            self.exception_stack_prev_baseline = None
            self.exception_stack_depth_baseline = None
        if needs_return_slot:
            self._init_return_slot()
        self._apply_type_facts(type_facts_name or name)

    def _capture_function_state(self) -> dict[str, Any]:
        return self._capture_function_scope_state()

    def _restore_function_state(self, state: dict[str, Any]) -> None:
        self._restore_function_scope_state(state)

    def _init_return_slot(self) -> None:
        if self.return_label is not None:
            return
        if not self.is_async():
            return
        self.return_label = self.next_label()
        self.return_slot_index = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=self.return_slot_index))
        init = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=init))
        self.return_slot = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[init], result=self.return_slot))

    def _store_return_slot_for_stateful(self) -> None:
        if not self.is_async() or self.return_slot is None:
            return
        if self.return_slot_offset is None:
            self.return_slot_offset = self._async_local_offset("__molt_return_slot")
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", self.return_slot_offset, self.return_slot],
                result=MoltValue("none"),
            )
        )

    def _load_return_slot(self) -> MoltValue | None:
        if self.return_slot is None:
            return None
        if self.is_async() and self.return_slot_offset is not None:
            slot_val = MoltValue(self.next_var(), type_hint="list")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.return_slot_offset],
                    result=slot_val,
                )
            )
            return slot_val
        return self.return_slot

    def _load_return_slot_index(self) -> MoltValue:
        if self.is_async():
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            return idx
        idx = self.return_slot_index
        if idx is None:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.return_slot_index = idx
        return idx

    def _emit_return_value(self, value: MoltValue) -> None:
        exit_baseline_now = self.return_slot is None or self.return_label is None
        if exit_baseline_now:
            self._emit_plain_local_scope_exit_boundaries(preserve=value)
            if self.current_func_name != "molt_main":
                self._emit_boxed_locals_cleanup()
            self._emit_restore_exception_stack_depth(exit_baseline=True)
            if self._function_needs_frame_trace():
                self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ret", args=[value], result=MoltValue("none")))
            return
        self._emit_restore_exception_stack_depth(exit_baseline=False)
        slot = self._load_return_slot()
        if slot is None:
            self._emit_plain_local_scope_exit_boundaries(preserve=value)
            if self.current_func_name != "molt_main":
                self._emit_boxed_locals_cleanup()
            if self._function_needs_frame_trace():
                self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ret", args=[value], result=MoltValue("none")))
            return
        idx = self._load_return_slot_index()
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[slot, idx, value],
                result=MoltValue("none"),
            )
        )
        self._emit_plain_local_scope_exit_boundaries()
        if self.current_func_name != "molt_main":
            self._emit_boxed_locals_cleanup()
        self.emit(
            MoltOp(kind="JUMP", args=[self.return_label], result=MoltValue("none"))
        )

    def _emit_return_label(self) -> None:
        if self.return_label is None or self.return_slot is None:
            return
        self.emit(
            MoltOp(kind="LABEL", args=[self.return_label], result=MoltValue("none"))
        )
        self._emit_restore_exception_stack_depth()
        slot = self._load_return_slot()
        if slot is None:
            return
        res = MoltValue(self.next_var())
        idx = self._load_return_slot_index()
        self.emit(MoltOp(kind="INDEX", args=[slot, idx], result=res))
        if self._function_needs_frame_trace():
            self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))

    def _emit_boxed_locals_cleanup(self) -> None:
        if not self.boxed_locals:
            return
        skip = set(self.free_vars) | self.closure_locals
        for name, cell in self.boxed_locals.items():
            if name in skip:
                continue
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            missing = self._emit_missing_value()
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, idx, missing],
                    result=MoltValue("none"),
                )
            )

    def _emit_restore_exception_stack_depth(
        self, *, exit_baseline: bool = True
    ) -> None:
        baseline = self.exception_stack_depth_baseline
        if baseline is not None:
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_SET_DEPTH",
                    args=[baseline],
                    result=MoltValue("none"),
                )
            )
        if not exit_baseline:
            return
        prev_baseline = self.exception_stack_prev_baseline
        if prev_baseline is None:
            return
        self.emit(
            MoltOp(
                kind="EXCEPTION_STACK_EXIT",
                args=[prev_baseline],
                result=MoltValue("none"),
            )
        )

    def _emit_function_exception_handler(self, *, clear_handlers: bool = False) -> None:
        label = self.function_exception_label
        if label is None:
            return
        module_failure_cleanup = bool(
            self.module_name
            and (
                self.current_func_name == "molt_main"
                or self.current_func_name.startswith("molt_init_")
            )
        )
        if module_failure_cleanup and not self._ends_with_return_jump():
            self.emit(MoltOp(kind="ret_void", args=[], result=MoltValue("none")))
        prev_label = self.function_exception_label
        self.function_exception_label = None
        with self._suppress_check_exception(emit_on_exit=False):
            self.emit(MoltOp(kind="LABEL", args=[label], result=MoltValue("none")))
            if module_failure_cleanup:
                module_name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="CONST_STR",
                        args=[self.module_name],
                        result=module_name_val,
                    )
                )
                self.emit(
                    MoltOp(
                        kind="MODULE_CACHE_DEL",
                        args=[module_name_val],
                        result=MoltValue("none"),
                    )
                )
                if (
                    self.entry_module
                    and self.module_name == self.entry_module
                    and self.module_name != "__main__"
                ):
                    main_name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(
                            kind="CONST_STR", args=["__main__"], result=main_name_val
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="MODULE_CACHE_DEL",
                            args=[main_name_val],
                            result=MoltValue("none"),
                        )
                    )
        self._emit_restore_exception_stack_depth()
        if self._function_needs_frame_trace():
            self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True, clear_handlers=clear_handlers)
        self.function_exception_label = prev_label

    def _ends_with_return_jump(self) -> bool:
        if not self.current_ops:
            return False
        last = self.current_ops[-1]
        if last.kind in {"ret", "ret_void"}:
            return True
        if (
            last.kind == "JUMP"
            and self.return_label is not None
            and last.args
            and last.args[0] == self.return_label
        ):
            return True
        return False

    def resume_function(self, name: str) -> None:
        if self.current_func_name != name:
            self._emit_function_exception_handler()
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]
