"""Purpose: differential coverage for chained assignment order."""

events = []


def rhs():
    events.append("rhs")
    return 10


class Recorder:
    def __init__(self, label):
        object.__setattr__(self, "label", label)

    def __setattr__(self, name, value):
        events.append(f"{self.label}.{name}={value}")
        object.__setattr__(self, name, value)


a = Recorder("a")
b = Recorder("b")

a.x = b.y = rhs()
print("events", events)
print("values", a.x, b.y)
