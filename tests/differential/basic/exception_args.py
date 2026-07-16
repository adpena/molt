"""Purpose: differential coverage for exception args."""


def _report(label, fn):
    try:
        fn()
    except Exception as exc:
        print(
            label,
            type(exc).__name__,
            type(exc.args).__name__,
            exc.args,
            exc.__class__ is type(exc),
            exc.__class__ is ValueError,
            str(exc),
        )


def _raise_instance():
    raise ValueError("boom", 3)


def _raise_class():
    raise ValueError


def _raise_call():
    raise ValueError("x")


def _report_direct_instance():
    exc = ValueError(7)
    print(
        "direct",
        type(exc.args).__name__,
        exc.args,
        exc.args is exc.args,
        str(exc),
        repr(exc),
    )


def _raise_non_exception():
    try:
        raise 1
    except Exception as exc:
        print("non-exc", type(exc).__name__, str(exc))


def _constructor_calling_convention():
    for cls in (Exception, OSError, ValueError):
        try:
            cls(unexpected=1)
        except TypeError as exc:
            print("keyword-rejected", cls.__name__, str(exc))

    class CustomNew(Exception):
        def __new__(cls, *args, **kwargs):
            return super().__new__(cls)

    value = CustomNew(1, 2)
    print("custom-new-inherited-init", value.args)

    class KeywordInit(Exception):
        def __init__(self, *, value):
            self.value = value

    value = KeywordInit(value=7)
    print("inherited-new-custom-init", value.value, value.args)


def main():
    _report("instance", _raise_instance)
    _report("class", _raise_class)
    _report("call", _raise_call)
    _report_direct_instance()
    _raise_non_exception()
    _constructor_calling_convention()


if __name__ == "__main__":
    main()
