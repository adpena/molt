"""CallMethodDispatchMixin: extracted call-lowering authority."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
)

from molt.frontend._types import (
    BUILTIN_TYPE_TAGS,
    ClassInfo,
    MethodInfo,
    MoltOp,
    MoltValue,
    _InlineSuperFoldRequired,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallMethodDispatchMixin(_MixinBase):
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
        # ``Base``) does not — that is the parity bug.  Sema precomputes the
        # methods whose successor-owner is stable across the whole entry-module
        # subclass graph (and leaves non-entry modules empty, since downstream
        # subclasses may be invisible here). When the fact is absent, super()
        # lowers to the runtime path, which the backend fuses into the
        # allocation-free ``call_super_method_ic`` -- already the fast path.
        assert self._sema is not None, "module sema must be populated before lowering"
        sound_super_methods = (
            self._sema.class_facts.super_fold_sound_methods_by_class.get(
                current_class, frozenset()
            )
        )
        if method_name not in sound_super_methods:
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
