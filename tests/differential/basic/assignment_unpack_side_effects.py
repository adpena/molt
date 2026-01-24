"""Purpose: differential coverage for unpacking evaluation order and side effects."""

log = []


class Seq:
    def __iter__(self):
        log.append("iter")
        return iter([1, 2])


class Slot:
    def __set__(self, obj, value):
        log.append(("set", value))
        obj.value = value


class Target:
    slot = Slot()


if __name__ == "__main__":
    target = Target()
    target.slot, other = Seq()
    print("value", target.value, other)
    print("log", log)
