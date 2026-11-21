"""Canonical callable execution kinds shared by source analysis and codegen."""

from enum import StrEnum


class FunctionKind(StrEnum):
    SYNC = "sync"
    ASYNC = "async"
    GENERATOR = "gen"
    ASYNC_GENERATOR = "asyncgen"


FUNCTION_KIND_VALUES = frozenset(kind.value for kind in FunctionKind)
STATEFUL_FUNCTION_KINDS = frozenset(
    {FunctionKind.ASYNC, FunctionKind.GENERATOR, FunctionKind.ASYNC_GENERATOR}
)


def normalize_function_kind(kind: object) -> FunctionKind | None:
    if isinstance(kind, FunctionKind):
        return kind
    if isinstance(kind, str) and kind in FUNCTION_KIND_VALUES:
        return FunctionKind(kind)
    return None


# The names whose plain function `type.__new__` wraps when the class statement
# runs (CPython `Objects/typeobject.c`, `type_new_set_attrs`): a classmethod for
# these two, a staticmethod for `__new__`. A selection of such a name never
# passes the instance as the function's first argument.
IMPLICIT_CLASSMETHOD_NAMES = frozenset({"__init_subclass__", "__class_getitem__"})
IMPLICIT_STATICMETHOD_NAMES = frozenset({"__new__"})
IMPLICIT_DESCRIPTOR_NAMES = IMPLICIT_CLASSMETHOD_NAMES | IMPLICIT_STATICMETHOD_NAMES
