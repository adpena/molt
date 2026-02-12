"""Purpose: differential coverage for del name."""
# ruff: noqa: F821


def show_err(label: str, func) -> None:
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


x = 1
del x
try:
    print("module_after_del", x)
except Exception as exc:
    print("module_after_del", type(exc).__name__, exc)

try:
    del missing_module
except Exception as exc:
    print("module_del_missing", type(exc).__name__, exc)


def local_del() -> None:
    x = 1
    del x
    try:
        _ = x
    except Exception as exc:
        print("local_after_del", type(exc).__name__, exc)


def local_del_missing() -> None:
    try:
        del y
    except Exception as exc:
        print("local_del_missing", type(exc).__name__, exc)


def nonlocal_del() -> None:
    x = 1

    def inner() -> None:
        nonlocal x
        del x
        try:
            _ = x
        except Exception as exc:
            print("nonlocal_after_del", type(exc).__name__, exc)

    inner()
    try:
        _ = x
    except Exception as exc:
        print("outer_after_nonlocal_del", type(exc).__name__, exc)


local_del()
local_del_missing()
nonlocal_del()
