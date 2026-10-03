"""LocalBindingMixin: local, closure, class-namespace, and locals() storage.

Move-only extraction from frontend/__init__.py. Owns the generator's canonical
name-binding storage paths: boxed locals, free-var cells, class-body namespace
routing, unbound guards, and a synchronous frame's binding homes, which own
its Python bindings.
"""

from __future__ import annotations

import ast
from contextlib import contextmanager
from functools import partial
from typing import (
    TYPE_CHECKING,
    Callable,
    Iterable,
    Iterator,
    Mapping,
    Sequence,
    TypeVar,
)

from molt.frontend._mixin_base import GeneratorMixinBase
from molt.compiler_analysis.static_truth import static_expression_result
from molt.frontend._types import (
    _MOLT_CLOSURE_PARAM,
    AsyncFrameSlotRole,
    ComprehensionBinding,
    ExactClassFact,
    MoltOp,
    MoltValue,
    ScratchCell,
    _ClassNsScope,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.lowering.generator_state import (
    FUNCTION_IMPORT_RESOLUTION_STATE_ATTRS,
)
from molt.frontend.sema.funcmeta import parse_stateful_function_type_hint

if TYPE_CHECKING:
    from molt.frontend.lowering.function_lifecycle import FrameRestoreScope


_ProjectionValue = TypeVar("_ProjectionValue")


def _restore_binding_projection(
    projection: dict[str, _ProjectionValue],
    source: dict[str, _ProjectionValue],
    name: str,
) -> None:
    """Restore one name without losing the projection's key/value coupling."""
    projection.pop(name, None)
    if name in source:
        projection[name] = source[name]


def _mask_binding_projection(
    projection: dict[str, _ProjectionValue], names: set[str]
) -> Callable[[], None]:
    """Suspend only these source names, retaining unrelated flow updates."""
    saved = {name: projection.pop(name) for name in names if name in projection}

    def restore() -> None:
        for name in names:
            projection.pop(name, None)
        projection.update(saved)

    return restore


class LocalBindingMixin(GeneratorMixinBase):
    def _advance_exact_class_token(self) -> int:
        self._next_exact_class_token += 1
        self.exact_class_token = self._next_exact_class_token
        return self.exact_class_token

    def _expire_exact_class_facts(self) -> None:
        self._advance_exact_class_token()
        self.exact_locals.clear()

    def _stamp_exact_class(self, value: MoltValue, class_id: str) -> MoltValue:
        value.exact_class = class_id
        value.exact_class_token = self.exact_class_token
        return value

    def _publish_exact_local(self, name: str, class_id: str) -> None:
        self.exact_locals[name] = ExactClassFact(class_id, self.exact_class_token)

    def _exact_class_for_name(self, name: str) -> str | None:
        fact = self.exact_locals.get(name)
        if fact is None or fact.token != self.exact_class_token:
            return None
        return fact.class_id

    def _snapshot_live_exact_bindings(self) -> dict[str, ExactClassFact]:
        return {
            name: fact
            for name, fact in self.exact_locals.items()
            if fact.token == self.exact_class_token
        }

    def _mask_exact_binding_projection(self, names: set[str]) -> Callable[[], None]:
        entry_token = self.exact_class_token
        saved = {
            name: fact
            for name in names
            if (fact := self.exact_locals.pop(name, None)) is not None
            and fact.token == entry_token
        }

        def restore() -> None:
            for name in names:
                self.exact_locals.pop(name, None)
            if self.exact_class_token == entry_token:
                self.exact_locals.update(saved)

        return restore

    def _emit_cell_new(self, value: MoltValue) -> MoltValue:
        return self._emit_runtime_call("molt_cell_new", [value], type_hint="cell")

    def _emit_cell_get(self, cell: MoltValue, *, type_hint: str = "Any") -> MoltValue:
        return self._emit_runtime_call("molt_cell_get", [cell], type_hint=type_hint)

    def _emit_cell_set(self, cell: MoltValue, value: MoltValue) -> None:
        self._emit_runtime_call("molt_cell_set", [cell, value], type_hint="None")

    def _specializable_builtin_name(self, node: ast.AST) -> str | None:
        """Shared identity/lifetime authorization for call and fused consumers."""
        if not isinstance(node, ast.Call) or self.python_binding_index is None:
            return None
        fact = self.python_binding_index.call_fact(node)
        if fact is None or not fact.callee_elision_safe:
            return None
        return fact.exact_builtin_name()

    def _expression_has_invalidated_binding(self, node: ast.expr) -> bool | None:
        """Query source-order authority before cached-name/call specialization.

        None when the binding analysis has no fact for the name. A read of a
        frame's home-backed binding then counts as possibly invalidated
        (`_load_local_value` takes the same tri-state): only a source fact can
        prove a cached view current.
        """
        while isinstance(node, ast.Attribute):
            node = node.value
        if not isinstance(node, ast.Name) or self.python_binding_index is None:
            return None
        fact = self.python_binding_index.expression_fact(node)
        return None if fact is None else fact.binding_invalidated

    def _call_has_bound_builtin_name(self, node: ast.expr) -> bool:
        if not isinstance(node, ast.Name) or not self._name_resolves_to_builtin(
            node.id
        ):
            return False
        if node.id in self.comp_shadow_locals:
            return True
        if self.python_binding_index is None:
            return False
        fact = self.python_binding_index.expression_fact(node)
        return fact is not None and fact.binding_is_bound

    def _function_transport_params(
        self,
        params: list[str],
        *,
        has_closure: bool,
    ) -> tuple[list[str], dict[str, str]]:
        """Separate public parameter identity from compiler-only ABI params."""
        bindings: dict[str, str] = {}
        for public_name in params:
            transport_name = public_name
            if has_closure and public_name == _MOLT_CLOSURE_PARAM:
                transport_name = self.next_var()
            bindings[public_name] = transport_name
        transport_params = [bindings[name] for name in params]
        if has_closure:
            transport_params.insert(0, _MOLT_CLOSURE_PARAM)
        if len(transport_params) != len(set(transport_params)):
            raise AssertionError("function transport parameters must be unique")
        return transport_params, bindings

    def _parameter_value(self, public_name: str, *, type_hint: str) -> MoltValue:
        transport_name = self.parameter_bindings.get(public_name, public_name)
        return MoltValue(transport_name, type_hint=type_hint)

    def _emit_name_from_obj(self, obj: MoltValue) -> MoltValue:
        name_key = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__name__"], result=name_key))
        missing = self._emit_missing_value()
        name_val = MoltValue(self.next_var(), type_hint="str")
        self._emit_getattr_name_default(
            obj,
            name_key,
            missing,
            name_val,
            literal_name="__name__",
        )
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[name_val, missing], result=is_missing))
        # Async/poll-function bodies must thread the result through a closure
        # slot rather than a LIST_NEW + STORE_INDEX cell. The cell pattern is
        # unsafe under Cranelift's loop-header phi resolver: the cell SSA
        # value can be merged with the entry-block default (None) on the
        # first iteration, producing store_index(None, ...) crashes.
        if self.is_async():
            slot = self._new_async_internal_slot()
            placeholder = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[""], result=placeholder))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, placeholder],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
            fallback = self._emit_str_from_obj(obj)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, fallback],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, name_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=res))
            return res

        # Sync path: a single SSA value updated in both branches replaces the
        # LIST_NEW + STORE_INDEX cell.
        res = MoltValue(self.next_var(), type_hint="str")
        placeholder = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=placeholder))
        self.emit(MoltOp(kind="COPY", args=[placeholder], result=res))
        self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
        fallback = self._emit_str_from_obj(obj)
        self.emit(MoltOp(kind="COPY", args=[fallback], result=res))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="COPY", args=[name_val], result=res))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    def _emit_type_name(self, value: MoltValue) -> MoltValue:
        type_val = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="TYPE_OF", args=[value], result=type_val))
        return self._emit_name_from_obj(type_val)

    def _box_local(self, name: str) -> None:
        binding = self.comprehension_bindings.get(name)
        if binding is not None:
            if not binding.is_cell:
                raise AssertionError("comprehension capture missing lexical cell fact")
            return
        if name in self.global_decls:
            return
        if name in self.boxed_locals:
            return
        if name in self.free_vars:
            cell = self._load_free_var_cell(name)
            if cell is None:
                return
            self.boxed_locals[name] = cell
            hint = self.free_var_hints.get(name)
            self.boxed_local_hints[name] = hint or "Any"
            self.locals[name] = cell
            return
        init: MoltValue
        if self.is_async() and name in self.async_locals:
            init = MoltValue(
                self.next_var(), type_hint=self.async_public_hints.get(name, "Any")
            )
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.async_locals[name].offset],
                    result=init,
                )
            )

        elif name in self.locals:
            init = self.locals[name]
        else:
            if name in self.scope_assigned or name in self.del_targets:
                init = self._emit_missing_value()
            else:
                init = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=init))
        cell = self._emit_cell_new(init)
        if self.frame_home_slots is not None:
            # MAKE_CELL: the cell enters the variable's home, and the frame
            # reaches it through the store's view from here on. The cell took
            # its own reference, so a parameter's frame reference ends here.
            cell = self._emit_frame_home_store(
                name,
                cell,
                kind=(
                    "FRAME_HOME_CELL"
                    if self._frame_slot_is_cell(name)
                    else "FRAME_HOME_PRIVATE_CELL"
                ),
            )
            if init.name == self.parameter_bindings.get(name):
                self.emit(
                    MoltOp(
                        kind="DEL_BOUNDARY",
                        args=[init],
                        result=MoltValue("none"),
                        metadata={"var": name},
                    )
                )
        self.boxed_locals[name] = cell
        if init.type_hint:
            self.boxed_local_hints[name] = init.type_hint
        else:
            self.boxed_local_hints[name] = "Unknown"
        self._update_python_argument_zero(name, cell, cell=True)
        self.locals[name] = cell
        if self.is_async():
            offset = self._async_local_offset(name)
            offset_value = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[offset], result=offset_value))
            self.emit(
                MoltOp(
                    kind="CALL",
                    args=["molt_frame_cell_publish", offset_value, cell],
                    result=MoltValue(self.next_var(), type_hint="None"),
                )
            )

    def _new_scratch_cell(
        self,
        initial: MoltValue | None = None,
        *,
        type_hint: str = "Any",
    ) -> ScratchCell:
        """Allocate opaque mutable storage outside every Python name map."""
        if initial is None:
            initial = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=initial))
        if self.is_async():
            slot = self._allocate_async_frame_slot(AsyncFrameSlotRole.SCRATCH)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot.offset, initial],
                    result=MoltValue("none"),
                )
            )
            return ScratchCell(
                value=None,
                async_slot=slot,
                type_hint=type_hint,
            )
        cell = self._emit_cell_new(initial)
        return ScratchCell(value=cell, async_slot=None, type_hint=type_hint)

    def _load_scratch_cell(self, cell: ScratchCell) -> MoltValue:
        result = MoltValue(self.next_var(), type_hint=cell.type_hint)
        if cell.async_slot is not None:
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", cell.async_slot.offset],
                    result=result,
                )
            )
            return result
        if cell.value is None:
            raise AssertionError("synchronous scratch cell has no storage value")
        return self._emit_cell_get(cell.value, type_hint=cell.type_hint)

    def _consume_scratch_cell(self, cell: ScratchCell) -> MoltValue:
        """Retain the loaded value before releasing compiler-owned storage."""
        value = self._load_scratch_cell(cell)
        self._clear_scratch_cell(cell)
        return value

    def _clear_scratch_cell(self, cell: ScratchCell) -> None:
        """Release hidden storage without acquiring another owned reference."""
        with self._suppress_check_exception(emit_on_exit=False):
            cleared = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared))
            self._store_scratch_cell(cell, cleared)

    def _store_scratch_cell(self, cell: ScratchCell, value: MoltValue) -> None:
        if cell.async_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", cell.async_slot.offset, value],
                    result=MoltValue("none"),
                )
            )
            return
        if cell.value is None:
            raise AssertionError("synchronous scratch cell has no storage value")
        self._emit_cell_set(cell.value, value)

    def _load_boxed_cell(self, name: str) -> MoltValue | None:
        binding = self.comprehension_bindings.get(name)
        if binding is not None:
            return self._load_comprehension_slot(binding) if binding.is_cell else None
        cell = self.boxed_locals.get(name)
        if cell is None:
            return None
        if name in self.free_vars:
            # The closure tuple is the stable owner; a cached extraction may
            # have been emitted in an untaken sibling branch. Reload transport
            # in the consuming block, just like async frame-owned cells.
            return self._load_free_var_cell(name)
        if not self.is_async():
            return cell
        if name not in self.async_locals:
            return cell
        slot_val = MoltValue(self.next_var(), type_hint="cell")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", self.async_locals[name].offset],
                result=slot_val,
            )
        )
        return slot_val

    def _capture_lexical_closure(
        self,
        candidates: Iterable[str],
        *,
        value_captures: dict[str, MoltValue] | None = None,
        cell_captures: Mapping[str, MoltValue] | None = None,
        class_scope: _ClassNsScope | None = None,
    ) -> tuple[list[str], dict[str, str], MoltValue | None, bool]:
        """Capture one lexical owner for every function-like source region.

        A class mapping is never an enclosing lexical frame. Its implicit
        class cell is a separate input, consulted only during closure creation;
        defaults and decorators retain the surrounding namespace/cell view.
        Module globals remain globals, except actual comprehension locals.
        """
        names = set(candidates)
        values = value_captures or {}
        captured_cells = cell_captures or {}
        scope = class_scope or (
            self._class_ns_stack[-1] if self._class_ns_stack else None
        )
        if scope is not None and scope.class_node is None:
            scope = None
        saved_scopes, saved_locals = self._class_ns_stack, self.locals
        saved_cell = self.boxed_locals.get("__class__")
        saved_hint = self.boxed_local_hints.get("__class__")
        if scope is not None:
            self._class_ns_stack, self.locals = [], scope.enclosing_locals
            if scope.class_cell is not None:
                self.boxed_locals["__class__"] = scope.class_cell
                self.boxed_local_hints["__class__"] = "type"
        try:
            if self.current_func_name == "molt_main":
                free_vars = sorted(names.intersection(self.comp_shadow_locals))
                if (
                    scope is not None
                    and scope.class_cell is not None
                    and "__class__" in names
                ):
                    free_vars = sorted({*free_vars, "__class__"})
            else:
                free_vars = self._free_vars_in_outer_scope(names)
            free_vars = sorted(
                set(free_vars)
                | (names & values.keys())
                | (names & captured_cells.keys())
            )
            if not free_vars:
                return [], {}, None, False
            self.unbound_check_names.update(free_vars)
            hints: dict[str, str] = {}
            cells: list[MoltValue] = []
            for name in free_vars:
                if name in captured_cells:
                    cells.append(captured_cells[name])
                    hints[name] = "Any"
                    continue
                if name in values:
                    value = values[name]
                    cell = self._emit_cell_new(value)
                    cells.append(cell)
                    hints[name] = value.type_hint or "Any"
                    continue
                self._box_local(name)
                binding = self.comprehension_bindings.get(name)
                if binding is None:
                    self.closure_locals.add(name)
                hint = (
                    binding.type_hint
                    if binding is not None
                    else self.boxed_local_hints.get(name)
                )
                if hint is None and (value := self.locals.get(name)) is not None:
                    hint = value.type_hint
                hints[name] = hint or "Any"
                cells.extend(self._closure_cells_for([name]))
            closure = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=cells, result=closure))
            return free_vars, hints, closure, True
        finally:
            self._class_ns_stack, self.locals = saved_scopes, saved_locals
            if scope is not None:
                if saved_cell is None:
                    self.boxed_locals.pop("__class__", None)
                else:
                    self.boxed_locals["__class__"] = saved_cell
                if saved_hint is None:
                    self.boxed_local_hints.pop("__class__", None)
                else:
                    self.boxed_local_hints["__class__"] = saved_hint

    def _closure_cells_for(self, names: Sequence[str]) -> list[MoltValue]:
        items: list[MoltValue] = []
        for name in names:
            cell = self._load_boxed_cell(name)
            if cell is None:
                cell = self.boxed_locals[name]
            items.append(cell)
        return items

    def _varnames_from_params(
        self,
        *,
        posonly_params: list[str],
        pos_or_kw_params: list[str],
        kwonly_params: list[str],
        vararg: str | None,
        varkw: str | None,
    ) -> list[str]:
        names: list[str] = []
        names.extend(posonly_params)
        names.extend(pos_or_kw_params)
        names.extend(kwonly_params)
        if vararg is not None:
            names.append(vararg)
        if varkw is not None:
            names.append(varkw)
        return names

    def _prebox_scope_cell_vars(
        self, cell_vars: Sequence[str], *, private_cells: Sequence[str] = ()
    ) -> None:
        if self.is_async():
            self.emit(
                MoltOp(
                    kind="CALL",
                    args=["molt_frame_locals_begin"],
                    result=MoltValue(self.next_var(), type_hint="None"),
                )
            )
        for name in cell_vars:
            self._box_local(name)
            self.closure_locals.add(name)
        for name in private_cells:
            self._box_local(name)

    def _emit_free_var_load(
        self,
        name: str,
        *,
        guard_unbound: bool = True,
        binding_invalidated: bool | None = None,
    ) -> MoltValue | None:
        cell = self._load_free_var_cell(name)
        if cell is None:
            return None
        hint = "Any" if binding_invalidated else self.free_var_hints.get(name, "Any")
        res = self._emit_cell_get(cell, type_hint=hint)
        if guard_unbound:
            self._emit_unbound_free_guard(res, name)
        return res

    def _emit_free_var_store(self, name: str, value: MoltValue) -> bool:
        cell = self._load_free_var_cell(name)
        if cell is None:
            return False
        self._emit_cell_set(cell, value)
        return True

    def _load_free_var_cell(self, name: str) -> MoltValue | None:
        closure = self.compiler_bindings.get(_MOLT_CLOSURE_PARAM)
        if (
            closure is None
            and self.is_async()
            and self.async_closure_offset is not None
        ):
            closure = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.async_closure_offset],
                    result=closure,
                )
            )
        if closure is None:
            return None
        idx = self.free_vars.get(name)
        if idx is None:
            return None
        idx_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[idx], result=idx_val))
        cell = MoltValue(self.next_var(), type_hint="cell")
        self.emit(MoltOp(kind="INDEX", args=[closure, idx_val], result=cell))
        return cell

    def _module_globals_dict_escapes(self, node: ast.Module) -> bool:
        """Use executed binding facts, including reflective aliases and callbacks."""
        return (
            self.python_binding_index is None
            or self.python_binding_index.module_namespace_may_be_observed(node)
        )

    def _builtin_exact_type_from_expr(self, value: ast.AST | None) -> str | None:
        if not isinstance(value, ast.expr):
            return None
        result = (
            self.python_binding_index.expression_result(value)
            if self.python_binding_index is not None
            else static_expression_result(value)
        )
        if result.kind == "unknown":
            return None
        return "None" if result.kind == "NoneType" else result.kind

    def _update_exact_local(
        self,
        name: str,
        source_expr: ast.AST | None,
        lowered_value: MoltValue | None,
    ) -> None:
        """Publish only exact identity proven by lowering or a live local alias."""
        if isinstance(source_expr, ast.Name):
            source_class = self._exact_class_for_name(source_expr.id)
            lowered_class = self._exact_class_for_value(lowered_value)
            if (
                source_class is not None
                and lowered_class == source_class
                and (
                    self.current_func_name == "molt_main"
                    or source_expr.id not in self.global_decls
                )
            ):
                self._publish_exact_local(name, source_class)
                return
            self.exact_locals.pop(name, None)
            return
        exact_class = self._exact_class_for_value(lowered_value)
        if exact_class is not None:
            self._publish_exact_local(name, exact_class)
            return
        self.exact_locals.pop(name, None)

    def _exact_class_for_value(
        self, value: MoltValue | None, source_name: str | None = None
    ) -> str | None:
        """Resolve the canonical lowered fact, with named-binding recovery."""
        if source_name is not None:
            named_class = self._exact_class_for_name(source_name)
            if (
                named_class is not None
                and value is not None
                and value.exact_class == named_class
                and value.exact_class_token == self.exact_class_token
            ):
                return named_class
            return None
        if (
            value is not None
            and value.exact_class is not None
            and value.exact_class_token == self.exact_class_token
        ):
            return value.exact_class
        return None

    def _propagate_func_type_hint(
        self, value_node: MoltValue, source_expr: ast.AST | None
    ) -> None:
        if (
            not isinstance(source_expr, ast.Name)
            or self._expression_has_invalidated_binding(source_expr) is not False
        ):
            return
        source_info = self.locals.get(source_expr.id) or self.globals.get(
            source_expr.id
        )
        if source_info is None:
            return
        hint = source_info.type_hint
        if not isinstance(hint, str):
            return
        stateful_hint = parse_stateful_function_type_hint(hint)
        if stateful_hint is not None:
            target = self.funcs_map.get(stateful_hint.poll_symbol)
            frame_plan = (
                target.get("stateful_frame_plan") if target is not None else None
            )
            if (
                frame_plan is not None
                and frame_plan.kind == stateful_hint.kind
                and frame_plan.has_closure == stateful_hint.has_closure
            ):
                value_node.type_hint = hint
            return
        if hint.startswith("Func:"):
            symbol = hint.split(":")[1]
            if (
                symbol in self.func_default_specs
                or self._known_function_symbol_target(symbol) is not None
            ):
                value_node.type_hint = hint

    @staticmethod
    def _is_class_body_managed_name(name: str) -> bool:
        # Calls into name-binding storage now carry only Python source bindings;
        # compiler-only bindings live in ``compiler_bindings`` or typed scratch
        # storage and never enter the class namespace path.
        return True

    def _async_binding_hint(self, name: str) -> str:
        slot = self._async_binding_slot(name)
        if slot.role is AsyncFrameSlotRole.PUBLIC:
            return self.async_public_hints.get(name, "Any")
        return self.async_internal_hints.get(name, "Any")

    def _active_class_ns_scope(self, name: str) -> "_ClassNsScope | None":
        # The innermost class-body scope manages ``name`` when the body is being
        # lowered as a block.  A nested ``class``/``def`` pushes its own scope (or
        # a function frame), so only the top-of-stack entry — and only while we
        # are still emitting that class's body statements (``_class_body_depth``
        # tracks the active body) — is consulted.  Names that are loop/scaffold
        # temps are excluded so the SSA machinery handles them unchanged.
        if not self._class_ns_stack:
            return None
        if name in self.comp_shadow_locals:
            return None
        if not self._is_class_body_managed_name(name):
            return None
        return self._class_ns_stack[-1]

    def _class_ns_store(
        self, scope: "_ClassNsScope", name: str, value: MoltValue
    ) -> None:
        if name in scope.global_names:
            self._emit_module_attr_set_runtime(name, value)
            return
        if name in scope.nonlocal_names:
            saved_scopes, saved_locals = self._class_ns_stack, self.locals
            self._class_ns_stack, self.locals = [], scope.enclosing_locals
            try:
                self._store_local_value(name, value)
            finally:
                self._class_ns_stack, self.locals = saved_scopes, saved_locals
            return
        # Bind a class-body name: snapshot the SSA value for the static fast path
        # AND, when a runtime namespace dict exists, publish it there so the dict
        # is the loop-carried-correct mutable home (and so a custom mapping's
        # ``__setitem__`` observes the store, matching CPython's class body).
        scope.names.add(name)
        scope.attr_values[name] = value
        scope.methods.pop(name, None)
        if scope.ns is not None:
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[scope.ns, key_val, value],
                    result=MoltValue("none"),
                )
            )

    def _class_ns_load(
        self,
        scope: "_ClassNsScope",
        name: str,
        *,
        binding_invalidated: bool | None = None,
    ) -> MoltValue | None:
        # Source binding invalidation does not change the storage owner. Probe
        # the live mapping even for never-stored names: __prepare__ or callbacks
        # may supply them. A missing class-local falls back to globals, whereas
        # an unassigned free name may fall back to its enclosing lexical cell.
        if name in scope.global_names:
            return self._emit_global_get(name)
        namespace = scope.ns
        if namespace is None and scope.annotation_namespace_cell is not None:
            namespace = self._emit_cell_get(
                scope.annotation_namespace_cell, type_hint="Any"
            )
        merge = None
        value = None
        if namespace is not None:
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
            missing = self._emit_missing_value()
            value = self._emit_runtime_call(
                "molt_namespace_get", [namespace, key_val, missing]
            )
            absent = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[value, missing], result=absent))
            merge = self._new_condition_merge(1, ())
            self.emit(MoltOp(kind="IF", args=[absent], result=MoltValue("none")))
        elif (value := scope.attr_values.get(name)) is not None:
            return value
        # A static namespace projection and a live mapping have the same miss
        # law. Neither may skip the enclosing cell or consult an outer class.
        fallback = None
        if name not in scope.local_names or name in scope.nonlocal_names:
            saved_scopes, saved_locals = self._class_ns_stack, self.locals
            self._class_ns_stack, self.locals = [], scope.enclosing_locals
            try:
                fallback = self._emit_free_var_load(
                    name, guard_unbound=False, binding_invalidated=binding_invalidated
                )
                if fallback is None and self.current_func_name != "molt_main":
                    fallback = self._load_local_value(
                        name,
                        guard_unbound=False,
                        binding_invalidated=binding_invalidated,
                    )
            finally:
                self._class_ns_stack, self.locals = saved_scopes, saved_locals
            if fallback is not None:
                self._emit_unbound_free_guard(fallback, name)
        if fallback is None:
            fallback = self._emit_global_get(name)
        if merge is None:
            return fallback
        assert value is not None
        absent_values = self._store_condition_branch(merge, (fallback,))
        self._condition_else(merge)
        present_values = self._store_condition_branch(merge, (value,))
        return self._finish_condition_merge(merge, absent_values, present_values)[0]

    def _class_ns_delete(self, scope: "_ClassNsScope", name: str) -> None:
        if name in scope.global_names:
            self._emit_module_global_del(name)
            return
        if name in scope.nonlocal_names:
            saved_scopes, saved_locals = self._class_ns_stack, self.locals
            self._class_ns_stack, self.locals = [], scope.enclosing_locals
            try:
                self._emit_delete_name(name)
            finally:
                self._class_ns_stack, self.locals = saved_scopes, saved_locals
            return
        # ``del name`` in a class body removes the binding from the namespace.
        # DELETE_NAME normalizes every failed mapping deletion to NameError;
        # unlike loads this is not restricted to KeyError. The runtime primitive
        # owns that exception translation for all backend consumers.
        scope.names.discard(name)
        scope.attr_values.pop(name, None)
        scope.methods.pop(name, None)
        if scope.ns is not None:
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
            self._emit_runtime_call(
                "molt_namespace_del", [scope.ns, key_val], type_hint="None"
            )

    def _load_local_value(
        self,
        name: str,
        *,
        guard_unbound: bool = True,
        binding_invalidated: bool | None = None,
        binding_may_be_unbound: bool | None = None,
    ) -> MoltValue | None:
        # Class-body loads own the full mapping/lexical/global lookup chain.
        # Comprehension and function scopes bypass the class mapping through
        # their scope boundary, not by falling through on a missing class key.
        # `binding_invalidated` is the binding analysis's fact for this read,
        # None when the caller has none; a frame's plain binding is then read
        # from its home wherever a frame proxy could have written it.
        # Source reads use the canonical source-point binding fact. The
        # emitter projection remains only for synthesized reads with no AST
        # fact: assignments emitted inside a loop do not dominate its exit.
        possibly_unbound = (
            name in self.unbound_check_names
            if binding_may_be_unbound is None
            else binding_may_be_unbound
        )

        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None:
            value = self._class_ns_load(
                class_scope, name, binding_invalidated=binding_invalidated
            )
            if value is None:
                return None
            return MoltValue(value.name, type_hint=value.type_hint)
        if name in self.comp_shadow_locals:
            binding = self.comprehension_bindings.get(name)
            if binding is None:
                value = self.locals.get(name)
                if value is None:
                    return None
                result = MoltValue(
                    value.name,
                    type_hint="Any" if binding_invalidated else value.type_hint,
                    borrows_binding=True,
                )
                exact_class = (
                    None if binding_invalidated else self._exact_class_for_name(name)
                )
                if exact_class is not None:
                    self._stamp_exact_class(result, exact_class)
                return result
            possibly_unbound = (
                not binding.definitely_bound
                if binding_may_be_unbound is None
                else binding_may_be_unbound
            )
            if (
                not binding.is_cell
                and binding.variable_slot is not None
                and self._comprehension_binds_homes()
                and (
                    possibly_unbound or self._binding_read_needs_home(binding_invalidated)
                )
            ):
                # The scope's binding lives in its name's home: a read that may
                # precede its first store observes the home's unbound state,
                # and a frame proxy may have written it since the last store.
                value = self._emit_frame_home_load(name)
            else:
                value = self._load_comprehension_slot(binding)
                if binding.is_cell:
                    value = self._emit_cell_get(
                        value,
                        type_hint="Any" if binding_invalidated else binding.type_hint,
                    )
                elif binding_invalidated:
                    value = MoltValue(value.name, type_hint="Any")
                if not binding.is_cell and binding.variable_slot is not None:
                    value.borrows_binding = True
                exact_class = (
                    None if binding_invalidated else self._exact_class_for_name(name)
                )
                if exact_class is not None:
                    self._stamp_exact_class(value, exact_class)
            if guard_unbound and possibly_unbound:
                self._emit_unbound_local_guard(value, name)
            return value
        if self.current_func_name != "molt_main" and name in self.global_decls:
            return self._emit_global_get(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
            hint = None if binding_invalidated else self.boxed_local_hints.get(name)
            res = self._emit_cell_get(cell, type_hint=hint or "Any")
            if not binding_invalidated:
                exact_class = self._exact_class_for_name(name)
                if exact_class is not None:
                    self._stamp_exact_class(res, exact_class)
                self._copy_container_hints_for_name_load(name, res.name)
            if guard_unbound and possibly_unbound:
                self._emit_unbound_local_guard(res, name)
            return res
        if self.is_async() and (
            name in self.async_locals or name in self.async_internal_bindings
        ):
            offset = self._async_binding_slot(name).offset
            hint = "Any" if binding_invalidated else self._async_binding_hint(name)
            res = MoltValue(self.next_var(), type_hint=hint)
            exact_class = (
                None if binding_invalidated else self._exact_class_for_name(name)
            )
            if exact_class is not None:
                self._stamp_exact_class(res, exact_class)
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
            if guard_unbound and possibly_unbound:
                self._emit_unbound_local_guard(res, name)
            return res
        cached = self.locals.get(name)
        possibly_unbound = cached is None or possibly_unbound
        if self._frame_home_is_plain(name) and (
            possibly_unbound or self._binding_read_needs_home(binding_invalidated)
        ):
            # The home is the binding. A read that may precede its store
            # observes the home's unbound state, and from 3.13 a callback may
            # have rebound it through a frame proxy since this frame wrote it.
            res = self._emit_frame_home_load(name)
            if guard_unbound and possibly_unbound:
                self._emit_unbound_local_guard(res, name)
            return res
        if cached is None:
            return None
        # Emit explicit load_var for non-boxed function locals so TIR can
        # track variable mutations through loop iterations via SSA phis.
        if (
            self.current_func_name != "molt_main"
            and not self.is_async()
            and name in self.scope_assigned
            and name not in self.boxed_locals
        ):
            exact_class = (
                None if binding_invalidated else self._exact_class_for_name(name)
            )
            res = MoltValue(
                self.next_var(),
                type_hint="Any" if binding_invalidated else cached.type_hint,
                borrows_binding=True,
            )
            self.emit(
                MoltOp(
                    kind="LOAD_VAR",
                    args=[],
                    result=res,
                    metadata={"var": name},
                )
            )
            if exact_class is not None:
                self._stamp_exact_class(res, exact_class)
                self._publish_exact_local(name, exact_class)
            if not binding_invalidated:
                self._copy_container_hints_for_name_load(name, res.name)
            if guard_unbound and possibly_unbound:
                self._emit_unbound_local_guard(res, name)
            return res
        result = MoltValue(
            cached.name, type_hint="Any" if binding_invalidated else cached.type_hint,
            borrows_binding=self.current_func_name != "molt_main",
        )
        exact_class = None if binding_invalidated else self._exact_class_for_name(name)
        if exact_class is not None:
            self._stamp_exact_class(result, exact_class)
        return result

    def _frame_home_slot(self, name: str) -> int:
        """The code slot of ``name`` in the running synchronous frame.

        The code object's slot declaration is the layout the runtime gives
        the frame's homes; every Python binding of an optimized frame has one.
        """
        slots = self.frame_home_slots
        slot = None if slots is None else slots.get(name)
        if slot is None:
            raise FrontendRejection(
                Diagnostic.INTERNAL_INVARIANT,
                f"binding {name!r} has no code slot in {self.current_func_name}",
            )
        return slot

    def _frame_slot_is_cell(self, name: str) -> bool:
        """Whether ``name`` is a cell variable of the running frame's code."""
        declaration = self.frame_code_slots
        return declaration is not None and name in declaration.cellvars

    def _frame_home_is_plain(self, name: str) -> bool:
        """Whether ``name``'s home in the running synchronous frame holds its
        binding itself, as `_store_local_value` stores it: a local this frame
        binds without a cell. A read that may precede its store must load
        that home."""
        slots = self.frame_home_slots
        return (
            slots is not None
            and name in slots
            and name not in self.boxed_locals
            and name not in self.free_vars
        )

    def _emit_frame_home_store(
        self,
        name: str,
        value: MoltValue,
        *,
        kind: str = "FRAME_HOME_STORE",
        slot: int | None = None,
    ) -> MoltValue:
        """Bind ``name`` in its home, which takes ``value``'s reference.

        The home publishes the new binding before it releases the one it
        displaces, as STORE_FAST does, so a finalizer that release runs sees
        the new binding. ``FRAME_HOME_CELL`` binds the frame's cell of a
        captured or free variable, ``FRAME_HOME_PRIVATE_CELL`` a cell the
        compiler keeps for a plain local. The result is the binding's view:
        the published object or raw carrier, borrowed until the slot's next
        write. A plain store's boxed view can allocate; ``emit`` authors its
        immediate exception edge. Cell stores transfer existing cell objects.
        """
        view = MoltValue(
            self.next_var(), type_hint=value.type_hint, borrows_binding=True
        )
        self.emit(
            MoltOp(
                kind=kind,
                args=[value],
                result=view,
                metadata={
                    "slot": self._frame_home_slot(name) if slot is None else slot
                },
            )
        )
        self._copy_container_hints_for_name_load(value.name, view.name)
        if value.name in self.const_ints:
            self.const_ints[view.name] = self.const_ints[value.name]
        return view

    def _emit_frame_home_load(self, name: str) -> MoltValue:
        """Read ``name``'s plain binding from its home: a view of whatever it
        holds now, of unknown type, the missing sentinel while unbound."""
        result = MoltValue(self.next_var(), type_hint="Any", borrows_binding=True)
        self.emit(
            MoltOp(
                kind="FRAME_HOME_LOAD",
                args=[],
                result=result,
                metadata={"slot": self._frame_home_slot(name)},
            )
        )
        return result

    def _emit_frame_home_take(self, name: str) -> MoltValue:
        """Move ``name``'s binding out of its home and leave it unbound: PEP
        709's save of an enclosing binding. The result is owned: the object,
        the cell, or the missing sentinel while unbound."""
        result = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="FRAME_HOME_TAKE",
                args=[],
                result=result,
                metadata={"slot": self._frame_home_slot(name)},
            )
        )
        return result

    def _emit_frame_home_clear(self, name: str) -> None:
        """``del name``: the home is left unbound and releases the binding."""
        self.emit(
            MoltOp(
                kind="FRAME_HOME_CLEAR",
                args=[],
                result=MoltValue("none"),
                metadata={"slot": self._frame_home_slot(name)},
            )
        )

    def _binding_read_needs_home(self, binding_invalidated: bool | None) -> bool:
        """Whether a read of a frame's plain binding must come from its home.

        Only a PEP 667 frame proxy writes a live optimized frame's bindings,
        so before 3.13 the frame's own view is current. From 3.13 a read the
        binding analysis has not proven clean, including one without a fact,
        reads the home: a callback may have rebound it through a proxy.
        """
        return (
            self.frame_home_slots is not None
            and self.target_python >= (3, 13)
            and binding_invalidated is not False
        )

    def _comprehension_binds_homes(self) -> bool:
        """Whether a comprehension lowered here binds its names in the running
        frame's homes. A class body lowered inline is a code object of its
        own, which keeps no homes."""
        return self.frame_home_slots is not None and not any(
            scope.class_node is not None for scope in self._class_ns_stack
        )

    def _frame_home_restore_kind(
        self, name: str, bindings: Mapping[str, ComprehensionBinding]
    ) -> str:
        """The home store that puts back ``name``'s enclosing binding, which
        ``bindings`` (the enclosing comprehension scopes) may shadow."""
        outer = bindings.get(name)
        if outer is not None:
            return "FRAME_HOME_CELL" if outer.is_cell else "FRAME_HOME_STORE"
        if name in self.boxed_locals and name not in self.free_vars:
            return (
                "FRAME_HOME_CELL"
                if self._frame_slot_is_cell(name)
                else "FRAME_HOME_PRIVATE_CELL"
            )
        return "FRAME_HOME_STORE"

    def _emit_frame_home_prologue(self, parameters: Sequence[str]) -> None:
        """Bind a synchronous frame's parameters and free variables in their homes.

        The entry adopts every Python argument: each parameter's reference
        moves into its home, and the frame reads it through the store's view.
        A parameter a cell holds entered its home with the cell (``_box_local``),
        a free variable's home holds the closure's cell, and every other local
        starts unbound, as its home does.
        """
        declaration = self.frame_code_slots
        if declaration is None:
            return
        for name in parameters:
            if name in self.boxed_locals:
                continue
            view = self._emit_frame_home_store(name, self.locals[name])
            self.locals[name] = view
            # Bound from entry: a read before its next store or delete borrows
            # this view (or its SSA transport) rather than loading the home.
            self.unbound_check_names.discard(name)
            if name in self.scope_assigned:
                self.emit(
                    MoltOp(
                        kind="STORE_VAR",
                        args=[view],
                        result=MoltValue("none"),
                        metadata={"var": name},
                    )
                )
        free_base = len(declaration.slots()) - len(declaration.freevars)
        for index, name in enumerate(declaration.freevars):
            cell = self._load_free_var_cell(name)
            if cell is None:
                raise FrontendRejection(
                    Diagnostic.INTERNAL_INVARIANT,
                    f"free variable {name!r} of {self.current_func_name} "
                    "has no closure cell",
                )
            self._emit_frame_home_store(
                name, cell, kind="FRAME_HOME_CELL", slot=free_base + index
            )

    def _capture_expression_reference(self, value: MoltValue) -> MoltValue:
        """Capture a borrowed expression before its storage can be released."""
        return self._emit_owned_value_alias(value) if value.borrows_binding else value

    def _emit_owned_value_alias(self, value: MoltValue) -> MoltValue:
        exact_class = self._exact_class_for_value(value)
        retained = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="BINDING_ALIAS", args=[value], result=retained))
        if exact_class is not None:
            self._stamp_exact_class(retained, exact_class)
        return retained

    def _capture_class_import_state(self) -> dict[str, object]:
        """Isolate lexical import projections while executing a class body."""
        saved: dict[str, object] = {}
        for attr in (*FUNCTION_IMPORT_RESOLUTION_STATE_ATTRS, "_typing_import_aliases"):
            value = getattr(self, attr)
            saved[attr] = value
            if attr == "_module_provenance_flow_stack":
                # Class-local paths are not enclosing-function/module bindings.
                # Explicit global publications are projected on restoration.
                setattr(self, attr, [])
            elif isinstance(value, (dict, set, list)):
                setattr(self, attr, value.copy())
            else:
                raise AssertionError(f"unsupported lexical import state: {attr}")
        return saved

    def _restore_class_import_state(
        self,
        saved: dict[str, object],
        global_names: frozenset[str],
        nonlocal_names: frozenset[str] = frozenset(),
    ) -> None:
        """Restore lexical identity, retaining explicit class-global publications."""
        for attr, value in saved.items():
            setattr(self, attr, value)
        for name in sorted(global_names):
            if not self._binding_targets_module_namespace(name):
                # A class global cannot replace an enclosing function's local.
                continue
            _restore_binding_projection(
                self.imported_modules, self.global_imported_modules, name
            )
            _restore_binding_projection(
                self.imported_module_provenance,
                self.global_imported_module_provenance,
                name,
            )
            _restore_binding_projection(
                self.imported_names, self.global_imported_names, name
            )
            _restore_binding_projection(
                self.imported_attr_names, self.global_imported_attr_names, name
            )
            self.local_imported_modules.discard(name)
            self.local_imported_names.discard(name)
            self._typing_import_aliases.discard(name)
            if self.global_imported_modules.get(name) in {
                "typing",
                "typing_extensions",
            }:
                self._typing_import_aliases.add(name)
        for name in sorted(nonlocal_names):
            # The body may replace the enclosing cell conditionally or through
            # callbacks. Restoring its pre-class import origin would authorize
            # dispatch through a stale module. Neither a last-visited body map
            # nor the old outer map proves the post-class binding's identity.
            self._clear_imported_module_binding(name)
            self.imported_names.pop(name, None)
            self.imported_attr_names.pop(name, None)
            self.local_imported_names.discard(name)
            self._typing_import_aliases.discard(name)
        self._record_module_provenance_flow_state()

    def _binding_targets_module_namespace(self, name: str) -> bool:
        """Resolve publication ownership before consulting enclosing-frame flags."""
        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None:
            return name in class_scope.global_names
        return self.current_func_name == "molt_main" or name in self.global_decls

    def _publish_import_binding(self, name: str, value: MoltValue) -> None:
        """Publish an imported value exactly once to its Python binding owner."""
        self.exact_locals.pop(name, None)
        module_owned = self._binding_targets_module_namespace(name)
        if self._active_class_ns_scope(name) is not None:
            self._store_local_value(name, value)
            if module_owned:
                self.module_global_mutations.add(name)
                self.globals[name] = value
            return
        if module_owned and self.current_func_name == "molt_main":
            self.module_global_mutations.add(name)
            self.globals[name] = value
        self._store_local_value(name, value, publish_module=True)

    def _publish_definition_binding(self, name: str, value: MoltValue) -> None:
        """Publish a definition once without inventing an unboxed local cache."""
        if self._active_class_ns_scope(name) is not None:
            self._store_local_value(name, value)
            return
        if self.current_func_name == "molt_main":
            self.globals[name] = value
            if name not in self.boxed_locals:
                self._emit_module_attr_set(name, value)
                return
        self._store_local_value(name, value, publish_module=True)

    def _store_local_value(
        self,
        name: str,
        value: MoltValue,
        *,
        publish_module: bool = False,
    ) -> None:
        exact_class = self._exact_class_for_value(value)
        self._invalidate_loop_guard(name)
        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None:
            self._class_ns_store(class_scope, name, value)
            return
        if name in self.comp_shadow_locals:
            self._store_comprehension_local_value(name, value)
            return
        if self.current_func_name != "molt_main" and name in self.global_decls:
            self._emit_module_attr_set_runtime(name, value)
            return
        if self.current_func_name == "molt_main":
            live_module_binding = name in self.module_global_mutations or (
                self.control_flow_depth > 0 and name in self.scope_assigned
            )
            if publish_module or live_module_binding:
                # Statement publication and mutable storage are one replacement,
                # not two writes. Compiler-private stores do not request public
                # publication; live control-flow bindings must never defer it.
                self._emit_module_attr_set(name, value, defer=not live_module_binding)
        if name in self.nonlocal_decls and name not in self.free_vars:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM, "nonlocal binding not found"
            )
        if name in self.free_vars or name in self.nonlocal_decls:
            if self._emit_free_var_store(name, value):
                return
        self._update_python_argument_zero(name, value)
        # Discard the name from the unbound-check set — at any flow
        # depth.  Within the current basic block, the assignment we're
        # about to emit dominates all subsequent loads of `name` until
        # the next flow boundary.  The flow visitors (visit_If,
        # visit_While, visit_For, visit_Try, visit_With,
        # visit_AsyncWith) snapshot `unbound_check_names` on entry and
        # restore on exit, so a name discarded inside a loop or branch
        # becomes "checked again" after the flow exits — the parent
        # path can't rely on the inner assignment having happened.
        # Inside the current scope (until the next flow boundary), the
        # discard eliminates the redundant `is missing → raise
        # UnboundLocalError` guard that would otherwise be emitted on
        # every subsequent load_var, which is the dominant per-iter
        # overhead in `obj = Class(...)` / `obj.x = …` / `obj.y = …`
        # loop bodies (bench_struct).
        if name in self.unbound_check_names:
            self.unbound_check_names.discard(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
            self._emit_cell_set(cell, value)
            if value.type_hint:
                self.boxed_local_hints[name] = value.type_hint
            return
        if self.is_async():
            slot = self._async_binding_slot(name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot.offset, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                if slot.role is AsyncFrameSlotRole.PUBLIC:
                    self.async_public_hints[name] = value.type_hint
                else:
                    self.async_internal_hints[name] = value.type_hint
            return
        # Do NOT cache in self.locals when the variable is module-backed
        # (in module_global_mutations). The canonical store is the module dict
        # and bare-name reads must go through MODULE_GET_GLOBAL to see the
        # latest value across loop iterations while keeping builtins fallback
        # and NameError-on-miss semantics. Without this guard, the stale local
        # SSA value shadows the module dict, causing while-loop conditions and
        # augmented assignments to read outdated values.
        if (
            self.current_func_name == "molt_main"
            and name in self.module_global_mutations
        ):
            return
        if value.name in self.bytearray_len_hints:
            self.bytearray_len_hints[name] = self.bytearray_len_hints[value.name]
        else:
            self.bytearray_len_hints.pop(name, None)
        if self.frame_home_slots is None:
            # Module code: the module namespace is the binding; this is its
            # lexical projection. Nothing was displaced here, so the producer
            # fact survives emission's coarse heap effect.
            self.locals[name] = value
            if exact_class is not None:
                self._stamp_exact_class(value, exact_class)
                self._publish_exact_local(name, exact_class)
            return
        # The binding's home takes the value's reference and releases the
        # binding it displaces, as STORE_FAST does. The frame carries the
        # store's view; reads borrow it while no write can intervene.
        view = self._emit_frame_home_store(name, value)
        self.locals[name] = view
        if name in self.scope_assigned:
            # The view's SSA transport across the body's control flow.
            self.emit(
                MoltOp(
                    kind="STORE_VAR",
                    args=[view],
                    result=MoltValue("none"),
                    metadata={"var": name},
                )
            )

    def _emit_delete_local_value(self, name: str, missing: MoltValue) -> None:
        """``del name`` of a synchronous frame's plain local: its home is left
        unbound and releases the binding. No missing value is carried: the
        caller marks the name possibly unbound, so a later read loads the home.
        ``missing`` only keeps the name's lexical entry."""
        self._invalidate_loop_guard(name)
        self.bytearray_len_hints.pop(name, None)
        self._emit_frame_home_clear(name)
        self.locals[name] = missing

    def _load_comprehension_slot(self, binding: ComprehensionBinding) -> MoltValue:
        value = MoltValue(
            self.next_var(), type_hint="cell" if binding.is_cell else binding.type_hint
        )
        if binding.async_slot is not None:
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", binding.async_slot.offset],
                    result=value,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="LOAD_VAR",
                    args=[],
                    result=value,
                    metadata={"var": binding.variable_slot},
                )
            )
        return value

    def _store_comprehension_slot(
        self, binding: ComprehensionBinding, value: MoltValue
    ) -> None:
        if binding.async_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", binding.async_slot.offset, value],
                    result=MoltValue("none"),
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="STORE_VAR",
                    args=[value],
                    result=MoltValue("none"),
                    metadata={"var": binding.variable_slot},
                )
            )

    @contextmanager
    def _comprehension_scope(
        self, node: ast.ListComp | ast.SetComp | ast.DictComp
    ) -> Iterator[None]:
        """Fresh PEP 709 locals; caller storage survives normal and exceptional exit.

        In a synchronous frame each scope binding takes over its name's home:
        the enclosing binding is moved out on entry and stored back on both
        exits, the exceptional one with its exception pending.
        """
        names = {
            name
            for comp in node.generators
            for name in self._collect_target_names(comp.target)
        }
        captured = set(self._collect_comprehension_cell_vars(node))
        old_bindings = self.comprehension_bindings
        old_shadow = self.comp_shadow_locals
        old_locals = {name: self.locals.get(name) for name in names}
        old_unbound = self.unbound_check_names & names
        homes = self._comprehension_binds_homes()
        restore_kinds = (
            {name: self._frame_home_restore_kind(name, old_bindings) for name in names}
            if homes
            else {}
        )
        home_scopes: list[FrameRestoreScope] = []
        restore_projections = (
            self._mask_exact_binding_projection(names),
            _mask_binding_projection(self.boxed_local_hints, names),
            _mask_binding_projection(self.explicit_type_hints, names),
            _mask_binding_projection(self.container_elem_hints, names),
            _mask_binding_projection(self.dict_key_hints, names),
            _mask_binding_projection(self.dict_value_hints, names),
            _mask_binding_projection(self.bytearray_len_hints, names),
            _mask_binding_projection(self.imported_names, names),
            _mask_binding_projection(self.imported_attr_names, names),
            _mask_binding_projection(self.imported_modules, names),
            _mask_binding_projection(self.imported_module_provenance, names),
        )
        self.comprehension_bindings = dict(old_bindings)
        self.comp_shadow_locals = old_shadow | names
        frame_scope = None
        try:
            for name in sorted(names):
                if homes:
                    # Move the enclosing binding out. Every exit taken after
                    # the move, a failed move of a later name included, puts
                    # it back.
                    saved = self._emit_frame_home_take(name)
                    home_scopes.append(
                        self._enter_frame_restore_scope(
                            partial(
                                self._emit_frame_home_store,
                                name,
                                saved,
                                kind=restore_kinds[name],
                            )
                        )
                    )
                missing = self._emit_missing_value()
                value = missing
                is_cell = name in captured
                if is_cell:
                    value = self._emit_cell_new(missing)
                    if homes:
                        value = self._emit_frame_home_store(
                            name, value, kind="FRAME_HOME_CELL"
                        )
                slot = (
                    self._allocate_async_frame_slot(AsyncFrameSlotRole.SCRATCH)
                    if self.is_async()
                    else None
                )
                binding = ComprehensionBinding(
                    variable_slot=self.next_var() if slot is None else None,
                    async_slot=slot,
                    is_cell=is_cell,
                )
                self.comprehension_bindings[name] = binding
                if is_cell or not homes:
                    # A plain binding in a home starts unbound there, as the
                    # take left it, with no missing transport: a read before
                    # its first store loads the home.
                    self._store_comprehension_slot(binding, value)
                # Lexical closure selection sees the source name, while every
                # actual read/write uses the scoped transport above.
                self.locals[name] = missing
            if (
                not homes
                and self.python_frame_context_active
                and isinstance(self.current_python_first_arg, str)
                and self.current_python_first_arg in names
            ):
                # A synchronous frame's argument zero is its first home, which
                # the scope's binding took over by itself.
                frame_scope = self._enter_python_frame_context_scope()
            yield
        finally:
            self.comprehension_bindings = old_bindings
            self.comp_shadow_locals = old_shadow
            self.unbound_check_names.difference_update(names)
            self.unbound_check_names.update(old_unbound)
            for restore in reversed(restore_projections):
                restore()
            for name, value in old_locals.items():
                if value is None:
                    self.locals.pop(name, None)
                else:
                    self.locals[name] = value
            if frame_scope is not None:
                self._exit_python_frame_context_scope(frame_scope)
            for scope in reversed(home_scopes):
                self._exit_frame_restore_scope(scope)

    def _store_comprehension_local_value(self, name: str, value: MoltValue) -> None:
        binding = self.comprehension_bindings.get(name)
        if binding is not None:
            self._invalidate_loop_guard(name)
            binding.type_hint = value.type_hint or "Any"
            binding.definitely_bound = True
            self._update_python_argument_zero(name, value)
            if binding.is_cell:
                cell = self._load_comprehension_slot(binding)
                self._emit_cell_set(cell, value)
            else:
                if binding.variable_slot is not None and self._comprehension_binds_homes():
                    # The scope's binding took over its name's home.
                    value = self._emit_frame_home_store(name, value)
                self._store_comprehension_slot(binding, value)
            self.locals[name] = value
            return
        self._invalidate_loop_guard(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
            self.locals[name] = value
            self._emit_cell_set(cell, value)
            if value.type_hint:
                self.boxed_local_hints[name] = value.type_hint
            return
        self.locals[name] = value
