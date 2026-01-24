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


def _raise_non_exception():
    try:
        raise 1
    except Exception as exc:
        print("non-exc", type(exc).__name__, str(exc))


def main():
    _report("instance", _raise_instance)
    _report("class", _raise_class)
    _report("call", _raise_call)
    _raise_non_exception()


if __name__ == "__main__":
    main()
