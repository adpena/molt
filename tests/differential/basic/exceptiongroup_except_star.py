"""Purpose: differential coverage for ExceptionGroup and except* behavior."""

try:
    raise ExceptionGroup("eg", [ValueError("a"), TypeError("b")])
except* ValueError as exc:
    print("value", [type(e).__name__ for e in exc.exceptions])
except* TypeError as exc:
    print("type", [type(e).__name__ for e in exc.exceptions])


# Handled outer state must never be collected as a newly raised subgroup error.
import sys

outer = RuntimeError("outer")
replacement = LookupError("replacement")
try:
    raise outer
except RuntimeError:
    try:
        try:
            raise ExceptionGroup("replace", [ValueError("a"), TypeError("b")])
        except* ValueError as matched:
            print("subgroup-active", sys.exception() is matched)
            raise replacement
        except* TypeError:
            print("subgroup-consumed", True)
    except LookupError as caught:
        print("subgroup-replacement", caught is replacement)
        print(
            "subgroup-context",
            type(caught.__context__).__name__,
            [type(e).__name__ for e in caught.__context__.exceptions],
        )
    print("subgroup-outer-restored", sys.exception() is outer)
    try:
        raise ExceptionGroup("all", [ValueError("done")])
    except* ValueError:
        print("subgroup-all-consumed", True)
    print("subgroup-no-spurious-error", sys.exception() is outer)
