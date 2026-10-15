"""Purpose: staticmethod and classmethod called through an instance match CPython (HF-216).

A static method receives no receiver and a class method receives the class,
including for a subclass instance and inside loops that warm the call site.
"""


class Tools:
    scale = 2

    @staticmethod
    def double(value):
        return value * 2

    @classmethod
    def scaled(cls, value):
        return (cls.__name__, value * cls.scale)

    def plain(self, value):
        return ("plain", value)


class MoreTools(Tools):
    scale = 3


tools = Tools()
more = MoreTools()
print(tools.double(4))
print(tools.scaled(4))
print(more.scaled(4))
print(Tools.double(5), Tools.scaled(5))


def mixed():
    results = []
    for receiver in (tools, more, tools):
        results.append((receiver.double(1), receiver.scaled(1), receiver.plain(1)))
    return results


print(mixed())
