"""Types helpers for Molt.

Types are provided by runtime intrinsics; missing intrinsics are a hard error.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# The runtime owns every type and class helper; a missing payload is a hard
# error. Each name binds once, so the module namespace stays statically known.
_PAYLOAD = _require_intrinsic("molt_types_bootstrap")()
if not isinstance(_PAYLOAD, dict):
    raise RuntimeError("types intrinsics unavailable")
AsyncGeneratorType = _PAYLOAD["AsyncGeneratorType"]
BuiltinFunctionType = _PAYLOAD["BuiltinFunctionType"]
BuiltinMethodType = _PAYLOAD["BuiltinMethodType"]
CapsuleType = _PAYLOAD["CapsuleType"]
CellType = _PAYLOAD["CellType"]
ClassMethodDescriptorType = _PAYLOAD["ClassMethodDescriptorType"]
CodeType = _PAYLOAD["CodeType"]
CoroutineType = _PAYLOAD["CoroutineType"]
DynamicClassAttribute = _PAYLOAD["DynamicClassAttribute"]
EllipsisType = _PAYLOAD["EllipsisType"]
FrameType = _PAYLOAD["FrameType"]
FunctionType = _PAYLOAD["FunctionType"]
GeneratorType = _PAYLOAD["GeneratorType"]
GenericAlias = _PAYLOAD["GenericAlias"]
GetSetDescriptorType = _PAYLOAD["GetSetDescriptorType"]
LambdaType = _PAYLOAD["LambdaType"]
MappingProxyType = _PAYLOAD["MappingProxyType"]
MemberDescriptorType = _PAYLOAD["MemberDescriptorType"]
MethodDescriptorType = _PAYLOAD["MethodDescriptorType"]
MethodType = _PAYLOAD["MethodType"]
MethodWrapperType = _PAYLOAD["MethodWrapperType"]
ModuleType = _PAYLOAD["ModuleType"]
NoneType = _PAYLOAD["NoneType"]
NotImplementedType = _PAYLOAD["NotImplementedType"]
SimpleNamespace = _PAYLOAD["SimpleNamespace"]
TracebackType = _PAYLOAD["TracebackType"]
UnionType = _PAYLOAD["UnionType"]
WrapperDescriptorType = _PAYLOAD["WrapperDescriptorType"]
get_original_bases = _PAYLOAD["get_original_bases"]
new_class = _PAYLOAD["new_class"]
prepare_class = _PAYLOAD["prepare_class"]
resolve_bases = _PAYLOAD["resolve_bases"]
_coroutine_intrinsic = _PAYLOAD["coroutine"]
del _PAYLOAD

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


class _GeneratorWrapper:
    """Awaitable adapter for generator-like results from general callables."""

    def __init__(self, gen):
        self.__wrapped = gen
        self.__isgen = gen.__class__ is GeneratorType
        self.__name__ = getattr(gen, "__name__", None)
        self.__qualname__ = getattr(gen, "__qualname__", None)

    def send(self, val):
        return self.__wrapped.send(val)

    def throw(self, *args):
        return self.__wrapped.throw(*args)

    def close(self):
        return self.__wrapped.close()

    @property
    def gi_code(self):
        return self.__wrapped.gi_code

    @property
    def gi_frame(self):
        return self.__wrapped.gi_frame

    @property
    def gi_running(self):
        return self.__wrapped.gi_running

    @property
    def gi_yieldfrom(self):
        return self.__wrapped.gi_yieldfrom

    cr_code = gi_code
    cr_frame = gi_frame
    cr_running = gi_running
    cr_await = gi_yieldfrom

    def __next__(self):
        return next(self.__wrapped)

    def __iter__(self):
        if self.__isgen:
            return self.__wrapped
        return self

    __await__ = __iter__


def coroutine(func):
    """Convert a generator function, or adapt a general callable, to awaitable form."""

    if not callable(func):
        raise TypeError("types.coroutine() expects a callable")

    if (
        func.__class__ is FunctionType
        and getattr(func, "__code__", None).__class__ is CodeType
    ):
        flags = func.__code__.co_flags
        if flags & 0x180:
            return func
        if flags & 0x20:
            return _coroutine_intrinsic(func)

    import _collections_abc
    import functools

    @functools.wraps(func)
    def wrapped(*args, **kwargs):
        result = func(*args, **kwargs)
        if result.__class__ is CoroutineType:
            return result
        if result.__class__ is GeneratorType:
            code = getattr(result, "gi_code", None)
            if code is not None and code.co_flags & 0x100:
                return result
        if isinstance(result, _collections_abc.Generator) and not isinstance(
            result, _collections_abc.Coroutine
        ):
            return _GeneratorWrapper(result)
        return result

    return wrapped
