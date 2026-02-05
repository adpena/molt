"""Minimal _abc shim for Molt."""

from __future__ import annotations

# Intrinsic-only stdlib guard.
from _intrinsics import require_intrinsic as _require_intrinsic


_require_intrinsic("molt_stdlib_probe", globals())

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): tighten
# _abc parity once weakref/GC semantics are complete and ABC caching is validated.

from _weakrefset import WeakSet

_abc_invalidation_counter = 0


def _is_abstract(value):
    if getattr(value, "__isabstractmethod__", False):
        return True
    func = getattr(value, "__func__", None)
    if func is not None and getattr(func, "__isabstractmethod__", False):
        return True
    if isinstance(value, property):
        fget = getattr(value, "fget", None)
        fset = getattr(value, "fset", None)
        fdel = getattr(value, "fdel", None)
        for accessor in (fget, fset, fdel):
            if accessor is not None and getattr(
                accessor, "__isabstractmethod__", False
            ):
                return True
    return False


def _call_subclasshook(cls, subclass):
    hook = cls.__dict__.get("__subclasshook__")
    if hook is None:
        hook = type.__dict__.get("__subclasshook__")
    if hook is None:
        return NotImplemented

    def _try_call(func, *args):
        try:
            return func(*args)
        except TypeError as exc:
            if "call arity mismatch" in str(exc):
                return None
            raise

    if hasattr(hook, "__func__"):
        func = hook.__func__
        if func is not None:
            result = _try_call(func, cls, subclass)
            if result is not None:
                return result

    if hasattr(hook, "__get__"):
        try:
            bound = hook.__get__(None, cls)
        except TypeError as exc:
            if "call arity mismatch" not in str(exc):
                raise
            bound = None
        if callable(bound):
            result = _try_call(bound, subclass)
            if result is not None:
                return result

    if callable(hook):
        result = _try_call(hook, subclass)
        if result is not None:
            return result

    bound = getattr(cls, "__subclasshook__", None)
    if callable(bound):
        result = _try_call(bound, subclass)
        if result is not None:
            return result
        result = _try_call(bound, cls, subclass)
        if result is not None:
            return result

    return NotImplemented


def _get_subclasshook(cls):
    def _hook(subclass):
        return _call_subclasshook(cls, subclass)

    return _hook


def get_cache_token():
    return _abc_invalidation_counter


def _abc_init(cls):
    abstracts = {name for name, value in cls.__dict__.items() if _is_abstract(value)}
    for base in cls.__mro__[1:]:
        for name in getattr(base, "__abstractmethods__", set()):
            value = getattr(cls, name, None)
            if _is_abstract(value):
                abstracts.add(name)
    cls.__abstractmethods__ = frozenset(abstracts)
    cls._abc_registry = WeakSet()
    cls._abc_cache = WeakSet()
    cls._abc_negative_cache = WeakSet()
    cls._abc_negative_cache_version = _abc_invalidation_counter


def _abc_register(cls, subclass):
    global _abc_invalidation_counter
    if not isinstance(subclass, type):
        raise TypeError("Can only register classes")
    if _safe_issubclass(subclass, cls):
        return subclass
    if _safe_issubclass(cls, subclass):
        raise RuntimeError("Refusing to create an inheritance cycle")
    WeakSet.add(cls._abc_registry, subclass)
    _abc_invalidation_counter += 1
    return subclass


def _abc_instancecheck(cls, instance):
    subtype = type(instance)
    if subtype in cls._abc_cache:
        return True
    if (
        cls._abc_negative_cache_version == _abc_invalidation_counter
        and subtype in cls._abc_negative_cache
    ):
        return False
    return cls.__subclasscheck__(subtype)


def _abc_subclasscheck(cls, subclass):
    if not isinstance(subclass, type):
        raise TypeError("issubclass() arg 1 must be a class")
    if subclass in cls._abc_cache:
        return True
    if cls._abc_negative_cache_version < _abc_invalidation_counter:
        cls._abc_negative_cache = WeakSet()
        cls._abc_negative_cache_version = _abc_invalidation_counter
    elif subclass in cls._abc_negative_cache:
        return False
    ok = _call_subclasshook(cls, subclass)
    if ok is not NotImplemented:
        if ok:
            WeakSet.add(cls._abc_cache, subclass)
        else:
            WeakSet.add(cls._abc_negative_cache, subclass)
        return bool(ok)
    try:
        if cls in _safe_mro(subclass):
            WeakSet.add(cls._abc_cache, subclass)
            return True
    except TypeError as exc:
        if "call arity mismatch" not in str(exc):
            raise
    for rcls in cls._abc_registry:
        if _safe_issubclass(subclass, rcls):
            WeakSet.add(cls._abc_cache, subclass)
            return True
    subclasses_func = getattr(type, "__subclasses__", None)
    if callable(subclasses_func):
        try:
            subclasses = subclasses_func(cls)
        except TypeError as exc:
            if "call arity mismatch" not in str(exc):
                raise
            subclasses = ()
    else:
        subclasses = ()
    for scls in subclasses:
        if _safe_issubclass(subclass, scls):
            WeakSet.add(cls._abc_cache, subclass)
            return True
    WeakSet.add(cls._abc_negative_cache, subclass)
    return False


def _safe_issubclass(subclass, cls):
    try:
        return issubclass(subclass, cls)
    except TypeError as exc:
        if "call arity mismatch" not in str(exc):
            raise
        return _mro_contains(_safe_mro(subclass), cls)


def _safe_mro(cls):
    try:
        mro = type.mro(cls)
    except TypeError as exc:
        if "call arity mismatch" in str(exc):
            return ()
        raise
    except Exception:
        try:
            mro = cls.__mro__
        except Exception:
            return ()
    if isinstance(mro, tuple):
        return mro
    if isinstance(mro, list):
        return tuple(mro)
    return ()


def _mro_contains(mro, needle):
    for base in mro:
        if base is needle:
            return True
    return False


def _get_dump(cls):
    return (
        cls._abc_registry,
        cls._abc_cache,
        cls._abc_negative_cache,
        cls._abc_negative_cache_version,
    )


def _reset_registry(cls):
    WeakSet.clear(cls._abc_registry)


def _reset_caches(cls):
    WeakSet.clear(cls._abc_cache)
    WeakSet.clear(cls._abc_negative_cache)
