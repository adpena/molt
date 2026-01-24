"""Purpose: differential coverage for class body execution semantics."""

events = []


class Recorder:
    def __init__(self, label):
        self.label = label

    def __set_name__(self, owner, name):
        events.append(f"set_name:{owner.__name__}.{name}:{self.label}")


class Base:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        events.append(f"init_subclass:{cls.__name__}")


class Box(Base):
    field = Recorder("field")
    other = Recorder("other")

    def method(self):
        return "ok"


print("events", events)
print("attrs", hasattr(Box, "field"), hasattr(Box, "other"))
