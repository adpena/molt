"""Purpose: differential coverage for augassign attribute evaluation order."""

log = []


class Value:
    def __init__(self, value):
        self.value = value

    def __iadd__(self, other):
        log.append("iadd")
        self.value += other
        return self


class Slot:
    def __get__(self, obj, owner):
        log.append("get")
        return obj._value

    def __set__(self, obj, value):
        log.append("set")
        obj._value = value


class Box:
    slot = Slot()

    def __init__(self):
        self._value = Value(1)


if __name__ == "__main__":
    box = Box()
    box.slot += 2
    print("log", log)
    print("value", box._value.value)
