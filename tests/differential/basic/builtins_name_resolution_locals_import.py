"""Purpose: differential coverage for builtin name resolution (locals/globals/__import__)."""

import builtins


def snapshot_semantics() -> tuple[bool, bool]:
    d = locals()
    after = 1
    # CPython: locals() returns a snapshot dict for function frames; it should not
    # acquire later locals by mutation.
    return ("after" in d, "after" in locals())


def main() -> list[object]:
    return [
        locals is builtins.locals,
        globals is builtins.globals,
        __import__ is builtins.__import__,
        __import__("builtins") is builtins,
        snapshot_semantics(),
    ]


print(main())
