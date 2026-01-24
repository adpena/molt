"""Purpose: differential coverage for __setattr__ return value ignored."""

log = []


class Demo:
    def __setattr__(self, name, value):
        log.append((name, value))
        super().__setattr__(name, value)
        return "ignored"


if __name__ == "__main__":
    demo = Demo()
    demo.value = 3
    print("value", demo.value)
    print("log", log)
