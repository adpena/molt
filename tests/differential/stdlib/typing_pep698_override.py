from typing import override


class Base:
    def method(self):
        return "base"


class Child(Base):
    @override
    def method(self):
        return "child"


c = Child()
print(c.method())
print(hasattr(Child.method, "__override__"))
print(Child.method.__override__)
