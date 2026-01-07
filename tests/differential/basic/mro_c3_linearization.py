class Root:
    def f(self) -> str:
        return "O"


class A(Root):
    def f(self) -> str:
        return "A" + super().f()


class B(Root):
    def f(self) -> str:
        return "B" + super().f()


class C(Root):
    def f(self) -> str:
        return "C" + super().f()


class D(A, B):
    def f(self) -> str:
        return "D" + super().f()


class E(B, C):
    def f(self) -> str:
        return "E" + super().f()


class F(D, E):
    def f(self) -> str:
        return "F" + super().f()


print(F().f())
