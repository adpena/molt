"""CallRuntimeHelperMixin: extracted call-lowering authority."""

from __future__ import annotations

from molt.frontend.class_model import guarded_initializer_candidate
from molt.frontend.lowering.op_kinds_generated import (
    FrontendKinds,
)

import ast
from molt.frontend._mixin_base import GeneratorMixinBase
from molt.compiler_analysis.python_call_arguments import (
    CallForm,
    call_argument_plan,
    call_form,
)
from molt.frontend._types import (
    ClassId,
    MoltOp,
    MoltValue,
    TypeHint,
    loaded_method_self_name,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.exception_kinds_generated import BUILTIN_EXCEPTION_CODES


class CallRuntimeHelperMixin(GeneratorMixinBase):
    def _function_result_hint(self, func_symbol: str) -> str:
        info = self.funcs_map.get(func_symbol)
        hint = info.get("return_hint") if info is not None else None
        return hint or "Any"

    def _record_container_elem_hint(
        self, target: MoltValue, elem_hint: TypeHint | None
    ) -> None:
        elem_map = (
            self.global_elem_hints
            if self.current_func_name == "molt_main"
            else self.container_elem_hints
        )
        if elem_hint and elem_hint not in {"Any", "Unknown", "missing"}:
            elem_map[target.name] = elem_hint
        else:
            elem_map.pop(target.name, None)

    def _remember_bytearray_len_hint(
        self, value: MoltValue, length: int | None
    ) -> None:
        if length is not None and length >= 0:
            self.bytearray_len_hints[value.name] = length
        else:
            self.bytearray_len_hints.pop(value.name, None)

    def _emit_locals_dict(self) -> MoltValue:
        if self._class_ns_stack and self._class_ns_stack[-1].class_node is not None:
            namespace = self._class_ns_stack[-1].ns
            if namespace is None:
                raise FrontendRejection(
                    Diagnostic.INTERNAL_INVARIANT,
                    "Reflected class body has no namespace mapping",
                )
            return namespace
        if self.current_func_name == "molt_main":
            return self._emit_globals_dict()
        # A function's locals() is its frame's: the runtime reads the executing
        # frame's homes or task payload, so the op carries no operands.
        # `shared`: before PEP 667 an activation's locals() is one dict that
        # each call refreshes.
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(
            MoltOp(
                kind="FRAME_LOCALS",
                args=[],
                result=res,
                metadata={"shared": self.target_python < (3, 13)},
            )
        )
        return res

    def _emit_exception_new_from_class(
        self, class_val: MoltValue, args: list[MoltValue]
    ) -> MoltValue:
        args_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=args, result=args_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW_FROM_CLASS",
                args=[class_val, args_val],
                result=exc_val,
            )
        )
        return exc_val

    def _emit_type_error_value(self, message: str, type_hint: str = "Any") -> MoltValue:
        err_val = self._emit_exception_new("TypeError", message)
        self._emit_raise(err_val, origin=True)
        res = MoltValue(self.next_var(), type_hint=type_hint)
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
        return res

    def _emit_stop_iteration_from_value(self, value: MoltValue) -> None:
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, none_val], result=is_none))
        home = self._new_compiler_local("tuple")
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
        self._store_compiler_local(home, empty_tuple)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        value_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=[value], result=value_tuple))
        self._store_compiler_local(home, value_tuple)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        args_val = self._load_compiler_local(home)
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW_BUILTIN",
                args=[args_val],
                result=exc_val,
                metadata={
                    "exception_name": "StopIteration",
                    "exception_tag": BUILTIN_EXCEPTION_CODES["StopIteration"],
                },
            )
        )
        self._emit_raise(exc_val, origin=False)

    def _call_needs_bind(self, node: ast.Call) -> bool:
        if self._sema is None:
            raise AssertionError("module sema must be populated before lowering calls")
        return (
            bool(node.keywords)
            or call_form(node, module_imports=self._sema.import_names) == "expanded"
        )

    def _new_call_args_builder(self, form: CallForm) -> MoltValue:
        created = MoltValue(self.next_var(), type_hint="callargs")
        self.emit(
            MoltOp(
                kind="CALLARGS_NEW",
                args=[],
                result=created,
                metadata={"call_form": form},
            )
        )
        return created

    def _emit_call_args_builder(
        self,
        node: ast.Call,
        *,
        evaluated: tuple[MoltValue, ...] | None = None,
        initial: MoltValue | None = None,
    ) -> MoltValue:
        # Guarded dispatch shares argument assembly without revisiting source
        # expressions. Pre-evaluation is valid only for flat calls: expansions
        # have interleaved observable work owned by the argument plan.
        plan = call_argument_plan(node)
        if evaluated is not None and (
            len(evaluated) != len(plan.operands) or plan.has_expansion
        ):
            raise AssertionError("pre-evaluated call requires flat argument syntax")
        if self._sema is None:
            raise AssertionError("module sema must be populated before lowering calls")
        # The runtime cannot tell CALL from CALL_FUNCTION_EX by the values it
        # receives, and the two own their arguments differently while the
        # callee binds them. The builder carries the compiler's choice.
        form = call_form(node, module_imports=self._sema.import_names)

        # The builder is CPython's value stack for this call and comes into
        # existence at its first push. Operands still pending then (a named
        # keyword group, a deferred sole ``*x``) are older than it, so
        # exceptional cleanup, which releases newest first, unwinds the
        # builder's keyword mapping before them, as CPython unwinds its kwargs
        # dictionary before the operands beneath it.
        callargs = initial
        values: list[MoltValue] = []
        for step in plan.steps:
            if step.action == "evaluate":
                if evaluated is None:
                    value = self.visit(step.expression)
                    if value is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE, "Unsupported call argument"
                        )
                    values.append(value)
                continue
            value = (
                evaluated[step.index] if evaluated is not None else values[step.index]
            )
            if step.materialization == "tuple":
                # CALL_FUNCTION_EX consumes a tuple, unlike list accumulation
                # for mixed stars. The runtime tuple authority owns versioned
                # length-hint callbacks and exact-tuple identity preservation.
                value = self._emit_tuple_from_iter(value)
            if callargs is None:
                callargs = self._new_call_args_builder(form)
            args = [callargs, value]
            if step.action == "kw":
                if step.name is None:
                    raise AssertionError("keyword schedule has no name")
                key = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[step.name], result=key))
                args = [callargs, key, value]
            kind = {
                "pos": "CALLARGS_PUSH_POS",
                "star": "CALLARGS_EXPAND_STAR",
                "kw": "CALLARGS_PUSH_KW",
                "kwstar": "CALLARGS_EXPAND_KWSTAR",
            }[step.action]
            self.emit(
                MoltOp(
                    kind=kind,
                    args=args,
                    result=MoltValue(self.next_var(), type_hint="None"),
                )
            )
        return callargs if callargs is not None else self._new_call_args_builder(form)

    def _emit_tuple_from_iter(self, iterable: MoltValue) -> MoltValue:
        constructor = self._emit_builtin_type_value("tuple")
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="CALL_FUNC", args=[constructor, iterable], result=res))
        return res

    def _emit_frozenset_from_iter(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="frozenset")
        self.emit(MoltOp(kind="FROZENSET_NEW", args=[], result=res))
        iter_obj = self._emit_iter_new(iterable)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(
            MoltOp(kind="FROZENSET_ADD", args=[res, item], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return res

    def _builtin_str_single_object_arg(self, node: ast.AST) -> ast.expr | None:
        if not isinstance(node, ast.Call):
            return None
        if self._specializable_builtin_name(node) != "str":
            return None
        if len(node.args) + len(node.keywords) != 1:
            return None
        kw_object: ast.expr | None = None
        for keyword in node.keywords:
            if keyword.arg != "object":
                return None
            if kw_object is not None:
                return None
            kw_object = keyword.value
        if node.args:
            return node.args[0]
        return kw_object

    def _emit_attribute_method_call(
        self,
        node: ast.Call,
        attr_node: ast.Attribute,
        receiver: MoltValue,
        obj_name: str | None,
        exact_class: ClassId | None,
    ) -> MoltValue:
        """CPython's method-form LOAD_ATTR and CALL for ``receiver.attr(...)``.

        The loaded receiver is consumed before argument evaluation. Flat calls
        keep their direct loaded-method path; keywords and suspension use the
        same stack builder, which can park in a Python frame without carrying
        the nullable word. CALL_FUNCTION_EX keeps a real bound attribute.
        """
        if self._sema is None:
            raise AssertionError("module sema must be populated before lowering calls")
        method_form = call_form(node, module_imports=self._sema.import_names) == "stack"
        callee = self._emit_attribute_load(
            attr_node, receiver, obj_name, exact_class, method_load=method_form
        )
        load = self._op_by_result.get(callee.name)
        if load is None or load.kind != FrontendKinds.LOAD_METHOD:
            return self._emit_dynamic_call(node, callee)
        return self._emit_loaded_method_call(node, callee, load)

    def _emit_loaded_method_call(
        self, node: ast.Call, pair: MoltValue, load: MoltOp
    ) -> MoltValue:
        """Consume the loaded receiver before source arguments can suspend.

        The receiver slot may hold the null sentinel, which no frame slot can
        carry, so a call whose arguments suspend pushes it into the builder
        first. The method itself crosses the suspension as a value.
        """
        suspends = self.is_async() and any(
            self._expr_may_yield(expression)
            for expression in [
                *node.args,
                *(keyword.value for keyword in node.keywords),
            ]
        )
        if self._call_needs_bind(node) or suspends:
            builder = self._new_call_args_builder("stack")
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_METHOD_RECEIVER",
                    args=[builder],
                    result=MoltValue("none"),
                    metadata={"self_or_null": loaded_method_self_name(load)},
                )
            )
            builder = self._emit_call_args_builder(node, initial=builder)
            res = MoltValue(
                self.next_var(),
                type_hint=self._builtin_exact_type_from_expr(node) or "Any",
            )
            self.emit(MoltOp(kind="CALL_BIND", args=[pair, builder], result=res))
            return res
        args = self._emit_call_args(node.args)
        res = MoltValue(
            self.next_var(), type_hint=self._builtin_exact_type_from_expr(node) or "Any"
        )
        self.emit(
            MoltOp(
                kind="CALL_LOADED_METHOD",
                args=[pair, *args],
                result=res,
                metadata={"self_or_null": loaded_method_self_name(load)},
            )
        )
        return res

    def _emit_positional_call(
        self,
        node: ast.Call,
        callee: MoltValue,
        args: list[MoltValue],
        result: MoltValue,
        *,
        defines_del: bool = False,
    ) -> None:
        candidate = None
        if (
            isinstance(node.func, ast.Name)
            and self._class_index is not None
            and self.python_binding_index is not None
            and (fact := self.python_binding_index.expression_fact(node.func))
            is not None
            and len(fact.result.source_origins) == 1
        ):
            # An exposed read can still name the originating statement while
            # evaluating to a replacement. Only the runtime guard may identify
            # the actual captured callable or its installed initializer.
            (origin,) = fact.result.source_origins
            owner = self._class_index.definitions.get(origin)
            if owner is not None and (view := self.classes.get(owner)) is not None:
                candidate = guarded_initializer_candidate(view)
        metadata: dict[str, object] | None = (
            {"initializer_candidate": candidate} if candidate is not None else None
        )
        if defines_del:
            if metadata is None:
                metadata = {}
            metadata["defines_del"] = True
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[callee, *args],
                result=result,
                metadata=metadata,
            )
        )

    def _emit_dynamic_call(self, node: ast.Call, callee: MoltValue) -> MoltValue:
        # The result authority already proves exactness at this source point.
        # It is independent of whether the callee is a constructor, method or
        # open alias, and never authorizes eliding the retained real callable.
        res_hint = self._builtin_exact_type_from_expr(node) or "Any"
        if self._call_needs_bind(node):
            callargs = self._emit_call_args_builder(node)
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_INDIRECT", args=[callee, callargs], result=res))
            return res
        args = self._emit_call_args(node.args)
        callee_hint = callee.type_hint
        if isinstance(callee_hint, str) and callee_hint.startswith("Func:"):
            func_symbol = callee_hint.split(":", 1)[1]
            res = MoltValue(self.next_var(), type_hint=res_hint)
            # A code-symbol hint is not a namespace or callable-identity proof.
            # Reuse guarded object dispatch so its fast path also transports the
            # exact function context; a second globals lookup can be rebound too.
            self.emit(
                MoltOp(
                    kind="CALL_GUARDED",
                    args=[callee] + args,
                    result=res,
                    metadata={"target": func_symbol},
                )
            )
            return res
        # Flat stack-form object dispatch already owns callable admission
        # and live defaults, including bound-method self. Syntax and the
        # canonical stack-use threshold alone choose builder transport.
        res = MoltValue(self.next_var(), type_hint=res_hint)
        self._emit_positional_call(node, callee, args, res)
        return res
