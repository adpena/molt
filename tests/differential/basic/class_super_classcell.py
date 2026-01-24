"""Purpose: differential coverage for __classcell__ and super() in class body."""


class Base:
    def hello(self):
        return "base"


class Child(Base):
    def hello(self):
        return super().hello() + "+child"


print("hello", Child().hello())
