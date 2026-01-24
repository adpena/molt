"""Purpose: differential coverage for metaclass __call__ behavior."""

log = []


class Meta(type):
    def __call__(cls, *args, **kwargs):
        log.append(("call", args))
        return super().__call__(*args, **kwargs)


class Demo(metaclass=Meta):
    def __init__(self, value):
        self.value = value


if __name__ == "__main__":
    demo = Demo(3)
    print("value", demo.value)
    print("log", log)
