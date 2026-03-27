"""Purpose: verify dotted-name import failures preserve their message shape."""

import importlib


try:
    importlib.import_module("importlib.no_such_child")
except BaseException as exc:  # noqa: BLE001
    print(type(exc).__name__)
    print(str(exc).startswith("No module named "))
    print("importlib.no_such_child" in str(exc))
else:
    print("NO_ERROR")
