"""Purpose: differential coverage for __prepare__ order + __set_name__."""

events = []


class Desc:
    def __init__(self, label):
        self.label = label

    def __set_name__(self, owner, name):
        events.append(f"set_name:{name}:{self.label}")


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
    first = Desc("first")
    second = 2
    third = Desc("third")


print("events", events)
