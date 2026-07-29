"""ImportLoweringMixin: import resolution, module load, and transaction lowering.

Move-only extraction from frontend/__init__.py. This lowering authority owns
relative import resolution, module override tracking, source/importlib import
transactions, stub import policy, module-load fallback behavior, from-import
binding, and import guards shared by statement, expression, attribute, and call
visitors.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Sequence

from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
)
from molt.frontend._types import MoltOp, MoltValue
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION,
    SIMPLEIR_RUNTIME_QUALIFIED_CALLABLE_SYMBOL,
)

_NON_MODULE_PROVENANCE = "<non-module>"

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ImportLoweringMixin(_MixinBase):
    @staticmethod
    def _normalize_allowlist_module(module_name: str | None) -> str | None:
        if not module_name or module_name == "molt.stdlib":
            return None
        if module_name.startswith("molt.stdlib."):
            return module_name[len("molt.stdlib.") :]
        return module_name

    def _module_import_contexts(self, node: ast.AST) -> tuple[ModuleImportContext, ...]:
        base = ModuleImportContext(
            module_name=self.module_name,
            is_package=self.module_is_package,
            state=self.module_import_state,
            spec_name=self.module_spec_name,
            target_python=self.target_python,
            execution_kind=self.module_execution_kind,
        )
        if self.module_import_flow is None:
            return (base,)
        return tuple(
            base.with_state(state) for state in self.module_import_flow.states_for(node)
        )

    def _emit_relative_import_error(self, kind: str | None) -> None:
        if kind == "beyond_top":
            message = "attempted relative import beyond top-level package"
        else:
            message = "attempted relative import with no known parent package"
        exc_val = self._emit_exception_new("ImportError", message)
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))

    def _is_known_project_module(self, module_name: str | None) -> bool:
        """Return True only when *module_name* was discovered in the graph.

        Project/external module authority is exact: a discovered package does
        not authorize arbitrary children. Child modules must be present in the
        module graph with their own exact path/case proof.
        """
        if not module_name or not self.known_modules:
            return False
        return module_name in self.known_modules

    def _is_linkable_module_function_symbol(self, module_name: str | None) -> bool:
        """Return whether a direct ``module__function`` symbol can be emitted.

        ``known_modules`` is import visibility.  It is not link authority.
        Cross-module Python direct calls are legal only to modules in
        ``direct_call_modules``; native packages admitted as visible imports
        must route through explicit callable export ABI metadata or remain
        dynamic/bound calls.
        """
        if not module_name:
            return False
        normalized = self._normalize_allowlist_module(module_name) or module_name
        if normalized == self.module_name:
            return True
        if not self.known_modules and not self.direct_call_modules:
            return True
        return normalized in self.direct_call_modules

    def _imported_module_binding_target(self, binding_name: str) -> str | None:
        if self._local_name_shadows_import_binding(binding_name):
            return None
        module_name = self.imported_modules.get(binding_name)
        if (
            module_name is None
            and binding_name not in self.free_vars
            and binding_name not in self.nonlocal_decls
        ):
            module_name = self.global_imported_modules.get(binding_name)
        return module_name

    def _set_imported_module_binding(
        self,
        binding_name: str,
        module_name: str | None,
        provenance: frozenset[str] | None = None,
    ) -> None:
        """Set one lexical module binding through the shared provenance authority."""
        self.imported_modules.pop(binding_name, None)
        self.local_imported_modules.discard(binding_name)
        if module_name is not None:
            self.imported_modules[binding_name] = module_name
            if self.current_func_name != "molt_main":
                self.local_imported_modules.add(binding_name)
        self.imported_module_provenance[binding_name] = (
            provenance
            if provenance is not None
            else frozenset(
                (module_name,) if module_name is not None else (_NON_MODULE_PROVENANCE,)
            )
        )
        self._record_module_provenance_flow_state()

    def _clear_imported_module_binding(self, binding_name: str) -> None:
        self._set_imported_module_binding(binding_name, None)

    def _imported_module_alias_target(self, value: ast.AST | None) -> str | None:
        """Resolve exact module identity for an ordinary alias assignment.

        Import provenance is a binding fact, not syntax limited to ``import``.
        Chained aliases therefore enter the same lexical maps as direct import
        bindings; later rebinding removes them through the ordinary assignment
        authority.
        """
        provenance = self._imported_module_alias_provenance(value)
        modules = provenance - {_NON_MODULE_PROVENANCE}
        if len(modules) == 1 and _NON_MODULE_PROVENANCE not in provenance:
            return next(iter(modules))
        return None

    def _imported_module_alias_provenance(
        self, value: ast.AST | None
    ) -> frozenset[str]:
        """Return every module identity an expression may yield unchanged.

        Python conditionals, boolean expressions, and assignment expressions
        return one of their operand values. Treating every non-``Name`` syntax
        as non-module loses exactly the may-provenance needed for target
        admission (for example ``sys if flag else other``). This authority is
        deliberately about value-preserving expression families; arithmetic,
        calls, containers, and attribute access create a different value and
        therefore contribute only the non-module state.
        """

        if isinstance(value, ast.Name):
            provenance = self.imported_module_provenance.get(value.id)
            if provenance is not None:
                return provenance
            target = self._imported_module_binding_target(value.id)
            return (
                frozenset((target,))
                if target is not None
                else frozenset((_NON_MODULE_PROVENANCE,))
            )
        if isinstance(value, ast.IfExp):
            return frozenset().union(
                self._imported_module_alias_provenance(value.body),
                self._imported_module_alias_provenance(value.orelse),
            )
        if isinstance(value, ast.BoolOp):
            return frozenset().union(
                *(
                    self._imported_module_alias_provenance(operand)
                    for operand in value.values
                )
            )
        if isinstance(value, ast.NamedExpr):
            return self._imported_module_alias_provenance(value.value)
        return frozenset((_NON_MODULE_PROVENANCE,))

    @staticmethod
    def _join_imported_module_provenance(
        *states: dict[str, frozenset[str]],
    ) -> dict[str, frozenset[str]]:
        names = set().union(*(state.keys() for state in states))
        return {
            name: frozenset().union(
                *(
                    state.get(name, frozenset((_NON_MODULE_PROVENANCE,)))
                    for state in states
                )
            )
            for name in names
        }

    def _runtime_qualified_callable_provenance_for_binding(
        self, binding_name: str | None, attr_name: str
    ) -> tuple[str | None, int]:
        if binding_name is None:
            return None, 0
        modules = self.imported_module_provenance.get(binding_name)
        if modules is None:
            exact = self._imported_module_binding_target(binding_name)
            modules = frozenset((exact,)) if exact is not None else frozenset()
        modules_with_symbols = {
            module
            for module in modules
            if module != _NON_MODULE_PROVENANCE
            and self._runtime_qualified_callable_symbol(module, attr_name) is not None
        }
        symbols = {
            symbol
            for module in modules_with_symbols
            if (symbol := self._runtime_qualified_callable_symbol(module, attr_name))
            is not None
        }
        if not symbols:
            return None, 0
        if (
            len(symbols) == 1
            and len(modules_with_symbols) == len(modules)
            and _NON_MODULE_PROVENANCE not in modules
        ):
            return next(iter(symbols)), 0
        return None, SIMPLEIR_RUNTIME_REQUIREMENT_FRAME_INTROSPECTION

    def _begin_module_provenance_flow(
        self, *, record_exception_prefixes: bool
    ) -> list[dict[str, frozenset[str]]]:
        paths = [dict(self.imported_module_provenance)]
        self._module_provenance_flow_stack.append((paths, record_exception_prefixes))
        return paths

    def _record_module_provenance_flow_state(self) -> None:
        state = dict(self.imported_module_provenance)
        for paths, record_exception_prefixes in self._module_provenance_flow_stack:
            if record_exception_prefixes:
                paths.append(state)

    def _finish_module_provenance_flow(
        self,
        paths: list[dict[str, frozenset[str]]],
        *,
        normal_paths: Sequence[dict[str, frozenset[str]]] = (),
    ) -> None:
        active_paths, _ = self._module_provenance_flow_stack.pop()
        if active_paths is not paths:
            raise AssertionError("module provenance flow scopes must be LIFO")
        candidates = list(normal_paths)
        if not candidates:
            candidates.extend(paths)
            candidates.append(dict(self.imported_module_provenance))
        self.imported_module_provenance = self._join_imported_module_provenance(
            *candidates
        )

    def _runtime_qualified_callable_symbol(
        self, module_name: str | None, attr_name: str
    ) -> str | None:
        normalized = self._normalize_allowlist_module(module_name) or module_name
        if normalized is None:
            return None
        return SIMPLEIR_RUNTIME_QUALIFIED_CALLABLE_SYMBOL.get(
            f"{normalized}.{attr_name}"
        )

    def _should_attempt_runtime_module_import(self, module_name: str) -> bool:
        if module_name in self.known_modules:
            return True
        if module_name in self.stdlib_allowlist:
            return True
        normalized_name = self._normalize_allowlist_module(module_name)
        if normalized_name and (
            normalized_name in self.stdlib_allowlist
            or normalized_name in self.known_modules
        ):
            return True
        if "." not in module_name:
            return False
        top_level = module_name.split(".", 1)[0]
        if top_level in self.stdlib_allowlist:
            return True
        normalized_top = self._normalize_allowlist_module(top_level)
        return bool(normalized_top and normalized_top in self.stdlib_allowlist)

    def _emit_import_transaction(
        self,
        module_name: str,
        *,
        fromlist_names: Sequence[str],
        level: int = 0,
        globals_val: MoltValue | None = None,
    ) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=name_val))

        if globals_val is None:
            globals_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=globals_val))
        locals_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=locals_val))

        fromlist_items: list[MoltValue] = []
        for name in fromlist_names:
            item_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=item_val))
            fromlist_items.append(item_val)
        fromlist_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=fromlist_items, result=fromlist_val))

        level_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[level], result=level_val))

        transaction_func = self._emit_intrinsic_function(
            "molt_importlib_import_transaction"
        )
        imported_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[
                    transaction_func,
                    name_val,
                    globals_val,
                    locals_val,
                    fromlist_val,
                    level_val,
                ],
                result=imported_val,
            )
        )
        return imported_val

    def _emit_importlib_import_module_leaf(self, module_name: str) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=name_val))
        package_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=package_val))
        import_module_func = self._emit_intrinsic_function(
            "molt_importlib_import_module"
        )
        imported_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[import_module_func, name_val, package_val],
                result=imported_val,
            )
        )
        return imported_val

    def _emit_source_import_transaction(
        self,
        module_name: str,
        *,
        fromlist_names: Sequence[str],
        level: int = 0,
    ) -> MoltValue:
        return self._emit_import_transaction(
            module_name,
            fromlist_names=fromlist_names,
            level=level,
            globals_val=self._emit_globals_dict(),
        )

    def _emit_source_import_alias_binding(self, module_name: str) -> MoltValue:
        bound_val = self._emit_source_import_transaction(
            module_name,
            fromlist_names=(),
            level=0,
        )
        for attr_name in module_name.split(".")[1:]:
            bound_val = self._emit_module_import_from_value(bound_val, attr_name)
        return bound_val

    def _emit_module_load(self, module_name: str) -> MoltValue:
        # NOTE: Earlier versions cached loaded_val in _module_cache_values to
        # avoid redundant MODULE_CACHE_GET + conditional-init sequences.  However,
        # the WASM state-machine backend (used for module init functions with
        # jumps/labels) can split the code into states where the cached local's
        # assignment lives in a state that an exception-redirect path skips.
        # When the later state that uses the cached local runs, the local is
        # still 0 (its WASM default), causing "module attribute access expects
        # module" errors in linked WASM artifacts.  Re-emitting the full
        # load sequence each time ensures the local is populated in the state
        # that actually uses it.
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=name_val))
        uses_runtime_import = module_name in self.known_modules or (
            self._should_attempt_runtime_module_import(module_name)
        )
        if uses_runtime_import:
            imported_val = MoltValue(self.next_var(), type_hint="module")
            self.emit(
                MoltOp(kind="MODULE_IMPORT", args=[name_val], result=imported_val)
            )
            return imported_val
        module_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(MoltOp(kind="MODULE_CACHE_GET", args=[name_val], result=module_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[module_val, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        if self.known_modules:
            exc_val = self._emit_exception_new(
                "ModuleNotFoundError", f"No module named '{module_name}'"
            )
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        loaded_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(MoltOp(kind="MODULE_CACHE_GET", args=[name_val], result=loaded_val))
        self._emit_import_guard(loaded_val, module_name)
        return loaded_val

    def _emit_module_load_with_parents(self, module_name: str) -> MoltValue:
        parts = module_name.split(".")
        parent_val: MoltValue | None = None
        current_val: MoltValue | None = None
        for idx, part in enumerate(parts):
            name = ".".join(parts[: idx + 1])
            current_val = self._emit_module_load(name)
            if parent_val is not None:
                self._emit_module_attr_set_on(parent_val, part, current_val)
            parent_val = current_val
        if current_val is None:
            raise FrontendRejection(Diagnostic.IMPORT_RESOLUTION, "Invalid module name")
        return current_val

    def _emit_module_import_from_value(
        self,
        module_val: MoltValue,
        attr_name: str,
        *,
        module_name: str | None = None,
    ) -> MoltValue:
        attr_val = MoltValue(self.next_var(), type_hint="Any")
        attr_name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[attr_name], result=attr_name_val))
        # `from MODULE import name` has CPython IMPORT_FROM semantics: a
        # missing attribute raises ImportError ("cannot import name ...") after
        # a sys.modules submodule fallback, NOT the AttributeError that a plain
        # `MODULE.name` (MODULE_GET_ATTR) read raises.
        runtime_symbol = self._runtime_qualified_callable_symbol(module_name, attr_name)
        self.emit(
            MoltOp(
                kind="MODULE_IMPORT_FROM",
                args=[module_val, attr_name_val],
                result=attr_val,
                metadata=(
                    {"runtime_symbol": runtime_symbol} if runtime_symbol else None
                ),
            )
        )
        return attr_val

    def _emit_import_guard(self, module_val: MoltValue, module_name: str) -> None:
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[module_val, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        exc_val = self._emit_exception_new(
            "ImportError", f"No module named '{module_name}'"
        )
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        # On the native backend, RAISE sets a pending exception but does not
        # alter control flow — execution falls through to END_IF and continues.
        # Without an explicit exit here, the caller proceeds to use the None
        # module_val in MODULE_GET_ATTR / MODULE_SET_ATTR, triggering a
        # "module attribute access expects module" TypeError that masks the
        # real ImportError.  Emit _emit_raise_exit() to jump to the nearest
        # exception handler (or return) so the ImportError propagates cleanly.
        self._emit_raise_exit()
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    # Modules whose API calls are lowered directly to IR ops by the frontend.
    # ``import molt_buffer`` etc. are no-ops: the module object is never used
    # at runtime because every ``molt_buffer.new()`` / ``molt_msgpack.parse()``
    # call is already emitted as specialised IR (BUFFER2D_NEW, MSGPACK_PARSE, …).
    _STUB_IMPORT_MODULES: frozenset[str] = frozenset(
        {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}
    )
    _IMPORT_TRANSACTION_BOOTSTRAP_MODULES: frozenset[str] = frozenset(
        {"builtins", "_molt_importer"}
    )

    def _source_imports_use_transaction(self) -> bool:
        return not (
            self.module_name in self._IMPORT_TRANSACTION_BOOTSTRAP_MODULES
            or self.module_name == "importlib"
            or self.module_name.startswith("importlib.")
        )
