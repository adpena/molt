"""Purpose: an instance attribute set by an argument does not redirect a loaded method call (HF-216).

The method load reads the class function before the argument stores an
instance attribute of the same name; the instance attribute shadows only later
loads. A slotted class, a class whose instances carry the attribute from
construction, and a plain function stored on the instance are checked.
"""


class Widget:
    def render(self, value):
        return ("method", value)


def shadow(widget, value):
    widget.render = lambda value: ("instance", value)
    return value


widget = Widget()
print(widget.render(shadow(widget, 1)))
print(widget.render(2))
del widget.render
print(widget.render(3))


class Preset:
    def __init__(self):
        self.action = lambda value: ("own", value)

    def action(self, value):
        return ("class", value)


preset = Preset()
print(preset.action(1))


class Slotted:
    __slots__ = ("label",)

    def label_of(self, value):
        return ("slotted", value)


slotted = Slotted()
print(slotted.label_of(4))


def plain(value):
    return ("plain", value)


widget.render = plain
print(widget.render(5))


def warm_then_shadow():
    target = Widget()
    seen = []
    for index in range(4):
        if index == 2:
            target.render = lambda value: ("late", value)
        seen.append(target.render(index))
    return seen


print(warm_then_shadow())
