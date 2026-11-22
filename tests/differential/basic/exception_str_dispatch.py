"""Purpose: str() of an exception calls the slot its class resolves, override or builtin."""


class Plain(ValueError):
    pass


class Custom(ValueError):
    def __str__(self):
        return "custom:" + repr(self.args)


class Mixin:
    def __str__(self):
        return "mixin"


class FromMixin(Mixin, ValueError):
    pass


class AfterMixin(ValueError, Mixin):
    pass


class KeyChild(KeyError):
    pass


class Late(ValueError):
    pass


cases = [
    ValueError(),
    ValueError(7),
    ValueError("a", 2),
    Plain(3),
    Custom(4),
    FromMixin(5),
    AfterMixin(6),
    KeyError("k"),
    KeyChild("kc"),
    OSError(2, "missing", "file.txt"),
    UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid start byte"),
    ExceptionGroup("group", [ValueError(1), TypeError(2)]),
    StopIteration(9),
    SystemExit(3),
]
for error in cases:
    print(type(error).__name__, repr(str(error)), f"{error}")

for i in range(4):
    try:
        if i % 2:
            raise Plain(i)
        raise Custom(i)
    except ValueError as error:
        print("loop", type(error).__name__, str(error))

Late.__str__ = lambda self: "patched"
print("late", str(Late(1)))
del Late.__str__
print("restored", str(Late(1)))

total = 0
for i in range(30):
    try:
        if i % 3 == 0:
            raise ValueError(i)
        total += i
    except ValueError as e:
        total += int(str(e))
print("total", total)


class ChangesArgs:
    def __str__(self):
        changed.args = (43,)
        return "29"


changed = ValueError(ChangesArgs())
print("mutated args", int(str(changed)), changed.args)
