"""Purpose: differential coverage for function definition evaluation order."""

events = []


def mark(label):
    events.append(label)
    return label


def make_default():
    def inner(a=mark("a"), b=mark("b")):
        return a, b

    return inner


func = make_default()
print("events", events)
print("call", func())


def annotated(x: mark("ann")) -> mark("ret"):
    return x

print("ann_events", events)
print("annotations", annotated.__annotations__)
