"""Purpose: differential coverage for metaclass __prepare__ ordering."""

events = []


class LogDict(dict):
    def __setitem__(self, key, value):
        events.append(key)
        super().__setitem__(key, value)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        events.append("prepare")
        return LogDict()


class Box(metaclass=Meta):
    a = 1
    b = 2

    def method(self):
        return "ok"


print("events", events)
