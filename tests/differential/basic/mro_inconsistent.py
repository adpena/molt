"""Purpose: differential coverage for mro inconsistent."""


class X:
    pass


class Y:
    pass


class A(X, Y):
    pass


class B(Y, X):
    pass


try:

    class C(A, B):
        pass

    print("noerror")
except TypeError:
    print("typeerror")
