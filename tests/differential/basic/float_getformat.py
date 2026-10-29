"""Purpose: differential coverage for float.__getformat__ (HF-566).

CPython's `Lib/test/support/__init__.py` calls `float.__getformat__("double")`
at import, so every regrtest module depends on it. The classmethod reports the
layout of the C `double` and `float` types and checks its argument through
Argument Clinic: str only, strict UTF-8, no embedded NUL, then the two names.
"""


def show(label, func):
    try:
        value = func()
    except BaseException as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "OK", repr(value))


class FloatSubclass(float):
    pass


class StrSubclass(str):
    pass


class Plain:
    pass


show("double", lambda: float.__getformat__("double"))
show("float", lambda: float.__getformat__("float"))
show("instance receiver", lambda: (1.5).__getformat__("double"))
show("subclass receiver", lambda: FloatSubclass.__getformat__("float"))
show("str subclass argument", lambda: float.__getformat__(StrSubclass("double")))
show("same answer for both", lambda: float.__getformat__("double") == float.__getformat__("float"))

show("unknown name", lambda: float.__getformat__("long double"))
show("empty name", lambda: float.__getformat__(""))
show("case differs", lambda: float.__getformat__("Double"))
show("str subclass unknown", lambda: float.__getformat__(StrSubclass("x")))
show("embedded NUL", lambda: float.__getformat__("dou\0ble"))
show("trailing NUL", lambda: float.__getformat__("double\0"))
show("lone surrogate", lambda: float.__getformat__("\ud800"))

show("int argument", lambda: float.__getformat__(1))
show("float argument", lambda: float.__getformat__(1.5))
show("bytes argument", lambda: float.__getformat__(b"double"))
show("None argument", lambda: float.__getformat__(None))
show("user class argument", lambda: float.__getformat__(Plain()))

bound = float.__getformat__
print("bound type", type(bound).__name__)
print("bound self is float", bound.__self__ is float)
print("bound name", bound.__name__, bound.__qualname__)
print("text signature", bound.__text_signature__)
print("descriptor type", type(float.__dict__["__getformat__"]).__name__)
print("listed by dir", "__getformat__" in dir(float))
print("no __setformat__", not hasattr(float, "__setformat__"))
