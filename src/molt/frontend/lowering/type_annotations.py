"""TypeAnnotationMixin: type-hint propagation and annotation emission.

Move-only extraction from frontend/__init__.py. This shared lowering authority
owns annotation parsing/emission, type-parameter publication, explicit type-fact
application, container/dict/bytearray hint propagation, and runtime type guards.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Callable, Sequence

from molt.frontend._types import (
    _MOLT_CLOSURE_PARAM,
    MoltOp,
    MoltValue,
    normalize_type_hint,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class TypeAnnotationMixin(_MixinBase):
    def _apply_explicit_hint(self, name: str, value: MoltValue) -> None:
        hint = self.explicit_type_hints.get(name)
        if hint is None:
            return
        if self.type_hint_policy == "check":
            self._emit_guard_type(value, hint)
            self._apply_hint_to_value(name, value, hint)
            return
        if self.type_hint_policy == "trust" or self.stdlib_hint_trust:
            self._apply_hint_to_value(name, value, hint)

    def _module_has_future_annotations(self, node: ast.Module) -> bool:
        found = False

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                return

            def visit_ListComp(self, node: ast.ListComp) -> None:
                return

            def visit_SetComp(self, node: ast.SetComp) -> None:
                return

            def visit_DictComp(self, node: ast.DictComp) -> None:
                return

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                nonlocal found
                if node.module != "__future__":
                    return
                for alias in node.names:
                    if alias.name == "annotations":
                        found = True
                        return

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
            if found:
                break
        return found

    def _normalized_return_hint(self, returns: ast.expr | None) -> str | None:
        hint = self._annotation_to_hint(returns)
        if hint and hint[:1] in {"'", '"'} and hint[-1:] == hint[:1]:
            hint = hint[1:-1]
        return hint

    def _parse_container_hint(self, hint: str) -> tuple[str, str | None]:
        if hint.endswith("]") and "[" in hint:
            base, inner = hint.split("[", 1)
            base = base.strip()
            inner = inner[:-1].strip()
            if base in {"list", "tuple"} and inner:
                if "," in inner:
                    parts = [part.strip() for part in inner.split(",") if part.strip()]
                    if parts:
                        inner = parts[0]
                return base, inner
            if base == "dict":
                return base, None
        return hint, None

    def _parse_dict_hint(self, hint: str) -> tuple[str | None, str | None]:
        if not hint.startswith("dict[") or not hint.endswith("]"):
            return None, None
        inner = hint[len("dict[") : -1]
        parts = [part.strip() for part in inner.split(",") if part.strip()]
        if len(parts) != 2:
            return None, None
        return parts[0], parts[1]

    def _apply_hint_to_value(
        self, _name: str | None, value: MoltValue, hint: str
    ) -> None:
        base, elem = self._parse_container_hint(hint)
        value.type_hint = base
        if self.current_func_name == "molt_main":
            elem_target = self.global_elem_hints
            key_target = self.global_dict_key_hints
            val_target = self.global_dict_value_hints
        else:
            elem_target = self.container_elem_hints
            key_target = self.dict_key_hints
            val_target = self.dict_value_hints
        key = value.name
        if base == "dict":
            dict_key, dict_val = self._parse_dict_hint(hint)
            if dict_key and dict_val:
                key_target[key] = dict_key
                val_target[key] = dict_val
            else:
                key_target.pop(key, None)
                val_target.pop(key, None)
            elem_target.pop(key, None)
        else:
            if elem:
                elem_target[key] = elem
            else:
                elem_target.pop(key, None)
            key_target.pop(key, None)
            val_target.pop(key, None)

    def _propagate_container_hints(self, dest: str, src: MoltValue) -> None:
        if self.current_func_name == "molt_main":
            elem_map = self.global_elem_hints
            key_map = self.global_dict_key_hints
            val_map = self.global_dict_value_hints
        else:
            elem_map = self.container_elem_hints
            key_map = self.dict_key_hints
            val_map = self.dict_value_hints
        if src.name in elem_map:
            elem_map[dest] = elem_map[src.name]
        else:
            elem_map.pop(dest, None)
        if src.name in key_map and src.name in val_map:
            key_map[dest] = key_map[src.name]
            val_map[dest] = val_map[src.name]
        else:
            key_map.pop(dest, None)
            val_map.pop(dest, None)
        # Propagate list_int container tracking across assignments
        li_set = getattr(self, "_list_int_containers", set())
        if src.name in li_set:
            li_set.add(dest)
        else:
            li_set.discard(dest)
        if src.name in self.bytearray_len_hints:
            self.bytearray_len_hints[dest] = self.bytearray_len_hints[src.name]
        else:
            self.bytearray_len_hints.pop(dest, None)

    def _record_list_element_write(
        self,
        target: MoltValue,
        target_name: str | None,
        elem_hint: str | None,
    ) -> None:
        elem_map = (
            self.global_elem_hints
            if self.current_func_name == "molt_main"
            else self.container_elem_hints
        )
        keys = [target.name]
        if target_name is not None:
            keys.append(target_name)
        current = next((elem_map[key] for key in keys if key in elem_map), None)
        if elem_hint in {None, "Any", "Unknown", "missing"}:
            for key in keys:
                elem_map.pop(key, None)
            return
        if current is not None and current != elem_hint:
            for key in keys:
                elem_map.pop(key, None)
            return
        hint = elem_hint
        if hint is None:
            return
        for key in keys:
            elem_map[key] = hint

    def _bytearray_len_hint_for(
        self, name: str | None, value: MoltValue | None
    ) -> int | None:
        if value is not None and value.name in self.bytearray_len_hints:
            return self.bytearray_len_hints[value.name]
        if name is not None:
            return self.bytearray_len_hints.get(name)
        return None

    def _invalidate_bytearray_len_hint(
        self, name: str | None, value: MoltValue | None = None
    ) -> None:
        if value is not None:
            self.bytearray_len_hints.pop(value.name, None)
        if name is not None:
            self.bytearray_len_hints.pop(name, None)

    def _copy_container_hints_for_name_load(self, var_name: str, ssa_name: str) -> None:
        """Copy container element/dict hints from a Python variable binding to
        a fresh SSA name produced by a load."""
        if self.current_func_name == "molt_main":
            elem_map = self.global_elem_hints
            key_map = self.global_dict_key_hints
            val_map = self.global_dict_value_hints
        else:
            elem_map = self.container_elem_hints
            key_map = self.dict_key_hints
            val_map = self.dict_value_hints
        if var_name in elem_map:
            elem_map[ssa_name] = elem_map[var_name]
        if var_name in key_map:
            key_map[ssa_name] = key_map[var_name]
        if var_name in val_map:
            val_map[ssa_name] = val_map[var_name]
        # Propagate list_int container tracking to boxed reload
        li_set = getattr(self, "_list_int_containers", set())
        if var_name in li_set:
            li_set.add(ssa_name)

    def _container_elem_hint(self, value: MoltValue) -> str | None:
        if value.name in self.container_elem_hints:
            return self.container_elem_hints[value.name]
        return self.global_elem_hints.get(value.name)

    def _dict_key_hint(self, value: MoltValue) -> str | None:
        if value.name in self.dict_key_hints:
            return self.dict_key_hints[value.name]
        return self.global_dict_key_hints.get(value.name)

    def _iterable_element_hint(self, iterable: MoltValue) -> str | None:
        hint = iterable.type_hint
        if hint in {"range", "intarray"}:
            return "int"
        if hint == "str":
            return "str"
        if hint in {"bytes", "bytearray"}:
            return "int"
        if hint == "file_text":
            return "str"
        if hint == "file_bytes":
            return "bytes"
        if hint == "dict":
            return self._dict_key_hint(iterable)
        return self._container_elem_hint(iterable)

    def _reduction_acc_numeric_hint(self, name: str, value: MoltValue) -> str | None:
        hint = self.boxed_local_hints.get(name) or value.type_hint
        if hint in {"int", "float"}:
            return hint
        return None

    def _dict_value_hint(self, value: MoltValue) -> str | None:
        if value.name in self.dict_value_hints:
            return self.dict_value_hints[value.name]
        return self.global_dict_value_hints.get(value.name)

    def _apply_type_facts(self, func_name: str) -> None:
        if self.type_facts is None:
            return
        if func_name == "molt_main":
            hints = self.type_facts.hints_for_globals(
                self.type_facts_module, self.type_hint_policy
            )
        else:
            hints = self.type_facts.hints_for_function(
                self.type_facts_module, func_name, self.type_hint_policy
            )
        self.explicit_type_hints.update(hints)

    def _annotation_to_hint(self, node: ast.expr | None) -> str | None:
        if node is None:
            return None
        try:
            text = ast.unparse(node)
        except Exception:
            return None
        stripped = text.strip()
        if stripped[:1] in {"'", '"'} and stripped[-1:] == stripped[:1]:
            stripped = stripped[1:-1]
        return normalize_type_hint(stripped)

    def _annotation_source(self, node: ast.expr) -> str:
        try:
            return ast.unparse(node)
        except Exception as exc:
            raise FrontendRejection(
                Diagnostic.TYPE_FORM, "Unsupported annotation expression"
            ) from exc

    def _emit_annotation_value(
        self, node: ast.expr, *, stringize: bool | None = None
    ) -> MoltValue:
        use_string = self.future_annotations if stringize is None else stringize
        if use_string:
            text = self._annotation_source(node)
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[text], result=res))
            return res
        prev_in_annotation = self.in_annotation
        self.in_annotation = True
        try:
            val = self.visit(node)
        finally:
            self.in_annotation = prev_in_annotation
        if val is None:
            raise FrontendRejection(
                Diagnostic.TYPE_FORM, "Unsupported annotation expression"
            )
        return val

    def _annotation_exec_name(self, owner: str) -> str:
        name = f"__molt_annotations_exec_{owner}_{self.annotation_name_counter}"
        self.annotation_name_counter += 1
        return name

    def _annotation_exec_id(self, *, is_module: bool) -> int:
        if is_module:
            ident = self.module_annotation_exec_counter
            self.module_annotation_exec_counter += 1
            return ident
        ident = self.class_annotation_exec_counter
        self.class_annotation_exec_counter += 1
        return ident

    def _publish_annotation_exec_map(self, name: str, exec_map: MoltValue) -> None:
        """Bind execution state where the deferred annotate body resolves it."""
        self._store_local_value(name, exec_map)
        if self.current_func_name == "molt_main" or self.current_func_name.startswith(
            "molt_init_"
        ):
            self.globals[name] = exec_map
            self._emit_module_attr_set(name, exec_map)

    def _annotate_qualname(self) -> str:
        prefix = self._qualname_prefix()
        if not prefix:
            return "__annotate__"
        return f"{prefix}.__annotate__"

    def _ensure_module_annotation_exec_map(self) -> MoltValue:
        if self.module_annotation_exec_map is not None:
            return self.module_annotation_exec_map
        if self.module_chunking and self.module_annotation_exec_name:
            existing = self._emit_module_attr_get(self.module_annotation_exec_name)
            self.module_annotation_exec_map = existing
            return existing
        owner = self._sanitize_module_name(self.module_name)
        name = self._annotation_exec_name(owner)
        self.module_annotation_exec_name = name
        exec_map = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=exec_map))
        self.module_annotation_exec_map = exec_map
        self._publish_annotation_exec_map(name, exec_map)
        return exec_map

    def _emit_annotation_exec_mark(self, exec_map: MoltValue, exec_id: int) -> None:
        key_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[exec_id], result=key_val))
        val_val = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=val_val))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[exec_map, key_val, val_val],
                result=MoltValue("none"),
            )
        )

    def _emit_module_annotations_dict(self) -> MoltValue:
        if self.control_flow_depth > 0:
            self.module_annotations_conditional = True
        if not self.module_annotations_conditional:
            if self.module_annotations is not None:
                return self.module_annotations
            existing = self.locals.get("__annotations__")
            if existing is not None and existing.type_hint == "dict":
                self.module_annotations = existing
                return existing
            ann = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=ann))
            self._emit_module_attr_set("__annotations__", ann)
            if self.current_func_name == "molt_main":
                self.globals["__annotations__"] = ann
            self.locals["__annotations__"] = ann
            self.module_annotations = ann
            return ann
        return self._emit_module_annotations_dict_dynamic()

    def _emit_module_annotations_dict_dynamic(self) -> MoltValue:
        module_dict = self._emit_globals_dict()
        key_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__annotations__"], result=key_val))
        default_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
        existing = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="DICT_GET",
                args=[module_dict, key_val, default_val],
                result=existing,
            )
        )
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[existing, default_val], result=is_none))
        use_phi = self.enable_phi and not self.is_async()
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            ann = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=ann))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[module_dict, key_val, ann],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            merged = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="PHI", args=[ann, existing], result=merged))
            return merged

        placeholder = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[placeholder], result=cell))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=idx))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        ann = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=ann))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[module_dict, key_val, ann],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, ann],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, existing],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        merged = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=merged))
        return merged

    def _annotation_items_for_function(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> list[tuple[str, ast.expr]]:
        items: list[tuple[str, ast.expr]] = []
        for arg in node.args.posonlyargs + node.args.args:
            if arg.annotation is not None:
                items.append((arg.arg, arg.annotation))
        if node.args.vararg is not None and node.args.vararg.annotation is not None:
            items.append((node.args.vararg.arg, node.args.vararg.annotation))
        for arg in node.args.kwonlyargs:
            if arg.annotation is not None:
                items.append((arg.arg, arg.annotation))
        if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
            items.append((node.args.kwarg.arg, node.args.kwarg.annotation))
        if node.returns is not None:
            items.append(("return", node.returns))
        return items

    def _emit_type_params_values(
        self,
        type_params: Sequence[ast.AST | ast.type_param] | None,
        *,
        expression_rewriter: Callable[[ast.expr], ast.expr] | None = None,
        module_override: str | None = None,
    ) -> tuple[list[MoltValue], dict[str, MoltValue]]:
        if not type_params:
            return [], {}
        type_param_func = self._emit_module_attr_get_on("typing", "_molt_type_param")
        values: list[MoltValue] = []
        mapping: dict[str, MoltValue] = {}
        for param in type_params:
            if isinstance(param, (ast.TypeVar, ast.ParamSpec, ast.TypeVarTuple)):
                name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[param.name], result=name_val))
                kind_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="CONST_STR",
                        args=[type(param).__name__],
                        result=kind_val,
                    )
                )
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[type_param_func, name_val, kind_val],
                        result=res,
                    )
                )
                values.append(res)
                mapping[param.name] = res
                continue
            raise FrontendRejection(
                Diagnostic.TYPE_FORM,
                f"Unsupported type parameter: {type(param).__name__}",
            )
        evaluator_setter = self._emit_module_attr_get_on(
            "typing", "_molt_type_param_set_evaluators"
        )
        previous_type_params = self.annotation_type_params
        merged = dict(previous_type_params)
        merged.update(mapping)
        self.annotation_type_params = merged
        try:
            for param, value in zip(type_params, values):
                bound_expr = getattr(param, "bound", None)
                default_expr = getattr(param, "default_value", None)
                if default_expr is not None and self.target_python < (3, 13):
                    raise FrontendRejection(
                        Diagnostic.TYPE_FORM,
                        "Type parameter defaults require target Python 3.13+",
                    )
                if bound_expr is None and default_expr is None:
                    continue
                bound_evaluator: MoltValue | None = None
                constraints_evaluator: MoltValue | None = None
                default_evaluator: MoltValue | None = None
                if isinstance(bound_expr, ast.Tuple):
                    expression = (
                        expression_rewriter(bound_expr)
                        if expression_rewriter is not None
                        else bound_expr
                    )
                    constraints_evaluator = self._emit_lazy_type_value_evaluator(
                        expression,
                        key="__constraints__",
                        module_override=module_override,
                    )
                elif isinstance(bound_expr, ast.expr):
                    expression = (
                        expression_rewriter(bound_expr)
                        if expression_rewriter is not None
                        else bound_expr
                    )
                    bound_evaluator = self._emit_lazy_type_value_evaluator(
                        expression,
                        key="__bound__",
                        module_override=module_override,
                    )
                if isinstance(default_expr, ast.expr):
                    expression = (
                        expression_rewriter(default_expr)
                        if expression_rewriter is not None
                        else default_expr
                    )
                    default_evaluator = self._emit_lazy_type_value_evaluator(
                        expression,
                        key="__default__",
                        module_override=module_override,
                    )
                evaluators: list[MoltValue] = []
                for evaluator in (
                    bound_evaluator,
                    constraints_evaluator,
                    default_evaluator,
                ):
                    if evaluator is not None:
                        evaluators.append(evaluator)
                        continue
                    none_value = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_value))
                    evaluators.append(none_value)
                configured = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[evaluator_setter, value, *evaluators],
                        result=configured,
                    )
                )
        finally:
            self.annotation_type_params = previous_type_params
        return values, mapping

    def _emit_lazy_type_value_evaluator(
        self,
        expression: ast.expr,
        *,
        key: str,
        module_override: str | None = None,
    ) -> MoltValue:
        return self._emit_annotate_function_obj(
            items=[(key, expression, 0)],
            exec_map_name=None,
            stringize=False,
            module_override=module_override,
        )

    def _emit_type_alias_value(
        self,
        node: ast.TypeAlias,
        *,
        expression_rewriter: Callable[[ast.expr], ast.expr] | None = None,
        module_override: str | None = None,
    ) -> MoltValue:
        if not isinstance(node.name, ast.Name):
            raise FrontendRejection(
                Diagnostic.TYPE_FORM, "Unsupported type alias target"
            )
        alias_fn = self._emit_module_attr_get_on("typing", "_molt_type_alias")
        type_param_values, type_param_map = self._emit_type_params_values(
            node.type_params,
            expression_rewriter=expression_rewriter,
            module_override=module_override,
        )
        expression = (
            expression_rewriter(node.value)
            if expression_rewriter is not None
            else node.value
        )
        previous_type_params = self.annotation_type_params
        merged = dict(previous_type_params)
        merged.update(type_param_map)
        self.annotation_type_params = merged
        try:
            evaluator = self._emit_lazy_type_value_evaluator(
                expression,
                key="__value__",
                module_override=module_override,
            )
        finally:
            self.annotation_type_params = previous_type_params
        name_value = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[node.name.id], result=name_value))
        params_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=type_param_values, result=params_tuple))
        alias_value = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[alias_fn, name_value, evaluator, params_tuple],
                result=alias_value,
            )
        )
        return alias_value

    def _emit_attach_type_params(
        self, owner: MoltValue, type_params: list[MoltValue]
    ) -> None:
        if not type_params:
            return
        tuple_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=type_params, result=tuple_val))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[owner, "__type_params__", tuple_val],
                result=MoltValue("none"),
            )
        )

    def _emit_annotate_function_obj(
        self,
        *,
        items: list[tuple[str, ast.expr, int]],
        exec_map_name: str | None,
        stringize: bool,
        module_override: str | None = None,
    ) -> MoltValue:
        func_symbol = self._function_symbol("__annotate__")
        free_vars: set[str] = set()
        for _name, expr, _exec_id in items:
            free_vars.update(self._collect_annotation_free_vars(expr))
        if exec_map_name and self.current_func_name != "molt_main":
            free_vars.add(exec_map_name)
        free_vars_list = sorted(free_vars)
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if free_vars_list:
            self.unbound_check_names.update(free_vars_list)
            closure_items: list[MoltValue] = []
            for name in free_vars_list:
                type_param_value = self.annotation_type_params.get(name)
                if type_param_value is not None:
                    cell = MoltValue(self.next_var(), type_hint="list")
                    self.emit(
                        MoltOp(kind="LIST_NEW", args=[type_param_value], result=cell)
                    )
                    closure_items.append(cell)
                    free_var_hints[name] = type_param_value.type_hint or "Any"
                    continue
                self._box_local(name)
                self.closure_locals.add(name)
                hint = self.boxed_local_hints.get(name)
                if hint is None:
                    value = self.locals.get(name)
                    if value is not None and value.type_hint:
                        hint = value.type_hint
                free_var_hints[name] = hint or "Any"
                closure_cell = self._load_boxed_cell(name)
                if closure_cell is None:
                    closure_cell = self.boxed_locals[name]
                closure_items.append(closure_cell)
            closure_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val))
            has_closure = True
        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[func_symbol, 1, closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(MoltOp(kind="FUNC_NEW", args=[func_symbol, 1], result=func_val))
        self._emit_function_metadata(
            func_val,
            name="__annotate__",
            qualname=self._annotate_qualname(),
            trace_lineno=None,
            posonly_params=["format"],
            pos_or_kw_params=[],
            kwonly_params=[],
            vararg=None,
            varkw=None,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=None,
            module_override=module_override,
        )

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        params, parameter_bindings = self._function_transport_params(
            ["format"],
            has_closure=has_closure,
        )
        self.start_function(func_symbol, params=params, type_facts_name="__annotate__")
        self.parameter_bindings = parameter_bindings
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars_list)}
            self.free_var_hints = free_var_hints
            self.compiler_bindings[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        self.del_targets = set()
        self.unbound_check_names = set()
        format_val = self._parameter_value("format", type_hint="int")
        self.locals["format"] = format_val

        one_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one_val))
        is_one = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[format_val, one_val], result=is_one))
        two_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[2], result=two_val))
        is_two = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[format_val, two_val], result=is_two))
        exec_map_val: MoltValue | None = None
        if exec_map_name is not None:
            exec_map_val = self.visit(ast.Name(id=exec_map_name, ctx=ast.Load()))
        missing_val = MoltValue(self.next_var(), type_hint="missing")
        self.emit(MoltOp(kind="MISSING", args=[], result=missing_val))

        def emit_annotation_body(use_stringize: bool) -> None:
            res_dict = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=res_dict))
            for name, expr, exec_id in items:
                if exec_map_val is not None:
                    key_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[exec_id], result=key_val))
                    exec_flag = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="DICT_GET",
                            args=[exec_map_val, key_val, missing_val],
                            result=exec_flag,
                        )
                    )
                    is_missing = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(
                            kind="IS",
                            args=[exec_flag, missing_val],
                            result=is_missing,
                        )
                    )
                    self.emit(
                        MoltOp(kind="IF", args=[is_missing], result=MoltValue("none"))
                    )
                    self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                value_val = self._emit_annotation_value(expr, stringize=use_stringize)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[res_dict, key_val, value_val],
                        result=MoltValue("none"),
                    )
                )
                if exec_map_val is not None:
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self._emit_normal_return_terminator(res_dict)

        self.emit(MoltOp(kind="IF", args=[is_one], result=MoltValue("none")))
        emit_annotation_body(stringize)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="IF", args=[is_two], result=MoltValue("none")))
        emit_annotation_body(stringize)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        msg_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=msg_val))
        err_val = self._emit_exception_new("NotImplementedError", msg_val)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        return func_val

    def _emit_function_annotate(
        self, func_val: MoltValue, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> None:
        items = self._annotation_items_for_function(node)
        type_params = getattr(node, "type_params", None)
        type_param_vals, type_param_map = self._emit_type_params_values(type_params)
        if not items:
            self._emit_attach_type_params(func_val, type_param_vals)
            return
        annotated_items = [(name, expr, idx) for idx, (name, expr) in enumerate(items)]
        prev_type_params = self.annotation_type_params
        if type_param_map:
            merged = dict(prev_type_params)
            merged.update(type_param_map)
            self.annotation_type_params = merged
        try:
            if not self.future_annotations and not self.eager_annotations:
                annotate_val = self._emit_annotate_function_obj(
                    items=annotated_items,
                    exec_map_name=None,
                    stringize=self.future_annotations,
                )
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[func_val, "__annotate__", annotate_val],
                        result=MoltValue("none"),
                    )
                )
            else:
                # Build __annotations__ dict directly from the annotation
                # items.  For future_annotations, all values are strings.
                # For eager_annotations, they're evaluated types.
                # Calling __annotate__(1) through CALL_FUNC has been unreliable
                # for TIR-compiled functions, so we build the dict inline.
                ann_items: list[MoltValue] = []
                for name, expr in items:
                    key_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                    val = self._emit_annotation_value(
                        expr, stringize=self.future_annotations
                    )
                    ann_items.extend([key_val, val])
                ann_dict = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=ann_items, result=ann_dict))
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[func_val, "__annotations__", ann_dict],
                        result=MoltValue("none"),
                    )
                )
        finally:
            self.annotation_type_params = prev_type_params
        self._emit_attach_type_params(func_val, type_param_vals)

    def _guard_tag_for_hint(self, hint: str) -> int | None:
        mapping = {
            "Any": 0,
            "Unknown": 0,
            "int": 1,
            "float": 2,
            "bool": 3,
            "None": 4,
            "str": 5,
            "bytes": 6,
            "bytearray": 7,
            "complex": 19,
            "list": 8,
            "tuple": 9,
            "dict": 10,
            "range": 11,
            "slice": 12,
            "dataclass": 13,
            "buffer2d": 14,
            "memoryview": 15,
            "intarray": 16,
            "set": 17,
            "frozenset": 18,
        }
        return mapping.get(hint)

    def _emit_guard_type(self, value: MoltValue, hint: str) -> None:
        base = hint.split("[", 1)[0] if "[" in hint else hint
        tag = self._guard_tag_for_hint(base)
        if tag is None or tag == 0:
            return
        tag_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[tag], result=tag_val))
        self.emit(
            MoltOp(kind="GUARD_TAG", args=[value, tag_val], result=MoltValue("none"))
        )
