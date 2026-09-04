"""Purpose: a property getter on a module-level class accessed at module
scope must invoke the getter, not return None.
"""


class Temperature:
    def __init__(self, celsius: float) -> None:
        self._celsius = celsius

    @property
    def fahrenheit(self) -> float:
        return self._celsius * 9.0 / 5.0 + 32.0

    @property
    def label(self) -> str:
        return "T=" + str(self._celsius)


# Module-scope construction + property access (the bug site).
t = Temperature(100.0)
print(t.fahrenheit)
print(t.label)

t2 = Temperature(0.0)
print(t2.fahrenheit)
print(t2.label)


# Property with a setter, exercised at module scope.
class Box:
    def __init__(self) -> None:
        self._v = 1

    @property
    def v(self) -> int:
        return self._v * 10

    @v.setter
    def v(self, value: int) -> None:
        self._v = value


b = Box()
print(b.v)
b.v = 5
print(b.v)
