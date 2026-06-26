"""ModuleLifecycleMixin: module metadata, frame tracing, and chunk boundaries.

Move-only extraction from frontend/__init__.py. These helpers own module object
metadata emission, module frame enter/exit, and top-level module chunk state.
Sibling visitor mixins call them through ``self.<method>`` via the generator MRO.
"""

from __future__ import annotations

import ast
from pathlib import Path
from typing import TYPE_CHECKING

from molt.frontend._types import (
    _BOOTSTRAP_TRACE_EXEMPT_MODULES,
    _MOLT_GLOBALS_BUILTIN,
    _MOLT_MODULE_CHUNK_PARAM,
    _MOLT_MODULE_CHUNK_PREFIX,
    FuncInfo,
    MoltOp,
    MoltValue,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ModuleLifecycleMixin(_MixinBase):
    def _emit_module_metadata(self) -> None:
        if self.module_obj is None:
            return
        path_obj: Path | None = None
        origin_val: MoltValue | None = None
        path_list_val: MoltValue | None = None
        if self.source_path:
            path_obj = Path(self.source_path)
            normalized = path_obj.as_posix()
            file_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[normalized], result=file_val))
            self._emit_module_attr_set_on(self.module_obj, "__file__", file_val)
            origin_val = file_val
        is_package = self.module_is_package
        spec_name = self.module_spec_name or self.module_name or ""
        package_name = self._spec_parent(spec_name, is_package)
        package_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[package_name], result=package_val))
        self._emit_module_attr_set_on(self.module_obj, "__package__", package_val)
        if is_package and path_obj is not None and not self.module_is_namespace:
            package_dir = path_obj.parent.as_posix()
            path_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[package_dir], result=path_val))
            list_val = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[path_val], result=list_val))
            self._emit_module_attr_set_on(self.module_obj, "__path__", list_val)
            path_list_val = list_val
        if (
            self.module_name == "importlib.machinery"
            or "importlib.machinery" not in self.known_modules
        ):
            spec_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=spec_none))
            self._emit_module_attr_set_on(self.module_obj, "__spec__", spec_none)
            return
        spec_name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(kind="CONST_STR", args=[self.module_spec_name], result=spec_name_val)
        )
        loader_default = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=loader_default))
        loader_val = self._emit_module_attr_get_default_on(
            "importlib.machinery", "MOLT_LOADER", loader_default
        )
        if origin_val is None:
            origin_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=origin_val))
        is_package_val = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[is_package], result=is_package_val))
        spec_cls = self._emit_module_attr_get_on("importlib.machinery", "ModuleSpec")
        spec_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[spec_cls, spec_name_val, loader_val, origin_val, is_package_val],
                result=spec_val,
            )
        )
        if path_list_val is not None:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[spec_val, "submodule_search_locations", path_list_val],
                    result=MoltValue("none"),
                )
            )
        self._emit_module_attr_set_on(self.module_obj, "__spec__", spec_val)

    def _emit_module_frame_enter(self, node: ast.Module) -> None:
        if (
            self.current_func_name != "molt_main"
            and not self.current_func_name.startswith("molt_init_")
        ) or self.module_frame_entered:
            return
        if self.module_name in _BOOTSTRAP_TRACE_EXEMPT_MODULES:
            return
        self.module_frame_entered = True
        code_id = self.module_frame_code_id
        if code_id is None:
            current_func = self.current_func_name
            code_id = self.func_code_ids.get(current_func)
            if code_id is None:
                code_id = self._register_code_symbol(current_func)
            self.module_frame_code_id = code_id
        if not self.module_frame_emitted:
            self.module_frame_emitted = True
            filename = self.source_path or "<unknown>"
            first_line = 1
            if node.body:
                first_line = int(getattr(node.body[0], "lineno", 1) or 1)
            file_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[filename], result=file_val))
            line_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[first_line], result=line_val))
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=["<module>"], result=name_val))
            linetable_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=linetable_val))
            varnames_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=varnames_val))
            names_vals: list[MoltValue] = []
            for code_name in self._collect_code_names_for_body(
                node.body,
                varnames=[],
                free_vars=[],
                module_scope=True,
            ):
                name_item = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[code_name], result=name_item))
                names_vals.append(name_item)
            names_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=names_vals, result=names_tuple))
            argcount_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=argcount_val))
            posonly_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=posonly_val))
            kwonly_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=kwonly_val))
            code_val = MoltValue(self.next_var(), type_hint="code")
            self.emit(
                MoltOp(
                    kind="CODE_NEW",
                    args=[
                        file_val,
                        name_val,
                        line_val,
                        linetable_val,
                        varnames_val,
                        names_tuple,
                        argcount_val,
                        posonly_val,
                        kwonly_val,
                    ],
                    result=code_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="CODE_SLOT_SET",
                    args=[code_val],
                    result=MoltValue("none"),
                    metadata={"code_id": code_id},
                )
            )
        self.emit(
            MoltOp(
                kind="TRACE_ENTER_SLOT",
                args=[code_id],
                result=MoltValue("none"),
            )
        )
        # Module-scope locals() must behave like globals(); pin the module dict on
        # the frame entry so builtins.locals/globals work even via getattr aliases.
        locals_dict = self._emit_globals_dict()
        self.emit(
            MoltOp(
                kind="FRAME_LOCALS_SET", args=[locals_dict], result=MoltValue("none")
            )
        )

    def _emit_module_frame_exit(self) -> None:
        if (
            (
                self.current_func_name != "molt_main"
                and not self.current_func_name.startswith("molt_init_")
            )
            or not self.module_frame_entered
            or self.module_frame_exited
        ):
            return
        self.module_frame_exited = True
        self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))

    def _function_needs_frame_trace(self, name: str | None = None) -> bool:
        func_name = self.current_func_name if name is None else name
        if func_name is None:
            return False
        if func_name == "molt_main" or func_name.startswith("molt_init_"):
            return False
        if func_name == _MOLT_GLOBALS_BUILTIN or func_name.endswith(
            f"__{_MOLT_GLOBALS_BUILTIN}"
        ):
            return False
        if name is not None and func_name not in self.funcs_map:
            return False
        return True

    def _module_chunk_param_value(self) -> MoltValue:
        return MoltValue(_MOLT_MODULE_CHUNK_PARAM, type_hint="module")

    def _new_module_chunk_symbol(self) -> str:
        self.module_chunk_counter += 1
        symbol = f"{self.module_prefix}{_MOLT_MODULE_CHUNK_PREFIX}_{self.module_chunk_counter}"
        while symbol in self.funcs_map:
            self.module_chunk_counter += 1
            symbol = f"{self.module_prefix}{_MOLT_MODULE_CHUNK_PREFIX}_{self.module_chunk_counter}"
        self.func_symbol_names[symbol] = "<module_chunk>"
        self._register_code_symbol(symbol)
        self.funcs_map[symbol] = FuncInfo(
            params=[_MOLT_MODULE_CHUNK_PARAM],
            param_types=[],
            return_hint=None,
            ops=self._new_tracked_ops(count_function=True),
        )
        self.module_chunk_symbols.append(symbol)
        return symbol

    def _reset_module_chunk_state(self) -> None:
        # Merge all module-level names defined so far into module_chunk_globals
        # so subsequent chunks can resolve them via MODULE_GET_GLOBAL.  This
        # covers class definitions, function definitions, imports, and plain
        # assignments — any name that was added to self.globals during prior
        # chunks.  Without this, names defined in chunk N but referenced in
        # chunk N+M would fall through to incorrect resolution paths (e.g.
        # stdlib_allowlist matching a variable alias against a module name).
        self.module_chunk_globals.update(self.globals.keys())
        self._reset_local_binding_state(
            reset_locals_cache=False,
            reset_del_targets=False,
        )
        self.globals = {}
        self._reset_import_resolution_state(reset_module_attr_mutations=False)
        # Clear the per-function module cache so that module references are
        # re-fetched via MODULE_CACHE_GET in each new chunk function.  Without
        # this, a cached MoltValue from a previous chunk's WASM locals would be
        # reused, but the corresponding WASM local does not exist in the new
        # chunk — leaving the variable at its zero-initialized default (0x0),
        # which is not a valid module object.
        self._reset_function_cache_state()
        self._reset_async_scope_state()
        self._reset_type_hint_scope_state(reset_bytearray_len=True)
        self.module_annotations = None
        self.module_annotations_conditional = True
        self.module_annotation_exec_map = None
        self._reset_control_flow_state(reset_function_exception_label=False)

    def _module_chunk_stmt_cost(self, stmt: ast.stmt) -> int:
        # Chunking decisions happen before lowering the next top-level
        # statement, so use a cheap AST-size heuristic to avoid letting one
        # expensive statement balloon the chunk that came before it.
        node_cost = sum(1 for _ in ast.walk(stmt)) * 3
        line_span = (
            max(1, getattr(stmt, "end_lineno", stmt.lineno) - stmt.lineno + 1)
            if getattr(stmt, "lineno", None) is not None
            else 1
        )
        span_cost = line_span * 20
        dominant = max(node_cost, span_cost)
        secondary = min(node_cost, span_cost)
        # Reserve headroom for lowering-time metadata expansion
        # (labels/check_exception/class wiring) so large statements start a
        # fresh chunk before they poison the preceding one.
        return max(1, dominant + secondary // 4)
