"""CallDefaultsMixin: extracted call-lowering authority."""

from __future__ import annotations

import ast

from collections.abc import Callable
from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    MoltOp,
    MoltValue,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallDefaultsMixin(_MixinBase):
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
