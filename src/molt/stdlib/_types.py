"""Intrinsic-backed compatibility surface for CPython's `_types`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from types import (
    AsyncGeneratorType,
    BuiltinFunctionType,
    BuiltinMethodType,
    CapsuleType,
    CellType,
    ClassMethodDescriptorType,
    CodeType,
    CoroutineType,
    EllipsisType,
    FrameType,
    FunctionType,
    GeneratorType,
    GenericAlias,
    GetSetDescriptorType,
    LambdaType,
    MappingProxyType,
    MemberDescriptorType,
    MethodDescriptorType,
    MethodType,
    MethodWrapperType,
    ModuleType,
    NoneType,
    NotImplementedType,
    SimpleNamespace,
    TracebackType,
    UnionType,
    WrapperDescriptorType,
)

_MOLT_TYPES_BOOTSTRAP = _require_intrinsic("molt_types_bootstrap")

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
    "GenericAlias",
    "GetSetDescriptorType",
    "LambdaType",
    "MappingProxyType",
    "MemberDescriptorType",
    "MethodDescriptorType",
    "MethodType",
    "MethodWrapperType",
    "ModuleType",
    "NoneType",
    "NotImplementedType",
    "SimpleNamespace",
    "TracebackType",
    "UnionType",
    "WrapperDescriptorType",
]

del _MOLT_TYPES_BOOTSTRAP
