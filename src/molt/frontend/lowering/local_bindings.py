"""LocalBindingMixin: local, closure, class-namespace, and locals() storage.

Move-only extraction from frontend/__init__.py. Owns the generator's canonical
name-binding storage paths: boxed locals, free-var cells, class-body namespace
routing, unbound guards, plain-local ownership boundaries, and locals-cache
updates used by visitor mixins.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Sequence

from molt.frontend._types import (
    MoltOp,
    MoltValue,
    _ClassNsScope,
    _MOLT_CLOSURE_PARAM,
    _MOLT_LOCALS_CACHE,
    _STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class LocalBindingMixin(_MixinBase):
    def _emit_name_from_obj(self, obj: MoltValue) -> MoltValue:
        name_key = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__name__"], result=name_key))
        missing = self._emit_missing_value()
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(
                kind="GETATTR_NAME_DEFAULT",
                args=[obj, name_key, missing],
                result=name_val,
            )
        )
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[name_val, missing], result=is_missing))
        # Async/poll-function bodies must thread the result through a closure
        # slot rather than a LIST_NEW + STORE_INDEX cell. The cell pattern is
        # unsafe under Cranelift's loop-header phi resolver: the cell SSA
        # value can be merged with the entry-block default (None) on the
        # first iteration, producing store_index(None, ...) crashes.
        if self.is_async():
            slot = self._async_local_offset(f"__name_from_obj_{len(self.async_locals)}")
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
                self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
            )
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.async_locals[name]],
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
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[init], result=cell))
        self.boxed_locals[name] = cell
        if init.type_hint:
            self.boxed_local_hints[name] = init.type_hint
        else:
            self.boxed_local_hints[name] = "Unknown"
        self.locals[name] = cell
        if self.is_async():
            offset = self._async_local_offset(name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", offset, cell],
                    result=MoltValue("none"),
                )
            )

    def _load_boxed_cell(self, name: str) -> MoltValue | None:
        cell = self.boxed_locals.get(name)
        if cell is None:
            return None
        if not self.is_async():
            return cell
        if name not in self.async_locals:
            return cell
        slot_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", self.async_locals[name]],
                result=slot_val,
            )
        )
        return slot_val

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
        self, *, body: Sequence[ast.stmt], arg_nodes: Sequence[ast.arg]
    ) -> None:
        local_candidates = set(self.scope_assigned)
        local_candidates.update(arg.arg for arg in arg_nodes)
        local_candidates -= self.global_decls
        local_candidates -= self.nonlocal_decls
        if not local_candidates:
            return
        for name in sorted(self._collect_scope_cell_vars(body, local_candidates)):
            self._box_local(name)
            self.closure_locals.add(name)
        # Unify storage for names bound by both a comprehension walrus and a
        # non-comprehension assignment: box them so the inline-comprehension
        # cell and the SSA-local writer share one cell across loop back-edges.
        for name in self._collect_comp_walrus_shared_names(body):
            self._box_local(name)

    def _emit_free_var_load(
        self, name: str, *, guard_unbound: bool = True
    ) -> MoltValue | None:
        cell = self._load_free_var_cell(name)
        if cell is None:
            return None
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        hint = self.free_var_hints.get(name, "Any")
        res = MoltValue(self.next_var(), type_hint=hint)
        self.emit(MoltOp(kind="INDEX", args=[cell, zero], result=res))
        if guard_unbound:
            self._emit_unbound_free_guard(res, name)
        return res

    def _emit_free_var_store(self, name: str, value: MoltValue) -> bool:
        cell = self._load_free_var_cell(name)
        if cell is None:
            return False
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, zero, value],
                result=MoltValue("none"),
            )
        )
        return True

    def _load_free_var_cell(self, name: str) -> MoltValue | None:
        closure = self.locals.get(_MOLT_CLOSURE_PARAM)
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
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="INDEX", args=[closure, idx_val], result=cell))
        return cell

    def _push_loop_static_class_refs(self, body: list[ast.stmt]) -> None:
        refs: dict[str, MoltValue] = {}
        eager_refs: set[str] = set()
        for class_name in self._collect_loop_static_class_candidates(body):
            self.loop_static_class_counter += 1
            slot = f"__molt_static_class_{self.loop_static_class_counter}_{class_name}"
            init = self._emit_module_attr_get(
                class_name, effect_proof=_STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF
            )
            self.emit(
                MoltOp(
                    kind="STORE_VAR",
                    args=[init],
                    result=MoltValue("none"),
                    metadata={"var": slot},
                )
            )
            refs[class_name] = MoltValue(slot, type_hint="type")
            eager_refs.add(class_name)
        self.loop_static_class_refs.append(refs)
        self.loop_static_class_eager_refs.append(eager_refs)

    def _pop_loop_static_class_refs(self) -> None:
        if self.loop_static_class_refs:
            self.loop_static_class_refs.pop()
        if self.loop_static_class_eager_refs:
            self.loop_static_class_eager_refs.pop()

    def _module_globals_dict_escapes(self, node: ast.Module) -> bool:
        for child in ast.walk(node):
            if (
                isinstance(child, ast.Call)
                and isinstance(child.func, ast.Name)
                and child.func.id in {"globals", "vars"}
                and not child.args
                and not child.keywords
            ):
                return True
            if (
                isinstance(child, ast.Name)
                and isinstance(child.ctx, ast.Load)
                and child.id in {"globals", "vars"}
            ):
                return True
        return False

    def _class_id_from_call(self, node: ast.Call) -> str | None:
        if isinstance(node.func, ast.Name) and node.func.id in self.classes:
            return node.func.id
        return None

    def _builtin_exact_type_from_expr(self, value: ast.AST | None) -> str | None:
        if isinstance(value, (ast.Dict, ast.DictComp)):
            return "dict"
        if isinstance(value, (ast.List, ast.ListComp)):
            return "list"
        if isinstance(value, (ast.Set, ast.SetComp)):
            return "set"
        if isinstance(value, ast.Tuple):
            return "tuple"
        if isinstance(value, ast.Call) and isinstance(value.func, ast.Name):
            func_id = value.func.id
            if func_id in {"dict", "list", "set", "tuple"}:
                return func_id
            if (
                func_id in {"globals", "locals", "vars"}
                and not value.args
                and not value.keywords
            ):
                return "dict"
        return None

    def _update_exact_local(self, name: str, value: ast.AST | None) -> None:
        builtin_exact = self._builtin_exact_type_from_expr(value)
        if builtin_exact is not None:
            self.exact_builtin_locals[name] = builtin_exact
            self.exact_locals.pop(name, None)
            return
        if isinstance(value, ast.Call):
            class_id = self._class_id_from_call(value)
            if class_id is not None:
                class_info = self.classes.get(class_id)
                if (
                    class_info
                    and not class_info.get("dynamic")
                    and not class_info.get("dataclass")
                ):
                    self.exact_locals[name] = class_id
                    self.exact_builtin_locals.pop(name, None)
                    return
        if isinstance(value, ast.Name):
            if value.id in self.exact_locals and (
                self.current_func_name == "molt_main"
                or value.id not in self.global_decls
            ):
                self.exact_locals[name] = self.exact_locals[value.id]
                self.exact_builtin_locals.pop(name, None)
                return
            if value.id in self.exact_builtin_locals and (
                self.current_func_name == "molt_main"
                or value.id not in self.global_decls
            ):
                self.exact_builtin_locals[name] = self.exact_builtin_locals[value.id]
                self.exact_locals.pop(name, None)
                return
        self.exact_locals.pop(name, None)
        self.exact_builtin_locals.pop(name, None)

    def _propagate_func_type_hint(
        self, value_node: MoltValue, source_expr: ast.AST | None
    ) -> None:
        if not isinstance(source_expr, ast.Name):
            return
        source_info = self.locals.get(source_expr.id) or self.globals.get(
            source_expr.id
        )
        if source_info is None:
            return
        hint = source_info.type_hint
        if not isinstance(hint, str):
            return
        if hint.startswith(
            (
                "AsyncFunc:",
                "AsyncClosureFunc:",
                "AsyncGenFunc:",
                "AsyncGenClosureFunc:",
                "GenFunc:",
                "GenClosureFunc:",
            )
        ):
            symbol = hint.split(":")[1]
            base_symbol = (
                symbol[: -len("_poll")] if symbol.endswith("_poll") else symbol
            )
            if (
                base_symbol in self.func_default_specs
                or self._known_function_symbol_target(base_symbol) is not None
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
        # Internal lowering scaffolding (loop break flags, the locals cache, any
        # ``__molt_*`` temp) is NOT a Python-visible class-body name and must
        # never be threaded through the namespace mapping — it is pure SSA
        # plumbing.  Everything else (including dunders the body assigns) routes
        # through the class namespace.
        return not (
            name == _MOLT_LOCALS_CACHE
            or name == _MOLT_CLOSURE_PARAM
            or name.startswith("__molt_")
        )

    def _active_class_ns_scope(self, name: str) -> "_ClassNsScope | None":
        # The innermost class-body scope manages ``name`` when the body is being
        # lowered as a block.  A nested ``class``/``def`` pushes its own scope (or
        # a function frame), so only the top-of-stack entry — and only while we
        # are still emitting that class's body statements (``_class_body_depth``
        # tracks the active body) — is consulted.  Names that are loop/scaffold
        # temps are excluded so the SSA machinery handles them unchanged.
        if not self._class_ns_stack:
            return None
        if not self._is_class_body_managed_name(name):
            return None
        return self._class_ns_stack[-1]

    def _class_ns_store(
        self, scope: "_ClassNsScope", name: str, value: MoltValue
    ) -> None:
        # Bind a class-body name: snapshot the SSA value for the static fast path
        # AND, when a runtime namespace dict exists, publish it there so the dict
        # is the loop-carried-correct mutable home (and so a custom mapping's
        # ``__setitem__`` observes the store, matching CPython's class body).
        scope.names.add(name)
        scope.attr_values[name] = value
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

    def _class_ns_load(self, scope: "_ClassNsScope", name: str) -> MoltValue | None:
        # Read a class-body name.  When a runtime namespace dict backs the body
        # the dict is authoritative (it survives loop back-edges and reflects
        # mutations done through control flow), so read from it via INDEX.  With
        # no dict (a straight-line static body that never entered this path under
        # control flow) the SSA snapshot is exact.  A name this body never bound
        # returns None so ``visit_Name`` falls through to global/builtin
        # resolution — CPython's ``LOAD_NAME`` KeyError fallthrough.
        if name not in scope.names:
            return None
        if scope.ns is not None:
            hint = "Any"
            cached = scope.attr_values.get(name)
            if cached is not None and cached.type_hint:
                hint = cached.type_hint
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
            res = MoltValue(self.next_var(), type_hint=hint)
            self.emit(MoltOp(kind="INDEX", args=[scope.ns, key_val], result=res))
            return res
        return scope.attr_values.get(name)

    def _class_ns_delete(self, scope: "_ClassNsScope", name: str) -> None:
        # ``del name`` in a class body removes the binding from the namespace.
        # CPython raises NameError if the name is unbound; the binding-presence
        # check is the namespace dict's own ``__delitem__`` (which raises
        # KeyError -> NameError at the boundary).  Drop the SSA snapshot and,
        # when a dict backs the body, DEL_INDEX it (routing a custom mapping's
        # ``__delitem__``).
        scope.names.discard(name)
        scope.attr_values.pop(name, None)
        if scope.ns is not None:
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
            self.emit(
                MoltOp(
                    kind="DEL_INDEX",
                    args=[scope.ns, key_val],
                    result=MoltValue("none"),
                )
            )

    def _load_local_value(
        self, name: str, *, guard_unbound: bool = True
    ) -> MoltValue | None:
        # A class-body name resolves through the class namespace; but a name that
        # the class body has NOT bound (e.g. a parameter of a function inlined
        # into the body, like an inlined ``__init__``'s args, or an enclosing
        # local) must fall through to ordinary resolution.  Only short-circuit
        # when the active class scope actually owns ``name`` — otherwise continue
        # below so genuine locals/cells/globals still resolve.  (P0 #50.)
        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None and name in class_scope.names:
            return self._class_ns_load(class_scope, name)
        if name in self.comp_shadow_locals:
            return self.locals.get(name)
        if self.current_func_name != "molt_main" and name in self.global_decls:
            return self._emit_global_get(name)
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
            if guard_unbound and name in self.unbound_check_names:
                self._emit_unbound_local_guard(res, name)
            return res
        if self.is_async() and name in self.async_locals:
            offset = self.async_locals[name]
            res = MoltValue(
                self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
            )
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
            if guard_unbound and name in self.unbound_check_names:
                self._emit_unbound_local_guard(res, name)
            return res
        cached = self.locals.get(name)
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
            if guard_unbound and name in self.unbound_check_names:
                self._emit_unbound_local_guard(res, name)
            return res
        return cached

    def _plain_local_del_boundary_value(
        self, name: str, value: MoltValue | None
    ) -> MoltValue | None:
        if (
            value is None
            or value.name in ("none", "")
            or value.type_hint == "missing"
            or self.current_func_name == "molt_main"
            or name not in self.scope_assigned
            or name in self.closure_locals
            or name in self.boxed_locals
            or self.is_async()
        ):
            return None
        return value

    def _plain_local_del_boundary_enabled(
        self, name: str, value: MoltValue | None
    ) -> bool:
        return self._plain_local_del_boundary_value(name, value) is not None

    def _emit_plain_local_del_boundary(
        self, name: str, value: MoltValue | None
    ) -> None:
        boundary_source = self._plain_local_del_boundary_value(name, value)
        if boundary_source is None:
            return
        # `self.locals[name]` is the syntactic cached producer. Across loop
        # phis the current frame slot may be a block argument, so make the
        # boundary read the slot at the release point instead of retaining a
        # stale pre-loop SSA value.
        boundary_value = MoltValue(
            self.next_var(), type_hint=boundary_source.type_hint
        )
        self.emit(
            MoltOp(
                kind="LOAD_VAR",
                args=[],
                result=boundary_value,
                metadata={"var": name},
            )
        )
        self.emit(
            MoltOp(
                kind="DEL_BOUNDARY",
                args=[boundary_value],
                result=MoltValue("none"),
                metadata={"var": name},
            )
        )

    def _emit_plain_local_alias_retain(self, name: str, value: MoltValue) -> MoltValue:
        if (
            value.name in ("none", "")
            or value.type_hint == "missing"
            or self.current_func_name == "molt_main"
            or name not in self.scope_assigned
            or name in self.closure_locals
            or name in self.boxed_locals
            or self.is_async()
        ):
            return value
        producer = self._op_by_result.get(value.name)
        loaded_plain_local = False
        if producer is not None and producer.kind == "LOAD_VAR":
            source_name = (producer.metadata or {}).get("var")
            loaded_plain_local = (
                isinstance(source_name, str)
                and source_name != name
                and source_name in self.locals
                and source_name not in self.closure_locals
                and source_name not in self.boxed_locals
            )
        has_existing_binding = any(
            other_name != name and other_value.name == value.name
            for other_name, other_value in self.locals.items()
        )
        if not has_existing_binding and not loaded_plain_local:
            return value
        # `alias = local` gives the alias its own frame-owned reference in
        # CPython. Model that as a value-producing alias so TIR ownership sees a
        # distinct droppable root instead of a side-effect retain on shared bits.
        retained = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="BINDING_ALIAS", args=[value], result=retained))
        return retained

    def _plain_local_scope_exit_bindings(self) -> list[tuple[str, MoltValue]]:
        if (
            self.current_func_name == "molt_main"
            or self.is_async()
            or self.in_generator
        ):
            return []
        params = set(self.funcs_map.get(self.current_func_name, {}).get("params", []))
        candidate_names = sorted(set(self.scope_assigned) | params)
        bindings: list[tuple[str, MoltValue]] = []
        for name in candidate_names:
            if (
                name == _MOLT_CLOSURE_PARAM
                or name == _MOLT_LOCALS_CACHE
                or name.startswith("__molt_")
                or name in self.closure_locals
                or name in self.boxed_locals
                or name in self.global_decls
                or name in self.nonlocal_decls
            ):
                continue
            value = self.locals.get(name)
            if (
                value is None
                or value.name in ("none", "")
                or value.type_hint == "missing"
                or self._plain_local_scope_exit_boundary_exempt(value)
            ):
                continue
            bindings.append((name, value))
        return bindings

    @staticmethod
    def _plain_local_scope_exit_boundary_exempt(value: MoltValue) -> bool:
        hint = value.type_hint or ""
        return hint == "code"

    def _value_reads_plain_local_binding(
        self, value: MoltValue, bindings: list[tuple[str, MoltValue]]
    ) -> bool:
        if value.name in ("none", "") or value.type_hint == "missing":
            return False
        for _, bound_value in bindings:
            if bound_value.name == value.name:
                return True
        producer = self._op_by_result.get(value.name)
        if producer is not None and producer.kind == "LOAD_VAR":
            source_name = (producer.metadata or {}).get("var")
            return any(name == source_name for name, _ in bindings)
        return False

    def _emit_plain_local_scope_exit_boundaries(
        self, preserve: MoltValue | None = None
    ) -> None:
        bindings = self._plain_local_scope_exit_bindings()
        if not bindings:
            return
        if preserve is not None and self._value_reads_plain_local_binding(
            preserve, bindings
        ):
            self.emit(MoltOp(kind="INC_REF", args=[preserve], result=MoltValue("none")))
        for name, value in bindings:
            boundary_value = self._load_local_value(name, guard_unbound=False) or value
            self._emit_plain_local_del_boundary(name, boundary_value)

    def _store_local_value(
        self,
        name: str,
        value: MoltValue,
        *,
        emit_rebind_boundary: bool = True,
    ) -> None:
        def update_locals_cache() -> None:
            self._emit_locals_cache_update(name, value)

        self._invalidate_loop_guard(name)
        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None:
            self._class_ns_store(class_scope, name, value)
            return
        if name in self.comp_shadow_locals:
            self.locals[name] = value
            return
        if self.current_func_name != "molt_main" and name in self.global_decls:
            self._emit_module_attr_set_runtime(name, value)
            return
        if (
            self.current_func_name == "molt_main"
            and name in self.module_global_mutations
            and hasattr(self, "module_obj")
            and self.module_obj is not None
        ):
            self._emit_module_attr_set_on(self.module_obj, name, value)
        # Module-level stores inside loops must sync to the module dict
        # so that module_get_attr reads see the updated value on each iteration.
        if (
            self.current_func_name == "molt_main"
            and self.control_flow_depth > 0
            and hasattr(self, "module_obj")
            and self.module_obj is not None
            and not name.startswith("__molt_")
            and name in self.scope_assigned
        ):
            self._emit_module_attr_set_on(self.module_obj, name, value)
        if name in self.nonlocal_decls and name not in self.free_vars:
            raise NotImplementedError("nonlocal binding not found")
        if name in self.free_vars or name in self.nonlocal_decls:
            if self._emit_free_var_store(name, value):
                return
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
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, idx, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                self.boxed_local_hints[name] = value.type_hint
            update_locals_cache()
            return
        if self.is_async():
            if name not in self.async_locals:
                self._async_local_offset(name)
            offset = self.async_locals[name]
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", offset, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                self.async_local_hints[name] = value.type_hint
            update_locals_cache()
            return
        # Do NOT cache in self.locals when the variable is module-backed
        # (in module_global_mutations). The canonical store is the module dict
        # and reads must go through MODULE_GET_ATTR to see the latest value
        # across loop iterations. Without this guard, the stale local SSA
        # value shadows the module dict, causing while-loop conditions and
        # augmented assignments to read outdated values.
        if (
            self.current_func_name == "molt_main"
            and name in self.module_global_mutations
        ):
            update_locals_cache()
            return
        if value.name in self.bytearray_len_hints:
            self.bytearray_len_hints[name] = self.bytearray_len_hints[value.name]
        else:
            self.bytearray_len_hints.pop(name, None)
        if emit_rebind_boundary:
            value = self._emit_plain_local_alias_retain(name, value)
            previous = self.locals.get(name)
            if (
                previous is not None
                and previous.name != value.name
                and self._plain_local_del_boundary_enabled(name, previous)
            ):
                boundary_value = (
                    self._load_local_value(name, guard_unbound=False) or previous
                )
                self._emit_plain_local_del_boundary(name, boundary_value)
        self.locals[name] = value
        # Named-local fact (#58 ordering keystone): stamp `bound_local` on the
        # op that PRODUCED the bound value. CPython holds a named local in the
        # frame until `del`/rebinding/scope exit, so a finalizer-sensitive
        # value bound to a name must not be released at its SSA last-use; an
        # UNNAMED expression temp (e.g. `bag.append(A())`'s argument) dies at
        # the statement like CPython's stack ref. The IR otherwise erases this
        # distinction. Same condition as the named-local STORE_VAR below —
        # this is metadata on an already-emitted op, not a new op.
        if (
            self.current_func_name != "molt_main"
            and name in self.scope_assigned
            and value.name not in ("none", "")
        ):
            producer = self._op_by_result.get(value.name)
            if producer is not None:
                if producer.metadata is None:
                    producer.metadata = {}
                producer.metadata["bound_local"] = True
        # Emit explicit store_var for non-boxed function locals so TIR can
        # track variable mutations through loop iterations via SSA phis.
        if (
            self.current_func_name != "molt_main"
            and not self.is_async()
            and name in self.scope_assigned
            and name not in self.boxed_locals
        ):
            self.emit(
                MoltOp(
                    kind="STORE_VAR",
                    args=[value],
                    result=MoltValue("none"),
                    metadata={"var": name},
                )
            )
        update_locals_cache()

    def _emit_locals_cache_update(self, name: str, value: MoltValue) -> None:
        # Never include internal Molt scaffolding in `locals()`.
        if (
            name == _MOLT_LOCALS_CACHE
            or name == _MOLT_CLOSURE_PARAM
            or name.startswith("__molt_")
        ):
            return
        # Only include Python-visible locals (params + assigned names). The compiler
        # may introduce additional locals/temps for lowering; those must never leak
        # into `locals()` (CPython parity).
        params = self.funcs_map.get(self.current_func_name, {}).get("params", [])
        if name not in self.scope_assigned and name not in params:
            return
        cache = self.locals_cache_val
        if cache is None:
            return
        key = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=key))
        if value.type_hint == "missing":
            # Keep the pinned frame-locals cache in sync for `del`/unbound transitions.
            self.emit(
                MoltOp(
                    kind="DICT_UPDATE_MISSING",
                    args=[cache, key, value],
                    result=MoltValue("none"),
                )
            )
            return
        self.emit(
            MoltOp(
                kind="DICT_SET",
                args=[cache, key, value],
                result=MoltValue("none"),
            )
        )

    def _emit_delete_local_value(
        self, name: str, missing: MoltValue, old_value: MoltValue
    ) -> None:
        self._invalidate_loop_guard(name)
        self.bytearray_len_hints.pop(name, None)
        self.locals[name] = missing
        self.emit(
            MoltOp(
                kind="DELETE_VAR",
                args=[missing, old_value],
                result=MoltValue("none"),
                metadata={"var": name},
            )
        )
        self._emit_locals_cache_update(name, missing)

    def _store_comprehension_local_value(self, name: str, value: MoltValue) -> None:
        self._invalidate_loop_guard(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
            self.locals[name] = value
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, idx, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                self.boxed_local_hints[name] = value.type_hint
            return
        self.locals[name] = value
