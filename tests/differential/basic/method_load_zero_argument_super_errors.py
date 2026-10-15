"""Purpose: zero-argument super() method loads raise CPython 3.12's errors (HF-216).

The method load reads the first argument and the __class__ cell like
LOAD_FAST and LOAD_DEREF: a deleted first argument raises UnboundLocalError and
an empty __class__ cell raises NameError, before the first argument's
error when both are unbound. Without a __class__ cell or a
positional parameter the generic super() call raises RuntimeError. A first
argument captured by a closure still works.
"""


class A:
    def f(self):
        return 1


class Deleted(A):
    def f(self):
        del self
        return super().f()


try:
    Deleted().f()
except UnboundLocalError as error:
    print("UnboundLocalError", error)


class EarlyCall(A):
    def f(self):
        return super().f()

    try:
        f(None)
    except NameError as error:
        print("NameError", error)


class DeletedEarly(A):
    def f(self):
        del self
        return super().f()

    try:
        f(None)
    except (NameError, UnboundLocalError) as error:
        print(type(error).__name__, error)


def outside(self):
    return super().f()


try:
    outside(1)
except RuntimeError as error:
    print("RuntimeError", error)


class Starred(A):
    def f(*args):
        return super().f()


try:
    Starred().f()
except RuntimeError as error:
    print("RuntimeError", error)


class Captured(A):
    def f(self):
        def read():
            return self

        return super().f() + (read() is self)


print(Captured().f())
