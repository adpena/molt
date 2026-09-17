"""Types helpers for Molt.

Types are provided by runtime intrinsics; missing intrinsics are a hard error.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# Bind types eagerly following CPython's types.py pattern.
# The runtime intrinsic system exports these as module attributes,
# but annotations alone don't create bindings.  Use require_intrinsic
# for types backed by runtime objects, and direct Python construction
# for types derivable from existing objects.
import sys as _sys


def _f():
    yield  # helper to get GeneratorType


_g = _f()

ModuleType = type(_sys)
NoneType = type(None)
FunctionType = type(lambda: None)
LambdaType = FunctionType
GeneratorType = type(_g)
CodeType = type(_f.__code__) if hasattr(_f, "__code__") else type
BuiltinFunctionType = type(len)
BuiltinMethodType = type([].append)
MethodType = type  # placeholder — bound methods don't have a stable type() in Molt

# Types bound by the runtime intrinsic system.  If not yet available,
# fall back to `type` so the module can still be imported.
MappingProxyType = type(
    type.__dict__
)  # CPython: MappingProxyType = type(type.__dict__)
SimpleNamespace = type  # placeholder until intrinsic wires it
GenericAlias = type  # placeholder until intrinsic wires it

# Types that may not be available yet — fallback to `type` sentinel.
AsyncGeneratorType = type
CapsuleType = type
CellType = type
ClassMethodDescriptorType = type
CoroutineType = type
EllipsisType = type(...)
FrameType = type
GetSetDescriptorType = type
MemberDescriptorType = type
MethodDescriptorType = type
MethodWrapperType = type
NotImplementedType = type(NotImplemented)
TracebackType = type
UnionType = type
WrapperDescriptorType = type
DynamicClassAttribute = type

del _f, _g

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


def _bootstrap() -> dict:
    intrinsic = _require_intrinsic("molt_types_bootstrap")
    data = intrinsic()
    if not isinstance(data, dict):
        raise RuntimeError("types intrinsics unavailable")
    # Use module-level assignment instead of globals().update() to avoid
    # the compiled dict lookup bug. Each assignment goes through MODULE_SET_ATTR.
    import sys

    mod = sys.modules[__name__]
    for key, val in data.items():
        if key == "coroutine":
            continue
        setattr(mod, key, val)
    return data


_coroutine_intrinsic = _bootstrap()["coroutine"]


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
