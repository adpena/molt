"""Purpose: differential coverage for __prepare__ ordering with decorators."""

events = []


def deco(fn):
    events.append(f"deco:{fn.__name__}")
    return fn


class LogDict(dict):
    def __setitem__(self, key, value):
        events.append(f"set:{key}")
        super().__setitem__(key, value)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        events.append("prepare")
        return LogDict()


class Box(metaclass=Meta):
    @deco
    def method(self):
        return "ok"


print("events", events)
