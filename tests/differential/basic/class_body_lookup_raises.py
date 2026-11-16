"""Purpose: an exception from a prepared namespace's lookup leaves the class body.

A class body runs in its own frame. When the mapping that `__prepare__`
returned raises from `__getitem__` during a name load, the class frame exits
before the enclosing handler runs, so the program continues normally and
later frames see a consistent stack (HF-879).
"""


class Namespace(dict):
    def __getitem__(self, name):
        if name == "broken":
            raise ValueError("mapping-error")
        return super().__getitem__(name)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        return Namespace()


def caught_at_module_level():
    try:

        class Broken(metaclass=Meta):
            observed = broken  # noqa: F821

    except ValueError as error:
        return str(error)
    return "no error"


try:

    class Broken(metaclass=Meta):
        observed = broken  # noqa: F821

except ValueError as error:
    print("module", error)

print("function", caught_at_module_level())

try:

    class Looping(metaclass=Meta):
        seen = []
        for step in range(3):
            seen.append(step)
            if step == 2:
                observed = broken  # noqa: F821

except ValueError as error:
    print("loop", error)


class Fine(metaclass=Meta):
    value = 1
    doubled = value * 2


print("after", Fine.doubled)
