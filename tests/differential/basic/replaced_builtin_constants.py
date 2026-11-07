"""Replacing each builtin among the constant lowerings reaches every read of it
(HF-776).

The frontend lowers a proved read of these builtins by name: `NotImplemented`,
`Ellipsis`.
For each one in turn, a module binding, a `builtins` member write and a
`__builtins__` mapping select another object, so every read below must reach
the replacement, as in CPython. The mapping form comes last: a function takes
its builtins from the mapping when it is created.
"""

import builtins

ORIGINALS = {name: getattr(builtins, name) for name in ('NotImplemented', 'Ellipsis')}


class Fake:
    """A replacement whose value prints deterministically."""

    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return "fake-" + self.name


def use_notimplemented():
    return NotImplemented


print("NotImplemented original", use_notimplemented(), NotImplemented)
NotImplemented = Fake("NotImplemented-module")
print("NotImplemented module binding", use_notimplemented(), NotImplemented)
del NotImplemented
print("NotImplemented module binding deleted", use_notimplemented(), NotImplemented)
builtins.NotImplemented = Fake("NotImplemented-member")
print("NotImplemented member", use_notimplemented(), NotImplemented)
builtins.NotImplemented = ORIGINALS["NotImplemented"]
print("NotImplemented member restored", use_notimplemented(), NotImplemented)


def use_ellipsis():
    return Ellipsis


print("Ellipsis original", use_ellipsis(), Ellipsis)
Ellipsis = Fake("Ellipsis-module")
print("Ellipsis module binding", use_ellipsis(), Ellipsis)
del Ellipsis
print("Ellipsis module binding deleted", use_ellipsis(), Ellipsis)
builtins.Ellipsis = Fake("Ellipsis-member")
print("Ellipsis member", use_ellipsis(), Ellipsis)
builtins.Ellipsis = ORIGINALS["Ellipsis"]
print("Ellipsis member restored", use_ellipsis(), Ellipsis)

custom = dict(builtins.__dict__)
custom["NotImplemented"] = Fake("NotImplemented-mapping")
custom["Ellipsis"] = Fake("Ellipsis-mapping")
__builtins__ = custom


def after_notimplemented():
    return NotImplemented


print("NotImplemented mapping", use_notimplemented(), after_notimplemented())


def after_ellipsis():
    return Ellipsis


print("Ellipsis mapping", use_ellipsis(), after_ellipsis())
