"""Purpose: differential coverage for manual exception context mutation."""


def main():
    try:
        raise ValueError("inner")
    except ValueError as exc:
        outer = RuntimeError("outer")
        outer.__cause__ = exc
        outer.__suppress_context__ = True
        try:
            raise outer
        except Exception as err:
            print("cause", type(err.__cause__).__name__)
            print("suppress", err.__suppress_context__)


if __name__ == "__main__":
    main()
