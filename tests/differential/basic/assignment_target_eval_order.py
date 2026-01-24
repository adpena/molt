"""Purpose: differential coverage for assignment target evaluation order."""

events = []


def rhs_pair():
    events.append("rhs")
    return (1, 2)


class Box:
    def __init__(self, label):
        object.__setattr__(self, "label", label)

    def __setattr__(self, name, value):
        events.append(f"set:{self.label}.{name}={value}")
        object.__setattr__(self, name, value)


class Bag:
    def __init__(self, label):
        self.label = label
        self.data = [None]

    def __setitem__(self, key, value):
        events.append(f"set:{self.label}[{key}]={value}")
        self.data[key] = value


def idx(label):
    events.append(f"idx:{label}")
    return 0


box_a = Box("a")
box_b = Box("b")
box_a.x, box_b.y = rhs_pair()

bag = Bag("bag")
bag[idx("first")], bag[idx("second")] = rhs_pair()

print("events", events)
