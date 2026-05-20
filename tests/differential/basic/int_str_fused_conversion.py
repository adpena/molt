class LoudValueError(ValueError):
    def __str__(self):
        return "custom-13"


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
show("plain-base", lambda: int(str(10), 2))
