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

    # The unbound slot wrapper takes its receiver first and skips the
    # metaclass's own __call__.
    for label, call in (
        ("unbound", lambda: type.__call__(Demo, 5).value),
        ("unbound-builtin", lambda: type.__call__(int, "7")),
        ("unbound-type", lambda: type.__call__(type, 3)),
        ("unbound-keyword", lambda: type.__call__(Demo, value=9).value),
        ("unbound-missing", lambda: type.__call__()),
        ("unbound-non-type", lambda: type.__call__(1)),
    ):
        try:
            print(label, repr(call()))
        except TypeError as error:
            print(label, "TypeError", error)
    print("log-after", log)
