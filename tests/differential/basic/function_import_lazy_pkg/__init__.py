"""Companion package for function_import_package_init.py (HF-557).

Every submodule import sits in a function body.
"""

import contextlib


def value():
    from .helper import compute

    return compute(5)


@contextlib.contextmanager
def scoped():
    from .helper import scope

    with scope() as got:
        yield got


def shadowed_json(value):
    from .json import dumps

    return dumps(value)
