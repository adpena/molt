"""Purpose: differential coverage for super() resolution and MRO order."""

class A:
    def f(self):
        return "A"


class B(A):
    def f(self):
        return "B" + super().f()


class C(A):
    def f(self):
        return "C" + super().f()


class D(B, C):
    def f(self):
        return "D" + super().f()


if __name__ == "__main__":
    print("mro", [cls.__name__ for cls in D.mro()])
    print("call", D().f())
