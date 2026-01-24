"""Purpose: differential coverage for unpacking error order and evaluation."""

log = []


class Source:
    def __iter__(self):
        log.append("iter")
        return iter([1])


class Target:
    def __setattr__(self, name, value):
        log.append(("set", name, value))
        super().__setattr__(name, value)


if __name__ == "__main__":
    target = Target()
    try:
        target.x, target.y = Source()
        print("error", "missed")
    except Exception as exc:
        print("error", type(exc).__name__)
    print("log", log)
