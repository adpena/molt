"""Purpose: a missing method raises AttributeError before its arguments run (HF-216).

CPython's LOAD_ATTR fails first, so a side effect in an argument never
happens. The same holds for a missing zero-argument super() method and for a
receiver whose __getattr__ raises.
"""

events = []


def effect(label):
    events.append(label)
    return label


class Empty:
    pass


try:
    Empty().missing(effect("plain"))
except AttributeError as error:
    print("AttributeError", error)
print(events)


class Base:
    pass


class Child(Base):
    def call_parent(self):
        return super().absent(effect("super"))


try:
    Child().call_parent()
except AttributeError as error:
    print("AttributeError", error)
print(events)


class Refusing:
    def __getattr__(self, name):
        raise KeyError(name)


try:
    Refusing().anything(effect("getattr"))
except KeyError as error:
    print("KeyError", error)
print(events)

try:
    (5).no_such_method(effect("int"))
except AttributeError as error:
    print("AttributeError", error)
print(events)
