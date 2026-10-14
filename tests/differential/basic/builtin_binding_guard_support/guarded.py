"""Guarded reads of len, int and isinstance for the shadowing matrix.

Nothing in this module binds len, int or isinstance, so Molt compiles each
read below behind a builtin binding guard (HF-79).
``tests/test_frontend_builtin_binding_guards.py`` pins that. ``bind`` and
``unbind`` mutate this module's namespace through ``globals()`` with a key the
compiler cannot see.
"""


def size(value):
    return len(value)


def to_int(text):
    return int(text)


def is_int(value):
    return isinstance(value, int)


def bind(name, value):
    globals()[name] = value


def unbind(name):
    del globals()[name]
