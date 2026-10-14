"""SerializationObjectAttrOpsMixin: JSON serialization for allocation, object layout, attribute, guard, refcount, alias, and parsing ops."""

from __future__ import annotations

from typing import Any

from molt.frontend._types import (
    BuiltinClass,
    MoltOp,
    MoltValue,
)
from molt.frontend.lowering.serialization_context import SerializationContext
from molt.frontend._mixin_base import GeneratorMixinBase


class SerializationObjectAttrOpsMixin(GeneratorMixinBase):
    def _serialize_object_attr_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == "DATACLASS_NEW":
            ctx.json_ops.append(
                {
                    "kind": "dataclass_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DATACLASS_NEW_VALUES":
            ctx.json_ops.append(
                {
                    "kind": "dataclass_new_values",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SETATTR":
            obj, attr, val, expected_class = op.args
            offset = self._serialization_field_offset(expected_class, attr)
            # Metaclass __init__ receives `cls` which is a TYPE object,
            # not an INSTANCE. Field offsets apply to instances, not to
            # the class itself. When the expected class IS the metaclass
            # (a subclass of `type`), emit generic setattr so the
            # attribute goes into the class __dict__.
            _class_info_meta = self.classes.get(expected_class)
            _is_type_subclass = _class_info_meta is not None and BuiltinClass(
                "type"
            ) in _class_info_meta.get("bases", ())
            if offset is None or _is_type_subclass:
                if self._class_is_exception_subclass(expected_class):
                    ctx.json_ops.append(
                        {
                            "kind": "set_attr_generic_obj",
                            "args": [obj.name, val.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
                else:
                    ctx.json_ops.append(
                        {
                            "kind": "set_attr_generic_ptr",
                            "args": [obj.name, val.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
            else:
                ctx.json_ops.append(
                    {
                        "kind": "store",
                        "args": [obj.name, val.name],
                        "value": offset,
                    }
                )
        elif op.kind == "GUARDED_SETATTR":
            obj, class_ref, expected_version, attr, val, expected_class = op.args
            offset = self._serialization_field_offset(expected_class, attr)
            if offset is None:
                if self._class_is_exception_subclass(expected_class):
                    ctx.json_ops.append(
                        {
                            "kind": "set_attr_generic_obj",
                            "args": [obj.name, val.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
                else:
                    ctx.json_ops.append(
                        {
                            "kind": "set_attr_generic_ptr",
                            "args": [obj.name, val.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
            else:
                ctx.json_ops.append(
                    {
                        "kind": "guarded_field_set",
                        "args": [
                            obj.name,
                            class_ref.name,
                            expected_version.name,
                            val.name,
                        ],
                        "s_value": attr,
                        "value": offset,
                        "out": op.result.name,
                    }
                )
        elif op.kind == "SETATTR_GENERIC_PTR":
            ctx.json_ops.append(
                {
                    "kind": "set_attr_generic_ptr",
                    "args": [op.args[0].name, op.args[2].name],
                    "s_value": op.args[1],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SETATTR_GENERIC_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "set_attr_generic_obj",
                    "args": [op.args[0].name, op.args[2].name],
                    "s_value": op.args[1],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DELATTR_GENERIC_PTR":
            ctx.json_ops.append(
                {
                    "kind": "del_attr_generic_ptr",
                    "args": [op.args[0].name],
                    "s_value": op.args[1],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DELATTR_GENERIC_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "del_attr_generic_obj",
                    "args": [op.args[0].name],
                    "s_value": op.args[1],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DATACLASS_GET":
            ctx.json_ops.append(
                {
                    "kind": "dataclass_get",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DATACLASS_SET":
            ctx.json_ops.append(
                {
                    "kind": "dataclass_set",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DATACLASS_SET_CLASS":
            ctx.json_ops.append(
                {
                    "kind": "dataclass_set_class",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GETATTR":
            obj, attr, expected_class = op.args
            offset = self._serialization_field_offset(expected_class, attr)
            # Metaclass methods operate on TYPE objects, not instances.
            # Field offsets don't apply — use generic getattr.
            _ga_class_info = self.classes.get(expected_class)
            _ga_is_type_sub = _ga_class_info is not None and BuiltinClass(
                "type"
            ) in _ga_class_info.get("bases", ())
            if offset is None or _ga_is_type_sub:
                if self._class_is_exception_subclass(expected_class):
                    ctx.json_ops.append(
                        {
                            "kind": "get_attr_generic_obj",
                            "args": [obj.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
                else:
                    ctx.json_ops.append(
                        {
                            "kind": "get_attr_generic_ptr",
                            "args": [obj.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
            else:
                ctx.json_ops.append(
                    {
                        "kind": "load",
                        "args": [obj.name],
                        "value": offset,
                        "out": op.result.name,
                    }
                )
        elif op.kind == "GUARDED_GETATTR":
            obj, class_ref, expected_version, attr, expected_class = op.args
            offset = self._serialization_field_offset(expected_class, attr)
            if offset is None:
                if self._class_is_exception_subclass(expected_class):
                    ctx.json_ops.append(
                        {
                            "kind": "get_attr_generic_obj",
                            "args": [obj.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
                else:
                    ctx.json_ops.append(
                        {
                            "kind": "get_attr_generic_ptr",
                            "args": [obj.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
            else:
                ctx.json_ops.append(
                    {
                        "kind": "guarded_field_get",
                        "args": [obj.name, class_ref.name, expected_version.name],
                        "s_value": attr,
                        "value": offset,
                        "out": op.result.name,
                    }
                )
        elif op.kind == "GETATTR_GENERIC_PTR":
            ptr_entry: dict[str, Any] = {
                "kind": "get_attr_generic_ptr",
                "args": [op.args[0].name],
                "s_value": op.args[1],
                "out": op.result.name,
            }
            ctx.json_ops.append(ptr_entry)
        elif op.kind == "GETATTR_GENERIC_OBJ":
            entry = {
                "kind": "get_attr_generic_obj",
                "args": [op.args[0].name],
                "s_value": op.args[1],
                "out": op.result.name,
            }
            if op.metadata is not None:
                runtime_requirement_bits = op.metadata.get("runtime_requirement_bits")
                if (
                    isinstance(runtime_requirement_bits, int)
                    and runtime_requirement_bits
                ):
                    entry["runtime_requirement_bits"] = runtime_requirement_bits
            ctx.json_ops.append(entry)
        elif op.kind == "GETATTR_SPECIAL_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "get_attr_special_obj",
                    "args": [op.args[0].name],
                    "s_value": op.args[1],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GETATTR_NAME_DEFAULT":
            entry = {
                "kind": "get_attr_name_default",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if op.metadata is not None:
                runtime_requirement_bits = op.metadata.get("runtime_requirement_bits")
                if (
                    isinstance(runtime_requirement_bits, int)
                    and runtime_requirement_bits
                ):
                    entry["runtime_requirement_bits"] = runtime_requirement_bits
            ctx.json_ops.append(entry)
        elif op.kind == "HASATTR_NAME":
            ctx.json_ops.append(
                {
                    "kind": "has_attr_name",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SETATTR_NAME":
            ctx.json_ops.append(
                {
                    "kind": "set_attr_name",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DELATTR_NAME":
            ctx.json_ops.append(
                {
                    "kind": "del_attr_name",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GUARD_TAG":
            guard = {
                "kind": op.kind.lower(),
                "args": [arg.name for arg in op.args],
            }
            if op.result.name != "none":
                guard["out"] = op.result.name
            ctx.json_ops.append(guard)
        elif op.kind == "INC_REF":
            ctx.json_ops.append(
                {
                    "kind": "inc_ref",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DEC_REF":
            ctx.json_ops.append(
                {
                    "kind": "dec_ref",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BORROW":
            ctx.json_ops.append(
                {
                    "kind": "inc_ref",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "RELEASE":
            ctx.json_ops.append(
                {
                    "kind": "dec_ref",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind in {
            "BOX",
            "UNBOX",
            "CAST",
            "WIDEN",
        }:
            if (
                op.args
                and isinstance(op.args[0], MoltValue)
                and op.result.name != "none"
            ):
                lowered_kind = {
                    "BOX": "box",
                    "UNBOX": "unbox",
                    "CAST": "cast",
                    "WIDEN": "widen",
                }[op.kind]
                ctx.json_ops.append(
                    {
                        "kind": lowered_kind,
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
        elif op.kind == "IDENTITY_ALIAS":
            if (
                op.args
                and isinstance(op.args[0], MoltValue)
                and op.result.name != "none"
            ):
                ctx.json_ops.append(
                    {
                        "kind": "identity_alias",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
        elif op.kind == "BINDING_ALIAS":
            if (
                op.args
                and isinstance(op.args[0], MoltValue)
                and op.result.name != "none"
            ):
                ctx.json_ops.append(
                    {
                        "kind": "binding_alias",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
        else:
            return False
        return True
