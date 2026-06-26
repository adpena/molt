"""GeneratorStateMixin: generator construction and reset-state authority.

Owns the assembled generator's constructor plus shared reset primitives for
per-function and module-chunk transient state. The constructor body is a
move-only extraction from frontend/__init__.py; reset helpers centralize the
state lists that were previously duplicated across constructor, start_function,
and module chunk reset paths.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Any, Literal

from molt.frontend._types import (
    _ClassNsScope,
    ClassInfo,
    CompatibilityReporter,
    FallbackPolicy,
    FormatToken,
    FuncInfo,
    MidendProfile,
    MoltOp,
    MoltValue,
)
from molt.frontend.sema import FunctionKind, SemaResult

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol
    from molt.type_facts import TypeFacts

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class GeneratorStateMixin(_MixinBase):
    def _reset_local_binding_state(
        self,
        *,
        reset_locals_cache: bool,
        reset_del_targets: bool,
    ) -> None:
        self.locals = {}
        if reset_locals_cache:
            # Backing store for the current frame's `locals()` snapshot semantics.
            # Stored outside `self.locals` to avoid accidental shadowing/rewrites.
            self.locals_cache_val = None
        self.boxed_locals = {}
        self.closure_locals = set()
        self.comp_shadow_locals = set()
        self.boxed_local_hints = {}
        self.free_vars = {}
        self.free_var_hints = {}
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        if reset_del_targets:
            self.del_targets = set()
        self.unbound_check_names = set()
        self.exact_locals = {}
        self.exact_builtin_locals = {}

    def _reset_import_resolution_state(
        self,
        *,
        reset_module_attr_mutations: bool,
    ) -> None:
        self.imported_names = dict(self.global_imported_names)
        self.imported_attr_names = dict(self.global_imported_attr_names)
        self.imported_modules = dict(self.global_imported_modules)
        self.local_imported_names = set()
        self.local_imported_modules = set()
        if reset_module_attr_mutations:
            self.imported_module_attr_mutations = set(
                self.global_imported_module_attr_mutations
            )

    def _reset_async_scope_state(self) -> None:
        self.async_locals = {}
        self.async_internal_locals = set()
        self.async_public_locals = set()
        self.async_locals_base = 0
        self.async_closure_offset = None
        self.async_local_hints = {}

    def _reset_type_hint_scope_state(self, *, reset_bytearray_len: bool) -> None:
        self.explicit_type_hints = {}
        self.container_elem_hints = {}
        self.dict_key_hints = {}
        self.dict_value_hints = {}
        if reset_bytearray_len:
            self.bytearray_len_hints = {}

    def _reset_function_cache_state(self) -> None:
        self.const_ints = {}
        # Producing-op index (result SSA name -> MoltOp), maintained by emit().
        # Value names are globally unique (next_var), so no per-function reset is
        # needed beyond clearing the current function/chunk view.
        self._op_by_result = {}
        # Per-function cache: module name -> cached MoltValue from MODULE_CACHE_GET.
        self._module_cache_values = {}
        self.in_generator = False
        self.async_context = False
        self.current_line = None

    def _reset_control_flow_state(
        self,
        *,
        reset_function_exception_label: bool,
    ) -> None:
        self.context_depth = 0
        self.control_flow_depth = 0
        self.try_end_labels = []
        self.try_scopes = []
        self.try_suppress_depth = None
        self.try_handler_scopes = []
        if reset_function_exception_label:
            self.function_exception_label = self.next_label()
        self.exception_stack_depth_baseline = None
        self.exception_stack_prev_baseline = None
        self.return_unwind_depth = 0
        self.return_unwind_popped_scopes = []
        self.finally_depth = 0
        self.return_label = None
        self.return_slot = None
        self.return_slot_index = None
        self.return_slot_offset = None
        self.block_terminated = False
        self.range_loop_stack = []
        self.async_index_loop_stack = []
        self.loop_break_flags = []
        self.loop_try_depths = []
        self.loop_break_counter = 0
        self.loop_layout_guards = []
        self.loop_guard_assumptions = []
        self.loop_static_class_refs = []
        self.loop_static_class_eager_refs = []
        self.loop_static_class_counter = 0
        self.active_exceptions = []

    def __init__(
        self,
        parse_codec: Literal["msgpack", "cbor", "json"] = "msgpack",
        type_hint_policy: Literal["ignore", "trust", "check"] = "ignore",
        fallback_policy: FallbackPolicy = "error",
        source_path: str | None = None,
        type_facts: "TypeFacts | None" = None,
        type_facts_module: str | None = None,
        module_name: str | None = None,
        module_spec_name: str | None = None,
        module_is_namespace: bool = False,
        entry_module: str | None = None,
        enable_phi: bool = True,
        known_modules: set[str] | None = None,
        known_classes: dict[str, ClassInfo] | None = None,
        stdlib_allowlist: set[str] | None = None,
        known_func_defaults: dict[str, dict[str, dict[str, Any]]] | None = None,
        known_func_kinds: dict[str, dict[str, str]] | None = None,
        module_chunking: bool = False,
        module_chunk_max_ops: int = 0,
        optimization_profile: MidendProfile = "release",
        pgo_hot_functions: set[str] | None = None,
    ) -> None:
        self._module_pressure_function_count = 0
        self._module_pressure_total_ops = 0
        self.funcs_map: dict[str, FuncInfo] = {
            "molt_main": {
                "params": [],
                "param_types": [],
                "return_hint": None,
                "ops": self._new_tracked_ops(),
            }
        }
        self._module_pressure_funcs_map_ref = self.funcs_map
        self.current_func_name: str = "molt_main"
        self.current_ops: list[MoltOp] = self.funcs_map["molt_main"]["ops"]
        self.func_code_ids: dict[str, int] = {}
        self.code_id_counter = 0
        self.code_slots_emitted = False
        self.var_count: int = 0
        self.state_count: int = 0
        self.classes: dict[str, ClassInfo] = dict(known_classes or {})
        self.local_class_names: set[str] = set()
        # Depth of class-statement bodies currently being lowered.  A nested
        # ``class`` statement (a class defined inside another class body) must
        # bind into the *enclosing class namespace* — exactly like a method or
        # a class-attribute assignment — never into module globals, even when
        # the outermost enclosing class is at module scope (where
        # ``current_func_name == "molt_main"``).  ``visit_ClassDef`` increments
        # this around its body-statement loop and consults it when publishing
        # the finished class so the nested class is bound as a class-body local
        # that the enclosing loop harvests into ``class_attr_values``.
        self._class_body_depth: int = 0
        # Class-body block-execution scope stack (P0 #50).  When the body of a
        # ``class`` statement is lowered as a NORMAL block (so arbitrary control
        # flow / ``del`` "just work", exactly like CPython's class-body code
        # object), the innermost entry on this stack describes the active class
        # namespace.  ``_store_local_value`` / ``_load_local_value`` /
        # ``_emit_delete_name`` consult it so a class-body name binds into the
        # namespace mapping (STORE_INDEX) and reads back from it (INDEX) — the
        # heap-backed dict IS the mutable store, so loop-carried mutation is
        # correct without SSA phi participation (the same mechanism the module
        # dict provides at module scope).  ``ns`` is the namespace MoltValue when
        # the class is built dynamically (``dynamic_build``); ``attr_values`` is
        # the name→MoltValue snapshot the static ``CLASS_DEF`` path consumes.
        # ``names`` is the set of names that are class-body-scoped (a Name not in
        # this set falls through to enclosing/global/builtin resolution, matching
        # CPython LOAD_NAME).
        self._class_ns_stack: list[_ClassNsScope] = []
        self._reset_local_binding_state(
            reset_locals_cache=True,
            reset_del_targets=True,
        )
        # Cell list (1-element list MoltValue) backing the implicit
        # ``__class__`` closure variable of the class currently having its
        # methods compiled.  Set by visit_ClassDef before the method
        # compilation loop when any method references ``super()``/``__class__``
        # (see `_function_needs_classcell`), and cleared afterwards.  Zero-arg
        # ``super()`` and bare ``__class__`` loads read the class object from
        # this cell — exactly mirroring CPython's ``__class__`` closure cell —
        # rather than re-deriving the class by module-attribute name (which is
        # wrong for function-local / nested classes that are not module
        # globals).  The same cell is stored as ``__classcell__`` in the class
        # namespace and filled with the finished class object after the
        # metaclass call.
        self._active_classcell_cell: MoltValue | None = None
        self._expr_col: tuple[int, int] | None = (
            None  # expression-level col_offset for traceback carets
        )
        # Set while inlining a method that closes over the implicit ``__class__``
        # super cell: any ``super()`` in the inlined body that cannot fold
        # statically raises ``_InlineSuperFoldRequired`` to abort the inline,
        # because the caller's spliced scope has no ``__class__`` cell.
        self._inline_super_must_fold: bool = False
        self.globals: dict[str, MoltValue] = {}
        self.module_chunk_globals: set[str] = set()
        self.func_symbol_names: dict[str, str] = {}
        self.func_default_specs: dict[str, dict[str, Any]] = {}
        self.stable_module_funcs: set[str] = set()
        self.module_declared_funcs: dict[str, FunctionKind] = {}
        self.module_declared_classes: set[str] = set()
        # Static class graph for this module — the soundness substrate for the
        # zero-arg ``super()`` fold (see ``_collect_module_class_graph``).
        # ``module_subclassed_names``: names referenced as a base anywhere.
        # ``module_class_bases``: class name -> list of base-name lists across
        # all class statements defining that name (``["<opaque>"]`` for a class
        # whose bases are not all simple names).
        self.module_subclassed_names: set[str] = set()
        self.module_class_bases: dict[str, list[list[str]]] = {}
        self.stable_module_classes: set[str] = set()
        self.module_defined_funcs: set[str] = set()
        self.class_definition_pending: set[str] = set()
        self.module_global_mutations: set[str] = set()
        self.module_globals_dict_escaped = False
        self.module_intrinsic_globals: dict[str, str] = {}
        self.reserved_external_func_symbols: set[str] = set()
        # Track the last-known type hint for module-scope attributes.
        # Populated by _emit_module_attr_set_on and read by _emit_module_attr_get.
        # Enables fast_int/fast_float paths for module-scope loop variables.
        self._module_attr_type_hints: dict[str, str] = {}
        self.mutated_classes: set[str] = set()
        self.instance_attr_mutations: dict[str, set[str]] = {}
        self.global_imported_names: dict[str, str] = {}
        # Maps bind_name -> original attr_name for `from X import Y as Z`
        # (bind_name="Z", attr_name="Y").  Used to resolve cross-module call
        # targets to the original function name rather than the alias.
        self.global_imported_attr_names: dict[str, str] = {}
        self.global_imported_modules: dict[str, str] = {}
        self.global_imported_module_attr_mutations: set[tuple[str, str]] = set()
        self._reset_import_resolution_state(reset_module_attr_mutations=True)
        self.local_intrinsic_wrappers: set[str] = set()
        self.gpu_kernel_symbols_by_name: dict[str, str] = {}
        self.current_gpu_kernel_context: bool = False
        # Track aliases for ``import typing as <alias>`` so that
        # ``@<alias>.overload`` is recognised as a typing overload stub.
        self._typing_import_aliases: set[str] = set()
        self._reset_async_scope_state()
        # Always eagerly emit __annotations__ dicts.  Our runtime does not
        # implement the deferred __annotate__ protocol (PEP 749), so we must
        # materialise annotations at definition time regardless of the host
        # Python version.
        self.eager_annotations = True
        self.parse_codec = parse_codec
        self.type_hint_policy = type_hint_policy
        self.annotation_type_params: dict[str, MoltValue] = {}
        self.in_annotation = False
        self._reset_type_hint_scope_state(reset_bytearray_len=True)
        self.global_elem_hints: dict[str, str] = {}
        self.stdlib_hint_trust = False
        if source_path:
            normalized_path = source_path.replace("\\", "/")
            if "/src/molt/stdlib/" in normalized_path or normalized_path.startswith(
                "src/molt/stdlib/"
            ):
                self.stdlib_hint_trust = True
        self._source_is_stdlib_module = self.stdlib_hint_trust
        self.global_dict_key_hints: dict[str, str] = {}
        self.global_dict_value_hints: dict[str, str] = {}
        self.type_facts = type_facts
        self.module_name = module_name or "__main__"
        self.type_facts_module = type_facts_module or self.module_name
        self.module_spec_name = module_spec_name or self.module_name
        self.entry_module = entry_module
        self.enable_phi = enable_phi
        self.module_prefix = f"{self._sanitize_module_name(self.module_name)}__"
        self.known_modules = set(known_modules or [])
        self.stdlib_allowlist = set(stdlib_allowlist or [])
        self.known_func_defaults: dict[str, dict[str, dict[str, Any]]] = (
            known_func_defaults or {}
        )
        self.known_func_kinds: dict[str, dict[str, str]] = known_func_kinds or {}
        self.module_func_defaults: dict[str, dict[str, Any]] = {}
        self.module_annotations: MoltValue | None = None
        self.module_annotation_items: list[tuple[str, ast.expr, int]] = []
        self.module_annotation_ids: dict[int, int] = {}
        self.module_annotation_exec_map: MoltValue | None = None
        self.module_annotation_exec_name: str | None = None
        self.module_annotation_exec_counter = 0
        self.module_annotation_emitted = False
        self.globals_builtin_val: MoltValue | None = None
        self.globals_builtin_emitted = False
        self.module_annotations_conditional = False
        self.module_frame_emitted = False
        self.module_chunking = module_chunking
        self.module_chunk_max_ops = module_chunk_max_ops
        self.module_stmt_offsets: list[int] = []
        self.module_chunk_counter = 0
        self.module_chunk_symbols: list[str] = []
        self._init_midend_state(optimization_profile, pgo_hot_functions)
        self.module_frame_entered = False
        self.module_frame_exited = False
        self.module_frame_code_id: int | None = None
        self.class_annotation_items: list[tuple[str, ast.expr, int]] = []
        self.class_annotation_exec_map: MoltValue | None = None
        self.class_annotation_exec_name: str | None = None
        self.class_annotation_exec_counter = 0
        self.annotation_name_counter = 0
        self.module_obj: MoltValue | None = None
        self.future_annotations = False
        self.defer_module_attrs = False
        self.deferred_module_attrs: set[str] = set()
        self.fallback_policy = fallback_policy
        self.compat = CompatibilityReporter(fallback_policy, source_path)
        self.source_path = source_path
        self.module_is_package = False
        self.module_is_namespace = module_is_namespace
        self._emitted_syntax_warnings: set[tuple[str, int, str]] = set()
        self._deferred_runtime_warnings: list[str] = []
        if source_path:
            normalized_path = source_path.replace("\\", "/")
            if normalized_path.endswith("/__init__.py") or normalized_path.endswith(
                "/__init__.pyi"
            ):
                self.module_is_package = True
        if self.module_is_namespace:
            self.module_is_package = True
        self.module_package_override: str | None = None
        self.module_package_override_set = False
        self.module_spec_override: str | None = None
        self.module_spec_override_set = False
        self.module_spec_override_is_package: bool | None = None
        self._reset_control_flow_state(reset_function_exception_label=True)
        self.func_aliases: dict[str, str] = {}
        self.reserved_func_symbols: dict[str, str] = {}
        self._reset_function_cache_state()
        self.format_token_cache: dict[
            tuple[str, int, tuple[str, ...]], list[FormatToken]
        ] = {}
        self.lambda_counter = 0
        self.genexpr_counter = 0
        self.qualname_stack: list[tuple[str, bool]] = []
        self.current_class: str | None = None
        self.current_method_first_param: str | None = None
        # Module-level constant dicts: name → {str_key: constant_value}
        # Populated during visit_Module to support compile-time **kwargs resolution
        # (e.g. @dataclass(**SLOTS) where SLOTS = {"slots": True}).
        self.module_const_dicts: dict[str, dict[str, Any]] = {}
        # The immutable SemaResult for the module currently being lowered (doc 44
        # §F2b). Computed once, pre-walk, by _populate_sema_state in visit_Module;
        # None until then (and for direct non-module lowering entry points).
        self._sema: SemaResult | None = None
        self._register_code_symbol("molt_main")
        if self.module_name:
            is_real_entry_module = (
                self.entry_module
                and self.module_name == self.entry_module
                and self.module_name != "__main__"
            )
            entry_spawn_override_enabled = (
                is_real_entry_module and "multiprocessing.spawn" in self.known_modules
            )
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=[self.module_name], result=name_val)
            )
            module_val = MoltValue(self.next_var(), type_hint="module")
            self.emit(MoltOp(kind="MODULE_NEW", args=[name_val], result=module_val))
            self.emit(
                MoltOp(
                    kind="MODULE_CACHE_SET",
                    args=[name_val, module_val],
                    result=MoltValue("none"),
                )
            )
            if entry_spawn_override_enabled:
                spawn_key = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["MOLT_MP_SPAWN"], result=spawn_key)
                )
                spawn_default = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[""], result=spawn_default))
                spawn_value = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="ENV_GET",
                        args=[spawn_key, spawn_default],
                        result=spawn_value,
                    )
                )
                spawn_expected = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["1"], result=spawn_expected))
                spawn_eq = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(
                        kind="STRING_EQ",
                        args=[spawn_value, spawn_expected],
                        result=spawn_eq,
                    )
                )
                spawn_not = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[spawn_eq], result=spawn_not))
                self.emit(MoltOp(kind="IF", args=[spawn_not], result=MoltValue("none")))
            if is_real_entry_module:
                main_name = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__main__"], result=main_name))
                self.emit(
                    MoltOp(
                        kind="MODULE_CACHE_SET",
                        args=[main_name, module_val],
                        result=MoltValue("none"),
                    )
                )
                name_attr = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__name__"], result=name_attr))
                self.emit(
                    MoltOp(
                        kind="MODULE_SET_ATTR",
                        args=[module_val, name_attr, main_name],
                        result=MoltValue("none"),
                    )
                )
                if entry_spawn_override_enabled:
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.module_obj = module_val
        self._emit_module_metadata()
        self._apply_type_facts("molt_main")
