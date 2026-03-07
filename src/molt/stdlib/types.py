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

# Runtime-backed parity notes:
# - class helper APIs (`get_original_bases`, `prepare_class`, `resolve_bases`, `new_class`)
#   lower to Rust intrinsics and raise on missing intrinsic support.
# - `DynamicClassAttribute` descriptor behavior (clone/getter/setter/deleter and
#   argument validation/error mapping) is implemented in the runtime descriptor path.
# Remaining stdlib `types` gaps are tracked in STATUS/ROADMAP.


def _bootstrap() -> None:
    import sys

    intrinsic = _require_intrinsic("molt_types_bootstrap", None)
    data = intrinsic()
    if not isinstance(data, dict):
        raise RuntimeError("types intrinsics unavailable")
    # In compiled Molt binaries, globals() may not return the module's __dict__.
    # Inject directly into the module object via sys.modules to ensure attributes
    # are visible to importers (e.g. `from types import ModuleType`).
    mod = sys.modules.get("types")
    if mod is not None:
        mod.__dict__.update(data)
    else:
        sys.modules[__name__].__dict__.update(data)


_bootstrap()
