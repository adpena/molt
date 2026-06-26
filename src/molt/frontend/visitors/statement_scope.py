"""StatementScopeVisitorMixin: module, global/nonlocal, import, and type-alias statements.

Move-only extraction from frontend/__init__.py. Keeps module assembly and import
binding statements together while assignment and control-flow statements live in
separate under-ceiling mixins.
"""

from __future__ import annotations

import ast

from typing import TYPE_CHECKING

from molt.frontend._types import (
    MoltOp,
    MoltValue,
)
from molt.frontend.sema import normalize_function_kind

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class StatementScopeVisitorMixin(_MixinBase):
    def visit_Module(self, node: ast.Module) -> None:
        defer = self._module_can_defer_attrs(node)
        if self.module_chunking:
            defer = False
        prev_defer = self.defer_module_attrs
        prev_dirty = self.deferred_module_attrs
        prev_stable = self.stable_module_funcs
        prev_mutated = self.mutated_classes
        prev_declared = self.module_declared_funcs
        prev_declared_classes = self.module_declared_classes
        prev_stable_classes = self.stable_module_classes
        prev_reserved = self.reserved_func_symbols
        prev_defined = self.module_defined_funcs
        prev_defaults = self.module_func_defaults
        prev_future = self.future_annotations
        prev_annotations = self.module_annotations
        prev_annotation_items = self.module_annotation_items
        prev_annotation_ids = self.module_annotation_ids
        prev_annotation_exec_map = self.module_annotation_exec_map
        prev_annotation_exec_name = self.module_annotation_exec_name
        prev_annotation_exec_counter = self.module_annotation_exec_counter
        prev_annotation_emitted = self.module_annotation_emitted
        prev_global_mutations = self.module_global_mutations
        prev_globals_dict_escaped = self.module_globals_dict_escaped
        prev_module_intrinsic_globals = self.module_intrinsic_globals
        prev_reserved_external = self.reserved_external_func_symbols
        prev_module_chunk_globals = self.module_chunk_globals
        prev_pending_classes = self.class_definition_pending
        self.stable_module_funcs = self._module_stable_funcs(node)
        self.mutated_classes = self._collect_module_class_mutations(node)
        # F2b (doc 44): the static class graph, const environment, and top-level
        # function metadata are now computed once, pre-walk, by frontend/sema/ and
        # the existing god-object dicts are populated FROM the immutable SemaResult.
        # This is the additive shim — the walk reads the same dicts, so the IR is
        # byte-identical; F2c rewires the read-sites onto SemaResult directly.
        self._populate_sema_state(node)
        self.stable_module_classes = self._collect_stable_module_classes(node)
        self.class_definition_pending = set(self.module_declared_classes)
        self.reserved_func_symbols = {}
        self.module_intrinsic_globals = self._collect_module_optional_intrinsic_globals(
            node
        )
        self.reserved_external_func_symbols = set(
            self.module_intrinsic_globals.values()
        )
        for func_name, kind in self.module_declared_funcs.items():
            if normalize_function_kind(kind) is not None:
                self._reserve_function_symbol(func_name)
        self.module_defined_funcs = set()
        # module_func_defaults is populated by _populate_sema_state above (the
        # AST-derived defaults from SemaResult, with the known_func_defaults
        # runtime override applied in the shim).
        self.future_annotations = self._module_has_future_annotations(node)
        self.module_annotations = None
        self.module_annotation_items = []
        self.module_annotation_ids = {}
        self.module_annotation_exec_map = None
        self.module_annotation_exec_name = None
        self.module_annotation_exec_counter = 0
        self.module_annotation_emitted = False
        self.module_global_mutations = set()
        self.module_globals_dict_escaped = self._module_globals_dict_escapes(node)
        self.module_chunk_globals = set()
        self._ensure_globals_builtin()
        if not self.future_annotations and not self.eager_annotations:
            items, id_map = self._collect_module_annotation_items(node)
            if items:
                self.module_annotation_items = items
                self.module_annotation_ids = id_map
                self.module_annotation_exec_counter = len(items)
                self._ensure_module_annotation_exec_map()
                annotate_val = self._emit_annotate_function_obj(
                    items=list(self.module_annotation_items),
                    exec_map_name=self.module_annotation_exec_name,
                    stringize=False,
                )
                self.globals["__annotate__"] = annotate_val
                self.locals["__annotate__"] = annotate_val
                self._emit_module_attr_set("__annotate__", annotate_val)
                self.module_annotation_emitted = True
        if defer:
            self.defer_module_attrs = True
            self.deferred_module_attrs = set()
        self._emit_module_frame_enter(node)
        # Pre-scan for compile-time warnings (~bool, etc.) and emit
        # WARN_STDERR ops at module startup, before any print output.
        # This matches CPython which emits compile-time warnings before
        # executing any code.
        self._prescan_compile_warnings(node)
        self._emit_deferred_warnings()
        self.del_targets = self._collect_deleted_names(node.body)
        if self.module_chunking and self.module_chunk_max_ops > 0:
            wrapper_ops = self.current_ops
            wrapper_exception_label = self.function_exception_label
            wrapper_module_obj = self.module_obj
            module_param = self._module_chunk_param_value()

            def emit_wrapper(op: MoltOp) -> None:
                prev_ops = self.current_ops
                prev_label = self.function_exception_label
                prev_module_obj = self.module_obj
                self.current_ops = wrapper_ops
                self.function_exception_label = wrapper_exception_label
                self.module_obj = wrapper_module_obj
                self.emit(op)
                self.current_ops = prev_ops
                self.function_exception_label = prev_label
                self.module_obj = prev_module_obj

            def start_chunk() -> str:
                symbol = self._new_module_chunk_symbol()
                chunk_ops = self.funcs_map[symbol]["ops"]
                old_ops = self.funcs_map["molt_main"]["ops"]
                self.funcs_map["molt_main"]["ops"] = chunk_ops
                self._adjust_module_pressure_counts(
                    ops_delta=len(chunk_ops) - len(old_ops)
                )
                self.current_ops = chunk_ops
                self.function_exception_label = self.next_label()
                self.module_obj = module_param
                self._reset_module_chunk_state()
                return symbol

            def flush_chunk(symbol: str) -> None:
                self._emit_function_exception_handler(clear_handlers=True)
                old_ops = self.funcs_map["molt_main"]["ops"]
                self.funcs_map["molt_main"]["ops"] = wrapper_ops
                self._adjust_module_pressure_counts(
                    ops_delta=len(wrapper_ops) - len(old_ops)
                )
                call_out = MoltValue(self.next_var(), type_hint="Any")
                emit_wrapper(
                    MoltOp(
                        kind="CALL",
                        args=[symbol, wrapper_module_obj],
                        result=call_out,
                    )
                )

            current_chunk: str | None = None
            for stmt in node.body:
                if current_chunk is None:
                    current_chunk = start_chunk()
                elif (
                    self.module_chunk_max_ops > 0
                    and self.current_ops
                    and len(self.current_ops) + self._module_chunk_stmt_cost(stmt)
                    >= self.module_chunk_max_ops
                ):
                    flush_chunk(current_chunk)
                    current_chunk = start_chunk()
                self.visit(stmt)
                if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    self.module_defined_funcs.add(stmt.name)
                if isinstance(stmt, ast.ClassDef):
                    self.class_definition_pending.discard(stmt.name)
                if (
                    current_chunk is not None
                    and self.module_chunk_max_ops > 0
                    and len(self.current_ops) >= self.module_chunk_max_ops
                ):
                    flush_chunk(current_chunk)
                    current_chunk = None

            if current_chunk is not None:
                flush_chunk(current_chunk)

            old_ops = self.funcs_map["molt_main"]["ops"]
            self.funcs_map["molt_main"]["ops"] = wrapper_ops
            self._adjust_module_pressure_counts(
                ops_delta=len(wrapper_ops) - len(old_ops)
            )
            self.current_ops = wrapper_ops
            self.function_exception_label = wrapper_exception_label
            self.module_obj = wrapper_module_obj
        else:
            for stmt in node.body:
                self.visit(stmt)
                if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    self.module_defined_funcs.add(stmt.name)
                if isinstance(stmt, ast.ClassDef):
                    self.class_definition_pending.discard(stmt.name)
        if defer:
            self._flush_deferred_module_attrs()
        if (
            not self.future_annotations
            and not self.eager_annotations
            and self.module_annotation_items
            and not self.module_annotation_emitted
        ):
            annotate_val = self._emit_annotate_function_obj(
                items=list(self.module_annotation_items),
                exec_map_name=self.module_annotation_exec_name,
                stringize=False,
            )
            self.globals["__annotate__"] = annotate_val
            self.locals["__annotate__"] = annotate_val
            self._emit_module_attr_set("__annotate__", annotate_val)
        if self.current_func_name == "molt_main":
            self._emit_raise_if_pending(emit_exit=True, clear_handlers=True)
            complete_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=complete_val))
            self._emit_module_attr_set("__molt_module_complete__", complete_val)
            self.globals["__molt_module_complete__"] = complete_val
            self.locals["__molt_module_complete__"] = complete_val
            self._emit_module_frame_exit()
            self._emit_function_exception_handler(clear_handlers=True)
        self.defer_module_attrs = prev_defer
        self.deferred_module_attrs = prev_dirty
        self.stable_module_funcs = prev_stable
        self.mutated_classes = prev_mutated
        self.module_declared_funcs = prev_declared
        self.module_declared_classes = prev_declared_classes
        self.stable_module_classes = prev_stable_classes
        self.reserved_func_symbols = prev_reserved
        self.module_defined_funcs = prev_defined
        self.class_definition_pending = prev_pending_classes
        self.module_func_defaults = prev_defaults
        self.future_annotations = prev_future
        self.module_annotations = prev_annotations
        self.module_annotation_items = prev_annotation_items
        self.module_annotation_ids = prev_annotation_ids
        self.module_annotation_exec_map = prev_annotation_exec_map
        self.module_annotation_exec_name = prev_annotation_exec_name
        self.module_annotation_exec_counter = prev_annotation_exec_counter
        self.module_annotation_emitted = prev_annotation_emitted
        self.module_global_mutations = prev_global_mutations
        self.module_globals_dict_escaped = prev_globals_dict_escaped
        self.module_intrinsic_globals = prev_module_intrinsic_globals
        self.reserved_external_func_symbols = prev_reserved_external
        self.module_chunk_globals = prev_module_chunk_globals
        return None

    def visit_Global(self, node: ast.Global) -> None:
        if self.current_func_name == "molt_main":
            return None
        self.global_decls.update(node.names)
        return None

    def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
        if self.current_func_name == "molt_main":
            raise NotImplementedError("nonlocal declarations at module scope")
        for name in node.names:
            if name in self.global_decls:
                raise NotImplementedError("nonlocal conflicts with global declaration")
        self.nonlocal_decls.update(node.names)
        return None

    def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
        if self.current_func_name != "molt_main":
            raise NotImplementedError("Type aliases are only supported at module scope")
        if not isinstance(node.name, ast.Name):
            raise NotImplementedError("Unsupported type alias target")
        # Eagerly load typing._molt_type_alias before evaluating the alias
        # value.  This forces the typing module to be initialized prior to
        # any annotation expression evaluation, ensuring consistent runtime
        # state regardless of whether type_params trigger an earlier load.
        alias_fn = self._emit_module_attr_get_on("typing", "_molt_type_alias")
        type_param_vals, type_param_map = self._emit_type_params_values(
            node.type_params
        )
        prev_type_params = self.annotation_type_params
        if type_param_map:
            merged = dict(prev_type_params)
            merged.update(type_param_map)
            self.annotation_type_params = merged
        try:
            alias_value = self._emit_annotation_value(
                node.value, stringize=self.future_annotations
            )
        finally:
            self.annotation_type_params = prev_type_params
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[node.name.id], result=name_val))
        params_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=type_param_vals, result=params_tuple))
        alias_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[alias_fn, name_val, alias_value, params_tuple],
                result=alias_val,
            )
        )
        self.locals[node.name.id] = alias_val
        if self.current_func_name == "molt_main":
            self.globals[node.name.id] = alias_val
            self._emit_module_attr_set(node.name.id, alias_val)
        return None

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            module_name = alias.name
            if module_name in {"typing", "typing_extensions"}:
                # Track the alias so @<alias>.overload is recognised.
                if alias.asname:
                    self._typing_import_aliases.add(alias.asname)
                # Fall through — typing names have runtime significance.
            if module_name in self._STUB_IMPORT_MODULES:
                continue
            bind_name = alias.asname or module_name.split(".")[0]
            if self._source_imports_use_transaction():
                if alias.asname:
                    bound_val = self._emit_source_import_alias_binding(module_name)
                else:
                    bound_val = self._emit_source_import_transaction(
                        module_name,
                        fromlist_names=(),
                        level=0,
                    )
            else:
                module_val = self._emit_module_load_with_parents(module_name)
                if alias.asname:
                    bound_val = module_val
                else:
                    top_name = module_name.split(".")[0]
                    bound_val = self._emit_module_load(top_name)
            self.exact_locals.pop(bind_name, None)
            if self.current_func_name == "molt_main":
                self.module_global_mutations.add(bind_name)
                self.globals[bind_name] = bound_val
                if bind_name in self.boxed_locals:
                    self._store_local_value(bind_name, bound_val)
                else:
                    self.locals[bind_name] = bound_val
            else:
                self._store_local_value(bind_name, bound_val)
            self._emit_module_attr_set(bind_name, bound_val)
            self.imported_modules[bind_name] = module_name
            if self.current_func_name != "molt_main":
                self.local_imported_modules.add(bind_name)
            self.module_intrinsic_globals.pop(bind_name, None)
            if self.current_func_name == "molt_main":
                self.global_imported_modules[bind_name] = module_name
        return None

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        module_name = node.module
        transaction_name = node.module or ""
        transaction_level = node.level
        if node.level:
            resolved, error_kind = self._resolve_relative_import(
                node.module, node.level
            )
            if resolved is None:
                self._emit_relative_import_error(error_kind)
                return None
            module_name = resolved
        else:
            transaction_level = 0
        if module_name is None:
            raise self.compat.unsupported(
                node,
                "relative import missing module name",
                detail="from . import ...",
            )
        if module_name == "__future__":
            for alias in node.names:
                if alias.name == "annotations":
                    self.future_annotations = True
            return None
        if module_name in {"typing", "typing_extensions"}:
            # Typing names with runtime significance (TypeVar, Generic,
            # Protocol, Any, cast, etc.) must be loaded from the actual
            # typing module.  Fall through to the normal import path.
            pass
        if self._is_intrinsics_module_name(module_name):
            # _intrinsics is a synthetic runtime module whose
            # require_intrinsic/load_intrinsic calls are validated at compile
            # time and then resolved through the runtime intrinsic registry by
            # _try_lower_intrinsic_lookup_call. We skip the module load (it
            # would emit a static ImportError in AOT mode) but still record the
            # imported name so chunked stdlib modules keep the import origin
            # visible after module-state resets.
            for alias in node.names:
                if alias.name == "*":
                    continue
                bind_name = alias.asname or alias.name
                self.imported_names[bind_name] = module_name
                if self.current_func_name != "molt_main":
                    self.local_imported_names.add(bind_name)
                self.imported_attr_names[bind_name] = alias.name
                if self.current_func_name == "molt_main":
                    self.global_imported_names[bind_name] = module_name
                    self.global_imported_attr_names[bind_name] = alias.name
                    self.module_intrinsic_globals.pop(bind_name, None)
                # Direct calls like `_require_intrinsic("name")` are rewritten
                # to invoke the canonical runtime resolver by
                # `_try_lower_intrinsic_lookup_call`. When the imported name is
                # captured as a value (for example `_ri=_require_intrinsic` in a
                # default argument), bind it to that same runtime callable so
                # aliasing preserves the public two-argument API contract
                # (`name`, optional `namespace`).
                if alias.name in {
                    "require_intrinsic",
                    "_require_intrinsic",
                }:
                    bound_val = self._emit_runtime_function_with_none_defaults(
                        "molt_require_intrinsic_runtime",
                        2,
                        default_count=1,
                    )
                elif alias.name in {"load_intrinsic", "_load_intrinsic"}:
                    bound_val = self._emit_runtime_function_with_none_defaults(
                        "molt_load_intrinsic_runtime",
                        2,
                        default_count=1,
                    )
                elif alias.name == "runtime_active":
                    bound_val = self._emit_runtime_function(
                        "molt_runtime_active_runtime",
                        0,
                    )
                else:
                    bound_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=bound_val))
                self._store_local_value(bind_name, bound_val)
                self._emit_module_attr_set(bind_name, bound_val)
                if self.current_func_name == "molt_main":
                    self.module_global_mutations.add(bind_name)
                    self.globals[bind_name] = bound_val
                    self.locals.pop(bind_name, None)
                else:
                    self.locals[bind_name] = bound_val
            return None
        if module_name in self._STUB_IMPORT_MODULES:
            return None
        fromlist_names = tuple(alias.name for alias in node.names)
        if self._source_imports_use_transaction():
            module_val = self._emit_source_import_transaction(
                transaction_name,
                fromlist_names=fromlist_names,
                level=transaction_level,
            )
        else:
            module_val = self._emit_module_load_with_parents(module_name)
        for alias in node.names:
            if alias.name == "*":
                if self.current_func_name != "molt_main":
                    raise self.compat.unsupported(
                        node,
                        "import * only allowed at module level",
                        detail="from ... import *",
                    )
                if self.module_obj is None:
                    raise self.compat.unsupported(
                        node,
                        "import * requires module scope",
                        detail="module object missing",
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="MODULE_IMPORT_STAR",
                        args=[module_val, self.module_obj],
                        result=res,
                    )
                )
                return None
            attr_name = alias.name
            bind_name = alias.asname or attr_name
            attr_val = self._emit_module_import_from_value(module_val, attr_name)
            if module_name == "asyncio" and attr_name in {"run", "sleep"}:
                module_prefix = f"{self._sanitize_module_name(module_name)}__"
                attr_val.type_hint = f"Func:{module_prefix}{attr_name}"
            known_func_hint = self._known_module_function_type_hint(
                module_name, attr_name
            )
            if known_func_hint is not None:
                attr_val.type_hint = known_func_hint
            # Only update the import-origin binding when the source module is
            # resolvable (in known_modules, stdlib_allowlist, or at least
            # importable at runtime).  This prevents a try/except ImportError
            # fallback branch from overwriting a valid binding with an
            # unresolvable module (e.g. ``from _dummy_thread import get_ident``
            # clobbering the ``_thread`` binding established in the try body).
            _mod_resolvable = (
                not self.known_modules
                or module_name in self.known_modules
                or module_name in self.stdlib_allowlist
                or self._should_attempt_runtime_module_import(module_name)
                or bind_name not in self.imported_names
            )
            if _mod_resolvable:
                self.imported_names[bind_name] = module_name
                # Track the original attr name so cross-module call targets
                # resolve to the canonical function name, not the alias.
                # e.g. `from X import Y as Z` -> imported_attr_names["Z"] = "Y"
                self.imported_attr_names[bind_name] = attr_name
                if self.current_func_name != "molt_main":
                    self.local_imported_names.add(bind_name)
                if self.current_func_name == "molt_main":
                    self.global_imported_names[bind_name] = module_name
                    self.global_imported_attr_names[bind_name] = attr_name
                    self.module_intrinsic_globals.pop(bind_name, None)
            self.exact_locals.pop(bind_name, None)
            if self.current_func_name == "molt_main":
                self.module_global_mutations.add(bind_name)
                self.globals[bind_name] = attr_val
                if bind_name in self.boxed_locals:
                    self._store_local_value(bind_name, attr_val)
                else:
                    self.locals[bind_name] = attr_val
            else:
                self._store_local_value(bind_name, attr_val)
            self._emit_module_attr_set(bind_name, attr_val)
        return None
