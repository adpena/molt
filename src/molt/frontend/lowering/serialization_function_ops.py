"""SerializationFunctionOpsMixin: JSON serialization for function, code-slot, class-constructor, and module/context object setup ops."""

from __future__ import annotations

from molt.frontend.lowering.op_kinds_generated import (
    FrontendKinds,
)


from molt.frontend._types import (
    ClassId,
    MoltOp,
    MoltValue,
)
from molt.frontend.lowering.serialization_context import (
    SerializationContext,
    SimpleIROp,
)
from molt.frontend._mixin_base import GeneratorMixinBase


class SerializationFunctionOpsMixin(GeneratorMixinBase):
    def _serialize_function_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == FrontendKinds.CODE_SLOTS_INIT:
            # The program wrapper owns global allocation. A module record
            # contains symbolic owners only, even after a program projection.
            if ctx.module_init_symbol is None:
                ctx.json_ops.append(
                    {
                        "kind": "code_slots_init",
                        "value": int(op.args[0]),
                    }
                )
        elif op.kind == FrontendKinds.CLASS_DEF:
            ctx.json_ops.append(
                {
                    "kind": "class_def",
                    "args": [
                        arg.name if isinstance(arg, MoltValue) else str(arg)
                        for arg in op.args
                    ],
                    "s_value": op.metadata["s_value"] if op.metadata else "",
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.MISSING:
            ctx.json_ops.append(
                {
                    "kind": "missing",
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BUILTIN_TYPE:
            ctx.json_ops.append(
                {
                    "kind": "builtin_type",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.TYPE_OF:
            ctx.json_ops.append(
                {
                    "kind": "type_of",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CLASS_SET_DEFINITION:
            (created_class,) = op.args
            entry: SimpleIROp = {
                "kind": "class_set_definition",
                "args": [created_class.name],
                "class_definition": (op.metadata or {})["class_definition"],
                "out": op.result.name,
            }
            # The class's facts are frozen once lowering ends, so its exact
            # field slots are read here (design 78, 5.8).
            slots = self._exact_field_slots((op.metadata or {})["class_id"])
            if slots:
                entry["exact_field_slots"] = [
                    [name, offset] for name, offset in sorted(slots.items())
                ]
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.GUARD_LAYOUT:
            obj, expected_version = op.args
            ctx.json_ops.append(
                {
                    "kind": "guard_layout",
                    "args": [obj.name, expected_version.name],
                    "class_definition": (op.metadata or {})["class_definition"],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.ISINSTANCE:
            ctx.json_ops.append(
                {
                    "kind": "isinstance",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_MATCH_BUILTIN:
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "exception_match_builtin",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                    "s_value": metadata["exception_name"],
                    "value": int(metadata["exception_tag"]),
                }
            )
        elif op.kind == FrontendKinds.ISSUBCLASS:
            ctx.json_ops.append(
                {
                    "kind": "issubclass",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.OBJECT_NEW:
            ctx.json_ops.append(
                {"kind": "object_new", "args": [], "out": op.result.name}
            )
        elif op.kind == FrontendKinds.OBJECT_NEW_BOUND:
            # Phase-1-sibling class-instantiation fast path:
            # `Point(args)` for a known non-dynamic class lowers to
            # `OBJECT_NEW_BOUND(class_ref)` (allocates instance with
            # the right type tag) followed by a direct `CALL` to
            # `__init__`'s symbol — bypassing
            # `type.__call__` → bound-method-init → CALL_BIND.
            # See `_try_emit_class_static_call` for the predicates.
            # Static payload extent supports typed-field analysis; it does
            # not authorize frame placement. The runtime owns allocation size.
            _onb_op: SimpleIROp = {
                "kind": "object_new_bound",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            # The allocating constructor bound the class at emission. Its
            # result is exactly an instance of the class statement it names
            # (HF-847 slice 1, D6): the backend's typed `ClassId` comes from
            # this symbol, never from a spelled class name.
            _onb_md = op.metadata or {}
            _onb_class = _onb_md["class_id"]
            if not isinstance(_onb_class, ClassId):
                raise TypeError("OBJECT_NEW_BOUND requires its ClassId")
            _onb_definition = self._class_layout_definition(_onb_class)
            if _onb_definition is None:
                raise TypeError(
                    "OBJECT_NEW_BOUND requires a class statement that stamps its identity"
                )
            _onb_op["class_definition"] = _onb_definition
            _onb_size = _onb_md.get("class_size_bytes")
            if isinstance(_onb_size, int) and _onb_size > 0:
                _onb_op["value"] = _onb_size
            # The class's instance plan, when its layout proves one
            # (``class_model.release_plan``): the field word count.
            _onb_plan = _onb_md.get("release_plan")
            if isinstance(_onb_plan, int) and not isinstance(_onb_plan, bool):
                _onb_op["release_plan"] = _onb_plan
            # Positive finalizer metadata constrains lifetime transformations.
            # Its absence does not prove finalizer freedom: a mutable class
            # can acquire __del__ later. Owned allocation always retains RC.
            _del_member, _ = self._resolve_member(_onb_class, "__del__")
            if _del_member is not None:
                _onb_op["defines_del"] = True
            ctx.json_ops.append(_onb_op)
        elif op.kind == FrontendKinds.CLASSMETHOD_NEW:
            ctx.json_ops.append(
                {
                    "kind": "classmethod_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STATICMETHOD_NEW:
            ctx.json_ops.append(
                {
                    "kind": "staticmethod_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.PROPERTY_NEW:
            ctx.json_ops.append(
                {
                    "kind": "property_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BOUND_METHOD_NEW:
            ctx.json_ops.append(
                {
                    "kind": "bound_method_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.MODULE_NEW:
            ctx.json_ops.append(
                {
                    "kind": "module_new",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.IMPORT_NAME:
            assert op.metadata is not None
            ctx.json_ops.append(
                {
                    "kind": "import_name",
                    "s_value": op.metadata["module"],
                    "value": op.metadata["level"],
                    "fromlist": list(op.metadata["fromlist"]),
                    "import_form": op.metadata["import_form"],
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.MODULE_CACHE_SET:
            ctx.json_ops.append(
                {
                    "kind": "module_cache_set",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.MODULE_CACHE_DEL:
            ctx.json_ops.append(
                {
                    "kind": "module_cache_del",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.MODULE_GET_ATTR:
            entry: SimpleIROp = {
                "kind": "module_get_attr",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if op.metadata is not None:
                runtime_symbol = op.metadata.get("runtime_symbol")
                if isinstance(runtime_symbol, str) and runtime_symbol:
                    entry["runtime_symbol"] = runtime_symbol
                runtime_requirement_bits = op.metadata.get("runtime_requirement_bits")
                if (
                    isinstance(runtime_requirement_bits, int)
                    and runtime_requirement_bits
                ):
                    entry["runtime_requirement_bits"] = runtime_requirement_bits
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.MODULE_IMPORT_FROM:
            entry: SimpleIROp = {
                "kind": "module_import_from",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if op.metadata is not None:
                runtime_symbol = op.metadata.get("runtime_symbol")
                if isinstance(runtime_symbol, str) and runtime_symbol:
                    entry["runtime_symbol"] = runtime_symbol
                runtime_requirement_bits = op.metadata.get("runtime_requirement_bits")
                if (
                    isinstance(runtime_requirement_bits, int)
                    and runtime_requirement_bits
                ):
                    entry["runtime_requirement_bits"] = runtime_requirement_bits
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.MODULE_GET_GLOBAL:
            entry: SimpleIROp = {
                "kind": "module_get_global",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if op.metadata is not None:
                runtime_symbol = op.metadata.get("runtime_symbol")
                if isinstance(runtime_symbol, str) and runtime_symbol:
                    entry["runtime_symbol"] = runtime_symbol
                runtime_requirement_bits = op.metadata.get("runtime_requirement_bits")
                if (
                    isinstance(runtime_requirement_bits, int)
                    and runtime_requirement_bits
                ):
                    entry["runtime_requirement_bits"] = runtime_requirement_bits
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.MODULE_DEL_GLOBAL:
            ctx.json_ops.append(
                {
                    "kind": "module_del_global",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.MODULE_DEL_GLOBAL_IF_PRESENT:
            ctx.json_ops.append(
                {
                    "kind": "module_del_global_if_present",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.MODULE_SET_ATTR:
            ctx.json_ops.append(
                {
                    "kind": "module_set_attr",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.MODULE_IMPORT_STAR:
            ctx.json_ops.append(
                {
                    "kind": "module_import_star",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CONTEXT_ENTER:
            ctx.json_ops.append(
                {
                    "kind": "context_enter",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CONTEXT_EXIT:
            ctx.json_ops.append(
                {
                    "kind": "context_exit",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        else:
            return False
        return True
