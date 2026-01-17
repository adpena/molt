# ruff: noqa: F821


def show_err(label: str, func) -> None:
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


try:
    print("compare_module", x is None)
except Exception as exc:
    print("compare_module", type(exc).__name__, exc)


def compare_func() -> bool:
    return x is None


show_err("compare_func", compare_func)
