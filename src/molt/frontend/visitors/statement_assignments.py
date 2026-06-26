"""AssignmentStatementVisitorMixin: assignment, annotation, delete, and augassign statements.

Move-only extraction from frontend/__init__.py. Assignment-family statements stay
separate from control flow so statement decomposition does not create a new god
file.
"""

from __future__ import annotations

import ast

from typing import TYPE_CHECKING, cast

from molt.frontend._types import (
    MoltOp,
    MoltValue,
    _canonical_intrinsic_runtime_name,
)
from molt.frontend.lowering.op_kinds_generated import AUGASSIGN_OP_KIND

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class AssignmentStatementVisitorMixin(_MixinBase):
    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        if not isinstance(node.target, (ast.Name, ast.Attribute)):
            raise NotImplementedError("Only simple annotated assignments are supported")
        if node.value is not None:
            self._maybe_record_module_overrides([node.target], node.value)
        hint = None
        if self._hints_enabled():
            hint = self._annotation_to_hint(node.annotation)
            if (
                isinstance(node.target, ast.Name)
                and hint is not None
                and node.target.id not in self.explicit_type_hints
            ):
                self.explicit_type_hints[node.target.id] = hint
        if isinstance(node.target, ast.Name) and self.current_func_name == "molt_main":
            if self.future_annotations:
                ann_dict = self._emit_module_annotations_dict()
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=[node.target.id], result=key_val)
                )
                ann_val = self._emit_annotation_value(node.annotation, stringize=True)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[ann_dict, key_val, ann_val],
                        result=MoltValue("none"),
                    )
                )
            else:
                if self.eager_annotations:
                    ann_dict = self._emit_module_annotations_dict()
                    key_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(kind="CONST_STR", args=[node.target.id], result=key_val)
                    )
                    ann_val = self._emit_annotation_value(
                        node.annotation, stringize=False
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_INDEX",
                            args=[ann_dict, key_val, ann_val],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    exec_map = self._ensure_module_annotation_exec_map()
                    exec_id = self.module_annotation_ids.get(id(node))
                    if exec_id is None:
                        exec_id = self._annotation_exec_id(is_module=True)
                        self.module_annotation_items.append(
                            (node.target.id, node.annotation, exec_id)
                        )
                    self._emit_annotation_exec_mark(exec_map, exec_id)
        if node.value is None:
            return None
        optional_intrinsic_name = self._match_optional_intrinsic_loader_expr(node.value)
        if optional_intrinsic_name is not None:
            value_node = self._emit_optional_intrinsic_lookup_value(
                optional_intrinsic_name
            )
        else:
            value_node = self.visit(node.value)
        if isinstance(node.target, ast.Name):
            if self.current_func_name == "molt_main":
                if optional_intrinsic_name is None:
                    self.module_intrinsic_globals.pop(node.target.id, None)
                else:
                    runtime_name = _canonical_intrinsic_runtime_name(
                        optional_intrinsic_name
                    )
                    self.module_intrinsic_globals[node.target.id] = runtime_name
                    self.reserved_external_func_symbols.add(runtime_name)
            self._apply_explicit_hint(node.target.id, value_node)
            if (
                self.current_func_name == "molt_main"
                or node.target.id not in self.global_decls
            ):
                self._update_exact_local(node.target.id, node.value)
            if (
                self.current_func_name != "molt_main"
                and node.target.id in self.global_decls
            ):
                self._store_local_value(node.target.id, value_node)
                return None
            if self.is_async():
                self._store_local_value(node.target.id, value_node)
            else:
                self._store_local_value(node.target.id, value_node)
                if value_node is not None:
                    self._propagate_container_hints(node.target.id, value_node)
                self._emit_module_attr_set(node.target.id, value_node)
                if self.current_func_name == "molt_main":
                    self.globals[node.target.id] = value_node
            return None

        obj = self.visit(node.target.value)
        obj_name = None
        if isinstance(node.target.value, ast.Name):
            class_name = node.target.value.id
            obj_name = class_name
            if class_name in self.classes:
                self._invalidate_loop_guards_for_class(class_name)
        exact_class = None
        if isinstance(node.target.value, ast.Name):
            exact_class = self.exact_locals.get(node.target.value.id)
        class_info = None
        if obj is not None:
            class_info = self.classes.get(obj.type_hint)
        if exact_class is not None:
            self._record_instance_attr_mutation(exact_class, node.target.attr)
        elif obj is not None and obj.type_hint in self.classes:
            self._record_instance_attr_mutation(obj.type_hint, node.target.attr)
        if exact_class is not None and obj is not None:
            exact_info = self.classes.get(exact_class)
            if (
                exact_info
                and not exact_info.get("dynamic")
                and not exact_info.get("dataclass")
            ):
                field_map = exact_info.get("fields", {})
                if (
                    node.target.attr in field_map
                    and not self._class_attr_is_data_descriptor(
                        exact_class, node.target.attr
                    )
                ):
                    self._emit_guarded_setattr(
                        obj,
                        node.target.attr,
                        value_node,
                        exact_class,
                        obj_name=obj_name,
                        assume_exact=True,
                    )
                    return None
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if node.target.attr not in field_map:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[obj, node.target.attr, value_node],
                        result=MoltValue("none"),
                    )
                )
                return None
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[field_map[node.target.attr]], result=idx_val)
            )
            self.emit(
                MoltOp(
                    kind="DATACLASS_SET",
                    args=[obj, idx_val, value_node],
                    result=MoltValue("none"),
                )
            )
        else:
            field_map = class_info.get("fields", {}) if class_info else {}
            if obj is not None and obj.type_hint in self.classes:
                if class_info and class_info.get("dynamic"):
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, node.target.attr, value_node],
                            result=MoltValue("none"),
                        )
                    )
                elif node.target.attr in field_map:
                    if self._class_attr_is_data_descriptor(
                        obj.type_hint, node.target.attr
                    ):
                        self.emit(
                            MoltOp(
                                kind="SETATTR_GENERIC_PTR",
                                args=[obj, node.target.attr, value_node],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        # Inside a method body, `self` (the first parameter)
                        # is guaranteed to be an instance of the current class.
                        # Mark it as exact so the guarded setattr can use a
                        # direct field store instead of the slow generic path.
                        is_method_self = (
                            self.current_class is not None
                            and isinstance(node.target.value, ast.Name)
                            and node.target.value.id == self.current_method_first_param
                            and obj.type_hint == self.current_class
                        )
                        self._emit_guarded_setattr(
                            obj,
                            node.target.attr,
                            value_node,
                            obj.type_hint,
                            obj_name=obj_name,
                            assume_exact=is_method_self,
                        )
                else:
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, node.target.attr, value_node],
                            result=MoltValue("none"),
                        )
                    )
            else:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[obj, node.target.attr, value_node],
                        result=MoltValue("none"),
                    )
                )
        return None

    def visit_Assign(self, node: ast.Assign) -> None:
        self._maybe_record_module_overrides(node.targets, node.value)
        dict_inc_match = self._match_dict_increment_assign(node)
        if dict_inc_match is not None:
            dict_expr, key_expr, delta_expr = dict_inc_match
            dict_obj = self.visit(dict_expr)
            key_obj = self.visit(key_expr)
            delta_obj = self.visit(delta_expr)
            if dict_obj is not None and key_obj is not None and delta_obj is not None:
                # Fast-path increment lanes assume a stable dict object shape.
                self._emit_guard_dict_shape(dict_obj)
                self.emit(
                    MoltOp(
                        kind="DICT_STR_INT_INC",
                        args=[dict_obj, key_obj, delta_obj],
                        result=MoltValue("none"),
                    )
                )
                return None
        optional_intrinsic_name = self._match_optional_intrinsic_loader_expr(node.value)
        if optional_intrinsic_name is not None:
            value_node = self._emit_optional_intrinsic_lookup_value(
                optional_intrinsic_name
            )
        else:
            value_node = self.visit(node.value)
        for target in node.targets:
            self._emit_assign_target(target, value_node, node.value)
        return None

    def visit_Delete(self, node: ast.Delete) -> None:
        def delete_target(target: ast.AST) -> None:
            if isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    delete_target(elt)
                return
            if isinstance(target, ast.Name):
                self._emit_delete_name(target.id)
                return
            if isinstance(target, ast.Attribute):
                obj = self.visit(target.value)
                if obj is None:
                    raise NotImplementedError("del expects attribute owner")
                exact_class = None
                if isinstance(target.value, ast.Name):
                    exact_class = self.exact_locals.get(target.value.id)
                if exact_class is not None:
                    self._record_instance_attr_mutation(exact_class, target.attr)
                elif obj.type_hint in self.classes:
                    self._record_instance_attr_mutation(obj.type_hint, target.attr)
                res = MoltValue(self.next_var(), type_hint="None")
                if obj.type_hint in self.classes:
                    self.emit(
                        MoltOp(
                            kind="DELATTR_GENERIC_PTR",
                            args=[obj, target.attr],
                            result=res,
                        )
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="DELATTR_GENERIC_OBJ",
                            args=[obj, target.attr],
                            result=res,
                        )
                    )
                return
            if isinstance(target, ast.Subscript):
                target_obj = self.visit(target.value)
                if target_obj is None:
                    raise NotImplementedError("del expects subscript owner")
                target_name = (
                    target.value.id if isinstance(target.value, ast.Name) else None
                )
                if target_obj.type_hint == "bytearray":
                    self._invalidate_bytearray_len_hint(target_name, target_obj)
                if isinstance(target.slice, ast.Slice):
                    if target.slice.lower is None:
                        start = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                    else:
                        start = self.visit(target.slice.lower)
                    if target.slice.upper is None:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                    else:
                        end = self.visit(target.slice.upper)
                    if target.slice.step is None:
                        step = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                    else:
                        step = self.visit(target.slice.step)
                    slice_obj = MoltValue(self.next_var(), type_hint="slice")
                    self.emit(
                        MoltOp(
                            kind="SLICE_NEW",
                            args=[start, end, step],
                            result=slice_obj,
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="DEL_INDEX",
                            args=[target_obj, slice_obj],
                            result=MoltValue("none"),
                        )
                    )
                    return
                index_val = self.visit(target.slice)
                self.emit(
                    MoltOp(
                        kind="DEL_INDEX",
                        args=[target_obj, index_val],
                        result=MoltValue("none"),
                    )
                )
                return
            raise NotImplementedError(
                "del only supports name, attribute, or subscript deletion"
            )

        for target in node.targets:
            delete_target(target)
        return None

    def visit_AugAssign(self, node: ast.AugAssign) -> None:
        op_kind = self._augassign_op_kind(node.op)
        may_yield = self._expr_may_yield(node.value)
        if isinstance(node.target, ast.Name):
            self.exact_locals.pop(node.target.id, None)
            load_node = ast.Name(id=node.target.id, ctx=ast.Load())
            if may_yield and self.is_async() and node.target.id in self.async_locals:
                value_node = self.visit(node.value)
                current = self._load_local_value(node.target.id)
            else:
                current = self.visit(load_node)
                value_node = self.visit(node.value)
            if current is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            if value_node is None:
                raise NotImplementedError("Unsupported augmented assignment value")
            res = MoltValue(self.next_var(), type_hint=current.type_hint)
            self.emit(MoltOp(kind=op_kind, args=[current, value_node], result=res))
            # Class-body augmented assignment binds back into the class
            # namespace only (P0 #50); skip module/global publication so the
            # name does not leak into the enclosing scope.
            if self._active_class_ns_scope(node.target.id) is not None:
                self._store_local_value(node.target.id, res)
                return None
            if (
                self.current_func_name != "molt_main"
                and node.target.id in self.global_decls
            ):
                self._store_local_value(node.target.id, res)
                return None
            if self.is_async():
                self._store_local_value(node.target.id, res)
            else:
                self._apply_explicit_hint(node.target.id, res)
                self._store_local_value(node.target.id, res)
                if res is not None:
                    self._propagate_container_hints(node.target.id, res)
                self._emit_module_attr_set(node.target.id, res)
                if self.current_func_name == "molt_main":
                    self.globals[node.target.id] = res
            return None
        if isinstance(node.target, ast.Attribute):
            obj = self.visit(node.target.value)
            if obj is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            obj_name = None
            exact_class = None
            if isinstance(node.target.value, ast.Name):
                obj_name = node.target.value.id
                exact_class = self.exact_locals.get(obj_name)
            current = self._emit_attribute_load(node.target, obj, obj_name, exact_class)
            if self.is_async() and may_yield:
                obj_slot = self._spill_async_value(
                    obj, f"__augattr_obj_{len(self.async_locals)}"
                )
                current_slot = self._spill_async_value(
                    current, f"__augattr_cur_{len(self.async_locals)}"
                )
                value_node = self.visit(node.value)
                obj = self._reload_async_value(obj_slot, obj.type_hint)
                current = self._reload_async_value(current_slot, current.type_hint)
            else:
                value_node = self.visit(node.value)
            if value_node is None:
                raise NotImplementedError("Unsupported augmented assignment value")
            if current is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            res = MoltValue(self.next_var(), type_hint=current.type_hint)
            self.emit(MoltOp(kind=op_kind, args=[current, value_node], result=res))
            self._emit_attribute_store(
                obj,
                node.target.value,
                obj_name,
                exact_class,
                node.target.attr,
                res,
            )
            return None
        if isinstance(node.target, ast.Subscript):
            target_obj = self.visit(node.target.value)
            if target_obj is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            if isinstance(node.target.slice, ast.Slice):
                slice_node = node.target.slice
                if slice_node.lower is None:
                    start = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                else:
                    start = self.visit(slice_node.lower)
                if slice_node.upper is None:
                    end = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                else:
                    end = self.visit(slice_node.upper)
                if slice_node.step is None:
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                else:
                    step = self.visit(slice_node.step)
                if start is None or end is None or step is None:
                    raise NotImplementedError("Unsupported slice augmented assignment")
                res_type = "Any"
                if target_obj.type_hint in {
                    "bytes",
                    "bytearray",
                    "list",
                    "tuple",
                    "str",
                    "memoryview",
                }:
                    res_type = target_obj.type_hint
                slice_obj: MoltValue | None = None
                if slice_node.step is None:
                    current = MoltValue(self.next_var(), type_hint=res_type)
                    self.emit(
                        MoltOp(
                            kind="SLICE",
                            args=[target_obj, start, end],
                            result=current,
                        )
                    )
                else:
                    slice_obj = MoltValue(self.next_var(), type_hint="slice")
                    self.emit(
                        MoltOp(
                            kind="SLICE_NEW",
                            args=[start, end, step],
                            result=slice_obj,
                        )
                    )
                    current = MoltValue(self.next_var(), type_hint=res_type)
                    self.emit(
                        MoltOp(
                            kind="INDEX",
                            args=[target_obj, slice_obj],
                            result=current,
                        )
                    )
                if self.is_async() and may_yield:
                    obj_slot = self._spill_async_value(
                        target_obj, f"__augsub_obj_{len(self.async_locals)}"
                    )
                    start_slot = self._spill_async_value(
                        start, f"__augsub_start_{len(self.async_locals)}"
                    )
                    end_slot = self._spill_async_value(
                        end, f"__augsub_end_{len(self.async_locals)}"
                    )
                    step_slot = self._spill_async_value(
                        step, f"__augsub_step_{len(self.async_locals)}"
                    )
                    cur_slot = self._spill_async_value(
                        current, f"__augsub_cur_{len(self.async_locals)}"
                    )
                    slice_slot = None
                    if slice_obj is not None:
                        slice_slot = self._spill_async_value(
                            slice_obj, f"__augsub_slice_{len(self.async_locals)}"
                        )
                    value_node = self.visit(node.value)
                    target_obj = self._reload_async_value(
                        obj_slot, target_obj.type_hint
                    )
                    start = self._reload_async_value(start_slot, start.type_hint)
                    end = self._reload_async_value(end_slot, end.type_hint)
                    step = self._reload_async_value(step_slot, step.type_hint)
                    current = self._reload_async_value(cur_slot, current.type_hint)
                    if slice_slot is not None:
                        slice_obj = self._reload_async_value(slice_slot, "slice")
                else:
                    value_node = self.visit(node.value)
                if value_node is None:
                    raise NotImplementedError("Unsupported augmented assignment value")
                res = MoltValue(self.next_var(), type_hint=current.type_hint)
                self.emit(MoltOp(kind=op_kind, args=[current, value_node], result=res))
                if slice_obj is None:
                    slice_obj = MoltValue(self.next_var(), type_hint="slice")
                    self.emit(
                        MoltOp(
                            kind="SLICE_NEW",
                            args=[start, end, step],
                            result=slice_obj,
                        )
                    )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[target_obj, slice_obj, res],
                        result=MoltValue("none"),
                    )
                )
                return None
            index_val = self.visit(node.target.slice)
            if index_val is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            current = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="INDEX",
                    args=[target_obj, index_val],
                    result=current,
                )
            )
            if self.is_async() and may_yield:
                obj_slot = self._spill_async_value(
                    target_obj, f"__augsub_obj_{len(self.async_locals)}"
                )
                idx_slot = self._spill_async_value(
                    index_val, f"__augsub_idx_{len(self.async_locals)}"
                )
                cur_slot = self._spill_async_value(
                    current, f"__augsub_cur_{len(self.async_locals)}"
                )
                value_node = self.visit(node.value)
                target_obj = self._reload_async_value(obj_slot, target_obj.type_hint)
                index_val = self._reload_async_value(idx_slot, index_val.type_hint)
                current = self._reload_async_value(cur_slot, current.type_hint)
            else:
                value_node = self.visit(node.value)
            if value_node is None:
                raise NotImplementedError("Unsupported augmented assignment value")
            res = MoltValue(self.next_var(), type_hint=current.type_hint)
            self.emit(MoltOp(kind=op_kind, args=[current, value_node], result=res))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[target_obj, index_val, res],
                    result=MoltValue("none"),
                )
            )
            return None
        raise NotImplementedError("Unsupported augmented assignment target")

    def _emit_unpack_assign(
        self, target: ast.Tuple | ast.List, value_node: MoltValue | None
    ) -> None:
        if value_node is None:
            raise NotImplementedError("Unsupported unpack assignment value")
        star_index: int | None = None
        for idx, elt in enumerate(target.elts):
            if isinstance(elt, ast.Starred):
                if star_index is not None:
                    raise NotImplementedError(
                        "Multiple starred assignment is not supported"
                    )
                star_index = idx
        seq_val: MoltValue | None = None
        length: MoltValue | None = None

        def emit_unpack_error(
            prefix: str, expected: MoltValue, got: MoltValue | None
        ) -> None:
            parts: list[MoltValue] = []
            head = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[prefix], result=head))
            parts.append(head)
            parts.append(self._emit_str_from_obj(expected))
            if got is not None:
                mid = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[", got "], result=mid))
                parts.append(mid)
                parts.append(self._emit_str_from_obj(got))
            tail = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[")"], result=tail))
            parts.append(tail)
            msg_val = self._emit_string_join(parts)
            exc_val = self._emit_exception_new("ValueError", msg_val)
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
            self._emit_raise_exit()

        # For the no-star case without an indexable hint, we used to inline
        # a materialization loop that appended elements to a LIST_NEW list.
        # That caused heap corruption when the same list was reused across
        # outer-loop iterations.  Now we always pass value_node directly to
        # UNPACK_SEQUENCE and let the runtime handle validation + extraction.
        if star_index is None and not self._iterable_is_indexable(value_node):
            pass  # seq_val stays None → handled below
        if star_index is not None:
            if seq_val is None:
                seq_val = self._emit_list_from_iter(value_node)
            if length is None:
                length = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LEN", args=[seq_val], result=length))
        if star_index is None:
            if seq_val is None:
                seq_val = value_node
            # Emit a single outlined unpack_sequence op that validates the
            # length and extracts all elements in one runtime call.
            item_vals: list[MoltValue] = []
            for _ in target.elts:
                item_vals.append(MoltValue(self.next_var(), type_hint="Any"))
            self.emit(
                MoltOp(
                    kind="UNPACK_SEQUENCE",
                    args=[seq_val] + item_vals,
                    result=MoltValue("none"),
                    metadata={"expected_count": len(target.elts)},
                )
            )
            for elt, item_val in zip(target.elts, item_vals):
                self._emit_assign_target(elt, item_val, None)
            return

        prefix_len = star_index
        suffix_len = len(target.elts) - star_index - 1
        min_expected = prefix_len + suffix_len
        min_expected_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[min_expected], result=min_expected_val))
        too_few = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[length, min_expected_val], result=too_few))
        self.emit(MoltOp(kind="IF", args=[too_few], result=MoltValue("none")))
        emit_unpack_error(
            "not enough values to unpack (expected at least ",
            min_expected_val,
            length,
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        for idx in range(prefix_len):
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[idx], result=idx_val))
            item_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[seq_val, idx_val], result=item_val))
            self._emit_assign_target(target.elts[idx], item_val, None)

        start_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[prefix_len], result=start_val))
        end_val = length
        if suffix_len:
            suffix_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[suffix_len], result=suffix_val))
            end_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="SUB", args=[length, suffix_val], result=end_val))
        slice_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(kind="SLICE", args=[seq_val, start_val, end_val], result=slice_val)
        )
        star_target = cast(ast.Starred, target.elts[star_index]).value
        self._emit_assign_target(star_target, slice_val, None)

        if suffix_len:
            suffix_base = end_val
            for offset in range(suffix_len):
                if offset == 0:
                    idx_val = suffix_base
                else:
                    offset_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                    idx_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(
                            kind="ADD", args=[suffix_base, offset_val], result=idx_val
                        )
                    )
                item_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="INDEX", args=[seq_val, idx_val], result=item_val)
                )
                self._emit_assign_target(
                    target.elts[star_index + 1 + offset], item_val, None
                )
        return

    def _emit_assign_target(
        self,
        target: ast.AST,
        value_node: MoltValue | None,
        source_expr: ast.AST | None,
    ) -> None:
        if isinstance(target, (ast.Tuple, ast.List)):
            self._emit_unpack_assign(target, value_node)
            return
        if value_node is None:
            raise NotImplementedError("Unsupported assignment value")
        if isinstance(target, ast.Attribute):
            self._record_imported_module_attr_mutation(target)
            obj = self.visit(target.value)
            obj_name = None
            exact_class = None
            if isinstance(target.value, ast.Name):
                obj_name = target.value.id
                exact_class = self.exact_locals.get(obj_name)
            self._emit_attribute_store(
                obj,
                target.value,
                obj_name,
                exact_class,
                target.attr,
                value_node,
            )
            return
        if isinstance(target, ast.Name):
            # A class-body name (for-loop target, with-as target, tuple-unpack
            # element, plain assign) binds ONLY into the class namespace mapping
            # (P0 #50).  ``_store_local_value`` routes it there via the class-ns
            # hook; the module/global publication side effects below (module
            # attr-set, ``self.globals`` registration, exact-local tracking) are
            # for module/function scope and must NOT fire — they would leak the
            # class-body name into the enclosing namespace and steer later reads
            # away from the class dict.  Short-circuit to the single store.
            if self._active_class_ns_scope(target.id) is not None:
                self._store_local_value(target.id, value_node)
                return
            optional_intrinsic_name = (
                self._match_optional_intrinsic_loader_expr(source_expr)
                if source_expr is not None
                else None
            )
            self.imported_names.pop(target.id, None)
            self.imported_attr_names.pop(target.id, None)
            self.imported_modules.pop(target.id, None)
            self.local_imported_names.discard(target.id)
            self.local_imported_modules.discard(target.id)
            if self.current_func_name == "molt_main":
                self.global_imported_names.pop(target.id, None)
                self.global_imported_attr_names.pop(target.id, None)
                self.global_imported_modules.pop(target.id, None)
                if optional_intrinsic_name is None:
                    self.module_intrinsic_globals.pop(target.id, None)
                else:
                    runtime_name = _canonical_intrinsic_runtime_name(
                        optional_intrinsic_name
                    )
                    self.module_intrinsic_globals[target.id] = runtime_name
                    self.reserved_external_func_symbols.add(runtime_name)
            if (
                self.current_func_name == "molt_main"
                or target.id not in self.global_decls
            ):
                if source_expr is not None:
                    self._update_exact_local(target.id, source_expr)
                    self._propagate_func_type_hint(value_node, source_expr)
            if self.current_func_name != "molt_main" and target.id in self.global_decls:
                self._store_local_value(target.id, value_node)
                # Also update the module-level attribute so global assignment
                # is visible to other functions reading the module dict.
                self._emit_module_attr_set_runtime(target.id, value_node)
                return
            if self.is_async():
                self._store_local_value(target.id, value_node)
            else:
                self._apply_explicit_hint(target.id, value_node)
                self._store_local_value(target.id, value_node)
                if value_node is not None:
                    self._propagate_container_hints(target.id, value_node)
                self._emit_module_attr_set(target.id, value_node)
                if self.current_func_name == "molt_main":
                    self.module_chunk_globals.add(target.id)
                    self.globals[target.id] = value_node
            return
        if isinstance(target, ast.Subscript):
            target_obj = self.visit(target.value)
            target_name = (
                target.value.id if isinstance(target.value, ast.Name) else None
            )
            if isinstance(target.slice, ast.Slice):
                if target_obj is None:
                    raise NotImplementedError("Unsupported slice assignment target")
                if target_obj.type_hint == "bytearray":
                    self._invalidate_bytearray_len_hint(target_name, target_obj)
                if target.slice.lower is None:
                    start = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                else:
                    start = self.visit(target.slice.lower)
                if target.slice.upper is None:
                    end = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                else:
                    end = self.visit(target.slice.upper)
                if target.slice.step is None:
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                else:
                    step = self.visit(target.slice.step)
                slice_obj = MoltValue(self.next_var(), type_hint="slice")
                self.emit(
                    MoltOp(kind="SLICE_NEW", args=[start, end, step], result=slice_obj)
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[target_obj, slice_obj, value_node],
                        result=MoltValue("none"),
                    )
                )
                return
            index_val = self.visit(target.slice)
            if target_obj is not None and target_obj.type_hint == "list":
                self._record_list_element_write(
                    target_obj, target_name, value_node.type_hint
                )
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[target_obj, index_val, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        raise NotImplementedError("Unsupported assignment target")

    def _emit_delete_name(self, name: str, *, allow_missing: bool = False) -> None:
        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None:
            self._class_ns_delete(class_scope, name)
            return
        if self.current_func_name == "molt_main":
            if name in self.boxed_locals:
                cell = self.boxed_locals[name]
                idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=idx))
                # Read old value from cell before overwriting, then dec_ref
                # to release the initial allocation ref.  Without this, the
                # object's __del__ won't fire until function return.
                old_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=old_val))
                missing = self._emit_missing_value()
                self.globals.pop(name, None)
                if allow_missing:
                    self._emit_module_global_del_safe(name)
                else:
                    self._emit_module_global_del(name)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[cell, idx, missing],
                        result=MoltValue("none"),
                    )
                )
                self._emit_drop_owned_value(old_val)
                self.unbound_check_names.add(name)
                return
            # Module scope already has a canonical mutable store: the module
            # object. Do not synthesize boxed list cells just because a name is
            # later deleted; those cells add an extra mutable indirection layer
            # that diverges from module-backed semantics and can miscompile in
            # large chunked stdlib modules.
            local_val = self.locals.pop(name, None)
            self.globals.pop(name, None)
            if allow_missing:
                self._emit_module_global_del_safe(name)
            else:
                self._emit_module_global_del(name)
            # Emit dec_ref for the local SSA variable so refcount drops
            # to zero immediately, triggering __del__ (CPython parity).
            self._emit_drop_owned_value(local_val)
            return
        if name in self.global_decls:
            if allow_missing:
                self._emit_module_global_del_safe(name)
            else:
                self._emit_module_global_del(name)
            return
        if name in self.nonlocal_decls or name in self.free_vars:
            old_val = self._emit_free_var_load(name, guard_unbound=not allow_missing)
            missing = self._emit_missing_value()
            if not self._emit_free_var_store(name, missing):
                raise NotImplementedError("nonlocal binding not found")
            self._emit_drop_owned_value(old_val)
            return
        # Only box for closure-captured variables; non-closure locals use the
        # first-class delete_var local-slot transition so the backend sees one
        # atomic "mark unbound, then release old occupant" boundary.
        if name in self.closure_locals:
            self._box_local(name)
        old_val = self._load_local_value(name, guard_unbound=not allow_missing)
        missing = self._emit_missing_value()
        if (
            self.current_func_name != "molt_main"
            and not self.is_async()
            and name in self.scope_assigned
            and name not in self.boxed_locals
            and name not in self.free_vars
            and name not in self.nonlocal_decls
            and old_val is not None
        ):
            self._emit_delete_local_value(name, missing, old_val)
        else:
            self._store_local_value(name, missing)
            self._emit_drop_owned_value(old_val)
        self.unbound_check_names.add(name)
        return

    def _augassign_op_kind(self, op: ast.operator) -> str:
        # Every augmented assignment lowers to a dedicated INPLACE_* kind so the
        # runtime tries the in-place dunder (__iadd__/__ifloordiv__/__ipow__/...)
        # BEFORE the binary fallback, matching CPython. The boxed runtime symbol
        # for each (molt_inplace_floordiv etc.) first calls call_inplace_dunder
        # and only falls through to the binary protocol on NotImplemented. The
        # static int/float fast lanes remain identical to the binary op because
        # builtin int/float define no in-place dunders (so += on an int is byte-
        # identical whether it routes through molt_add or molt_inplace_add).
        #
        # AUGASSIGN_OP_KIND is generated from op_kinds.toml's [[binary_op]] table,
        # which is EXHAUSTIVE over ast.operator (a missing operator is a
        # generation-time failure — the task-#27 lesson). A KeyError here would
        # mean a NEW ast.operator subclass CPython added that the registry has
        # not yet been regenerated for.
        try:
            return AUGASSIGN_OP_KIND[type(op).__name__]
        except KeyError:
            raise NotImplementedError(
                f"Unsupported augmented assignment operator: {type(op).__name__}"
            ) from None
