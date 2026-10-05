"""ExpressionVisitorMixin: expression lowering visitor family.

Move-only extraction from frontend/__init__.py. Covers scalar names/constants,
string templates, collection literals, indexing/slicing, attributes, named
expressions, comparisons, unary/binary operations, conditional expressions, and
boolean short-circuit lowering. Shared helpers resolve through the assembled
SimpleTIRGenerator MRO via self.<method>.
"""

from __future__ import annotations

import ast
from molt.compiler_analysis.python_builtin_shapes import BUILTIN_SHAPE_NAMES
from molt.compiler_analysis.python_value_identity import UNBOUND_IDENTITY
from typing import (
    Any,
)

from molt.frontend._types import (
    _INLINE_INT_MAX,
    _INLINE_INT_MIN,
    BUILTIN_FUNC_SPECS,
    BUILTIN_TYPE_TAGS,
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.lowering.op_kinds_generated import BINOP_OP_KIND
from molt.frontend.sema import FunctionKind
from molt.frontend._mixin_base import GeneratorMixinBase


class ExpressionVisitorMixin(GeneratorMixinBase):
    def visit_Name(self, node: ast.Name) -> Any:
        value = self._load_name_in_scope(node)
        if isinstance(node.ctx, ast.Load) and isinstance(value, MoltValue):
            fact = (
                self.python_binding_index.expression_fact(node)
                if self.python_binding_index is not None
                else None
            )
            if fact is None or fact.binding_capture_required:
                value = self._capture_expression_reference(value)
        return value

    def _load_name_in_scope(self, node: ast.Name) -> Any:
        if (
            isinstance(node.ctx, ast.Load)
            and self._class_ns_stack
            and self.python_binding_index is not None
            and (fact := self.python_binding_index.expression_fact(node)) is not None
            and not fact.class_namespace_lookup
        ):
            # Inlined code retains its Python lexical scope. In particular a
            # comprehension's first iterable belongs to the class body, while
            # its payload, filters and later iterables skip the class mapping.
            scopes, locals_ = self._class_ns_stack, self.locals
            if fact.binding_global_lookup:
                return self._emit_global_get(node.id)
            self._class_ns_stack = []
            if node.id not in self.comp_shadow_locals:
                self.locals = scopes[-1].enclosing_locals
            try:
                return self._load_name_expression(node)
            finally:
                self._class_ns_stack, self.locals = scopes, locals_
        return self._load_name_expression(node)

    def _load_name_expression(self, node: ast.Name) -> Any:
        if isinstance(node.ctx, ast.Load):
            binding_invalidated = self._expression_has_invalidated_binding(node)
            if (
                self.in_annotation
                and self.python_binding_index is not None
                and (fact := self.python_binding_index.expression_fact(node))
                is not None
                and fact.binding_global_lookup
            ):
                # A deferred class-global annotation bypasses captured type
                # parameters even when no class mapping needs to be captured.
                return self._emit_global_get(node.id)
            if node.id == "__molt_missing__":
                res = MoltValue(self.next_var(), type_hint="missing")
                self.emit(MoltOp(kind="MISSING", args=[], result=res))
                return res
            class_scope = self._active_class_ns_scope(node.id)
            if class_scope is not None:
                local = self._class_ns_load(
                    class_scope, node.id, binding_invalidated=binding_invalidated
                )
                if local is not None:
                    # Class namespaces are live mappings and may be changed by
                    # metaclass callbacks; never reuse a producer-only fact here.
                    return MoltValue(local.name, type_hint=local.type_hint)
            if (
                binding_invalidated
                and self.python_binding_index is not None
                and (fact := self.python_binding_index.expression_fact(node))
                is not None
                and fact.binding_global_lookup
            ):
                # Invalidation removes value facts, not lexical storage. Only
                # namespace-owned names bypass cached values for a global read;
                # cells/locals retain their live loads and unbound guards below.
                return self._emit_global_get(node.id)
            if node.id in self.nonlocal_decls and node.id not in self.free_vars:
                raise FrontendRejection(
                    Diagnostic.SYNTAX_FORM, "nonlocal binding not found"
                )
            if node.id in self.free_vars:
                free_val = self._emit_free_var_load(
                    node.id, binding_invalidated=binding_invalidated
                )
                if free_val is not None:
                    return free_val
            if self.in_annotation and node.id in self.annotation_type_params:
                return self.annotation_type_params[node.id]
            # Check locals BEFORE module_global_mutations: inline
            # comprehensions bind their iterator variable in self.locals,
            # which must shadow the module-level name (CPython scoping).
            binding_fact = (
                self.python_binding_index.expression_fact(node)
                if self.python_binding_index is not None
                else None
            )
            binding_may_be_unbound = (
                bool(binding_fact.identities & UNBOUND_IDENTITY)
                if binding_fact is not None and binding_fact.name_lookup == "lexical"
                else None
            )
            local = self._load_local_value(
                node.id,
                binding_invalidated=binding_invalidated,
                binding_may_be_unbound=binding_may_be_unbound,
            )
            if local is not None:
                return local
            if node.id == "__name__":
                if self.entry_module and self.module_name == self.entry_module:
                    return self._emit_module_attr_get("__name__")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[self.module_name], result=res))
                return res
            if (
                self.current_func_name == "molt_main"
                and node.id in self.del_targets
                and not self._name_resolves_to_builtin(node.id)
            ):
                # The name is `del`'d or is an `except ... as` target somewhere
                # in this module scope, so on any reachable path it may be
                # unbound when read. A bare Name read at module scope has
                # LOAD_GLOBAL semantics: it must raise NameError (not the
                # AttributeError that MODULE_GET_ATTR yields) when the binding
                # is absent, while still returning the live value when present
                # (e.g. deleted only on a conditional branch, or re-bound after
                # the delete). MODULE_GET_GLOBAL reads the live module dict and
                # encodes exactly those semantics.  A name that shadows a builtin
                # is excluded: deleting it must fall back to the builtin (CPython
                # LOAD_GLOBAL semantics), which the regular resolution below
                # handles via the static builtin-type/func/exception lookup once
                # the module binding is gone.
                return self._emit_global_get(node.id)
            if (
                self.current_func_name == "molt_main"
                and node.id in self.module_global_mutations
            ):
                # The module dict is the mutable storage home for top-level
                # control-flow bindings, but a bare Name read still has
                # LOAD_GLOBAL semantics: dict lookup, builtins fallback, and
                # NameError on a miss.  MODULE_GET_ATTR is reserved for real
                # module.attribute reads; using it here leaks AttributeError
                # into package initializers such as ``try: __NUMPY_SETUP__``.
                return self._emit_global_get(node.id)
            global_val = self.globals.get(node.id)
            if global_val is None:
                if node.id in BUILTIN_SHAPE_NAMES:
                    # A Python reference must capture the live object, not
                    # manufacture a new builtin wrapper from its spelling.
                    return self._emit_global_get(node.id)
                if node.id == "NotImplemented":
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="CONST_NOT_IMPLEMENTED", args=[], result=res))
                    return res
                if node.id == "Ellipsis":
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="CONST_ELLIPSIS", args=[], result=res))
                    return res
                if node.id in self.module_chunk_globals:
                    return self._emit_global_get(node.id)
                if (
                    node.id in self.module_declared_funcs
                    or node.id in self.module_declared_classes
                ):
                    if (
                        self.current_func_name != "molt_main"
                        or self.module_name in self.stdlib_allowlist
                        or self._is_known_project_module(self.module_name)
                    ):
                        if node.id in self.stable_module_funcs:
                            return self._emit_stable_module_func_ref(node.id)
                        return self._emit_global_get(node.id)
                if node.id in {"locals", "__import__"}:
                    return self._emit_module_attr_get_on("builtins", node.id)
                if node.id in BUILTIN_TYPE_TAGS:
                    # Python names read the captured live builtins namespace.
                    # Type tags remain an internal runtime representation, not
                    # authority to replace a rebound Python callable.
                    return self._emit_global_get(node.id)
                if node.id in BUILTIN_FUNC_SPECS:
                    return self._emit_global_get(node.id)
                if self._builtin_exception_is_available(node.id):
                    return self._emit_exception_class(node.id)
                if node.id in self.stdlib_allowlist:
                    module_val = self._emit_module_load(node.id)
                    if self.current_func_name == "molt_main":
                        self.globals[node.id] = module_val
                        self._emit_module_attr_set(node.id, module_val)
                    return module_val
                return self._emit_global_get(node.id)
            if self.current_func_name == "molt_main":
                return global_val
            if node.id in self.stable_module_funcs:
                return self._emit_stable_module_func_ref(node.id)
            return self._emit_global_get(node.id)
        return node.id

    def _emit_stable_module_func_ref(self, name: str) -> "MoltValue":
        """Read a stable module-level sync function without erasing identity."""
        mod_attr = self._emit_module_attr_get(name)
        if self.module_declared_funcs.get(name) == FunctionKind.SYNC:
            symbol = self._function_symbol_for_reference(name)
            mod_attr.type_hint = f"Func:{symbol}"
        return mod_attr

    def visit_BinOp(self, node: ast.BinOp) -> Any:
        # Constant-fold string concatenation: "a" + "b" -> "ab"
        # Handles chained adds like "a" + "b" + "c" recursively.
        if isinstance(node.op, ast.Add):
            folded_str = self._try_extract_const_str(node)
            if folded_str is not None:
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[folded_str], result=res))
                return res
        # Keep both source multiplication orders in canonical operator dispatch.
        # Shared representation facts specialize only exact admitted operands.
        left = self.visit(node.left)
        if left is None:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported binary operator left operand",
            )
        left_slot: int | None = None
        if self.is_async() and self._expr_may_yield(node.right):
            left_slot = self._spill_async_value(left)
        right = self.visit(node.right)
        if right is None:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported binary operator right operand",
            )
        if left_slot is not None:
            left = self._reload_async_value(left_slot, left.type_hint)
        res_type = "Unknown"
        hint_src: MoltValue | None = None
        complex_in = "complex" in {left.type_hint, right.type_hint}
        # The op.kind is registry data (BINOP_OP_KIND, generated from op_kinds.toml's
        # [[binary_op]] table — EXHAUSTIVE over ast.operator). The isinstance chain
        # below now selects ONLY the static result-type hint; a node.op outside the
        # 13 ast.operator subclasses keeps the "UNKNOWN" kind / "Unknown" hint.
        op_kind = BINOP_OP_KIND.get(type(node.op).__name__, "UNKNOWN")
        if isinstance(node.op, ast.Add):
            if left.type_hint == right.type_hint and left.type_hint in {
                "int",
                "float",
                "str",
                "bytes",
                "bytearray",
                "list",
                "tuple",
                "complex",
            }:
                res_type = left.type_hint
            elif {left.type_hint, right.type_hint} == {"int", "float"}:
                res_type = "float"
            elif complex_in:
                res_type = "complex"
        elif isinstance(node.op, ast.Sub):
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
            elif complex_in:
                res_type = "complex"
            elif left.type_hint in {"set", "frozenset"} and right.type_hint in {
                "set",
                "frozenset",
            }:
                res_type = left.type_hint
        elif isinstance(node.op, ast.Mult):
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
            elif complex_in:
                res_type = "complex"
            elif left.type_hint in {"list", "tuple"}:
                res_type = left.type_hint
                hint_src = left
            elif right.type_hint in {"list", "tuple"}:
                res_type = right.type_hint
                hint_src = right
        elif isinstance(node.op, ast.Div):
            res_type = "complex" if complex_in else "float"
        elif isinstance(node.op, ast.FloorDiv):
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        elif isinstance(node.op, ast.Mod):
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        elif isinstance(node.op, ast.Pow):
            if complex_in:
                res_type = "complex"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        elif isinstance(node.op, ast.BitOr):
            if left.type_hint == right.type_hint == "bool":
                res_type = "bool"
            elif {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
            elif left.type_hint in {"set", "frozenset"} and right.type_hint in {
                "set",
                "frozenset",
            }:
                res_type = left.type_hint
        elif isinstance(node.op, ast.BitAnd):
            if left.type_hint == right.type_hint == "bool":
                res_type = "bool"
            elif {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
            elif left.type_hint in {"set", "frozenset"} and right.type_hint in {
                "set",
                "frozenset",
            }:
                res_type = left.type_hint
        elif isinstance(node.op, ast.BitXor):
            if left.type_hint == right.type_hint == "bool":
                res_type = "bool"
            elif {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
            elif left.type_hint in {"set", "frozenset"} and right.type_hint in {
                "set",
                "frozenset",
            }:
                res_type = left.type_hint
        elif isinstance(node.op, ast.LShift):
            if {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
        elif isinstance(node.op, ast.RShift):
            if {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
        elif isinstance(node.op, ast.MatMult):
            if left.type_hint == right.type_hint == "buffer2d":
                res_type = "buffer2d"
        res = MoltValue(self.next_var(), type_hint=res_type)
        self.emit(MoltOp(kind=op_kind, args=[left, right], result=res))
        if hint_src is not None:
            self._propagate_container_hints(res.name, hint_src)
        return res

    def visit_Constant(self, node: ast.Constant) -> Any:
        if node.value is None:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            return res
        if node.value is Ellipsis:
            res = MoltValue(self.next_var(), type_hint="ellipsis")
            self.emit(MoltOp(kind="CONST_ELLIPSIS", args=[], result=res))
            return res
        if isinstance(node.value, bool):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[node.value], result=res))
            return res
        if isinstance(node.value, int):
            if _INLINE_INT_MIN <= node.value <= _INLINE_INT_MAX:
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[node.value], result=res))
            else:
                # Use "bigint" hint so _should_fast_int returns False —
                # bigint values are heap-allocated pointers and must not
                # be unboxed as inline ints in the fast arithmetic path.
                res = MoltValue(self.next_var(), type_hint="bigint")
                self.emit(
                    MoltOp(kind="CONST_BIGINT", args=[str(node.value)], result=res)
                )
            return res
        if isinstance(node.value, bytes):
            res = MoltValue(self.next_var(), type_hint="bytes")
            self.emit(MoltOp(kind="CONST_BYTES", args=[node.value], result=res))
            return res
        if isinstance(node.value, float):
            res = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[node.value], result=res))
            return res
        if isinstance(node.value, complex):
            real = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[node.value.real], result=real))
            imag = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[node.value.imag], result=imag))
            has_imag = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_imag))
            res = MoltValue(self.next_var(), type_hint="complex")
            self.emit(
                MoltOp(kind="COMPLEX_FROM_OBJ", args=[real, imag, has_imag], result=res)
            )
            return res
        if isinstance(node.value, str):
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.value], result=res))
            return res
        res = MoltValue(self.next_var(), type_hint=type(node.value).__name__)
        self.emit(MoltOp(kind="CONST", args=[node.value], result=res))
        return res

    def visit_JoinedStr(self, node: ast.JoinedStr) -> Any:
        parts: list[MoltValue] = []
        for item in node.values:
            if isinstance(item, ast.Constant) and isinstance(item.value, str):
                lit = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[item.value], result=lit))
                parts.append(lit)
                continue
            if isinstance(item, ast.FormattedValue):
                value = self.visit(item.value)
                if item.conversion != -1:
                    if item.conversion == ord("r"):
                        value = self._emit_repr_from_obj(value)
                    elif item.conversion == ord("s"):
                        value = self._emit_str_from_obj(value)
                    elif item.conversion == ord("a"):
                        value = self._emit_ascii_from_obj(value)
                    else:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Formatted value conversion not supported",
                        )
                if item.format_spec is None:
                    if item.conversion != -1:
                        parts.append(value)
                    else:
                        parts.append(self._emit_string_format(value, ""))
                    continue
                spec_val = self._emit_format_spec_value(item.format_spec)
                parts.append(self._emit_string_format_value(value, spec_val))
                continue
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM, "Unsupported f-string segment"
            )
        return self._emit_string_join(parts)

    def visit_TemplateStr(self, node: Any) -> Any:
        """Lower a PEP 750 ``t"..."`` literal to a ``Template`` object.

        The AST shape (CPython 3.14) is::

            TemplateStr(values=[Constant | Interpolation, ...])

        We construct a flat positional argument list of ``str`` literals and
        ``Interpolation`` objects in source order, then call
        ``string.templatelib.Template(*args)``. The Template constructor
        normalizes the sequence so that ``len(strings) == len(interpolations) + 1``.
        """
        ast_interpolation_cls = getattr(ast, "Interpolation", None)
        ast_constant_cls = ast.Constant
        positional_vals: list[MoltValue] = []
        for item in node.values:
            if isinstance(item, ast_constant_cls) and isinstance(item.value, str):
                lit = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[item.value], result=lit))
                positional_vals.append(lit)
                continue
            if ast_interpolation_cls is not None and isinstance(
                item, ast_interpolation_cls
            ):
                positional_vals.append(self._emit_template_interpolation(item))
                continue
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM, "Unsupported t-string segment"
            )
        template_class = self._emit_module_attr_get_on("string.templatelib", "Template")
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        for val in positional_vals:
            push_res = MoltValue(self.next_var(), type_hint="None")
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[callargs, val],
                    result=push_res,
                )
            )
        result = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="CALL_BIND",
                args=[template_class, callargs],
                result=result,
            )
        )
        return result

    def visit_List(self, node: ast.List) -> Any:
        if any(isinstance(elt, ast.Starred) for elt in node.elts):
            res = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
            for elt in node.elts:
                if isinstance(elt, ast.Starred):
                    val = self.visit(elt.value)
                    if val is None:
                        raise FrontendRejection(
                            Diagnostic.SYNTAX_FORM,
                            "Unsupported list unpacking value",
                        )
                    self.emit(
                        MoltOp(
                            kind="LIST_EXTEND",
                            args=[res, val],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    val = self.visit(elt)
                    if val is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE, "Unsupported list element"
                        )
                    self.emit(
                        MoltOp(
                            kind="LIST_APPEND",
                            args=[res, val],
                            result=MoltValue("none"),
                        )
                    )
            return res
        elems = self._emit_expr_list(node.elts)
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=elems, result=res))
        if elems:
            first = elems[0].type_hint
            if first in {"int", "float", "str", "bytes", "bytearray", "bool"} and all(
                elem.type_hint == first for elem in elems
            ):
                if self.current_func_name == "molt_main":
                    self.global_elem_hints[res.name] = first
                else:
                    self.container_elem_hints[res.name] = first
        return res

    def visit_Tuple(self, node: ast.Tuple) -> Any:
        if any(isinstance(elt, ast.Starred) for elt in node.elts):
            items = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[], result=items))
            for elt in node.elts:
                if isinstance(elt, ast.Starred):
                    val = self.visit(elt.value)
                    if val is None:
                        raise FrontendRejection(
                            Diagnostic.SYNTAX_FORM,
                            "Unsupported tuple unpacking value",
                        )
                    self.emit(
                        MoltOp(
                            kind="LIST_EXTEND",
                            args=[items, val],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    val = self.visit(elt)
                    if val is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Unsupported tuple element",
                        )
                    self.emit(
                        MoltOp(
                            kind="LIST_APPEND",
                            args=[items, val],
                            result=MoltValue("none"),
                        )
                    )
            res = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_FROM_LIST", args=[items], result=res))
            return res
        elems = self._emit_expr_list(node.elts)
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=elems, result=res))
        if elems:
            first = elems[0].type_hint
            if first in {"int", "float", "str", "bytes", "bytearray", "bool"} and all(
                elem.type_hint == first for elem in elems
            ):
                if self.current_func_name == "molt_main":
                    self.global_elem_hints[res.name] = first
                else:
                    self.container_elem_hints[res.name] = first
        return res

    def visit_Set(self, node: ast.Set) -> Any:
        if any(isinstance(elt, ast.Starred) for elt in node.elts):
            res = MoltValue(self.next_var(), type_hint="set")
            self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
            for elt in node.elts:
                if isinstance(elt, ast.Starred):
                    val = self.visit(elt.value)
                    if val is None:
                        raise FrontendRejection(
                            Diagnostic.SYNTAX_FORM,
                            "Unsupported set unpacking value",
                        )
                    self.emit(
                        MoltOp(
                            kind="SET_UPDATE",
                            args=[res, val],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    val = self.visit(elt)
                    if val is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE, "Unsupported set element"
                        )
                    self.emit(
                        MoltOp(
                            kind="SET_ADD",
                            args=[res, val],
                            result=MoltValue("none"),
                        )
                    )
            return res
        elems = self._emit_expr_list(node.elts)
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=elems, result=res))
        if elems:
            first = elems[0].type_hint
            if first in {"int", "float", "str", "bytes", "bytearray", "bool"} and all(
                elem.type_hint == first for elem in elems
            ):
                if self.current_func_name == "molt_main":
                    self.global_elem_hints[res.name] = first
                else:
                    self.container_elem_hints[res.name] = first
        return res

    def visit_Dict(self, node: ast.Dict) -> Any:
        if any(key is None for key in node.keys):
            res = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
            for key, value in zip(node.keys, node.values):
                if key is None:
                    mapping = self.visit(value)
                    if mapping is None:
                        raise FrontendRejection(
                            Diagnostic.SYNTAX_FORM,
                            "Unsupported dict unpacking value",
                        )
                    self.emit(
                        MoltOp(
                            kind="DICT_UPDATE",
                            args=[res, mapping],
                            result=MoltValue("none"),
                        )
                    )
                    continue
                key_val = self.visit(key)
                val_val = self.visit(value)
                if key_val is None or val_val is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE, "Unsupported dict entry"
                    )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[res, key_val, val_val],
                        result=MoltValue("none"),
                    )
                )
            return res
        items: list[ast.expr] = []
        for key, value in zip(node.keys, node.values):
            if key is None:
                # Fallback: re-enter the unpacking path (should not normally
                # reach here because the ``any(key is None ...)`` guard above
                # catches it, but handle defensively).
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
                for k2, v2 in zip(node.keys, node.values):
                    if k2 is None:
                        mapping = self.visit(v2)
                        if mapping is None:
                            raise FrontendRejection(
                                Diagnostic.SYNTAX_FORM,
                                "Unsupported dict unpacking value",
                            )
                        self.emit(
                            MoltOp(
                                kind="DICT_UPDATE",
                                args=[res, mapping],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        key_val = self.visit(k2)
                        val_val = self.visit(v2)
                        if key_val is None or val_val is None:
                            raise FrontendRejection(
                                Diagnostic.OPERAND_VALUE,
                                "Unsupported dict entry",
                            )
                        self.emit(
                            MoltOp(
                                kind="STORE_INDEX",
                                args=[res, key_val, val_val],
                                result=MoltValue("none"),
                            )
                        )
                return res
            items.append(key)
            items.append(value)
        values = self._emit_expr_list(items)
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=values, result=res))
        if values:
            key_vals = values[::2]
            val_vals = values[1::2]
            if all(key.type_hint == "str" for key in key_vals):
                first_val = val_vals[0].type_hint
                if first_val in {
                    "int",
                    "float",
                    "str",
                    "bytes",
                    "bytearray",
                    "bool",
                } and all(val.type_hint == first_val for val in val_vals):
                    if self.current_func_name == "molt_main":
                        self.global_dict_key_hints[res.name] = "str"
                        self.global_dict_value_hints[res.name] = first_val
                    else:
                        self.dict_key_hints[res.name] = "str"
                        self.dict_value_hints[res.name] = first_val
        return res

    def visit_Subscript(self, node: ast.Subscript) -> Any:
        target = self.visit(node.value)
        if isinstance(node.slice, ast.Slice):
            lower = node.slice.lower
            upper = node.slice.upper
            step_val = node.slice.step
            if lower is None:
                start = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
            else:
                start = self.visit(lower)
            if upper is None:
                end = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
            else:
                end = self.visit(upper)
            res_type = "Any"
            if target is not None and target.type_hint in {
                "bytes",
                "bytearray",
                "list",
                "tuple",
                "str",
                "memoryview",
            }:
                res_type = target.type_hint
            if step_val is None:
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(MoltOp(kind="SLICE", args=[target, start, end], result=res))
                return res
            step = self.visit(step_val)
            slice_obj = MoltValue(self.next_var(), type_hint="slice")
            self.emit(
                MoltOp(kind="SLICE_NEW", args=[start, end, step], result=slice_obj)
            )
            res = MoltValue(self.next_var(), type_hint=res_type)
            self.emit(MoltOp(kind="INDEX", args=[target, slice_obj], result=res))
            return res
        index_val = self.visit(node.slice)
        res_type = "Any"
        if target is not None:
            if target.type_hint == "memoryview":
                res_type = "int"
            elif target.type_hint in {"list", "tuple"}:
                elem_hint = self._container_elem_hint(target)
                if elem_hint:
                    res_type = elem_hint
            elif target.type_hint == "dict" and self.type_hint_policy == "trust":
                val_hint = self._dict_value_hint(target)
                if val_hint:
                    res_type = val_hint
        res = MoltValue(self.next_var(), type_hint=res_type)
        self.emit(MoltOp(kind="INDEX", args=[target, index_val], result=res))
        return res
        return None

    def visit_Slice(self, node: ast.Slice) -> Any:
        if node.lower is None:
            start = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
        else:
            start = self.visit(node.lower)
        if node.upper is None:
            stop = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=stop))
        else:
            stop = self.visit(node.upper)
        if node.step is None:
            step = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
        else:
            step = self.visit(node.step)
        if start is None or stop is None or step is None:
            raise FrontendRejection(Diagnostic.SYNTAX_FORM, "Unsupported slice element")
        res = MoltValue(self.next_var(), type_hint="slice")
        self.emit(MoltOp(kind="SLICE_NEW", args=[start, stop, step], result=res))
        return res

    def visit_Attribute(self, node: ast.Attribute) -> Any:
        obj = self.visit(node.value)
        if obj is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "Unsupported attribute receiver"
            )
        obj_name = None
        if isinstance(
            node.value, ast.Name
        ) and not self._expression_has_invalidated_binding(node.value):
            obj_name = node.value.id
        elif isinstance(node.value, ast.NamedExpr) and isinstance(
            node.value.target, ast.Name
        ):
            obj_name = node.value.target.id
        exact_class = self._exact_class_for_value(obj, obj_name)
        return self._emit_attribute_load(node, obj, obj_name, exact_class)

    def visit_NamedExpr(self, node: ast.NamedExpr) -> Any:
        value_node = self.visit(node.value)
        if value_node is None:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported assignment expression value",
            )
        if not isinstance(node.target, ast.Name):
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported assignment expression target",
            )
        # Assignment expressions publish a binding and return an independent
        # expression reference (Python's COPY before STORE). Capture before
        # publication: releasing the displaced slot may reenter Python.
        result = self._emit_owned_value_alias(value_node)
        self._emit_assign_target(node.target, value_node, node.value)
        return result

    def visit_Compare(self, node: ast.Compare) -> Any:
        return self._emit_expression_flow(node, "value")[0]

    def visit_UnaryOp(self, node: ast.UnaryOp) -> Any:
        operand = self.visit(node.operand)
        if operand is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "Unsupported unary operand"
            )
        if isinstance(node.op, ast.UAdd):
            type_hint = (
                "int"
                if operand.type_hint in {"int", "bool"}
                else operand.type_hint
                if operand.type_hint in {"float", "complex"}
                else "Any"
            )
            res = MoltValue(self.next_var(), type_hint=type_hint)
            self.emit(MoltOp(kind="POS", args=[operand], result=res))
            return res
        if isinstance(node.op, ast.USub):
            type_hint = (
                "int"
                if operand.type_hint in {"int", "bool"}
                else operand.type_hint
                if operand.type_hint in {"float", "complex"}
                else "Any"
            )
            res = MoltValue(self.next_var(), type_hint=type_hint)
            self.emit(MoltOp(kind="NEG", args=[operand], result=res))
            return res
        if isinstance(node.op, ast.Not):
            return self._emit_not(operand)
        if isinstance(node.op, ast.Invert):
            # DeprecationWarning for ~bool is handled by _prescan_compile_warnings
            # at module startup. No inline emission needed here.
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="INVERT", args=[operand], result=res))
            return res
        raise FrontendRejection(Diagnostic.SYNTAX_FORM, "Unary operator not supported")

    def visit_IfExp(self, node: ast.IfExp) -> Any:
        return self._emit_expression_flow(node, "value")[0]

    def visit_BoolOp(self, node: ast.BoolOp) -> Any:
        return self._emit_expression_flow(node, "value")[0]
