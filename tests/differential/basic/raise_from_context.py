def raise_from() -> None:
    try:
        raise KeyError("x")
    except Exception as exc:
        raise ValueError("boom") from exc


def raise_context() -> None:
    try:
        raise KeyError("x")
    except Exception:
        try:
            raise RuntimeError("inner")
        except Exception:
            raise ValueError("outer")


try:
    raise_from()
except Exception as exc:
    print(exc.__cause__ is None)
    print(exc.__context__ is None)
    print(exc.__cause__ is exc.__context__)
    print(exc.__suppress_context__ is True)

try:
    raise_context()
except Exception as exc:
    print(exc.__cause__ is None)
    print(exc.__context__ is None)
    print(exc.__cause__ is exc.__context__)
    print(exc.__suppress_context__ is True)
