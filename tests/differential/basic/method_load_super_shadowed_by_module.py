"""Purpose: a module that binds `super` keeps CPython's dynamic super() call (HF-216).

The module rebinds `super`, so the method's `super()` calls the module global
with no arguments; the method load runs on whatever object it returns.
"""


class Proxy:
    def hello(self, value):
        return ("proxy", value)


def super():
    return Proxy()


class Base:
    def hello(self, value):
        return ("base", value)


class Child(Base):
    def hello(self, value):
        return ("child",) + super().hello(value)


print(Child().hello(1))
