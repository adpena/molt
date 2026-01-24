"""Purpose: differential coverage for descriptor __delete__ behavior."""

log = []


class Slot:
    def __set__(self, obj, value):
        obj._value = value
        log.append(("set", value))

    def __delete__(self, obj):
        log.append("delete")
        del obj._value


class Demo:
    slot = Slot()


if __name__ == "__main__":
    demo = Demo()
    demo.slot = 5
    print("value", demo._value)
    del demo.slot
    print("has", hasattr(demo, "_value"))
    print("log", log)
