"""CallModuleDispatchMixin: extracted call-lowering authority."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    GEN_CONTROL_SIZE,
    INTRINSIC_HANDLE_CLASS_CONSTRUCTORS,
    MOLT_DIRECT_CALLS,
    MOLT_REEXPORT_FUNCTIONS,
    MoltOp,
    MoltValue,
    STDLIB_DIRECT_CALL_MODULES,
    _intrinsic_arity_exact,
)
from molt.frontend.sema import (
    FunctionKind,
    normalize_function_kind,
    parse_stateful_function_type_hint,
    stateful_function_frame_plan,
    stateful_function_result_type_hint,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallModuleDispatchMixin(_MixinBase):
    @staticmethod
    def _is_internal_module(module_name: str | None) -> bool:
        if not module_name:
            return False
        if module_name == "molt.stdlib" or module_name.startswith("molt.stdlib."):
            return False
        return module_name == "molt" or module_name.startswith("molt.")

    @staticmethod
    def _display_allowlist_module(module_name: str) -> str:
        if module_name in STDLIB_DIRECT_CALL_MODULES:
            return f"molt.stdlib.{module_name}"
        return module_name

    def _call_allowlist_suggestion(
        self, func_id: str, imported_from: str | None
    ) -> str | None:
        if imported_from == "molt":
            target_module = MOLT_REEXPORT_FUNCTIONS.get(func_id)
            if target_module:
                return f"{target_module}.{func_id}"
        if imported_from:
            normalized = self._normalize_allowlist_module(imported_from)
            if (
                normalized
                and normalized in MOLT_DIRECT_CALLS
                and func_id in MOLT_DIRECT_CALLS[normalized]
            ):
                display_module = self._display_allowlist_module(normalized)
                return f"{display_module}.{func_id}"
            if (
                imported_from in MOLT_DIRECT_CALLS
                and func_id in MOLT_DIRECT_CALLS[imported_from]
            ):
                display_module = self._display_allowlist_module(imported_from)
                return f"{display_module}.{func_id}"
        return None

    @staticmethod
    def _known_module_func_kind(info: dict[str, Any] | None) -> FunctionKind | None:
        if info is None:
            return None
        kind = normalize_function_kind(info.get("kind"))
        if kind in {
            FunctionKind.ASYNC,
            FunctionKind.ASYNC_GENERATOR,
            FunctionKind.GENERATOR,
        }:
            return kind
        return None

    def _emit_call_bind_for_known_module_func(
        self,
        node: ast.Call,
        *,
        result_hint: str,
    ) -> MoltValue:
        callee = self.visit(node.func)
        if callee is None:
            raise NotImplementedError("Unsupported call target")
        callargs = self._emit_call_args_builder(node)
        res = MoltValue(self.next_var(), type_hint=result_hint)
        self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
        return res

    def _native_callable_export(
        self,
        target_module: str,
        attr_name: str,
    ) -> dict[str, Any] | None:
        qualified_name = f"{target_module}.{attr_name}"
        spec = self.native_callable_exports.get(qualified_name)
        if isinstance(spec, dict):
            return spec
        return None

    def _try_emit_native_callable_export_call(
        self,
        target_module: str,
        attr_name: str,
        node: ast.Call,
    ) -> MoltValue | None:
        spec = self._native_callable_export(target_module, attr_name)
        if spec is None:
            return None

        qualified_name = f"{target_module}.{attr_name}"
        binding = spec.get("binding")
        abi = spec.get("abi")
        symbol = spec.get("symbol")
        if binding not in {"module_attr", "direct_symbol"} or not isinstance(abi, str):
            raise self.compat.unsupported(
                node,
                f"native callable export '{qualified_name}' has incomplete ABI metadata",
                impact="high",
                alternative="declare binding and abi in the native artifact manifest",
                detail="native callable exports must fail closed before lowering",
            )
        if binding == "direct_symbol" and not isinstance(symbol, str):
            raise self.compat.unsupported(
                node,
                f"native callable export '{qualified_name}' is missing a direct symbol",
                impact="high",
                alternative="declare symbol for direct_symbol native exports",
                detail="direct native symbols cannot be invented as Python call targets",
            )
        if node.keywords or any(isinstance(arg, ast.Starred) for arg in node.args):
            raise self.compat.unsupported(
                node,
                f"native callable export '{qualified_name}' with dynamic call arguments",
                impact="high",
                alternative="call the export with positional arguments supported by its ABI",
                detail="native callable ABI dispatch does not lower keyword, *args, or **kwargs packing",
            )

        callee = self.visit(node.func)
        if callee is None:
            raise NotImplementedError("Unsupported native callable target")
        args = self._emit_call_args(node.args)
        res = MoltValue(self.next_var(), type_hint="Any")
        metadata: dict[str, Any] = {
            "native_callable_export": qualified_name,
            "native_callable_binding": binding,
            "native_callable_abi": abi,
        }
        if isinstance(symbol, str):
            metadata["native_callable_symbol"] = symbol
        self.emit(
            MoltOp(
                kind="INVOKE_FFI",
                args=[callee] + args,
                result=res,
                metadata=metadata,
            )
        )
        return res

    def _emit_stateful_function_value_call(
        self,
        target_info: MoltValue | None,
        func_id: str,
        node: ast.Call,
        *,
        needs_bind: bool,
    ) -> MoltValue | None:
        if target_info is None:
            return None
        stateful_hint = parse_stateful_function_type_hint(target_info.type_hint)
        if stateful_hint is None:
            return None

        def target_value_for_call() -> MoltValue:
            if (
                self.current_func_name != "molt_main"
                and func_id not in self.locals
                and func_id not in self.async_locals
            ):
                return self._emit_module_attr_get(func_id)
            return target_info

        def emit_bind_call() -> MoltValue:
            callargs = self._emit_call_args_builder(node)
            res = MoltValue(self.next_var(), type_hint=stateful_hint.result_type_hint)
            self.emit(
                MoltOp(
                    kind="CALL_BIND",
                    args=[target_value_for_call(), callargs],
                    result=res,
                )
            )
            return res

        if needs_bind or stateful_hint.poll_symbol == self.current_func_name:
            return emit_bind_call()

        func_symbol = (
            stateful_hint.poll_symbol[: -len("_poll")]
            if stateful_hint.poll_symbol.endswith("_poll")
            else stateful_hint.poll_symbol
        )
        args, _ = self._emit_direct_call_args_for_symbol(func_symbol, node)
        if args is None:
            return emit_bind_call()

        if stateful_hint.has_closure and stateful_hint.kind != FunctionKind.GENERATOR:
            res = MoltValue(self.next_var(), type_hint=stateful_hint.result_type_hint)
            self.emit(
                MoltOp(
                    kind="CALL_FUNC",
                    args=[target_value_for_call()] + args,
                    result=res,
                )
            )
            return res

        task_args = args
        if stateful_hint.has_closure:
            closure_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="FUNCTION_CLOSURE_BITS",
                    args=[target_value_for_call()],
                    result=closure_val,
                )
            )
            task_args = [closure_val] + args
        frame_plan = stateful_hint.frame_plan(
            param_count=len(args),
            gen_control_size=GEN_CONTROL_SIZE,
        )
        closure_size = max(
            stateful_hint.closure_size,
            self._task_closure_size(
                frame_plan.payload_slots,
                include_gen_control=frame_plan.include_gen_control,
            ),
        )
        if stateful_hint.kind == FunctionKind.ASYNC:
            res = MoltValue(self.next_var(), type_hint=frame_plan.result_type_hint)
            self.emit(
                MoltOp(
                    kind="ALLOC_TASK",
                    args=[stateful_hint.poll_symbol, closure_size] + task_args,
                    result=res,
                    metadata={"task_kind": frame_plan.task_kind},
                )
            )
            return res
        gen_val = MoltValue(
            self.next_var(),
            type_hint=stateful_function_result_type_hint(FunctionKind.GENERATOR),
        )
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[stateful_hint.poll_symbol, closure_size] + task_args,
                result=gen_val,
                metadata={"task_kind": frame_plan.task_kind},
            )
        )
        if stateful_hint.kind == FunctionKind.GENERATOR:
            return gen_val
        res = MoltValue(self.next_var(), type_hint=frame_plan.result_type_hint)
        self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
        return res

    def _emit_known_module_task_func_call(
        self,
        target_module: str,
        func_id: str,
        node: ast.Call,
        *,
        needs_bind: bool,
    ) -> MoltValue | None:
        info = self._lookup_func_defaults(target_module, func_id)
        kind = self._known_module_func_kind(info)
        if kind is None:
            raw_kind = self._lookup_func_kind(target_module, func_id)
            if raw_kind in {
                FunctionKind.ASYNC,
                FunctionKind.ASYNC_GENERATOR,
                FunctionKind.GENERATOR,
            }:
                kind = raw_kind
        if kind is None:
            return None
        result_hint = stateful_function_result_type_hint(kind)
        if info is None:
            if needs_bind or node.keywords:
                return self._emit_call_bind_for_known_module_func(
                    node,
                    result_hint=result_hint,
                )
            args = self._emit_call_args(node.args)
            params = len(args)
        else:
            decorated = bool(info.get("has_decorators"))
            if needs_bind or decorated or info.get("has_vararg"):
                bind_hint = "Any" if decorated else result_hint
                return self._emit_call_bind_for_known_module_func(
                    node,
                    result_hint=bind_hint,
                )
            else:
                params = info.get("params")
                if not isinstance(params, int):
                    return self._emit_call_bind_for_known_module_func(
                        node,
                        result_hint=result_hint,
                    )
                args = self._emit_direct_call_args(target_module, func_id, node)
                if args is None:
                    return self._emit_call_bind_for_known_module_func(
                        node,
                        result_hint=result_hint,
                    )
        poll_func = f"{self._sanitize_module_name(target_module)}__{func_id}_poll"
        frame_plan = stateful_function_frame_plan(
            kind=kind,
            poll_symbol=poll_func,
            param_count=params,
            has_closure=False,
            gen_control_size=GEN_CONTROL_SIZE,
        )
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        if kind == FunctionKind.ASYNC:
            res = MoltValue(self.next_var(), type_hint=frame_plan.result_type_hint)
            self.emit(
                MoltOp(
                    kind="ALLOC_TASK",
                    args=[poll_func, closure_size] + args,
                    result=res,
                    metadata={"task_kind": frame_plan.task_kind},
                )
            )
            return res
        gen_val = MoltValue(
            self.next_var(),
            type_hint=stateful_function_result_type_hint(FunctionKind.GENERATOR),
        )
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[poll_func, closure_size] + args,
                result=gen_val,
                metadata={"task_kind": frame_plan.task_kind},
            )
        )
        if kind == FunctionKind.GENERATOR:
            return gen_val
        res = MoltValue(self.next_var(), type_hint="async_generator")
        self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
        return res

    def _try_emit_imported_module_direct_or_task_call(
        self,
        target_module: str | None,
        original_attr: str,
        node: ast.Call,
        *,
        imported_from: str | None,
        normalized: str | None,
        needs_bind: bool,
        force_bind: bool,
        direct_registry_authorized: bool,
    ) -> MoltValue | None:
        if target_module is None:
            return None
        native_callable_export_call = self._try_emit_native_callable_export_call(
            target_module,
            original_attr,
            node,
        )
        if native_callable_export_call is not None:
            return native_callable_export_call
        target_kind = self._lookup_func_kind(target_module, original_attr)
        known_direct_target = self._lookup_func_defaults(target_module, original_attr)
        has_known_direct_target = known_direct_target is not None
        known_info_kind = self._known_module_func_kind(known_direct_target)
        has_known_task_target = (
            target_kind not in {None, FunctionKind.SYNC} or known_info_kind is not None
        )
        direct_target_is_linkable = self._is_linkable_module_function_symbol(
            target_module
        )
        allow_speculative_internal_direct = (
            not has_known_direct_target
            and target_kind in {None, FunctionKind.SYNC}
            and imported_from is not None
            and imported_from not in self.stdlib_allowlist
            and (normalized is None or normalized not in self.stdlib_allowlist)
            and (
                self._is_internal_module(imported_from)
                or self._is_known_project_module(imported_from)
            )
            and not force_bind
        )
        if (
            not direct_target_is_linkable
            or not self._imported_module_attr_is_stable(target_module, original_attr)
            or not (
                direct_registry_authorized
                or has_known_direct_target
                or has_known_task_target
                or allow_speculative_internal_direct
            )
        ):
            return None

        lowered_task_func = self._emit_known_module_task_func_call(
            target_module,
            original_attr,
            node,
            needs_bind=needs_bind or force_bind,
        )
        if lowered_task_func is not None:
            return lowered_task_func
        if needs_bind or force_bind or has_known_task_target:
            return self._emit_call_bind_for_known_module_func(
                node,
                result_hint="Any",
            )
        if allow_speculative_internal_direct and not has_known_direct_target:
            args = None if node.keywords else self._emit_call_args(node.args)
        else:
            args = self._emit_direct_call_args(target_module, original_attr, node)
        if args is None:
            return self._emit_call_bind_for_known_module_func(
                node,
                result_hint="Any",
            )
        res = MoltValue(self.next_var(), type_hint="Any")
        target_name = f"{self._sanitize_module_name(target_module)}__{original_attr}"
        self.emit(MoltOp(kind="CALL", args=[target_name] + args, result=res))
        return res

    def _try_emit_intrinsic_handle_class_constructor(
        self,
        target_module: str,
        attr_name: str,
        node: ast.Call,
    ) -> MoltValue | None:
        spec = INTRINSIC_HANDLE_CLASS_CONSTRUCTORS.get((target_module, attr_name))
        if spec is None:
            return None
        if node.keywords or any(isinstance(arg, ast.Starred) for arg in node.args):
            return None
        if len(node.args) > 1:
            return None

        runtime_args: list[MoltValue]
        if node.args:
            arg_hint = self._static_expr_type_hint_without_emitting(node.args[0])
            if arg_hint not in spec.iterable_types:
                return None
            intrinsic_name = spec.iterable_intrinsic
        else:
            intrinsic_name = spec.empty_intrinsic

        class_ref = self.visit(node.func)
        if class_ref is None:
            raise NotImplementedError("Unsupported intrinsic-backed class target")
        runtime_args = []
        if node.args:
            iterable = self.visit(node.args[0])
            if iterable is None:
                raise NotImplementedError(
                    "Unsupported intrinsic-backed class constructor argument"
                )
            runtime_args.append(iterable)

        intrinsic_func = self._emit_intrinsic_function(intrinsic_name)
        handle = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[intrinsic_func] + runtime_args,
                result=handle,
            )
        )
        res = MoltValue(self.next_var(), type_hint=spec.type_hint)
        self.emit(MoltOp(kind="OBJECT_NEW_BOUND", args=[class_ref], result=res))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[res, spec.handle_attr, handle],
                result=MoltValue("none"),
            )
        )
        return res

    def _try_lower_intrinsic_lookup_call(
        self,
        *,
        func_id: str,
        imported_from: str | None,
        node: ast.Call,
    ) -> MoltValue | None:
        if func_id not in {"require_intrinsic", "_require_intrinsic"}:
            return None
        if not self._is_intrinsics_module_name(imported_from):
            return None
        if len(node.args) > 2 or any(kw.arg is None for kw in node.keywords):
            return None
        name_expr: ast.expr | None = node.args[0] if node.args else None
        namespace_expr: ast.expr | None = node.args[1] if len(node.args) == 2 else None
        name_kw = next((kw for kw in node.keywords if kw.arg == "name"), None)
        if name_kw is not None:
            name_expr = name_kw.value
        if name_expr is None:
            return None
        runtime_name = self._try_extract_const_str(name_expr)
        if runtime_name is None:
            return None
        if any(kw.arg not in {"name", "namespace"} for kw in node.keywords):
            return None
        namespace_kw = next((kw for kw in node.keywords if kw.arg == "namespace"), None)
        if namespace_kw is not None:
            namespace_expr = namespace_kw.value
        arity = _intrinsic_arity_exact(runtime_name)
        if arity is None:
            return None
        if namespace_expr is not None and not self._is_safe_intrinsic_namespace_expr(
            namespace_expr
        ):
            return None
        return self._emit_intrinsic_function(runtime_name)

    def _try_lower_local_intrinsic_wrapper_call(
        self, *, func_id: str, node: ast.Call
    ) -> MoltValue | None:
        if func_id not in self.local_intrinsic_wrappers:
            return None
        if (
            not node.args
            or len(node.args) > 2
            or any(kw.arg is None for kw in node.keywords)
        ):
            return None
        runtime_name: str | None = None
        if node.args:
            runtime_name = self._try_extract_const_str(node.args[0])
            if runtime_name is None:
                return None
        if len(node.args) == 2 and not self._is_safe_intrinsic_namespace_expr(
            node.args[1]
        ):
            return None
        name_kw = next((kw for kw in node.keywords if kw.arg == "name"), None)
        if name_kw is not None:
            runtime_name = self._try_extract_const_str(name_kw.value)
            if runtime_name is None:
                return None
        namespace_kw = next((kw for kw in node.keywords if kw.arg == "namespace"), None)
        if namespace_kw is not None and not self._is_safe_intrinsic_namespace_expr(
            namespace_kw.value
        ):
            return None
        if runtime_name is None:
            return None
        if any(kw.arg not in {"name", "namespace"} for kw in node.keywords):
            return None
        arity = _intrinsic_arity_exact(runtime_name)
        if arity is None:
            return None
        return self._emit_intrinsic_function(runtime_name)

    def _emit_loop_static_class_ref(self, class_name: str) -> MoltValue | None:
        for refs, eager_refs in zip(
            reversed(self.loop_static_class_refs),
            reversed(self.loop_static_class_eager_refs),
            strict=True,
        ):
            slot = refs.get(class_name)
            if slot is None:
                continue
            cached = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="LOAD_VAR",
                    args=[],
                    result=cached,
                    metadata={"var": slot.name},
                )
            )
            if class_name in eager_refs:
                return cached
            missing = MoltValue(self.next_var(), type_hint="missing")
            self.emit(MoltOp(kind="MISSING", args=[], result=missing))
            is_missing = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[cached, missing], result=is_missing))
            result = MoltValue(self.next_var(), type_hint="type")
            placeholder = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
            self.emit(MoltOp(kind="COPY", args=[placeholder], result=result))
            self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
            resolved = self._emit_module_attr_get(class_name)
            self.emit(
                MoltOp(
                    kind="STORE_VAR",
                    args=[resolved],
                    result=MoltValue("none"),
                    metadata={"var": slot.name},
                )
            )
            self.emit(MoltOp(kind="COPY", args=[resolved], result=result))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="COPY", args=[cached], result=result))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return result
        return None

    def _local_name_shadows_import_binding(self, name: str) -> bool:
        if self.current_func_name == "molt_main":
            return False
        if name in getattr(self, "local_imported_names", set()) or name in getattr(
            self, "local_imported_modules", set()
        ):
            return False
        if name in self.global_decls:
            return False
        return name in self.locals or name in self.boxed_locals

    def _literal_importlib_import_module_target(self, node: ast.Call) -> str | None:
        if node.keywords or len(node.args) != 1:
            return None
        arg = node.args[0]
        if not isinstance(arg, ast.Constant) or not isinstance(arg.value, str):
            return None
        module_name = arg.value
        if not module_name or module_name.startswith("."):
            return None

        if isinstance(node.func, ast.Attribute):
            if node.func.attr != "import_module" or not isinstance(
                node.func.value, ast.Name
            ):
                return None
            binding_name = node.func.value.id
            if self._local_name_shadows_import_binding(binding_name):
                return None
            if self._imported_module_binding_target(binding_name) != "importlib":
                return None
            if not self._imported_module_attr_is_stable("importlib", "import_module"):
                return None
        elif isinstance(node.func, ast.Name):
            binding_name = node.func.id
            if self._local_name_shadows_import_binding(binding_name):
                return None
            imported_from = self.imported_names.get(binding_name)
            if imported_from is None:
                imported_from = self.global_imported_names.get(binding_name)
            if imported_from != "importlib":
                return None
            original_attr = self._imported_attr_name(binding_name)
            if original_attr != "import_module":
                return None
            if not self._imported_module_attr_is_stable("importlib", "import_module"):
                return None
        else:
            return None

        return module_name

    def _try_emit_importlib_import_module_literal_call(
        self, node: ast.Call
    ) -> MoltValue | None:
        module_name = self._literal_importlib_import_module_target(node)
        if module_name is None:
            return None
        if (
            module_name in self.known_modules
            or self._should_attempt_runtime_module_import(module_name)
        ):
            return self._emit_importlib_import_module_leaf(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=name_val))
        package_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=package_val))
        res = MoltValue(self.next_var(), type_hint="module")
        self.emit(
            MoltOp(
                kind="CALL",
                args=["importlib__import_module", name_val, package_val],
                result=res,
            )
        )
        return res
