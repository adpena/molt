"""Temporary closure calls preserve captured values and destruction order."""

import weakref


events = []
observers = []


class Capture:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        events.append("capture:" + self.label)


def make(label):
    capture = Capture(label)

    def invoke():
        events.append("body:" + capture.label)
        return capture.label

    observers.append(weakref.ref(invoke, lambda _: events.append("function:" + label)))
    return invoke


# Neither the caller nor an instance keeps a strong reference to the
# function. Its closure tuple must remain alive through the body, and its
# weakref notification must follow the body and precede captured teardown.
print("temporary", make("temporary")())
print("temporary events", events)
events.clear()

# A separate owner postpones both function and environment destruction.
held = make("held")
print("held", held())
print("held events", events)
del held
print("released events", events)
events.clear()


def make_returning_capture(label):
    capture = Capture(label)

    def invoke():
        events.append("body:" + capture.label)
        return capture

    observers.append(weakref.ref(invoke, lambda _: events.append("function:" + label)))
    return invoke


# The result acquires its own reference before either activation owner ends.
result = make_returning_capture("result")()
print("result", result.label, events)
del result
print("result released", events)
print("observers dead", all(observer() is None for observer in observers))
