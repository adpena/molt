"""Purpose: differential coverage for __prepare__ mapping behavior."""

events = []


class LogDict(dict):
    def __setitem__(self, key, value):
        events.append(key)
        super().__setitem__(key, value)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        return LogDict()


class Box(metaclass=Meta):
    z = 1
    a = 2
    b = 3

    def method(self):
        return "ok"


print("keys", list(Box.__dict__.keys())[:5])
print("events", events[:5])
