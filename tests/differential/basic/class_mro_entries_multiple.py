"""Purpose: differential coverage for __mro_entries__ with multiple bases."""


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


class Combined(Left(), Right()):
    pass


print("left", Combined().left())
print("right", Combined().right())
