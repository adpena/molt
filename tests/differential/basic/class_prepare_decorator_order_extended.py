"""Purpose: differential coverage for decorator order under __prepare__."""

events = []


def deco(label):
    def wrap(fn):
        events.append(f"deco:{label}:{fn.__name__}")
        return fn

    return wrap


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
    @deco("one")
    @deco("two")
    def method(self):
        return "ok"


print("events", events)
