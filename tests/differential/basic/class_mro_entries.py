"""Purpose: differential coverage for __mro_entries__ in base classes."""


class Base:
    def __mro_entries__(self, bases):
        class Replacement:
            def hello(self):
                return "replacement"

        return (Replacement,)


class Child(Base()):
    pass


print("hello", Child().hello())
