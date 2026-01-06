class A:
    def who(self) -> str:
        return "A"


class B:
    def who(self) -> str:
        return "B"

    def touch(self) -> str:
        return "B.touch"


class C(A, B):
    def who(self) -> str:
        return "C:" + super().who()

    def touch(self) -> str:
        return super().touch()


obj = C()
print(obj.who())
print(obj.touch())
print(super(C, obj).who())


class Base:
    @classmethod
    def kind(cls) -> str:
        return "Base"


class Child(Base):
    @classmethod
    def kind(cls) -> str:
        return "Child:" + super().kind()


print(Child.kind())
