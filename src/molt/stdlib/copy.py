"""Shallow/deep copy helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_require_intrinsic("molt_stdlib_probe", globals())

from typing import Any, Callable, Iterable

try:
    import copyreg as _copyreg
except Exception:
    _copyreg = None

__all__ = ["copy", "deepcopy", "Error", "dispatch_table"]


class Error(Exception):
    pass


if _copyreg is not None:
    dispatch_table: dict[type, Callable[[Any], Any]] = _copyreg.dispatch_table
else:
    dispatch_table = {}

_copy_dispatch: dict[type, Callable[[Any], Any]] = {}
_deepcopy_dispatch: dict[type, Callable[[Any, dict[int, Any]], Any]] = {}

_atomic_types = (
    type(None),
    bool,
    int,
    float,
    complex,
    str,
    bytes,
    range,
    type,
    type(NotImplemented),
    type(Ellipsis),
)


def _copy_atomic(obj: Any) -> Any:
    return obj


def _deepcopy_atomic(obj: Any, memo: dict[int, Any]) -> Any:
    return obj


def _iter_slots(cls: type) -> Iterable[str]:
    seen: set[str] = set()
    for base in getattr(cls, "__mro__", (cls,)):
        slots = getattr(base, "__slots__", ())
        if isinstance(slots, str):
            slots = (slots,)
        for name in slots:
            if name in ("__dict__", "__weakref__"):
                continue
            if name in seen:
                continue
            seen.add(name)
            yield name


def _copy_molt_fields(
    obj: Any, result: Any, memo: dict[int, Any] | None = None
) -> bool:
    offsets = getattr(obj.__class__, "__molt_field_offsets__", None)
    if not isinstance(offsets, dict):
        return True
    for name in offsets:
        try:
            value = getattr(obj, name)
        except AttributeError:
            continue
        if memo is not None:
            value = deepcopy(value, memo)
        setattr(result, name, value)
    return True


def _has_custom_reduce(obj_type: type) -> bool:
    base_reduce_ex = getattr(object, "__reduce_ex__", None)
    base_reduce = getattr(object, "__reduce__", None)
    reduce_ex = getattr(obj_type, "__reduce_ex__", None)
    if reduce_ex is not None and reduce_ex is not base_reduce_ex:
        return True
    reduce = getattr(obj_type, "__reduce__", None)
    if reduce is not None and reduce is not base_reduce:
        return True
    return False


def _copy_slots(obj: Any, result: Any, memo: dict[int, Any] | None) -> None:
    for name in _iter_slots(obj.__class__):
        try:
            value = getattr(obj, name)
        except AttributeError:
            continue
        if memo is not None:
            value = deepcopy(value, memo)
        try:
            setattr(result, name, value)
        except Exception:
            continue


def _copy_list(obj: list[Any]) -> list[Any]:
    return list(obj)


def _deepcopy_list(obj: list[Any], memo: dict[int, Any]) -> list[Any]:
    result: list[Any] = []
    memo[id(obj)] = result
    for item in obj:
        result.append(deepcopy(item, memo))
    return result


def _copy_dict(obj: dict[Any, Any]) -> dict[Any, Any]:
    return dict(obj)


def _deepcopy_dict(obj: dict[Any, Any], memo: dict[int, Any]) -> dict[Any, Any]:
    result: dict[Any, Any] = {}
    memo[id(obj)] = result
    for key, value in obj.items():
        result[deepcopy(key, memo)] = deepcopy(value, memo)
    return result


def _copy_set(obj: set[Any]) -> set[Any]:
    return set(obj)


def _deepcopy_set(obj: set[Any], memo: dict[int, Any]) -> set[Any]:
    result: set[Any] = set()
    memo[id(obj)] = result
    for item in obj:
        result.add(deepcopy(item, memo))
    return result


def _copy_frozenset(obj: frozenset[Any]) -> frozenset[Any]:
    return frozenset(obj)


def _deepcopy_frozenset(obj: frozenset[Any], memo: dict[int, Any]) -> frozenset[Any]:
    return frozenset(deepcopy(item, memo) for item in obj)


def _copy_tuple(obj: tuple[Any, ...]) -> tuple[Any, ...]:
    return tuple(obj)


def _deepcopy_tuple(obj: tuple[Any, ...], memo: dict[int, Any]) -> tuple[Any, ...]:
    if not obj:
        return obj
    items = [deepcopy(item, memo) for item in obj]
    if all(new is old for new, old in zip(items, obj)):
        return obj
    return tuple(items)


def _copy_bytearray(obj: bytearray) -> bytearray:
    return bytearray(obj)


def _deepcopy_bytearray(obj: bytearray, memo: dict[int, Any]) -> bytearray:
    result = bytearray(obj)
    memo[id(obj)] = result
    return result


def _normalize_reduce(rv: Any) -> tuple[Any, Any, Any, Any, Any]:
    if isinstance(rv, str):
        raise Error("reduction returned string")
    if not isinstance(rv, tuple):
        raise Error("reduction did not return a tuple")
    if len(rv) < 2:
        raise Error("reduction must return at least 2 items")
    if len(rv) > 5:
        raise Error("reduction returned too many items")
    func = rv[0]
    args = rv[1]
    state = rv[2] if len(rv) > 2 else None
    listiter = rv[3] if len(rv) > 3 else None
    dictiter = rv[4] if len(rv) > 4 else None
    return func, args, state, listiter, dictiter


def _reduce_ex(obj: Any, protocol: int) -> tuple[Any, Any, Any, Any, Any]:
    reducer = getattr(obj, "__reduce_ex__", None)
    if callable(reducer):
        rv = reducer(protocol)
    else:
        reducer = getattr(obj, "__reduce__", None)
        if callable(reducer):
            rv = reducer()
        else:
            raise Error("unreduceable object")
    return _normalize_reduce(rv)


def _reconstruct(
    obj: Any,
    memo: dict[int, Any] | None,
    func: Any,
    args: Any,
    state: Any,
    listiter: Any,
    dictiter: Any,
    deep: bool,
) -> Any:
    if not isinstance(args, tuple):
        try:
            args = tuple(args)
        except Exception as exc:
            raise Error("reduction args must be a tuple") from exc
    if deep:
        args = tuple(deepcopy(item, memo) for item in args)
    result = func(*args)
    if memo is not None:
        memo[id(obj)] = result
    if state is not None:
        if deep:
            state = deepcopy(state, memo)
        setstate = getattr(result, "__setstate__", None)
        if callable(setstate):
            setstate(state)
        else:
            if (
                isinstance(state, tuple)
                and len(state) == 2
                and isinstance(state[0], dict)
            ):
                dict_state, slot_state = state
            else:
                dict_state, slot_state = state, None
            if dict_state:
                try:
                    result.__dict__.update(dict_state)
                except Exception:
                    pass
            if slot_state:
                try:
                    for key, value in slot_state.items():
                        setattr(result, key, value)
                except Exception:
                    pass
    if listiter is not None:
        if deep:
            listiter = (deepcopy(item, memo) for item in listiter)
        append = getattr(result, "append", None)
        extend = getattr(result, "extend", None)
        add = getattr(result, "add", None)
        if callable(append):
            for item in listiter:
                append(item)
        elif callable(extend):
            extend(listiter)
        elif callable(add):
            for item in listiter:
                add(item)
    if dictiter is not None:
        if deep:
            dictiter = (
                (deepcopy(key, memo), deepcopy(value, memo)) for key, value in dictiter
            )
        for key, value in dictiter:
            result[key] = value
    return result


def _copy_reduce(
    obj: Any,
    memo: dict[int, Any] | None,
    deep: bool,
    reducer: Callable[[Any], Any] | None = None,
) -> Any:
    if reducer is None:
        func, args, state, listiter, dictiter = _reduce_ex(obj, protocol=4)
    else:
        func, args, state, listiter, dictiter = _normalize_reduce(reducer(obj))
    return _reconstruct(obj, memo, func, args, state, listiter, dictiter, deep)


def _copy_instance(obj: Any, memo: dict[int, Any] | None, deep: bool) -> Any:
    cls = obj.__class__
    try:
        result = cls.__new__(cls)
    except Exception:
        return obj
    if memo is not None:
        memo[id(obj)] = result
    try:
        _copy_molt_fields(obj, result, memo if deep else None)
        if hasattr(obj, "__dict__"):
            if deep:
                for key, value in obj.__dict__.items():
                    result.__dict__[key] = deepcopy(value, memo)
            else:
                result.__dict__.update(obj.__dict__)
        _copy_slots(obj, result, memo if deep else None)
    except Exception:
        return result if deep else obj
    return result


_copy_dispatch[list] = _copy_list
_copy_dispatch[dict] = _copy_dict
_copy_dispatch[set] = _copy_set
_copy_dispatch[tuple] = _copy_tuple
_copy_dispatch[bytearray] = _copy_bytearray
_copy_dispatch[frozenset] = _copy_frozenset
_copy_dispatch[slice] = _copy_atomic

_deepcopy_dispatch[list] = _deepcopy_list
_deepcopy_dispatch[dict] = _deepcopy_dict
_deepcopy_dispatch[set] = _deepcopy_set
_deepcopy_dispatch[tuple] = _deepcopy_tuple
_deepcopy_dispatch[bytearray] = _deepcopy_bytearray
_deepcopy_dispatch[frozenset] = _deepcopy_frozenset
_deepcopy_dispatch[slice] = _deepcopy_atomic

for _atomic in _atomic_types:
    _copy_dispatch[_atomic] = _copy_atomic
    _deepcopy_dispatch[_atomic] = _deepcopy_atomic


def copy(obj: Any) -> Any:
    obj_type = type(obj)
    memo: dict[int, Any] | None = {}
    copier = _copy_dispatch.get(obj_type)
    if copier is not None:
        return copier(obj)
    reducer = dispatch_table.get(obj_type)
    if reducer is not None:
        return _copy_reduce(obj, memo, False, reducer)
    copier = getattr(obj, "__copy__", None)
    if callable(copier):
        return copier()
    has_attrs = hasattr(obj, "__dict__") or list(_iter_slots(obj_type))
    if has_attrs and not _has_custom_reduce(obj_type):
        return _copy_instance(obj, memo, False)
    try:
        return _copy_reduce(obj, memo, False)
    except Error:
        pass
    if has_attrs:
        return _copy_instance(obj, memo, False)
    return obj


def deepcopy(obj: Any, memo: dict[int, Any] | None = None) -> Any:
    if memo is None:
        memo = {}
    obj_id = id(obj)
    if obj_id in memo:
        return memo[obj_id]
    obj_type = type(obj)
    copier = _deepcopy_dispatch.get(obj_type)
    if copier is not None:
        result = copier(obj, memo)
        memo[obj_id] = result
        return result
    reducer = dispatch_table.get(obj_type)
    if reducer is not None:
        result = _copy_reduce(obj, memo, True, reducer)
        memo[obj_id] = result
        return result
    copier = getattr(obj, "__deepcopy__", None)
    if callable(copier):
        result = copier(memo)
        memo[obj_id] = result
        return result
    has_attrs = hasattr(obj, "__dict__") or list(_iter_slots(obj_type))
    if has_attrs and not _has_custom_reduce(obj_type):
        result = _copy_instance(obj, memo, True)
        memo[obj_id] = result
        return result
    try:
        result = _copy_reduce(obj, memo, True)
        memo[obj_id] = result
        return result
    except Error:
        pass
    if has_attrs:
        result = _copy_instance(obj, memo, True)
        memo[obj_id] = result
        return result
    memo[obj_id] = obj
    return obj
