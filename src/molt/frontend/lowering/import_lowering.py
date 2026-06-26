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

from molt.frontend._types import MoltOp, MoltValue

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

    @staticmethod
    def _spec_parent(spec_name: str, is_package: bool) -> str:
        if is_package:
            return spec_name
        if "." in spec_name:
            return spec_name.rsplit(".", 1)[0]
        return ""

    def _relative_import_package(self) -> str:
        if self.module_package_override_set:
            return self.module_package_override or ""
        spec_is_package = self.module_is_package
        spec_name = None
        if self.module_spec_override_set and self.module_spec_override:
            spec_name = self.module_spec_override
            if self.module_spec_override_is_package is not None:
                spec_is_package = self.module_spec_override_is_package
        if spec_name is None:
            spec_name = self.module_spec_name or self.module_name or ""
        return self._spec_parent(spec_name, spec_is_package)

    def _resolve_relative_import(
        self, module: str | None, level: int
    ) -> tuple[str | None, str | None]:
        if level <= 0:
            return module, None
        package = self._relative_import_package()
        if not package:
            return None, "no_parent"
        parts = package.split(".")
        if level > len(parts):
            return None, "beyond_top"
        base_parts = parts[: len(parts) - (level - 1)]
        base_name = ".".join(base_parts)
        if module:
            if base_name:
                return f"{base_name}.{module}", None
            return module, None
        return base_name or None, None

    def _emit_relative_import_error(self, kind: str | None) -> None:
        if kind == "beyond_top":
            message = "attempted relative import beyond top-level package"
        else:
            message = "attempted relative import with no known parent package"
        exc_val = self._emit_exception_new("ImportError", message)
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))

    def _should_track_module_overrides(self) -> bool:
        # Names assigned inside a class body lowered as a block (P0 #50) are
        # class-namespace members, never module overrides — even though the
        # outermost class may live at module scope with ``control_flow_depth``
        # 0.  The class-ns stack being non-empty means we are emitting such a
        # body; suppress module-override tracking for it.
        if self._class_ns_stack:
            return False
        return self.current_func_name == "molt_main" and self.control_flow_depth == 0

    @staticmethod
    def _is_modulespec_ctor(node: ast.AST) -> bool:
        if isinstance(node, ast.Name):
            return node.id == "ModuleSpec"
        if isinstance(node, ast.Attribute):
            return node.attr == "ModuleSpec"
        return False

    def _parse_modulespec_override(
        self, value: ast.AST
    ) -> tuple[str, bool | None] | None:
        if not isinstance(value, ast.Call):
            return None
        if not self._is_modulespec_ctor(value.func):
            return None
        spec_name = None
        if value.args:
            first = value.args[0]
            if isinstance(first, ast.Constant) and isinstance(first.value, str):
                spec_name = first.value
        for kw in value.keywords:
            if (
                kw.arg == "name"
                and spec_name is None
                and isinstance(kw.value, ast.Constant)
                and isinstance(kw.value.value, str)
            ):
                spec_name = kw.value.value
        if spec_name is None:
            return None
        is_package = None
        if len(value.args) >= 4:
            arg = value.args[3]
            if isinstance(arg, ast.Constant) and isinstance(arg.value, bool):
                is_package = arg.value
        for kw in value.keywords:
            if (
                kw.arg == "is_package"
                and isinstance(kw.value, ast.Constant)
                and isinstance(kw.value.value, bool)
            ):
                is_package = kw.value.value
        return spec_name, is_package

    def _record_module_override(self, target: ast.AST, value: ast.AST) -> None:
        if not isinstance(target, ast.Name):
            return
        if target.id == "__package__":
            if isinstance(value, ast.Constant) and isinstance(value.value, str):
                self.module_package_override_set = True
                self.module_package_override = value.value
            elif isinstance(value, ast.Constant) and value.value is None:
                self.module_package_override_set = False
                self.module_package_override = None
            else:
                self.module_package_override_set = False
                self.module_package_override = None
            return
        if target.id == "__spec__":
            if isinstance(value, ast.Constant) and value.value is None:
                self.module_spec_override_set = False
                self.module_spec_override = None
                self.module_spec_override_is_package = None
                return
            parsed = self._parse_modulespec_override(value)
            if parsed is None:
                return
            spec_name, is_package = parsed
            self.module_spec_override_set = True
            self.module_spec_override = spec_name
            self.module_spec_override_is_package = None
            if is_package is not None:
                self.module_spec_override_is_package = is_package

    def _maybe_record_module_overrides(
        self, targets: Sequence[ast.AST], value: ast.AST
    ) -> None:
        if not self._should_track_module_overrides():
            return
        for target in targets:
            self._record_module_override(target, value)

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

        The frontend may know defaults/signatures for many stdlib functions from
        source indexes.  That metadata is not link authority.  Once the build
        provides a closed module graph, cross-module direct calls are legal only
        to modules in that graph; absent modules must go through import/bound
        call lowering so missing optional paths do not leak undefined symbols
        into shared stdlib partitions.
        """
        if not module_name:
            return False
        normalized = self._normalize_allowlist_module(module_name) or module_name
        if normalized == self.module_name:
            return True
        if not self.known_modules:
            return True
        return normalized in self.known_modules

    def _imported_module_binding_target(self, binding_name: str) -> str | None:
        if self._local_name_shadows_import_binding(binding_name):
            return None
        module_name = self.imported_modules.get(binding_name)
        if module_name is None:
            module_name = self.global_imported_modules.get(binding_name)
        return module_name

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
            raise NotImplementedError("Invalid module name")
        return current_val

    def _emit_module_import_from_value(
        self, module_val: MoltValue, attr_name: str
    ) -> MoltValue:
        attr_val = MoltValue(self.next_var(), type_hint="Any")
        attr_name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[attr_name], result=attr_name_val))
        # `from MODULE import name` has CPython IMPORT_FROM semantics: a
        # missing attribute raises ImportError ("cannot import name ...") after
        # a sys.modules submodule fallback, NOT the AttributeError that a plain
        # `MODULE.name` (MODULE_GET_ATTR) read raises.
        self.emit(
            MoltOp(
                kind="MODULE_IMPORT_FROM",
                args=[module_val, attr_name_val],
                result=attr_val,
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
