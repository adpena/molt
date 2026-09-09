"""CallMethodDispatchMixin: extracted call-lowering authority."""

from __future__ import annotations

import ast
from typing import (
    TYPE_CHECKING,
)

from molt.compiler_analysis.python_inlining import (
    inline_expression_is_frame_independent,
)
from molt.frontend._types import (
    BUILTIN_TYPE_TAGS,
    ClassInfo,
    MethodInfo,
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallMethodDispatchMixin(_MixinBase):
    def _class_resolves_default_object_new(
        self, class_name: str, class_info: ClassInfo
    ) -> bool:
        if class_info.get("dynamic"):
            return False
        for base_name in self._class_mro_names(class_name):
            if base_name == "object":
                return True
            if base_name in BUILTIN_TYPE_TAGS:
                return False
            base_info = self.classes.get(base_name)
            if base_info is None or base_info.get("dynamic"):
                return False
            methods = base_info.get("methods", {})
            class_attrs = base_info.get("class_attrs", {})
            pending = base_info.get("pending_methods")
            if (
                "__new__" in methods
                or "__new__" in class_attrs
                or (pending and "__new__" in pending)
            ):
                return False
        return False

    def _class_new_policy(
        self, class_name: str, class_info: ClassInfo
    ) -> tuple[bool, bool]:
        if self._class_resolves_default_object_new(class_name, class_info):
            return False, False
        if class_info.get("dynamic"):
            return True, True
        for base_name in self._class_mro_names(class_name):
            if base_name == "object":
                continue
            if base_name in BUILTIN_TYPE_TAGS:
                return True, False
            base_info = self.classes.get(base_name)
            if base_info is None:
                return True, True
            methods = base_info.get("methods", {})
            class_attrs = base_info.get("class_attrs", {})
            pending = base_info.get("pending_methods")
            if (
                "__new__" in methods
                or "__new__" in class_attrs
                or (pending and "__new__" in pending)
            ):
                return True, True
        return False, False

    def _has_exact_builtin_receiver(
        self, node: ast.AST, receiver: MoltValue, expected_type: str
    ) -> bool:
        if receiver.type_hint != expected_type:
            return False
        exact_from_expr = self._builtin_exact_type_from_expr(node)
        if exact_from_expr == expected_type:
            return True
        if isinstance(node, ast.Name):
            return self.exact_builtin_locals.get(node.id) == expected_type
        return False

    def _load_local_value_unchecked(self, name: str) -> MoltValue | None:
        if name in self.comp_shadow_locals:
            return self._load_local_value(name, guard_unbound=False)
        if self.current_func_name != "molt_main" and name in self.global_decls:
            return None
        cell = self._load_boxed_cell(name)
        if cell is not None:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            res = MoltValue(self.next_var())
            hint = self.boxed_local_hints.get(name)
            if hint is not None:
                res.type_hint = hint
            self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=res))
            self._copy_container_hints_for_name_load(name, res.name)
            return res
        if self.is_async() and (
            name in self.async_locals or name in self.async_internal_bindings
        ):
            offset = self._async_binding_slot(name).offset
            res = MoltValue(self.next_var(), type_hint=self._async_binding_hint(name))
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
            return res
        cached = self.locals.get(name)
        if cached is None:
            return None
        # Emit explicit load_var for non-boxed function locals (no unbound
        # guard in the unchecked variant).
        if (
            self.current_func_name != "molt_main"
            and not self.is_async()
            and name in self.scope_assigned
            and name not in self.boxed_locals
        ):
            res = MoltValue(self.next_var(), type_hint=cached.type_hint)
            self.emit(
                MoltOp(
                    kind="LOAD_VAR",
                    args=[],
                    result=res,
                    metadata={"var": name},
                )
            )
            self._copy_container_hints_for_name_load(name, res.name)
            return res
        return cached

    def _maybe_spill_receiver(
        self, receiver: MoltValue, args: list[ast.expr]
    ) -> tuple[MoltValue, int | None]:
        if not self.is_async() or not args:
            return receiver, None
        if not any(self._expr_may_yield(arg) for arg in args):
            return receiver, None
        slot = self._spill_async_value(receiver)
        return receiver, slot

    def _emit_call_args(self, args: list[ast.expr]) -> list[MoltValue]:
        if not args:
            return []
        if not self.is_async():
            values: list[MoltValue] = []
            for expr in args:
                arg = self.visit(expr)
                if arg is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE, "Unsupported call argument"
                    )
                values.append(arg)
            return values
        yield_flags = [self._expr_may_yield(expr) for expr in args]
        if not any(yield_flags):
            values = []
            for expr in args:
                arg = self.visit(expr)
                if arg is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE, "Unsupported call argument"
                    )
                values.append(arg)
            return values
        values = []
        spills: list[tuple[int, int, str]] = []
        for idx, expr in enumerate(args):
            arg = self.visit(expr)
            if arg is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported call argument"
                )
            values.append(arg)
            if any(yield_flags[idx + 1 :]):
                slot = self._spill_async_value(arg)
                spills.append((idx, slot, arg.type_hint))
        for idx, slot, hint in spills:
            values[idx] = self._reload_async_value(slot, hint)
        return values

    def _try_emit_static_dataclass_constructor(
        self,
        node: ast.Call,
        class_id: str,
        class_info: ClassInfo,
        class_ref: MoltValue,
    ) -> MoltValue | None:
        dataclass_params = class_info.get("dataclass_params", {})
        field_order = class_info.get("field_order", [])
        methods = class_info.get("methods", {})
        if not isinstance(dataclass_params, dict) or not isinstance(field_order, list):
            return None
        if (
            class_info.get("dynamic")
            or class_info.get("slots")
            or class_info.get("custom_metaclass")
            or class_info.get("decorated")
            or class_info.get("class_attrs")
            or not dataclass_params.get("init", True)
            or dataclass_params.get("kw_only", False)
            or methods.get("__init__") is not None
            or methods.get("__post_init__") is not None
            or methods.get("__setattr__") is not None
            or methods.get("__getattribute__") is not None
            or node.keywords
            or any(isinstance(arg, ast.Starred) for arg in node.args)
            or len(node.args) != len(field_order)
        ):
            return None

        values = self._emit_call_args(list(node.args))
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[class_id], result=name_val))
        field_name_vals: list[MoltValue] = []
        for field in field_order:
            field_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[field], result=field_val))
            field_name_vals.append(field_val)
        field_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(
            MoltOp(kind="TUPLE_NEW", args=field_name_vals, result=field_names_tuple)
        )
        flags = 0
        if class_info.get("frozen"):
            flags |= 0x1
        if class_info.get("eq"):
            flags |= 0x2
        if class_info.get("repr"):
            flags |= 0x4
        if class_info.get("slots"):
            flags |= 0x8
        flags_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[flags], result=flags_val))
        res = MoltValue(self.next_var(), type_hint=class_id)
        self.emit(
            MoltOp(
                kind="DATACLASS_NEW_VALUES",
                args=[name_val, field_names_tuple, flags_val] + values,
                result=res,
            )
        )
        self.emit(
            MoltOp(
                kind="DATACLASS_SET_CLASS",
                args=[res, class_ref],
                result=MoltValue("none"),
            )
        )
        return res

    def _emit_inline_expression(
        self, expression: ast.expr, bindings: dict[str, MoltValue]
    ) -> MoltValue:
        """Lower a preflighted expression without borrowing the caller's scope."""
        if isinstance(expression, ast.Name):
            return bindings[expression.id]
        if isinstance(expression, ast.Constant):
            value = self.visit(expression)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.INTERNAL_INVARIANT, "Inline constant produced no value"
                )
            return value
        if isinstance(expression, (ast.Tuple, ast.List)):
            values = [
                self._emit_inline_expression(element, bindings)
                for element in expression.elts
            ]
            is_tuple = isinstance(expression, ast.Tuple)
            result = MoltValue(
                self.next_var(), type_hint="tuple" if is_tuple else "list"
            )
            self.emit(
                MoltOp(
                    kind="TUPLE_NEW" if is_tuple else "LIST_NEW",
                    args=values,
                    result=result,
                )
            )
            return result
        raise FrontendRejection(
            Diagnostic.INTERNAL_INVARIANT, "Unproved inline expression reached lowering"
        )

    def _try_inline_method_call(
        self,
        method_info: MethodInfo,
        receiver: MoltValue,
        call_args: list[MoltValue],
    ) -> MoltValue | None:
        """Validate the whole frame-elision proof before emitting any operation."""
        expression = method_info.get("inline_return")
        parameters = method_info.get("inline_params")
        if (
            expression is None
            or parameters is None
            or method_info.get("has_closure")
            or len(parameters) != 1 + len(call_args)
            or not inline_expression_is_frame_independent(expression, parameters)
        ):
            return None
        bindings = dict(zip(parameters, [receiver, *call_args], strict=True))
        return self._emit_inline_expression(expression, bindings)

    def _try_inline_init_assigns(
        self,
        init_assigns: list[tuple[str, ast.expr]],
        inline_params: list[str],
        receiver: MoltValue,
        call_args: list[MoltValue],
    ) -> bool:
        """Preflight every value and store, then emit in Python source order."""
        if len(inline_params) != 1 + len(call_args):
            return False
        class_name = receiver.type_hint
        if class_name is None or class_name not in self.classes:
            return False
        class_info = self.classes[class_name]
        if (
            class_info.get("dynamic")
            or class_info.get("dataclass")
            or class_info.get("custom_metaclass")
            or class_info.get("decorated")
        ):
            return False
        for owner in self._class_mro_names(class_name):
            if owner == "object":
                continue
            owner_info = self.classes.get(owner)
            if owner_info is None or owner_info.get("dynamic"):
                return False
            if (
                "__setattr__" in owner_info.get("methods", {})
                or "__setattr__" in owner_info.get("class_attrs", {})
                or "__setattr__" in (owner_info.get("pending_methods") or ())
            ):
                return False
        fields = class_info.get("fields", {})
        seen: set[str] = set()
        for name, expression in init_assigns:
            if (
                name in seen
                or name not in fields
                or self._class_attr_is_data_descriptor(class_name, name)
                or not inline_expression_is_frame_independent(expression, inline_params)
            ):
                return False
            seen.add(name)
        bindings = dict(zip(inline_params, [receiver, *call_args], strict=True))
        for name, expression in init_assigns:
            value = self._emit_inline_expression(expression, bindings)
            self._emit_guarded_setattr(
                receiver, name, value, class_name, use_init=True, assume_exact=True
            )
        return True

    def _try_emit_user_method_static_call(self, node: ast.Call) -> "MoltValue | None":
        """Phase 1 (frontend variant) — direct call for monomorphic user methods.

        Pattern: ``obj.method(args)`` where ``obj`` is a local with a
        statically-known concrete class registered in ``exact_locals``,
        the class is non-dynamic and non-dataclass, and ``method`` is a
        regular function descriptor on that class with a clean signature
        (no closure / vararg / varkw / kwonly / defaults).

        Bypasses the bound-method allocation that the general dispatch
        path performs at every call site.  In a tight loop this saves
        N heap allocations + N IC dispatches (the allocation is the
        dominant cost on bench_class_hierarchy, ~4.5s of the 5s
        single-class-call overhead measured experimentally).

        Bails out on any condition that would change the observable
        binding semantics (descriptors, properties, dataclass, dynamic
        class layout, kwargs, *args spread, default-spec evaluation).

        Returns ``None`` on bail to signal the caller to fall through to
        the general path.
        """
        if not isinstance(node.func, ast.Attribute):
            return None
        attr_node = node.func
        if not isinstance(attr_node.value, ast.Name):
            return None
        obj_name = attr_node.value.id
        class_name = self.exact_locals.get(obj_name)
        if class_name is None:
            return None
        class_info = self.classes.get(class_name)
        if class_info is None:
            return None
        # Conservative bail-outs: anything that touches the descriptor /
        # attribute-resolution machinery other than a vanilla bound-method
        # binding.
        if class_info.get("dynamic"):
            return None
        if class_info.get("dataclass"):
            return None
        if class_info.get("metaclass"):
            return None
        method_name = attr_node.attr
        method_info, owner_class = self._resolve_method_info(class_name, method_name)
        if method_info is None or owner_class is None:
            return None
        if method_info.get("descriptor") != "function":
            return None
        # Closures require their real lexical cell transport and executing frame.
        target_is_closure = bool(method_info.get("has_closure"))
        if target_is_closure:
            return None
        if method_info.get("has_vararg"):
            return None
        if method_info.get("has_varkw"):
            return None
        # Honour __getattribute__ overrides: the runtime path goes
        # through the override and could observe the bound-method
        # construction.  Skip the fold for those.
        getattribute_info, _ = self._resolve_method_info(class_name, "__getattribute__")
        if getattribute_info is not None:
            return None
        # Same for __getattr__ (only fires when normal lookup misses,
        # but a fold that bypasses the BoundMethod allocation could
        # observably skip the lookup ordering).
        getattr_info, _ = self._resolve_method_info(class_name, "__getattr__")
        if getattr_info is not None:
            return None
        if node.keywords:
            return None
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                return None
        param_count = method_info.get("param_count")
        if param_count is None:
            return None
        if (
            method_info.get("defaults")
            or method_info.get("kwonly_count")
            or len(node.args) != param_count - 1
        ):
            return None
        function = method_info.get("func")
        if function is None or not function.type_hint.startswith("Func:"):
            return None
        method_symbol = function.type_hint.split(":", 1)[1]
        if method_symbol not in self.func_symbol_names:
            return None

        # No eligibility fallback is permitted after evaluating Python operands.
        receiver = self.visit(attr_node.value)
        if receiver is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "Unsupported method receiver"
            )
        call_args = self._emit_call_args(list(node.args))
        inlined = self._try_inline_method_call(method_info, receiver, call_args)
        if inlined is not None:
            return inlined

        res_hint = "Any"
        return_hint = method_info.get("return_hint")
        if return_hint and (
            return_hint in self.classes or return_hint in BUILTIN_TYPE_TAGS
        ):
            res_hint = return_hint
        res = MoltValue(self.next_var(), type_hint=res_hint)
        self.emit(
            MoltOp(
                kind="CALL",
                args=[method_symbol, receiver] + call_args,
                result=res,
            )
        )
        return res
