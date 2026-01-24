"""Purpose: differential coverage for bare raise errors outside except."""


def bare_raise_outside():
    try:
        raise
    except Exception as exc:
        print("bare_outside", type(exc).__name__)


def bare_raise_nested():
    try:
        raise ValueError("boom")
    except Exception:
        try:
            raise
        except Exception as exc:
            print("bare_inside", type(exc).__name__, str(exc))


bare_raise_outside()
bare_raise_nested()
