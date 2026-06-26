"""SerializationObjectAttrOpsMixin: JSON serialization for allocation, object layout, attribute, guard, refcount, alias, and parsing ops."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt.frontend._types import (
    _next_ic_index,
    MoltOp,
    MoltValue,
)
from molt.frontend.lowering.serialization_context import SerializationContext

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class SerializationObjectAttrOpsMixin(_MixinBase):
    def _serialize_object_attr_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == "ALLOC":
            ctx.json_ops.append(
                {
                    "kind": "alloc",
                    "value": self.classes[op.args[0]]["size"],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ALLOC_CLASS":
            class_ref, class_id = op.args
            ctx.json_ops.append(
                {
                    "kind": "alloc_class",
                    "args": [class_ref.name],
                    "value": self.classes[class_id]["size"],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ALLOC_CLASS_TRUSTED":
            class_ref, class_id = op.args
            ctx.json_ops.append(
                {
                    "kind": "alloc_class_trusted",
                    "args": [class_ref.name],
                    "value": self.classes[class_id]["size"],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ALLOC_CLASS_STATIC":
            class_ref, class_id = op.args
            ctx.json_ops.append(
                {
                    "kind": "alloc_class_static",
                    "args": [class_ref.name],
                    "value": self.classes[class_id]["size"],
                    "out": op.result.name,
                }
            )
        elif op.kind == "OBJECT_SET_CLASS":
            ctx.json_ops.append(
                {
                    "kind": "object_set_class",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DATACLASS_NEW":
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
            obj, attr, val, *rest = op.args
            if rest:
                expected_class = rest[0]
            else:
                expected_class = list(self.classes.keys())[-1]
            offset = self._serialization_field_offset(expected_class, attr)
            # Metaclass __init__ receives `cls` which is a TYPE object,
            # not an INSTANCE. Field offsets apply to instances, not to
            # the class itself. When the expected class IS the metaclass
            # (a subclass of `type`), emit generic setattr so the
            # attribute goes into the class __dict__.
            _class_info_meta = self.classes.get(expected_class)
            _is_type_subclass = (
                _class_info_meta is not None
                and "type" in _class_info_meta.get("bases", [])
            )
            if offset is None or _is_type_subclass:
                class_info = self.classes.get(expected_class)
                if class_info and self._class_is_exception_subclass(
                    expected_class, class_info
                ):
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
                        # The concrete class whose fixed layout authored this
                        # `offset` (the frontend emits the raw-offset form only
                        # when the object's class is statically proven here).
                        # Carried through TIR so the alias oracle can assign a
                        # class+offset `TypedField` region (S5-1.5).
                        "class": expected_class,
                    }
                )
        elif op.kind == "SETATTR_INIT":
            obj, attr, val, *rest = op.args
            if rest:
                expected_class = rest[0]
            else:
                expected_class = list(self.classes.keys())[-1]
            offset = self._serialization_field_offset(expected_class, attr)
            if offset is None:
                class_info = self.classes.get(expected_class)
                if class_info and self._class_is_exception_subclass(
                    expected_class, class_info
                ):
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
                        "kind": "store_init",
                        "args": [obj.name, val.name],
                        "value": offset,
                        "class": expected_class,
                    }
                )
        elif op.kind == "GUARDED_SETATTR":
            obj, class_ref, expected_version, attr, val, expected_class = op.args
            offset = self._serialization_field_offset(expected_class, attr)
            if offset is None:
                class_info = self.classes.get(expected_class)
                if class_info and self._class_is_exception_subclass(
                    expected_class, class_info
                ):
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
                        # The class the runtime version-guard proves at this
                        # op; authority for `offset`. Carried through TIR for
                        # the class+offset `TypedField` alias region (S5-1.5).
                        "class": expected_class,
                    }
                )
        elif op.kind == "GUARDED_SETATTR_INIT":
            obj, class_ref, expected_version, attr, val, expected_class = op.args
            offset = self._serialization_field_offset(expected_class, attr)
            if offset is None:
                class_info = self.classes.get(expected_class)
                if class_info and self._class_is_exception_subclass(
                    expected_class, class_info
                ):
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
                        "kind": "guarded_field_init",
                        "args": [
                            obj.name,
                            class_ref.name,
                            expected_version.name,
                            val.name,
                        ],
                        "s_value": attr,
                        "value": offset,
                        "out": op.result.name,
                        "class": expected_class,
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
            obj, attr, *rest = op.args
            if rest:
                expected_class = rest[0]
            else:
                expected_class = list(self.classes.keys())[-1]
            offset = self._serialization_field_offset(expected_class, attr)
            # Metaclass methods operate on TYPE objects, not instances.
            # Field offsets don't apply — use generic getattr.
            _ga_class_info = self.classes.get(expected_class)
            _ga_is_type_sub = (
                _ga_class_info is not None and "type" in _ga_class_info.get("bases", [])
            )
            if offset is None or _ga_is_type_sub:
                class_info = self.classes.get(expected_class)
                if class_info and self._class_is_exception_subclass(
                    expected_class, class_info
                ):
                    ctx.json_ops.append(
                        {
                            "kind": "get_attr_generic_obj",
                            "args": [obj.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
                else:
                    _ic = (
                        op.metadata["ic_index"]
                        if op.metadata and "ic_index" in op.metadata
                        else _next_ic_index()
                    )
                    ctx.json_ops.append(
                        {
                            "kind": "get_attr_generic_ptr",
                            "args": [obj.name],
                            "s_value": attr,
                            "out": op.result.name,
                            "metadata": {"ic_index": _ic},
                        }
                    )
            else:
                ctx.json_ops.append(
                    {
                        "kind": "load",
                        "args": [obj.name],
                        "value": offset,
                        "out": op.result.name,
                        # The statically-proven class authoring `offset`.
                        # Carried through TIR for the class+offset
                        # `TypedField` alias region (S5-1.5).
                        "class": expected_class,
                    }
                )
        elif op.kind == "GUARDED_GETATTR":
            obj, class_ref, expected_version, attr, expected_class = op.args
            offset = self._serialization_field_offset(expected_class, attr)
            if offset is None:
                class_info = self.classes.get(expected_class)
                if class_info and self._class_is_exception_subclass(
                    expected_class, class_info
                ):
                    ctx.json_ops.append(
                        {
                            "kind": "get_attr_generic_obj",
                            "args": [obj.name],
                            "s_value": attr,
                            "out": op.result.name,
                        }
                    )
                else:
                    _ic = (
                        op.metadata["ic_index"]
                        if op.metadata and "ic_index" in op.metadata
                        else _next_ic_index()
                    )
                    ctx.json_ops.append(
                        {
                            "kind": "get_attr_generic_ptr",
                            "args": [obj.name],
                            "s_value": attr,
                            "out": op.result.name,
                            "metadata": {"ic_index": _ic},
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
                        "metadata": {"expected_type_id": 100},
                        # The class the runtime version-guard proves at this
                        # op; authority for `offset`. Carried through TIR for
                        # the class+offset `TypedField` alias region (S5-1.5).
                        "class": expected_class,
                    }
                )
        elif op.kind == "GETATTR_GENERIC_PTR":
            ptr_entry: dict[str, Any] = {
                "kind": "get_attr_generic_ptr",
                "args": [op.args[0].name],
                "s_value": op.args[1],
                "out": op.result.name,
            }
            if op.metadata and "ic_index" in op.metadata:
                ptr_entry["metadata"] = {"ic_index": op.metadata["ic_index"]}
            ctx.json_ops.append(ptr_entry)
        elif op.kind == "GETATTR_GENERIC_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "get_attr_generic_obj",
                    "args": [op.args[0].name],
                    "s_value": op.args[1],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FUNCTION_DEFAULTS_VERSION":
            # Read a function object's __defaults__/__kwdefaults__ mutation
            # version stamp (one MoltValue operand: the function object).
            # Non-foldable; consumed by the defaults-devirt deopt guard.
            ctx.json_ops.append(
                {
                    "kind": "function_defaults_version",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GETATTR_SPECIAL_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "get_attr_special_obj",
                    "args": [op.args[0].name],
                    "s_value": op.args[1],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GETATTR_NAME":
            ctx.json_ops.append(
                {
                    "kind": "get_attr_name",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GETATTR_NAME_DEFAULT":
            ctx.json_ops.append(
                {
                    "kind": "get_attr_name_default",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "HASATTR_NAME":
            ctx.json_ops.append(
                {
                    "kind": "has_attr_name",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "IS_NATIVE_AWAITABLE":
            ctx.json_ops.append(
                {
                    "kind": "is_native_awaitable",
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
        elif op.kind == "GUARD_TYPE":
            ctx.json_ops.append(
                {
                    "kind": "guard_type",
                    "args": [arg.name for arg in op.args],
                }
            )
        elif op.kind == "GUARD_TAG":
            ctx.json_ops.append(
                {
                    "kind": "guard_tag",
                    "args": [arg.name for arg in op.args],
                }
            )
        elif op.kind == "GUARD_DICT_SHAPE":
            ctx.json_ops.append(
                {
                    "kind": "guard_dict_shape",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
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
        elif op.kind == "JSON_PARSE":
            ctx.json_ops.append(
                {
                    "kind": "json_parse",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MSGPACK_PARSE":
            ctx.json_ops.append(
                {
                    "kind": "msgpack_parse",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CBOR_PARSE":
            ctx.json_ops.append(
                {
                    "kind": "cbor_parse",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        else:
            return False
        return True
