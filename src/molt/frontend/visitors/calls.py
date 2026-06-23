"""CallVisitorMixin: call-expression lowering (F1 decomposition).

Move-only extraction from frontend/__init__.py (F1 phase 2). Covers visit_Call
and its exclusively-owned call/super/intrinsic/dataclass/print/format emission
helpers (every method here is, transitively, called only from within this
family). self.<method> / self.<attr> references resolve through the
SimpleTIRGenerator MRO at runtime.
"""

from __future__ import annotations

import ast
import sys

from collections.abc import Callable
from typing import (
    TYPE_CHECKING,
    Any,
    cast,
)

from molt.frontend._types import (
    BUILTIN_FUNC_SPECS,
    BUILTIN_TYPE_TAGS,
    ClassInfo,
    FormatParseState,
    INTRINSIC_HANDLE_CLASS_CONSTRUCTORS,
    MOLT_DIRECT_CALLS,
    MOLT_DIRECT_CALL_BIND_ALWAYS,
    MOLT_REEXPORT_FUNCTIONS,
    MethodInfo,
    MoltOp,
    MoltValue,
    STDLIB_DIRECT_CALL_MODULES,
    _InlineSuperFoldRequired,
    _MOLT_CLOSURE_PARAM,
    _MOLT_LOCALS_CACHE,
    _intrinsic_arity_exact,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


_BUILTINS_IMPORT_ALIAS_CALL_NAMES = frozenset(BUILTIN_FUNC_SPECS) | frozenset(
    {
        "BaseExceptionGroup",
        "ExceptionGroup",
        "abs",
        "aiter",
        "all",
        "anext",
        "any",
        "bool",
        "bytearray",
        "bytes",
        "callable",
        "chr",
        "classmethod",
        "complex",
        "delattr",
        "dict",
        "dir",
        "enumerate",
        "filter",
        "float",
        "frozenset",
        "getattr",
        "globals",
        "hasattr",
        "id",
        "int",
        "isinstance",
        "issubclass",
        "iter",
        "len",
        "list",
        "locals",
        "map",
        "max",
        "memoryview",
        "min",
        "next",
        "object",
        "open",
        "ord",
        "pow",
        "print",
        "property",
        "range",
        "repr",
        "reversed",
        "round",
        "set",
        "setattr",
        "slice",
        "sorted",
        "staticmethod",
        "str",
        "sum",
        "super",
        "tuple",
        "type",
        "vars",
        "zip",
    }
)


class CallVisitorMixin(_MixinBase):
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

    def _static_method_owner_after(
        self, mro: list[str], start: str, method: str
    ) -> str | None:
        """The first class defining ``method`` strictly after ``start`` in
        ``mro``, mirroring ``super(start, ...).method`` resolution.  Returns
        ``None`` if ``start`` is absent, no class after it defines ``method``,
        or any class after it is not statically resolvable (so its method set
        is unknown).
        """
        if start not in mro:
            return None
        for name in mro[mro.index(start) + 1 :]:
            if name == "object":
                # ``object`` defines a small fixed set; the methods exercised by
                # the fold (user methods) are never on ``object``, so reaching
                # object means "not found above" — bail rather than guess.
                return None
            info = self.classes.get(name)
            if info is None:
                # A class on the path whose method table we cannot see — bail.
                return None
            if method in info.get("methods", {}):
                return name
            # If this class is statically a subclass-graph node but absent from
            # the (dep-closure) class table, we cannot know whether it defines
            # the method; the caller restricts this to entry-module folding
            # where every class on a subclass MRO is in self.classes.
        return None

    def _visible_subclasses_of(self, class_name: str) -> list[str] | None:
        """All classes in the module class graph that have ``class_name`` in
        their static MRO (proper subclasses), or ``None`` if any candidate's
        MRO is not statically computable (fail-closed).
        """
        subclasses: list[str] = []
        for other in self.module_class_bases:
            if other == class_name:
                continue
            mro = self._static_mro_names(other)
            if mro is None:
                # Cannot prove this class is NOT a differently-ordered subclass.
                if class_name in self._reachable_base_names(other):
                    return None
                continue
            if class_name in mro:
                subclasses.append(other)
        return subclasses

    def _super_fold_is_sound(self, class_name: str, method: str) -> bool:
        """Soundness predicate for the static zero-arg ``super()`` fold of
        ``super().method(...)`` inside ``class_name.method``.

        The fold rewrites the call to a direct call on the first class defining
        ``method`` strictly after ``class_name`` in *``class_name``'s own* MRO.
        For a receiver whose runtime type is the subclass ``S``, CPython instead
        resolves to the first class defining ``method`` after ``class_name`` in
        ``S``'s MRO.  These agree for every possible receiver iff that
        successor-owner is identical across ``class_name`` and every subclass of
        ``class_name``.  Linear hierarchies always satisfy this; diamonds (where
        a subclass interposes a cooperative C3 sibling that defines ``method``)
        do not — that is the parity bug this guard fixes.

        Cross-module soundness: the frontend only sees this module's dependency
        closure, so a downstream module could subclass ``class_name`` invisibly.
        We therefore restrict the fold to the entry module, whose classes are
        import-closed (nothing imports the program entry point).  Within the
        entry module the static class graph (``module_class_bases``) is complete,
        including subclasses defined *after* ``class_name`` in source order.

        When the predicate returns ``False`` the fold bails and ``super()``
        lowers to the runtime super path, which the backend fuses into the
        allocation-free ``call_super_method_ic`` — already the fast path.
        """
        is_entry = self.module_name == "__main__" or (
            self.entry_module is not None and self.module_name == self.entry_module
        )
        if not is_entry:
            return False
        # The defining class must itself be a statically resolvable entry-module
        # class graph node.
        if class_name not in self.module_class_bases:
            return False
        own_mro = self._static_mro_names(class_name)
        if own_mro is None:
            return False
        expected_owner = self._static_method_owner_after(own_mro, class_name, method)
        if expected_owner is None:
            return False
        subclasses = self._visible_subclasses_of(class_name)
        if subclasses is None:
            return False
        for sub in subclasses:
            sub_mro = self._static_mro_names(sub)
            if sub_mro is None:
                return False
            sub_owner = self._static_method_owner_after(sub_mro, class_name, method)
            if sub_owner != expected_owner:
                # A subclass routes super() through a different class — the
                # static fold would skip a cooperative override. Do not fold.
                return False
        return True

    def _emit_nullcontext(self, payload: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="context_manager")
        self.emit(MoltOp(kind="CONTEXT_NULL", args=[payload], result=res))
        return res

    def _emit_closing(self, payload: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="context_manager")
        self.emit(MoltOp(kind="CONTEXT_CLOSING", args=[payload], result=res))
        return res

    def _emit_open_call(self, node: ast.Call) -> MoltValue:
        mode_expr = None
        if len(node.args) > 1:
            mode_expr = node.args[1]
        for kw in node.keywords:
            if kw.arg == "mode" and mode_expr is None:
                mode_expr = kw.value
        mode_hint = None
        if mode_expr is None:
            mode_hint = "file_text"
        elif isinstance(mode_expr, ast.Constant) and isinstance(mode_expr.value, str):
            mode_hint = "file_bytes" if "b" in mode_expr.value else "file_text"
        res = MoltValue(self.next_var(), type_hint=mode_hint or "file")
        callee = self._emit_builtin_function("open")
        callargs = self._emit_call_args_builder(node)
        self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
        return res

    @staticmethod
    def _is_gpu_intrinsic_call(node: ast.Call) -> str | None:
        """If *node* is a gpu.thread_id() / gpu.block_id() / etc., return the
        intrinsic name (e.g. ``"gpu_thread_id"``).  Otherwise return None."""
        _GPU_INTRINSICS = {
            "thread_id": "gpu_thread_id",
            "block_id": "gpu_block_id",
            "block_dim": "gpu_block_dim",
            "grid_dim": "gpu_grid_dim",
            "barrier": "gpu_barrier",
        }
        # gpu.thread_id()
        if (
            isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "gpu"
            and node.func.attr in _GPU_INTRINSICS
        ):
            return _GPU_INTRINSICS[node.func.attr]
        # bare thread_id() after `from molt.gpu import thread_id`
        if isinstance(node.func, ast.Name) and node.func.id in _GPU_INTRINSICS:
            return _GPU_INTRINSICS[node.func.id]
        return None

    def _emit_gpu_kernel_intrinsic_op(self, gpu_intrinsic: str) -> MoltValue:
        hint = "int" if gpu_intrinsic != "gpu_barrier" else "None"
        res = MoltValue(self.next_var(), type_hint=hint)
        self.emit(MoltOp(kind=gpu_intrinsic, args=[], result=res))
        return res

    def _parse_gpu_launch_config_expr(
        self, config_expr: ast.expr
    ) -> tuple[MoltValue, MoltValue] | None:
        default_threads = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[256], result=default_threads))
        if isinstance(config_expr, ast.Tuple):
            if len(config_expr.elts) == 0:
                return None
            grid = self.visit(config_expr.elts[0])
            if grid is None:
                return None
            if len(config_expr.elts) == 1:
                return grid, default_threads
            threads = self.visit(config_expr.elts[1])
            if threads is None:
                return None
            return grid, threads
        grid = self.visit(config_expr)
        if grid is None:
            return None
        return grid, default_threads

    def _lower_gpu_kernel_launch_call(self, node: ast.Call) -> MoltValue | None:
        if not isinstance(node.func, ast.Subscript):
            return None
        base = node.func.value
        if not isinstance(base, ast.Name):
            return None
        if base.id not in self.gpu_kernel_symbols_by_name:
            return None
        launcher = self.visit(base)
        if launcher is None:
            return None
        config = self._parse_gpu_launch_config_expr(node.func.slice)
        if config is None:
            return None
        grid, threads = config
        callargs = self._emit_call_args_builder(node)
        res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(
                kind="CALL",
                args=["molt_gpu_kernel_launch", launcher, grid, threads, callargs],
                result=res,
            )
        )
        return res

    def _function_symbol_for_reference(self, name: str) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None:
            return reserved
        return self._function_symbol(name)

    def _function_result_hint(self, func_symbol: str) -> str:
        info = self.funcs_map.get(func_symbol)
        hint = info.get("return_hint") if info is not None else None
        return hint or "Any"

    def _record_container_elem_hint(
        self, target: MoltValue, elem_hint: str | None
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
        if self.current_func_name == "molt_main":
            return self._emit_globals_dict()
        use_snapshot = sys.version_info >= (3, 13)
        if use_snapshot:
            res = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
        else:
            res = self._load_local_value_unchecked(_MOLT_LOCALS_CACHE)
            if res is None:
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
                self._store_local_value(_MOLT_LOCALS_CACHE, res)
        for name in sorted(self.locals):
            if name == _MOLT_CLOSURE_PARAM or name.startswith("__molt_"):
                continue
            value = self._load_local_value_unchecked(name)
            if value is None:
                continue
            key = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key))
            # Update the locals dict without emitting control-flow:
            # - value is `__molt_missing__` => delete key if present
            # - else => set key to value
            self.emit(
                MoltOp(
                    kind="DICT_UPDATE_MISSING",
                    args=[res, key, value],
                    result=MoltValue("none"),
                )
            )
        for name in sorted(self.free_vars):
            if name in self.locals:
                continue
            if name == _MOLT_CLOSURE_PARAM or name.startswith("__molt_"):
                continue
            cell = self._load_free_var_cell(name)
            if cell is None:
                continue
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            hint = self.free_var_hints.get(name, "Any")
            value = MoltValue(self.next_var(), type_hint=hint)
            self.emit(MoltOp(kind="INDEX", args=[cell, zero], result=value))
            key = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key))
            self.emit(
                MoltOp(
                    kind="DICT_UPDATE_MISSING",
                    args=[res, key, value],
                    result=MoltValue("none"),
                )
            )
        return res

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

    def _emit_function_defaults_version(self, func_obj: MoltValue) -> MoltValue:
        """Read the function object's __defaults__/__kwdefaults__ mutation
        version stamp (0 == never mutated since creation)."""
        res = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="FUNCTION_DEFAULTS_VERSION",
                args=[func_obj],
                result=res,
            )
        )
        return res

    def _emit_defaults_pristine_guard(self, func_obj: MoltValue) -> MoltValue:
        """Emit `func.__defaults_version__ == 0` as a bool guard.

        True iff neither `__defaults__` nor `__kwdefaults__` has been reassigned
        since the function object was created — i.e. a compile-time-baked literal
        default is still observably correct.  Any reassignment bumps the stamp
        (runtime: the generic function attribute setter), flipping this to False
        so the call deopts to a live `__defaults__`/`__kwdefaults__` read.
        """
        version = self._emit_function_defaults_version(func_obj)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        is_pristine = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[version, zero], result=is_pristine))
        return is_pristine

    def _emit_guarded_default_value(
        self,
        guard: MoltValue,
        baked_value: object,
        emit_live: Callable[[], MoltValue],
    ) -> MoltValue:
        """Select a default argument value under the pristine guard.

        `IF guard -> baked literal (compile-time constant, the fast/devirt path)
        ELSE -> emit_live() (a thunk that reads the live __defaults__ tuple /
        __kwdefaults__ dict) END_IF; PHI`.  Mirrors
        `_emit_guarded_field_get_with_guard`'s structured-conditional shape
        (portable across all backends).

        The baked literal is materialized INSIDE the fast arm and the live read
        is materialized INSIDE the slow arm, so the hot (never-mutated) path
        executes only the literal const — no `__defaults__` GETATTR/INDEX — which
        is the whole point of the devirtualization.  The PHI merges the two
        producers; the result type is `Any` unless both sides agree.
        """
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = self._emit_const_value(baked_value)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = emit_live()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        res_hint = (
            fast_val.type_hint if fast_val.type_hint == slow_val.type_hint else "Any"
        )
        merged = MoltValue(self.next_var(), type_hint=res_hint)
        self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
        return merged

    def _emit_function_defaults_tuple(self, func_obj: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[func_obj, "__defaults__"],
                result=res,
            )
        )
        return res

    def _emit_function_kwdefaults_dict(self, func_obj: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[func_obj, "__kwdefaults__"],
                result=res,
            )
        )
        return res

    def _emit_bound_method_func(self, method_obj: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[method_obj, "__func__"],
                result=res,
            )
        )
        return res

    def _apply_default_specs(
        self,
        total_params: int | None,
        default_specs: list[dict[str, Any]],
        args: list[MoltValue],
        node: ast.AST,
        *,
        call_name: str,
        func_obj: MoltValue | None = None,
        implicit_self: bool = False,
        positional_limit: int | None = None,
    ) -> list[MoltValue] | None:
        if total_params is None:
            return args
        arg_count = len(args) + (1 if implicit_self else 0)
        if positional_limit is not None and arg_count > positional_limit:
            return None
        if arg_count > total_params:
            return None
        missing = total_params - arg_count
        if missing <= 0:
            return args
        if missing > len(default_specs):
            return None
        base_index = len(default_specs) - missing
        specs_slice = default_specs[base_index : base_index + missing]
        has_const = any(spec.get("const", False) for spec in specs_slice)
        needs_tuple = any(
            not spec.get("const", False) and not spec.get("kwonly", False)
            for spec in specs_slice
        )
        needs_kwdefaults = any(
            not spec.get("const", False) and spec.get("kwonly", False)
            for spec in specs_slice
        )
        # The pristine guard: when ANY default is a compile-time-baked literal
        # AND we have a handle on the function object, read the live
        # __defaults__/__kwdefaults__ too and guard each baked literal with a
        # `defaults-version == 0` check.  This devirtualizes the common
        # (never-mutated) call to a direct CALL with inlined literals while
        # preserving CPython's call-time `__defaults__` binding: a runtime
        # `func.__defaults__ = (...)` reassignment bumps the version, flips the
        # guard, and deopts the baked value to the live read.  Without a
        # function object we cannot read the version or the live tuple, so the
        # literals are baked unguarded (the truly anonymous, not-reachable-by-
        # name case — its defaults cannot be mutated through a name binding).
        #
        # The guard uses a structured IF/ELSE/PHI conditional, sound only on the
        # phi-enabled, non-async lowering path (an async body threads merged
        # values through closure slots, not phis). When phis are unavailable the
        # const defaults fall through to the UNCONDITIONAL live read below
        # (always CPython-correct: it binds the live tuple/dict at call time),
        # just without the baked-literal fast path. Mutation semantics stay
        # correct everywhere; only the devirt fast path's speed is phi-gated.
        use_phi = self.enable_phi and not self.is_async()
        guard_const = has_const and func_obj is not None and use_phi
        # No-phi / async: route const defaults through the live read too (so they
        # observe a runtime mutation) instead of baking them.
        live_const_fallback = has_const and func_obj is not None and not use_phi
        # A positional const default's ELSE branch reads `__defaults__[idx]`; a
        # kwonly const default's ELSE branch reads `__kwdefaults__[name]`.  When
        # guarding (or live-fallback), ensure the corresponding live container is
        # materialized even if every default in this class happens to be const.
        const_or_guard = guard_const or live_const_fallback
        needs_tuple_live = needs_tuple or (
            const_or_guard
            and any(
                spec.get("const", False) and not spec.get("kwonly", False)
                for spec in specs_slice
            )
        )
        needs_kwdefaults_live = needs_kwdefaults or (
            const_or_guard
            and any(
                spec.get("const", False) and spec.get("kwonly", False)
                for spec in specs_slice
            )
        )
        defaults_tuple: MoltValue | None = None
        kwdefaults_dict: MoltValue | None = None
        pristine_guard: MoltValue | None = None
        if needs_tuple or needs_kwdefaults:
            if func_obj is None:
                raise self.compat.unsupported(
                    node,
                    f"call to {call_name} with non-constant defaults",
                    impact="medium",
                    alternative="pass explicit arguments",
                    detail="only literal defaults are supported for direct calls",
                )
        if func_obj is not None:
            if needs_tuple_live:
                defaults_tuple = self._emit_function_defaults_tuple(func_obj)
            if needs_kwdefaults_live:
                kwdefaults_dict = self._emit_function_kwdefaults_dict(func_obj)
            if guard_const:
                pristine_guard = self._emit_defaults_pristine_guard(func_obj)
        missing_val: MoltValue | None = None

        def _live_positional_default(spec_offset: int) -> MoltValue:
            assert defaults_tuple is not None
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[base_index + spec_offset], result=idx_val)
            )
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[defaults_tuple, idx_val], result=res))
            return res

        def _live_kwonly_default(spec: dict[str, Any]) -> MoltValue:
            nonlocal missing_val
            assert kwdefaults_dict is not None
            if missing_val is None:
                missing_val = self._emit_missing_value()
            key_name = spec.get("name")
            if not isinstance(key_name, str):
                raise NotImplementedError("Invalid kwonly default spec name")
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[key_name], result=key_val))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="DICT_GET",
                    args=[kwdefaults_dict, key_val, missing_val],
                    result=res,
                )
            )
            return res

        for offset, spec in enumerate(specs_slice):
            if spec.get("const", False):
                if live_const_fallback:
                    # No-phi / async: read the live default unconditionally so a
                    # runtime __defaults__/__kwdefaults__ mutation is observed
                    # (no baked literal, no guard branch).
                    if spec.get("kwonly", False):
                        args.append(_live_kwonly_default(spec))
                    else:
                        args.append(_live_positional_default(offset))
                    continue
                if pristine_guard is None:
                    # No function object: bake unguarded (anonymous callee whose
                    # __defaults__ is not reachable by name to be mutated).
                    args.append(self._emit_const_value(spec.get("value")))
                    continue
                # Guard the baked literal against a runtime defaults mutation.
                # The live read is a thunk so it is emitted only inside the
                # deopt (ELSE) arm — the fast path is purely the const literal.
                baked_value = spec.get("value")
                # Bind the loop-varying capture (`spec` / `offset`) as a default
                # parameter so each thunk reads its own default, not the final
                # iteration's — the same late-binding guard the prior lambdas had.
                if spec.get("kwonly", False):

                    def live_thunk(_spec: dict[str, Any] = spec) -> MoltValue:
                        return _live_kwonly_default(_spec)
                else:

                    def live_thunk(_off: int = offset) -> MoltValue:
                        return _live_positional_default(_off)

                args.append(
                    self._emit_guarded_default_value(
                        pristine_guard, baked_value, live_thunk
                    )
                )
                continue
            if spec.get("kwonly", False):
                if kwdefaults_dict is None:
                    raise self.compat.unsupported(
                        node,
                        f"call to {call_name} with non-constant defaults",
                        impact="medium",
                        alternative="pass explicit arguments",
                        detail="only literal defaults are supported for direct calls",
                    )
                args.append(_live_kwonly_default(spec))
                continue
            if defaults_tuple is None:
                raise self.compat.unsupported(
                    node,
                    f"call to {call_name} with non-constant defaults",
                    impact="medium",
                    alternative="pass explicit arguments",
                    detail="only literal defaults are supported for direct calls",
                )
            args.append(_live_positional_default(offset))
        return args

    def _apply_direct_call_defaults(
        self,
        module_name: str | None,
        func_id: str,
        args: list[MoltValue],
        node: ast.AST,
    ) -> list[MoltValue] | None:
        info = self._lookup_func_defaults(module_name, func_id)
        if info is None:
            return args
        if info.get("has_vararg"):
            return None
        total_params = info.get("params")
        defaults = info.get("defaults", [])
        kwonly_count = info.get("kwonly")
        if kwonly_count:
            return None
        positional_limit = None
        if total_params is not None and isinstance(kwonly_count, int):
            positional_limit = total_params - kwonly_count
        func_obj = None
        if total_params is not None:
            missing = total_params - len(args)
            # Load the function object whenever a trailing default is filled.
            # A non-const default needs the live `__defaults__`/`__kwdefaults__`
            # read; a CONST default needs the version stamp for the
            # `__defaults__`-mutation deopt guard (heals the module-level
            # baked-defaults divergence — a runtime `func.__defaults__ = (...)`
            # reassignment must be observed even for a literal def-site default).
            # `_apply_default_specs` bakes unguarded only when no function object
            # is available (a truly anonymous callee, not reachable by name).
            if 0 < missing <= len(defaults):
                resolved_module = module_name or self.module_name
                normalized = self._normalize_allowlist_module(resolved_module)
                if normalized is not None:
                    resolved_module = normalized
                if resolved_module == self.module_name:
                    func_obj = self._emit_module_attr_get(func_id)
                else:
                    func_obj = self._emit_module_attr_get_on(resolved_module, func_id)
        return self._apply_default_specs(
            total_params,
            defaults,
            args,
            node,
            call_name=func_id,
            func_obj=func_obj,
            positional_limit=positional_limit,
        )

    def _emit_direct_call_args(
        self, module_name: str | None, func_id: str, node: ast.Call
    ) -> list[MoltValue] | None:
        if node.keywords:
            # Keywords (including **kwargs) cannot be resolved at compile time
            # for direct calls — return None so the caller falls back to the
            # generic CALL_BIND / CALL_INDIRECT path which handles them at runtime.
            return None
        if (
            module_name is not None
            and self._lookup_func_defaults(module_name, func_id) is None
        ):
            return None
        args = self._emit_call_args(node.args)
        return self._apply_direct_call_defaults(module_name, func_id, args, node)

    def _emit_direct_call_args_for_symbol(
        self,
        func_symbol: str,
        node: ast.Call,
        func_obj: MoltValue | None = None,
    ) -> tuple[list[MoltValue] | None, MoltValue | None]:
        if node.keywords:
            # Keywords (including **kwargs) cannot be resolved at compile time
            # for direct calls — return None so the caller falls back to the
            # generic CALL_BIND / CALL_INDIRECT path which handles them at runtime.
            return None, func_obj
        # Check for vararg/kwarg BEFORE emitting call args; emitting args
        # has side effects (IR ops) that conflict with the CALL_BIND fallback
        # which emits its own arg builder.
        info = self.func_default_specs.get(func_symbol)
        known_symbol_target = self._known_function_symbol_target(func_symbol)
        if info is None:
            func_name = self.func_symbol_names.get(func_symbol)
            if func_name is not None:
                info = self._lookup_func_defaults(None, func_name)
            elif known_symbol_target is not None:
                module_name, func_name = known_symbol_target
                info = self._lookup_func_defaults(module_name, func_name)
        if info is not None and info.get("has_vararg"):
            return None, func_obj
        if info is not None and info.get("kwonly"):
            return None, func_obj
        args = self._emit_call_args(node.args)
        if info is None:
            return args, func_obj
        total_params = info.get("params")
        defaults = info.get("defaults", [])
        kwonly_count = info.get("kwonly")
        positional_limit = None
        if total_params is not None and isinstance(kwonly_count, int):
            positional_limit = total_params - kwonly_count
        if total_params is not None:
            missing = total_params - len(args)
            # Load the function object whenever a trailing default is filled: a
            # const default needs the version stamp for the `__defaults__`-
            # mutation deopt guard, a non-const default needs the live read.
            if 0 < missing <= len(defaults):
                if func_obj is None:
                    if known_symbol_target is not None:
                        module_name, func_name = known_symbol_target
                        func_obj = self._emit_module_attr_get_on(module_name, func_name)
                    else:
                        func_obj = self.visit(node.func)
        args = self._apply_default_specs(
            total_params,
            defaults,
            args,
            node,
            call_name=(
                known_symbol_target[1]
                if known_symbol_target is not None
                else self.func_symbol_names.get(func_symbol, func_symbol)
            ),
            func_obj=func_obj,
            positional_limit=positional_limit,
        )
        return args, func_obj

    @staticmethod
    def _known_module_func_kind(info: dict[str, Any] | None) -> str | None:
        if info is None:
            return None
        kind = info.get("kind")
        if kind == "async_gen":
            return "asyncgen"
        if kind in {"async", "asyncgen", "gen"}:
            return cast(str, kind)
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
            if raw_kind in {"async", "asyncgen", "gen"}:
                kind = raw_kind
        if kind is None:
            return None
        positional_only_kind_fact = info is None
        decorated = bool(info.get("has_decorators")) if info is not None else False
        result_hint = {
            "async": "Future",
            "asyncgen": "async_generator",
            "gen": "generator",
        }[kind]
        if positional_only_kind_fact:
            if needs_bind or node.keywords:
                return self._emit_call_bind_for_known_module_func(
                    node,
                    result_hint=result_hint,
                )
            args = self._emit_call_args(node.args)
            params = len(args)
        elif needs_bind or decorated or info.get("has_vararg"):
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
        include_gen_control = kind != "async"
        closure_size = self._task_closure_size(
            params,
            include_gen_control=include_gen_control,
        )
        if kind == "async":
            res = MoltValue(self.next_var(), type_hint="Future")
            self.emit(
                MoltOp(
                    kind="ALLOC_TASK",
                    args=[poll_func, closure_size] + args,
                    result=res,
                    metadata={"task_kind": "coroutine"},
                )
            )
            return res
        gen_val = MoltValue(self.next_var(), type_hint="generator")
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[poll_func, closure_size] + args,
                result=gen_val,
                metadata={"task_kind": "generator"},
            )
        )
        if kind == "gen":
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
        target_kind = self._lookup_func_kind(target_module, original_attr)
        known_direct_target = self._lookup_func_defaults(target_module, original_attr)
        has_known_direct_target = known_direct_target is not None
        known_info_kind = self._known_module_func_kind(known_direct_target)
        has_known_task_target = (
            target_kind not in {None, "sync"} or known_info_kind is not None
        )
        direct_target_is_linkable = self._is_linkable_module_function_symbol(
            target_module
        )
        allow_speculative_internal_direct = (
            not has_known_direct_target
            and target_kind in {None, "sync"}
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

    def _emit_dataclasses_field_call(
        self, module_name: str, node: ast.Call
    ) -> MoltValue:
        if any(kw.arg is None for kw in node.keywords):
            # Try to resolve **kwargs spreads from module-level constant dicts
            expanded: list[ast.keyword] = []
            all_resolved = True
            for kw in node.keywords:
                if (
                    kw.arg is None
                    and isinstance(kw.value, ast.Name)
                    and kw.value.id in self.module_const_dicts
                ):
                    for dk, dv in self.module_const_dicts[kw.value.id].items():
                        expanded.append(
                            ast.keyword(arg=dk, value=ast.Constant(value=dv))
                        )
                elif kw.arg is None:
                    # Dynamic **kwargs — cannot resolve at compile time.
                    # Fall through to emit CALLARGS_EXPAND_KWSTAR at runtime.
                    all_resolved = False
                    break
                else:
                    expanded.append(kw)
            if all_resolved:
                node.keywords = expanded
        if node.args:
            raise NotImplementedError("field does not support positional arguments")
        func_val = self._emit_module_attr_get_on(module_name, "field")
        callargs = self._emit_call_args_builder(node)
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CALL_BIND", args=[func_val, callargs], result=res))
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
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        res = MoltValue(self.next_var(), type_hint=type_hint)
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
        return res

    def _emit_stop_iteration_from_value(self, value: MoltValue) -> None:
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, none_val], result=is_none))
        # Async/poll-function bodies need a closure-slot result, not a list
        # cell. The cell SSA value can be merged with the entry-block default
        # by Cranelift's loop-header phi resolver, producing
        # store_index(None, ...) crashes (see _emit_guarded_field_get for the
        # full rationale).
        if self.is_async():
            slot = self._async_local_offset(
                f"__stop_iter_args_{len(self.async_locals)}"
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, none_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, empty_tuple],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            value_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[value], result=value_tuple))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, value_tuple],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            args_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=args_val))
        else:
            # Sync path: a single SSA value updated in both branches.
            args_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=args_val))
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
            self.emit(MoltOp(kind="COPY", args=[empty_tuple], result=args_val))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            value_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[value], result=value_tuple))
            self.emit(MoltOp(kind="COPY", args=[value_tuple], result=args_val))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["StopIteration"], result=kind_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW",
                args=[kind_val, args_val],
                result=exc_val,
            )
        )
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))

    def _static_expr_type_hint_without_emitting(self, expr: ast.expr) -> str | None:
        if isinstance(expr, ast.List):
            return "list"
        if isinstance(expr, ast.Tuple):
            return "tuple"
        if isinstance(expr, ast.Dict):
            return "dict"
        if isinstance(expr, ast.Set):
            return "set"
        if isinstance(expr, ast.Constant):
            if isinstance(expr.value, str):
                return "str"
            if isinstance(expr.value, bytes):
                return "bytes"
            if isinstance(expr.value, bool):
                return "bool"
            if isinstance(expr.value, int):
                return "int"
            if isinstance(expr.value, float):
                return "float"
            if expr.value is None:
                return "None"
        if not isinstance(expr, ast.Name):
            return None
        if self.is_async() and expr.id in self.async_local_hints:
            return self.async_local_hints[expr.id]
        boxed_hint = self.boxed_local_hints.get(expr.id)
        if boxed_hint is not None:
            return boxed_hint
        local_val = self.locals.get(expr.id)
        if local_val is not None:
            return local_val.type_hint
        global_val = self.globals.get(expr.id)
        if global_val is not None:
            return global_val.type_hint
        return None

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
            return self.locals.get(name)
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
        if self.is_async() and name in self.async_locals:
            offset = self.async_locals[name]
            res = MoltValue(
                self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
            )
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
        slot = self._spill_async_value(
            receiver, f"__recv_spill_{len(self.async_locals)}"
        )
        return receiver, slot

    def _emit_call_args(self, args: list[ast.expr]) -> list[MoltValue]:
        if not args:
            return []
        if not self.is_async():
            values: list[MoltValue] = []
            for expr in args:
                arg = self.visit(expr)
                if arg is None:
                    raise NotImplementedError("Unsupported call argument")
                values.append(arg)
            return values
        yield_flags = [self._expr_may_yield(expr) for expr in args]
        if not any(yield_flags):
            values = []
            for expr in args:
                arg = self.visit(expr)
                if arg is None:
                    raise NotImplementedError("Unsupported call argument")
                values.append(arg)
            return values
        values = []
        spills: list[tuple[int, int, str]] = []
        for idx, expr in enumerate(args):
            arg = self.visit(expr)
            if arg is None:
                raise NotImplementedError("Unsupported call argument")
            values.append(arg)
            if any(yield_flags[idx + 1 :]):
                slot = self._spill_async_value(
                    arg, f"__arg_spill_{len(self.async_locals)}"
                )
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

    @staticmethod
    def _call_needs_bind(node: ast.Call) -> bool:
        if node.keywords:
            return True
        return any(isinstance(arg, ast.Starred) for arg in node.args)

    def _emit_call_args_builder(self, node: ast.Call) -> MoltValue:
        items: list[tuple[str, ast.expr, str | None]] = []
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                items.append(("star", arg.value, None))
            else:
                items.append(("pos", arg, None))
        for kw in node.keywords:
            if kw.arg is None:
                items.append(("kwstar", kw.value, None))
            else:
                items.append(("kw", kw.value, kw.arg))
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        if not items:
            self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
            return callargs
        values: list[MoltValue] = []
        if not self.is_async():
            for _, expr, _ in items:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported call argument")
                values.append(val)
        else:
            yield_flags = [self._expr_may_yield(expr) for _, expr, _ in items]
            if not any(yield_flags):
                for _, expr, _ in items:
                    val = self.visit(expr)
                    if val is None:
                        raise NotImplementedError("Unsupported call argument")
                    values.append(val)
            else:
                spills: list[tuple[int, int, str]] = []
                for idx, (_, expr, _) in enumerate(items):
                    val = self.visit(expr)
                    if val is None:
                        raise NotImplementedError("Unsupported call argument")
                    values.append(val)
                    if any(yield_flags[idx + 1 :]):
                        slot = self._spill_async_value(
                            val, f"__arg_spill_{len(self.async_locals)}"
                        )
                        spills.append((idx, slot, val.type_hint))
                for idx, slot, hint in spills:
                    values[idx] = self._reload_async_value(slot, hint)
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        for (kind, _, name), val in zip(items, values):
            if kind == "pos":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="CALLARGS_PUSH_POS", args=[callargs, val], result=res)
                )
            elif kind == "star":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_STAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            elif kind == "kw":
                if name is None:
                    raise NotImplementedError("Keyword name is missing")
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_PUSH_KW",
                        args=[callargs, key_val, val],
                        result=res,
                    )
                )
            elif kind == "kwstar":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_KWSTAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            else:
                raise NotImplementedError("Unknown call argument kind")
        return callargs

    def _emit_print_call_args_builder(self, node: ast.Call) -> tuple[MoltValue, bool]:
        items: list[tuple[str, ast.expr, str | None]] = []
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                items.append(("star", arg.value, None))
            else:
                items.append(("pos", arg, None))
        for kw in node.keywords:
            if kw.arg is None:
                items.append(("kwstar", kw.value, None))
            else:
                items.append(("kw", kw.value, kw.arg))
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        if not items:
            self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
            return callargs, False
        values: list[MoltValue] = []
        saw_name_error = False
        if not self.is_async():
            for _, expr, _ in items:
                val = self.visit(expr)
                if val is None:
                    if isinstance(expr, ast.Name):
                        exc_val = self._emit_exception_new(
                            "NameError", f"name '{expr.id}' is not defined"
                        )
                        self.emit(
                            MoltOp(
                                kind="RAISE",
                                args=[exc_val],
                                result=MoltValue("none"),
                            )
                        )
                        saw_name_error = True
                        val = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                    else:
                        raise NotImplementedError("Unsupported call argument")
                values.append(val)
        else:
            yield_flags = [self._expr_may_yield(expr) for _, expr, _ in items]
            if not any(yield_flags):
                for _, expr, _ in items:
                    val = self.visit(expr)
                    if val is None:
                        if isinstance(expr, ast.Name):
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{expr.id}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            saw_name_error = True
                            val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    values.append(val)
            else:
                spills: list[tuple[int, int, str]] = []
                for idx, (_, expr, _) in enumerate(items):
                    val = self.visit(expr)
                    if val is None:
                        if isinstance(expr, ast.Name):
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{expr.id}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            saw_name_error = True
                            val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    values.append(val)
                    if any(yield_flags[idx + 1 :]):
                        slot = self._spill_async_value(
                            val, f"__arg_spill_{len(self.async_locals)}"
                        )
                        spills.append((idx, slot, val.type_hint))
                for idx, slot, hint in spills:
                    values[idx] = self._reload_async_value(slot, hint)
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        for (kind, _, name), val in zip(items, values):
            if kind == "pos":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="CALLARGS_PUSH_POS", args=[callargs, val], result=res)
                )
            elif kind == "star":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_STAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            elif kind == "kw":
                if name is None:
                    raise NotImplementedError("Keyword name is missing")
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_PUSH_KW",
                        args=[callargs, key_val, val],
                        result=res,
                    )
                )
            elif kind == "kwstar":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_KWSTAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            else:
                raise NotImplementedError("Unknown call argument kind")
        return callargs, saw_name_error

    def _emit_tuple_from_iter(self, iterable: MoltValue) -> MoltValue:
        items = self._emit_list_from_iter(iterable)
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_FROM_LIST", args=[items], result=res))
        return res

    def _emit_set_update_from_iter(
        self, target: MoltValue, iterable: MoltValue
    ) -> None:
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
        self.emit(MoltOp(kind="SET_ADD", args=[target, item], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

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

    def _builtin_str_single_object_arg(self, node: ast.AST) -> ast.AST | None:
        if not isinstance(node, ast.Call):
            return None
        if not (isinstance(node.func, ast.Name) and node.func.id == "str"):
            return None
        if len(node.args) > 1:
            return None
        kw_object: ast.AST | None = None
        for keyword in node.keywords:
            if keyword.arg != "object":
                return None
            if kw_object is not None:
                return None
            kw_object = keyword.value
        if node.args:
            return node.args[0]
        return kw_object

    def _lower_string_format_call(
        self, node: ast.Call, format_str: str
    ) -> MoltValue | None:
        if any(isinstance(arg, ast.Starred) for arg in node.args):
            return None
        kw_names: list[str] = []
        for keyword in node.keywords:
            if keyword.arg is None:
                return None
            kw_names.append(keyword.arg)
        if len(set(kw_names)) != len(kw_names):
            return None
        cache_key = (format_str, len(node.args), tuple(sorted(kw_names)))
        tokens = self.format_token_cache.get(cache_key)
        if tokens is None:
            state = FormatParseState()
            try:
                tokens = self._parse_format_tokens(
                    format_str,
                    len(node.args),
                    set(kw_names),
                    state,
                )
            except ValueError as exc:
                err_val = self._emit_exception_new("ValueError", str(exc))
                self.emit(
                    MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none"))
                )
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if tokens is None:
                return None
            self.format_token_cache[cache_key] = tokens
        args: list[MoltValue] = []
        for arg in node.args:
            value = self.visit(arg)
            if value is None:
                raise NotImplementedError("Unsupported format argument")
            args.append(value)
        kwargs: dict[str, MoltValue] = {}
        for keyword in node.keywords:
            value = self.visit(keyword.value)
            if value is None:
                raise NotImplementedError("Unsupported format argument")
            key = keyword.arg
            if key is None:
                raise NotImplementedError("Unsupported format argument")
            kwargs[key] = value
        return self._emit_format_tokens(tokens, args, kwargs)

    def _can_inline_sum_genexpr(self, node: ast.GeneratorExp) -> bool:
        if self.is_async():
            return False
        if not self._can_inline_simple_comp(node.generators, [node.elt]):
            return False
        comp = node.generators[0]
        if self._collect_inline_comp_walrus_names([node.elt], comp.ifs):
            return False
        target_names = set(self._collect_target_names(comp.target))
        lambda_free_vars = self._collect_inline_comp_lambda_free_vars(
            [node.elt], comp.ifs
        )
        return not bool(target_names & lambda_free_vars)

    @staticmethod
    def _sum_add_result_hint(acc: MoltValue, value: MoltValue) -> str:
        if acc.type_hint == "float" or value.type_hint == "float":
            return "float"
        if acc.type_hint in {"bool", "int"} and value.type_hint in {"bool", "int"}:
            return "int"
        return "Any"

    def _try_emit_inline_sum_genexpr(self, node: ast.Call) -> MoltValue | None:
        if (
            len(node.args) != 1
            or node.keywords
            or not isinstance(node.args[0], ast.GeneratorExp)
        ):
            return None
        genexpr = node.args[0]
        if not self._can_inline_sum_genexpr(genexpr):
            return None

        comp = genexpr.generators[0]
        target_name, tuple_target_names = self._inline_simple_comp_target(
            comp, "__molt_sum_genexpr_unpack"
        )
        user_target_names = (
            [target_name] if tuple_target_names is None else list(tuple_target_names)
        )
        saved_locals = {name: self.locals.get(name) for name in user_target_names}
        saved_boxed = {
            name: self.boxed_locals.pop(name, None) for name in user_target_names
        }
        saved_boxed_hints = {
            name: self.boxed_local_hints.pop(name, None) for name in user_target_names
        }
        outer_comp_shadow_locals = set(self.comp_shadow_locals)
        self.comp_shadow_locals.add(target_name)
        if tuple_target_names is not None:
            self.comp_shadow_locals.update(tuple_target_names)

        iterable_val = self.visit(comp.iter)
        if iterable_val is None:
            self.comp_shadow_locals = outer_comp_shadow_locals
            for name, boxed in saved_boxed.items():
                if boxed is not None:
                    self.boxed_locals[name] = boxed
            for name, hint in saved_boxed_hints.items():
                if hint is not None:
                    self.boxed_local_hints[name] = hint
            return None
        iter_obj = self._emit_iter_new(iterable_val)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))

        start_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
        acc_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[start_val], result=acc_cell))

        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        iter_elem_hint = self._iterable_element_hint(iterable_val) or "Any"
        item = MoltValue(self.next_var(), type_hint=iter_elem_hint)
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.locals[target_name] = item
        self._store_comprehension_local_value(target_name, item)
        if tuple_target_names is not None:
            item_vals = [
                MoltValue(self.next_var(), type_hint="Any") for _ in tuple_target_names
            ]
            self.emit(
                MoltOp(
                    kind="UNPACK_SEQUENCE",
                    args=[item] + item_vals,
                    result=MoltValue("none"),
                    metadata={"expected_count": len(tuple_target_names)},
                )
            )
            for tname, item_val in zip(tuple_target_names, item_vals):
                self._store_comprehension_local_value(tname, item_val)
        for if_node in comp.ifs:
            cond_val = self.visit(if_node)
            not_cond = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[cond_val], result=not_cond))
            self.emit(MoltOp(kind="IF", args=[not_cond], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        value = self.visit(genexpr.elt)
        if value is None:
            raise NotImplementedError("Unsupported sum generator expression")
        acc_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[acc_cell, zero], result=acc_val))
        acc_next = MoltValue(
            self.next_var(),
            type_hint=self._sum_add_result_hint(acc_val, cast(MoltValue, value)),
        )
        self.emit(MoltOp(kind="ADD", args=[acc_val, value], result=acc_next))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[acc_cell, zero, acc_next],
                result=MoltValue("none"),
            )
        )
        for name in user_target_names:
            prior = saved_locals.get(name)
            if prior is not None:
                self.locals[name] = prior
            else:
                self.locals.pop(name, None)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        result = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[acc_cell, zero], result=result))
        for name in user_target_names:
            boxed = saved_boxed.get(name)
            hint = saved_boxed_hints.get(name)
            if boxed is not None:
                self.boxed_locals[name] = boxed
            else:
                self.boxed_locals.pop(name, None)
            if hint is not None:
                self.boxed_local_hints[name] = hint
            else:
                self.boxed_local_hints.pop(name, None)
        self.comp_shadow_locals = outer_comp_shadow_locals
        return result

    def _emit_dynamic_call(
        self, node: ast.Call, callee: MoltValue, needs_bind: bool
    ) -> MoltValue:
        res_hint = "Any"
        if callee.type_hint.startswith("BoundMethod:"):
            parts = callee.type_hint.split(":", 2)
            if len(parts) == 3:
                class_name = parts[1]
                method_name = parts[2]
                method_info = (
                    self.classes.get(class_name, {}).get("methods", {}).get(method_name)
                )
                if method_info:
                    return_hint = method_info["return_hint"]
                    # Builtin scalar/container return types must propagate as
                    # type hints — without this, method calls returning `int`
                    # become type-erased `Any`, which forces the lane-inference
                    # pass to fall back to a NaN-boxed (effectively float-coerced)
                    # accumulator in tight loops like
                    # `total += obj.compute(i)`.
                    if return_hint and (
                        return_hint in self.classes or return_hint in BUILTIN_TYPE_TAGS
                    ):
                        res_hint = return_hint
        if needs_bind:
            callargs = self._emit_call_args_builder(node)
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_INDIRECT", args=[callee, callargs], result=res))
            return res
        if callee.type_hint.startswith("BoundMethod:"):
            args = self._emit_call_args(node.args)
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_METHOD", args=[callee] + args, result=res))
            return res
        if callee.type_hint.startswith("Func:"):
            func_symbol = callee.type_hint.split(":", 1)[1]
            args, _ = self._emit_direct_call_args_for_symbol(
                func_symbol, node, func_obj=callee
            )
            if args is None:
                callargs = self._emit_call_args_builder(node)
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="CALL_INDIRECT", args=[callee, callargs], result=res)
                )
                return res
            func_name = self.func_symbol_names.get(func_symbol)
            if func_name and func_name in self.globals:
                # Devirtualized call: check if callee is the expected function,
                # then call directly by symbol.  Falls back to INVOKE_FFI if
                # the identity check fails (e.g. function was rebound).
                #
                # Both branches write to the same output variable (`res`)
                # so the result is available after END_IF without an
                # intermediate list cell.  The old res_cell + STORE_INDEX
                # pattern broke in WASM because CHECK_EXCEPTION between
                # CALL and STORE_INDEX could skip the store, leaving None.
                expected = self._emit_module_attr_get(func_name)
                matches = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS", args=[callee, expected], result=matches))
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self.emit(MoltOp(kind="IF", args=[matches], result=MoltValue("none")))
                self.emit(MoltOp(kind="CALL", args=[func_symbol] + args, result=res))
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self.emit(
                    MoltOp(
                        kind="INVOKE_FFI",
                        args=[callee] + args,
                        result=res,
                    )
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                return res
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL", args=[func_symbol] + args, result=res))
            return res
        callargs = self._emit_call_args_builder(node)
        res = MoltValue(self.next_var(), type_hint=res_hint)
        self.emit(MoltOp(kind="CALL_INDIRECT", args=[callee, callargs], result=res))
        return res

    def _lower_statistics_slice_call(
        self, func_id: str, node: ast.Call
    ) -> MoltValue | None:
        if func_id not in {"mean", "stdev"}:
            return None
        if node.keywords or len(node.args) != 1:
            return None
        data_arg = node.args[0]
        if not isinstance(data_arg, ast.Subscript):
            return None
        data_slice = data_arg.slice
        if not isinstance(data_slice, ast.Slice):
            return None
        if data_slice.step is not None:
            return None
        seq = self.visit(data_arg.value)
        if seq is None:
            return None
        if data_slice.lower is None:
            start = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
            has_start = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=has_start))
        else:
            start = self.visit(data_slice.lower)
            if start is None:
                return None
            has_start = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
        if data_slice.upper is None:
            end = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
            has_end = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=has_end))
        else:
            end = self.visit(data_slice.upper)
            if end is None:
                return None
            has_end = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_end))
        kind = (
            "STATISTICS_MEAN_SLICE" if func_id == "mean" else "STATISTICS_STDEV_SLICE"
        )
        res = MoltValue(self.next_var(), type_hint="float")
        self.emit(
            MoltOp(
                kind=kind,
                args=[seq, start, end, has_start, has_end],
                result=res,
            )
        )
        return res

    def _try_emit_super_static_call(self, node: ast.Call) -> "MoltValue | None":
        """Phase 4a: fold `super().method(args)` to a direct CALL when the
        MRO is statically resolvable.  Returns the result MoltValue on
        success or None to signal the caller should fall through to the
        general dispatch path.

        Bails out (returns None) on any of:
          - `node.func` is not `Attribute(Call(super, []), method)`
          - super() has args / kwargs (typed-super: out of scope)
          - current method's class or first-parameter is unknown
          - MRO walk doesn't find the method (dispatch error path)
          - method is a property/classmethod/staticmethod descriptor
          - method has *args/**kwargs/closure/defaults (defer to general
            path which handles default-binding correctly)
          - call site has *args/**kwargs/keyword args (the static fold
            is positional-only)
        """
        if not isinstance(node.func, ast.Attribute):
            return None
        super_call = node.func.value
        if not isinstance(super_call, ast.Call):
            return None
        if not isinstance(super_call.func, ast.Name) or super_call.func.id != "super":
            return None
        if super_call.args or super_call.keywords:
            # typed super(T, obj) — leave to general path.  But if we are
            # inlining a ``__class__``-cell method, the runtime super path the
            # general path takes has no cell in the caller's spliced scope, so
            # the inline must be aborted (caller falls back to dispatch).
            if self._inline_super_must_fold:
                raise _InlineSuperFoldRequired
            return None
        current_class = self.current_class
        current_first_param = self.current_method_first_param
        if current_class is None or current_first_param is None:
            if self._inline_super_must_fold:
                raise _InlineSuperFoldRequired
            return None
        if node.keywords:
            if self._inline_super_must_fold:
                raise _InlineSuperFoldRequired
            return None  # kwargs hit defaults / kwonly machinery
        for arg in node.args:
            if isinstance(arg, (ast.Starred,)):
                if self._inline_super_must_fold:
                    raise _InlineSuperFoldRequired
                return None  # *args spread — needs builder
        method_name = node.func.attr
        folded = self._fold_bare_super_static(
            node, method_name, current_class, current_first_param
        )
        if folded is None and self._inline_super_must_fold:
            # A bare ``super().method()`` that did not fold while inlining a
            # ``__class__``-cell method: the general dispatch fallback would
            # emit a runtime super into a scope with no ``__class__`` cell.
            # Abort the inline so the caller routes through the dispatch path.
            raise _InlineSuperFoldRequired
        return folded

    def _fold_bare_super_static(
        self,
        node: ast.Call,
        method_name: str,
        current_class: str,
        current_first_param: str,
    ) -> "MoltValue | None":
        """Fold a confirmed bare ``super().method(*positional)`` call to a
        direct CALL / inline when the MRO is statically resolvable.  Returns
        ``None`` to fall through to the general dispatch path.  The caller
        (``_try_emit_super_static_call``) has already validated the bare-super
        shape and handles the inline-abort policy.  It also proved
        ``self.current_class`` / ``self.current_method_first_param`` non-None
        and threads them in as ``current_class`` / ``current_first_param`` so
        the fold reads the narrowed values rather than the optional attributes.
        """
        # SOUNDNESS GATE for the static super fold.  ``super().method()`` in
        # ``current_class.method`` resolves to the first class defining
        # ``method`` after ``current_class`` in ``type(self).__mro__``.  Folding
        # statically picks that successor in ``current_class``'s *own* MRO, which
        # equals the runtime answer for every possible receiver only when the
        # successor-owner is identical across ``current_class`` and all of its
        # subclasses.  Linear hierarchies satisfy this; a diamond subclass
        # (``Final(Left, Right)`` interposing ``Right`` between ``Left`` and
        # ``Base``) does not — that is the parity bug.  ``_super_fold_is_sound``
        # verifies the successor-owner is stable across the whole entry-module
        # subclass graph (and bails for non-entry modules, whose subclasses may
        # be defined downstream and are invisible here).  When it bails, super()
        # lowers to the runtime path, which the backend fuses into the
        # allocation-free ``call_super_method_ic`` — already the fast path.
        if not self._super_fold_is_sound(current_class, method_name):
            return None
        method_info, owner_class = self._resolve_super_method_info(
            current_class, method_name
        )
        if method_info is None or owner_class is None:
            return None
        if method_info.get("descriptor") != "function":
            return None
        # A method whose closure is exactly the implicit ``__class__`` super
        # cell (``inline_closure_ok``) can still be folded — but ONLY via the
        # inline path below, which resolves its own ``super()`` chain statically
        # and never reads the cell.  A direct CALL to that closure symbol would
        # omit the cell argument, so ``target_is_closure`` forbids the direct
        # CALL fallback for these methods (they route to general dispatch on
        # inline failure).  A method with real captured locals is not foldable.
        target_is_closure = bool(method_info.get("has_closure"))
        if target_is_closure and not method_info.get("inline_closure_ok"):
            return None
        if method_info.get("has_vararg"):
            return None
        if method_info.get("has_varkw"):
            return None
        if method_info.get("kwonly_count"):
            return None
        defaults = method_info.get("defaults") or []
        # Param count includes self; call site provides only positional
        # args, so required positional count is param_count - 1.  We
        # require an exact match (no defaults filled) to keep this fold
        # purely structural — anything else routes through the general
        # path that knows how to evaluate default-spec expressions.
        param_count = method_info.get("param_count")
        if param_count is None:
            return None
        if defaults:
            return None
        expected_positional = param_count - 1  # exclude self
        if len(node.args) != expected_positional:
            return None

        self_val = self._load_local_value(current_first_param)
        if self_val is None and current_first_param in self.free_vars:
            self_val = self._emit_free_var_load(current_first_param)
        if self_val is None:
            return None

        call_args = [self.visit(a) for a in node.args]
        if any(a is None for a in call_args):
            return None

        # Phase 2 inline opportunity at the super-fold site: if the
        # MRO-resolved target method has a trivially inlinable body
        # (single Return of a constant/param/binop/etc. expression),
        # emit the body inline rather than a CALL.  This composes
        # with Phase 4a's recursive super-walk — `Leaf.compute` calls
        # `super().compute(x) * 2` which folds to `Mid.compute`,
        # whose body inlines to `super().compute(x) + 1` whose super
        # again folds to `Base.compute(x) = x`, fully unwinding the
        # super-chain at compile time.  This is the only foldable path
        # for a ``__class__``-cell closure target (see ``target_is_closure``).
        inlined = self._try_inline_method_call(method_info, self_val, call_args)
        if inlined is not None:
            return inlined

        # The MRO-resolved target carries the implicit ``__class__`` super cell
        # but did not inline (its body was not trivially inlinable): a direct
        # CALL to its closure symbol would omit the cell, so route to the
        # general dispatch path, which threads the real closure tuple.
        if target_is_closure:
            return None

        # Extract the method symbol from method_info["func"].type_hint
        # (format: "Func:<symbol>"), which was set when the ClassDef
        # compiled the method.  Calling `_function_symbol` here would
        # increment the collision counter and return a fresh dangling
        # symbol — same bug Phase 1 documents at line 16720.
        func_val = method_info.get("func")
        if func_val is None or not getattr(func_val, "type_hint", "").startswith(
            "Func:"
        ):
            return None
        method_symbol = func_val.type_hint.split(":", 1)[1]
        if method_symbol not in self.func_symbol_names:
            return None

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
                args=[method_symbol, self_val] + call_args,
                result=res,
            )
        )
        return res

    def _try_inline_method_call(
        self,
        method_info: MethodInfo,
        receiver: MoltValue,
        call_args: list[MoltValue],
    ) -> MoltValue | None:
        """Inline a Phase-1-direct-call into the current scope.

        Substitutes parameters → arg MoltValues in the locals map
        for the duration of visiting the inline-return-value AST,
        then restores.  Returns the resulting MoltValue, or None if
        inlining failed (caller should fall through to a regular CALL).
        """
        inline_return = method_info.get("inline_return")
        inline_params = method_info.get("inline_params")
        inline_owner = method_info.get("inline_owner_class")
        if inline_return is None or inline_params is None:
            return None
        # Fail-closed soundness gate (cross-module global mis-resolution).
        # The body is spliced into the *current* module's scope and re-lowered.
        # Any bare Name in the body that is not a substituted parameter or a
        # builtin (recorded in `inline_free_names`) resolves through
        # `visit_Name -> _emit_global_get` against the CURRENT module's globals.
        # When the method is defined in a *different* module, such a name is one
        # of the defining module's globals (e.g. `_MOLT_ARRAY_TOLIST`, a sibling
        # helper, a module-level constant) and would mis-resolve here — a silent
        # NameError at runtime.  Refuse the inline so the call site falls through
        # to a real CALL to the method symbol, which reads the defining module's
        # globals correctly (via the method body's own module-cache lookup).
        # Same-module inlines are unaffected: the current module IS the defining
        # module, so the global resolves to the same dict either way.
        inline_free_names = method_info.get("inline_free_names")
        if inline_free_names:
            owner_module = method_info.get("inline_owner_module")
            if owner_module is not None and owner_module != self.module_name:
                return None
        # The first param is `self` (for non-classmethod / non-static).
        if len(inline_params) != 1 + len(call_args):
            return None
        # Build the substitution map.
        # NB: we replace `self.locals` wholesale rather than overlay,
        # because the visitor's Name-resolution logic falls through to
        # `self.locals` for any name not otherwise resolved.  Names
        # outside the substitution would resolve in the caller's scope
        # and emit ops with caller-scope ValueIds — wrong inlining
        # semantics.  By replacing, we force the body to reference only
        # the substituted MoltValues; if the body has any other Name
        # reference, visit_Name will find it absent and bail.
        subst = {inline_params[0]: receiver}
        for pname, arg_val in zip(inline_params[1:], call_args):
            subst[pname] = arg_val
        old_locals = self.locals
        old_exact = self.exact_locals
        old_class = self.current_class
        old_first_param = self.current_method_first_param
        # Preserve self.exact_locals across inline so receiver-class
        # attribute folds inside the body still resolve.  But scrub
        # any caller locals that share names with our params, so they
        # don't accidentally surface during attribute lookups.
        new_exact = dict(old_exact) if isinstance(old_exact, dict) else {}
        for pname in inline_params:
            new_exact.pop(pname, None)
        # Set the inline scope's current_class / first_param so that
        # `super()` references inside the body resolve against the
        # callee's MRO (Base for Mid.compute's super, Mid for Leaf's
        # super), enabling the recursive Phase 4a fold + Phase 2
        # inline pipeline to unwind nested super-call chains at
        # compile time.
        if inline_owner is not None:
            self.current_class = inline_owner
            self.current_method_first_param = (
                inline_params[0] if inline_params else None
            )
        self.locals = subst
        self.exact_locals = new_exact
        # When the inlined method closes over the implicit ``__class__`` super
        # cell, every ``super()`` in its body MUST fold statically at this
        # site: the inlined body is spliced into the caller's scope, which has
        # no ``__class__`` cell, so a ``super()`` that falls to the runtime
        # path would bind to the wrong cell (or none — ``RuntimeError:
        # super(): __class__ cell not found``).  Setting ``_inline_super_must_fold``
        # makes the static super-fold raise ``_InlineSuperFoldRequired`` when it
        # cannot fold, aborting the whole inline so the caller falls back to the
        # cell-threaded dispatch path.  Stacks correctly across nested inlines.
        prev_super_must_fold = self._inline_super_must_fold
        if method_info.get("inline_closure_ok") and method_info.get("has_closure"):
            self._inline_super_must_fold = True
        try:
            result = self.visit(inline_return)
        except (
            KeyError,
            AttributeError,
            NotImplementedError,
            _InlineSuperFoldRequired,
        ):
            return None
        finally:
            self._inline_super_must_fold = prev_super_must_fold
            self.locals = old_locals
            self.exact_locals = old_exact
            self.current_class = old_class
            self.current_method_first_param = old_first_param
        # Re-stamp the inlined result's type_hint with the method's
        # declared return type when the visit produced a less-specific
        # hint.  Inlining can degrade type_hint propagation through
        # the BinOp/Compare visitors that don't always walk back to
        # the method signature; reasserting `int → int` here keeps
        # the lane preanalysis on the int-accumulator hot path in
        # tight loops like `total += obj.compute(i)`.
        if result is not None:
            return_hint = method_info.get("return_hint")
            if return_hint and (
                return_hint in self.classes or return_hint in BUILTIN_TYPE_TAGS
            ):
                current_hint = getattr(result, "type_hint", None)
                if current_hint in (None, "Any", "") and current_hint != return_hint:
                    result.type_hint = return_hint
        return result

    def _try_inline_init_assigns(
        self,
        init_assigns: "list[tuple[str, ast.expr]]",
        inline_params: list[str],
        receiver: "MoltValue",
        call_args: list,
    ) -> bool:
        """Inline an `__init__`-style body's `self.attr = expr`
        assignments at the call site.  Substitutes
        params → call_args in self.locals for the duration of
        visiting each value-expression, then emits a STORE_ATTR for
        each pair on `receiver`.  Returns True on success, False if
        any value-expression failed to lower (caller falls back to a
        regular CALL).
        """
        if len(inline_params) != 1 + len(call_args):
            return False
        subst = {inline_params[0]: receiver}
        for pname, arg_val in zip(inline_params[1:], call_args):
            subst[pname] = arg_val
        old_locals = self.locals
        old_exact = self.exact_locals
        new_exact = dict(old_exact) if isinstance(old_exact, dict) else {}
        for pname in inline_params:
            new_exact.pop(pname, None)
        self.locals = subst
        self.exact_locals = new_exact
        emitted_pairs: list[tuple[str, MoltValue]] = []
        try:
            for attr_name, expr in init_assigns:
                value = self.visit(expr)
                if value is None:
                    return False
                emitted_pairs.append((attr_name, value))
        except (KeyError, AttributeError, NotImplementedError):
            return False
        finally:
            self.locals = old_locals
            self.exact_locals = old_exact
        # All value-expressions visited successfully — emit each
        # store via `_emit_guarded_setattr(..., use_init=True,
        # assume_exact=True)`.  `use_init=True` is sound because the
        # receiver was just produced by OBJECT_NEW_BOUND (whose
        # backing allocation goes through `alloc_object_zeroed_with_pool`),
        # so every slot starts as `None`/0 with no live pointer to
        # decref.  This routes the lowering through `store_init`
        # (`function_compiler.rs:21162`) which has an inline tag-
        # check fast path: for immediate values (int/float/bool/
        # none) it emits a direct memory store with `MemFlags::trusted`
        # and zero runtime calls.  Targets bench_struct's 2-ops-per-
        # iter __init__ overhead.
        #
        # Falls back to `_emit_attribute_store` (which emits SETATTR
        # / runtime helper) only when the field map doesn't cover the
        # attribute — i.e. dynamic-class / non-static-layout edges
        # the assume_exact path declines to handle.
        receiver_class = receiver.type_hint
        if receiver_class is not None and receiver_class in self.classes:
            class_info = self.classes[receiver_class]
            field_map = class_info.get("fields", {}) if class_info else {}
            for attr_name, value in emitted_pairs:
                if (
                    attr_name in field_map
                    and not class_info.get("dynamic")
                    and not class_info.get("dataclass")
                    and not self._class_attr_is_data_descriptor(
                        receiver_class, attr_name
                    )
                ):
                    self._emit_guarded_setattr(
                        receiver,
                        attr_name,
                        value,
                        receiver_class,
                        use_init=True,
                        assume_exact=True,
                    )
                else:
                    self._emit_attribute_store(
                        receiver,
                        None,
                        None,
                        receiver_class,
                        attr_name,
                        value,
                    )
        else:
            for attr_name, value in emitted_pairs:
                self._emit_attribute_store(
                    receiver,
                    None,
                    None,
                    None,
                    attr_name,
                    value,
                )
        return True

    def _method_func_obj_for_defaults(
        self, owner_class: str, method_name: str
    ) -> "MoltValue | None":
        """Load the unbound function object for ``owner_class.method_name`` so the
        defaults-devirt deopt guard can read its ``__defaults__`` version stamp
        and live default tuple/dict.

        ``owner_class`` is the class that actually *defines* the method (the MRO
        owner, which may be a base class), so the function object — and thus the
        ``__defaults__`` a runtime ``Class.method.__defaults__ = (...)`` mutates
        — is read from there.  Returns ``None`` if that class cannot be resolved
        as a module attribute (the caller then declines to devirtualize the
        defaults-bearing call rather than guard against a missing object).
        """
        class_info = self.classes.get(owner_class)
        if class_info is None:
            return None
        module_name = class_info.get("module")
        if not isinstance(module_name, str):
            return None
        class_ref = self._emit_module_attr_get_on(module_name, owner_class)
        return self._emit_class_method_func(class_ref, method_name)

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
        # A method whose closure is exactly the implicit ``__class__`` super
        # cell (``inline_closure_ok``) is foldable, but ONLY through the inline
        # path below — its recursive static super-fold resolves the chain at
        # compile time and never reads the cell.  A direct CALL to the closure
        # symbol would omit the cell argument, so ``target_is_closure`` forbids
        # the direct-CALL fallback (routing those to the general dispatch path,
        # which threads the real closure tuple).  Real captured locals are not
        # foldable here at all.
        target_is_closure = bool(method_info.get("has_closure"))
        if target_is_closure and not method_info.get("inline_closure_ok"):
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
        expected_positional = param_count - 1  # exclude self
        # A method with positional defaults / kw-only params is direct-fillable:
        # the missing trailing arguments are padded from the function's live
        # __defaults__/__kwdefaults__ (or the compile-time literals when the
        # defaults version proves no runtime mutation — see
        # `_apply_default_specs`).  Reject only over-supply; under-supply is
        # filled below.  `kwonly_count` caps the positional region.
        kwonly_count = method_info.get("kwonly_count") or 0
        defaults_specs = method_info.get("defaults") or []
        has_fillable_defaults = bool(defaults_specs) or bool(kwonly_count)
        positional_param_count = expected_positional - kwonly_count
        if len(node.args) > positional_param_count:
            return None
        if len(node.args) < expected_positional and not has_fillable_defaults:
            return None

        receiver = self.visit(attr_node.value)
        if receiver is None:
            return None

        call_args = [self.visit(a) for a in node.args]
        if any(a is None for a in call_args):
            return None

        # Pad missing trailing arguments from the method's defaults.  When the
        # method has no defaults this is an exact-arity call and the list is
        # unchanged.  `_apply_default_specs` emits the `__defaults__`-mutation
        # deopt guard (baked literal fast path + live-read fallback) so a runtime
        # `Class.method.__defaults__ = (...)` reassignment is observed, matching
        # CPython's call-time default binding.  The function object the guard
        # reads is loaded from the (statically-known, non-dynamic) class.
        if has_fillable_defaults:
            method_func_obj = self._method_func_obj_for_defaults(
                owner_class, method_name
            )
            if method_func_obj is None:
                return None
            positional_limit = positional_param_count
            padded = self._apply_default_specs(
                expected_positional,
                defaults_specs,
                call_args,
                node,
                call_name=f"{class_name}.{method_name}",
                func_obj=method_func_obj,
                implicit_self=False,
                positional_limit=positional_limit if kwonly_count else None,
            )
            if padded is None:
                return None
            call_args = padded

        # Phase 2 inline opportunity: if the method has a trivially
        # inlinable body (single Return of a constant/param/binop/etc.
        # expression), emit the body inline rather than a CALL.  For a
        # ``__class__``-cell closure target this is the only foldable path:
        # the recursive super-fold inside the inlined body resolves the chain
        # statically and never reads the cell.  Only attempt the inline when the
        # supplied-arg arity already matches (a defaults-padded call has the full
        # argument list materialized above, so this still holds).
        inlined = self._try_inline_method_call(method_info, receiver, call_args)
        if inlined is not None:
            return inlined

        # The target carries the implicit ``__class__`` super cell but did not
        # inline: a direct CALL to its closure symbol would omit the cell, so
        # route to the general dispatch path (which threads the real closure).
        if target_is_closure:
            return None

        # The method's symbol was registered when the ClassDef was
        # compiled (frontend/__init__.py:14117).  `method_info["func"]`
        # is the MoltValue produced for the method, with
        # `type_hint=f"Func:{method_symbol}"`.  Extract the symbol from
        # there rather than re-calling `_function_symbol` (which would
        # increment the collision counter and create a fresh, dangling
        # symbol → undefined-symbol link error).
        func_val = method_info.get("func")
        if func_val is None or not getattr(func_val, "type_hint", "").startswith(
            "Func:"
        ):
            return None
        method_symbol = func_val.type_hint.split(":", 1)[1]
        if method_symbol not in self.func_symbol_names:
            return None

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

    def visit_Call(self, node: ast.Call) -> Any:
        gpu_launch = self._lower_gpu_kernel_launch_call(node)
        if gpu_launch is not None:
            return gpu_launch

        gpu_intrinsic = self._is_gpu_intrinsic_call(node)
        if gpu_intrinsic is not None and self.current_gpu_kernel_context:
            return self._emit_gpu_kernel_intrinsic_op(gpu_intrinsic)

        # Phase 1 (frontend variant) — monomorphic user-method direct call.
        #
        # Eliminates per-iteration BoundMethod allocation when
        # ``obj.method(args)`` has a statically-known concrete receiver
        # class.  Falls through to the general path on any signature
        # complexity (kwargs, defaults, descriptors, dataclass, etc.).
        user_method_fold = self._try_emit_user_method_static_call(node)
        if user_method_fold is not None:
            return user_method_fold

        # Phase 4a — static super().method(args) fold.
        #
        # Pattern: bare `super()` (no args) called inside a method body
        # whose class and first-parameter are both known statically, and
        # whose MRO is statically resolvable (no metaclass / dynamic
        # bases / __init_subclass__ surprises).
        #
        # Eliminates per-iteration:
        #   - SUPER_NEW heap allocation (super object)
        #   - GETATTR_GENERIC_OBJ heap allocation (bound method)
        #   - CALL_BIND IC dispatch overhead
        # Replaces all three with a single direct CALL to the parent
        # class's method symbol with `self` prepended — which is what
        # Python semantics require but the dynamic dispatch path was
        # discovering at runtime per call.
        super_fold = self._try_emit_super_static_call(node)
        if super_fold is not None:
            return super_fold

        importlib_literal = self._try_emit_importlib_import_module_literal_call(node)
        if importlib_literal is not None:
            return importlib_literal

        needs_bind = self._call_needs_bind(node)
        if isinstance(node.func, ast.Attribute):
            attr_node = node.func
            if (
                node.func.attr == "format"
                and isinstance(node.func.value, ast.Constant)
                and isinstance(node.func.value.value, str)
            ):
                lowered = self._lower_string_format_call(node, node.func.value.value)
                if lowered is not None:
                    return lowered
            # ...
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "contextlib"
                and node.func.attr == "nullcontext"
            ):
                if len(node.args) > 1:
                    raise NotImplementedError("nullcontext expects 0 or 1 argument")
                if node.args:
                    payload = self.visit(node.args[0])
                else:
                    payload = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=payload))
                return self._emit_nullcontext(payload)
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "contextlib"
                and node.func.attr == "closing"
            ):
                if len(node.args) != 1:
                    raise NotImplementedError("closing expects 1 argument")
                payload = self.visit(node.args[0])
                return self._emit_closing(payload)
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "math"
                and node.func.attr == "trunc"
            ):
                if len(node.args) != 1:
                    raise NotImplementedError("math.trunc expects 1 argument")
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported math.trunc input")
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="TRUNC", args=[value], result=res))
                return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_json"
            ):
                if node.func.attr == "parse" and len(node.args) == 1:
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if self.parse_codec == "cbor":
                        kind = "CBOR_PARSE"
                    elif self.parse_codec == "json":
                        kind = "JSON_PARSE"
                    else:
                        kind = "MSGPACK_PARSE"
                    self.emit(MoltOp(kind=kind, args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_msgpack"
            ):
                if node.func.attr == "parse" and len(node.args) == 1:
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="MSGPACK_PARSE", args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_cbor"
            ):
                if node.func.attr == "parse" and len(node.args) == 1:
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="CBOR_PARSE", args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_buffer"
            ):
                if node.func.attr == "new":
                    if len(node.args) not in (2, 3):
                        raise NotImplementedError(
                            "molt_buffer.new expects 2 or 3 arguments"
                        )
                    rows = self.visit(node.args[0])
                    cols = self.visit(node.args[1])
                    if len(node.args) == 3:
                        init = self.visit(node.args[2])
                    else:
                        init = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=init))
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(kind="BUFFER2D_NEW", args=[rows, cols, init], result=res)
                    )
                    return res
                if node.func.attr == "get":
                    if len(node.args) != 3:
                        raise NotImplementedError("molt_buffer.get expects 3 arguments")
                    buf = self.visit(node.args[0])
                    row = self.visit(node.args[1])
                    col = self.visit(node.args[2])
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="BUFFER2D_GET", args=[buf, row, col], result=res)
                    )
                    return res
                if node.func.attr == "set":
                    if len(node.args) != 4:
                        raise NotImplementedError("molt_buffer.set expects 4 arguments")
                    buf = self.visit(node.args[0])
                    row = self.visit(node.args[1])
                    col = self.visit(node.args[2])
                    val = self.visit(node.args[3])
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(
                            kind="BUFFER2D_SET", args=[buf, row, col, val], result=res
                        )
                    )
                    return res
                if node.func.attr == "matmul":
                    if len(node.args) != 2:
                        raise NotImplementedError(
                            "molt_buffer.matmul expects 2 arguments"
                        )
                    lhs = self.visit(node.args[0])
                    rhs = self.visit(node.args[1])
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(kind="BUFFER2D_MATMUL", args=[lhs, rhs], result=res)
                    )
                    return res
            receiver = self.visit(attr_node.value)
            if receiver is None:
                receiver = MoltValue("unknown_obj", type_hint="Unknown")
            obj_name = None
            exact_class = None
            if isinstance(attr_node.value, ast.Name):
                obj_name = attr_node.value.id
                exact_class = self.exact_locals.get(obj_name)

            def load_attr_callee() -> MoltValue:
                return self._emit_attribute_load(
                    attr_node, receiver, obj_name, exact_class
                )

            method = attr_node.attr
            if receiver.type_hint == "bytearray" and method in {
                "append",
                "clear",
                "extend",
                "insert",
                "pop",
                "remove",
                "resize",
            }:
                self._invalidate_bytearray_len_hint(obj_name, receiver)
            if method == "sort" and receiver.type_hint == "list":
                needs_bind = True
            if receiver.type_hint == "generator":
                if method == "send":
                    if len(node.args) != 1:
                        raise NotImplementedError("generator.send expects 1 argument")
                    arg = self.visit(node.args[0])
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="GEN_SEND", args=[receiver, arg], result=pair)
                    )
                    one = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[1], result=one))
                    zero = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                    value = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=value))
                    done = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
                    self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
                    self._emit_stop_iteration_from_value(value)
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    return value
                if method == "throw":
                    if len(node.args) not in {1, 2, 3}:
                        raise NotImplementedError(
                            "generator.throw expects 1 to 3 arguments"
                        )
                    exc_type = self.visit(node.args[0])
                    if exc_type is None:
                        raise NotImplementedError("generator.throw expects exception")
                    if len(node.args) > 1:
                        value = self.visit(node.args[1])
                        if value is None:
                            raise NotImplementedError(
                                "generator.throw expects exception value"
                            )
                        callargs = MoltValue(self.next_var(), type_hint="callargs")
                        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                        self.emit(
                            MoltOp(
                                kind="CALLARGS_PUSH_POS",
                                args=[callargs, value],
                                result=MoltValue("none"),
                            )
                        )
                        arg = MoltValue(self.next_var(), type_hint="exception")
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[exc_type, callargs],
                                result=arg,
                            )
                        )
                        if len(node.args) == 3:
                            tb_val = self.visit(node.args[2])
                            if tb_val is None:
                                raise NotImplementedError(
                                    "generator.throw expects traceback value"
                                )
                            self.emit(
                                MoltOp(
                                    kind="SETATTR_GENERIC_OBJ",
                                    args=[arg, "__traceback__", tb_val],
                                    result=MoltValue("none"),
                                )
                            )
                    else:
                        arg = exc_type
                    callee = load_attr_callee()
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_METHOD", args=[callee, arg], result=res)
                    )
                    return res
                if method == "close":
                    if node.args:
                        raise NotImplementedError("generator.close expects 0 arguments")
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="GEN_CLOSE", args=[receiver], result=res))
                    return res
            class_name = None
            class_info = self.classes.get(receiver.type_hint)
            receiver_is_class_obj = False
            if isinstance(node.func.value, ast.Name):
                candidate = node.func.value.id
                candidate_info = self.classes.get(candidate)
                if candidate in BUILTIN_TYPE_TAGS or candidate_info is not None:
                    receiver_is_class_obj = True
                    if candidate_info is not None:
                        class_name = candidate
                        class_info = candidate_info
            if receiver_is_class_obj:
                needs_bind = True
            lookup_class = class_name
            if lookup_class is None and receiver.type_hint in self.classes:
                lookup_class = receiver.type_hint
            method_info = None
            method_class = None
            if lookup_class:
                method_info, method_class = self._resolve_method_info(
                    lookup_class, method
                )
            if method_info and (
                needs_bind
                or method_info.get("descriptor") == "decorated"
                or method_info.get("has_vararg", False)
                or method_info.get("has_varkw", False)
                or method_info.get("has_closure", False)
            ):
                callee = load_attr_callee()
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                callargs = self._emit_call_args_builder(node)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[callee, callargs],
                        result=res,
                    )
                )
                return res
            if method_info and not needs_bind:
                if class_name is None and receiver.type_hint not in self.classes:
                    callee = load_attr_callee()
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                func_val = method_info["func"]
                descriptor = method_info["descriptor"]
                args = self._emit_call_args(node.args)
                if descriptor == "function":
                    if class_name is None and receiver.type_hint in self.classes:
                        if not receiver_is_class_obj:
                            args = [receiver] + args
                elif descriptor == "classmethod":
                    if class_name is None and receiver.type_hint in self.classes:
                        class_name = receiver.type_hint
                    if class_name is None:
                        raise NotImplementedError("Unsupported classmethod call")
                    class_ref = (
                        receiver
                        if isinstance(node.func.value, ast.Name)
                        and class_name == node.func.value.id
                        else self._emit_module_attr_get(class_name)
                    )
                    args = [class_ref] + args
                elif descriptor != "staticmethod":
                    args = []
                if args or descriptor in {"function", "classmethod", "staticmethod"}:
                    param_count = method_info.get("param_count")
                    defaults = method_info.get("defaults", [])
                    has_vararg = method_info.get("has_vararg", False)
                    has_varkw = method_info.get("has_varkw", False)
                    kwonly_count = method_info.get("kwonly_count")
                    if param_count is not None:
                        fixed_param_count = param_count
                        if has_vararg:
                            fixed_param_count -= 1
                        if has_varkw:
                            fixed_param_count -= 1
                        func_obj = None
                        missing = fixed_param_count - len(args)
                        # Load the function object whenever a trailing default is
                        # filled: a const default needs the version stamp for the
                        # `__defaults__`-mutation deopt guard, a non-const default
                        # needs the live `__defaults__`/`__kwdefaults__` read.
                        if 0 < missing <= len(defaults):
                            class_ref = None
                            if lookup_class:
                                class_info = self.classes.get(lookup_class)
                                if class_info:
                                    class_ref = self._emit_module_attr_get_on(
                                        class_info["module"], lookup_class
                                    )
                            if class_ref is not None:
                                class_attr = self._emit_class_method_func(
                                    class_ref, method
                                )
                                if descriptor == "classmethod":
                                    func_obj = self._emit_bound_method_func(class_attr)
                                else:
                                    func_obj = class_attr
                            else:
                                callee = load_attr_callee()
                                if callee is not None:
                                    if descriptor == "classmethod":
                                        func_obj = self._emit_bound_method_func(callee)
                                    elif descriptor == "function":
                                        if isinstance(
                                            callee.type_hint, str
                                        ) and callee.type_hint.startswith(
                                            "BoundMethod:"
                                        ):
                                            func_obj = self._emit_bound_method_func(
                                                callee
                                            )
                                        else:
                                            func_obj = callee
                                    else:
                                        func_obj = callee
                        positional_limit = None
                        if isinstance(kwonly_count, int):
                            positional_limit = fixed_param_count - kwonly_count
                            if positional_limit < 0:
                                positional_limit = 0
                        args = self._apply_default_specs(
                            fixed_param_count,
                            defaults,
                            args,
                            node,
                            call_name=f"{lookup_class}.{method}",
                            func_obj=func_obj,
                            implicit_self=False,
                            positional_limit=positional_limit,
                        )
                        if args is None:
                            callee = load_attr_callee()
                            if callee is None:
                                raise NotImplementedError("Unsupported call target")
                            callargs = self._emit_call_args_builder(node)
                            res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        if has_vararg:
                            if len(args) > fixed_param_count:
                                extra = args[fixed_param_count:]
                                tuple_val = MoltValue(
                                    self.next_var(), type_hint="tuple"
                                )
                                self.emit(
                                    MoltOp(
                                        kind="TUPLE_NEW",
                                        args=extra,
                                        result=tuple_val,
                                    )
                                )
                                args = args[:fixed_param_count] + [tuple_val]
                            elif len(args) == fixed_param_count:
                                empty_tuple = MoltValue(
                                    self.next_var(), type_hint="tuple"
                                )
                                self.emit(
                                    MoltOp(
                                        kind="TUPLE_NEW",
                                        args=[],
                                        result=empty_tuple,
                                    )
                                )
                                args = args + [empty_tuple]
                        if has_varkw:
                            empty_kwargs = MoltValue(self.next_var(), type_hint="dict")
                            self.emit(
                                MoltOp(
                                    kind="DICT_NEW",
                                    args=[],
                                    result=empty_kwargs,
                                )
                            )
                            args = args + [empty_kwargs]
                    res_hint = "Any"
                    return_hint = method_info["return_hint"]
                    # Builtin scalar/container return types must propagate as
                    # type hints — see _resolve_method_call_hints for the same
                    # fix; lane inference falls back to NaN-boxed accumulator
                    # if `int` returns are erased here.
                    if return_hint and (
                        return_hint in self.classes or return_hint in BUILTIN_TYPE_TAGS
                    ):
                        res_hint = return_hint
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    # Route known-method calls through CALL_BIND so descriptor binding and
                    # handle semantics stay aligned with dynamic attribute calls.
                    callee = load_attr_callee()
                    if callee is None:
                        target_name = func_val.type_hint.split(":", 1)[1]
                        self.emit(
                            MoltOp(kind="CALL", args=[target_name] + args, result=res)
                        )
                        return res
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
            if method == "add" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.add expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_ADD", args=[receiver, arg], result=res))
                return res
            if method == "discard" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.discard expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_DISCARD", args=[receiver, arg], result=res))
                return res
            if method == "remove" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.remove expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_REMOVE", args=[receiver, arg], result=res))
                return res
            if (
                method
                in {
                    "union",
                    "intersection",
                    "difference",
                    "symmetric_difference",
                }
                and receiver.type_hint in {"set", "frozenset"}
                and not any(isinstance(a, ast.Starred) for a in node.args)
            ):
                if method == "symmetric_difference":
                    if len(node.args) != 1:
                        raise NotImplementedError(
                            "set.symmetric_difference expects 1 argument"
                        )
                    other = self.visit(node.args[0])
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if other.type_hint not in {"set", "frozenset"}:
                        other = self._emit_set_from_iter(other)
                    op_kind = "BIT_XOR"
                    res = MoltValue(self.next_var(), type_hint=receiver.type_hint)
                    self.emit(MoltOp(kind=op_kind, args=[receiver, other], result=res))
                    return res
                if len(node.args) == 0:
                    if receiver.type_hint == "frozenset":
                        return self._emit_frozenset_from_iter(receiver)
                    return self._emit_set_from_iter(receiver)
                if method == "union":
                    res = self._emit_set_from_iter(receiver)
                    for arg in node.args:
                        other = self.visit(arg)
                        if other is None:
                            raise NotImplementedError("Unsupported set operation input")
                        if other.type_hint in {"set", "frozenset"}:
                            self.emit(
                                MoltOp(
                                    kind="SET_UPDATE",
                                    args=[res, other],
                                    result=MoltValue("none"),
                                )
                            )
                        else:
                            self._emit_set_update_from_iter(res, other)
                    if receiver.type_hint == "frozenset":
                        return self._emit_frozenset_from_iter(res)
                    return res
                res = receiver
                for arg in node.args:
                    other = self.visit(arg)
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if other.type_hint not in {"set", "frozenset"}:
                        # intersection probes the receiver (bare unhashable
                        # context); difference inserts into a result set
                        # (set-element context on 3.14).
                        other = self._emit_set_from_iter(
                            other, probe=(method == "intersection")
                        )
                    op_kind = {
                        "intersection": "BIT_AND",
                        "difference": "SUB",
                    }[method]
                    next_res = MoltValue(self.next_var(), type_hint=receiver.type_hint)
                    self.emit(MoltOp(kind=op_kind, args=[res, other], result=next_res))
                    res = next_res
                return res
            if (
                method
                in {
                    "update",
                    "intersection_update",
                    "difference_update",
                    "symmetric_difference_update",
                }
                and receiver.type_hint == "set"
                and not any(isinstance(a, ast.Starred) for a in node.args)
            ):
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                if method == "symmetric_difference_update":
                    if len(node.args) != 1:
                        raise NotImplementedError(
                            "set.symmetric_difference_update expects 1 argument"
                        )
                if len(node.args) == 0:
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                op_kind = {
                    "update": "SET_UPDATE",
                    "intersection_update": "SET_INTERSECTION_UPDATE",
                    "difference_update": "SET_DIFFERENCE_UPDATE",
                    "symmetric_difference_update": "SET_SYMDIFF_UPDATE",
                }[method]
                for arg in node.args:
                    other = self.visit(arg)
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if recv_slot is not None:
                        receiver = self._reload_async_value(
                            recv_slot, receiver.type_hint
                        )
                    if other.type_hint in {"set", "frozenset"} or method != "update":
                        if other.type_hint not in {"set", "frozenset"}:
                            # intersection_update probes the receiver (bare
                            # unhashable context); the other update-family ops
                            # insert (set-element context on 3.14).
                            other = self._emit_set_from_iter(
                                other, probe=(method == "intersection_update")
                            )
                        self.emit(
                            MoltOp(kind=op_kind, args=[receiver, other], result=res)
                        )
                    else:
                        self._emit_set_update_from_iter(receiver, other)
                return res
            if method == "append" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.append expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("list.append expects a value")
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                self._record_list_element_write(receiver, obj_name, arg.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_APPEND", args=[receiver, arg], result=res))
                return res
            if method == "extend" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.extend expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                other = self.visit(node.args[0])
                if other is None:
                    raise NotImplementedError("list.extend expects an iterable")
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                self._record_list_element_write(
                    receiver,
                    obj_name,
                    self._iterable_element_hint(other),
                )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_EXTEND", args=[receiver, other], result=res)
                )
                return res
            if method == "insert" and receiver.type_hint == "list":
                if len(node.args) != 2:
                    raise NotImplementedError("list.insert expects 2 arguments")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                idx = self.visit(node.args[0])
                val = self.visit(node.args[1])
                if idx is None or val is None:
                    raise NotImplementedError("list.insert expects index and value")
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                self._record_list_element_write(receiver, obj_name, val.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_INSERT", args=[receiver, idx, val], result=res)
                )
                return res
            if method == "remove" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.remove expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                val = self.visit(node.args[0])
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REMOVE", args=[receiver, val], result=res))
                return res
            if method == "clear" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.clear expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_CLEAR", args=[receiver], result=res))
                return res
            if method == "copy" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.copy expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="LIST_COPY", args=[receiver], result=res))
                return res
            if method == "reverse" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.reverse expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REVERSE", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LIST_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "list":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("list.index expects 1 to 3 arguments")
                val = self.visit(node.args[0])
                start = None
                end = None
                if len(node.args) >= 2:
                    start = self.visit(node.args[1])
                    if start is None:
                        raise NotImplementedError("Unsupported list.index start")
                if len(node.args) == 3:
                    end = self.visit(node.args[2])
                    if end is None:
                        raise NotImplementedError("Unsupported list.index end")
                for keyword in node.keywords:
                    if keyword.arg is None:
                        raise NotImplementedError(
                            "list.index does not support **kwargs"
                        )
                    if keyword.arg == "start":
                        if start is not None:
                            return self._emit_type_error_value(
                                "list.index() got multiple values for argument 'start'",
                                "int",
                            )
                        start = self.visit(keyword.value)
                        if start is None:
                            raise NotImplementedError("Unsupported list.index start")
                    elif keyword.arg == "end":
                        if end is not None:
                            return self._emit_type_error_value(
                                "list.index() got multiple values for argument 'end'",
                                "int",
                            )
                        end = self.visit(keyword.value)
                        if end is None:
                            raise NotImplementedError("Unsupported list.index end")
                    else:
                        return self._emit_type_error_value(
                            "list.index() got an unexpected keyword argument "
                            f"'{keyword.arg}'",
                            "int",
                        )
                if start is None and end is None:
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="LIST_INDEX", args=[receiver, val], result=res)
                    )
                    return res
                if start is None:
                    start = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=start))
                if end is None:
                    stop = MoltValue(self.next_var(), type_hint="missing")
                    self.emit(MoltOp(kind="MISSING", args=[], result=stop))
                else:
                    stop = end
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="LIST_INDEX_RANGE",
                        args=[receiver, val, start, stop],
                        result=res,
                    )
                )
                return res
            if method == "pop" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("dict.pop expects 1 or 2 arguments")
                key = self.visit(node.args[0])
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                    has_default = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[1], result=has_default))
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                    has_default = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=has_default))
                res_type = "Any"
                if self.type_hint_policy == "trust":
                    hint = self._dict_value_hint(receiver)
                    if hint is not None:
                        res_type = hint
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(
                        kind="DICT_POP",
                        args=[receiver, key, default, has_default],
                        result=res,
                    )
                )
                return res
            if method == "pop" and receiver.type_hint == "set":
                if node.args:
                    raise NotImplementedError("set.pop expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="SET_POP", args=[receiver], result=res))
                return res
            if method == "pop" and receiver.type_hint == "list":
                if len(node.args) > 1:
                    raise NotImplementedError("list.pop expects 0 or 1 argument")
                if node.args:
                    idx = self.visit(node.args[0])
                else:
                    idx = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=idx))
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="LIST_POP", args=[receiver, idx], result=res))
                return res
            if method == "get" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("dict.get expects 1 or 2 arguments")
                key = self.visit(node.args[0])
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                res_type = "Any"
                if self.type_hint_policy == "trust":
                    hint = self._dict_value_hint(receiver)
                    if hint is not None:
                        res_type = hint
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(kind="DICT_GET", args=[receiver, key, default], result=res)
                )
                return res
            if method == "setdefault" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if node.keywords or len(node.args) not in (1, 2):
                    raise NotImplementedError(
                        "dict.setdefault expects 1 or 2 arguments"
                    )
                key = self.visit(node.args[0])
                if (
                    len(node.args) == 2
                    and isinstance(node.args[1], ast.List)
                    and not node.args[1].elts
                ):
                    res_type = "Any"
                    if self.type_hint_policy == "trust":
                        hint = self._dict_value_hint(receiver)
                        if hint is not None:
                            res_type = hint
                    res = MoltValue(self.next_var(), type_hint=res_type)
                    self.emit(
                        MoltOp(
                            kind="DICT_SETDEFAULT_EMPTY_LIST",
                            args=[receiver, key],
                            result=res,
                        )
                    )
                    return res
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                res_type = "Any"
                if self.type_hint_policy == "trust":
                    hint = self._dict_value_hint(receiver)
                    if hint is not None:
                        res_type = hint
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(
                        kind="DICT_SETDEFAULT",
                        args=[receiver, key, default],
                        result=res,
                    )
                )
                return res
            if method == "update" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if len(node.args) > 1:
                    msg = f"update expected at most 1 argument, got {len(node.args)}"
                    return self._emit_type_error_value(msg, "None")
                res = MoltValue(self.next_var(), type_hint="None")
                if node.args:
                    other = self.visit(node.args[0])
                    if other is None:
                        raise NotImplementedError("Unsupported dict.update input")
                    self.emit(
                        MoltOp(
                            kind="DICT_UPDATE",
                            args=[receiver, other],
                            result=res,
                        )
                    )
                for kw in node.keywords:
                    if kw.arg is None:
                        mapping = self.visit(kw.value)
                        if mapping is None:
                            raise NotImplementedError(
                                "Unsupported dict.update ** input"
                            )
                        self.emit(
                            MoltOp(
                                kind="DICT_UPDATE_KWSTAR",
                                args=[receiver, mapping],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        key = MoltValue(self.next_var(), type_hint="str")
                        self.emit(MoltOp(kind="CONST_STR", args=[kw.arg], result=key))
                        val = self.visit(kw.value)
                        if val is None:
                            raise NotImplementedError(
                                "Unsupported dict.update kw value"
                            )
                        self.emit(
                            MoltOp(
                                kind="STORE_INDEX",
                                args=[receiver, key, val],
                                result=MoltValue("none"),
                            )
                        )
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if method == "clear" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if node.args or node.keywords:
                    raise NotImplementedError("dict.clear expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="DICT_CLEAR", args=[receiver], result=res))
                return res
            if method == "copy" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if node.args or node.keywords:
                    raise NotImplementedError("dict.copy expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_COPY", args=[receiver], result=res))
                return res
            if method == "popitem" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if node.args or node.keywords:
                    raise NotImplementedError("dict.popitem expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="DICT_POPITEM", args=[receiver], result=res))
                return res
            if method == "keys" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                res = MoltValue(self.next_var(), type_hint="dict_keys_view")
                self.emit(MoltOp(kind="DICT_KEYS", args=[receiver], result=res))
                return res
            if method == "values" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                res = MoltValue(self.next_var(), type_hint="dict_values_view")
                self.emit(MoltOp(kind="DICT_VALUES", args=[receiver], result=res))
                return res
            if method == "items" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                res = MoltValue(self.next_var(), type_hint="dict_items_view")
                self.emit(MoltOp(kind="DICT_ITEMS", args=[receiver], result=res))
                return res
            if method == "read" and receiver.type_hint.startswith("file"):
                if len(node.args) > 1:
                    raise NotImplementedError("file.read expects 0 or 1 argument")
                if node.args:
                    size_val = self.visit(node.args[0])
                else:
                    size_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=size_val))
                if receiver.type_hint == "file_bytes":
                    res_hint = "bytes"
                elif receiver.type_hint == "file_text":
                    res_hint = "str"
                else:
                    res_hint = "Any"
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="FILE_READ", args=[receiver, size_val], result=res)
                )
                return res
            if method == "write" and receiver.type_hint.startswith("file"):
                if len(node.args) != 1:
                    raise NotImplementedError("file.write expects 1 argument")
                data = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="FILE_WRITE", args=[receiver, data], result=res))
                return res
            if method == "close" and receiver.type_hint.startswith("file"):
                if node.args:
                    raise NotImplementedError("file.close expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FILE_CLOSE", args=[receiver], result=res))
                return res
            if method == "flush" and receiver.type_hint.startswith("file"):
                if node.args:
                    raise NotImplementedError("file.flush expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FILE_FLUSH", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "tuple":
                if len(node.args) != 1:
                    raise NotImplementedError("tuple.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="TUPLE_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "tuple":
                if len(node.args) == 1 and not node.keywords:
                    val = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="TUPLE_INDEX", args=[receiver, val], result=res)
                    )
                    return res
            if method == "tobytes":
                if node.args:
                    raise NotImplementedError("tobytes expects 0 arguments")
                if receiver.type_hint == "memoryview":
                    res = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(
                        MoltOp(kind="MEMORYVIEW_TOBYTES", args=[receiver], result=res)
                    )
                    return res
            if method == "count":
                if receiver.type_hint in {"str", "bytes", "bytearray"}:
                    if len(node.args) not in (1, 2, 3):
                        pass
                    elif any(kw.arg is None for kw in node.keywords):
                        pass
                    else:
                        needle_node = node.args[0]
                        start_node: ast.expr | None = None
                        end_node: ast.expr | None = None
                        start_provided = False
                        end_provided = False
                        if len(node.args) >= 2:
                            start_node = node.args[1]
                            start_provided = True
                        if len(node.args) == 3:
                            end_node = node.args[2]
                            end_provided = True
                        for keyword in node.keywords:
                            if keyword.arg == "start":
                                if start_node is not None:
                                    return self._emit_type_error_value(
                                        "count() got multiple values for argument 'start'",
                                        "int",
                                    )
                                start_node = keyword.value
                                start_provided = True
                            elif keyword.arg == "end":
                                if end_node is not None:
                                    return self._emit_type_error_value(
                                        "count() got multiple values for argument 'end'",
                                        "int",
                                    )
                                end_node = keyword.value
                                end_provided = True
                            else:
                                return self._emit_type_error_value(
                                    "count() got an unexpected keyword argument "
                                    f"'{keyword.arg}'",
                                    "int",
                                )
                        needle = self.visit(needle_node)
                        use_slice = start_provided or end_provided
                        if receiver.type_hint == "str":
                            res = MoltValue(self.next_var(), type_hint="int")
                            if not use_slice:
                                self.emit(
                                    MoltOp(
                                        kind="STRING_COUNT",
                                        args=[receiver, needle],
                                        result=res,
                                    )
                                )
                                return res
                            if start_node is None:
                                start = MoltValue(self.next_var(), type_hint="int")
                                self.emit(MoltOp(kind="CONST", args=[0], result=start))
                            else:
                                start = self.visit(start_node)
                                if start is None:
                                    raise NotImplementedError(
                                        "Unsupported count start argument"
                                    )
                            if end_node is None:
                                end = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=end)
                                )
                            else:
                                end = self.visit(end_node)
                                if end is None:
                                    raise NotImplementedError(
                                        "Unsupported count end argument"
                                    )
                            has_end = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[end_provided],
                                    result=has_end,
                                )
                            )
                            has_start = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[start_provided],
                                    result=has_start,
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="STRING_COUNT_SLICE",
                                    args=[
                                        receiver,
                                        needle,
                                        start,
                                        end,
                                        has_start,
                                        has_end,
                                    ],
                                    result=res,
                                )
                            )
                            return res
                        if receiver.type_hint in {"bytes", "bytearray"}:
                            res = MoltValue(self.next_var(), type_hint="int")
                            if not use_slice:
                                op_kind = (
                                    "BYTES_COUNT"
                                    if receiver.type_hint == "bytes"
                                    else "BYTEARRAY_COUNT"
                                )
                                self.emit(
                                    MoltOp(
                                        kind=op_kind,
                                        args=[receiver, needle],
                                        result=res,
                                    )
                                )
                                return res
                            if start_node is None:
                                start = MoltValue(self.next_var(), type_hint="int")
                                self.emit(MoltOp(kind="CONST", args=[0], result=start))
                            else:
                                start = self.visit(start_node)
                                if start is None:
                                    raise NotImplementedError(
                                        "Unsupported count start argument"
                                    )
                            if end_node is None:
                                end = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=end)
                                )
                            else:
                                end = self.visit(end_node)
                                if end is None:
                                    raise NotImplementedError(
                                        "Unsupported count end argument"
                                    )
                            has_end = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[end_provided],
                                    result=has_end,
                                )
                            )
                            has_start = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[start_provided],
                                    result=has_start,
                                )
                            )
                            op_kind = (
                                "BYTES_COUNT_SLICE"
                                if receiver.type_hint == "bytes"
                                else "BYTEARRAY_COUNT_SLICE"
                            )
                            self.emit(
                                MoltOp(
                                    kind=op_kind,
                                    args=[
                                        receiver,
                                        needle,
                                        start,
                                        end,
                                        has_start,
                                        has_end,
                                    ],
                                    result=res,
                                )
                            )
                            return res
            if method == "startswith":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("startswith expects 1-3 arguments")
                needle = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            if method == "endswith":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("endswith expects 1-3 arguments")
                needle = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            if method == "join":
                if len(node.args) != 1:
                    callee = load_attr_callee()
                    return self._emit_dynamic_call(node, callee, True)
                items = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_JOIN", args=[receiver, items], result=res)
                    )
                    return res
            if method == "split":
                if len(node.args) > 2:
                    raise NotImplementedError("split expects 0-2 arguments")
                # Support keyword args: split(sep=',') and split(sep=',', maxsplit=2)
                kw_sep = next(
                    (kw.value for kw in node.keywords if kw.arg == "sep"), None
                )
                kw_maxsplit = next(
                    (kw.value for kw in node.keywords if kw.arg == "maxsplit"), None
                )
                if node.args:
                    needle = self.visit(node.args[0])
                elif kw_sep is not None:
                    needle = self.visit(kw_sep)
                else:
                    needle = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=needle))
                maxsplit = None
                if len(node.args) == 2:
                    maxsplit = self.visit(node.args[1])
                elif kw_maxsplit is not None:
                    maxsplit = self.visit(kw_maxsplit)
                res = MoltValue(self.next_var(), type_hint="list")
                if receiver.type_hint == "str":
                    if maxsplit is not None:
                        self.emit(
                            MoltOp(
                                kind="STRING_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="STRING_SPLIT", args=[receiver, needle], result=res
                            )
                        )
                    self._record_container_elem_hint(res, "str")
                    return res
                if receiver.type_hint == "bytes":
                    if maxsplit is not None:
                        self.emit(
                            MoltOp(
                                kind="BYTES_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="BYTES_SPLIT", args=[receiver, needle], result=res
                            )
                        )
                    self._record_container_elem_hint(res, "bytes")
                    return res
                if receiver.type_hint == "bytearray":
                    if maxsplit is not None:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_SPLIT",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                    self._record_container_elem_hint(res, "bytearray")
                    return res
            if method == "lower" and receiver.type_hint == "str":
                if node.args:
                    raise NotImplementedError("lower expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_LOWER", args=[receiver], result=res))
                return res
            if method == "upper" and receiver.type_hint == "str":
                if node.args:
                    raise NotImplementedError("upper expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_UPPER", args=[receiver], result=res))
                return res
            if method == "capitalize" and receiver.type_hint == "str":
                if node.args:
                    raise NotImplementedError("capitalize expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_CAPITALIZE", args=[receiver], result=res))
                return res
            if method == "strip" and receiver.type_hint in {
                "str",
                "bytes",
                "bytearray",
            }:
                if len(node.args) > 1:
                    raise NotImplementedError("strip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_STRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "lstrip" and receiver.type_hint in {
                "str",
                "bytes",
                "bytearray",
            }:
                if len(node.args) > 1:
                    raise NotImplementedError("lstrip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_LSTRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "rstrip" and receiver.type_hint in {
                "str",
                "bytes",
                "bytearray",
            }:
                if len(node.args) > 1:
                    raise NotImplementedError("rstrip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_RSTRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "replace":
                if receiver.type_hint in {"str", "bytes", "bytearray"}:
                    if any(isinstance(arg, ast.Starred) for arg in node.args):
                        pass
                    elif any(kw.arg is None for kw in node.keywords):
                        pass
                    else:
                        count_expr: ast.expr | None = None
                        extra_kw = False
                        for kw in node.keywords:
                            if kw.arg == "count":
                                count_expr = kw.value
                            else:
                                extra_kw = True
                                break
                        if not extra_kw and len(node.args) in (2, 3):
                            if len(node.args) == 3 and count_expr is not None:
                                pass
                            else:
                                old = self.visit(node.args[0])
                                new = self.visit(node.args[1])
                                if len(node.args) == 3:
                                    count = self.visit(node.args[2])
                                elif count_expr is not None:
                                    count = self.visit(count_expr)
                                else:
                                    count = MoltValue(self.next_var(), type_hint="int")
                                    self.emit(
                                        MoltOp(kind="CONST", args=[-1], result=count)
                                    )
                                res = MoltValue(
                                    self.next_var(), type_hint=receiver.type_hint
                                )
                                if receiver.type_hint == "str":
                                    self.emit(
                                        MoltOp(
                                            kind="STRING_REPLACE",
                                            args=[receiver, old, new, count],
                                            result=res,
                                        )
                                    )
                                    return res
                                if receiver.type_hint == "bytes":
                                    self.emit(
                                        MoltOp(
                                            kind="BYTES_REPLACE",
                                            args=[receiver, old, new, count],
                                            result=res,
                                        )
                                    )
                                    return res
                                if receiver.type_hint == "bytearray":
                                    self.emit(
                                        MoltOp(
                                            kind="BYTEARRAY_REPLACE",
                                            args=[receiver, old, new, count],
                                            result=res,
                                        )
                                    )
                                    return res
            if method == "find" and receiver.type_hint in {"str", "bytes", "bytearray"}:
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("find expects 1-3 arguments")
                needle = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                if receiver.type_hint == "bytes":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_FIND", args=[receiver, needle], result=res
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_FIND",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_FIND", args=[receiver, needle], result=res
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            module_name = (
                self._imported_module_binding_target(obj_name) if obj_name else None
            )
            if module_name is None:
                callee = load_attr_callee()
                # Dynamic attribute calls must use binder semantics so bound methods
                # receive `self` even when local type inference is imprecise.
                return self._emit_dynamic_call(node, callee, True)

        if isinstance(node.func, ast.Attribute):
            module_name = None
            if isinstance(node.func.value, ast.Name):
                module_name = self._imported_module_binding_target(node.func.value.id)
            if module_name:
                func_id = node.func.attr
                normalized = self._normalize_allowlist_module(module_name)
                allowlist_key = normalized or module_name
                if func_id == "field" and allowlist_key == "dataclasses":
                    return self._emit_dataclasses_field_call(allowlist_key, node)
                if func_id == "open" and allowlist_key == "builtins":
                    return self._emit_open_call(node)
                enforce_allowlist = (
                    allowlist_key in MOLT_DIRECT_CALLS
                    or allowlist_key in self.stdlib_allowlist
                )
                force_bind = func_id[
                    :1
                ].isupper() or func_id in MOLT_DIRECT_CALL_BIND_ALWAYS.get(
                    allowlist_key, set()
                )
                if (
                    allowlist_key in MOLT_DIRECT_CALLS
                    and func_id in MOLT_DIRECT_CALLS[allowlist_key]
                ):
                    lowered_imported_call = (
                        self._try_emit_imported_module_direct_or_task_call(
                            allowlist_key,
                            func_id,
                            node,
                            imported_from=module_name,
                            normalized=normalized,
                            needs_bind=needs_bind,
                            force_bind=force_bind,
                            direct_registry_authorized=True,
                        )
                    )
                    if lowered_imported_call is not None:
                        return lowered_imported_call
                if (
                    allowlist_key in self.stdlib_allowlist
                    or self._is_internal_module(module_name)
                    or self._is_known_project_module(module_name)
                ):
                    lowered_handle_ctor = (
                        self._try_emit_intrinsic_handle_class_constructor(
                            allowlist_key,
                            func_id,
                            node,
                        )
                    )
                    if lowered_handle_ctor is not None:
                        return lowered_handle_ctor
                    lowered_imported_call = (
                        self._try_emit_imported_module_direct_or_task_call(
                            allowlist_key,
                            func_id,
                            node,
                            imported_from=module_name,
                            normalized=normalized,
                            needs_bind=needs_bind,
                            force_bind=force_bind,
                            direct_registry_authorized=False,
                        )
                    )
                    if lowered_imported_call is not None:
                        return lowered_imported_call
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    res_hint = func_id if func_id in self.classes else "Any"
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                if enforce_allowlist:
                    suggestion = self._call_allowlist_suggestion(func_id, module_name)
                    if suggestion:
                        alternative = f"use {suggestion}"
                    else:
                        alternative = (
                            "import from an allowlisted module (see docs/spec/"
                            "areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)"
                        )
                    detail = (
                        "Tier 0 only allows direct calls to allowlisted module-level"
                        " functions; rebinding/monkey-patching is not observed"
                    )
                    if suggestion:
                        detail = f"{detail}. warning: allowlisted path is {suggestion}"
                    if self.fallback_policy == "bridge":
                        self.compat.bridge_unavailable(
                            node,
                            f"call to non-allowlisted function '{func_id}'",
                            impact="high",
                            alternative=alternative,
                            detail=detail,
                        )
                        callee = self.visit(node.func)
                        if callee is None:
                            raise NotImplementedError("Unsupported call target")
                        res = MoltValue(self.next_var(), type_hint="Any")
                        if needs_bind:
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                        else:
                            args = self._emit_call_args(node.args)
                            self.emit(
                                MoltOp(
                                    kind="INVOKE_FFI",
                                    args=[callee] + args,
                                    result=res,
                                    metadata={"ffi_lane": "bridge"},
                                )
                            )
                        return res
                    raise self.compat.unsupported(
                        node,
                        f"call to non-allowlisted function '{func_id}'",
                        impact="high",
                        alternative=alternative,
                        detail=detail,
                    )

        if isinstance(node.func, ast.Name):
            func_id = node.func.id
            imported_binding = self.imported_names.get(func_id)
            if (
                imported_binding is None
                and func_id not in self.locals
                and func_id not in self.boxed_locals
            ):
                imported_binding = self.global_imported_names.get(func_id)
            imported_from = imported_binding
            intrinsic_global_symbol = self.module_intrinsic_globals.get(func_id)
            target_info = self.locals.get(func_id)
            if target_info is None and intrinsic_global_symbol is not None:
                target_info = MoltValue(
                    func_id, type_hint=f"Func:{intrinsic_global_symbol}"
                )
            if target_info is None:
                target_info = self.globals.get(func_id)
            is_local = func_id in self.locals or func_id in self.boxed_locals
            if self.is_async() and func_id in self.async_locals:
                loaded = self._load_local_value(func_id)
                if loaded is not None:
                    target_info = loaded
                is_local = True
            if is_local and imported_binding is None:
                imported_from = None
            if imported_from == "builtins":
                imported_attr = self._imported_attr_name(func_id)
                if (
                    imported_attr is not None
                    and imported_attr in _BUILTINS_IMPORT_ALIAS_CALL_NAMES
                ):
                    func_id = imported_attr
            if imported_from:
                normalized = self._normalize_allowlist_module(imported_from)
                allowlist_key = normalized or imported_from
                if func_id == "field" and allowlist_key == "dataclasses":
                    return self._emit_dataclasses_field_call(allowlist_key, node)
                if allowlist_key == "statistics" and func_id in {"mean", "stdev"}:
                    lowered_stats = self._lower_statistics_slice_call(func_id, node)
                    if lowered_stats is not None:
                        return lowered_stats
                original_attr = self._imported_attr_name(func_id)
                known_func_hint = self._known_module_function_type_hint(
                    allowlist_key, original_attr
                )
                if known_func_hint is not None:
                    if target_info is None:
                        target_info = MoltValue(func_id, type_hint=known_func_hint)
                    else:
                        target_info.type_hint = known_func_hint
            # Try lowering _intrinsics.require_intrinsic("name") calls to a
            # BUILTIN_FUNC opcode early, before any local-function dispatch
            # path (e.g. a `def _require_intrinsic(...)` defined in an except
            # handler) can intercept the call and produce a CALL_BIND on a
            # never-assigned sentinel local.
            if self._is_intrinsics_module_name(imported_binding) and func_id in {
                "require_intrinsic",
                "_require_intrinsic",
            }:
                lowered_intrinsic_early = self._try_lower_intrinsic_lookup_call(
                    func_id=func_id,
                    imported_from=imported_binding,
                    node=node,
                )
                if lowered_intrinsic_early is not None:
                    return lowered_intrinsic_early
            if (
                target_info is None
                and self.current_func_name != "molt_main"
                and self.module_declared_funcs.get(func_id) == "sync"
            ):
                func_symbol = self._function_symbol_for_reference(func_id)
                target_info = MoltValue(func_id, type_hint=f"Func:{func_symbol}")
            lowered_wrapper_intrinsic = self._try_lower_local_intrinsic_wrapper_call(
                func_id=func_id,
                node=node,
            )
            if lowered_wrapper_intrinsic is not None:
                return lowered_wrapper_intrinsic
            if imported_from:
                target_module = self._normalize_allowlist_module(imported_from)
                if target_module is None:
                    target_module = imported_from
                original_attr = self._imported_attr_name(func_id)
                lowered_handle_ctor = self._try_emit_intrinsic_handle_class_constructor(
                    target_module,
                    original_attr,
                    node,
                )
                if lowered_handle_ctor is not None:
                    return lowered_handle_ctor
            if func_id in {"BaseExceptionGroup", "ExceptionGroup"}:
                if node.keywords:
                    self._bridge_fallback(
                        node,
                        f"{func_id} with keywords",
                        impact="medium",
                        alternative=f"{func_id} with positional arguments only",
                        detail="keywords are not supported for exception constructors",
                    )
                    return None
                args: list[MoltValue] = []
                for arg in node.args:
                    arg_val = self.visit(arg)
                    if arg_val is None:
                        self._bridge_fallback(
                            node,
                            f"{func_id} with unsupported arg expression",
                            impact="medium",
                            alternative=f"{func_id} with simple literals",
                            detail="argument expression could not be lowered",
                        )
                        return None
                    args.append(arg_val)
                class_val = self._emit_exception_class(func_id)
                return self._emit_exception_new_from_class(class_val, args)
            if func_id in {
                "BaseException",
                "Exception",
                "KeyError",
                "IndexError",
                "ValueError",
                "TypeError",
                "RuntimeError",
                "StopIteration",
            }:
                if node.keywords or any(isinstance(a, ast.Starred) for a in node.args):
                    pass  # fall through to generic call handler
                else:
                    args: list[MoltValue] = []
                    for arg in node.args:
                        arg_val = self.visit(arg)
                        if arg_val is None:
                            self._bridge_fallback(
                                node,
                                f"{func_id} with unsupported arg expression",
                                impact="medium",
                                alternative=f"{func_id} with simple literals",
                                detail="argument expression could not be lowered",
                            )
                            return None
                        args.append(arg_val)
                    return self._emit_exception_new_from_args(func_id, args)
            if func_id == "abs" and len(node.args) == 1 and not node.keywords:
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("abs expects a lowerable operand")
                if value.type_hint in {"bool", "int"}:
                    result_hint = "int"
                elif value.type_hint == "float":
                    result_hint = "float"
                else:
                    result_hint = "Any"
                res = MoltValue(self.next_var(), type_hint=result_hint)
                self.emit(MoltOp(kind="ABS", args=[value], result=res))
                return res
            if func_id == "globals":
                if node.args or node.keywords:
                    count = len(node.args) + len(node.keywords)
                    msg = f"globals() takes no arguments ({count} given)"
                    return self._emit_type_error_value(msg, "dict")
                return self._emit_globals_dict()
            if func_id == "locals":
                if node.args or node.keywords:
                    count = len(node.args) + len(node.keywords)
                    msg = f"locals() takes no arguments ({count} given)"
                    return self._emit_type_error_value(msg, "dict")
                return self._emit_locals_dict()
            if func_id == "vars":
                if node.keywords:
                    return self._emit_type_error_value(
                        "vars() takes no keyword arguments", "dict"
                    )
                if len(node.args) > 1:
                    msg = f"vars() takes at most 1 argument ({len(node.args)} given)"
                    return self._emit_type_error_value(msg, "dict")
                if not node.args:
                    return self._emit_locals_dict()
                obj = self.visit(node.args[0])
                if obj is None:
                    raise NotImplementedError("vars expects a simple object")
                callee = self._emit_builtin_function("vars")
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee, obj], result=res))
                return res
            if func_id == "dir":
                if node.keywords:
                    return self._emit_type_error_value(
                        "dir() takes no keyword arguments", "list"
                    )
                if len(node.args) > 1:
                    msg = f"dir() takes at most 1 argument ({len(node.args)} given)"
                    return self._emit_type_error_value(msg, "list")
                if not node.args:
                    locals_dict = self._emit_locals_dict()
                    keys = MoltValue(self.next_var(), type_hint="dict_keys")
                    self.emit(MoltOp(kind="DICT_KEYS", args=[locals_dict], result=keys))
                    callee = self._emit_builtin_function("sorted")
                    key_none = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_none))
                    reverse_false = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[False], result=reverse_false)
                    )
                    res = MoltValue(self.next_var(), type_hint="list")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[callee, keys, key_none, reverse_false],
                            result=res,
                        )
                    )
                    return res
                obj = self.visit(node.args[0])
                if obj is None:
                    raise NotImplementedError("dir expects a simple object")
                callee = self._emit_builtin_function("dir")
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee, obj], result=res))
                return res
            if func_id == "getattr":
                if len(node.args) not in {2, 3} or node.keywords:
                    raise NotImplementedError("getattr expects 2 or 3 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("getattr expects object and name")
                res_hint = "Any"
                name_lit = None
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    name_lit = node.args[1].value
                if name_lit and obj.type_hint in self.classes:
                    class_info = self.classes[obj.type_hint]
                    if not class_info.get("dynamic"):
                        field_map = class_info.get("fields", {})
                        if name_lit in field_map:
                            if class_info.get("dataclass"):
                                idx_val = MoltValue(self.next_var(), type_hint="int")
                                self.emit(
                                    MoltOp(
                                        kind="CONST",
                                        args=[field_map[name_lit]],
                                        result=idx_val,
                                    )
                                )
                                res = MoltValue(self.next_var())
                                self.emit(
                                    MoltOp(
                                        kind="DATACLASS_GET",
                                        args=[obj, idx_val],
                                        result=res,
                                    )
                                )
                                return res
                            else:
                                obj_name = None
                                assume_exact = False
                                if isinstance(node.args[0], ast.Name):
                                    obj_name = node.args[0].id
                                    assume_exact = (
                                        self.exact_locals.get(obj_name) == obj.type_hint
                                    )
                                return self._emit_guarded_getattr(
                                    obj,
                                    name_lit,
                                    obj.type_hint,
                                    assume_exact=assume_exact,
                                    obj_name=obj_name,
                                )
                if name_lit:
                    class_name = None
                    if obj.type_hint in self.classes:
                        class_name = obj.type_hint
                    elif isinstance(node.args[0], ast.Name):
                        if node.args[0].id in self.classes:
                            class_name = node.args[0].id
                    if class_name:
                        method_info, method_class = self._resolve_method_info(
                            class_name, name_lit
                        )
                        if method_info:
                            descriptor = method_info["descriptor"]
                            if descriptor in {"function", "classmethod"}:
                                method_owner = method_class or class_name
                                res_hint = f"BoundMethod:{method_owner}:{name_lit}"
                            elif descriptor == "staticmethod":
                                res_hint = method_info["func"].type_hint
                res = MoltValue(self.next_var(), type_hint=res_hint)
                if len(node.args) == 3:
                    default = self.visit(node.args[2])
                    if default is None:
                        default = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                    self.emit(
                        MoltOp(
                            kind="GETATTR_NAME_DEFAULT",
                            args=[obj, name, default],
                            result=res,
                        )
                    )
                else:
                    default = self._emit_missing_value()
                    self.emit(
                        MoltOp(
                            kind="GETATTR_NAME_DEFAULT",
                            args=[obj, name, default],
                            result=res,
                        )
                    )
                return res
            if func_id == "setattr":
                if len(node.args) != 3 or node.keywords:
                    raise NotImplementedError("setattr expects 3 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                val = self.visit(node.args[2])
                if obj is None or name is None or val is None:
                    raise NotImplementedError("setattr expects object, name, value")
                attr_name = None
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    attr_name = node.args[1].value
                if attr_name:
                    obj_name = None
                    exact_class = None
                    if isinstance(node.args[0], ast.Name):
                        obj_name = node.args[0].id
                        exact_class = self.exact_locals.get(obj_name)
                    if exact_class is not None:
                        self._record_instance_attr_mutation(exact_class, attr_name)
                    elif obj.type_hint in self.classes:
                        self._record_instance_attr_mutation(obj.type_hint, attr_name)
                if (
                    isinstance(node.args[1], ast.Constant)
                    and isinstance(node.args[1].value, str)
                    and obj.type_hint in self.classes
                ):
                    attr_name = node.args[1].value
                    class_info = self.classes[obj.type_hint]
                    if not class_info.get("dynamic"):
                        field_map = class_info.get("fields", {})
                        if attr_name in field_map:
                            if class_info.get("dataclass"):
                                idx_val = MoltValue(self.next_var(), type_hint="int")
                                self.emit(
                                    MoltOp(
                                        kind="CONST",
                                        args=[field_map[attr_name]],
                                        result=idx_val,
                                    )
                                )
                                self.emit(
                                    MoltOp(
                                        kind="DATACLASS_SET",
                                        args=[obj, idx_val, val],
                                        result=MoltValue("none"),
                                    )
                                )
                                res = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=res)
                                )
                            else:
                                res = MoltValue(self.next_var(), type_hint="None")
                                if self._class_attr_is_data_descriptor(
                                    obj.type_hint, attr_name
                                ):
                                    self.emit(
                                        MoltOp(
                                            kind="SETATTR_GENERIC_PTR",
                                            args=[obj, attr_name, val],
                                            result=res,
                                        )
                                    )
                                else:
                                    assume_exact = (
                                        exact_class is not None
                                        and exact_class == obj.type_hint
                                    )
                                    self._emit_guarded_setattr(
                                        obj,
                                        attr_name,
                                        val,
                                        obj.type_hint,
                                        obj_name=obj_name,
                                        assume_exact=assume_exact,
                                    )
                                    self.emit(
                                        MoltOp(
                                            kind="CONST_NONE",
                                            args=[],
                                            result=res,
                                        )
                                    )
                            return res
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="SETATTR_NAME",
                        args=[obj, name, val],
                        result=res,
                    )
                )
                return res
            if func_id == "delattr":
                if len(node.args) != 2 or node.keywords:
                    raise NotImplementedError("delattr expects 2 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("delattr expects object and name")
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    attr_name = node.args[1].value
                    exact_class = None
                    if isinstance(node.args[0], ast.Name):
                        exact_class = self.exact_locals.get(node.args[0].id)
                    if exact_class is not None:
                        self._record_instance_attr_mutation(exact_class, attr_name)
                    elif obj.type_hint in self.classes:
                        self._record_instance_attr_mutation(obj.type_hint, attr_name)
                    res = MoltValue(self.next_var(), type_hint="None")
                    attr_name = node.args[1].value
                    if obj.type_hint in self.classes:
                        self.emit(
                            MoltOp(
                                kind="DELATTR_GENERIC_PTR",
                                args=[obj, attr_name],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="DELATTR_GENERIC_OBJ",
                                args=[obj, attr_name],
                                result=res,
                            )
                        )
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="DELATTR_NAME",
                        args=[obj, name],
                        result=res,
                    )
                )
                return res
            if func_id == "hasattr":
                if len(node.args) != 2 or node.keywords:
                    raise NotImplementedError("hasattr expects 2 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("hasattr expects object and name")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(
                        kind="HASATTR_NAME",
                        args=[obj, name],
                        result=res,
                    )
                )
                return res
            if func_id == "super":
                if node.keywords:
                    raise NotImplementedError("super does not support keywords")
                if len(node.args) == 0:
                    # Zero-arg ``super()`` reads the class object from the
                    # implicit ``__class__`` closure cell (filled with the
                    # finished class after the class is built) and binds it to
                    # the method's first parameter — exactly mirroring CPython's
                    # ``__build_class__`` / ``super.__init__`` zero-arg path.
                    # Reading the cell rather than re-deriving the class by
                    # module-attribute name makes ``super()`` correct for
                    # function-local, nested, and module-level classes
                    # (including metaclasses) alike.
                    class_ref = (
                        self._emit_free_var_load("__class__")
                        if "__class__" in self.free_vars
                        else None
                    )
                    if (
                        class_ref is not None
                        and self.current_method_first_param is not None
                    ):
                        obj = self._load_local_value(self.current_method_first_param)
                        if (
                            obj is None
                            and self.current_method_first_param in self.free_vars
                        ):
                            obj = self._emit_free_var_load(
                                self.current_method_first_param
                            )
                        if obj is None:
                            raise NotImplementedError("super() missing method receiver")
                        super_hint = (
                            f"super:{self.current_class}"
                            if self.current_class is not None
                            else "super"
                        )
                        res = MoltValue(self.next_var(), type_hint=super_hint)
                        self.emit(
                            MoltOp(kind="SUPER_NEW", args=[class_ref, obj], result=res)
                        )
                        return res
                    if self.current_method_first_param is None:
                        msg = "super(): no arguments"
                    else:
                        msg = "super(): __class__ cell not found"
                    err_val = self._emit_exception_new("RuntimeError", msg)
                    self.emit(
                        MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none"))
                    )
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    return res
                if len(node.args) == 2:
                    type_val = self.visit(node.args[0])
                    obj_val = self.visit(node.args[1])
                    if type_val is None or obj_val is None:
                        raise NotImplementedError("super expects type and object")
                    super_hint = "super"
                    if isinstance(node.args[0], ast.Name):
                        super_hint = f"super:{node.args[0].id}"
                    res = MoltValue(self.next_var(), type_hint=super_hint)
                    self.emit(
                        MoltOp(kind="SUPER_NEW", args=[type_val, obj_val], result=res)
                    )
                    return res
                raise NotImplementedError("super expects 0 or 2 arguments")
            if func_id == "classmethod":
                if len(node.args) != 1 or node.keywords:
                    raise NotImplementedError("classmethod expects 1 argument")
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise NotImplementedError("classmethod expects a function")
                res = MoltValue(self.next_var(), type_hint="classmethod")
                self.emit(MoltOp(kind="CLASSMETHOD_NEW", args=[func_val], result=res))
                return res
            if func_id == "staticmethod":
                if len(node.args) != 1 or node.keywords:
                    raise NotImplementedError("staticmethod expects 1 argument")
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise NotImplementedError("staticmethod expects a function")
                res = MoltValue(self.next_var(), type_hint="staticmethod")
                self.emit(MoltOp(kind="STATICMETHOD_NEW", args=[func_val], result=res))
                return res
            if func_id == "property":
                if any(kw.arg is None for kw in node.keywords):
                    raise NotImplementedError("property does not support **kwargs")
                if len(node.args) > 4:
                    return self._emit_type_error_value(
                        "property expected at most 4 arguments", "property"
                    )
                getter_expr = node.args[0] if len(node.args) > 0 else None
                setter_expr = node.args[1] if len(node.args) > 1 else None
                deleter_expr = node.args[2] if len(node.args) > 2 else None
                doc_expr = node.args[3] if len(node.args) > 3 else None
                for kw in node.keywords:
                    if kw.arg == "fget":
                        if getter_expr is not None:
                            return self._emit_type_error_value(
                                "property() got multiple values for argument 'fget'",
                                "property",
                            )
                        getter_expr = kw.value
                    elif kw.arg == "fset":
                        if setter_expr is not None:
                            return self._emit_type_error_value(
                                "property() got multiple values for argument 'fset'",
                                "property",
                            )
                        setter_expr = kw.value
                    elif kw.arg == "fdel":
                        if deleter_expr is not None:
                            return self._emit_type_error_value(
                                "property() got multiple values for argument 'fdel'",
                                "property",
                            )
                        deleter_expr = kw.value
                    elif kw.arg == "doc":
                        if doc_expr is not None:
                            return self._emit_type_error_value(
                                "property() got multiple values for argument 'doc'",
                                "property",
                            )
                        doc_expr = kw.value
                    else:
                        return self._emit_type_error_value(
                            f"property() got an unexpected keyword argument '{kw.arg}'",
                            "property",
                        )
                if getter_expr is None:
                    getter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=getter))
                else:
                    getter = self.visit(getter_expr)
                    if getter is None:
                        raise NotImplementedError("property expects a getter")
                if setter_expr is None:
                    setter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=setter))
                else:
                    setter = self.visit(setter_expr)
                    if setter is None:
                        raise NotImplementedError("property setter unsupported")
                if deleter_expr is None:
                    deleter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=deleter))
                else:
                    deleter = self.visit(deleter_expr)
                    if deleter is None:
                        raise NotImplementedError("property deleter unsupported")
                res = MoltValue(self.next_var(), type_hint="property")
                self.emit(
                    MoltOp(
                        kind="PROPERTY_NEW",
                        args=[getter, setter, deleter],
                        result=res,
                    )
                )
                if doc_expr is not None:
                    doc_val = self.visit(doc_expr)
                    if doc_val is None:
                        raise NotImplementedError("property doc unsupported")
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_OBJ",
                            args=[res, "__doc__", doc_val],
                            result=MoltValue("none"),
                        )
                    )
                return res
            if func_id == "open":
                return self._emit_open_call(node)
            if func_id == "nullcontext":
                if len(node.args) > 1:
                    raise NotImplementedError("nullcontext expects 0 or 1 argument")
                if node.args:
                    payload = self.visit(node.args[0])
                else:
                    payload = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=payload))
                return self._emit_nullcontext(payload)
            if func_id == "closing":
                if len(node.args) != 1:
                    raise NotImplementedError("closing expects 1 argument")
                payload = self.visit(node.args[0])
                return self._emit_closing(payload)
            if func_id == "print":
                needs_bind = self._call_needs_bind(node)
                if needs_bind:
                    callargs, saw_name_error = self._emit_print_call_args_builder(node)
                    if saw_name_error:
                        return None
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
                if len(node.args) == 0:
                    self.emit(
                        MoltOp(kind="PRINT_NEWLINE", args=[], result=MoltValue("none"))
                    )
                    return None
                args: list[MoltValue] = []
                saw_name_error = False
                for expr in node.args:
                    arg = self.visit(expr)
                    if arg is None:
                        if isinstance(expr, ast.Name):
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{expr.id}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            saw_name_error = True
                            arg = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    args.append(arg)
                if saw_name_error:
                    return None
                if len(args) == 1:
                    self.emit(
                        MoltOp(kind="PRINT", args=[args[0]], result=MoltValue("none"))
                    )
                    return None
                parts = [self._emit_str_from_obj(arg) for arg in args]
                sep = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[" "], result=sep))
                items = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=parts, result=items))
                joined = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_JOIN", args=[sep, items], result=joined))
                self.emit(MoltOp(kind="PRINT", args=[joined], result=MoltValue("none")))
                return None
            elif func_id == "molt_spawn":
                arg = self.visit(node.args[0])
                self.emit(MoltOp(kind="SPAWN", args=[arg], result=MoltValue("none")))
                return None
            elif func_id == "molt_cancel_token_new":
                if node.keywords or len(node.args) > 1:
                    raise NotImplementedError(
                        "molt_cancel_token_new expects 0 or 1 argument"
                    )
                if node.args:
                    parent = self.visit(node.args[0])
                    if parent is None:
                        raise NotImplementedError(
                            "Unsupported parent in molt_cancel_token_new"
                        )
                else:
                    parent = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=parent))
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CANCEL_TOKEN_NEW", args=[parent], result=res))
                return res
            elif func_id == "molt_cancel_token_clone":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_clone expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_CLONE", args=[token], result=res))
                return res
            elif func_id == "molt_cancel_token_drop":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_drop expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_DROP", args=[token], result=res))
                return res
            elif func_id == "molt_cancel_token_cancel":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_cancel expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_CANCEL", args=[token], result=res))
                return res
            elif func_id == "molt_future_cancel":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_future_cancel expects 1 argument")
                future = self.visit(node.args[0])
                if future is None:
                    raise NotImplementedError("Unsupported future")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FUTURE_CANCEL", args=[future], result=res))
                return res
            elif func_id == "molt_future_cancel_msg":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_future_cancel_msg expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                msg = self.visit(node.args[1])
                if future is None or msg is None:
                    raise NotImplementedError("Unsupported future cancel message")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="FUTURE_CANCEL_MSG", args=[future, msg], result=res)
                )
                return res
            elif func_id == "molt_future_cancel_clear":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_future_cancel_clear expects 1 argument"
                    )
                future = self.visit(node.args[0])
                if future is None:
                    raise NotImplementedError("Unsupported future cancel clear")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FUTURE_CANCEL_CLEAR", args=[future], result=res))
                return res
            elif func_id == "molt_promise_new":
                if node.keywords or node.args:
                    raise NotImplementedError("molt_promise_new expects no arguments")
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(MoltOp(kind="PROMISE_NEW", args=[], result=res))
                return res
            elif func_id == "molt_promise_set_result":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_promise_set_result expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                result = self.visit(node.args[1])
                if future is None or result is None:
                    raise NotImplementedError("Unsupported promise set result")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="PROMISE_SET_RESULT", args=[future, result], result=res)
                )
                return res
            elif func_id == "molt_promise_set_exception":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_promise_set_exception expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                exc = self.visit(node.args[1])
                if future is None or exc is None:
                    raise NotImplementedError("Unsupported promise set exception")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="PROMISE_SET_EXCEPTION", args=[future, exc], result=res)
                )
                return res
            elif func_id == "molt_task_register_token_owned":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_task_register_token_owned expects 2 arguments"
                    )
                task = self.visit(node.args[0])
                token = self.visit(node.args[1])
                if task is None or token is None:
                    raise NotImplementedError("Unsupported task token registration")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="TASK_REGISTER_TOKEN_OWNED",
                        args=[task, token],
                        result=res,
                    )
                )
                return res
            elif func_id == "molt_cancel_token_is_cancelled":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_is_cancelled expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="CANCEL_TOKEN_IS_CANCELLED", args=[token], result=res)
                )
                return res
            elif func_id == "molt_cancel_token_set_current":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_set_current expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(kind="CANCEL_TOKEN_SET_CURRENT", args=[token], result=res)
                )
                return res
            elif func_id == "molt_cancel_token_get_current":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_cancel_token_get_current expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CANCEL_TOKEN_GET_CURRENT", args=[], result=res))
                return res
            elif func_id == "molt_cancelled":
                if node.keywords or node.args:
                    raise NotImplementedError("molt_cancelled expects no arguments")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CANCELLED", args=[], result=res))
                return res
            elif func_id == "molt_cancel_current":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_cancel_current expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_CURRENT", args=[], result=res))
                return res
            elif func_id == "molt_block_on":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_block_on expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="ASYNC_BLOCK_ON", args=[arg], result=res))
                self._emit_raise_if_pending()
                return res
            elif func_id == "molt_asyncgen_shutdown":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_asyncgen_shutdown expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="ASYNCGEN_SHUTDOWN", args=[], result=res))
                return res
            elif func_id == "molt_async_sleep":
                if node.keywords or len(node.args) > 2:
                    raise NotImplementedError("molt_async_sleep expects 0-2 arguments")
                args = []
                if node.args:
                    delay_val = self.visit(node.args[0])
                    if delay_val is None:
                        raise NotImplementedError(
                            "Unsupported delay in molt_async_sleep"
                        )
                    args.append(delay_val)
                if len(node.args) == 2:
                    result_val = self.visit(node.args[1])
                    if result_val is None:
                        raise NotImplementedError(
                            "Unsupported result in molt_async_sleep"
                        )
                    args.append(result_val)
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="CALL_ASYNC",
                        args=["molt_async_sleep_poll", *args],
                        result=res,
                    )
                )
                return res
            elif func_id == "molt_thread_submit":
                if node.keywords or len(node.args) != 3:
                    raise NotImplementedError("molt_thread_submit expects 3 arguments")
                callable_val = self.visit(node.args[0])
                args_val = self.visit(node.args[1])
                kwargs_val = self.visit(node.args[2])
                if callable_val is None or args_val is None or kwargs_val is None:
                    raise NotImplementedError("Unsupported thread submit arguments")
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="THREAD_SUBMIT",
                        args=[callable_val, args_val, kwargs_val],
                        result=res,
                    )
                )
                return res
            elif func_id == "molt_chan_new":
                if node.keywords:
                    raise NotImplementedError("molt_chan_new does not support keywords")
                if len(node.args) > 1:
                    raise NotImplementedError("molt_chan_new expects 0 or 1 argument")
                if node.args:
                    capacity = self.visit(node.args[0])
                    if capacity is None:
                        raise NotImplementedError("Unsupported channel capacity")
                else:
                    capacity = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=capacity))
                res = MoltValue(self.next_var(), type_hint="Channel")
                self.emit(MoltOp(kind="CHAN_NEW", args=[capacity], result=res))
                return res
            elif func_id == "molt_chan_send":
                chan = self.visit(node.args[0])
                val = self.visit(node.args[1])
                if not self.is_async():
                    callee = self._emit_builtin_function("molt_chan_send")
                    return self._emit_call_bound_or_func(callee, [chan, val])
                chan_slot = None
                val_slot = None
                chan_for_send = chan
                val_for_send = val
                if self.is_async():
                    chan_slot = self._async_local_offset(
                        f"__chan_send_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, chan],
                            result=MoltValue("none"),
                        )
                    )
                    val_slot = self._async_local_offset(
                        f"__chan_send_val_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", val_slot, val],
                            result=MoltValue("none"),
                        )
                    )
                self.state_count += 1
                pending_state_id = self.state_count
                self.emit(
                    MoltOp(
                        kind="STATE_LABEL",
                        args=[pending_state_id],
                        result=MoltValue("none"),
                    )
                )
                pending_state_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CONST", args=[pending_state_id], result=pending_state_val
                    )
                )
                if self.is_async() and chan_slot is not None and val_slot is not None:
                    chan_for_send = MoltValue(self.next_var(), type_hint="Channel")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", chan_slot],
                            result=chan_for_send,
                        )
                    )
                    val_for_send = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", val_slot],
                            result=val_for_send,
                        )
                    )
                self.state_count += 1
                next_state_id = self.state_count
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CHAN_SEND_YIELD",
                        args=[
                            chan_for_send,
                            val_for_send,
                            pending_state_val,
                            next_state_id,
                        ],
                        result=res,
                    )
                )
                if self.is_async() and chan_slot is not None and val_slot is not None:
                    cleared_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", val_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                return res
            elif func_id == "molt_chan_recv":
                chan = self.visit(node.args[0])
                if not self.is_async():
                    callee = self._emit_builtin_function("molt_chan_recv")
                    return self._emit_call_bound_or_func(callee, [chan])
                chan_slot = None
                chan_for_recv = chan
                if self.is_async():
                    chan_slot = self._async_local_offset(
                        f"__chan_recv_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, chan],
                            result=MoltValue("none"),
                        )
                    )
                self.state_count += 1
                pending_state_id = self.state_count
                self.emit(
                    MoltOp(
                        kind="STATE_LABEL",
                        args=[pending_state_id],
                        result=MoltValue("none"),
                    )
                )
                pending_state_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CONST", args=[pending_state_id], result=pending_state_val
                    )
                )
                if self.is_async() and chan_slot is not None:
                    chan_for_recv = MoltValue(self.next_var(), type_hint="Channel")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", chan_slot],
                            result=chan_for_recv,
                        )
                    )
                self.state_count += 1
                next_state_id = self.state_count
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CHAN_RECV_YIELD",
                        args=[chan_for_recv, pending_state_val, next_state_id],
                        result=res,
                    )
                )
                if self.is_async() and chan_slot is not None:
                    cleared_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                return res
            elif func_id == "molt_chan_drop":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_chan_drop expects 1 argument")
                chan = self.visit(node.args[0])
                if chan is None:
                    raise NotImplementedError("Unsupported channel handle")
                self.emit(
                    MoltOp(kind="CHAN_DROP", args=[chan], result=MoltValue("none"))
                )
                return None
            original_import_attr = self._imported_attr_name(func_id)
            class_id = None
            if func_id in self.classes:
                class_id = func_id
            elif imported_from and original_import_attr in self.classes:
                class_id = original_import_attr
            if class_id is not None and imported_from:
                class_ref = self._emit_module_attr_get_on(imported_from, class_id)
                # Imported class metadata is keyed by the class' export name, but
                # multiple modules can legally export the same simple class name.
                # Dispatch through the imported class object so runtime identity,
                # not frontend metadata keying, owns constructor semantics.
                callargs = self._emit_call_args_builder(node)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[class_ref, callargs],
                        result=res,
                    )
                )
                return res
            if class_id is not None and target_info is not None:
                class_value_name = self.classes[class_id].get("class_value_name")
                if (
                    class_value_name is not None
                    and target_info.name != class_value_name
                ):
                    class_id = None
                elif class_value_name is None and self.current_func_name == "molt_main":
                    class_id = None
            if (
                class_id is not None
                and self.current_func_name == "molt_main"
                and class_id in self.del_targets
                and not self._name_resolves_to_builtin(class_id)
            ):
                # A module-scope class whose name is `del`'d (or shadowed by an
                # `except ... as` target) may be unbound when called; a bare Name
                # read has LOAD_GLOBAL semantics, so resolve through
                # MODULE_GET_GLOBAL (NameError on a missing binding) rather than
                # the static class ref / MODULE_GET_ATTR (AttributeError) the
                # known-class fast path would otherwise take.  Dropping `class_id`
                # routes the callee through the generic Name resolution, which
                # applies the same del-target rule.
                class_id = None
            if class_id is not None:
                class_info = self.classes[class_id]
                if self.current_func_name == "molt_main":
                    # Resolve the class reference through the single audited
                    # static-class resolver, which enforces the chunk-liveness
                    # guard (`__init__.py` `_current_module_static_class_ref`,
                    # lines 4678-4680: `self.globals[class_id].name ==
                    # class_value_name`).  When `molt_main` is split into
                    # multiple `molt_module_chunk_N` functions, a class defined
                    # in chunk N and instantiated in chunk N+M has had its
                    # `class_value_name` SSA value reset out of `self.globals`
                    # at the chunk boundary (`_reset_module_chunk_state`); the
                    # resolver then returns None and we fall back to a
                    # chunk-safe MODULE_GET_ATTR re-fetch.  Trusting
                    # `class_value_name` directly here would materialise a
                    # dangling cross-chunk SSA ref that lowering degrades to a
                    # CONST_STR of the variable name, feeding a string where a
                    # type is expected (task #50).  The `constructor_fold_safe`
                    # gate is preserved: the fast alloc + inlined `__init__`
                    # fold still fires for the in-chunk case, because a live
                    # `constructor_fold_safe` class always satisfies the
                    # resolver's (superset) layout/decoration/mutation guards.
                    static_class_ref = self._current_module_static_class_ref(class_id)
                    if static_class_ref is not None and class_info.get(
                        "constructor_fold_safe"
                    ):
                        class_ref = static_class_ref
                    else:
                        class_ref = self._emit_module_attr_get(class_id)
                else:
                    static_class_ref = self._current_module_static_class_ref(class_id)
                    if static_class_ref is not None:
                        class_ref = static_class_ref
                    else:
                        loop_static_class_ref = self._emit_loop_static_class_ref(
                            class_id
                        )
                        if loop_static_class_ref is not None:
                            class_ref = loop_static_class_ref
                        else:
                            local_class = self._load_local_value(class_id)
                            if local_class is not None:
                                class_ref = local_class
                            else:
                                class_ref = self._emit_module_attr_get(class_id)
                if self._class_is_exception_subclass(class_id, class_info):
                    new_method = class_info.get("methods", {}).get("__new__")
                    if new_method is None:
                        for base_name in class_info.get("mro", [])[1:]:
                            base_info = self.classes.get(base_name)
                            if base_info and base_info.get("methods", {}).get(
                                "__new__"
                            ):
                                new_method = base_info["methods"]["__new__"]
                                break
                    if needs_bind or new_method is not None:
                        callargs = self._emit_call_args_builder(node)
                        res = MoltValue(self.next_var(), type_hint="exception")
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[class_ref, callargs],
                                result=res,
                            )
                        )
                        return res
                    args = self._emit_call_args(node.args)
                    res = self._emit_exception_new_from_class(class_ref, args)
                    init_method = class_info.get("methods", {}).get("__init__")
                    if init_method is None:
                        for base_name in class_info.get("mro", [])[1:]:
                            base_info = self.classes.get(base_name)
                            if base_info and base_info.get("methods", {}).get(
                                "__init__"
                            ):
                                init_method = base_info["methods"]["__init__"]
                                break
                    if init_method is not None:
                        init_func = init_method["func"]
                        target_name = init_func.type_hint.split(":", 1)[1]
                        init_args = [res] + args
                        if init_method.get("has_closure"):
                            # A closure __init__ (e.g. a bare `super()` body
                            # captures the implicit `__class__` cell) compiles
                            # with the cell as its leading parameter; a
                            # bare-name CALL would omit the cell argument and
                            # mis-match the symbol arity (LLVM verifier
                            # rejects; Cranelift only tolerates it when the
                            # cell is never read). Same invariant as the
                            # method-call fold: closure targets never get the
                            # direct symbol CALL — route through the bound
                            # path, which threads the cell via the function
                            # object.
                            init_func_val = self._emit_class_method_func(
                                class_ref, "__init__"
                            )
                            bound_init = MoltValue(self.next_var(), type_hint="method")
                            self.emit(
                                MoltOp(
                                    kind="BOUND_METHOD_NEW",
                                    args=[init_func_val, res],
                                    result=bound_init,
                                )
                            )
                            callargs = self._emit_call_args_builder(node)
                            init_res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[bound_init, callargs],
                                    result=init_res,
                                )
                            )
                            return res
                        func_obj = None
                        param_count = init_method.get("param_count")
                        defaults = init_method.get("defaults", [])
                        kwonly_count = init_method.get("kwonly_count")
                        positional_limit = None
                        if param_count is not None and isinstance(kwonly_count, int):
                            positional_limit = param_count - kwonly_count
                        if param_count is not None:
                            missing = param_count - len(init_args)
                            # Load __init__ whenever a trailing default is filled:
                            # a const default needs the version stamp for the
                            # `__defaults__`-mutation deopt guard, a non-const
                            # default needs the live read.
                            if 0 < missing <= len(defaults):
                                func_obj = self._emit_class_method_func(
                                    class_ref, "__init__"
                                )
                        init_args = self._apply_default_specs(
                            param_count,
                            defaults,
                            init_args,
                            node,
                            call_name=f"{class_id}.__init__",
                            func_obj=func_obj,
                            positional_limit=positional_limit,
                        )
                        if init_args is None:
                            init_func_val = self._emit_class_method_func(
                                class_ref, "__init__"
                            )
                            bound_init = MoltValue(self.next_var(), type_hint="method")
                            self.emit(
                                MoltOp(
                                    kind="BOUND_METHOD_NEW",
                                    args=[init_func_val, res],
                                    result=bound_init,
                                )
                            )
                            callargs = self._emit_call_args_builder(node)
                            init_res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[bound_init, callargs],
                                    result=init_res,
                                )
                            )
                            return res
                        init_res = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(
                                kind="CALL",
                                args=[target_name] + init_args,
                                result=init_res,
                            )
                        )
                    return res
                if class_info.get("dataclass"):
                    static_dataclass = self._try_emit_static_dataclass_constructor(
                        node,
                        class_id,
                        class_info,
                        class_ref,
                    )
                    if static_dataclass is not None:
                        return static_dataclass
                    field_order = class_info["field_order"]
                    name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(kind="CONST_STR", args=[class_id], result=name_val)
                    )
                    field_name_vals: list[MoltValue] = []
                    for field in field_order:
                        field_val = MoltValue(self.next_var(), type_hint="str")
                        self.emit(
                            MoltOp(kind="CONST_STR", args=[field], result=field_val)
                        )
                        field_name_vals.append(field_val)
                    field_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="TUPLE_NEW",
                            args=field_name_vals,
                            result=field_names_tuple,
                        )
                    )
                    missing_val = MoltValue(self.next_var(), type_hint="missing")
                    self.emit(MoltOp(kind="MISSING", args=[], result=missing_val))
                    field_values = [missing_val for _ in field_order]
                    values_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="TUPLE_NEW",
                            args=field_values,
                            result=values_tuple,
                        )
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
                            kind="DATACLASS_NEW",
                            args=[name_val, field_names_tuple, values_tuple, flags_val],
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
                    init_func = self._emit_class_method_func(class_ref, "__init__")
                    bound_init = MoltValue(self.next_var(), type_hint="method")
                    self.emit(
                        MoltOp(
                            kind="BOUND_METHOD_NEW",
                            args=[init_func, res],
                            result=bound_init,
                        )
                    )
                    callargs = self._emit_call_args_builder(node)
                    init_res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[bound_init, callargs],
                            result=init_res,
                        )
                    )
                    return res
                _, new_returns_any = self._class_new_policy(class_id, class_info)

                # Phase-1-sibling class-instantiation fold.
                #
                # When a class has a vanilla layout — no metaclass, no
                # constructor-fold safety proving default `object.__new__`, no
                # closures/varargs/kwargs/defaults on __init__, and the call
                # site supplies positional args
                # only — we can replace the `CALL_BIND(class_ref,
                # callargs)` dispatch (which goes through
                # `type.__call__` → `__new__` → bound-method-init →
                # IC dispatch) with a structurally-equivalent two-op
                # sequence: alloc instance, direct CALL to __init__.
                # Targets bench_struct (`Point(0, 0)` per iter), bench_
                # exception_heavy (`ValueError(i)` per iter), and any
                # tight loop instantiating user types.  Each iteration
                # saves a callargs-builder allocation, the IC slot
                # probe, the bound-method allocation that
                # `type.__call__` does on every call, and the sequence
                # of dispatch-step trampolines around `__init__`.
                # Fast constructor folding is only sound when an upstream class
                # analysis records an explicit proof; simple-name class metadata is
                # not enough to bypass runtime type.__call__ semantics.
                constructor_fold_safe = bool(class_info.get("constructor_fold_safe"))
                if (
                    constructor_fold_safe
                    and not class_info.get("dynamic")
                    and not class_info.get("dataclass")
                    and not class_info.get("custom_metaclass")
                    and not node.keywords
                    and all(not isinstance(a, ast.Starred) for a in node.args)
                ):
                    init_info, init_owner = self._resolve_method_info(
                        class_id, "__init__"
                    )
                    # Treat "no __init__ on this class or any base except
                    # object" as an instantiation that can run the alloc
                    # alone — `object.__init__` is a no-op for the no-arg
                    # case, and we'll only fold when the call has no args.
                    init_is_default = init_info is None or init_owner == "object"
                    if init_is_default and len(node.args) == 0:
                        res = MoltValue(self.next_var(), type_hint=class_id)
                        # Carry the static class-instance payload size
                        # (in bytes, header NOT included) so the
                        # backend's escape-analysis-rewritten
                        # `object_new_bound_stack` arm can size the
                        # Cranelift StackSlot at codegen time.  The
                        # heap arm ignores it (sizing happens at
                        # runtime via `class_layout_size`).
                        class_size_bytes = (
                            class_info.get("size", 0) if class_info else 0
                        )
                        self.emit(
                            MoltOp(
                                kind="OBJECT_NEW_BOUND",
                                args=[class_ref],
                                result=res,
                                metadata={"class_size_bytes": class_size_bytes},
                            )
                        )
                        return res
                    if (
                        init_info is not None
                        and init_info.get("descriptor") == "function"
                        and not init_info.get("has_closure")
                        and not init_info.get("has_vararg")
                        and not init_info.get("has_varkw")
                        and not init_info.get("kwonly_count")
                    ):
                        # Defaults are fine ONLY if the call site supplies
                        # all required positional args, so that no
                        # default-spec evaluation is needed at runtime.
                        # The arg-count match below enforces this.
                        # Constructor fold safety has already proven default
                        # `object.__new__` through the full MRO. Keep this arm
                        # focused on the remaining direct-`__init__` contract.
                        getattribute_info, _ = self._resolve_method_info(
                            class_id, "__getattribute__"
                        )
                        if (
                            getattribute_info is None
                            and (init_owner or class_id) in self.classes
                        ):
                            init_func_val = init_info.get("func")
                            if init_func_val is not None and getattr(
                                init_func_val, "type_hint", ""
                            ).startswith("Func:"):
                                init_symbol = init_func_val.type_hint.split(":", 1)[1]
                                if init_symbol in self.func_symbol_names:
                                    param_count = init_info.get("param_count")
                                    if param_count is not None:
                                        expected_positional = param_count - 1
                                        if len(node.args) == expected_positional:
                                            res = MoltValue(
                                                self.next_var(),
                                                type_hint=class_id,
                                            )
                                            # See sibling site above: payload
                                            # size in bytes carried via
                                            # metadata for the stack-alloc
                                            # lowering.
                                            class_size_bytes = (
                                                class_info.get("size", 0)
                                                if class_info
                                                else 0
                                            )
                                            self.emit(
                                                MoltOp(
                                                    kind="OBJECT_NEW_BOUND",
                                                    args=[class_ref],
                                                    result=res,
                                                    metadata={
                                                        "class_size_bytes": class_size_bytes
                                                    },
                                                )
                                            )
                                            init_args = [
                                                self.visit(a) for a in node.args
                                            ]
                                            if not any(a is None for a in init_args):
                                                # Phase 2 sibling — inline
                                                # __init__ body directly when
                                                # it's a sequence of
                                                # `self.attr = expr`
                                                # assignments.  Eliminates
                                                # the per-iter __init__ CALL
                                                # frame setup that dominates
                                                # bench_struct's overhead;
                                                # the substituted body emits
                                                # STORE_ATTR ops on `res`.
                                                init_assigns = init_info.get(
                                                    "inline_init_assigns"
                                                )
                                                inline_params = init_info.get(
                                                    "inline_params"
                                                )
                                                # Fail-closed cross-module gate
                                                # (mirrors _try_inline_method_call):
                                                # an __init__ value-expression that
                                                # reads a defining-module global
                                                # (recorded in inline_free_names)
                                                # must not be spliced into a
                                                # different module's scope, where
                                                # the global would mis-resolve.
                                                init_free_names = init_info.get(
                                                    "inline_free_names"
                                                )
                                                init_cross_module = False
                                                if init_free_names:
                                                    init_owner_module = init_info.get(
                                                        "inline_owner_module"
                                                    )
                                                    init_cross_module = (
                                                        init_owner_module is not None
                                                        and init_owner_module
                                                        != self.module_name
                                                    )
                                                if (
                                                    init_assigns is not None
                                                    and inline_params is not None
                                                    and not init_cross_module
                                                    and self._try_inline_init_assigns(
                                                        init_assigns,
                                                        inline_params,
                                                        res,
                                                        init_args,
                                                    )
                                                ):
                                                    return res
                                                init_res = MoltValue(
                                                    self.next_var(),
                                                    type_hint="None",
                                                )
                                                self.emit(
                                                    MoltOp(
                                                        kind="CALL",
                                                        args=[init_symbol, res]
                                                        + init_args,
                                                        result=init_res,
                                                    )
                                                )
                                                return res

                callargs = self._emit_call_args_builder(node)
                res_hint = "Any" if new_returns_any else class_id
                res = MoltValue(self.next_var(), type_hint=res_hint)
                metadata = (
                    {"defines_del": True}
                    if not new_returns_any and self._class_defines_finalizer(class_id)
                    else None
                )
                # Route user class construction through the class object so __new__,
                # metaclass __call__, and runtime constructor policy stay coherent.
                #
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[class_ref, callargs],
                        result=res,
                        metadata=metadata,
                    )
                )
                return res

            if target_info and str(target_info.type_hint).startswith(
                ("AsyncFunc:", "AsyncClosureFunc:")
            ):
                target_value = target_info
                if needs_bind:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                parts = target_info.type_hint.split(":")
                func_kind = parts[0]
                poll_func = parts[1]
                closure_size = int(parts[2])
                if poll_func == self.current_func_name:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                func_symbol = (
                    poll_func[: -len("_poll")]
                    if poll_func.endswith("_poll")
                    else poll_func
                )
                args, _ = self._emit_direct_call_args_for_symbol(func_symbol, node)
                if args is None:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var(), type_hint="Future")
                if func_kind == "AsyncClosureFunc":
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[target_value] + args, result=res)
                    )
                else:
                    closure_size = max(
                        closure_size,
                        self._task_closure_size(len(args), include_gen_control=False),
                    )
                    self.emit(
                        MoltOp(
                            kind="ALLOC_TASK",
                            args=[poll_func, closure_size] + args,
                            result=res,
                            metadata={"task_kind": "coroutine"},
                        )
                    )
                return res
            if target_info and str(target_info.type_hint).startswith(
                ("AsyncGenFunc:", "AsyncGenClosureFunc:")
            ):
                target_value = target_info
                if needs_bind:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="async_generator")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                parts = target_info.type_hint.split(":")
                func_kind = parts[0]
                poll_func = parts[1]
                closure_size = int(parts[2])
                if poll_func == self.current_func_name:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="async_generator")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                func_symbol = (
                    poll_func[: -len("_poll")]
                    if poll_func.endswith("_poll")
                    else poll_func
                )
                args, _ = self._emit_direct_call_args_for_symbol(func_symbol, node)
                if args is None:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="async_generator")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var(), type_hint="async_generator")
                if func_kind == "AsyncGenClosureFunc":
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[target_value] + args, result=res)
                    )
                else:
                    closure_size = max(
                        closure_size,
                        self._task_closure_size(len(args), include_gen_control=True),
                    )
                    gen_val = MoltValue(self.next_var(), type_hint="generator")
                    self.emit(
                        MoltOp(
                            kind="ALLOC_TASK",
                            args=[poll_func, closure_size] + args,
                            result=gen_val,
                            metadata={"task_kind": "generator"},
                        )
                    )
                    self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
                return res
            if target_info and str(target_info.type_hint).startswith(
                ("GenFunc:", "GenClosureFunc:")
            ):
                target_value = target_info
                if needs_bind:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="generator")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                parts = target_info.type_hint.split(":")
                func_kind = parts[0]
                poll_func = parts[1]
                closure_size = int(parts[2])
                if poll_func == self.current_func_name:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="generator")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                func_symbol = (
                    poll_func[: -len("_poll")]
                    if poll_func.endswith("_poll")
                    else poll_func
                )
                args, _ = self._emit_direct_call_args_for_symbol(func_symbol, node)
                if args is None:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="generator")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                if func_kind == "GenClosureFunc":
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    closure_val = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="FUNCTION_CLOSURE_BITS",
                            args=[target_value],
                            result=closure_val,
                        )
                    )
                    args = [closure_val] + args
                closure_size = max(
                    closure_size,
                    self._task_closure_size(len(args), include_gen_control=True),
                )
                res = MoltValue(self.next_var(), type_hint="generator")
                self.emit(
                    MoltOp(
                        kind="ALLOC_TASK",
                        args=[poll_func, closure_size] + args,
                        result=res,
                        metadata={"task_kind": "generator"},
                    )
                )
                return res

            if target_info and str(target_info.type_hint).startswith("BoundMethod:"):
                res_hint = "Any"
                class_name = "Unknown"
                method_name = "method"
                method_info = None
                return_hint = None
                parts = target_info.type_hint.split(":", 2)
                if len(parts) == 3:
                    class_name = parts[1]
                    method_name = parts[2]
                    method_info = (
                        self.classes.get(class_name, {})
                        .get("methods", {})
                        .get(method_name)
                    )
                    if method_info:
                        return_hint = method_info["return_hint"]
                    # Propagate builtin return types (int/float/bool/str/etc),
                    # not just user classes — otherwise method-call results in
                    # tight loops fall back to a NaN-boxed accumulator and the
                    # downstream lane inference forces float arithmetic.
                    if return_hint and (
                        return_hint in self.classes or return_hint in BUILTIN_TYPE_TAGS
                    ):
                        res_hint = return_hint
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_info, callargs],
                            result=res,
                        )
                    )
                    return res
                args = self._emit_call_args(node.args)
                if method_info:
                    func_obj = None
                    param_count = method_info.get("param_count")
                    defaults = method_info.get("defaults", [])
                    kwonly_count = method_info.get("kwonly_count")
                    positional_limit = None
                    if param_count is not None and isinstance(kwonly_count, int):
                        positional_limit = param_count - kwonly_count
                    if param_count is not None:
                        missing = param_count - (len(args) + 1)
                        # Load the bound method's function whenever a trailing
                        # default is filled: a const default needs the version
                        # stamp for the `__defaults__`-mutation deopt guard, a
                        # non-const default needs the live read.
                        if 0 < missing <= len(defaults):
                            func_obj = self._emit_bound_method_func(target_info)
                    args = self._apply_default_specs(
                        param_count,
                        defaults,
                        args,
                        node,
                        call_name=f"{class_name}.{method_name}",
                        func_obj=func_obj,
                        implicit_self=True,
                        positional_limit=positional_limit,
                    )
                    if args is None:
                        callargs = self._emit_call_args_builder(node)
                        res = MoltValue(self.next_var(), type_hint=res_hint)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[target_info, callargs],
                                result=res,
                            )
                        )
                        return res
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="CALL_METHOD", args=[target_info] + args, result=res)
                )
                return res

            if target_info and str(target_info.type_hint).startswith("Func:"):
                target_name = target_info.type_hint.split(":")[1]
                intrinsic_target = _intrinsic_arity_exact(target_name) is not None
                res_hint = self._function_result_hint(target_name)
                direct_ok = intrinsic_target or target_name in self.func_default_specs
                if not direct_ok:
                    func_name = self.func_symbol_names.get(target_name)
                    if func_name and self._lookup_func_defaults(None, func_name):
                        direct_ok = True
                    elif self._known_function_symbol_target(target_name) is not None:
                        direct_ok = True
                if needs_bind or not direct_ok:
                    callargs = self._emit_call_args_builder(node)
                    callee = target_info
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        callee = self._emit_module_attr_get(func_id)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                args, func_obj = self._emit_direct_call_args_for_symbol(
                    target_name, node
                )
                if args is None:
                    callargs = self._emit_call_args_builder(node)
                    callee = target_info
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        callee = self._emit_module_attr_get(func_id)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var(), type_hint=res_hint)
                if (
                    intrinsic_target
                    or self.is_async()
                    or (
                        isinstance(node.func, ast.Name)
                        and node.func.id in self.stable_module_funcs
                    )
                ):
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=res)
                    )
                else:
                    callee = func_obj or self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    self.emit(
                        MoltOp(
                            kind="CALL_GUARDED",
                            args=[callee] + args,
                            result=res,
                            metadata={"target": target_name},
                        )
                    )
                return res

            if target_info is not None and func_id in self.locals:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                if needs_bind:
                    res = MoltValue(self.next_var(), type_hint="Any")
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                else:
                    if isinstance(
                        callee.type_hint, str
                    ) and callee.type_hint.startswith("Func:"):
                        func_symbol = callee.type_hint.split(":", 1)[1]
                        res_hint = self._function_result_hint(func_symbol)
                        if func_symbol not in self.func_default_specs:
                            res = MoltValue(self.next_var(), type_hint=res_hint)
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        args, func_obj = self._emit_direct_call_args_for_symbol(
                            func_symbol, node, func_obj=callee
                        )
                        if args is None:
                            callargs = self._emit_call_args_builder(node)
                            res = MoltValue(self.next_var(), type_hint=res_hint)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        res = MoltValue(self.next_var(), type_hint=res_hint)
                        self.emit(
                            MoltOp(
                                kind="CALL_GUARDED",
                                args=[func_obj or callee] + args,
                                result=res,
                                metadata={"target": func_symbol},
                            )
                        )
                        return res
                    if isinstance(
                        callee.type_hint, str
                    ) and callee.type_hint.startswith("ClosureFunc:"):
                        func_symbol = callee.type_hint.split(":", 1)[1]
                        res_hint = self._function_result_hint(func_symbol)
                        if func_symbol not in self.func_default_specs:
                            res = MoltValue(self.next_var(), type_hint=res_hint)
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        args, _ = self._emit_direct_call_args_for_symbol(
                            func_symbol, node, func_obj=callee
                        )
                        if args is None:
                            callargs = self._emit_call_args_builder(node)
                            res = MoltValue(self.next_var(), type_hint=res_hint)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        res = MoltValue(self.next_var(), type_hint=res_hint)
                        self.emit(
                            MoltOp(
                                kind="CALL_GUARDED",
                                args=[callee] + args,
                                result=res,
                                metadata={"target": func_symbol},
                            )
                        )
                        return res
                    if imported_from:
                        imported_info = self._lookup_func_defaults(
                            imported_from, func_id
                        )
                        if imported_info is None or imported_info.get("kwonly"):
                            callargs = self._emit_call_args_builder(node)
                            res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                    args = self._emit_call_args(node.args)
                    if imported_from:
                        args = self._apply_direct_call_defaults(
                            imported_from, func_id, args, node
                        )
                        if args is None:
                            callargs = self._emit_call_args_builder(node)
                            res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res

            if func_id == "type":
                if node.keywords or len(node.args) != 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="type")
                self.emit(MoltOp(kind="TYPE_OF", args=[arg], result=res))
                return res
            if func_id == "isinstance":
                if len(node.args) != 2:
                    raise NotImplementedError("isinstance expects 2 arguments")
                obj = self.visit(node.args[0])
                clsinfo = self.visit(node.args[1])
                if obj is None or clsinfo is None:
                    raise NotImplementedError("Unsupported isinstance arguments")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="ISINSTANCE", args=[obj, clsinfo], result=res))
                return res
            if func_id == "issubclass":
                if len(node.args) != 2:
                    raise NotImplementedError("issubclass expects 2 arguments")
                sub = self.visit(node.args[0])
                clsinfo = self.visit(node.args[1])
                if sub is None or clsinfo is None:
                    raise NotImplementedError("Unsupported issubclass arguments")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="ISSUBCLASS", args=[sub, clsinfo], result=res))
                return res
            if func_id == "object":
                if node.args:
                    raise NotImplementedError("object expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="object")
                self.emit(MoltOp(kind="OBJECT_NEW", args=[], result=res))
                return res
            if func_id == "len":
                if node.keywords:
                    raise NotImplementedError("len does not support keywords")
                if len(node.args) != 1:
                    from molt.compat import CompatibilityIssue

                    issue = CompatibilityIssue(
                        feature="len() argument count",
                        tier="unsupported",
                        impact="high",
                        location=f"line {node.lineno}",
                        detail=f"len() takes exactly one argument ({len(node.args)} given)",
                    )
                    raise NotImplementedError(issue.format_error())
                # Constant-fold len() on string/bytes literals and
                # list/tuple literals with all-constant elements.
                raw_arg = node.args[0]
                if isinstance(raw_arg, ast.Constant) and isinstance(
                    raw_arg.value, (str, bytes)
                ):
                    folded_len = len(raw_arg.value)
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[folded_len], result=res))
                    return res
                if isinstance(raw_arg, (ast.List, ast.Tuple)) and all(
                    isinstance(e, ast.Constant) for e in raw_arg.elts
                ):
                    folded_len = len(raw_arg.elts)
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[folded_len], result=res))
                    return res
                arg = self.visit(node.args[0])
                spec = self._intrinsic_handle_class_spec_for_value(arg)
                if spec is not None and spec.len_intrinsic is not None:
                    return self._emit_intrinsic_handle_class_call(
                        arg,
                        spec,
                        spec.len_intrinsic,
                        [],
                        result_hint="int",
                    )
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LEN", args=[arg], result=res))
                return res
            if func_id == "id":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("id expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported id argument")
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ID", args=[arg], result=res))
                return res
            if func_id == "bool":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=res))
                    return res
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported bool argument")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="BOOL", args=[arg], result=res))
                return res
            if func_id == "ord":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("ord expects 1 argument")
                raw_arg = node.args[0]
                if isinstance(raw_arg, ast.Subscript) and not isinstance(
                    raw_arg.slice, ast.Slice
                ):
                    target = self.visit(raw_arg.value)
                    index_val = self.visit(raw_arg.slice)
                    if target is None or index_val is None:
                        raise NotImplementedError("Unsupported ord subscript argument")
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="ORD_AT", args=[target, index_val], result=res)
                    )
                    return res
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported ord argument")
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ORD", args=[arg], result=res))
                return res
            if func_id == "chr":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("chr expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported chr argument")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CHR", args=[arg], result=res))
                return res
            if func_id == "repr":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("repr expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported repr argument")
                return self._emit_repr_from_obj(arg)
            if func_id == "callable":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("callable expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported callable argument")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS_CALLABLE", args=[arg], result=res))
                return res
            if func_id == "str":
                # CPython str() signatures:
                #   str() → ''
                #   str(object) → str(object)
                #   str(object=x) → str(x)
                #   str(bytes, encoding) → decoded str
                #   str(bytes, encoding, errors) → decoded str
                #   str(bytes, encoding=..., errors=...) → decoded str
                kw_object = next(
                    (kw.value for kw in node.keywords if kw.arg == "object"), None
                )
                kw_encoding = next(
                    (kw.value for kw in node.keywords if kw.arg == "encoding"), None
                )
                known_kw = {"object", "encoding", "errors"}
                has_unsupported_kw = any(
                    kw.arg not in known_kw for kw in node.keywords if kw.arg is not None
                )
                has_star_kw = any(kw.arg is None for kw in node.keywords)
                if has_unsupported_kw or has_star_kw or len(node.args) > 3:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                # str(bytes_obj, encoding[, errors]) — decode bytes to str
                # Fall through to dynamic call which the runtime handles via
                # the str() builtin's multi-arg path.
                has_encoding = len(node.args) >= 2 or kw_encoding is not None
                if has_encoding:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                # str() → ''
                if not node.args and kw_object is None:
                    res = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[""], result=res))
                    return res
                # str(object) or str(object=x)
                if node.args:
                    arg = self.visit(node.args[0])
                elif kw_object is not None:
                    arg = self.visit(kw_object)
                else:
                    arg = None
                if arg is None:
                    arg = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
                return self._emit_str_from_obj(arg)
            if func_id == "range":
                if node.keywords:
                    for keyword in node.keywords:
                        val = self.visit(keyword.value)
                        if val is None:
                            raise NotImplementedError("Unsupported range keyword")
                    return self._emit_type_error_value(
                        "range() takes no keyword arguments", "range"
                    )
                range_args = self._parse_range_call(node)
                if range_args is None:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                start, stop, step, _lowerable = range_args
                res = MoltValue(self.next_var(), type_hint="range")
                self.emit(
                    MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=res)
                )
                return res
            if func_id == "enumerate":
                if len(node.args) > 2:
                    raise NotImplementedError("enumerate expects 1 or 2 arguments")
                if node.keywords:
                    for keyword in node.keywords:
                        if keyword.arg is None:
                            raise NotImplementedError(
                                "enumerate does not support **kwargs"
                            )
                        if keyword.arg != "start":
                            raise NotImplementedError(
                                f"enumerate got unexpected keyword {keyword.arg}"
                            )
                iterable = self.visit(node.args[0]) if node.args else None
                if iterable is None:
                    raise NotImplementedError("Unsupported enumerate iterable")
                start_val = None
                has_start = False
                if len(node.args) == 2:
                    start_val = self.visit(node.args[1])
                    has_start = True
                for keyword in node.keywords:
                    if keyword.arg == "start":
                        if has_start:
                            raise NotImplementedError(
                                "enumerate got multiple values for start"
                            )
                        start_val = self.visit(keyword.value)
                        has_start = True
                if start_val is None:
                    start_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
                has_start_val = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="CONST_BOOL", args=[has_start], result=has_start_val)
                )
                res = MoltValue(self.next_var(), type_hint="iter")
                self.emit(
                    MoltOp(
                        kind="ENUMERATE",
                        args=[iterable, start_val, has_start_val],
                        result=res,
                    )
                )
                return res
            if func_id == "slice":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("slice expects 1-3 arguments")
                if len(node.args) == 1:
                    start = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                    stop = self.visit(node.args[0])
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                elif len(node.args) == 2:
                    start = self.visit(node.args[0])
                    stop = self.visit(node.args[1])
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                else:
                    start = self.visit(node.args[0])
                    stop = self.visit(node.args[1])
                    step = self.visit(node.args[2])
                res = MoltValue(self.next_var(), type_hint="slice")
                self.emit(
                    MoltOp(kind="SLICE_NEW", args=[start, stop, step], result=res)
                )
                return res
            if func_id == "aiter":
                if len(node.args) != 1:
                    raise NotImplementedError("aiter expects 1 argument")
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported iterable in aiter()")
                return self._emit_aiter(iterable)
            if func_id == "anext":
                if node.keywords or len(node.args) not in (1, 2):
                    raise NotImplementedError(
                        "anext expects 1 or 2 positional arguments"
                    )
                iter_obj = self.visit(node.args[0])
                if iter_obj is None:
                    raise NotImplementedError("Unsupported iterator in anext()")
                if len(node.args) == 2:
                    default_val = self.visit(node.args[1])
                    if default_val is None:
                        raise NotImplementedError("Unsupported default in anext()")
                    placeholder = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(
                        MoltOp(
                            kind="CALL_ASYNC",
                            args=[
                                "molt_anext_default_poll",
                                iter_obj,
                                default_val,
                                placeholder,
                            ],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(MoltOp(kind="ANEXT", args=[iter_obj], result=res))
                return res
            if func_id in {"any", "all"}:
                # Inline any(genexpr)/all(genexpr) as a short-circuiting loop.
                is_any = func_id == "any"
                if (
                    len(node.args) == 1
                    and not node.keywords
                    and isinstance(node.args[0], ast.GeneratorExp)
                ):
                    genexpr = node.args[0]
                    if (
                        len(genexpr.generators) == 1
                        and not genexpr.generators[0].is_async
                        and isinstance(genexpr.generators[0].target, ast.Name)
                    ):
                        comp = genexpr.generators[0]
                        target = cast(ast.Name, comp.target)
                        target_name = target.id
                        iterable_val = self.visit(comp.iter)
                        iter_obj = self._emit_iter_new(iterable_val)
                        # Initial result: False for any(), True for all().
                        res = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(
                                kind="CONST_BOOL",
                                args=[not is_any],
                                result=res,
                            )
                        )
                        zero = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                        one = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[1], result=one))
                        res_slot = f"__molt_{func_id}_result_{self.next_var()}"
                        self.emit(
                            MoltOp(
                                kind="STORE_VAR",
                                args=[res],
                                result=MoltValue("none"),
                                metadata={"var": res_slot},
                            )
                        )
                        # Save/restore boxed cell for scoping.
                        cell = self._load_boxed_cell(target_name)
                        saved_cell_val: MoltValue | None = None
                        if cell is not None:
                            _save_idx = MoltValue(self.next_var(), type_hint="int")
                            self.emit(MoltOp(kind="CONST", args=[0], result=_save_idx))
                            saved_cell_val = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="INDEX",
                                    args=[cell, _save_idx],
                                    result=saved_cell_val,
                                )
                            )
                        self.emit(
                            MoltOp(kind="LOOP_START", args=[], result=MoltValue("none"))
                        )
                        pair = self._emit_iter_next_checked(iter_obj)
                        done = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
                        self.emit(
                            MoltOp(
                                kind="LOOP_BREAK_IF_TRUE",
                                args=[done],
                                result=MoltValue("none"),
                            )
                        )
                        iter_elem_hint = (
                            self._iterable_element_hint(iterable_val) or "Any"
                        )
                        item = MoltValue(self.next_var(), type_hint=iter_elem_hint)
                        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
                        # Bind loop variable.  CPython treats the genexpr target as
                        # belonging to the (separate) genexpr scope, so it must not
                        # bleed into the enclosing function's binding for the same
                        # name.  Since we're inlining the comp body into the
                        # outer function we re-use ``self.locals[target_name]`` as
                        # storage, but we must prevent ``_load_local_value`` from
                        # routing reads of ``target_name`` through the outer
                        # function's LOAD_VAR slot — that slot belongs to a
                        # different (outer) variable in CPython's model and may
                        # be unbound or hold an unrelated value.  Drop the name
                        # from ``scope_assigned`` and ``unbound_check_names`` so
                        # reads inside the body fall through to the cached
                        # ``item`` MoltValue (or to a boxed cell when one
                        # exists, handled below).  Both sets are restored after
                        # the loop emission completes.
                        old_local = self.locals.get(target_name)
                        target_in_scope_assigned = target_name in self.scope_assigned
                        target_in_unbound_check = (
                            target_name in self.unbound_check_names
                        )
                        if target_in_scope_assigned:
                            self.scope_assigned.discard(target_name)
                        if target_in_unbound_check:
                            self.unbound_check_names.discard(target_name)
                        self.locals[target_name] = item
                        if cell is not None:
                            _box_idx = MoltValue(self.next_var(), type_hint="int")
                            self.emit(MoltOp(kind="CONST", args=[0], result=_box_idx))
                            self.emit(
                                MoltOp(
                                    kind="STORE_INDEX",
                                    args=[cell, _box_idx, item],
                                    result=MoltValue("none"),
                                )
                            )
                        # Evaluate optional filter conditions.
                        for if_node in comp.ifs:
                            cond_val = self.visit(if_node)
                            not_cond = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(kind="NOT", args=[cond_val], result=not_cond)
                            )
                            self.emit(
                                MoltOp(
                                    kind="IF",
                                    args=[not_cond],
                                    result=MoltValue("none"),
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="LOOP_CONTINUE",
                                    args=[],
                                    result=MoltValue("none"),
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="END_IF",
                                    args=[],
                                    result=MoltValue("none"),
                                )
                            )
                        # Evaluate the element expression.
                        elt_val = self.visit(genexpr.elt)
                        # Test truthiness via NOT+NOT (coerces to bool).
                        neg = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(MoltOp(kind="NOT", args=[elt_val], result=neg))
                        truth = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(MoltOp(kind="NOT", args=[neg], result=truth))
                        if is_any:
                            # any: if element is truthy, set True and break.
                            self.emit(
                                MoltOp(
                                    kind="IF",
                                    args=[truth],
                                    result=MoltValue("none"),
                                )
                            )
                            true_val = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[True],
                                    result=true_val,
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="STORE_VAR",
                                    args=[true_val],
                                    result=MoltValue("none"),
                                    metadata={"var": res_slot},
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="LOOP_BREAK",
                                    args=[],
                                    result=MoltValue("none"),
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="END_IF",
                                    args=[],
                                    result=MoltValue("none"),
                                )
                            )
                        else:
                            # all: if element is falsy, set False and break.
                            self.emit(
                                MoltOp(kind="IF", args=[neg], result=MoltValue("none"))
                            )
                            false_val = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[False],
                                    result=false_val,
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="STORE_VAR",
                                    args=[false_val],
                                    result=MoltValue("none"),
                                    metadata={"var": res_slot},
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="LOOP_BREAK",
                                    args=[],
                                    result=MoltValue("none"),
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="END_IF",
                                    args=[],
                                    result=MoltValue("none"),
                                )
                            )
                        # Restore previous binding so the enclosing scope is
                        # untouched by the inlined genexpr (CPython parity: the
                        # genexpr target lives in its own scope).
                        if old_local is not None:
                            self.locals[target_name] = old_local
                        else:
                            self.locals.pop(target_name, None)
                        if target_in_scope_assigned:
                            self.scope_assigned.add(target_name)
                        if target_in_unbound_check:
                            self.unbound_check_names.add(target_name)
                        self.emit(
                            MoltOp(
                                kind="LOOP_CONTINUE",
                                args=[],
                                result=MoltValue("none"),
                            )
                        )
                        self.emit(
                            MoltOp(kind="LOOP_END", args=[], result=MoltValue("none"))
                        )
                        # Restore boxed cell after the loop.
                        if cell is not None and saved_cell_val is not None:
                            _post_idx = MoltValue(self.next_var(), type_hint="int")
                            self.emit(MoltOp(kind="CONST", args=[0], result=_post_idx))
                            self.emit(
                                MoltOp(
                                    kind="STORE_INDEX",
                                    args=[cell, _post_idx, saved_cell_val],
                                    result=MoltValue("none"),
                                )
                            )
                        # Read the loop-carried scalar result.
                        final_res = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(
                                kind="LOAD_VAR",
                                args=[],
                                result=final_res,
                                metadata={"var": res_slot},
                            )
                        )
                        return final_res
                # Fallback: call the builtin normally.
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="bool")
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                else:
                    args = self._emit_call_args(node.args)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res
            if func_id == "sum":
                if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
                    kw.arg is None for kw in node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if needs_bind:
                        callargs = self._emit_call_args_builder(node)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND", args=[callee, callargs], result=res
                            )
                        )
                    else:
                        args = self._emit_call_args(node.args)
                        self.emit(
                            MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                        )
                    return res
                if not node.args:
                    return self._emit_type_error_value(
                        "sum expected at least 1 argument, got 0"
                    )
                if len(node.args) > 2:
                    return self._emit_type_error_value(
                        f"sum expected at most 2 arguments, got {len(node.args)}"
                    )
                if len(node.args) == 1 and not node.keywords:
                    inline_sum = self._try_emit_inline_sum_genexpr(node)
                    if inline_sum is not None:
                        return inline_sum
                start_expr = None
                has_start = False
                if len(node.args) == 2:
                    start_expr = node.args[1]
                    has_start = True
                for keyword in node.keywords:
                    if keyword.arg != "start":
                        msg = (
                            f"sum() got an unexpected keyword argument '{keyword.arg}'"
                        )
                        return self._emit_type_error_value(msg)
                    if has_start:
                        return self._emit_type_error_value(
                            "sum() got multiple values for argument 'start'"
                        )
                    start_expr = keyword.value
                    has_start = True
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported sum iterable")
                if start_expr is None:
                    start_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
                else:
                    start_val = self.visit(start_expr)
                    if start_val is None:
                        raise NotImplementedError("Unsupported sum start value")
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC", args=[callee, iterable, start_val], result=res
                    )
                )
                return res
            if func_id == "map":
                if (
                    any(isinstance(arg, ast.Starred) for arg in node.args)
                    or node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
                if len(node.args) < 2:
                    return self._emit_type_error_value(
                        "map() must have at least two arguments"
                    )
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                callargs = self._emit_call_args_builder(node)
                self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
                return res
            if func_id == "zip":
                if (
                    any(isinstance(arg, ast.Starred) for arg in node.args)
                    or node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                callargs = self._emit_call_args_builder(node)
                self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
                return res
            if func_id in {"min", "max"}:
                if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
                    kw.arg is None for kw in node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if needs_bind:
                        callargs = self._emit_call_args_builder(node)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND", args=[callee, callargs], result=res
                            )
                        )
                    else:
                        args = self._emit_call_args(node.args)
                        self.emit(
                            MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                        )
                    return res
                if not node.args:
                    return self._emit_type_error_value(
                        f"{func_id} expected at least 1 argument, got 0"
                    )
                key_expr = None
                default_expr = None
                for keyword in node.keywords:
                    if keyword.arg not in {"key", "default"}:
                        msg = (
                            f"{func_id}() got an unexpected keyword argument "
                            f"'{keyword.arg}'"
                        )
                        return self._emit_type_error_value(msg)
                    if keyword.arg == "key":
                        if key_expr is not None:
                            return self._emit_type_error_value(
                                f"{func_id}() got multiple values for argument 'key'"
                            )
                        key_expr = keyword.value
                    else:
                        if default_expr is not None:
                            return self._emit_type_error_value(
                                f"{func_id}() got multiple values for argument 'default'"
                            )
                        default_expr = keyword.value
                if len(node.args) > 1 and default_expr is not None:
                    msg = (
                        f"Cannot specify a default for {func_id}() with "
                        "multiple positional arguments"
                    )
                    return self._emit_type_error_value(msg)
                res = MoltValue(self.next_var(), type_hint="Any")
                if node.keywords:
                    callee = self._emit_builtin_function(func_id)
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                else:
                    runtime_name = BUILTIN_FUNC_SPECS[func_id].runtime
                    callee = self._emit_runtime_function(runtime_name, 3)
                    arg_vals: list[MoltValue] = []
                    for expr in node.args:
                        arg_val = self.visit(expr)
                        if arg_val is None:
                            raise NotImplementedError(
                                f"Unsupported {func_id} positional argument"
                            )
                        arg_vals.append(arg_val)
                    args_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=arg_vals, result=args_tuple)
                    )
                    key_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_val))
                    default_val = self._emit_missing_value()
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[callee, args_tuple, key_val, default_val],
                            result=res,
                        )
                    )
                return res
            if func_id == "sorted":
                if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
                    kw.arg is None for kw in node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if needs_bind:
                        callargs = self._emit_call_args_builder(node)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND", args=[callee, callargs], result=res
                            )
                        )
                    else:
                        args = self._emit_call_args(node.args)
                        self.emit(
                            MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                        )
                    return res
                if not node.args:
                    return self._emit_type_error_value(
                        "sorted expected 1 argument, got 0"
                    )
                if len(node.args) > 1:
                    return self._emit_type_error_value(
                        f"sorted expected 1 argument, got {len(node.args)}"
                    )
                key_expr = None
                reverse_expr = None
                for keyword in node.keywords:
                    if keyword.arg not in {"key", "reverse"}:
                        msg = (
                            "sorted() got an unexpected keyword argument "
                            f"'{keyword.arg}'"
                        )
                        return self._emit_type_error_value(msg)
                    if keyword.arg == "key":
                        if key_expr is not None:
                            return self._emit_type_error_value(
                                "sorted() got multiple values for argument 'key'"
                            )
                        key_expr = keyword.value
                    else:
                        if reverse_expr is not None:
                            return self._emit_type_error_value(
                                "sorted() got multiple values for argument 'reverse'"
                            )
                        reverse_expr = keyword.value
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported sorted iterable")
                # Emit key argument (default: None)
                if key_expr is not None:
                    key_val = self.visit(key_expr)
                    if key_val is None:
                        raise NotImplementedError("Unsupported sorted key expression")
                else:
                    key_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_val))
                # Emit reverse argument (default: False)
                if reverse_expr is not None:
                    reverse_val = self.visit(reverse_expr)
                    if reverse_val is None:
                        raise NotImplementedError(
                            "Unsupported sorted reverse expression"
                        )
                else:
                    reverse_val = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[False], result=reverse_val)
                    )
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[callee, iterable, key_val, reverse_val],
                        result=res,
                    )
                )
                return res
            if func_id == "iter":
                if node.keywords:
                    return self._emit_type_error_value(
                        "iter() takes no keyword arguments", "iter"
                    )
                if len(node.args) == 1:
                    iterable = self.visit(node.args[0])
                    if iterable is None:
                        raise NotImplementedError("Unsupported iterable in iter()")
                    return self._emit_iter_new(iterable)
                if len(node.args) == 2:
                    callable_val = self.visit(node.args[0])
                    sentinel_val = self.visit(node.args[1])
                    if callable_val is None or sentinel_val is None:
                        raise NotImplementedError("Unsupported iter() arguments")
                    callee = MoltValue(self.next_var(), type_hint="function")
                    self.emit(
                        MoltOp(
                            kind="BUILTIN_FUNC",
                            args=["molt_iter_sentinel", 2],
                            result=callee,
                        )
                    )
                    self._emit_function_metadata(
                        callee,
                        name="iter",
                        qualname="iter",
                        posonly_params=["callable", "sentinel"],
                        pos_or_kw_params=[],
                        kwonly_params=[],
                        vararg=None,
                        varkw=None,
                        default_exprs=[],
                        kw_default_exprs=[],
                        docstring=None,
                        module_override="builtins",
                    )
                    res = MoltValue(self.next_var(), type_hint="iter")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[callee, callable_val, sentinel_val],
                            result=res,
                        )
                    )
                    return res
                if not node.args:
                    return self._emit_type_error_value(
                        "iter expected 1 argument, got 0", "iter"
                    )
                msg = f"iter expected at most 2 arguments, got {len(node.args)}"
                return self._emit_type_error_value(msg, "iter")
            if func_id == "list":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="list")
                    self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
                    return res
                range_args = self._parse_range_call(node.args[0])
                if range_args is not None:
                    start, stop, step, lowerable = range_args
                    if lowerable:
                        return self._emit_range_list(start, stop, step)
                    range_obj = self._emit_range_obj_from_args(start, stop, step)
                    return self._emit_list_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported list input")
                return self._emit_list_from_iter(iterable)
            if func_id == "tuple":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=res))
                    return res
                range_args = self._parse_range_call(node.args[0])
                if range_args is not None:
                    start, stop, step, _lowerable = range_args
                    range_obj = self._emit_range_obj_from_args(start, stop, step)
                    return self._emit_tuple_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported tuple input")
                if iterable.type_hint == "tuple":
                    return iterable
                if iterable.type_hint == "list":
                    res = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_FROM_LIST", args=[iterable], result=res)
                    )
                    return res
                return self._emit_tuple_from_iter(iterable)
            if func_id == "dict":
                has_starargs = len(node.args) > 1 or any(
                    isinstance(a, ast.Starred) for a in node.args
                )
                if has_starargs:
                    # dict(*args, ...) must unpack star-args into positional
                    # arguments at runtime, so route through CALL_BIND.
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                res = MoltValue(self.next_var(), type_hint="dict")
                if not node.args:
                    self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
                else:
                    iterable = self.visit(node.args[0])
                    if iterable is None:
                        raise NotImplementedError("Unsupported dict input")
                    self.emit(MoltOp(kind="DICT_FROM_OBJ", args=[iterable], result=res))
                for kw in node.keywords:
                    if kw.arg is None:
                        mapping = self.visit(kw.value)
                        if mapping is None:
                            raise NotImplementedError("Unsupported dict ** input")
                        self.emit(
                            MoltOp(
                                kind="DICT_UPDATE_KWSTAR",
                                args=[res, mapping],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        key = MoltValue(self.next_var(), type_hint="str")
                        self.emit(MoltOp(kind="CONST_STR", args=[kw.arg], result=key))
                        val = self.visit(kw.value)
                        if val is None:
                            raise NotImplementedError("Unsupported dict kw value")
                        self.emit(
                            MoltOp(
                                kind="STORE_INDEX",
                                args=[res, key, val],
                                result=MoltValue("none"),
                            )
                        )
                return res
            if func_id == "float":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="float")
                    self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=res))
                    return res
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported float input")
                res = MoltValue(self.next_var(), type_hint="float")
                self.emit(MoltOp(kind="FLOAT_FROM_OBJ", args=[value], result=res))
                return res
            if func_id == "complex":
                if any(kw.arg is None for kw in node.keywords) or len(node.args) > 2:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                kw_real = 0
                kw_imag = 0
                invalid_kw = False
                for kw in node.keywords:
                    if kw.arg == "real":
                        kw_real += 1
                    elif kw.arg == "imag":
                        kw_imag += 1
                    else:
                        invalid_kw = True
                        break
                if (
                    invalid_kw
                    or kw_real > 1
                    or kw_imag > 1
                    or (kw_real > 0 and len(node.args) >= 1)
                    or (kw_imag > 0 and len(node.args) >= 2)
                ):
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                real_val: MoltValue | None = None
                imag_val: MoltValue | None = None
                if node.args:
                    real_val = self.visit(node.args[0])
                    if real_val is None:
                        raise NotImplementedError("Unsupported complex real input")
                if len(node.args) == 2:
                    imag_val = self.visit(node.args[1])
                    if imag_val is None:
                        raise NotImplementedError("Unsupported complex imag input")
                for kw in node.keywords:
                    if kw.arg == "real":
                        if real_val is not None:
                            raise NotImplementedError("complex() real specified twice")
                        real_val = self.visit(kw.value)
                        if real_val is None:
                            raise NotImplementedError("Unsupported complex real input")
                    elif kw.arg == "imag":
                        if imag_val is not None:
                            raise NotImplementedError("complex() imag specified twice")
                        imag_val = self.visit(kw.value)
                        if imag_val is None:
                            raise NotImplementedError("Unsupported complex imag input")
                    else:
                        raise NotImplementedError(
                            "complex only supports real/imag keywords"
                        )
                if real_val is None:
                    real_val = MoltValue(self.next_var(), type_hint="float")
                    self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=real_val))
                if imag_val is None:
                    imag_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=imag_val))
                    has_imag = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=has_imag))
                else:
                    has_imag = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_imag))
                res = MoltValue(self.next_var(), type_hint="complex")
                self.emit(
                    MoltOp(
                        kind="COMPLEX_FROM_OBJ",
                        args=[real_val, imag_val, has_imag],
                        result=res,
                    )
                )
                return res
            if func_id == "int":
                if len(node.args) > 2:
                    raise NotImplementedError("int expects 0-2 arguments")
                value: MoltValue | None = None
                base_val: MoltValue | None = None
                has_base_flag = False
                from_str_source = False
                str_source_node = (
                    self._builtin_str_single_object_arg(node.args[0])
                    if node.args
                    else None
                )
                if node.args:
                    if str_source_node is not None:
                        value = self.visit(str_source_node)
                        from_str_source = True
                    else:
                        value = self.visit(node.args[0])
                    if value is None:
                        raise NotImplementedError("Unsupported int input")
                if len(node.args) == 2:
                    base_val = self.visit(node.args[1])
                    if base_val is None:
                        raise NotImplementedError("Unsupported int base")
                    has_base_flag = True
                for keyword in node.keywords:
                    if keyword.arg is None:
                        callee = self.visit(node.func)
                        if callee is None:
                            raise NotImplementedError("Unsupported call target")
                        return self._emit_dynamic_call(node, callee, True)
                    if keyword.arg == "base":
                        if has_base_flag:
                            return self._emit_type_error_value(
                                "int() got multiple values for argument 'base'",
                                "int",
                            )
                        base_val = self.visit(keyword.value)
                        if base_val is None:
                            raise NotImplementedError("Unsupported int base")
                        has_base_flag = True
                    else:
                        return self._emit_type_error_value(
                            f"int() got an unexpected keyword argument '{keyword.arg}'",
                            "int",
                        )
                if value is None:
                    if has_base_flag:
                        return self._emit_type_error_value(
                            "int() missing string argument", "int"
                        )
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=res))
                    return res
                if not has_base_flag:
                    base_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=base_val))
                has_base = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="CONST_BOOL", args=[has_base_flag], result=has_base)
                )
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind=(
                            "INT_FROM_STR_OF_OBJ" if from_str_source else "INT_FROM_OBJ"
                        ),
                        args=[value, base_val, has_base],
                        result=res,
                    )
                )
                return res
            if func_id == "pow":
                if node.keywords:
                    raise NotImplementedError("pow does not support keywords")
                if len(node.args) not in (2, 3):
                    raise NotImplementedError("pow expects 2 or 3 arguments")
                base = self.visit(node.args[0])
                exp = self.visit(node.args[1])
                if base is None or exp is None:
                    raise NotImplementedError("Unsupported pow inputs")
                if len(node.args) == 2:
                    if "complex" in {base.type_hint, exp.type_hint}:
                        res_type = "complex"
                    elif "float" in {base.type_hint, exp.type_hint}:
                        res_type = "float"
                    else:
                        res_type = "Unknown"
                    res = MoltValue(self.next_var(), type_hint=res_type)
                    self.emit(MoltOp(kind="POW", args=[base, exp], result=res))
                    return res
                mod = self.visit(node.args[2])
                if mod is None:
                    raise NotImplementedError("Unsupported pow mod input")
                int_like = {"int", "bool"}
                res_type = (
                    "int"
                    if {
                        base.type_hint,
                        exp.type_hint,
                        mod.type_hint,
                    }.issubset(int_like)
                    else "Unknown"
                )
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(MoltOp(kind="POW_MOD", args=[base, exp, mod], result=res))
                return res
            if func_id == "round":
                if node.keywords:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if len(node.args) not in (1, 2):
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported round input")
                if len(node.args) == 2:
                    ndigits = self.visit(node.args[1])
                    if ndigits is None:
                        ndigits = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=ndigits))
                    has_ndigits = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[True], result=has_ndigits)
                    )
                    if value.type_hint == "float":
                        res_type = "float"
                    elif value.type_hint in {"int", "bool"}:
                        res_type = "int"
                    else:
                        res_type = "Unknown"
                else:
                    ndigits = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=ndigits))
                    has_ndigits = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[False], result=has_ndigits)
                    )
                    res_type = (
                        "int"
                        if value.type_hint in {"int", "bool", "float"}
                        else "Unknown"
                    )
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(kind="ROUND", args=[value, ndigits, has_ndigits], result=res)
                )
                return res
            if func_id == "set":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="set")
                    self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
                    return res
                range_args = self._parse_range_call(node.args[0])
                if range_args is not None:
                    start, stop, step, _lowerable = range_args
                    range_obj = self._emit_range_obj_from_args(start, stop, step)
                    return self._emit_set_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported set input")
                return self._emit_set_from_iter(iterable)
            if func_id == "frozenset":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="frozenset")
                    self.emit(MoltOp(kind="FROZENSET_NEW", args=[], result=res))
                    return res
                range_args = self._parse_range_call(node.args[0])
                if range_args is not None:
                    start, stop, step, _lowerable = range_args
                    range_obj = self._emit_range_obj_from_args(start, stop, step)
                    return self._emit_frozenset_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported frozenset input")
                return self._emit_frozenset_from_iter(iterable)
                return self._emit_tuple_from_iter(iterable)
            if func_id == "bytes":
                if any(kw.arg is None for kw in node.keywords):
                    raise NotImplementedError("bytes does not support **kwargs")
                if len(node.args) > 3:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                source_expr = node.args[0] if node.args else None
                encoding_expr = node.args[1] if len(node.args) > 1 else None
                errors_expr = node.args[2] if len(node.args) > 2 else None
                has_encoding = encoding_expr is not None
                has_errors = errors_expr is not None
                for kw in node.keywords:
                    if kw.arg == "source":
                        if source_expr is not None:
                            return self._emit_type_error_value(
                                "bytes() got multiple values for argument 'source'",
                                "bytes",
                            )
                        source_expr = kw.value
                    elif kw.arg == "encoding":
                        if has_encoding:
                            return self._emit_type_error_value(
                                "bytes() got multiple values for argument 'encoding'",
                                "bytes",
                            )
                        encoding_expr = kw.value
                        has_encoding = True
                    elif kw.arg == "errors":
                        if has_errors:
                            return self._emit_type_error_value(
                                "bytes() got multiple values for argument 'errors'",
                                "bytes",
                            )
                        errors_expr = kw.value
                        has_errors = True
                    else:
                        msg = f"bytes() got an unexpected keyword argument '{kw.arg}'"
                        return self._emit_type_error_value(msg, "bytes")
                if source_expr is None and not has_encoding and not has_errors:
                    res = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=res))
                    return res
                if source_expr is None:
                    source_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=source_val))
                else:
                    source_val = self.visit(source_expr)
                    if source_val is None:
                        raise NotImplementedError("Unsupported bytes input")
                if has_encoding:
                    if encoding_expr is None:
                        raise NotImplementedError("Unsupported bytes encoding")
                    encoding_val = self.visit(encoding_expr)
                    if encoding_val is None:
                        raise NotImplementedError("Unsupported bytes encoding")
                else:
                    encoding_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=encoding_val))
                if has_errors:
                    if errors_expr is None:
                        raise NotImplementedError("Unsupported bytes errors")
                    errors_val = self.visit(errors_expr)
                    if errors_val is None:
                        raise NotImplementedError("Unsupported bytes errors")
                else:
                    errors_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=errors_val))
                res = MoltValue(self.next_var(), type_hint="bytes")
                if has_encoding or has_errors:
                    self.emit(
                        MoltOp(
                            kind="BYTES_FROM_STR",
                            args=[source_val, encoding_val, errors_val],
                            result=res,
                        )
                    )
                else:
                    self.emit(
                        MoltOp(kind="BYTES_FROM_OBJ", args=[source_val], result=res)
                    )
                return res
            if func_id == "bytearray":
                if any(kw.arg is None for kw in node.keywords):
                    raise NotImplementedError("bytearray does not support **kwargs")
                if len(node.args) > 3:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                source_expr = node.args[0] if node.args else None
                encoding_expr = node.args[1] if len(node.args) > 1 else None
                errors_expr = node.args[2] if len(node.args) > 2 else None
                has_encoding = encoding_expr is not None
                has_errors = errors_expr is not None
                for kw in node.keywords:
                    if kw.arg == "source":
                        if source_expr is not None:
                            return self._emit_type_error_value(
                                "bytearray() got multiple values for argument 'source'",
                                "bytearray",
                            )
                        source_expr = kw.value
                    elif kw.arg == "encoding":
                        if has_encoding:
                            return self._emit_type_error_value(
                                "bytearray() got multiple values for argument 'encoding'",
                                "bytearray",
                            )
                        encoding_expr = kw.value
                        has_encoding = True
                    elif kw.arg == "errors":
                        if has_errors:
                            return self._emit_type_error_value(
                                "bytearray() got multiple values for argument 'errors'",
                                "bytearray",
                            )
                        errors_expr = kw.value
                        has_errors = True
                    else:
                        msg = (
                            f"bytearray() got an unexpected keyword argument '{kw.arg}'"
                        )
                        return self._emit_type_error_value(msg, "bytearray")
                if source_expr is None and not has_encoding and not has_errors:
                    arg = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=arg))
                    res = MoltValue(self.next_var(), type_hint="bytearray")
                    self.emit(MoltOp(kind="BYTEARRAY_FROM_OBJ", args=[arg], result=res))
                    return res
                if source_expr is None:
                    source_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=source_val))
                    source_len_hint = None
                else:
                    source_len_hint = self._const_int_from_expr(source_expr)
                    source_val = self.visit(source_expr)
                    if source_val is None:
                        raise NotImplementedError("Unsupported bytearray input")
                if has_encoding:
                    if encoding_expr is None:
                        raise NotImplementedError("Unsupported bytearray encoding")
                    encoding_val = self.visit(encoding_expr)
                    if encoding_val is None:
                        raise NotImplementedError("Unsupported bytearray encoding")
                else:
                    encoding_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=encoding_val))
                if has_errors:
                    if errors_expr is None:
                        raise NotImplementedError("Unsupported bytearray errors")
                    errors_val = self.visit(errors_expr)
                    if errors_val is None:
                        raise NotImplementedError("Unsupported bytearray errors")
                else:
                    errors_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=errors_val))
                res = MoltValue(self.next_var(), type_hint="bytearray")
                if has_encoding or has_errors:
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FROM_STR",
                            args=[source_val, encoding_val, errors_val],
                            result=res,
                        )
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FROM_OBJ",
                            args=[source_val],
                            result=res,
                        )
                    )
                    self._remember_bytearray_len_hint(
                        res,
                        source_len_hint
                        if source_len_hint is not None
                        else self.const_ints.get(source_val.name),
                    )
                return res
            if func_id == "memoryview":
                if len(node.args) != 1:
                    raise NotImplementedError("memoryview expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="memoryview")
                self.emit(MoltOp(kind="MEMORYVIEW_NEW", args=[arg], result=res))
                return res
            if func_id in BUILTIN_FUNC_SPECS:
                if func_id == "open":
                    needs_bind = True
                spec = BUILTIN_FUNC_SPECS[func_id]
                # CALL_FUNC bypasses argument binding; vararg/kwonly builtins must
                # route through CALL_BIND to preserve Python call semantics.
                needs_bind = needs_bind or (
                    spec.vararg is not None or bool(spec.kwonly_params)
                )
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                else:
                    args = self._emit_call_args(node.args)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res

            if target_info is not None or imported_from is not None:
                target_module = None
                normalized = None
                if imported_from == "molt":
                    if func_id in MOLT_DIRECT_CALLS.get("molt", set()):
                        target_module = MOLT_REEXPORT_FUNCTIONS.get(func_id)
                elif imported_from:
                    normalized = self._normalize_allowlist_module(imported_from)
                    if (
                        normalized in MOLT_DIRECT_CALLS
                        and func_id in MOLT_DIRECT_CALLS[normalized]
                    ):
                        target_module = normalized
                    elif (
                        imported_from in MOLT_DIRECT_CALLS
                        and func_id in MOLT_DIRECT_CALLS[imported_from]
                    ):
                        target_module = imported_from
                original_attr = self._imported_attr_name(func_id)
                force_bind = original_attr[
                    :1
                ].isupper() or original_attr in MOLT_DIRECT_CALL_BIND_ALWAYS.get(
                    target_module or "", set()
                )
                lowered_imported_call = (
                    self._try_emit_imported_module_direct_or_task_call(
                        target_module,
                        original_attr,
                        node,
                        imported_from=imported_from,
                        normalized=normalized,
                        needs_bind=needs_bind,
                        force_bind=force_bind,
                        direct_registry_authorized=target_module in MOLT_DIRECT_CALLS,
                    )
                )
                if lowered_imported_call is not None:
                    return lowered_imported_call
            if imported_from is not None:
                normalized = self._normalize_allowlist_module(imported_from)
            else:
                normalized = None
            lowered_intrinsic = self._try_lower_intrinsic_lookup_call(
                func_id=func_id,
                imported_from=imported_from,
                node=node,
            )
            if lowered_intrinsic is not None:
                return lowered_intrinsic
            if self._is_intrinsics_module_name(imported_from) and func_id in {
                "require_intrinsic",
                "_require_intrinsic",
                "load_intrinsic",
                "_load_intrinsic",
            }:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                res = MoltValue(self.next_var(), type_hint="Any")
                callargs = self._emit_call_args_builder(node)
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[callee, callargs],
                        result=res,
                    )
                )
                return res
            if imported_from is not None and (
                imported_from in self.stdlib_allowlist
                or (normalized is not None and normalized in self.stdlib_allowlist)
                or self._is_internal_module(imported_from)
                or self._is_known_project_module(imported_from)
            ):
                target_module = normalized or imported_from
                # Resolve alias -> original attr name for cross-module calls
                original_attr = self._imported_attr_name(func_id)
                force_bind = original_attr[
                    :1
                ].isupper() or original_attr in MOLT_DIRECT_CALL_BIND_ALWAYS.get(
                    target_module, set()
                )
                lowered_imported_call = (
                    self._try_emit_imported_module_direct_or_task_call(
                        target_module,
                        original_attr,
                        node,
                        imported_from=imported_from,
                        normalized=normalized,
                        needs_bind=needs_bind,
                        force_bind=force_bind,
                        direct_registry_authorized=False,
                    )
                )
                if lowered_imported_call is not None:
                    return lowered_imported_call
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                res = MoltValue(self.next_var(), type_hint="Any")
                callargs = self._emit_call_args_builder(node)
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[callee, callargs],
                        result=res,
                    )
                )
                return res

            if imported_from is None:
                callee = self.visit(node.func)
                if callee is not None:
                    return self._emit_dynamic_call(node, callee, needs_bind)

            suggestion = self._call_allowlist_suggestion(func_id, imported_from)
            if suggestion:
                alternative = f"use {suggestion}"
            else:
                alternative = (
                    "import from an allowlisted module (see docs/spec/"
                    "areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)"
                )
            detail = (
                "Tier 0 only allows direct calls to allowlisted module-level"
                " functions; rebinding/monkey-patching is not observed"
            )
            if suggestion:
                detail = f"{detail}. warning: allowlisted path is {suggestion}"
            if self.fallback_policy == "bridge":
                self.compat.bridge_unavailable(
                    node,
                    f"call to non-allowlisted function '{func_id}'",
                    impact="high",
                    alternative=alternative,
                    detail=detail,
                )
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                res = MoltValue(self.next_var(), type_hint="Any")
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                else:
                    args = self._emit_call_args(node.args)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res

            raise self.compat.unsupported(
                node,
                f"call to non-allowlisted function '{func_id}'",
                impact="high",
                alternative=alternative,
                detail=detail,
            )

        callee = self.visit(node.func)
        if callee is None:
            raise NotImplementedError("Unsupported call target")
        return self._emit_dynamic_call(node, callee, needs_bind)
