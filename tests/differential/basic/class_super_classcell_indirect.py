"""Purpose: differential coverage for __class__ cell via indirect super()."""


def make_method():
    def method(self):
        return super().hello()

    return method


class Base:
    def hello(self):
        return "base"


class Child(Base):
    hello = make_method()


print("hello", Child().hello())
