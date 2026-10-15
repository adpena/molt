"""Purpose: zero-argument super() follows a rebound `super` name (HF-216).

When `super` is not the builtin, CPython calls whatever the name holds with no
arguments and then loads the attribute. A module-level rebinding, a rebinding
of builtins.super at run time, and its restoration are checked.
"""

import builtins

real_super = super


class Base:
    def hello(self, value):
        return ("base", value)


class Child(Base):
    def hello(self, value):
        return ("child",) + super().hello(value)


child = Child()
print(child.hello(1))


class FakeProxy:
    def hello(self, value):
        return ("fake", value)


def fake_super(*args):
    return FakeProxy()


builtins.super = fake_super
try:
    print(child.hello(2))
finally:
    builtins.super = real_super
print(child.hello(3))
