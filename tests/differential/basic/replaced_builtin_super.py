"""Replacing the builtin `super` reaches zero-argument calls of it (HF-776).

CPython's zero-argument `super()` loads the global `super` and calls it; a
replacement receives no arguments and its result is used as is.
"""

import builtins

ORIGINAL = builtins.super


class Fake:
    def __init__(self, name):
        self.name = name

    def __call__(self, *args):
        return "called-" + self.name + "-" + str(len(args))


class Base:
    def who(self):
        return "base"


class Child(Base):
    def bare(self):
        return super()

    def method(self):
        return super().who()


def show(value):
    return value if value.__class__ is str else value.__class__.__name__


print("original", show(Child().bare()), Child().method())
super = Fake("super-module")
print("module binding", show(Child().bare()))
del super
print("module binding deleted", show(Child().bare()), Child().method())
builtins.super = Fake("super-member")
print("member", show(Child().bare()))
builtins.super = ORIGINAL
print("member restored", show(Child().bare()), Child().method())
custom = dict(builtins.__dict__)
custom["super"] = Fake("super-mapping")
__builtins__ = custom


class Later(Base):
    def bare(self):
        return super()


print("mapping", show(Child().bare()), show(Later().bare()))
