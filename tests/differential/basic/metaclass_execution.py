"""Purpose: differential coverage for metaclass execution semantics."""


events = []


class Meta(type):
    def __new__(mcls, name, bases, namespace):
        events.append(("new", name, tuple(base.__name__ for base in bases)))
        return super().__new__(mcls, name, bases, namespace)

    def __init__(cls, name, bases, namespace):
        events.append(("init", name, "value" in namespace))
        super().__init__(name, bases, namespace)


class Base:
    pass


try:
    class Demo(Base, metaclass=Meta):
        value = 42
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    print(Demo.value, isinstance(Demo, Meta), events)
