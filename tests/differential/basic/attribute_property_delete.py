"""Purpose: differential coverage for property deleter behavior."""

class Demo:
    def __init__(self):
        self._value = 1

    @property
    def value(self):
        return self._value

    @value.deleter
    def value(self):
        del self._value


if __name__ == "__main__":
    demo = Demo()
    print("before", demo.value)
    del demo.value
    print("has", hasattr(demo, "_value"))
