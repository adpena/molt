"""Purpose: differential coverage for default evaluation in class body."""

events = []


def mark(label):
    events.append(label)
    return label


class Box:
    value = mark("class_attr")

    def method(self, arg=mark("default")):
        return arg


print("events", events)
print("method_default", Box().method())
