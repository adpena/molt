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
