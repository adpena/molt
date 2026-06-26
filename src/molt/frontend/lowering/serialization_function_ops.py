"""SerializationFunctionOpsMixin: JSON serialization for function, code-slot, class-constructor, and module/context object setup ops."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt.frontend._types import (
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


class SerializationFunctionOpsMixin(_MixinBase):
    def _serialize_function_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == "CODE_SLOTS_INIT":
            ctx.json_ops.append(
                {
                    "kind": "code_slots_init",
                    "value": int(op.args[0]),
                }
            )
        elif op.kind == "CLASS_NEW":
            ctx.json_ops.append(
                {
                    "kind": "class_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CLASS_SET_BASE":
            ctx.json_ops.append(
                {
                    "kind": "class_set_base",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CLASS_APPLY_SET_NAME":
            ctx.json_ops.append(
                {
                    "kind": "class_apply_set_name",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CLASS_DEF":
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
        elif op.kind == "SUPER_NEW":
            ctx.json_ops.append(
                {
                    "kind": "super_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MISSING":
            ctx.json_ops.append(
                {
                    "kind": "missing",
                    "out": op.result.name,
                }
            )
        elif op.kind == "COPY":
            ctx.json_ops.append(
                {
                    "kind": "copy",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FUNCTION_CLOSURE_BITS":
            ctx.json_ops.append(
                {
                    "kind": "function_closure_bits",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BUILTIN_TYPE":
            ctx.json_ops.append(
                {
                    "kind": "builtin_type",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TYPE_OF":
            ctx.json_ops.append(
                {
                    "kind": "type_of",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CLASS_VERSION":
            ctx.json_ops.append(
                {
                    "kind": "class_layout_version",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CLASS_SET_LAYOUT_VERSION":
            ctx.json_ops.append(
                {
                    "kind": "class_set_layout_version",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CLASS_MERGE_LAYOUT":
            ctx.json_ops.append(
                {
                    "kind": "class_merge_layout",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GUARD_LAYOUT":
            ctx.json_ops.append(
                {
                    "kind": "guard_layout",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ISINSTANCE":
            ctx.json_ops.append(
                {
                    "kind": "isinstance",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_MATCH_BUILTIN":
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "exception_match_builtin",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                    "s_value": metadata.get("exception_name", "Exception"),
                    "value": int(metadata.get("exception_tag", 2)),
                }
            )
        elif op.kind == "ISSUBCLASS":
            ctx.json_ops.append(
                {
                    "kind": "issubclass",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "OBJECT_NEW":
            ctx.json_ops.append(
                {"kind": "object_new", "args": [], "out": op.result.name}
            )
        elif op.kind == "OBJECT_NEW_BOUND":
            # Phase-1-sibling class-instantiation fast path:
            # `Point(args)` for a known non-dynamic class lowers to
            # `OBJECT_NEW_BOUND(class_ref)` (allocates instance with
            # the right type tag) followed by a direct `CALL` to
            # `__init__`'s symbol — bypassing
            # `type.__call__` → bound-method-init → CALL_BIND.
            # See `_try_emit_class_static_call` for the predicates.
            # The optional `value` field carries the static
            # class-instance payload size in bytes (header NOT
            # included), which the escape-analysis-rewritten
            # `object_new_bound_stack` lowering uses to size the
            # Cranelift StackSlot.  Heap arm ignores it.
            _onb_op: dict[str, Any] = {
                "kind": "object_new_bound",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            _onb_class = op.result.type_hint
            if _onb_class and _onb_class != "Any":
                _onb_op["type_hint"] = _onb_class
            _onb_md = op.metadata or {}
            _onb_size = _onb_md.get("class_size_bytes")
            if isinstance(_onb_size, int) and _onb_size > 0:
                _onb_op["value"] = _onb_size
            # A class that defines `__del__` (directly or anywhere in its MRO
            # except `object`) has a finalizer that CPython runs at the last
            # reference drop.  The backend escape pass would otherwise treat a
            # non-escaping instance as `NoEscape` and (a) strip its IncRef/
            # DecRef and/or (b) rewrite it to a stack-allocated immortal
            # object — either of which makes the refcount-zero transition
            # never occur, so `__del__` would silently never run.  Carry the
            # finalizer fact so escape analysis keeps such instances
            # heap-allocated with a live refcount (the finalizer-aware
            # `dec_ref_ptr` then dispatches `__del__` at the last drop).  The
            # heap `object_new_bound` fast path (and inlined `__init__`) is
            # preserved — only the unsound stack/RC-strip is suppressed.
            if _onb_class and _onb_class != "Any":
                _del_info, _del_owner = self._resolve_method_info(_onb_class, "__del__")
                if _del_info is not None and _del_owner != "object":
                    _onb_op["defines_del"] = True
            if (op.metadata or {}).get("bound_local"):
                _onb_op["bound_local"] = True
            ctx.json_ops.append(_onb_op)
        elif op.kind == "CLASSMETHOD_NEW":
            ctx.json_ops.append(
                {
                    "kind": "classmethod_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STATICMETHOD_NEW":
            ctx.json_ops.append(
                {
                    "kind": "staticmethod_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "PROPERTY_NEW":
            ctx.json_ops.append(
                {
                    "kind": "property_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BOUND_METHOD_NEW":
            ctx.json_ops.append(
                {
                    "kind": "bound_method_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_NEW":
            ctx.json_ops.append(
                {
                    "kind": "module_new",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_CACHE_GET":
            entry = {
                "kind": "module_cache_get",
                "args": [op.args[0].name],
                "out": op.result.name,
            }
            if op.metadata is not None:
                effect_proof = op.metadata.get("effect_proof")
                if isinstance(effect_proof, str) and effect_proof:
                    entry["effect_proof"] = effect_proof
            ctx.json_ops.append(entry)
        elif op.kind == "MODULE_IMPORT":
            ctx.json_ops.append(
                {
                    "kind": "module_import",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_CACHE_SET":
            ctx.json_ops.append(
                {
                    "kind": "module_cache_set",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_CACHE_DEL":
            ctx.json_ops.append(
                {
                    "kind": "module_cache_del",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_GET_ATTR":
            entry = {
                "kind": "module_get_attr",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if op.metadata is not None:
                effect_proof = op.metadata.get("effect_proof")
                if isinstance(effect_proof, str) and effect_proof:
                    entry["effect_proof"] = effect_proof
            ctx.json_ops.append(entry)
        elif op.kind == "MODULE_IMPORT_FROM":
            ctx.json_ops.append(
                {
                    "kind": "module_import_from",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_GET_GLOBAL":
            ctx.json_ops.append(
                {
                    "kind": "module_get_global",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_DEL_GLOBAL":
            ctx.json_ops.append(
                {
                    "kind": "module_del_global",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_DEL_GLOBAL_IF_PRESENT":
            ctx.json_ops.append(
                {
                    "kind": "module_del_global_if_present",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_SET_ATTR":
            ctx.json_ops.append(
                {
                    "kind": "module_set_attr",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MODULE_IMPORT_STAR":
            ctx.json_ops.append(
                {
                    "kind": "module_import_star",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONTEXT_NULL":
            ctx.json_ops.append(
                {
                    "kind": "context_null",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONTEXT_ENTER":
            ctx.json_ops.append(
                {
                    "kind": "context_enter",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONTEXT_EXIT":
            ctx.json_ops.append(
                {
                    "kind": "context_exit",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONTEXT_UNWIND":
            ctx.json_ops.append(
                {
                    "kind": "context_unwind",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONTEXT_DEPTH":
            ctx.json_ops.append({"kind": "context_depth", "out": op.result.name})
        elif op.kind == "CONTEXT_UNWIND_TO":
            ctx.json_ops.append(
                {
                    "kind": "context_unwind_to",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONTEXT_CLOSING":
            ctx.json_ops.append(
                {
                    "kind": "context_closing",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        else:
            return False
        return True
