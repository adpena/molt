"""Purpose: verify importlib.import_module relative-package TypeError parity."""

import importlib


math_mod = importlib.import_module("math", 1)
print("abs-nonstr-package", math_mod.__name__)


for label, package in (
    ("rel-int", 1),
    ("rel-bytes", b"pkg"),
    ("rel-none", None),
):
    try:
        importlib.import_module(".x", package)
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "NO_ERROR")
