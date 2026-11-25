"""CallMethodDispatchMixin: extracted call-lowering authority."""

from __future__ import annotations

import ast

from molt.compiler_analysis.python_inlining import (
    inline_expression_is_frame_independent,
)
from molt.frontend.class_model import (
    ClassView,
    Closure,
    Member,
    MemberKind,
    defines,
    namespace_closed,
)
from molt.frontend._types import (
    BUILTIN_TYPE_TAGS,
    OBJECT_CLASS,
    BuiltinClass,
    ClassId,
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend._mixin_base import GeneratorMixinBase


class CallMethodDispatchMixin(GeneratorMixinBase):
    def _class_resolves_default_object_new(
        self, class_id: ClassId, view: ClassView
    ) -> bool:
        if not namespace_closed(view):
            return False
        for base in self._class_mro(class_id):
            if base == OBJECT_CLASS:
                return True
            if isinstance(base, BuiltinClass):
                return False
            base_view = self.classes.get(base)
            if base_view is None or not namespace_closed(base_view):
                return False
            if defines(base_view, "__new__"):
                return False
        return False

    def _class_new_policy(
        self, class_id: ClassId, view: ClassView
    ) -> tuple[bool, bool]:
        if self._class_resolves_default_object_new(class_id, view):
            return False, False
        if not namespace_closed(view):
            return True, True
        for base in self._class_mro(class_id):
            if base == OBJECT_CLASS:
                continue
            if isinstance(base, BuiltinClass):
                # A builtin type owns its ``__new__``; a builtin exception's
                # constructor result stays opaque.
                return True, base.name not in BUILTIN_TYPE_TAGS
            base_view = self.classes.get(base)
            if base_view is None:
                return True, True
            if defines(base_view, "__new__"):
                return True, True
        return False, False

    def _load_local_value_unchecked(self, name: str) -> MoltValue | None:
        """A binding's current value for ``locals()``, missing while unbound.

        A ``global`` declaration names a module binding rather than a local,
        unless a comprehension scope shadows it. Every other name is read by the
        canonical local reader, without an unbound guard and without a source
        fact: this read is no Python expression, so a binding a callback may
        have rebound comes from its storage.
        """
        if (
            name not in self.comp_shadow_locals
            and self.current_func_name != "molt_main"
            and name in self.global_decls
        ):
            return None
        return self._load_local_value(name, guard_unbound=False)

    def _emit_receiver_call_args(
        self, receiver: MoltValue, args: list[ast.expr]
    ) -> tuple[MoltValue, list[MoltValue]]:
        return receiver, self._emit_call_args(args)

    def _emit_call_args(self, args: list[ast.expr]) -> list[MoltValue]:
        if not args:
            return []
        values: list[MoltValue] = []
        for expr in args:
            arg = self.visit(expr)
            if arg is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported call argument"
                )
            values.append(arg)
        return values

    def _emit_inline_expression(
        self, expression: ast.expr, bindings: dict[str, MoltValue]
    ) -> MoltValue:
        """Lower a preflighted expression without borrowing the caller's scope."""
        if isinstance(expression, ast.Name):
            return bindings[expression.id]
        if isinstance(expression, ast.Constant):
            value = self.visit(expression)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.INTERNAL_INVARIANT, "Inline constant produced no value"
                )
            return value
        if isinstance(expression, (ast.Tuple, ast.List)):
            values = [
                self._emit_inline_expression(element, bindings)
                for element in expression.elts
            ]
            is_tuple = isinstance(expression, ast.Tuple)
            result = MoltValue(
                self.next_var(), type_hint="tuple" if is_tuple else "list"
            )
            self.emit(
                MoltOp(
                    kind="TUPLE_NEW" if is_tuple else "LIST_NEW",
                    args=values,
                    result=result,
                )
            )
            return result
        raise FrontendRejection(
            Diagnostic.INTERNAL_INVARIANT, "Unproved inline expression reached lowering"
        )

    def _try_inline_method_call(
        self,
        member: Member,
        receiver: MoltValue,
        call_args: list[MoltValue],
    ) -> MoltValue | None:
        """Validate the whole frame-elision proof before emitting any operation."""
        inline = member.inline
        expression = inline.returns if inline is not None else None
        parameters = inline.params if inline is not None else None
        if (
            expression is None
            or parameters is None
            or member.closure is not Closure.NONE
            or len(parameters) != 1 + len(call_args)
            or not inline_expression_is_frame_independent(expression, parameters)
        ):
            return None
        bindings = dict(zip(parameters, [receiver, *call_args], strict=True))
        return self._emit_inline_expression(expression, bindings)

    def _try_inline_init_assigns(
        self,
        class_id: ClassId,
        init_assigns: list[tuple[str, ast.expr]],
        inline_params: list[str],
        receiver: MoltValue,
        call_args: list[MoltValue],
    ) -> bool:
        """Preflight every value and store, then emit in Python source order.

        ``receiver`` is the fresh allocation of ``class_id``; the class comes
        from the allocating constructor, never from a value hint.
        """
        if len(inline_params) != 1 + len(call_args):
            return False
        view = self.classes.get(class_id)
        if view is None:
            return False
        if (
            not namespace_closed(view)
            or view.dataclass is not None
            or view.custom_metaclass
            or view.decorated
        ):
            return False
        for owner in self._class_mro(class_id):
            if owner == OBJECT_CLASS:
                continue
            owner_view = self.classes.get(owner) if isinstance(owner, ClassId) else None
            if owner_view is None or not namespace_closed(owner_view):
                return False
            if defines(owner_view, "__setattr__"):
                return False
        fields = {item.name: item.offset for item in view.layout.fields}
        seen: set[str] = set()
        for name, expression in init_assigns:
            if (
                name in seen
                or name not in fields
                or self._class_attr_may_be_data_descriptor(class_id, name)
                or not inline_expression_is_frame_independent(expression, inline_params)
            ):
                return False
            seen.add(name)
        bindings = dict(zip(inline_params, [receiver, *call_args], strict=True))
        for name, expression in init_assigns:
            value = self._emit_inline_expression(expression, bindings)
            self._emit_guarded_setattr(receiver, name, value, class_id)
        return True

    def _try_emit_user_method_static_call(
        self,
        node: ast.Call,
        attr_node: ast.Attribute,
        receiver: MoltValue,
        obj_name: str | None,
    ) -> MoltValue | None:
        """Select the ordinary method of an already evaluated exact instance.

        The allocation's value carries the same proof as a local holding it.
        A constructor through a replaceable binding, a custom __new__, or a
        return annotation alone supplies no such proof.
        """
        class_id = self._exact_class_for_value(receiver, obj_name, fields=True)
        if class_id is None:
            return None
        view = self.classes.get(class_id)
        if view is None:
            return None
        # Conservative bail-outs: anything that touches the descriptor /
        # attribute-resolution machinery other than a vanilla bound-method
        # binding.
        if not namespace_closed(view) or not view.binding_proven:
            return None
        if view.dataclass is not None:
            return None
        method_name = attr_node.attr
        member, owner_class = self._resolve_member(class_id, method_name)
        if member is None or owner_class is None:
            return None
        if member.kind is not MemberKind.FUNCTION:
            return None
        # A closure target (a method holding the `__class__` cell of a
        # zero-argument `super()`, or cells of an enclosing function) takes its
        # function object's live closure tuple as its environment; the late
        # stage reads it from the callable this call loads (design 78, 5.10).
        shape = member.shape
        if shape is None or shape.vararg or shape.varkw:
            return None
        # A lookup hook assigned as data can intercept the selection just
        # as a def can. Every class on the MRO must use ordinary lookup.
        for owner in self._class_mro(class_id):
            if owner == OBJECT_CLASS:
                continue
            owner_view = self.classes.get(owner) if isinstance(owner, ClassId) else None
            if (
                owner_view is None
                or not namespace_closed(owner_view)
                or defines(owner_view, "__getattribute__")
                or defines(owner_view, "__getattr__")
            ):
                return None
        if node.keywords:
            return None
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                return None
        if shape.defaults or shape.kwonly or len(node.args) != shape.params - 1:
            return None
        method_symbol = member.symbol
        if method_symbol is None or method_symbol not in self.func_symbol_names:
            return None
        # ``m.__code__ = other.__code__`` would replace the body this call
        # enters; with no code object reachable (``code_rebinding``), no code
        # can. An exact positional call binds no default.
        premise = self.namespace_premise
        if premise is None or premise.code_rebinding:
            return None
        # The source call adopts its operands (`_mark_source_call`); a proven
        # call enters the target's machine entry, which must adopt them too
        # (`static_module_cells._proven_call_reason` holds the same rule).
        if not self._entry_adopts_symbol(method_symbol):
            return None

        # Capture the method before an argument can change the receiver's
        # class. This selection itself runs no Python code and cannot fail.
        callable_value = self._emit_method_load(receiver, method_name)
        load_metadata = self._op_by_result[callable_value.name].metadata
        assert load_metadata is not None
        load_metadata["proven_target"] = True
        call_args = self._emit_call_args(node.args)
        inlined = self._try_inline_method_call(member, receiver, call_args)
        if inlined is not None:
            return inlined

        # A proven call of the resolved method (design 78, 3.8): the call graph,
        # the type flow and the inliner read it as a static edge, and the late
        # stage enters the target directly. A known callable body does not make
        # its return annotation a value proof; an uninlined Python call keeps
        # a generic representation without such evidence.
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="CALL_GUARDED",
                args=[callable_value, receiver, *call_args],
                result=res,
                metadata={"target": method_symbol, "proven_target": True},
            )
        )
        return res
