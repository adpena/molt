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
    try:
        intrinsic = _require_intrinsic("molt_types_bootstrap", globals())
    except Exception as e:
        print(f"[molt-types-debug] _require_intrinsic failed: {e}", file=sys.stderr)
        raise
    try:
        data = intrinsic()
    except Exception as e:
        print(f"[molt-types-debug] intrinsic() call failed: {e}", file=sys.stderr)
        raise
    if not isinstance(data, dict):
        print(f"[molt-types-debug] intrinsic returned non-dict: {type(data)}", file=sys.stderr)
        raise RuntimeError("types intrinsics unavailable")
    print(f"[molt-types-debug] bootstrap OK, got {len(data)} entries: {list(data.keys())[:5]}...", file=sys.stderr)
    g = globals()
    print(f"[molt-types-debug] globals() id={id(g)} type={type(g).__name__} keys_before={len(g)}", file=sys.stderr)
    g.update(data)
    print(f"[molt-types-debug] keys_after={len(g)} ModuleType_in_globals={'ModuleType' in g}", file=sys.stderr)
    # Also try direct assignment as fallback
    if 'ModuleType' not in g:
        print("[molt-types-debug] ModuleType NOT in globals after update, trying setattr", file=sys.stderr)


_bootstrap()

import sys as _sys_check
_mod = _sys_check.modules.get(__name__)
if _mod is not None:
    print(f"[molt-types-debug] post-bootstrap: hasattr(mod, 'ModuleType')={hasattr(_mod, 'ModuleType')}", file=_sys_check.stderr)
    print(f"[molt-types-debug] mod.__dict__ is globals()={_mod.__dict__ is globals()}", file=_sys_check.stderr)
    if not hasattr(_mod, 'ModuleType'):
        print(f"[molt-types-debug] FIXING: injecting into mod.__dict__", file=_sys_check.stderr)
        _mod.__dict__.update(globals())
del _sys_check, _mod
