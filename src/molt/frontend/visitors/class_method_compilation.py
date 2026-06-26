"""ClassMethodCompilationMixin: class method/function lowering authority.

Owns descriptor classification, method default specs, sync/generator/async
method compilation, implicit ``__class__`` closure computation, and inline
method body proofs for class definitions. ``classes.py`` keeps class object,
layout, namespace, and dataclass construction authority.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Literal, cast

from molt.frontend._types import (
    GEN_CLOSED_OFFSET,
    GEN_CONTROL_SIZE,
    MethodInfo,
    MoltOp,
    MoltValue,
    _MOLT_CLOSURE_PARAM,
)
from molt.frontend.sema import (
    FunctionKind,
    async_generator_contains_return_value,
    async_generator_contains_yield_from,
    function_contains_yield,
    signature_contains_yield,
    stateful_function_frame_plan,
    stateful_function_result_type_hint,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ClassMethodCompilationMixin(_MixinBase):
    def _function_needs_classcell(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> bool:
        for child in ast.walk(node):
            if isinstance(child, ast.Name) and child.id == "__class__":
                return True
            if (
                isinstance(child, ast.Call)
                and isinstance(child.func, ast.Name)
                and child.func.id == "super"
                and not child.args
                and not child.keywords
            ):
                return True
        return False

    def _property_field_from_method(self, node: ast.FunctionDef) -> str | None:
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        if not isinstance(stmt, ast.Return):
            return None
        value = stmt.value
        if not isinstance(value, ast.Attribute):
            return None
        if not isinstance(value.value, ast.Name):
            return None
        if value.value.id != "self":
            return None
        return value.attr

    def _emit_function_defaults(
        self,
        func_val: MoltValue,
        default_exprs: list[ast.expr],
        kw_default_exprs: list[ast.expr | None],
        kwonly_params: list[str],
    ) -> MoltValue:
        func_val, defaults_val, kwdefaults_val = self._emit_function_default_values(
            func_val, default_exprs, kw_default_exprs, kwonly_params
        )
        setter = self._emit_runtime_function("molt_function_set_defaults", 3)
        res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[setter, func_val, defaults_val, kwdefaults_val],
                result=res,
            )
        )
        return func_val

    def _method_needs_classcell_closure(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> bool:
        """True when ``node`` (a class method body being compiled) must
        receive the enclosing class's ``__class__`` cell as a closure free
        variable.

        This is the per-method companion to the class-level ``needs_classcell``
        decision: a method participates in the ``__class__`` closure iff it
        references zero-arg ``super()`` or ``__class__`` (directly or through a
        nested function/comprehension/lambda) AND the enclosing class actually
        created a ``__class__`` cell (``self._active_classcell_cell``).  When
        true, the method is compiled as a closure even at module scope so the
        cell — filled with the finished class object after the metaclass call —
        is what ``super()``/``__class__`` reads, identical to CPython.
        """
        return (
            self._active_classcell_cell is not None
            and self._function_needs_classcell(node)
        )

    def _compute_method_closure(
        self, item: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> tuple[list[str], dict[str, str], MoltValue | None, bool]:
        """Compute the closure for a class method being compiled.

        Returns ``(free_vars, free_var_hints, closure_val, has_closure)``.

        Two cases produce a closure:

        * The class body is itself nested inside a function (``current_func_name
          != "molt_main"``): the method may close over enclosing-function locals
          and, if it uses ``super()``/``__class__``, the injected ``__class__``
          cell — both captured by ``_collect_free_vars``.
        * The class body is at module scope (``molt_main``) but the method uses
          ``super()``/``__class__``: module-level names resolve as globals (not
          free vars), so the *only* closure variable is the implicit
          ``__class__`` cell.  Capturing the general free-var set here would
          wrongly demote module globals to free vars, so this case threads
          exactly ``["__class__"]``.

        Centralizing this here keeps the regular / generator / async / decorated
        method-compilation paths byte-identical and avoids re-deriving the
        ``molt_main`` vs nested decision four times.
        """
        free_vars: list[str] = []
        if self.current_func_name != "molt_main":
            free_vars = self._collect_free_vars(item)
        elif self._method_needs_classcell_closure(item):
            free_vars = ["__class__"]

        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if free_vars:
            self.unbound_check_names.update(free_vars)
            for name in free_vars:
                self._box_local(name)
                self.closure_locals.add(name)
            for name in free_vars:
                hint = self.boxed_local_hints.get(name)
                if hint is None:
                    value = self.locals.get(name)
                    if value is not None and value.type_hint:
                        hint = value.type_hint
                free_var_hints[name] = hint or "Any"
            closure_items = self._closure_cells_for(free_vars)
            closure_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val))
            has_closure = True
        return free_vars, free_var_hints, closure_val, has_closure

    def _method_inline_closure_ok(
        self,
        free_vars: list[str],
        item: "ast.FunctionDef | ast.AsyncFunctionDef",
    ) -> bool:
        """True when a method that is technically a *closure* is still safe to
        record as inline-eligible.

        A method using zero-arg ``super()`` closes over the enclosing class's
        implicit ``__class__`` cell, so its ``free_vars`` contains ``"__class__"``
        even though it captures no *real* enclosing locals.  That cell exists
        only to let the runtime dispatch path read the finished class object; at
        an **inline** call site the recursive static ``super()`` fold (which sets
        ``current_class`` / ``current_method_first_param`` to the inline owner)
        resolves the super-chain at compile time and never reads the cell.  When
        a ``super()`` in the inlined body cannot fold statically, the inline is
        aborted via ``_InlineSuperFoldRequired`` and the call routes through the
        cell-threaded dispatch path — so the cell is never needed at the inline
        site.

        The exception is a **bare ``__class__`` value load** (e.g.
        ``__class__.__name__``): that reads the cell directly, and the inlined
        body — spliced into a scope with no ``__class__`` cell — cannot reproduce
        it, so such a method must NOT be inline-eligible.  ``super()`` calls
        never appear as an ``ast.Name("__class__")`` (the cell binding is
        implicit), so any ``ast.Name`` with id ``"__class__"`` in the body is a
        bare value load that disqualifies the method.

        So a closure method is inline-eligible iff (a) it captures nothing beyond
        the implicit ``__class__`` cell, and (b) it contains no bare
        ``__class__`` value load.  Any genuine enclosing-local capture forces the
        CALL path, where the real closure tuple is threaded.
        """
        if not free_vars:
            return True
        real = [name for name in free_vars if name != "__class__"]
        if real:
            return False
        # ``free_vars == ["__class__"]``: eligible only if the body uses the
        # cell exclusively through ``super()`` (no bare ``__class__`` value).
        for child in ast.walk(item):
            if isinstance(child, ast.Name) and child.id == "__class__":
                return False
        return True

    def _inline_body_external_names(
        self, expr: "ast.expr", params: list[str]
    ) -> "frozenset[str]":
        """Collect the bare ``Name`` *loads* in an inline-body expression that
        are neither substituted parameters nor builtins.

        At inline time the body is spliced into the caller's scope and the
        parameters are substituted (``self.locals = {param: arg_value}``).  A
        bare ``Name`` that is NOT a parameter falls through ``visit_Name`` to
        ``_emit_global_get`` and resolves against the **caller's** module
        globals — which is only correct when the call site is compiled in the
        method's *defining* module.  A reference to the defining module's own
        global (a module-level constant, a sibling function, an intrinsic
        binding such as ``_MOLT_ARRAY_TOLIST``) therefore silently mis-resolves
        across a module boundary, yielding a ``NameError`` at runtime.

        Builtins (``len``, ``super``, exception/type names, …) are excluded
        because ``visit_Name`` materialises them statically and identically in
        every scope, so they remain sound under cross-module splicing.

        The returned set drives ``_try_inline_method_call``'s cross-module
        refusal (the fail-closed soundness gate).
        """
        param_set = set(params)
        external: set[str] = set()
        for node in ast.walk(expr):
            if not isinstance(node, ast.Name):
                continue
            if not isinstance(node.ctx, ast.Load):
                continue
            name = node.id
            if name in param_set:
                continue
            if self._name_resolves_to_builtin(name):
                continue
            external.add(name)
        return frozenset(external)

    def _extract_inline_return(
        self, item: "ast.FunctionDef", params: list[str]
    ) -> "ast.expr | None":
        """Return the body's `return <expr>` AST iff the method is
        trivially inlinable: body is a single Return statement (an
        optional leading docstring is allowed), the returned expression
        references only parameters, attributes-on-parameters,
        constants, builtins, and pure operations (BinOp, UnaryOp,
        Compare, BoolOp, IfExp, Tuple/List of inlinable elements), plus
        nested Calls thereof.

        The returned expression MAY reference globals of the *defining*
        module (e.g. ``_MOLT_ARRAY_TOLIST(self._handle)``); such bodies
        are still inline-eligible but ``_try_inline_method_call`` refuses
        to splice them across a module boundary (where the global would
        mis-resolve).  See ``_inline_body_external_names``.
        """
        body = item.body
        # Skip docstring if present.
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
        ):
            body = body[1:]
        if (
            len(body) != 1
            or not isinstance(body[0], ast.Return)
            or body[0].value is None
        ):
            return None

        def _safe(node: "ast.AST") -> bool:
            if isinstance(node, ast.Constant):
                return True
            if isinstance(node, ast.Name):
                # Builtin names like `super` are allowed even though
                # they aren't params — visit_Call's Phase 4a / general
                # path handles them correctly with the inline scope's
                # current_class set.  Plain identifier refs that
                # AREN'T params would fail at visit time (KeyError on
                # the substituted self.locals), which the caller
                # catches and bails on.
                return True
            if isinstance(node, ast.Attribute):
                return _safe(node.value)
            if isinstance(node, ast.BinOp):
                return _safe(node.left) and _safe(node.right)
            if isinstance(node, ast.UnaryOp):
                return _safe(node.operand)
            if isinstance(node, ast.Compare):
                return _safe(node.left) and all(_safe(c) for c in node.comparators)
            if isinstance(node, ast.BoolOp):
                return all(_safe(v) for v in node.values)
            if isinstance(node, ast.IfExp):
                return _safe(node.test) and _safe(node.body) and _safe(node.orelse)
            if (
                isinstance(node, (ast.Tuple, ast.List))
                and not getattr(node, "ctx", None).__class__.__name__ == "Store"
            ):
                return all(_safe(e) for e in node.elts)
            # Calls are allowed: when visited at inline time, Phase 4a's
            # super() fold or Phase 1's user-method fold will recursively
            # try inlining or emit a direct CALL.  Either way the body
            # composes cleanly with the substitution model — the Names
            # in arg positions get resolved against the inline locals.
            if isinstance(node, ast.Call):
                # Reject calls with kwargs / starred args — defensive,
                # the substitution model handles only positional.
                if node.keywords:
                    return False
                if any(isinstance(a, ast.Starred) for a in node.args):
                    return False
                return _safe(node.func) and all(_safe(a) for a in node.args)
            return False

        return body[0].value if _safe(body[0].value) else None

    def _extract_inline_init_assigns(
        self, item: "ast.FunctionDef", params: list[str]
    ) -> "list[tuple[str, ast.expr]] | None":
        """Detect `__init__`-style trivially-inlinable bodies.

        Accepts a body that is a sequence of `self.attr = <pure expr>`
        assignments (an optional leading docstring is allowed), where:
          - the assignment target is `Attribute(Name(<first param>),
            <attr>)` — i.e. `self.attr` for whatever `self` is named.
          - the value expression is `_safe` per `_extract_inline_return`'s
            criteria (constants, params, attributes-on-params, pure
            BinOp/UnaryOp/Compare/etc., Calls).
          - no other statement kinds (no `if`, no `for`, no `try`, no
            extra `Assign` to non-self targets, no `Return` other than
            implicit None).

        Returns the list of `(attr_name, expr_AST)` pairs in order, or
        ``None`` if the body doesn't match the pattern.

        At inline time the caller substitutes params → call args and
        emits a STORE_ATTR per pair on the freshly-allocated instance,
        eliminating the __init__ CALL frame setup that dominates
        bench_struct's per-iter cost.
        """
        if not params:
            return None
        self_name = params[0]
        body = item.body
        # Skip docstring if present.
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
        ):
            body = body[1:]
        if not body:
            # Empty body (only docstring) — equivalent to a no-op
            # __init__.  Inline as zero stores; still a perf win since
            # we skip the CALL.
            return []

        def _safe(node: "ast.AST") -> bool:
            if isinstance(node, ast.Constant):
                return True
            if isinstance(node, ast.Name):
                return True
            if isinstance(node, ast.Attribute):
                return _safe(node.value)
            if isinstance(node, ast.BinOp):
                return _safe(node.left) and _safe(node.right)
            if isinstance(node, ast.UnaryOp):
                return _safe(node.operand)
            if isinstance(node, ast.Compare):
                return _safe(node.left) and all(_safe(c) for c in node.comparators)
            if isinstance(node, ast.BoolOp):
                return all(_safe(v) for v in node.values)
            if isinstance(node, ast.IfExp):
                return _safe(node.test) and _safe(node.body) and _safe(node.orelse)
            if (
                isinstance(node, (ast.Tuple, ast.List))
                and not getattr(node, "ctx", None).__class__.__name__ == "Store"
            ):
                return all(_safe(e) for e in node.elts)
            if isinstance(node, ast.Call):
                if node.keywords:
                    return False
                if any(isinstance(a, ast.Starred) for a in node.args):
                    return False
                return _safe(node.func) and all(_safe(a) for a in node.args)
            return False

        assigns: list[tuple[str, ast.expr]] = []
        for stmt in body:
            # Allow trailing `return` / `return None` — Python __init__
            # implicitly returns None, but explicit `return None` is
            # also legal.  Anything else fails the pattern.
            if isinstance(stmt, ast.Return):
                if stmt.value is None:
                    continue
                if isinstance(stmt.value, ast.Constant) and stmt.value.value is None:
                    continue
                return None
            if not isinstance(stmt, ast.Assign):
                return None
            if len(stmt.targets) != 1:
                return None
            target = stmt.targets[0]
            if not isinstance(target, ast.Attribute):
                return None
            if not isinstance(target.value, ast.Name):
                return None
            if target.value.id != self_name:
                return None
            if not _safe(stmt.value):
                return None
            assigns.append((target.attr, stmt.value))
        return assigns

    def _compile_class_generator_method(
        self, class_node: ast.ClassDef, item: ast.FunctionDef
    ) -> MethodInfo:
        descriptor: Literal[
            "function",
            "classmethod",
            "staticmethod",
            "property",
            "decorated",
            "property_update",
        ] = "function"
        method_name = item.name
        property_update: Literal["setter", "deleter"] | None = None
        if item.decorator_list:
            if len(item.decorator_list) == 1 and isinstance(
                item.decorator_list[0], ast.Name
            ):
                deco = item.decorator_list[0]
                if deco.id in {"classmethod", "staticmethod", "property"}:
                    descriptor = cast(
                        Literal[
                            "function",
                            "classmethod",
                            "staticmethod",
                            "property",
                            "decorated",
                        ],
                        deco.id,
                    )
                else:
                    descriptor = "decorated"
            elif len(item.decorator_list) == 1 and isinstance(
                item.decorator_list[0], ast.Attribute
            ):
                deco = item.decorator_list[0]
                if (
                    isinstance(deco.value, ast.Name)
                    and deco.value.id == method_name
                    and deco.attr in {"setter", "deleter"}
                ):
                    descriptor = "property_update"
                    property_update = cast(Literal["setter", "deleter"], deco.attr)
                else:
                    descriptor = "decorated"
            else:
                descriptor = "decorated"
        if descriptor == "function" and method_name == "__class_getitem__":
            descriptor = "classmethod"
        property_field = None
        if descriptor == "property":
            property_field = self._property_field_from_method(item)
        return_hint = self._annotation_to_hint(item.returns)
        if (
            return_hint
            and return_hint[:1] in {"'", '"'}
            and return_hint[-1:] == return_hint[:1]
        ):
            return_hint = return_hint[1:-1]
        if return_hint == "Self":
            return_hint = class_node.name
        method_symbol = self._function_symbol(f"{class_node.name}_{method_name}")
        self._record_func_default_specs(method_symbol, item.args)
        poll_symbol = f"{method_symbol}_poll"
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(item.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(item.args)
        default_specs = self._default_specs_from_args(item.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if item.args.vararg is not None:
            arg_nodes.append(item.args.vararg)
        arg_nodes.extend(kwonly)
        if item.args.kwarg is not None:
            arg_nodes.append(item.args.kwarg)
        free_vars, free_var_hints, closure_val, has_closure = (
            self._compute_method_closure(item)
        )
        has_return = self._function_contains_return(item)
        frame_plan = stateful_function_frame_plan(
            kind=FunctionKind.GENERATOR,
            poll_symbol=poll_symbol,
            param_count=len(params),
            has_closure=has_closure,
            gen_control_size=GEN_CONTROL_SIZE,
        )
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        func_val = MoltValue(
            self.next_var(),
            type_hint=frame_plan.function_type_hint(closure_size),
        )
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[poll_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW",
                    args=[poll_symbol, len(params)],
                    result=func_val,
                )
            )
        func_spill = None
        if self.in_generator and signature_contains_yield(
            decorators=item.decorator_list,
            args=item.args,
            returns=item.returns,
        ):
            func_spill = self._spill_async_value(
                func_val, f"__func_meta_{len(self.async_locals)}"
            )
        varnames = self._collect_varnames_for_body(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=item.body,
        )
        self._emit_function_metadata(
            func_val,
            name=method_name,
            qualname=self._qualname_for_def(method_name),
            trace_lineno=item.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=ast.get_docstring(item, clean=False),
            is_generator=True,
            varnames=varnames,
        )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        self._emit_function_annotate(func_val, item)

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        prev_class = self.current_class
        prev_first_param = self.current_method_first_param
        self.current_class = class_node.name
        self.current_method_first_param = params[0] if params else None
        self.start_function(
            poll_symbol,
            params=["self"],
            type_facts_name=f"{class_node.name}.{method_name}",
            needs_return_slot=has_return,
        )
        self.global_decls = self._collect_global_decls(item.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
        assigned = self._collect_assigned_names(item.body)
        self.del_targets = self._collect_deleted_names(item.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        self.async_public_locals = set(self.scope_assigned) | {
            arg.arg for arg in arg_nodes
        }
        self.async_internal_locals = set()
        self.in_generator = True
        self.async_locals_base = frame_plan.async_locals_base
        if has_closure:
            self.async_closure_offset = frame_plan.async_closure_offset
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
        for i, arg in enumerate(arg_nodes):
            self.async_locals[arg.arg] = self.async_locals_base + i * 8
            hint = None
            if i == 0 and descriptor == "classmethod":
                hint = class_node.name
            elif i == 0 and descriptor not in ("classmethod", "staticmethod"):
                hint = class_node.name
            if self._hints_enabled():
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
            if hint is not None:
                self.async_local_hints[arg.arg] = hint
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        self._init_scope_async_locals(arg_nodes)
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
        self._push_qualname(method_name, True)
        try:
            for stmt in item.body:
                self.visit(stmt)
                if isinstance(stmt, (ast.Return, ast.Raise)):
                    break
        finally:
            self._pop_qualname()
        if self.return_label is not None:
            if not self._ends_with_return_jump():
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                closed = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", GEN_CLOSED_OFFSET, closed],
                        result=MoltValue("none"),
                    )
                )
                done = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair))
                self._emit_return_value(pair)
            self._emit_return_label()
        elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            closed = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", GEN_CLOSED_OFFSET, closed],
                    result=MoltValue("none"),
                )
            )
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair))
            self.emit(MoltOp(kind="ret", args=[pair], result=MoltValue("none")))
        self._spill_async_temporaries()
        gen_public_locals = self._async_locals_public_entries()
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_class = prev_class
        self.current_method_first_param = prev_first_param
        func_val.type_hint = frame_plan.function_type_hint(closure_size)
        names_vals: list[MoltValue] = []
        offsets_vals: list[MoltValue] = []
        for local_name, offset in gen_public_locals:
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[local_name], result=name_val))
            offset_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
            names_vals.append(name_val)
            offsets_vals.append(offset_val)
        names_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=names_vals, result=names_tuple))
        offsets_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=offsets_vals, result=offsets_tuple))
        self.emit(
            MoltOp(
                kind="GEN_LOCALS_REGISTER",
                args=[poll_symbol, names_tuple, offsets_tuple],
                result=MoltValue("none"),
            )
        )
        closure_size_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[closure_size], result=closure_size_val))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[func_val, "__molt_closure_size__", closure_size_val],
                result=MoltValue("none"),
            )
        )
        method_attr = func_val
        return {
            "func": func_val,
            "attr": method_attr,
            "descriptor": descriptor,
            "return_hint": return_hint,
            "param_count": len(params),
            "defaults": default_specs,
            "posonly_count": len(posonly),
            "kwonly_count": len(kwonly),
            "has_vararg": vararg is not None,
            "has_varkw": varkw is not None,
            "has_closure": has_closure,
            "property_field": property_field,
            "property_update": property_update,
        }

    def _compile_class_method(
        self, class_node: ast.ClassDef, item: ast.FunctionDef
    ) -> MethodInfo:
        descriptor: Literal[
            "function",
            "classmethod",
            "staticmethod",
            "property",
            "decorated",
            "property_update",
        ] = "function"
        method_name = item.name
        property_update: Literal["setter", "deleter"] | None = None
        if item.decorator_list:
            if len(item.decorator_list) == 1 and isinstance(
                item.decorator_list[0], ast.Name
            ):
                deco = item.decorator_list[0]
                if deco.id in {"classmethod", "staticmethod", "property"}:
                    descriptor = cast(
                        Literal[
                            "function",
                            "classmethod",
                            "staticmethod",
                            "property",
                            "decorated",
                        ],
                        deco.id,
                    )
                else:
                    descriptor = "decorated"
            elif len(item.decorator_list) == 1 and isinstance(
                item.decorator_list[0], ast.Attribute
            ):
                deco = item.decorator_list[0]
                if (
                    isinstance(deco.value, ast.Name)
                    and deco.value.id == method_name
                    and deco.attr in {"setter", "deleter"}
                ):
                    descriptor = "property_update"
                    property_update = cast(Literal["setter", "deleter"], deco.attr)
                else:
                    descriptor = "decorated"
            else:
                descriptor = "decorated"
        if descriptor == "function" and method_name == "__class_getitem__":
            descriptor = "classmethod"
        property_field = None
        if descriptor == "property":
            property_field = self._property_field_from_method(item)
        return_hint = self._annotation_to_hint(item.returns)
        if (
            return_hint
            and return_hint[:1] in {"'", '"'}
            and return_hint[-1:] == return_hint[:1]
        ):
            return_hint = return_hint[1:-1]
        if return_hint == "Self":
            return_hint = class_node.name
        method_symbol = self._function_symbol(f"{class_node.name}_{method_name}")
        self._record_func_default_specs(method_symbol, item.args)
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(item.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(item.args)
        default_specs = self._default_specs_from_args(item.args)
        free_vars, free_var_hints, closure_val, has_closure = (
            self._compute_method_closure(item)
        )

        func_hint = f"Func:{method_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{method_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[method_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW",
                    args=[method_symbol, len(params)],
                    result=func_val,
                )
            )
        func_spill = None
        if self.in_generator and signature_contains_yield(
            decorators=item.decorator_list,
            args=item.args,
            returns=item.returns,
        ):
            func_spill = self._spill_async_value(
                func_val, f"__func_meta_{len(self.async_locals)}"
            )
        varnames = self._collect_varnames_for_body(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=item.body,
        )
        self._emit_function_metadata(
            func_val,
            name=method_name,
            qualname=self._qualname_for_def(method_name),
            trace_lineno=item.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=ast.get_docstring(item, clean=False),
            varnames=varnames,
        )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        self._emit_function_annotate(func_val, item)

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        prev_class = self.current_class
        prev_first_param = self.current_method_first_param
        self.current_class = class_node.name
        self.current_method_first_param = params[0] if params else None
        method_params = params
        if has_closure:
            method_params = [_MOLT_CLOSURE_PARAM] + params
        self.start_function(
            method_symbol,
            params=method_params,
            type_facts_name=f"{class_node.name}.{method_name}",
            needs_return_slot=False,
            has_exception_handlers=self._body_has_exception_handlers(item.body),
        )
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if item.args.vararg is not None:
            arg_nodes.append(item.args.vararg)
        arg_nodes.extend(kwonly)
        if item.args.kwarg is not None:
            arg_nodes.append(item.args.kwarg)
        self.global_decls = self._collect_global_decls(item.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
        assigned = self._collect_assigned_names(item.body)
        self.del_targets = self._collect_deleted_names(item.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        for idx, arg in enumerate(arg_nodes):
            hint = None
            if idx == 0 and descriptor == "classmethod":
                hint = class_node.name
            elif idx == 0 and descriptor not in ("classmethod", "staticmethod"):
                hint = class_node.name
            if self._hints_enabled():
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
                elif hint is None:
                    hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "Unknown")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in item.args.args:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        self._prebox_scope_cell_vars(body=item.body, arg_nodes=arg_nodes)
        # Box ALL scope-assigned variables into cell lists.
        # Cell lists provide correct refcount management (inc_ref/
        # dec_ref in molt_store_index). The TIR backend's Memory SSA
        # rewrite converts cell store_index/index to store_var/load_var
        # for SSA phi visibility when optimization is enabled.
        for name in sorted(self.scope_assigned):
            self._box_local(name)
        for arg in arg_nodes:
            pval = self.locals.get(arg.arg)
            if pval is not None and arg.arg not in self.boxed_locals:
                self.emit(
                    MoltOp(
                        kind="STORE_VAR",
                        args=[pval],
                        result=MoltValue("none"),
                        metadata={"var": arg.arg},
                    )
                )
        self._push_qualname(method_name, True)
        try:
            for stmt in item.body:
                self.visit(stmt)
        finally:
            self._pop_qualname()
        if self.return_label is not None:
            if not self._ends_with_return_jump():
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self._emit_return_value(res)
            self._emit_return_label()
        elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_class = prev_class
        self.current_method_first_param = prev_first_param
        method_attr = func_val
        # Phase 2 — detect trivially-inlinable methods.
        #
        # If the body is a single `return <expr>` where `<expr>`
        # references only parameters, attributes-on-parameters, and
        # constants (no Calls, no Subscripts, no closure / global
        # references), record the return-expression AST so the
        # call site can substitute params → args and emit the body
        # inline instead of a CALL.  Targets bench_class_hierarchy's
        # call chain (`Base.compute(self, x): return x`,
        # `Mid.compute(self, x): return ... + 1` — the latter is
        # filtered out because the AST still contains a Call to
        # super(); Phase 4a's fold runs at visit time, not at
        # compile_method-time AST inspection).
        inline_return = None
        inline_init_assigns: list[tuple[str, ast.expr]] | None = None
        inline_closure_ok = self._method_inline_closure_ok(free_vars, item)
        if (
            descriptor == "function"
            and inline_closure_ok
            and not (vararg is not None or varkw is not None)
            and not kwonly_names
        ):
            inline_return = self._extract_inline_return(item, params)
            # Detect __init__-style trivially-inlinable bodies — a
            # sequence of `self.attr = <pure expr>` assignments
            # where <pure expr> only references params/constants
            # and the targets are attributes of the first param.
            # The class-instantiation fold (visit_Call) inlines
            # these as a sequence of STORE_ATTR ops directly on
            # the freshly-allocated instance, eliminating the
            # __init__ CALL frame setup that dominates
            # bench_struct's per-iter cost.
            if method_name == "__init__":
                inline_init_assigns = self._extract_inline_init_assigns(item, params)
        return {
            "func": func_val,
            "attr": method_attr,
            "descriptor": descriptor,
            "return_hint": return_hint,
            "param_count": len(params),
            "defaults": default_specs,
            "posonly_count": len(posonly),
            "kwonly_count": len(kwonly),
            "has_vararg": vararg is not None,
            "has_varkw": varkw is not None,
            "has_closure": has_closure,
            # True when ``has_closure`` is purely the implicit ``__class__``
            # super cell (no real enclosing-local capture).  The static
            # devirt / super-fold sites may then take the *inline* path
            # (whose recursive super-fold resolves the chain at compile
            # time and never reads the cell), but must NOT emit a direct
            # CALL to the closure symbol on inline failure — they fall back
            # to the general dispatch which threads the real closure tuple.
            "inline_closure_ok": inline_closure_ok,
            "property_field": property_field,
            "property_update": property_update,
            "inline_return": inline_return,
            "inline_params": (
                params
                if (inline_return is not None or inline_init_assigns is not None)
                else None
            ),
            # Owner class — needed at inline time to set
            # `self.current_class` so that Phase 4a's `super()`
            # fold inside the inlined body resolves against the
            # callee's MRO position, not the caller's.
            "inline_owner_class": (
                class_node.name
                if (inline_return is not None or inline_init_assigns is not None)
                else None
            ),
            # Module that defines this method, captured now (while compiling
            # the owner class) so the inline site can compare it against the
            # caller's module.  `inline_free_names` records the body's bare
            # references to that module's globals; a non-empty set forbids a
            # cross-module inline (the global would mis-resolve in the
            # caller's scope).  __init__-style inline-assign bodies carry the
            # same gate via the union over every assigned value-expression.
            "inline_owner_module": (
                self.module_name
                if (inline_return is not None or inline_init_assigns is not None)
                else None
            ),
            "inline_free_names": (
                self._inline_body_external_names(inline_return, params)
                if inline_return is not None
                else (
                    frozenset().union(
                        *(
                            self._inline_body_external_names(value_expr, params)
                            for _attr, value_expr in inline_init_assigns
                        )
                    )
                    if inline_init_assigns
                    else frozenset()
                )
            ),
            "inline_init_assigns": inline_init_assigns,
        }

    def _compile_class_async_method(
        self, class_node: ast.ClassDef, item: ast.AsyncFunctionDef
    ) -> MethodInfo:
        descriptor: Literal[
            "function",
            "classmethod",
            "staticmethod",
            "property",
            "decorated",
            "property_update",
        ] = "function"
        method_name = item.name
        property_update: Literal["setter", "deleter"] | None = None
        if item.decorator_list:
            if len(item.decorator_list) == 1 and isinstance(
                item.decorator_list[0], ast.Name
            ):
                deco = item.decorator_list[0]
                if deco.id in {"classmethod", "staticmethod", "property"}:
                    descriptor = cast(
                        Literal[
                            "function",
                            "classmethod",
                            "staticmethod",
                            "property",
                            "decorated",
                        ],
                        deco.id,
                    )
                else:
                    descriptor = "decorated"
            else:
                descriptor = "decorated"
        if descriptor == "function" and method_name == "__class_getitem__":
            descriptor = "classmethod"
        is_async_gen = function_contains_yield(item)
        if is_async_gen:
            if async_generator_contains_yield_from(item):
                raise SyntaxError("'yield from' inside async function")
            if async_generator_contains_return_value(item):
                raise SyntaxError("'return' with value in async generator")
            method_name = item.name
            property_field = None
            return_hint = self._annotation_to_hint(item.returns)
            if (
                return_hint
                and return_hint[:1] in {"'", '"'}
                and return_hint[-1:] == return_hint[:1]
            ):
                return_hint = return_hint[1:-1]
            if return_hint == "Self":
                return_hint = class_node.name
            wrapper_symbol = self._function_symbol(f"{class_node.name}_{method_name}")
            self._record_func_default_specs(wrapper_symbol, item.args)
            poll_symbol = f"{wrapper_symbol}_poll"
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                item.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(item.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if item.args.vararg is not None:
                arg_nodes.append(item.args.vararg)
            arg_nodes.extend(kwonly)
            if item.args.kwarg is not None:
                arg_nodes.append(item.args.kwarg)
            default_specs = self._default_specs_from_args(item.args)
            free_vars, free_var_hints, closure_val, has_closure = (
                self._compute_method_closure(item)
            )
            has_return = self._function_contains_return(item)
            frame_plan = stateful_function_frame_plan(
                kind=FunctionKind.ASYNC_GENERATOR,
                poll_symbol=poll_symbol,
                param_count=len(params),
                has_closure=has_closure,
                gen_control_size=GEN_CONTROL_SIZE,
            )

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_class = self.current_class
            prev_first_param = self.current_method_first_param
            self.current_class = class_node.name
            self.current_method_first_param = params[0] if params else None
            self.start_function(
                poll_symbol,
                params=["self"],
                type_facts_name=f"{class_node.name}.{method_name}",
                needs_return_slot=has_return,
            )
            self.async_context = True
            self.global_decls = self._collect_global_decls(item.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
            assigned = self._collect_assigned_names(item.body)
            self.del_targets = self._collect_deleted_names(item.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            self.async_public_locals = set(self.scope_assigned) | {
                arg.arg for arg in arg_nodes
            }
            self.async_internal_locals = set()
            self.in_generator = True
            self.async_locals_base = frame_plan.async_locals_base
            if has_closure:
                self.async_closure_offset = frame_plan.async_closure_offset
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                hint = None
                if i == 0 and descriptor == "classmethod":
                    hint = class_node.name
                elif i == 0 and descriptor not in ("classmethod", "staticmethod"):
                    hint = class_node.name
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                if hint is not None:
                    self.async_local_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._push_qualname(method_name, True)
            try:
                for stmt in item.body:
                    self.visit(stmt)
                    if isinstance(stmt, (ast.Return, ast.Raise)):
                        break
            finally:
                self._pop_qualname()
            if self.return_label is not None:
                if not self._ends_with_return_jump():
                    none_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                    closed = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", GEN_CLOSED_OFFSET, closed],
                            result=MoltValue("none"),
                        )
                    )
                    done = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair)
                    )
                    self._emit_return_value(pair)
                self._emit_return_label()
            elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                closed = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", GEN_CLOSED_OFFSET, closed],
                        result=MoltValue("none"),
                    )
                )
                done = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair))
                self.emit(MoltOp(kind="ret", args=[pair], result=MoltValue("none")))
            self._spill_async_temporaries()
            asyncgen_public_locals = self._async_locals_public_entries()
            closure_size = self._task_closure_size(
                frame_plan.payload_slots,
                include_gen_control=frame_plan.include_gen_control,
            )
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param

            func_hint = f"Func:{wrapper_symbol}"
            if has_closure:
                func_hint = f"ClosureFunc:{wrapper_symbol}"
            func_val = MoltValue(self.next_var(), type_hint=func_hint)
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[wrapper_symbol, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[wrapper_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=item.decorator_list,
                args=item.args,
                returns=item.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            varnames = self._collect_varnames_for_body(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=item.body,
            )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=self._qualname_for_def(method_name),
                trace_lineno=item.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=[],
                kw_default_exprs=[],
                docstring=ast.get_docstring(item, clean=False),
                is_async_generator=True,
                poll_fn_symbol=poll_symbol,
                varnames=varnames,
            )
            names_vals: list[MoltValue] = []
            offsets_vals: list[MoltValue] = []
            for local_name, offset in asyncgen_public_locals:
                name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[local_name], result=name_val))
                offset_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                names_vals.append(name_val)
                offsets_vals.append(offset_val)
            names_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=names_vals, result=names_tuple))
            offsets_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=offsets_vals, result=offsets_tuple))
            self.emit(
                MoltOp(
                    kind="ASYNCGEN_LOCALS_REGISTER",
                    args=[poll_symbol, names_tuple, offsets_tuple],
                    result=MoltValue("none"),
                )
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, item)
            closure_size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_closure_size__", closure_size_val],
                    result=MoltValue("none"),
                )
            )

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            wrapper_params = params
            if has_closure:
                wrapper_params = [_MOLT_CLOSURE_PARAM] + params
            self.start_function(
                wrapper_symbol,
                params=wrapper_params,
                type_facts_name=f"{class_node.name}.{method_name}",
            )
            if has_closure:
                self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                    _MOLT_CLOSURE_PARAM, type_hint="tuple"
                )
            self.global_decls = set()
            self.nonlocal_decls = set()
            self.scope_assigned = set()
            self.del_targets = set()
            for idx, arg in enumerate(arg_nodes):
                hint = None
                if idx == 0 and descriptor == "classmethod":
                    hint = class_node.name
                elif idx == 0 and descriptor not in ("classmethod", "staticmethod"):
                    hint = class_node.name
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
                        hint = "Any"
                value = MoltValue(arg.arg, type_hint=hint or "Unknown")
                if hint is not None:
                    self._apply_hint_to_value(arg.arg, value, hint)
                self.locals[arg.arg] = value
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(self.locals[arg.arg], hint)
            args = [self.locals[arg.arg] for arg in arg_nodes]
            if has_closure:
                args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
            gen_val = MoltValue(
                self.next_var(),
                type_hint=stateful_function_result_type_hint(FunctionKind.GENERATOR),
            )
            self.emit(
                MoltOp(
                    kind="ALLOC_TASK",
                    args=[poll_symbol, closure_size] + args,
                    result=gen_val,
                    metadata={"task_kind": frame_plan.task_kind},
                )
            )
            res = MoltValue(self.next_var(), type_hint=frame_plan.result_type_hint)
            self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)

            method_attr = func_val
            return {
                "func": func_val,
                "attr": method_attr,
                "descriptor": descriptor,
                "return_hint": return_hint,
                "param_count": len(params),
                "defaults": default_specs,
                "posonly_count": len(posonly),
                "kwonly_count": len(kwonly),
                "has_vararg": vararg is not None,
                "has_varkw": varkw is not None,
                "has_closure": has_closure,
                "property_field": property_field,
                "property_update": property_update,
            }
        method_name = item.name
        property_field = None
        return_hint = self._annotation_to_hint(item.returns)
        if (
            return_hint
            and return_hint[:1] in {"'", '"'}
            and return_hint[-1:] == return_hint[:1]
        ):
            return_hint = return_hint[1:-1]
        if return_hint == "Self":
            return_hint = class_node.name
        wrapper_symbol = self._function_symbol(f"{class_node.name}_{method_name}")
        self._record_func_default_specs(wrapper_symbol, item.args)
        poll_symbol = f"{wrapper_symbol}_poll"
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(item.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(item.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if item.args.vararg is not None:
            arg_nodes.append(item.args.vararg)
        arg_nodes.extend(kwonly)
        if item.args.kwarg is not None:
            arg_nodes.append(item.args.kwarg)
        default_specs = self._default_specs_from_args(item.args)
        free_vars, free_var_hints, closure_val, has_closure = (
            self._compute_method_closure(item)
        )
        has_return = self._function_contains_return(item)
        frame_plan = stateful_function_frame_plan(
            kind=FunctionKind.ASYNC,
            poll_symbol=poll_symbol,
            param_count=len(params),
            has_closure=has_closure,
            gen_control_size=GEN_CONTROL_SIZE,
        )

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        prev_class = self.current_class
        prev_first_param = self.current_method_first_param
        self.current_class = class_node.name
        self.current_method_first_param = params[0] if params else None
        self.start_function(
            poll_symbol,
            params=["self"],
            type_facts_name=f"{class_node.name}.{method_name}",
            needs_return_slot=has_return,
        )
        self.async_context = True
        self.global_decls = self._collect_global_decls(item.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
        assigned = self._collect_assigned_names(item.body)
        self.del_targets = self._collect_deleted_names(item.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        self.async_locals_base = frame_plan.async_locals_base
        if has_closure:
            self.async_closure_offset = frame_plan.async_closure_offset
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
        for i, arg in enumerate(arg_nodes):
            self.async_locals[arg.arg] = self.async_locals_base + i * 8
            hint = None
            if i == 0 and descriptor == "classmethod":
                hint = class_node.name
            elif i == 0 and descriptor not in ("classmethod", "staticmethod"):
                hint = class_node.name
            if self._hints_enabled():
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
            if hint is not None:
                self.async_local_hints[arg.arg] = hint
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        self._init_scope_async_locals(arg_nodes)
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
        self._push_qualname(method_name, True)
        try:
            for stmt in item.body:
                self.visit(stmt)
        finally:
            self._pop_qualname()
        if self.return_label is not None:
            if not self._ends_with_return_jump():
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self._emit_return_value(res)
            self._emit_return_label()
        else:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self._spill_async_temporaries()
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_class = prev_class
        self.current_method_first_param = prev_first_param

        func_hint = f"Func:{wrapper_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{wrapper_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[wrapper_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW",
                    args=[wrapper_symbol, len(params)],
                    result=func_val,
                )
            )
        func_spill = None
        if self.in_generator and signature_contains_yield(
            decorators=item.decorator_list,
            args=item.args,
            returns=item.returns,
        ):
            func_spill = self._spill_async_value(
                func_val, f"__func_meta_{len(self.async_locals)}"
            )
        varnames = self._collect_varnames_for_body(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=item.body,
        )
        self._emit_function_metadata(
            func_val,
            name=method_name,
            qualname=self._qualname_for_def(method_name),
            trace_lineno=item.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=ast.get_docstring(item, clean=False),
            is_coroutine=True,
            varnames=varnames,
        )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        self._emit_function_annotate(func_val, item)
        closure_size_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[closure_size], result=closure_size_val))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[func_val, "__molt_closure_size__", closure_size_val],
                result=MoltValue("none"),
            )
        )

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        wrapper_params = params
        if has_closure:
            wrapper_params = [_MOLT_CLOSURE_PARAM] + params
        self.start_function(
            wrapper_symbol,
            params=wrapper_params,
            type_facts_name=f"{class_node.name}.{method_name}",
        )
        if has_closure:
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        self.del_targets = set()
        for idx, arg in enumerate(arg_nodes):
            hint = None
            if idx == 0 and descriptor == "classmethod":
                hint = class_node.name
            elif idx == 0 and descriptor not in ("classmethod", "staticmethod"):
                hint = class_node.name
            if self._hints_enabled():
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
                elif hint is None:
                    hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "Unknown")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        args = [self.locals[arg.arg] for arg in arg_nodes]
        if has_closure:
            args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
        res = MoltValue(self.next_var(), type_hint=frame_plan.result_type_hint)
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[poll_symbol, closure_size] + args,
                result=res,
                metadata={"task_kind": frame_plan.task_kind},
            )
        )
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)

        method_attr = func_val
        return {
            "func": func_val,
            "attr": method_attr,
            "descriptor": descriptor,
            "return_hint": return_hint,
            "param_count": len(params),
            "defaults": default_specs,
            "posonly_count": len(posonly),
            "kwonly_count": len(kwonly),
            "has_vararg": vararg is not None,
            "has_varkw": varkw is not None,
            "has_closure": has_closure,
            "property_field": property_field,
            "property_update": property_update,
        }
