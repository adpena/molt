"""Purpose: differential coverage for multi-context with order semantics."""

events = []


class Ctx:
    def __init__(self, label):
        self.label = label

    def __enter__(self):
        events.append(f"enter:{self.label}")
        return self.label

    def __exit__(self, exc_type, exc, tb):
        events.append(f"exit:{self.label}")
        return False


def make(label):
    events.append(f"make:{label}")
    return Ctx(label)


with make("a") as a, make("b") as b:
    events.append(f"body:{a}-{b}")

print("events", events)
