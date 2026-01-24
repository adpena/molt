"""Purpose: differential coverage for __mro_entries__ with multiple adapters and prepare."""

events = []


class Left:
    def __mro_entries__(self, bases):
        class L:
            def left(self):
                return "left"

        return (L,)


class Right:
    def __mro_entries__(self, bases):
        class R:
            def right(self):
                return "right"

        return (R,)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        events.append("prepare")
        return {}


class Combo(Left(), Right(), metaclass=Meta):
    pass


c = Combo()
print("events", events)
print("left", c.left())
print("right", c.right())
