"""Purpose: differential coverage for chained assignment with descriptors."""

log = []


class Slot:
    def __set__(self, obj, value):
        log.append(("set", value))
        obj._value = value


class Demo:
    slot = Slot()


if __name__ == "__main__":
    demo = Demo()
    demo.slot = other = 5
    print("value", demo._value, other)
    print("log", log)
