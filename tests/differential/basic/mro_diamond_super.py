"""Purpose: differential coverage for mro diamond super."""


class A:
    def who(self) -> str:
        return "A"


class B(A):
    def who(self) -> str:
        return "B" + super().who()


class C(A):
    def who(self) -> str:
        return "C" + super().who()


class D(B, C):
    def who(self) -> str:
        return "D" + super().who()


print(D().who())
print(super(B, D()).who())


class Base:
    value = 10


class Child(Base):
    value = 20

    def get(self) -> int:
        return super().value


print(Child().get())


class X:
    tag = "X"


class Y(X):
    tag = "Y"


class Z(X):
    tag = "Z"


class W(Y, Z):
    pass


print(W().tag)
print(W.tag)
