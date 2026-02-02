"""Purpose: differential coverage for builtin conversion edges."""


class BoolWeird:
    def __bool__(self):
        return 2


class LenWeird:
    def __len__(self):
        return 3.5


def show(label: str, thunk) -> None:
    try:
        print(label, thunk())
    except Exception as exc:
        print(label, type(exc).__name__, exc)


show("bool-weird", lambda: bool(BoolWeird()))
show("bool-len-weird", lambda: bool(LenWeird()))
show("str-bytes", lambda: str(b"hi"))
show("complex-str", lambda: complex("1+2j"))
show("complex-bad", lambda: complex("nope"))
show("complex-two", lambda: complex(1, 2))
