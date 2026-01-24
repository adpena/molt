"""Purpose: differential coverage for super bound method."""


class A:
    def who(self) -> str:
        return "A"


class C(A):
    pass


obj = C()
fn = super(C, obj).who
print(fn())
