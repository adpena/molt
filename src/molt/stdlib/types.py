"""Types helpers for Molt.

Types are provided by runtime intrinsics; missing intrinsics are a hard error.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# Intrinsics populate these at import time; predeclare for static checkers.
AsyncGeneratorType: type
BuiltinFunctionType: type
BuiltinMethodType: type
CapsuleType: type
CellType: type
ClassMethodDescriptorType: type
CodeType: type
CoroutineType: type
EllipsisType: type
FrameType: type
FunctionType: type
GeneratorType: type
MappingProxyType: type
MethodType: type
MethodDescriptorType: type
MethodWrapperType: type
ModuleType: type
NoneType: type
NotImplementedType: type
GenericAlias: type
GetSetDescriptorType: type
LambdaType: type
MemberDescriptorType: type
SimpleNamespace: type
TracebackType: type
UnionType: type
WrapperDescriptorType: type
DynamicClassAttribute: type
coroutine: object
get_original_bases: object
new_class: object
prepare_class: object
resolve_bases: object

__all__ = [
    "AsyncGeneratorType",
    "BuiltinFunctionType",
    "BuiltinMethodType",
    "CapsuleType",
    "CellType",
    "ClassMethodDescriptorType",
    "CodeType",
    "CoroutineType",
    "EllipsisType",
    "FrameType",
    "FunctionType",
    "GeneratorType",
    "MappingProxyType",
    "MethodType",
    "MethodDescriptorType",
    "MethodWrapperType",
    "ModuleType",
    "NoneType",
    "NotImplementedType",
    "GenericAlias",
    "GetSetDescriptorType",
    "LambdaType",
    "MemberDescriptorType",
    "SimpleNamespace",
    "TracebackType",
    "UnionType",
    "WrapperDescriptorType",
    "DynamicClassAttribute",
    "coroutine",
    "get_original_bases",
    "new_class",
    "prepare_class",
    "resolve_bases",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish
# types helpers parity (descriptor classes, class helpers).


def _bootstrap() -> None:
    intrinsic = _require_intrinsic("molt_types_bootstrap", globals())
    data = intrinsic()
    if not isinstance(data, dict):
        raise RuntimeError("types intrinsics unavailable")
    globals().update(data)


_bootstrap()
