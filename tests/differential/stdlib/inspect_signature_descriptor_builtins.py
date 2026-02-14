"""Purpose: ensure descriptor builtin signatures are intrinsic-backed via text signatures."""

import builtins
import inspect


def _safe_signature(value: object) -> str:
    try:
        return str(inspect.signature(value))
    except BaseException as exc:  # noqa: BLE001
        return f"{type(exc).__name__}: {exc}"


for name, direct in (
    ("property", property),
    ("classmethod", classmethod),
    ("staticmethod", staticmethod),
):
    module_value = getattr(builtins, name)
    print(name)
    print(_safe_signature(direct))
    print(_safe_signature(module_value))
    print(direct is module_value)
