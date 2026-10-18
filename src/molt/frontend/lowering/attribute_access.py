"""AttributeAccessMixin: module, object, field, and property access lowering.

Move-only extraction from frontend/__init__.py. This lowering authority owns
module attribute get/set, imported-module attribute mutation tracking,
descriptor detection, guarded object field/property fast paths, and general
attribute load/store emission shared by expression, assignment, class, and call
visitors.
"""

from __future__ import annotations

import ast

from molt.compiler_analysis.python_imports import normalize_stdlib_module_name
from molt.compiler_analysis.python_lexical_scope import python_scope_declarations
from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import (
    _BUILTIN_FAST_METHODS,
    OBJECT_CLASS,
    BoundMethodHint,
    BuiltinClass,
    ClassId,
    MethodDescriptor,
    MoltOp,
    MoltValue,
    ReceiverKind,
    TypeHint,
)
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_RUNTIME_PROTECTED_ATTRIBUTE_REQUIREMENTS,
    SIMPLEIR_RUNTIME_PROTECTED_ACQUISITION_REQUIREMENTS,
)

# Data descriptors every user class reaches through ``object`` and ``type``:
# ``object.__class__`` and the per-class ``__dict__``/``__weakref__`` getsets.
_OBJECT_DATA_DESCRIPTOR_ATTRS = frozenset({"__class__", "__dict__", "__weakref__"})
# Functions, classmethods and staticmethods define no ``__set__``/``__delete__``.
_NON_DATA_METHOD_DESCRIPTORS: frozenset[MethodDescriptor] = frozenset(
    {"function", "classmethod", "staticmethod"}
)
# Builtin constructors whose results never define ``__set__``/``__delete__``.
_NON_DESCRIPTOR_BUILTIN_CONSTRUCTORS = frozenset(
    {
        "bool",
        "bytearray",
        "bytes",
        "classmethod",
        "complex",
        "dict",
        "float",
        "frozenset",
        "int",
        "list",
        "object",
        "set",
        "staticmethod",
        "str",
        "tuple",
    }
)
# Displays and literals evaluate to builtin values, never to descriptors.
_NON_DESCRIPTOR_DISPLAYS = (
    ast.Constant,
    ast.JoinedStr,
    ast.List,
    ast.Tuple,
    ast.Set,
    ast.Dict,
    ast.ListComp,
    ast.SetComp,
    ast.DictComp,
    ast.GeneratorExp,
    ast.Lambda,
)


def _class_body_unrecorded_bindings(body: list[ast.stmt]) -> frozenset[str]:
    """Class-namespace names bound by a form ``class_attrs`` and methods omit.

    ``class_attrs`` records ``name = value`` and ``name: T = value``; methods
    record ``def name``. Tuple targets, imports, nested classes and named
    expressions also bind class attributes, with values no proof has seen.
    """
    unrecorded: set[str] = set()
    for statement in body:
        bound = set(
            python_scope_declarations([statement], eager_annotations=True).bound
        )
        if isinstance(statement, ast.Assign):
            bound -= {
                target.id
                for target in statement.targets
                if isinstance(target, ast.Name)
            }
        elif isinstance(statement, ast.AnnAssign):
            # Without a value the annotation binds nothing.
            if isinstance(statement.target, ast.Name):
                bound.discard(statement.target.id)
        elif isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
            bound.discard(statement.name)
        unrecorded |= bound
    return frozenset(unrecorded)


def _is_non_descriptor_literal(expr: ast.expr) -> bool:
    """A display, or arithmetic over constants such as ``-1`` or ``2 ** 10``."""
    if isinstance(expr, _NON_DESCRIPTOR_DISPLAYS):
        return True
    if isinstance(expr, ast.UnaryOp):
        return isinstance(expr.operand, ast.Constant)
    if isinstance(expr, ast.BinOp):
        return isinstance(expr.left, ast.Constant) and isinstance(
            expr.right, ast.Constant
        )
    return False


class AttributeAccessMixin(GeneratorMixinBase):
    def _exact_dataclass_field(
        self,
        obj: MoltValue,
        obj_name: str | None,
        attr: str,
    ) -> tuple[ClassId, int] | None:
        class_id = self._exact_class_for_value(obj, obj_name)
        if class_id is None:
            return None
        class_info = self.classes.get(class_id)
        if class_info is None or not class_info.get("dataclass"):
            return None
        offset = class_info.get("fields", {}).get(attr)
        if not isinstance(offset, int) or self._class_attr_may_be_data_descriptor(
            class_id, attr
        ):
            return None
        return class_id, offset

    def _module_can_defer_attrs(self, node: ast.Module) -> bool:
        for current in ast.walk(node):
            if isinstance(
                current,
                (
                    ast.FunctionDef,
                    ast.AsyncFunctionDef,
                    ast.ClassDef,
                    ast.Lambda,
                    ast.ListComp,
                    ast.SetComp,
                    ast.DictComp,
                    ast.GeneratorExp,
                ),
            ):
                return False
            if isinstance(current, ast.Call) and isinstance(current.func, ast.Name):
                if current.func.id in {"globals", "locals", "vars"}:
                    return False
        return True

    def _record_instance_attr_mutation(self, class_id: ClassId, attr: str) -> None:
        if class_id not in self.classes:
            return
        self.instance_attr_mutations.setdefault(class_id, set()).add(attr)

    def _instance_attr_mutated(self, class_id: ClassId, attr: str) -> bool:
        return attr in self.instance_attr_mutations.get(class_id, set())

    def _flush_deferred_module_attrs(self, names: set[str] | None = None) -> None:
        if not self.deferred_module_attrs or self.module_obj is None:
            return
        pending = self.deferred_module_attrs
        if names is not None:
            pending = pending & names
        for name in sorted(pending):
            # Skip variables that are live in the module dict via
            # module_global_mutations (loop-carried variables).
            # Their current value is in the module dict, not in a
            # local SSA variable.  Writing back the stale SSA value
            # would overwrite the accumulated loop result.
            if name in self.module_global_mutations:
                self.deferred_module_attrs.discard(name)
                continue
            val = self._load_local_value(name)
            if val is None:
                val = self.globals.get(name)
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            self._emit_module_attr_set_on(self.module_obj, name, val)
            self.deferred_module_attrs.discard(name)

    def _expr_may_be_data_descriptor(self, expr: ast.expr) -> bool:
        """False only for a class-body value proved not to be a data descriptor.

        CPython stores ``obj.attr = value`` through ``type(obj).attr.__set__``
        whenever the class attribute is a data descriptor, so a fixed instance
        field is sound only after that proof. An unresolved name, a factory
        call or an alias may produce one; each keeps the generic store.
        """
        if _is_non_descriptor_literal(expr):
            return False
        if not isinstance(expr, ast.Call) or not isinstance(expr.func, ast.Name):
            return True
        descriptor_class = self._class_reference(expr.func)
        if descriptor_class is not None:
            return self._class_instances_may_be_data_descriptors(descriptor_class)
        if self._call_is_dataclasses_field(expr.func):
            # ``@dataclass`` replaces the Field by its ``default=`` value or
            # deletes it; an undecorated class keeps the Field, which defines
            # no ``__set__``/``__delete__``.
            return any(
                keyword.arg == "default"
                and self._expr_may_be_data_descriptor(keyword.value)
                for keyword in expr.keywords
            ) or any(keyword.arg is None for keyword in expr.keywords)
        return expr.func.id not in _NON_DESCRIPTOR_BUILTIN_CONSTRUCTORS or (
            self._name_has_source_binding(expr.func)
        )

    def _call_is_dataclasses_field(self, func: ast.Name) -> bool:
        """The read is the ``dataclasses.field`` an import binding supplied."""
        index = self.python_binding_index
        fact = index.expression_fact(func) if index is not None else None
        if fact is None or len(fact.result.source_origins) != 1:
            return False
        (origin,) = fact.result.source_origins
        if origin[4] != "alias":
            return False
        module_name = self.imported_names.get(
            func.id
        ) or self.global_imported_names.get(func.id)
        return (
            module_name is not None
            and (normalize_stdlib_module_name(module_name) or module_name)
            == "dataclasses"
            and self._imported_attr_name(func.id) == "field"
        )

    def _class_instances_may_be_data_descriptors(self, class_id: ClassId) -> bool:
        """False only when calling the class makes plain instances of it."""
        info = self.classes.get(class_id)
        if (
            info is None
            or info.get("dynamic")
            or info.get("custom_metaclass")
            or class_id in self.class_definition_pending
        ):
            return True
        for owner in self._class_mro(class_id):
            if owner == OBJECT_CLASS:
                continue
            owner_info = self.classes.get(owner) if isinstance(owner, ClassId) else None
            if owner_info is None or owner_info.get("dynamic"):
                return True
            # ``__new__`` may return an object of another type.
            for name in ("__set__", "__delete__", "__new__"):
                if (
                    name in owner_info.get("methods", {})
                    or name in (owner_info.get("pending_methods") or ())
                    or name in owner_info.get("class_attrs", {})
                    or name in owner_info["possible_data_descriptor_attrs"]
                ):
                    return True
        return False

    def _possible_data_descriptor_attrs(
        self, node: ast.ClassDef, class_attrs: dict[str, ast.expr]
    ) -> frozenset[str]:
        return _class_body_unrecorded_bindings(node.body) | frozenset(
            attr
            for attr, expr in class_attrs.items()
            if self._expr_may_be_data_descriptor(expr)
        )

    def _class_attr_may_be_data_descriptor(self, class_id: ClassId, attr: str) -> bool:
        """False only when no class on the MRO can bind ``attr`` to a data
        descriptor, so an instance field may serve ``obj.attr``."""
        if attr in _OBJECT_DATA_DESCRIPTOR_ATTRS or class_id not in self.classes:
            return True
        for owner in self._class_mro(class_id):
            if owner == OBJECT_CLASS:
                continue
            mro_info = self.classes.get(owner) if isinstance(owner, ClassId) else None
            if not mro_info:
                return True
            if attr in mro_info["possible_data_descriptor_attrs"]:
                return True
            method_info = mro_info.get("methods", {}).get(attr)
            if method_info is not None:
                if method_info["descriptor"] not in _NON_DATA_METHOD_DESCRIPTORS:
                    return True
            elif attr in (mro_info.get("pending_methods") or ()):
                # Not compiled yet: its decorators are still unclassified.
                return True
        return False

    def _emit_module_attr_set(
        self, name: str, value: MoltValue, *, defer: bool = True
    ) -> None:
        if self.current_func_name != "molt_main" or self.module_obj is None:
            return
        self._record_app_module_store(name, value)
        if (
            defer
            and self.defer_module_attrs
            and name not in self.module_global_mutations
        ):
            self.deferred_module_attrs.add(name)
            return
        if self.defer_module_attrs:
            # Mutation-tracked bindings already belong to the live module
            # dictionary. The deferred flush deliberately skips them, so their
            # actual store must happen here and retire any older queued value.
            self.deferred_module_attrs.discard(name)
        self._emit_module_attr_set_on(self.module_obj, name, value)

    def _emit_module_attr_set_on(
        self, module_val: MoltValue, name: str, value: MoltValue
    ) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[module_val, name_val, value],
                result=MoltValue("none"),
            )
        )
        # Track the value's type hint so _emit_module_attr_get can propagate
        # it to downstream consumers.
        if (
            isinstance(value, MoltValue)
            and value.type_hint
            and value.type_hint != "Any"
        ):
            self._module_attr_type_hints[name] = value.type_hint

    def _emit_module_attr_get(self, name: str) -> MoltValue:
        # A bare module binding referenced from a Python function belongs to
        # that function object's active globals mapping. MODULE_GET_GLOBAL
        # supplies its builtins fallback and NameError behavior; explicit
        # ``module.attribute`` reads continue through _emit_module_attr_get_on.
        if self._function_needs_frame_trace():
            # A rebound function can supply a different value and type. Lexical
            # module facts do not establish the active namespace's value type.
            return self._emit_global_get(name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self._lexical_module_owner()
        # Propagate the last-known type hint for this module attribute.
        # When a module-scope variable was assigned from a typed expression
        # (e.g., count = 0 → int), the MODULE_GET_ATTR result inherits
        # that type for downstream lowering.
        attr_hint: TypeHint = self._module_attr_type_hints.get(name, "Any")
        res = MoltValue(self.next_var(), type_hint=attr_hint)
        self.emit(
            MoltOp(
                kind="MODULE_GET_ATTR",
                args=[module_val, name_val],
                result=res,
            )
        )
        return res

    def _record_imported_module_attr_mutation(self, target: ast.Attribute) -> None:
        module_name = None
        if isinstance(target.value, ast.Name):
            module_name = self._imported_module_binding_target(target.value.id)
        else:
            parts = self._dotted_attribute_parts(target.value)
            if parts is not None:
                module_name = self._dotted_imported_module_target(parts)
        if module_name is None:
            return
        mutation = (module_name, target.attr)
        self.imported_module_attr_mutations.add(mutation)
        if self.current_func_name == "molt_main":
            self.global_imported_module_attr_mutations.add(mutation)

    def _imported_module_attr_is_stable(self, module_name: str, attr: str) -> bool:
        mutation = (module_name, attr)
        return (
            mutation not in self.imported_module_attr_mutations
            and mutation not in self.global_imported_module_attr_mutations
        )

    def _emit_module_attr_set_runtime(self, name: str, value: MoltValue) -> None:
        if self._function_needs_frame_trace():
            globals_dict = self._emit_globals_dict()
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
            self.emit(
                MoltOp(
                    kind="DICT_SET",
                    args=[globals_dict, name_val, value],
                    result=MoltValue("none"),
                )
            )
            return
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self._lexical_module_owner()
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[module_val, name_val, value],
                result=MoltValue("none"),
            )
        )

    def _imported_attr_name(self, bind_name: str) -> str:
        return self.imported_attr_names.get(
            bind_name, self.global_imported_attr_names.get(bind_name, bind_name)
        )

    def _emit_module_attr_get_on(self, module_name: str, name: str) -> MoltValue:
        module_val = self._emit_module_load(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        metadata = self._runtime_qualified_callable_metadata(module_name, name)
        self.emit(
            MoltOp(
                kind="MODULE_GET_ATTR",
                args=[module_val, name_val],
                result=res,
                metadata=metadata,
            )
        )
        return res

    def _emit_module_attr_get_default_on(
        self, module_name: str, name: str, default_val: MoltValue
    ) -> MoltValue:
        module_val = self._emit_module_load(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self._emit_getattr_name_default(
            module_val,
            name_val,
            default_val,
            res,
            literal_name=name,
            qualified_module_name=module_name,
        )
        return res

    def _runtime_protected_attribute_requirement_bits(
        self,
        obj: MoltValue,
        attr: str | None,
        *,
        exact_class: ClassId | None,
        qualified_module_name: str | None = None,
    ) -> int:
        if exact_class is not None or (
            isinstance(obj.type_hint, str) and obj.type_hint.startswith("super")
        ):
            return 0
        if qualified_module_name is not None and attr is not None:
            _, gateway_bits = self._runtime_qualified_callable_requirement(
                qualified_module_name, attr
            )
            if gateway_bits:
                return gateway_bits
        if attr is None:
            return SIMPLEIR_RUNTIME_PROTECTED_ACQUISITION_REQUIREMENTS
        return SIMPLEIR_RUNTIME_PROTECTED_ATTRIBUTE_REQUIREMENTS.get(attr, 0)

    def _emit_getattr_name_default(
        self,
        obj: MoltValue,
        name: MoltValue,
        default: MoltValue,
        result: MoltValue,
        *,
        literal_name: str | None,
        exact_class: ClassId | None = None,
        qualified_module_name: str | None = None,
    ) -> None:
        requirement_bits = self._runtime_protected_attribute_requirement_bits(
            obj,
            literal_name,
            exact_class=exact_class,
            qualified_module_name=qualified_module_name,
        )
        self.emit(
            MoltOp(
                kind="GETATTR_NAME_DEFAULT",
                args=[obj, name, default],
                result=result,
                metadata=(
                    {"runtime_requirement_bits": requirement_bits}
                    if requirement_bits
                    else None
                ),
            )
        )

    def _emit_guarded_setattr(
        self,
        obj: MoltValue,
        attr: str,
        value: MoltValue,
        expected_class: ClassId,
        *,
        obj_name: str | None = None,
    ) -> None:
        name = obj_name or obj.name
        class_info = self.classes.get(expected_class)
        if class_info and self._class_is_exception_subclass(expected_class):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        # Metaclass __init__ receives `cls` which is a TYPE object, not an
        # instance. Field offsets don't apply — always use generic setattr.
        if class_info and BuiltinClass("type") in class_info.get("bases", ()):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and attr not in class_info.get("fields", {}):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        assumption = self._loop_guard_assumption(name, expected_class)
        if assumption is True:
            self.emit(
                MoltOp(
                    kind="SETATTR",
                    args=[obj, attr, value, expected_class],
                    result=MoltValue("none"),
                )
            )
            return
        if assumption is False:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if self._class_layout_stable(expected_class):
            if self._exact_class_for_value(obj, obj_name) == expected_class:
                self.emit(
                    MoltOp(
                        kind="SETATTR",
                        args=[obj, attr, value, expected_class],
                        result=MoltValue("none"),
                    )
                )
                return
        guard = self._loop_guard_for(obj, expected_class, obj_name=name)
        if guard is None:
            definition = self._class_layout_definition(expected_class)
            if definition is None:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value],
                        result=MoltValue("none"),
                    )
                )
                return
            expected_version = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[self.classes[expected_class].get("layout_version", 0)],
                    result=expected_version,
                )
            )
            self.emit(
                MoltOp(
                    kind="GUARDED_SETATTR",
                    args=[obj, expected_version, attr, value, expected_class],
                    result=MoltValue("none"),
                    metadata={"class_definition": definition},
                )
            )
            return

        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="SETATTR",
                args=[obj, attr, value, expected_class],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_PTR",
                args=[obj, attr, value],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_guarded_getattr(
        self,
        obj: MoltValue,
        attr: str,
        expected_class: ClassId,
        *,
        obj_name: str | None = None,
    ) -> MoltValue:
        name = obj_name or obj.name
        class_info = self.classes.get(expected_class)
        if class_info and self._class_is_exception_subclass(expected_class):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        # Metaclass methods operate on TYPE objects — field offsets don't apply.
        if class_info and BuiltinClass("type") in class_info.get("bases", ()):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        if class_info and attr not in class_info.get("fields", {}):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        assumption = self._loop_guard_assumption(name, expected_class)
        if assumption is True:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, attr, expected_class],
                    result=res,
                )
            )
            return res
        if assumption is False:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        if self._class_layout_stable(expected_class):
            if self._exact_class_for_value(obj, obj_name) == expected_class:
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR",
                        args=[obj, attr, expected_class],
                        result=res,
                    )
                )
                return res
        guard = self._loop_guard_for(obj, expected_class, obj_name=name)
        if guard is None:
            definition = self._class_layout_definition(expected_class)
            if definition is None:
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(kind="GETATTR_GENERIC_PTR", args=[obj, attr], result=res)
                )
                return res
            expected_version = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[self.classes[expected_class].get("layout_version", 0)],
                    result=expected_version,
                )
            )
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GUARDED_GETATTR",
                    args=[obj, expected_version, attr, expected_class],
                    result=res,
                    metadata={"class_definition": definition},
                )
            )
            return res
        return self._emit_guarded_field_get_with_guard(
            obj,
            fast_attr=attr,
            fallback_attr=attr,
            expected_class=expected_class,
            guard=guard,
        )

    def _emit_guarded_field_get_with_guard(
        self,
        obj: MoltValue,
        fast_attr: str,
        fallback_attr: str,
        expected_class: ClassId,
        guard: MoltValue,
    ) -> MoltValue:
        use_phi = self.enable_phi and not self.is_async()
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, fast_attr, expected_class],
                    result=fast_val,
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, fallback_attr],
                    result=slow_val,
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = (
                fast_val.type_hint
                if fast_val.type_hint == slow_val.type_hint
                else "Any"
            )
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
            return merged

        # Non-phi path. Async/poll-function bodies must thread the merged
        # result through a closure slot — the LIST_NEW + STORE_INDEX cell
        # pattern was unsafe because Cranelift's loop-header phi resolver
        # could merge the cell SSA value with the entry-block default
        # (None) on the first iteration, producing store_index(None, ...)
        # crashes.
        if self.is_async():
            slot = self._new_async_internal_slot()
            none_init = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_init))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, none_init],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, fast_attr, expected_class],
                    result=fast_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, fast_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, fallback_attr],
                    result=slow_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, slow_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = (
                fast_val.type_hint
                if fast_val.type_hint == slow_val.type_hint
                else "Any"
            )
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=merged))
            return merged

        # Sync, non-phi path: a single SSA value updated in both branches.
        merged = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=merged))
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = MoltValue(self.next_var())
        self.emit(
            MoltOp(
                kind="GETATTR",
                args=[obj, fast_attr, expected_class],
                result=fast_val,
            )
        )
        self.emit(MoltOp(kind="COPY", args=[fast_val], result=merged))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_PTR",
                args=[obj, fallback_attr],
                result=slow_val,
            )
        )
        self.emit(MoltOp(kind="COPY", args=[slow_val], result=merged))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if fast_val.type_hint == slow_val.type_hint:
            merged.type_hint = fast_val.type_hint
        return merged

    def _emit_guarded_property_get(
        self,
        obj: MoltValue,
        attr: str,
        getter_symbol: str,
        expected_class: ClassId,
        *,
        obj_name: str | None = None,
    ) -> MoltValue:
        guard = self._loop_guard_for(obj, expected_class, obj_name=obj_name)
        if guard is None:
            guard = self._emit_layout_guard(obj, expected_class)
        use_phi = self.enable_phi and not self.is_async()
        # The receiver-layout guard certifies descriptor dispatch, not the
        # returned value. A getter annotation cannot certify its result lane.
        fast_hint = "Any"
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
            self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=slow_val,
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = fast_hint if fast_hint == slow_val.type_hint else "Any"
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
            return merged

        # Non-phi path. See `_emit_guarded_field_get_with_guard` for the full
        # rationale: in poll-function bodies we route the merged result
        # through a closure slot rather than a LIST_NEW + STORE_INDEX cell,
        # which is unsafe under Cranelift's loop-header phi resolver.
        if self.is_async():
            slot = self._new_async_internal_slot()
            none_init = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_init))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, none_init],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
            self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, fast_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=slow_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, slow_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = fast_hint if fast_hint == slow_val.type_hint else "Any"
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=merged))
            return merged

        # Sync, non-phi path: a single SSA value updated in both branches.
        merged = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=merged))
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
        self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
        self.emit(MoltOp(kind="COPY", args=[fast_val], result=merged))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_PTR",
                args=[obj, attr],
                result=slow_val,
            )
        )
        self.emit(MoltOp(kind="COPY", args=[slow_val], result=merged))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if fast_hint == slow_val.type_hint:
            merged.type_hint = fast_hint
        return merged

    def _emit_method_load(self, obj: MoltValue, attr: str) -> MoltValue:
        """CPython's LOAD_ATTR in method form, for a call that follows.

        One frontend value names the (callable, self_or_null) pair the runtime
        returns: a plain function on the receiver's type with the receiver, or
        any other attribute with the null sentinel. CALL_LOADED_METHOD consumes
        the pair directly, or CALLARGS_PUSH_METHOD_RECEIVER consumes the nullable
        receiver before argument assembly. Serialization names both results.
        """
        pair = MoltValue(self.next_var())
        self.emit(
            MoltOp(
                kind="LOAD_METHOD",
                args=[obj, attr],
                result=pair,
                metadata={"self_or_null": self.next_var()},
            )
        )
        return pair

    def _emit_generic_attribute_load(
        self,
        obj: MoltValue,
        attr: str,
        kind: str,
        *,
        method_load: bool,
        type_hint: TypeHint = "Unknown",
    ) -> MoltValue:
        """A generic attribute lookup, or its method form before a call."""
        if method_load:
            return self._emit_method_load(obj, attr)
        res = MoltValue(self.next_var(), type_hint=type_hint)
        self.emit(MoltOp(kind=kind, args=[obj, attr], result=res))
        return res

    def _emit_attribute_load(
        self,
        node: ast.Attribute,
        obj: MoltValue,
        obj_name: str | None,
        exact_class: ClassId | None,
        *,
        generic: bool = False,
        method_load: bool = False,
    ) -> MoltValue:
        # Set expression-level col_offset from the Attribute AST node so
        # that get_attr ops carry the correct column range for traceback
        # caret annotations (e.g. `x.upper` not `x.upper()`).
        _prev_expr_col = getattr(self, "_expr_col", None)
        _attr_col = getattr(node, "col_offset", None)
        _attr_end_col = getattr(node, "end_col_offset", None)
        if _attr_col is not None and _attr_end_col is not None:
            self._expr_col = (_attr_col, _attr_end_col)
        try:
            return self._emit_attribute_load_inner(
                node,
                obj,
                obj_name,
                exact_class,
                generic=generic,
                method_load=method_load,
            )
        finally:
            self._expr_col = _prev_expr_col

    def _emit_attribute_load_inner(
        self,
        node: ast.Attribute,
        obj: MoltValue,
        obj_name: str | None,
        exact_class: ClassId | None,
        *,
        generic: bool = False,
        method_load: bool = False,
    ) -> MoltValue:
        # `method_load` asks for CPython's method-form LOAD_ATTR wherever this
        # lookup is a generic attribute load: the result is then a method pair
        # (`_emit_method_load`). Field, property, module and protected loads
        # keep their own lowering, and their value is an ordinary callee.
        # Canonical imported-module callable acquisition is a producer fact.
        # It must be stamped before type-driven attribute lowering: a module
        # alias loaded through a function's module globals intentionally has an
        # ``Any`` value hint, yet lexical import resolution still proves the
        # exact sys/inspect binding. Target admission rejects this op itself, so
        # no use-sensitive callable/heap taint lane is necessary downstream.
        runtime_symbol, runtime_requirement_bits = (
            self._runtime_qualified_callable_provenance_for_binding(obj_name, node.attr)
        )
        if runtime_symbol is not None and not generic:
            attr_name = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.attr], result=attr_name))
            result = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="MODULE_GET_ATTR",
                    args=[obj, attr_name],
                    result=result,
                    metadata={"runtime_symbol": runtime_symbol},
                )
            )
            return result
        protected_requirement_bits = self._runtime_protected_attribute_requirement_bits(
            obj,
            node.attr,
            exact_class=exact_class,
        )
        if generic or runtime_requirement_bits or protected_requirement_bits:
            # Protected runtime callable acquisition is a value capability,
            # not a lexical-import spelling. Unknown receivers may carry a
            # sys/inspect module through conditionals, containers, calls, heap
            # storage, or any other Python value transport. Preserve ordinary
            # generic attribute semantics while stamping the acquisition
            # unless an exact non-module class is constructionally proven.
            result = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=result,
                    metadata={
                        "runtime_requirement_bits": (
                            runtime_requirement_bits | protected_requirement_bits
                        )
                    },
                )
            )
            return result
        declared = self._declared_class(obj)
        declared_class = (
            declared.class_id
            if declared is not None and declared.class_id in self.classes
            else None
        )
        # A classmethod's ``cls`` is the class object: instance layout,
        # instance methods and instance properties do not describe it.
        receiver_is_class_object = (
            declared is not None and declared.kind is ReceiverKind.CLASS_OBJECT
        )
        class_info = (
            self.classes[declared_class] if declared_class is not None else None
        )
        if declared_class is not None:
            getattribute_info, _ = self._resolve_method_info(
                declared_class, "__getattribute__"
            )
            if getattribute_info:
                return self._emit_generic_attribute_load(
                    obj, node.attr, "GETATTR_GENERIC_PTR", method_load=method_load
                )
        exact_dataclass_field = self._exact_dataclass_field(obj, obj_name, node.attr)
        if exact_dataclass_field is not None:
            exact_dataclass, field_offset = exact_dataclass_field
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_offset], result=idx_val))
            hint: TypeHint | None = None
            if self._hints_enabled():
                hint = (
                    self.classes[exact_dataclass].get("field_hints", {}).get(node.attr)
                )
            res = MoltValue(self.next_var(), type_hint=hint or "Unknown")
            self.emit(MoltOp(kind="DATACLASS_GET", args=[obj, idx_val], result=res))
            return res
        if class_info and class_info.get("dataclass"):
            return self._emit_generic_attribute_load(
                obj,
                node.attr,
                "GETATTR_GENERIC_OBJ",
                method_load=method_load,
                type_hint="Any",
            )
        method_info = None
        method_class: ClassId | None = None
        if declared_class is not None:
            method_info, method_class = self._resolve_method_info(
                declared_class, node.attr
            )
        if (
            method_info
            and method_info["descriptor"] == "function"
            and not receiver_is_class_object
        ):
            if method_class and method_class.module == self.module_name:
                # Avoid binding to same-module class methods directly; class
                # method objects are not guaranteed to be in scope for reuse.
                method_info = None
        if (
            declared_class is not None
            and class_info is not None
            and method_info
            and method_info["descriptor"] == "function"
            and not receiver_is_class_object
        ):
            fields = class_info.get("fields", {})
            if (
                not class_info.get("dynamic")
                and declared_class.module == self.module_name
                and node.attr not in fields
                and not self._instance_attr_mutated(declared_class, node.attr)
            ):
                if method_load:
                    # The method load selects the same function at run time
                    # and binds no method object.
                    return self._emit_method_load(obj, node.attr)
                func_val = method_info["func"]
                if self.current_func_name != "molt_main":
                    class_ref = MoltValue(self.next_var(), type_hint="type")
                    self.emit(MoltOp(kind="TYPE_OF", args=[obj], result=class_ref))
                    func_val = self._emit_class_method_func(class_ref, node.attr)
                res = MoltValue(
                    self.next_var(),
                    type_hint=BoundMethodHint(
                        method_class or declared_class, node.attr
                    ),
                )
                self.emit(
                    MoltOp(
                        kind="BOUND_METHOD_NEW",
                        args=[func_val, obj],
                        result=res,
                    )
                )
                return res
        if (
            declared_class is not None
            and class_info is not None
            and method_info
            and method_info["descriptor"] == "property"
            and not class_info.get("dynamic")
            and not receiver_is_class_object
        ):
            property_field = method_info.get("property_field")
            if property_field:
                field_map = class_info.get("fields", {})
                if (
                    property_field in field_map
                    and not self._class_attr_may_be_data_descriptor(
                        declared_class, property_field
                    )
                ):
                    guard = self._loop_guard_for(obj, declared_class, obj_name=obj_name)
                    if guard is None:
                        guard = self._emit_layout_guard(obj, declared_class)
                    return self._emit_guarded_field_get_with_guard(
                        obj,
                        fast_attr=property_field,
                        fallback_attr=node.attr,
                        expected_class=declared_class,
                        guard=guard,
                    )
            getter_hint = method_info["func"].type_hint
            assert isinstance(getter_hint, str), "property getter carries a Func hint"
            getter_symbol = getter_hint.split(":", 1)[1]
            return self._emit_guarded_property_get(
                obj,
                node.attr,
                getter_symbol,
                declared_class,
                obj_name=obj_name,
            )
        if isinstance(obj.type_hint, str) and obj.type_hint.startswith("module"):
            attr_name = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.attr], result=attr_name))
            res = MoltValue(self.next_var(), type_hint="Any")
            module_name = (
                self._imported_module_binding_target(obj_name)
                if obj_name is not None
                else None
            )
            metadata = (
                self._runtime_qualified_callable_metadata(module_name, node.attr)
                if module_name is not None
                else None
            )
            self.emit(
                MoltOp(
                    kind="MODULE_GET_ATTR",
                    args=[obj, attr_name],
                    result=res,
                    metadata=metadata,
                )
            )
            return res
        # Acquired method values use the same exact source-point result as
        # immediate method calls, never an annotation/transport hint.
        receiver_kind = self._builtin_exact_type_from_expr(node.value)
        _fast_methods = (
            _BUILTIN_FAST_METHODS.get(receiver_kind)
            if receiver_kind is not None
            else None
        )
        if receiver_kind is not None and _fast_methods is not None:
            if node.attr in _fast_methods:
                return self._emit_generic_attribute_load(
                    obj,
                    node.attr,
                    "GETATTR_GENERIC_OBJ",
                    method_load=method_load,
                    type_hint=BoundMethodHint(BuiltinClass(receiver_kind), node.attr),
                )
        expected_class = declared_class
        if expected_class is None or class_info is None:
            return self._emit_generic_attribute_load(
                obj, node.attr, "GETATTR_GENERIC_OBJ", method_load=method_load
            )
        if class_info.get("dynamic"):
            return self._emit_generic_attribute_load(
                obj, node.attr, "GETATTR_GENERIC_PTR", method_load=method_load
            )
        field_map = class_info.get("fields", {})
        if (
            receiver_is_class_object
            or node.attr not in field_map
            or self._class_attr_may_be_data_descriptor(expected_class, node.attr)
        ):
            return self._emit_generic_attribute_load(
                obj, node.attr, "GETATTR_GENERIC_PTR", method_load=method_load
            )
        field_hint: TypeHint | None = None
        if self._hints_enabled():
            field_hint = class_info.get("field_hints", {}).get(node.attr)
        res = self._emit_guarded_getattr(
            obj,
            node.attr,
            expected_class,
            obj_name=obj_name,
        )
        if field_hint is not None:
            res.type_hint = field_hint
        return res

    def _emit_attribute_store(
        self,
        obj: MoltValue | None,
        obj_expr: ast.expr | None,
        obj_name: str | None,
        attr: str,
        value_node: MoltValue,
    ) -> None:
        if obj_expr is not None:
            self._record_imported_module_attr_mutation(
                ast.Attribute(value=obj_expr, attr=attr, ctx=ast.Store())
            )
        if obj_expr is not None:
            stored_class = self._class_reference(obj_expr)
            if stored_class is not None:
                self._invalidate_loop_guards_for_class(stored_class)
        # Callers may have evaluated callback-capable operands since first
        # loading the receiver (notably augmented assignment). Re-resolve at
        # the consuming store; a captured class identity is never authority.
        exact_class = self._exact_class_for_value(obj, obj_name)
        declared = self._declared_class(obj)
        declared_class = (
            declared.class_id
            if declared is not None and declared.class_id in self.classes
            else None
        )
        class_info = (
            self.classes[declared_class] if declared_class is not None else None
        )
        if exact_class is not None:
            self._record_instance_attr_mutation(exact_class, attr)
        elif declared_class is not None:
            self._record_instance_attr_mutation(declared_class, attr)
        if exact_class is not None and obj is not None:
            exact_info = self.classes.get(exact_class)
            if (
                exact_info
                and not exact_info.get("dynamic")
                and not exact_info.get("dataclass")
            ):
                field_map = exact_info.get("fields", {})
                if attr in field_map and not self._class_attr_may_be_data_descriptor(
                    exact_class, attr
                ):
                    self._emit_guarded_setattr(
                        obj,
                        attr,
                        value_node,
                        exact_class,
                        obj_name=obj_name,
                    )
                    return
        exact_dataclass_field = (
            self._exact_dataclass_field(obj, obj_name, attr)
            if obj is not None
            else None
        )
        if exact_dataclass_field is not None:
            _, field_offset = exact_dataclass_field
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_offset], result=idx_val))
            self.emit(
                MoltOp(
                    kind="DATACLASS_SET",
                    args=[obj, idx_val, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and class_info.get("dataclass"):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        if obj is not None and declared_class is not None and class_info is not None:
            if (
                class_info.get("dynamic")
                or attr not in class_info.get("fields", {})
                or declared is not None
                and declared.kind is ReceiverKind.CLASS_OBJECT
                or self._class_attr_may_be_data_descriptor(declared_class, attr)
            ):
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
            else:
                self._emit_guarded_setattr(
                    obj,
                    attr,
                    value_node,
                    declared_class,
                    obj_name=obj_name,
                )
        else:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value_node],
                    result=MoltValue("none"),
                )
            )
