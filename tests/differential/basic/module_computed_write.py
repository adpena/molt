"""An indirect module receiver and a computed name can replace a compiled callee."""

# ruff: noqa: E402

from builtins import setattr as put
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

import module_static_cells_mod as api


def replacement() -> int:
    return 7


def change(owner: object) -> None:
    name = sys.argv[1] if len(sys.argv) > 1 else "read_value"
    put(owner, name, replacement)


def relay() -> None:
    change(sys.modules[api.__name__])


print(api.read_value())
relay()
print(api.read_value())
