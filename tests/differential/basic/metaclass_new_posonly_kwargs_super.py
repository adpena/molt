"""Purpose: zero-arg super in metaclass __new__ with pos-only/kwargs."""


class Meta(type):
    def __new__(mcls, name, bases, namespace, /, **kwargs):
        cls = super().__new__(mcls, name, bases, namespace, **kwargs)
        cls.created = True
        return cls


class Box(metaclass=Meta):
    value = 7


print(Box.__name__, Box.value, Box.created)
