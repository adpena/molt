"""Purpose: differential coverage for ExceptionGroup except* semantics."""


def raise_group():
    raise ExceptionGroup("eg", [ValueError("bad"), TypeError("oops")])


try:
    raise_group()
except* ValueError as exc:
    print("value", [type(e).__name__ for e in exc.exceptions])
except* Exception as exc:
    print("rest", [type(e).__name__ for e in exc.exceptions])
