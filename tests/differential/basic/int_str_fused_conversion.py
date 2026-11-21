class LoudValueError(ValueError):
    def __str__(self):
        return "custom-13"


class NumericString(str):
    def __int__(self):
        return 88


class NumericValueError(ValueError):
    def __str__(self):
        return NumericString("37")


def show(label, fn):
    try:
        print(label, fn())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


show("value-error-int", lambda: int(str(ValueError(17))))
show("value-error-negative", lambda: int(str(ValueError(-17))))
show("value-error-bool", lambda: int(str(ValueError(True))))
show("key-error-int", lambda: int(str(KeyError(17))))
show("custom-str", lambda: int(str(LoudValueError(17))))
show("str-subclass-number-slot", lambda: int(str(NumericValueError())))
show("str-subclass-explicit-base", lambda: int(str(NumericValueError()), 10))
show("unicode-decimal", lambda: int(str(ValueError("\u2003-１２_٣\u00a0"))))
show("plain-base", lambda: int(str(10), 2))
