"""Fixed aggregate construction preserves evaluation and element ownership."""

events = []


class Item:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append("drop " + self.name)


def value(index):
    events.append(index)
    return index


def fail():
    events.append("raise")
    raise ValueError("element")


def nested(depth):
    if depth == 0:
        return (), []
    return (depth, nested(depth - 1)), [depth, depth + 1]


item = Item("tuple")
pair = (item, item)
del item
print("tuple owners", pair[0] is pair[1], events)
del pair
print("tuple released", events)
events.clear()

item = Item("list")
pair = [item, item]
del item
pair.pop()
print("list owner", pair[0].name, events)
del pair
print("list released", events)
events.clear()

values = (
    value(0),
    value(1),
    value(2),
    value(3),
    value(4),
    value(5),
    value(6),
    value(7),
    value(8),
    value(9),
    value(10),
    value(11),
    value(12),
    value(13),
    value(14),
    value(15),
    value(16),
    value(17),
)
print("wide tuple", len(values), values[0], values[-1], sum(values), events)
events.clear()
values = [value(2), value(1), value(0)]
values.append(3)
print("list mutation", values, events)
events.clear()

try:
    values = (value(1), fail(), value(2))
except ValueError as exc:
    print("tuple failure", str(exc), events, values)
events.clear()
try:
    values = [value(3), fail(), value(4)]
except ValueError as exc:
    print("list failure", str(exc), events, values)

print("recursive construction", nested(3))


def defaults(pos=(1, 2), *, kw=[3, 4]):
    return pos, kw


first = defaults()
first[1].append(5)
print("metadata defaults", defaults(), defaults.__defaults__, defaults.__kwdefaults__)
